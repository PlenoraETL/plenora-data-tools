use std::borrow::Cow;
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use num_traits::ToPrimitive;
use plenora_core::arrow::array::{
    Array, ArrayRef, Float64Array, Int64Array, RecordBatch, StringArray, UInt64Array,
};
use plenora_core::arrow::schema::DataType;
use serde::Deserialize;

use plenora_core::{PlenoraError, Result};

use crate::float64_source::{
    prendi_righe, varianza_intera, ColonnaEsatta, ColonnaIntera, Float64Source, SommaEsatta,
};
use crate::{
    column_index, ordine_esatto, replace_or_append, scalar_as_numero, select_rows,
    validate_output_name, NumericBound,
};

use super::grouping::{
    build_binary_groups, build_native_groups, cmp_i64_group_key, cmp_str_group_key,
    cmp_u64_group_key, map_groups, TextSource, PARALLEL_THRESHOLD,
};

/// Cardinalita' di un gruppo come `i64`, in modo **fallibile**.
///
/// La colonna `count` e' non-nullable: un `.ok()` renderebbe `null` un
/// fallimento di conversione, violando in silenzio lo schema. Il caso e'
/// irraggiungibile sulle piattaforme correnti, ma la conversione o riesce o
/// produce un errore esplicito.
///
/// # Errors
///
/// `PlenoraError::Internal` se la cardinalita' non e' rappresentabile in
/// `i64`.
pub(super) fn conteggio_gruppo(righe: usize) -> Result<i64> {
    i64::try_from(righe).map_err(|_| {
        PlenoraError::Internal(
            "cardinalita' di un gruppo non rappresentabile in i64 per la colonna `count`"
                .to_owned(),
        )
    })
}

/// Funzione di un'aggregazione di `table.aggregate` (in JSON in
/// minuscolo: `"count"`, `"sum"`, ...).
///
/// Le funzioni numeriche rendono null con `skip_null` falso e un null nel
/// gruppo, o senza valori. Sul dominio intero (`Int64`, `UInt64`, `Date32`,
/// `Date64`, `Timestamp` di ogni unita' nel valore nativo) `sum` e' esatta
/// (`i128`) ed esce `Int64` (rifiutata su date e istanti), e media,
/// varianza e deviazione partono dalla somma esatta; `min` e `max` su
/// interi e `Decimal128` scelgono la cella col confronto esatto e tengono il
/// tipo d'ingresso. Altrove la cella si legge come `f64`, arrotondando
/// (`Decimal128`, testo numerico), e l'uscita e' `Float64`.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AggFunction {
    /// Celle non nulle del gruppo (null logico dei dizionari compreso),
    /// `Int64` non nullabile; qualunque tipo di colonna.
    Count,
    /// Somma: esatta ed `Int64` sul dominio intero (errore oltre `i64`),
    /// altrimenti `Float64` in ordine d'ingresso (da `-0.0`), e un NaN la
    /// rende NaN.
    Sum,
    /// Sinonimo di [`AggFunction::Mean`]; il nome d'uscita di default usa
    /// `mean`.
    Avg,
    /// Somma divisa per il numero di valori (`Float64`).
    Mean,
    /// Minimo: sul dominio intero e su `Decimal128` la cella minima, nel
    /// tipo d'ingresso; altrimenti `f64::min` (`Float64`), e i NaN si
    /// ignorano salvo che siano tutti NaN.
    Min,
    /// Massimo, come [`AggFunction::Min`].
    Max,
    /// La cella nella prima riga del gruppo (null compreso), nel tipo
    /// d'ingresso.
    First,
    /// La cella nell'ultima riga del gruppo (null compreso), nel tipo
    /// d'ingresso.
    Last,
    /// Testi delle celle in ordine d'ingresso uniti da `separator`, `Utf8`.
    Concat,
    /// Testi distinti del gruppo, piu' uno per il null con `skip_null`
    /// falso; `Int64` non nullabile.
    Nunique,
    /// Varianza in due passate, divisore `valori - ddof`; null con
    /// `valori <= ddof`.
    Variance,
    /// Radice di [`AggFunction::Variance`].
    Stddev,
    /// Interpolazione lineare fra i valori ordinati con `total_cmp`, alla
    /// posizione `quantile * (valori - 1)`.
    Quantile,
}

const fn default_agg() -> AggFunction {
    AggFunction::Count
}
pub(in crate::aggregation) const fn default_ddof() -> usize {
    1
}

/// Un'aggregazione di `table.aggregate`. Campi sconosciuti rifiutati; un
/// parametro scritto per una funzione che non lo usa si rifiuta
/// ([`Aggregation::verifica_parametri`]).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Aggregation {
    /// Colonna aggregata.
    pub column: String,
    /// Funzione (default [`AggFunction::Count`]).
    #[serde(default = "default_agg")]
    pub function: AggFunction,
    /// Separatore di `concat` (assente: `", "`).
    #[serde(default, deserialize_with = "crate::mai_null")]
    pub separator: Option<String>,
    /// Valori distinti (assente: no). Non ha effetto su `count`, `nunique`,
    /// `first` e `last`. Su `concat` tiene la prima occorrenza di ogni
    /// testo; sulle funzioni numeriche deduplica sul valore esatto e riduce
    /// i distinti in ordine crescente.
    #[serde(default, deserialize_with = "crate::mai_null")]
    pub distinct: Option<bool>,
    /// Null ignorati (assente: si'). Non ha effetto su `count`, `first` e
    /// `last`. Falso: un null rende null le funzioni numeriche, conta come
    /// un valore in `nunique` e vale il testo vuoto in `concat`.
    #[serde(default, deserialize_with = "crate::mai_null")]
    pub skip_null: Option<bool>,
    /// Nome della colonna d'uscita; vuoto (default) vale `column`, o
    /// `<column>_<funzione>` se `column` compare in piu' aggregazioni.
    #[serde(default)]
    pub alias: String,
    /// Quantile in `[0, 1]`: obbligatorio con `quantile`, rifiutato con le
    /// altre funzioni.
    #[serde(default, deserialize_with = "crate::mai_null")]
    pub quantile: Option<f64>,
    /// Gradi di liberta' di `variance` e `stddev` (assente: 1).
    #[serde(default, deserialize_with = "crate::mai_null")]
    pub ddof: Option<usize>,
}

impl Aggregation {
    /// Separatore di `concat`.
    #[must_use]
    pub fn separator(&self) -> &str {
        self.separator.as_deref().unwrap_or(", ")
    }

    /// Valori distinti.
    #[must_use]
    pub fn distinct(&self) -> bool {
        self.distinct.unwrap_or(false)
    }

    /// Null ignorati.
    #[must_use]
    pub fn skip_null(&self) -> bool {
        self.skip_null.unwrap_or(true)
    }

    /// Gradi di liberta' di `variance` e `stddev`.
    #[must_use]
    pub fn ddof(&self) -> usize {
        self.ddof.unwrap_or(default_ddof())
    }

    /// Un parametro scritto che la funzione non usa si rifiuta, non si
    /// ignora: `separator` fuori da `concat`, `distinct` su `count`,
    /// `nunique`, `first` e `last`, `skip_null` su `count`, `first` e
    /// `last`, `ddof` fuori da `variance` e `stddev`, `quantile` fuori da
    /// `quantile`. Un parametro assente prende il suo default. La chiamano il
    /// kernel e l'analisi dei contratti.
    ///
    /// # Errors
    ///
    /// `InvalidPlan` per il primo parametro che la funzione non usa.
    pub fn verifica_parametri(&self) -> Result<()> {
        use AggFunction as F;
        let funzione = &self.function;
        let ignorato = if self.separator.is_some() && !matches!(funzione, F::Concat) {
            Some("separator ammesso solo con function=concat")
        } else if self.distinct.is_some()
            && matches!(funzione, F::Count | F::Nunique | F::First | F::Last)
        {
            Some("distinct non ha effetto con count, nunique, first e last")
        } else if self.skip_null.is_some() && matches!(funzione, F::Count | F::First | F::Last) {
            Some("skip_null non ha effetto con count, first e last")
        } else if self.ddof.is_some() && !matches!(funzione, F::Variance | F::Stddev) {
            Some("ddof ammesso solo con function=variance o stddev")
        } else if self.quantile.is_some() && !matches!(funzione, F::Quantile) {
            Some("quantile ammesso solo con function=quantile")
        } else {
            None
        };
        ignorato.map_or(Ok(()), |messaggio| {
            Err(PlenoraError::InvalidPlan(messaggio.into()))
        })
    }
}

/// Config di `table.aggregate`. Campi sconosciuti rifiutati.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Aggregate {
    /// Colonne della chiave di gruppo, almeno una, leggibili come testo.
    pub group_by: Vec<String>,
    /// Aggregazioni, nell'ordine delle colonne d'uscita; vuoto (default)
    /// produce una colonna `count` con le righe di ogni gruppo.
    #[serde(default)]
    pub aggregations: Vec<Aggregation>,
}

impl AggFunction {
    /// Il nome della funzione nei nomi d'uscita di default
    /// (`<column>_<funzione>`): `avg` vale `mean`.
    #[must_use]
    pub const fn nome(self) -> &'static str {
        match self {
            Self::Count => "count",
            Self::Sum => "sum",
            Self::Avg | Self::Mean => "mean",
            Self::Min => "min",
            Self::Max => "max",
            Self::First => "first",
            Self::Last => "last",
            Self::Concat => "concat",
            Self::Nunique => "nunique",
            Self::Variance => "variance",
            Self::Stddev => "stddev",
            Self::Quantile => "quantile",
        }
    }
}

impl Aggregate {
    /// I nomi delle colonne aggregate, nell'ordine di `aggregations` (senza
    /// aggregazioni, `count`): `alias`, o `column`, o `<column>_<funzione>`
    /// se `column` compare in piu' aggregazioni.
    ///
    /// Autorita' unica per il kernel e l'analisi: ogni nome e' valido, e le
    /// chiavi di `group_by` con i nomi aggregati sono **tutti distinti**
    /// ([`crate::verifica_nomi_distinti`]). Prima due aggregazioni con lo
    /// stesso nome, o un alias uguale a una chiave, sostituivano la colonna
    /// precedente e una spariva senza errore.
    ///
    /// # Errors
    ///
    /// `InvalidPlan`: un nome non valido (`validate_output_name`), uguale a
    /// una chiave di `group_by` o ripetuto (anche fra le chiavi).
    pub fn nomi_uscita(&self) -> Result<Vec<String>> {
        let mut occorrenze: HashMap<&str, usize> = HashMap::new();
        for aggregation in &self.aggregations {
            *occorrenze.entry(aggregation.column.as_str()).or_insert(0) += 1;
        }
        let nomi = if self.aggregations.is_empty() {
            vec!["count".to_owned()]
        } else {
            self.aggregations
                .iter()
                .map(|aggregation| {
                    if !aggregation.alias.is_empty() {
                        aggregation.alias.clone()
                    } else if occorrenze
                        .get(aggregation.column.as_str())
                        .is_some_and(|volte| *volte > 1)
                    {
                        format!("{}_{}", aggregation.column, aggregation.function.nome())
                    } else {
                        aggregation.column.clone()
                    }
                })
                .collect::<Vec<_>>()
        };
        for nome in &nomi {
            validate_output_name(nome)?;
        }
        // Due messaggi distinti per i due casi, senza nomi: la chiave che
        // sparirebbe e l'aggregazione che sparirebbe.
        if nomi.iter().any(|nome| self.group_by.contains(nome)) {
            return Err(PlenoraError::InvalidPlan(
                "aggregate: nome d'uscita uguale a una colonna di group_by: la chiave                  sparirebbe"
                    .into(),
            ));
        }
        if crate::verifica_nomi_distinti("aggregate", nomi.iter().map(String::as_str)).is_err() {
            return Err(PlenoraError::InvalidPlan(
                "aggregate: nome d'uscita ripetuto: una aggregazione sparirebbe".into(),
            ));
        }
        crate::verifica_nomi_distinti("aggregate", self.group_by.iter().map(String::as_str))?;
        Ok(nomi)
    }
}

/// Riduzione Sum/Avg/Min/Max/Variance/Stddev di un gruppo senza materializzarlo.
///
/// Stesse operazioni f64 nello stesso ordine del percorso materializzato
/// (`values.iter().sum()`, due passate per la varianza): risultato
/// bit-identico.
fn reduce_numeric_streaming(raw: &[Option<f64>], aggregation: &Aggregation) -> Result<Option<f64>> {
    let mut len = 0_usize;
    // Inizializza a -0.0: `Iterator::sum` sui float in std fa fold da -0.0
    // (per preservare il segno dello zero); la parita' bit-a-bit con il
    // percorso materializzato include il segno dello zero della somma.
    let mut sum = -0.0_f64;
    for value in raw.iter().flatten() {
        sum += *value;
        len += 1;
    }
    if len == 0 {
        return Ok(None);
    }
    Ok(Some(match aggregation.function {
        AggFunction::Sum => sum,
        AggFunction::Avg | AggFunction::Mean => {
            sum / len.to_f64().ok_or_else(|| {
                PlenoraError::ResourceLimit("dimensione gruppo non rappresentabile".into())
            })?
        }
        AggFunction::Min => raw
            .iter()
            .flatten()
            .copied()
            .reduce(f64::min)
            .unwrap_or_default(),
        AggFunction::Max => raw
            .iter()
            .flatten()
            .copied()
            .reduce(f64::max)
            .unwrap_or_default(),
        AggFunction::Variance | AggFunction::Stddev => {
            if len <= aggregation.ddof() {
                return Ok(None);
            }
            let length = len.to_f64().ok_or_else(|| {
                PlenoraError::ResourceLimit("dimensione gruppo non rappresentabile".into())
            })?;
            let mean = sum / length;
            let divisor = (len - aggregation.ddof()).to_f64().ok_or_else(|| {
                PlenoraError::ResourceLimit("divisore statistico non rappresentabile".into())
            })?;
            let variance = raw
                .iter()
                .flatten()
                .map(|value| (value - mean).powi(2))
                .sum::<f64>()
                / divisor;
            if matches!(aggregation.function, AggFunction::Stddev) {
                variance.sqrt()
            } else {
                variance
            }
        }
        _ => {
            return Err(PlenoraError::Internal(
                "funzione fuori dal percorso streaming di reduce_numeric".into(),
            ));
        }
    }))
}

/// I valori distinti di un gruppo, deduplicati sul valore **esatto**: due
/// interi oltre 2^53 con lo stesso double restano due valori. Su una colonna
/// `Float64` il valore e' il double, con l'ordine di `total_cmp` (-0.0 e 0.0
/// distinti, i NaN distinti per segno e payload); sugli altri tipi
/// `ordine_esatto`.
///
/// Un null resta nell'elenco, in testa: `reduce_numeric` decide con
/// `skip_null`.
///
/// # Errors
///
/// Gli errori di `scalar_as_numero`.
pub(super) fn distinti_esatti(array: &ArrayRef, rows: &[usize]) -> Result<Vec<Option<f64>>> {
    let float64 = array.data_type() == &DataType::Float64;
    let mut nulli = 0_usize;
    let mut numeri = Vec::with_capacity(rows.len());
    for row in rows {
        match scalar_as_numero(array.as_ref(), *row)? {
            Some(cella) => numeri.push(cella),
            None => nulli += 1,
        }
    }
    let ordine = |sinistra: &(f64, NumericBound), destra: &(f64, NumericBound)| {
        if float64 {
            sinistra.0.total_cmp(&destra.0)
        } else {
            ordine_esatto(sinistra.1, destra.1)
        }
    };
    numeri.sort_by(ordine);
    numeri.dedup_by(|destra, sinistra| ordine(sinistra, destra) == Ordering::Equal);
    Ok(std::iter::repeat_n(None, nulli)
        .chain(numeri.into_iter().map(|(valore, _)| Some(valore)))
        .collect())
}

fn reduce_numeric(raw: Vec<Option<f64>>, aggregation: &Aggregation) -> Result<Option<f64>> {
    if !aggregation.skip_null() && raw.iter().any(Option::is_none) {
        return Ok(None);
    }
    // Solo `distinct` e `quantile` hanno bisogno del gruppo materializzato
    // (ordinamento); per le altre funzioni il secondo `Vec` e' lavoro
    // evitabile: si riduce sull'iteratore flatten, stesse operazioni f64
    // nello stesso ordine, quindi parita' bit per bit per costruzione.
    if !aggregation.distinct() && !matches!(aggregation.function, AggFunction::Quantile) {
        return reduce_numeric_streaming(&raw, aggregation);
    }
    // Con `distinct` i valori arrivano gia' distinti (`distinti_esatti`).
    let mut values = raw.into_iter().flatten().collect::<Vec<_>>();
    if values.is_empty() {
        return Ok(None);
    }
    let sum: f64 = values.iter().sum();
    Ok(Some(match aggregation.function {
        AggFunction::Sum => sum,
        AggFunction::Avg | AggFunction::Mean => {
            sum / values.len().to_f64().ok_or_else(|| {
                PlenoraError::ResourceLimit("dimensione gruppo non rappresentabile".into())
            })?
        }
        AggFunction::Min => values.iter().copied().reduce(f64::min).unwrap_or_default(),
        AggFunction::Max => values.iter().copied().reduce(f64::max).unwrap_or_default(),
        AggFunction::Variance | AggFunction::Stddev => {
            if values.len() <= aggregation.ddof() {
                return Ok(None);
            }
            let length = values.len().to_f64().ok_or_else(|| {
                PlenoraError::ResourceLimit("dimensione gruppo non rappresentabile".into())
            })?;
            let mean = sum / length;
            let divisor = (values.len() - aggregation.ddof())
                .to_f64()
                .ok_or_else(|| {
                    PlenoraError::ResourceLimit("divisore statistico non rappresentabile".into())
                })?;
            let variance = values
                .iter()
                .map(|value| (value - mean).powi(2))
                .sum::<f64>()
                / divisor;
            if matches!(aggregation.function, AggFunction::Stddev) {
                variance.sqrt()
            } else {
                variance
            }
        }
        AggFunction::Quantile => {
            let quantile = aggregation.quantile.ok_or_else(|| {
                PlenoraError::InvalidPlan("quantile richiede il parametro quantile".into())
            })?;
            values.sort_by(f64::total_cmp);
            let last = (values.len() - 1).to_f64().ok_or_else(|| {
                PlenoraError::ResourceLimit("dimensione quantile non rappresentabile".into())
            })?;
            let position = quantile * last;
            let lower = position
                .floor()
                .to_usize()
                .ok_or_else(|| PlenoraError::InvalidPlan("indice quantile non valido".into()))?;
            let upper = position
                .ceil()
                .to_usize()
                .ok_or_else(|| PlenoraError::InvalidPlan("indice quantile non valido".into()))?;
            let weight = position - position.floor();
            // Niente mul_add/FMA: la fusione cambia l'arrotondamento IEEE, e
            // se la usasse o no dipenderebbe dal target. La forma non fusa
            // e' il contratto numerico; produzione e oracolo usano la STESSA
            // forma, quindi l'equivalenza bit per bit resta per costruzione.
            #[allow(clippy::suboptimal_flops)]
            let interpolated = (values[upper] - values[lower]) * weight + values[lower];
            interpolated
        }
        // Il dispatch di `aggregate` instrada a `reduce_numeric` solo
        // Sum/Avg/Mean/Min/Max/Variance/Stddev/Quantile; le altre funzioni
        // hanno percorsi dedicati. Il compilatore non puo' dimostrarlo:
        // invariante interna, errore esplicito invece di un panico.
        _ => {
            return Err(PlenoraError::Internal(
                "funzione fuori dal percorso numerico di reduce_numeric".into(),
            ));
        }
    }))
}

/// Tipo d'uscita di un'aggregazione sulla colonna di tipo `data_type`:
/// l'autorita' di kernel e analisi.
#[must_use]
pub fn tipo_uscita(function: AggFunction, data_type: &DataType) -> DataType {
    match function {
        AggFunction::Count | AggFunction::Nunique => DataType::Int64,
        AggFunction::Concat => DataType::Utf8,
        AggFunction::First | AggFunction::Last => data_type.clone(),
        AggFunction::Sum => crate::float64_source::tipo_somma(data_type),
        AggFunction::Min | AggFunction::Max => crate::float64_source::tipo_estremo(data_type),
        AggFunction::Avg
        | AggFunction::Mean
        | AggFunction::Variance
        | AggFunction::Stddev
        | AggFunction::Quantile => DataType::Float64,
    }
}

/// I valori interi esatti di un gruppo; `None` se `skip_null` e' falso e il
/// gruppo ha un null (il risultato e' null). Con `distinct` i valori
/// distinti, in ordine crescente.
fn valori_interi(
    interi: &ColonnaIntera<'_>,
    rows: &[usize],
    aggregation: &Aggregation,
) -> Option<Vec<i128>> {
    let mut letti = Vec::with_capacity(rows.len());
    for row in rows {
        match interi.value(*row) {
            Some(numero) => letti.push(numero),
            None if !aggregation.skip_null() => return None,
            None => {}
        }
    }
    if aggregation.distinct() {
        letti.sort_unstable();
        letti.dedup();
    }
    Some(letti)
}

/// Somma esatta di un gruppo del dominio intero, come `Int64`.
///
/// # Errors
///
/// `DataMapping` se la somma esce dalla gamma di `i64`.
fn somma_intera(
    interi: &ColonnaIntera<'_>,
    rows: &[usize],
    aggregation: &Aggregation,
) -> Result<Option<i64>> {
    let Some(valori) = valori_interi(interi, rows, aggregation) else {
        return Ok(None);
    };
    if valori.is_empty() {
        return Ok(None);
    }
    let mut somma = SommaEsatta::default();
    for valore in valori {
        somma.aggiungi(valore)?;
    }
    somma.in_int64().map(Some)
}

/// Media, varianza e deviazione di un gruppo del dominio intero: la media
/// dalla somma esatta, la varianza dagli scarti esatti
/// ([`varianza_intera`]), arrotondate una volta al double.
///
/// # Errors
///
/// `DataMapping` se la somma esce da `i128`; `ResourceLimit` per dimensioni
/// non rappresentabili.
fn statistica_intera(
    interi: &ColonnaIntera<'_>,
    rows: &[usize],
    aggregation: &Aggregation,
) -> Result<Option<f64>> {
    let Some(valori) = valori_interi(interi, rows, aggregation) else {
        return Ok(None);
    };
    let mut somma = SommaEsatta::default();
    for valore in &valori {
        somma.aggiungi(*valore)?;
    }
    let Some(media) = somma.media() else {
        return Ok(None);
    };
    match aggregation.function {
        AggFunction::Avg | AggFunction::Mean => Ok(Some(media)),
        AggFunction::Variance | AggFunction::Stddev => {
            // Scarti esatti dalla media esatta (`varianza_intera`).
            Ok(
                varianza_intera(&valori, aggregation.ddof())?.map(|variance| {
                    if matches!(aggregation.function, AggFunction::Stddev) {
                        variance.sqrt()
                    } else {
                        variance
                    }
                }),
            )
        }
        _ => Err(PlenoraError::Internal(
            "funzione fuori dal percorso delle statistiche intere".into(),
        )),
    }
}

/// La riga dell'estremo (`min`, `max`) di un gruppo sul valore esatto
/// ([`ColonnaEsatta`]), a pari valore la prima; `None` se il gruppo non ha
/// valori o se `skip_null` e' falso e ha un null.
fn riga_estremo(
    colonna: &ColonnaEsatta<'_>,
    rows: &[usize],
    aggregation: &Aggregation,
) -> Option<usize> {
    let massimo = matches!(aggregation.function, AggFunction::Max);
    let mut estremo = None;
    for row in rows {
        match colonna.value(*row) {
            Some(valore) => estremo = ColonnaEsatta::aggiorna(estremo, *row, valore, massimo),
            None if !aggregation.skip_null() => return None,
            None => {}
        }
    }
    estremo.map(|(riga, _)| riga)
}

/// `table.aggregate`: una riga per chiave di gruppo distinta di `group_by`,
/// con le aggregazioni di `config.aggregations` (vuoto: solo la colonna
/// `count` con le righe di ogni gruppo).
///
/// Identita' di gruppo sul testo di ogni cella, come `table.distinct` (il
/// null e' un gruppo; `-0.0` e `0.0` distinti; un NaN solo). I gruppi escono
/// nell'ordine lessicografico delle loro chiavi testuali (`row_key`): null
/// prima, poi la stringa `<lunghezza>:<testo>` byte per byte, colonna per
/// colonna. Le righe di un gruppo si riducono in ordine d'ingresso. Le
/// chiavi e le colonne aggregate hanno nomi tutti distinti
/// ([`Aggregate::nomi_uscita`]): un nome ripetuto si rifiuta. Con
/// `Limits::default()`: [`aggregate_con_limiti`] con i limiti del chiamante.
///
/// Tipi d'uscita: [`tipo_uscita`]. Sul dominio intero `sum` e' esatta ed
/// esce `Int64`, `min`/`max` su interi e decimali e `first`/`last` tengono
/// il tipo d'ingresso. Nelle aggregazioni a risultato `Float64` (media,
/// varianza, quantile; ogni funzione su `Decimal128` e testo) un valore
/// oltre la precisione del double arrotonda, perche' il double e' il tipo
/// del risultato; la media sul dominio intero parte dalla somma esatta.
///
/// # Errors
///
/// - `InvalidPlan`: `group_by` vuoto; un parametro scritto per una funzione
///   che non lo usa ([`Aggregation::verifica_parametri`]); funzione
///   `quantile` senza il parametro `quantile` o con valore fuori `[0, 1]`;
///   nome d'uscita non valido (`validate_output_name`) o ripetuto, anche
///   uguale a una chiave di `group_by` ([`Aggregate::nomi_uscita`]);
/// - `ResourceLimit`: conteggi e dimensioni di gruppo non rappresentabili
///   (`i64`/`f64`), indice di riga oltre `u32::MAX` (`select_rows`);
/// - `Schema`: una colonna di `group_by` o delle aggregazioni assente dallo
///   schema; gli errori di `scalar_as_string` (chiavi, `first`, `last`,
///   `concat`, `nunique`) e della lettura numerica (testo non numerico,
///   tipo non numerico);
/// - `DataMapping`: una somma intera oltre la gamma di `Int64`; errori
///   Arrow (`arrow error: …`) di `select_rows`, `take` e `replace_or_append`;
/// - `Internal`: invarianti interne del raggruppamento.
pub fn aggregate(batch: &RecordBatch, config: &Aggregate) -> Result<RecordBatch> {
    aggregate_con_limiti(batch, config, &crate::Limits::default())
}

/// [`aggregate`] con i limiti del chiamante.
///
/// Il runner e la variante spilled passano i loro: il testo di `concat`,
/// che unisce le celle di un gruppo intero, non supera
/// `limits.max_string_bytes`, controllato prima di unirle.
///
/// # Errors
///
/// Come [`aggregate`], piu' `ResourceLimit` per un `concat` oltre
/// `limits.max_string_bytes`.
#[allow(clippy::too_many_lines)] // Le varianti condividono una passata di raggruppamento e i suoi invarianti.
pub fn aggregate_con_limiti(
    batch: &RecordBatch,
    config: &Aggregate,
    limits: &crate::Limits,
) -> Result<RecordBatch> {
    let group_indices = config
        .group_by
        .iter()
        .map(|name| column_index(batch, name))
        .collect::<Result<Vec<_>>>()?;
    if group_indices.is_empty() {
        return Err(PlenoraError::InvalidPlan(
            "aggregate richiede group_by".into(),
        ));
    }
    // Fail-closed prima dei dati (regola 1): un quantile fuori [0, 1]
    // produrrebbe indici oltre il gruppo ordinato — errore esplicito, mai
    // indexing out-of-bounds a meta' esecuzione.
    for aggregation in &config.aggregations {
        aggregation.verifica_parametri()?;
        if matches!(aggregation.function, AggFunction::Quantile)
            && aggregation
                .quantile
                .is_some_and(|quantile| !(0.0..=1.0).contains(&quantile))
        {
            return Err(PlenoraError::InvalidPlan(
                "quantile fuori dall'intervallo 0..=1".into(),
            ));
        }
    }
    // I nomi dopo i parametri, come nell'analisi.
    let nomi = config.nomi_uscita()?;
    // Raggruppamento: fast path nativo per colonna singola
    // Int64/UInt64/Utf8 (nessuna stringa di chiave), chiavi binarie con la
    // stessa identita' di `row_key` altrimenti. Stesso ordine canonico dei
    // gruppi in uscita.
    let groups = if group_indices.len() == 1 {
        let array = batch.column(group_indices[0]);
        if let Some(values) = array.as_any().downcast_ref::<Int64Array>() {
            build_native_groups(
                batch.num_rows(),
                |row| {
                    if values.is_null(row) {
                        None
                    } else {
                        Some(values.value(row))
                    }
                },
                |a, b| cmp_i64_group_key(*a, *b),
            )?
        } else if let Some(values) = array.as_any().downcast_ref::<UInt64Array>() {
            build_native_groups(
                batch.num_rows(),
                |row| {
                    if values.is_null(row) {
                        None
                    } else {
                        Some(values.value(row))
                    }
                },
                |a, b| cmp_u64_group_key(*a, *b),
            )?
        } else if let Some(values) = array.as_any().downcast_ref::<StringArray>() {
            build_native_groups(
                batch.num_rows(),
                |row| {
                    if values.is_null(row) {
                        None
                    } else {
                        Some(values.value(row))
                    }
                },
                |a, b| cmp_str_group_key(a, b),
            )?
        } else {
            build_binary_groups(batch, &group_indices)?
        }
    } else {
        build_binary_groups(batch, &group_indices)?
    };
    // Il calcolo per gruppo va in parallelo solo se la dimensione media dei
    // gruppi ripaga il costo di dispatch dei task (gruppi minuscoli restano
    // sequenziali).
    let parallel = batch.num_rows() >= PARALLEL_THRESHOLD
        && groups.len() > 1
        && batch.num_rows() / groups.len() >= 8;
    let representatives = groups.iter().map(|rows| rows[0]).collect::<Vec<_>>();
    let projected = select_rows(batch, &representatives)?;
    let group_columns = group_indices
        .iter()
        .map(|index| projected.column(*index).clone())
        .collect::<Vec<_>>();
    let group_fields = group_indices
        .iter()
        .map(|index| batch.schema().field(*index).as_ref().clone())
        .collect::<Vec<_>>();
    let righe_gruppi = group_columns
        .first()
        .map_or(0, plenora_core::arrow::array::Array::len);
    let mut result = crate::batch_with_rows(
        Arc::new(plenora_core::arrow::schema::Schema::new_with_metadata(
            group_fields,
            batch.schema().metadata().clone(),
        )),
        group_columns,
        righe_gruppi,
    )?;
    if config.aggregations.is_empty() {
        let counts = groups
            .iter()
            .map(|rows| conteggio_gruppo(rows.len()))
            .collect::<Result<Vec<_>>>()?;
        return replace_or_append(
            &result,
            "count",
            DataType::Int64,
            false,
            Arc::new(Int64Array::from(counts)),
        );
    }
    for (aggregation, name) in config.aggregations.iter().zip(&nomi) {
        let index = column_index(batch, &aggregation.column)?;
        if matches!(aggregation.function, AggFunction::Sum) {
            crate::float64_source::verifica_somma(batch.column(index).data_type())?;
        }
        match aggregation.function {
            AggFunction::Count => {
                let column = batch.column(index);
                let values = map_groups(&groups, parallel, |rows| {
                    // `count` conta i valori, non le chiavi: una chiave
                    // dictionary valida che punta a una entry nulla e' una
                    // riga senza valore e non va contata.
                    let count = rows
                        .iter()
                        .filter(|row| !crate::is_logically_null(column.as_ref(), **row))
                        .count();
                    i64::try_from(count).map(Some).map_err(|_| {
                        PlenoraError::ResourceLimit("conteggio gruppo oltre i64".into())
                    })
                })?;
                result = replace_or_append(
                    &result,
                    name,
                    DataType::Int64,
                    false,
                    Arc::new(Int64Array::from(values)),
                )?;
            }
            AggFunction::Nunique => {
                let source = TextSource::chiave(batch.column(index));
                let values = map_groups(&groups, parallel, |rows| {
                    let mut seen = HashSet::new();
                    let mut null_seen = false;
                    for row in rows {
                        if let Some(value) = source.value(*row)? {
                            seen.insert(value);
                        } else {
                            null_seen = true;
                        }
                    }
                    // Come il generico: valori distinti piu' una voce per il
                    // null solo quando `skip_null` e' falso.
                    let count = seen.len() + usize::from(null_seen && !aggregation.skip_null());
                    i64::try_from(count).map(Some).map_err(|_| {
                        PlenoraError::ResourceLimit("conteggio gruppo oltre i64".into())
                    })
                })?;
                result = replace_or_append(
                    &result,
                    name,
                    DataType::Int64,
                    false,
                    Arc::new(Int64Array::from(values)),
                )?;
            }
            AggFunction::First | AggFunction::Last => {
                // La cella com'e', nel tipo d'ingresso: il testo di un
                // istante o di un decimale non e' il valore. I tipi ammessi
                // sono quelli dell'analisi, senza il fuso: la cella non passa
                // dal testo.
                let column = batch.column(index);
                crate::validate_cella_prendibile(column.data_type(), &aggregation.column)?;
                let first = matches!(aggregation.function, AggFunction::First);
                let righe = groups
                    .iter()
                    .map(|rows| {
                        Some(if first {
                            rows[0]
                        } else {
                            *rows.last().unwrap_or(&rows[0])
                        })
                    })
                    .collect::<Vec<_>>();
                result = replace_or_append(
                    &result,
                    name,
                    column.data_type().clone(),
                    true,
                    prendi_righe(column, &righe)?,
                )?;
            }
            AggFunction::Concat => {
                let source = TextSource::new(batch.column(index));
                let values = map_groups(&groups, parallel, |rows| {
                    let mut seen = HashSet::new();
                    let mut values = Vec::new();
                    for row in rows {
                        if let Some(value) = source.value(*row)? {
                            if !aggregation.distinct() || seen.insert(value.clone()) {
                                values.push(value);
                            }
                        } else if !aggregation.skip_null() {
                            values.push(Cow::Borrowed(""));
                        }
                    }
                    // Byte del testo unito, prima di allocarlo.
                    let separatori = aggregation
                        .separator()
                        .len()
                        .checked_mul(values.len().saturating_sub(1));
                    let byte = values
                        .iter()
                        .try_fold(separatori.unwrap_or(usize::MAX), |totale, valore| {
                            totale.checked_add(valore.len())
                        })
                        .unwrap_or(usize::MAX);
                    crate::verifica_testo_prodotto("aggregate", byte, limits)?;
                    Ok(Some(values.join(aggregation.separator())))
                })?;
                result = replace_or_append(
                    &result,
                    name,
                    DataType::Utf8,
                    true,
                    Arc::new(StringArray::from(values)),
                )?;
            }
            AggFunction::Sum if ColonnaIntera::new(batch.column(index)).is_some() => {
                let column = batch.column(index);
                let interi = ColonnaIntera::new(column)
                    .ok_or_else(|| PlenoraError::Internal("colonna intera scomparsa".into()))?;
                let values = map_groups(&groups, parallel, |rows| {
                    somma_intera(&interi, rows, aggregation)
                })?;
                result = replace_or_append(
                    &result,
                    name,
                    DataType::Int64,
                    true,
                    Arc::new(Int64Array::from(values)),
                )?;
            }
            AggFunction::Avg | AggFunction::Mean | AggFunction::Variance | AggFunction::Stddev
                if ColonnaIntera::new(batch.column(index)).is_some() =>
            {
                let column = batch.column(index);
                let interi = ColonnaIntera::new(column)
                    .ok_or_else(|| PlenoraError::Internal("colonna intera scomparsa".into()))?;
                let values = map_groups(&groups, parallel, |rows| {
                    statistica_intera(&interi, rows, aggregation)
                })?;
                result = replace_or_append(
                    &result,
                    name,
                    DataType::Float64,
                    true,
                    Arc::new(Float64Array::from(values)),
                )?;
            }
            AggFunction::Min | AggFunction::Max
                if ColonnaEsatta::new(batch.column(index)).is_some() =>
            {
                let column = batch.column(index);
                let esatta = ColonnaEsatta::new(column)
                    .ok_or_else(|| PlenoraError::Internal("colonna esatta scomparsa".into()))?;
                let righe = map_groups(&groups, parallel, |rows| {
                    Ok(riga_estremo(&esatta, rows, aggregation))
                })?;
                result = replace_or_append(
                    &result,
                    name,
                    column.data_type().clone(),
                    true,
                    prendi_righe(column, &righe)?,
                )?;
            }
            _ => {
                let source = Float64Source::new(batch.column(index));
                let values = map_groups(&groups, parallel, |rows| {
                    let raw = if aggregation.distinct() {
                        distinti_esatti(batch.column(index), rows)?
                    } else {
                        rows.iter()
                            .map(|row| source.value(*row))
                            .collect::<Result<Vec<_>>>()?
                    };
                    reduce_numeric(raw, aggregation)
                })?;
                result = replace_or_append(
                    &result,
                    name,
                    DataType::Float64,
                    true,
                    Arc::new(Float64Array::from(values)),
                )?;
            }
        }
    }
    Ok(result)
}
