use std::cmp::Ordering;
use std::sync::Arc;

use num_traits::ToPrimitive;
use plenora_core::arrow::array::{Array, ArrayRef, Float64Array, Int64Array, RecordBatch};
use plenora_core::arrow::schema::DataType;
use serde::Deserialize;

use plenora_core::{PlenoraError, Result};

use crate::{column_index, replace_or_append};

use super::aggregate::default_ddof;
use super::grouping::{build_partitions, scatter_partitions};
use super::sort::{sort, Sort};
use crate::float64_source::{
    intero_in_f64, prendi_righe, tipo_estremo, tipo_somma, valida_valori_numerici, varianza_intera,
    ColonnaEsatta, ColonnaIntera, Float64Source, OrdineNumerico, SommaEsatta,
};

/// Aggregazione di `table.rolling_window` sui valori non nulli della
/// finestra (in JSON in minuscolo).
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RollingKind {
    /// Somma: esatta ed `Int64` sul dominio intero (errore oltre `i64`),
    /// altrimenti in ordine di riga (da `-0.0`), e un NaN la rende NaN.
    Sum,
    /// Somma divisa per il numero di valori (dalla somma esatta sul dominio
    /// intero).
    Mean,
    /// Minimo: su interi e `Decimal128` la cella minima nel tipo
    /// d'ingresso; altrimenti `f64::min`, e i NaN si ignorano salvo che
    /// siano tutti NaN.
    Min,
    /// Massimo, come [`RollingKind::Min`].
    Max,
    /// Deviazione standard in due passate, divisore `valori - ddof`; null
    /// con `valori <= ddof`.
    Stddev,
}

const fn default_min_periods() -> usize {
    1
}

/// Config di `table.rolling_window`. Campi sconosciuti rifiutati.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RollingWindow {
    /// Colonna numerica aggregata, letta come `f64`.
    pub column: String,
    /// Aggregazione della finestra.
    pub function: RollingKind,
    /// Colonna di partizione, letta come testo (il null e' una partizione);
    /// assente, una partizione sola.
    #[serde(default, deserialize_with = "crate::mai_null")]
    pub group_by: Option<String>,
    /// Colonna ordinabile: le righe si ordinano in ascendente su di essa
    /// (sort stabile, null in coda) prima del calcolo, e l'uscita resta in
    /// quell'ordine.
    #[serde(default, deserialize_with = "crate::mai_null")]
    pub order_column: Option<String>,
    /// Righe della finestra, corrente compresa (almeno 1): la finestra si
    /// misura in righe, e una cella nulla occupa il suo posto.
    pub window: usize,
    /// Valori non nulli minimi per un risultato (default 1, al piu'
    /// `window`); sotto, null.
    #[serde(default = "default_min_periods")]
    pub min_periods: usize,
    /// Gradi di liberta' di `stddev` (assente: 1).
    #[serde(default, deserialize_with = "crate::mai_null")]
    pub ddof: Option<usize>,
    /// Colonna d'uscita, nullabile, del tipo di [`tipo_uscita_rolling`]; se
    /// esiste gia' si sostituisce al suo posto.
    pub output_column: String,
}

impl RollingWindow {
    /// Gradi di liberta' di `stddev`.
    #[must_use]
    pub fn ddof(&self) -> usize {
        self.ddof.unwrap_or(default_ddof())
    }

    /// `ddof` vale solo per `stddev`: con un'altra funzione si rifiuta
    /// invece di essere ignorato. La chiamano il kernel e l'analisi.
    ///
    /// # Errors
    ///
    /// `InvalidPlan` se `ddof` accompagna una funzione diversa da `stddev`.
    pub fn verifica_parametri(&self) -> Result<()> {
        if self.ddof.is_some() && !matches!(self.function, RollingKind::Stddev) {
            return Err(PlenoraError::InvalidPlan(
                "ddof ammesso solo con function=stddev".into(),
            ));
        }
        Ok(())
    }
}

/// Un risultato non finito calcolato da valori tutti finiti e' un overflow
/// dell'aritmetica `f64` (`1e308 + 1e308`): si rifiuta invece di pubblicare
/// un infinito. Un `NaN` o un infinito gia' nei valori si propaga, come
/// dichiarano le schede.
fn senza_overflow(risultato: Option<f64>, valori_finiti: bool, op: &str) -> Result<Option<f64>> {
    match risultato {
        Some(valore) if valori_finiti && !valore.is_finite() => Err(PlenoraError::DataMapping(
            format!("{op}: risultato non finito da valori finiti (overflow di f64)"),
        )),
        altro => Ok(altro),
    }
}

/// Che cosa serve a una variante per essere calcolata.
///
/// Un `match` **esaustivo**: una `WindowKind` senza strategia non compila.
/// E' l'unica autorita' sulla classificazione, interrogata anche
/// dall'analizzatore.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Strategia {
    /// Rende un valore numerico: legge la colonna come `f64`, arrotondando.
    Valore,
    /// Ordina e confronta: legge la colonna nel dominio originale.
    Rango,
    /// Dipende dalla sola posizione nella partizione.
    Posizione,
}

#[must_use]
pub const fn strategia(funzione: &WindowKind) -> Strategia {
    match funzione {
        WindowKind::Cumsum
        | WindowKind::RunningMean
        | WindowKind::Lag
        | WindowKind::Lead
        | WindowKind::PctChange => Strategia::Valore,
        WindowKind::Rank
        | WindowKind::DenseRank
        | WindowKind::PercentRank
        | WindowKind::CumeDist => Strategia::Rango,
        WindowKind::Cumcount | WindowKind::Ntile => Strategia::Posizione,
    }
}

/// Tipo della colonna d'uscita di `rolling_window`.
///
/// Autorita' di kernel e analisi, sulla colonna di tipo `data_type`: `sum`
/// e' `Int64` sul dominio intero, `min`/`max` tengono il tipo d'ingresso su interi e decimali, il
/// resto e' `Float64`.
#[must_use]
pub fn tipo_uscita_rolling(funzione: RollingKind, data_type: &DataType) -> DataType {
    match funzione {
        RollingKind::Sum => tipo_somma(data_type),
        RollingKind::Min | RollingKind::Max => tipo_estremo(data_type),
        RollingKind::Mean | RollingKind::Stddev => DataType::Float64,
    }
}

/// Tipo della colonna d'uscita di `window_function`.
///
/// Autorita' di kernel e analisi, sulla colonna di tipo `data_type`: `lag`
/// e `lead` spostano la
/// cella e tengono il tipo d'ingresso, `cumsum` e' `Int64` sul dominio
/// intero, il resto e' `Float64`.
#[must_use]
pub fn tipo_uscita_finestra(funzione: &WindowKind, data_type: &DataType) -> DataType {
    match funzione {
        WindowKind::Lag | WindowKind::Lead => data_type.clone(),
        WindowKind::Cumsum => tipo_somma(data_type),
        WindowKind::Rank
        | WindowKind::DenseRank
        | WindowKind::Cumcount
        | WindowKind::PctChange
        | WindowKind::RunningMean
        | WindowKind::PercentRank
        | WindowKind::CumeDist
        | WindowKind::Ntile => DataType::Float64,
    }
}

/// Confronta due righe della stessa colonna, ricordando l'eventuale guasto.
///
/// `compare_cells_typed` e' fallibile e `sort_by`/`partition_point` non lo
/// sono: l'errore si mette da parte e lo rende il chiamante. Rendere `Equal`
/// e proseguire in silenzio sarebbe fail-open proprio dove si decide un
/// ordine.
fn confronta(
    ordine: &OrdineNumerico<'_>,
    sinistra: usize,
    destra: usize,
    guasto: &mut Option<PlenoraError>,
) -> Ordering {
    match ordine.compare(sinistra, destra) {
        Ok(ordine) => ordine,
        Err(errore) => {
            guasto.get_or_insert(errore);
            Ordering::Equal
        }
    }
}

/// Varianti di rango (`rank`, `dense_rank`, `percent_rank`, `cume_dist`)
/// su una partizione: un valore per posizione di `rows`, `None` sui null.
///
/// Le righe non nulle si ordinano per valore della cella, con lo stesso
/// ordinamento stabile di `confronta`; poi un solo passaggio sulle sequenze
/// di pari merito. Per una sequenza `[inizio, fine]` dell'ordine, `inizio` e'
/// il numero di valori minori e `fine + 1` quello dei valori minori o
/// uguali: sono le due ricerche binarie che si facevano per riga, e che su
/// una partizione grande costavano `2 log n` confronti per riga. Coincidono
/// perche' `compare_cells_typed` e' un preordine totale sui tipi che
/// `OrdineNumerico` ammette (interi, `total_cmp` sui double, decimali per
/// valore).
///
/// # Errors
///
/// Gli errori di `compare_cells_typed`; `Internal` per una variante che
/// non e' di rango.
fn ranghi(
    ordine: &OrdineNumerico<'_>,
    colonna: &ArrayRef,
    rows: &[usize],
    funzione: &WindowKind,
) -> Result<Vec<Option<f64>>> {
    let mut posizioni = (0..rows.len())
        .filter(|posizione| !colonna.is_null(rows[*posizione]))
        .collect::<Vec<_>>();
    let mut guasto = None;
    posizioni
        .sort_by(|sinistra, destra| confronta(ordine, rows[*sinistra], rows[*destra], &mut guasto));
    if let Some(errore) = guasto {
        return Err(errore);
    }
    let totale = posizioni.len();
    let mut uscita = vec![None; rows.len()];
    let mut inizio = 0;
    let mut sequenze = 0_usize;
    while let Some(primo) = posizioni.get(inizio).map(|posizione| rows[*posizione]) {
        let mut fine = inizio;
        while let Some(prossimo) = posizioni.get(fine + 1).map(|posizione| rows[*posizione]) {
            if ordine.compare(primo, prossimo)? != Ordering::Equal {
                break;
            }
            fine += 1;
        }
        sequenze += 1;
        let valore = match funzione {
            WindowKind::Rank => (inizio + fine + 2).to_f64().map(|somma| somma / 2.0),
            WindowKind::DenseRank => sequenze.to_f64(),
            WindowKind::PercentRank => {
                if totale <= 1 {
                    Some(0.0)
                } else {
                    inizio
                        .to_f64()
                        .zip((totale - 1).to_f64())
                        .map(|(numeratore, denominatore)| numeratore / denominatore)
                }
            }
            WindowKind::CumeDist => (fine + 1)
                .to_f64()
                .zip(totale.to_f64())
                .map(|(numeratore, denominatore)| numeratore / denominatore),
            WindowKind::Cumsum
            | WindowKind::Cumcount
            | WindowKind::Lag
            | WindowKind::Lead
            | WindowKind::PctChange
            | WindowKind::RunningMean
            | WindowKind::Ntile => {
                return Err(PlenoraError::Internal(
                    "ranghi chiamato per una variante che non e' di rango".into(),
                ));
            }
        };
        for posizione in &posizioni[inizio..=fine] {
            uscita[*posizione] = valore;
        }
        inizio = fine + 1;
    }
    Ok(uscita)
}

/// `table.rolling_window`: aggregazione mobile di `column`, riga per riga.
///
/// Per ogni riga, l'aggregazione dei valori non nulli di `column` sulle
/// ultime `window` righe della sua partizione (`group_by`), riga corrente
/// compresa, nella colonna `output_column`.
///
/// Con `order_column` le righe si ordinano prima in ascendente su quella
/// colonna ([`sort`]), e l'uscita resta in quell'ordine; senza,
/// ordine d'ingresso.
///
/// Tipo d'uscita: [`tipo_uscita_rolling`]. Sul dominio intero la somma e'
/// esatta (`Int64`) e media e deviazione partono dalla somma esatta;
/// `min`/`max` su interi e decimali scelgono la cella esatta. Negli altri
/// casi il risultato e' un `Float64` per contratto, e un valore oltre la
/// precisione del double arrotonda.
///
/// # Errors
///
/// - `InvalidPlan`: `window` o `min_periods` nulli, `min_periods > window`;
///   `ddof` con una funzione diversa da `stddev`;
/// - `DataMapping`: una somma intera oltre la gamma di `Int64`;
/// - `ResourceLimit`: dimensioni/divisori della finestra non rappresentabili
///   come `f64` (dipendono dal numero di righe nella finestra);
/// - `Schema`: colonna `column`, `group_by` o `order_column` assente dallo
///   schema; testo non numerico in `column`; in piu' gli errori di `sort`,
///   `scalar_as_string` (partizioni) e `replace_or_append`.
pub fn rolling_window(batch: &RecordBatch, config: &RollingWindow) -> Result<RecordBatch> {
    config.verifica_parametri()?;
    if config.window == 0 || config.min_periods == 0 || config.min_periods > config.window {
        return Err(PlenoraError::InvalidPlan(
            "rolling_window: finestra non valida".into(),
        ));
    }
    let ordered = if let Some(column) = &config.order_column {
        sort(
            batch,
            &Sort {
                columns: vec![column.clone()],
                ascending: true,
            },
        )?
    } else {
        batch.clone()
    };
    let source = column_index(&ordered, &config.column)?;
    let group = config
        .group_by
        .as_deref()
        .map(|name| column_index(&ordered, name))
        .transpose()?;
    // Partizionamento condiviso con `window_function` (`build_partitions`).
    let partitions = build_partitions(&ordered, group)?;
    let colonna = ordered.column(source);
    if matches!(config.function, RollingKind::Sum) {
        crate::float64_source::verifica_somma(colonna.data_type())?;
    }
    let tipo = tipo_uscita_rolling(config.function, colonna.data_type());
    if let Some(uscita) = rolling_esatto(&ordered, &partitions, colonna, config)? {
        return replace_or_append(&ordered, &config.output_column, tipo, true, uscita);
    }
    let numbers = Float64Source::new(ordered.column(source));
    let compute = |rows: &[usize]| -> Result<Vec<Option<f64>>> {
        let numbers = rows
            .iter()
            .map(|row| numbers.value(*row))
            .collect::<Result<Vec<_>>>()?;
        let track_extrema = matches!(config.function, RollingKind::Min | RollingKind::Max);
        let mut values = Vec::with_capacity(rows.len());
        for position in 0..rows.len() {
            let start = (position + 1).saturating_sub(config.window);
            let window = &numbers[start..=position];
            // Aggregazione della finestra senza allocazioni: una passata per
            // conteggio/somma/estremi (due per stddev), replicando ESATTAMENTE
            // le riduzioni originali sul `Vec` ricostruito a ogni riga:
            // - `Iterator::sum::<f64>` parte da -0.0 (sum([-0.0]) = -0.0);
            // - `reduce(f64::min/max)` parte dal primo elemento (finestra di
            //   solo NaN -> NaN, non +/-inf).
            let mut count = 0_usize;
            let mut sum = -0.0_f64;
            let mut minimum: Option<f64> = None;
            let mut maximum: Option<f64> = None;
            for value in window.iter().flatten() {
                count += 1;
                sum += value;
                if track_extrema {
                    minimum = Some(minimum.map_or(*value, |min| f64::min(min, *value)));
                    maximum = Some(maximum.map_or(*value, |max| f64::max(max, *value)));
                }
            }
            if count < config.min_periods {
                values.push(None);
                continue;
            }
            let finiti = window.iter().flatten().all(|value| value.is_finite());
            let risultato = match config.function {
                RollingKind::Sum => Some(sum),
                RollingKind::Mean => count.to_f64().map(|length| sum / length),
                RollingKind::Min => minimum,
                RollingKind::Max => maximum,
                RollingKind::Stddev if count <= config.ddof() => None,
                RollingKind::Stddev => {
                    let length = count.to_f64().ok_or_else(|| {
                        PlenoraError::ResourceLimit("dimensione rolling non rappresentabile".into())
                    })?;
                    let mean = sum / length;
                    let divisor = (count - config.ddof()).to_f64().ok_or_else(|| {
                        PlenoraError::ResourceLimit("divisore rolling non rappresentabile".into())
                    })?;
                    Some(
                        (window
                            .iter()
                            .flatten()
                            .map(|value| (value - mean).powi(2))
                            .sum::<f64>()
                            / divisor)
                            .sqrt(),
                    )
                }
            };
            values.push(senza_overflow(risultato, finiti, "rolling_window")?);
        }
        Ok(values)
    };
    let mut output = vec![None; ordered.num_rows()];
    scatter_partitions(&ordered, &partitions, &mut output, compute)?;
    replace_or_append(
        &ordered,
        &config.output_column,
        DataType::Float64,
        true,
        Arc::new(Float64Array::from(output)),
    )
}

/// Funzione di `table.window_function` (in JSON in `snake_case`).
///
/// `"dense_rank"`, `"pct_change"`, ...: si calcolano per partizione,
/// nell'ordine delle righe; il tipo d'uscita, nullabile, e' quello di
/// [`tipo_uscita_finestra`].
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WindowKind {
    /// Rango del valore fra i non nulli, da 1; a pari merito la media delle
    /// posizioni.
    Rank,
    /// 1, 2, 3... sui valori distinti.
    DenseRank,
    /// Somma cumulata dei valori non nulli; null sulle righe nulle. Esatta
    /// ed `Int64` sul dominio intero (errore oltre `i64`).
    Cumsum,
    /// Posizione della riga nella partizione, da 0.
    Cumcount,
    /// La cella `offset` righe prima, nel tipo d'ingresso.
    Lag,
    /// La cella `offset` righe dopo, nel tipo d'ingresso.
    Lead,
    /// `(corrente - precedente) / precedente` sulla riga subito prima; null
    /// senza precedente, con precedente nullo o zero, o corrente nullo.
    PctChange,
    /// Media dei valori non nulli fin qui (dalla somma esatta sul dominio
    /// intero); null sulle righe nulle.
    RunningMean,
    /// Valori minori diviso (valori non nulli - 1); 0 con un solo valore.
    PercentRank,
    /// Valori minori o uguali diviso valori non nulli.
    CumeDist,
    /// `posizione * min(buckets, righe) / righe + 1` in divisione intera,
    /// posizione da 0.
    Ntile,
}
const fn default_window() -> WindowKind {
    WindowKind::Rank
}
const fn default_offset() -> usize {
    1
}

/// Config di `table.window_function`. Campi sconosciuti rifiutati.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WindowFunction {
    /// Colonna numerica su cui si calcola (non `Utf8` per i ranghi).
    pub column: String,
    /// Funzione (default [`WindowKind::Rank`]).
    #[serde(default = "default_window")]
    pub function: WindowKind,
    /// Colonna di partizione, letta come testo (il null e' una partizione);
    /// assente, una partizione sola.
    #[serde(default, deserialize_with = "crate::mai_null")]
    pub group_by: Option<String>,
    /// Colonna ordinabile: le righe si ordinano in ascendente su di essa
    /// (sort stabile, null in coda) prima del calcolo, e l'uscita resta in
    /// quell'ordine.
    #[serde(default, deserialize_with = "crate::mai_null")]
    pub order_column: Option<String>,
    /// Distanza di `lag` e `lead` (assente: 1).
    #[serde(default, deserialize_with = "crate::mai_null")]
    pub offset: Option<usize>,
    /// Gruppi di `ntile`: obbligatorio e positivo con `ntile`, rifiutato
    /// con le altre funzioni.
    #[serde(default, deserialize_with = "crate::mai_null")]
    pub buckets: Option<usize>,
    /// Colonna d'uscita (assente: `<column>_<funzione>`); se esiste gia' si
    /// sostituisce al suo posto.
    #[serde(default, deserialize_with = "crate::mai_null")]
    pub output_column: Option<String>,
}

impl WindowFunction {
    /// Distanza di `lag` e `lead`.
    #[must_use]
    pub fn offset(&self) -> usize {
        self.offset.unwrap_or(default_offset())
    }

    /// `offset` vale solo per `lag` e `lead`, e deve essere positivo: con
    /// un'altra funzione si rifiuta invece di essere ignorato. La chiamano
    /// il kernel e l'analisi.
    ///
    /// # Errors
    ///
    /// `InvalidPlan` per `offset` nullo o scritto per un'altra funzione.
    pub fn verifica_offset(&self) -> Result<()> {
        match self.offset {
            Some(0) => Err(PlenoraError::InvalidPlan(
                "offset deve essere positivo".into(),
            )),
            Some(_) if !matches!(self.function, WindowKind::Lag | WindowKind::Lead) => Err(
                PlenoraError::InvalidPlan("offset ammesso solo con function=lag o lead".into()),
            ),
            _ => Ok(()),
        }
    }
}

#[allow(clippy::too_many_lines)] // Le varianti condividono partizioni, ordine e una sola passata d'uscita.
/// `table.window_function`: funzione finestra (`rank`, `lag`, `ntile`, ...)
/// su `column`, per partizione di `group_by`, nella colonna `output_column`.
///
/// Con `order_column` le righe si ordinano prima in ascendente su quella
/// colonna ([`sort`]), e l'uscita resta in quell'ordine; senza,
/// ordine d'ingresso. Le partizioni si formano sul testo di `group_by`.
///
/// # Errors
///
/// - `InvalidPlan`: `offset` nullo o fuori da `lag`/`lead`; `ntile` senza
///   `buckets` maggiore di zero;
///   `buckets` specificato per una funzione diversa da `ntile`;
/// - `Schema`: colonna `column`, `group_by` o `order_column` assente dallo
///   schema; `column` fuori dal dominio numerico, `Utf8` con una funzione
///   di rango, testo non numerico; in piu' gli errori di `sort`,
///   `scalar_as_string` (partizioni) e `replace_or_append`;
/// - `DataMapping`: una somma cumulata intera oltre la gamma di `Int64`.
///
/// Tipo d'uscita: [`tipo_uscita_finestra`]. `lag` e `lead` spostano la
/// cella senza convertirla; sul dominio intero `cumsum` e' esatta (`Int64`),
/// `running_mean` e `pct_change` partono da somme e differenze esatte. Negli
/// altri casi le varianti di **valore** rendono un `Float64` per contratto,
/// e un valore oltre la precisione del double arrotonda.
///
/// Le varianti di **rango** (`rank`, `dense_rank`, `percent_rank`,
/// `cume_dist`) non convertono: confrontano il dominio originale come il
/// sort, perche' l'arrotondamento renderebbe pari merito valori distinti.
/// Per la stessa ragione **non accettano colonne `Utf8`**.
///
/// `cumcount` e `ntile` dipendono dalla posizione, ma il contratto numerico
/// della colonna vale anche per loro.
pub fn window_function(batch: &RecordBatch, config: &WindowFunction) -> Result<RecordBatch> {
    config.verifica_offset()?;
    if matches!(config.function, WindowKind::Ntile) {
        if config.buckets.is_none_or(|buckets| buckets == 0) {
            return Err(PlenoraError::InvalidPlan(
                "ntile richiede buckets maggiore di zero".into(),
            ));
        }
    } else if config.buckets.is_some() {
        return Err(PlenoraError::InvalidPlan(
            "buckets e' ammesso solo per ntile".into(),
        ));
    }
    let source_index = column_index(batch, &config.column)?;
    let ordered = if let Some(column) = &config.order_column {
        sort(
            batch,
            &Sort {
                columns: vec![column.clone()],
                ascending: true,
            },
        )?
    } else {
        batch.clone()
    };
    let group_index = config
        .group_by
        .as_deref()
        .map(|name| column_index(&ordered, name))
        .transpose()?;
    // Partizionamento condiviso con `rolling_window`.
    let partitions = build_partitions(&ordered, group_index)?;
    let colonna = ordered.column(source_index);
    let name = config
        .output_column
        .clone()
        .unwrap_or_else(|| format!("{}_{}", config.column, suffisso(&config.function)));
    if matches!(config.function, WindowKind::Cumsum) {
        crate::float64_source::verifica_somma(colonna.data_type())?;
    }
    let tipo = tipo_uscita_finestra(&config.function, colonna.data_type());
    // `lag`/`lead` e, sul dominio intero, le varianti di somma: percorsi
    // esatti, nel tipo di `tipo_uscita_finestra`.
    if let Some(uscita) = finestra_esatta(&ordered, &partitions, colonna, config)? {
        return replace_or_append(&ordered, &name, tipo, true, uscita);
    }
    // Le varianti di VALORE leggono `Float64` (arrotondamento dichiarato),
    // quelle di RANGO il dominio originale (vedi il doc sopra). Il dominio
    // numerico si valida sempre, anche per le varianti di posizione.
    let strategia = strategia(&config.function);
    let source = (strategia == Strategia::Valore).then(|| Float64Source::new(colonna));
    let ordine = match strategia {
        Strategia::Rango => Some(OrdineNumerico::new(colonna)?),
        Strategia::Valore => None,
        // Non legge i valori, ma il contratto della colonna vale lo stesso:
        // una colonna dichiarata numerica e piena di parole e' un ingresso
        // invalido a prescindere da cosa il kernel ne fa.
        Strategia::Posizione => {
            valida_valori_numerici(colonna)?;
            None
        }
    };
    let compute = |rows: &[usize]| -> Result<Vec<Option<f64>>> {
        if let Some(ordine) = &ordine {
            return ranghi(ordine, colonna, rows, &config.function);
        }
        let numbers = match &source {
            Some(source) => rows
                .iter()
                .map(|row| source.value(*row))
                .collect::<Result<Vec<_>>>()?,
            None => Vec::new(),
        };
        let mut sum = 0.0;
        let mut count = 0.0_f64;
        // Tutti i valori letti fin qui finiti: un risultato non finito e'
        // allora un overflow (`senza_overflow`).
        let mut finiti = true;
        let mut values = Vec::with_capacity(rows.len());
        for position in 0..rows.len() {
            if let Some(value) = numbers.get(position).copied().flatten() {
                finiti &= value.is_finite();
            }
            let finiti_qui = match config.function {
                WindowKind::PctChange => {
                    position
                        .checked_sub(1)
                        .and_then(|previous| numbers.get(previous).copied().flatten())
                        .is_none_or(f64::is_finite)
                        && numbers
                            .get(position)
                            .copied()
                            .flatten()
                            .is_none_or(f64::is_finite)
                }
                WindowKind::Cumsum | WindowKind::RunningMean => finiti,
                // Copie di un valore (`lag`, `lead`) e conteggi: nessuna
                // aritmetica che possa traboccare.
                _ => false,
            };
            let risultato = match config.function {
                WindowKind::Cumcount => position.to_f64(),
                WindowKind::Cumsum => numbers[position].map(|value| {
                    sum += value;
                    sum
                }),
                WindowKind::RunningMean => numbers[position].map(|value| {
                    sum += value;
                    count += 1.0;
                    sum / count
                }),
                WindowKind::Lag => position
                    .checked_sub(config.offset())
                    .and_then(|other| numbers[other]),
                WindowKind::Lead => numbers.get(position + config.offset()).copied().flatten(),
                WindowKind::PctChange => position
                    .checked_sub(1)
                    .and_then(|previous| numbers[previous])
                    .and_then(|previous| {
                        numbers[position]
                            .filter(|_| previous != 0.0)
                            .map(|current| (current - previous) / previous)
                    }),
                // Le varianti di rango escono sopra, da `ranghi`: `ordine` c'e'
                // se e solo se la strategia e' di rango.
                WindowKind::Rank
                | WindowKind::DenseRank
                | WindowKind::PercentRank
                | WindowKind::CumeDist => {
                    return Err(PlenoraError::Internal(
                        "variante di rango senza ordine numerico".into(),
                    ));
                }
                WindowKind::Ntile => {
                    let buckets = config.buckets.unwrap_or(1);
                    let effective = buckets.min(rows.len());
                    position
                        .checked_mul(effective)
                        .and_then(|value| value.checked_div(rows.len()))
                        .and_then(|value| (value + 1).to_f64())
                }
            };
            values.push(senza_overflow(risultato, finiti_qui, "window_function")?);
        }
        Ok(values)
    };
    let mut output = vec![None; ordered.num_rows()];
    scatter_partitions(&ordered, &partitions, &mut output, compute)?;
    replace_or_append(
        &ordered,
        &name,
        DataType::Float64,
        true,
        Arc::new(Float64Array::from(output)),
    )
}

/// Il suffisso del nome d'uscita di default (`<column>_<suffisso>`).
#[must_use]
pub const fn suffisso(funzione: &WindowKind) -> &'static str {
    match funzione {
        WindowKind::Rank => "rank",
        WindowKind::DenseRank => "dense_rank",
        WindowKind::Cumsum => "cumsum",
        WindowKind::Cumcount => "cumcount",
        WindowKind::Lag => "lag",
        WindowKind::Lead => "lead",
        WindowKind::PctChange => "pct_change",
        WindowKind::RunningMean => "running_mean",
        WindowKind::PercentRank => "percent_rank",
        WindowKind::CumeDist => "cume_dist",
        WindowKind::Ntile => "ntile",
    }
}

type Partizioni<'a> = [(Option<std::borrow::Cow<'a, str>>, Vec<usize>)];

/// I percorsi esatti di `window_function`: `lag` e `lead` su ogni tipo
/// (spostano la riga, `take` tiene il tipo), e sul dominio intero `cumsum`
/// (somma `i128`, `Int64`), `running_mean` e `pct_change` (somme e
/// differenze esatte, poi `Float64`). `None` per le altre combinazioni, che
/// restano sul percorso `f64` e dei ranghi.
///
/// Sono il valore corretto anche dove il vecchio percorso in `f64`
/// arrotondava a ogni passo (somme parziali oltre 2^53).
///
/// # Errors
///
/// `Schema` per un testo non numerico (contratto della colonna, anche se
/// `lag`/`lead` non leggono il valore); `DataMapping` per una somma oltre
/// `Int64`; gli errori di `take`.
fn finestra_esatta(
    ordered: &RecordBatch,
    partitions: &Partizioni<'_>,
    colonna: &ArrayRef,
    config: &WindowFunction,
) -> Result<Option<ArrayRef>> {
    if matches!(config.function, WindowKind::Lag | WindowKind::Lead) {
        valida_valori_numerici(colonna)?;
        let offset = config.offset();
        let lag = matches!(config.function, WindowKind::Lag);
        let mut righe = vec![None; ordered.num_rows()];
        scatter_partitions(ordered, partitions, &mut righe, |rows| {
            Ok((0..rows.len())
                .map(|position| {
                    let altra = if lag {
                        position.checked_sub(offset)
                    } else {
                        position
                            .checked_add(offset)
                            .filter(|altra| *altra < rows.len())
                    };
                    altra.map(|altra| rows[altra])
                })
                .collect())
        })?;
        return prendi_righe(colonna, &righe).map(Some);
    }
    let Some(interi) = ColonnaIntera::new(colonna) else {
        return Ok(None);
    };
    match config.function {
        WindowKind::Cumsum => {
            let mut output = vec![None; ordered.num_rows()];
            scatter_partitions(ordered, partitions, &mut output, |rows| {
                let mut somma = SommaEsatta::default();
                rows.iter()
                    .map(|row| {
                        interi
                            .value(*row)
                            .map(|valore| {
                                somma.aggiungi(valore)?;
                                somma.in_int64()
                            })
                            .transpose()
                    })
                    .collect()
            })?;
            Ok(Some(Arc::new(Int64Array::from(output))))
        }
        WindowKind::RunningMean => {
            let mut output = vec![None; ordered.num_rows()];
            scatter_partitions(ordered, partitions, &mut output, |rows| {
                let mut somma = SommaEsatta::default();
                rows.iter()
                    .map(|row| {
                        interi
                            .value(*row)
                            .map(|valore| {
                                somma.aggiungi(valore)?;
                                somma.media().ok_or_else(|| {
                                    PlenoraError::Internal("media senza valori".into())
                                })
                            })
                            .transpose()
                    })
                    .collect()
            })?;
            Ok(Some(Arc::new(Float64Array::from(output))))
        }
        WindowKind::PctChange => {
            let mut output = vec![None; ordered.num_rows()];
            scatter_partitions(ordered, partitions, &mut output, |rows| {
                Ok((0..rows.len())
                    .map(|position| {
                        let precedente = position
                            .checked_sub(1)
                            .and_then(|precedente| interi.value(rows[precedente]))
                            .filter(|precedente| *precedente != 0)?;
                        let corrente = interi.value(rows[position])?;
                        // La differenza di due interi entro `u64`/`i64` sta
                        // in `i128`: esatta, poi un solo arrotondamento.
                        Some(intero_in_f64(corrente - precedente) / intero_in_f64(precedente))
                    })
                    .collect())
            })?;
            Ok(Some(Arc::new(Float64Array::from(output))))
        }
        _ => Ok(None),
    }
}

/// I percorsi esatti di `rolling_window`: sul dominio intero `sum` (somma
/// `i128`, `Int64`), `mean` dalla somma esatta, `stddev` dagli scarti
/// esatti; su interi e
/// decimali `min`/`max` scelgono la cella esatta nel tipo d'ingresso. `None`
/// per le altre combinazioni, che restano sul percorso `f64`.
///
/// # Errors
///
/// `DataMapping` per una somma oltre `Int64`; `ResourceLimit` per divisori
/// non rappresentabili; gli errori di `take`.
fn rolling_esatto(
    ordered: &RecordBatch,
    partitions: &Partizioni<'_>,
    colonna: &ArrayRef,
    config: &RollingWindow,
) -> Result<Option<ArrayRef>> {
    if matches!(config.function, RollingKind::Min | RollingKind::Max) {
        let Some(esatta) = ColonnaEsatta::new(colonna) else {
            return Ok(None);
        };
        let massimo = matches!(config.function, RollingKind::Max);
        let mut righe = vec![None; ordered.num_rows()];
        scatter_partitions(ordered, partitions, &mut righe, |rows| {
            Ok((0..rows.len())
                .map(|position| {
                    let start = (position + 1).saturating_sub(config.window);
                    let mut valori = 0_usize;
                    let mut estremo = None;
                    for row in &rows[start..=position] {
                        if let Some(valore) = esatta.value(*row) {
                            valori += 1;
                            estremo = ColonnaEsatta::aggiorna(estremo, *row, valore, massimo);
                        }
                    }
                    estremo
                        .filter(|_| valori >= config.min_periods)
                        .map(|(riga, _)| riga)
                })
                .collect())
        })?;
        return prendi_righe(colonna, &righe).map(Some);
    }
    let Some(interi) = ColonnaIntera::new(colonna) else {
        return Ok(None);
    };
    let finestra = |rows: &[usize], position: usize| -> Result<Option<(SommaEsatta, Vec<i128>)>> {
        let start = (position + 1).saturating_sub(config.window);
        let valori = rows[start..=position]
            .iter()
            .filter_map(|row| interi.value(*row))
            .collect::<Vec<_>>();
        if valori.len() < config.min_periods {
            return Ok(None);
        }
        let mut somma = SommaEsatta::default();
        for valore in &valori {
            somma.aggiungi(*valore)?;
        }
        Ok(Some((somma, valori)))
    };
    if matches!(config.function, RollingKind::Sum) {
        let mut output = vec![None; ordered.num_rows()];
        scatter_partitions(ordered, partitions, &mut output, |rows| {
            (0..rows.len())
                .map(|position| {
                    finestra(rows, position)?
                        .map(|(somma, _)| somma.in_int64())
                        .transpose()
                })
                .collect()
        })?;
        return Ok(Some(Arc::new(Int64Array::from(output))));
    }
    let mut output = vec![None; ordered.num_rows()];
    scatter_partitions(ordered, partitions, &mut output, |rows| {
        (0..rows.len())
            .map(|position| {
                let Some((somma, valori)) = finestra(rows, position)? else {
                    return Ok(None);
                };
                let Some(media) = somma.media() else {
                    return Ok(None);
                };
                if matches!(config.function, RollingKind::Mean) {
                    return Ok(Some(media));
                }
                // Scarti esatti dalla media esatta (`varianza_intera`).
                Ok(varianza_intera(&valori, config.ddof())?.map(f64::sqrt))
            })
            .collect()
    })?;
    Ok(Some(Arc::new(Float64Array::from(output))))
}

#[cfg(test)]
#[path = "window_oracolo.rs"]
mod oracolo;
