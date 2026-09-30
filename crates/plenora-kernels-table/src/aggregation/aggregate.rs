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

use crate::float64_source::Float64Source;
use crate::{
    column_index, ordine_esatto, replace_or_append, scalar_as_numero, scalar_as_string,
    select_rows, validate_output_name, NumericBound,
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
/// Le funzioni numeriche leggono la cella come `f64` arrotondando (interi
/// oltre `2^53`, `Decimal128`, testo numerico) e rendono `Float64`
/// nullabile: null con `skip_null` falso e un null nel gruppo, o senza
/// valori.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AggFunction {
    /// Celle non nulle del gruppo (null logico dei dizionari compreso),
    /// `Int64` non nullabile; qualunque tipo di colonna.
    Count,
    /// Somma in ordine d'ingresso (da `-0.0`); un NaN la rende NaN.
    Sum,
    /// Sinonimo di [`AggFunction::Mean`]; il nome d'uscita di default usa
    /// `mean`.
    Avg,
    /// Somma divisa per il numero di valori.
    Mean,
    /// Minimo con `f64::min`: i NaN si ignorano salvo che siano tutti NaN.
    Min,
    /// Massimo con `f64::max`: i NaN si ignorano salvo che siano tutti NaN.
    Max,
    /// Testo della cella nella prima riga del gruppo (null compreso),
    /// `Utf8`.
    First,
    /// Testo della cella nell'ultima riga del gruppo (null compreso),
    /// `Utf8`.
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
    #[serde(default)]
    pub separator: Option<String>,
    /// Valori distinti (assente: no). Non ha effetto su `count`, `nunique`,
    /// `first` e `last`. Su `concat` tiene la prima occorrenza di ogni
    /// testo; sulle funzioni numeriche deduplica sul valore esatto e riduce
    /// i distinti in ordine crescente.
    #[serde(default)]
    pub distinct: Option<bool>,
    /// Null ignorati (assente: si'). Non ha effetto su `count`, `first` e
    /// `last`. Falso: un null rende null le funzioni numeriche, conta come
    /// un valore in `nunique` e vale il testo vuoto in `concat`.
    #[serde(default)]
    pub skip_null: Option<bool>,
    /// Nome della colonna d'uscita; vuoto (default) vale `column`, o
    /// `<column>_<funzione>` se `column` compare in piu' aggregazioni.
    #[serde(default)]
    pub alias: String,
    /// Quantile in `[0, 1]`: obbligatorio con `quantile`, rifiutato con le
    /// altre funzioni.
    pub quantile: Option<f64>,
    /// Gradi di liberta' di `variance` e `stddev` (assente: 1).
    #[serde(default)]
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

impl Aggregation {
    /// Nome della funzione nel nome d'uscita `<column>_<funzione>`.
    #[must_use]
    pub const fn nome_funzione(&self) -> &'static str {
        match self.function {
            AggFunction::Count => "count",
            AggFunction::Sum => "sum",
            AggFunction::Avg | AggFunction::Mean => "mean",
            AggFunction::Min => "min",
            AggFunction::Max => "max",
            AggFunction::First => "first",
            AggFunction::Last => "last",
            AggFunction::Concat => "concat",
            AggFunction::Nunique => "nunique",
            AggFunction::Variance => "variance",
            AggFunction::Stddev => "stddev",
            AggFunction::Quantile => "quantile",
        }
    }
}

impl Aggregate {
    /// Nomi delle colonne aggregate, nell'ordine d'uscita: `alias`, o
    /// `column`, o `<column>_<funzione>` se `column` compare in piu'
    /// aggregazioni; `count` senza aggregazioni.
    ///
    /// Un nome ripetuto, o uguale a una colonna di `group_by`, farebbe
    /// sparire in silenzio la colonna scritta prima (l'uscita tiene l'ultima
    /// con quel nome): si rifiuta. La chiamano il kernel (anche per la
    /// variante spilled, che lo chiama per partizione) e l'analisi dei
    /// contratti.
    ///
    /// # Errors
    ///
    /// `InvalidPlan` per un nome d'uscita ripetuto o uguale a una chiave.
    pub fn nomi_uscita(&self) -> Result<Vec<String>> {
        let mut ripetizioni: HashMap<&str, usize> = HashMap::new();
        for aggregation in &self.aggregations {
            *ripetizioni.entry(aggregation.column.as_str()).or_insert(0) += 1;
        }
        let nomi: Vec<String> = if self.aggregations.is_empty() {
            vec!["count".to_owned()]
        } else {
            self.aggregations
                .iter()
                .map(|aggregation| {
                    if !aggregation.alias.is_empty() {
                        aggregation.alias.clone()
                    } else if ripetizioni
                        .get(aggregation.column.as_str())
                        .is_some_and(|volte| *volte > 1)
                    {
                        format!("{}_{}", aggregation.column, aggregation.nome_funzione())
                    } else {
                        aggregation.column.clone()
                    }
                })
                .collect()
        };
        let mut visti = std::collections::HashSet::new();
        for nome in &nomi {
            if self.group_by.contains(nome) {
                return Err(PlenoraError::InvalidPlan(format!(
                    "nome d'uscita {nome} uguale a una colonna di group_by: la chiave \
                     sparirebbe"
                )));
            }
            if !visti.insert(nome.as_str()) {
                return Err(PlenoraError::InvalidPlan(format!(
                    "nome d'uscita {nome} ripetuto: una aggregazione sparirebbe"
                )));
            }
        }
        Ok(nomi)
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

/// `table.aggregate`: una riga per chiave di gruppo distinta di `group_by`,
/// con le aggregazioni di `config.aggregations` (vuoto: solo la colonna
/// `count` con le righe di ogni gruppo).
///
/// Identita' di gruppo sul testo di ogni cella, come `table.distinct` (il
/// null e' un gruppo; `-0.0` e `0.0` distinti; un NaN solo). I gruppi escono
/// nell'ordine lessicografico delle loro chiavi testuali (`row_key`): null
/// prima, poi la stringa `<lunghezza>:<testo>` byte per byte, colonna per
/// colonna. Le righe di un gruppo si riducono in ordine d'ingresso. Un nome
/// d'uscita ripetuto o uguale a una chiave si rifiuta
/// ([`Aggregate::nomi_uscita`]). Con `Limits::default()`:
/// [`aggregate_con_limiti`] con i limiti del chiamante.
///
/// Un intero oltre `2^53` **non** e' un errore nelle aggregazioni a
/// risultato `Float64`: li' la conversione arrotonda, perche' il double e'
/// il tipo del risultato.
///
/// # Errors
///
/// - `InvalidPlan`: `group_by` vuoto; un parametro scritto per una funzione
///   che non lo usa ([`Aggregation::verifica_parametri`]); funzione
///   `quantile` senza il parametro `quantile` o con valore fuori `[0, 1]`;
///   nome d'uscita non valido (`validate_output_name`);
/// - `ResourceLimit`: conteggi e dimensioni di gruppo non rappresentabili
///   (`i64`/`f64`), indice di riga oltre `u32::MAX` (`select_rows`);
/// - `Schema`: una colonna di `group_by` o delle aggregazioni assente dallo
///   schema; gli errori di `scalar_as_string` (chiavi, `first`, `last`,
///   `concat`, `nunique`) e della lettura numerica (testo non numerico,
///   tipo non numerico);
/// - `DataMapping` (`arrow error: …`): errori Arrow di `select_rows` e `replace_or_append`;
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
        validate_output_name(name)?;
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
                let source = TextSource::new(batch.column(index));
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
                let column = batch.column(index);
                let first = matches!(aggregation.function, AggFunction::First);
                let values = map_groups(&groups, parallel, |rows| {
                    let row = if first {
                        rows[0]
                    } else {
                        *rows.last().unwrap_or(&rows[0])
                    };
                    scalar_as_string(column.as_ref(), row)
                })?;
                result = replace_or_append(
                    &result,
                    name,
                    DataType::Utf8,
                    true,
                    Arc::new(StringArray::from(values)),
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
