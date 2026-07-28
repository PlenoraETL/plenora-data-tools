use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use plenora_core::arrow::array::{Float64Array, RecordBatch, StringArray};
use plenora_core::arrow::schema::DataType;
use num_traits::ToPrimitive;
use serde::de::{self, DeserializeSeed, IgnoredAny, MapAccess, SeqAccess, Visitor};
use serde::Deserialize;
use serde_json::Value;

use crate::Limits;
use plenora_core::{PlenoraError, Result};
use crate::{
    column_index, replace_or_append, scalar_as_f64, scalar_as_string, select_rows,
    validate_output_name,
};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Lookup {
    pub column: String,
    pub mapping: BTreeMap<String, Value>,
    #[serde(default)]
    pub default: Value,
    pub output_column: Option<String>,
}

fn value_text(value: &Value) -> String {
    match value {
        Value::String(v) => v.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// Colonna di lookup: ogni valore di `column` e' tradotto via `mapping`
/// (o sostituito da `default`, se non null; altrimenti resta invariato).
///
/// # Errors
///
/// - `Contract`: nome di output non valido (come `validate_output_name`);
/// - `Schema`: colonna `column` assente dallo schema; in piu' gli errori
///   di `scalar_as_string` e `replace_or_append`.
pub fn lookup(batch: &RecordBatch, config: &Lookup) -> Result<RecordBatch> {
    let index = column_index(batch, &config.column)?;
    let source = batch.column(index);
    let output = config.output_column.as_deref().unwrap_or(&config.column);
    validate_output_name(output)?;
    let values = (0..batch.num_rows())
        .map(|row| {
            scalar_as_string(source.as_ref(), row).map(|value| {
                value.map(|value| {
                    config.mapping.get(&value).map_or_else(
                        || {
                            if config.default.is_null() {
                                value
                            } else {
                                value_text(&config.default)
                            }
                        },
                        value_text,
                    )
                })
            })
        })
        .collect::<Result<Vec<_>>>()?;
    replace_or_append(
        batch,
        output,
        DataType::Utf8,
        true,
        Arc::new(StringArray::from(values)),
    )
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum Bins {
    Count(usize),
    Edges(Vec<f64>),
}

const fn default_bins() -> Bins {
    Bins::Count(5)
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Bin {
    pub column: String,
    #[serde(default = "default_bins")]
    pub bins: Bins,
    pub labels: Option<Vec<String>>,
    pub output_column: Option<String>,
}

fn equal_width_edges(numeric: &[Option<f64>], count: usize) -> Result<Vec<f64>> {
    if !(2..=100).contains(&count) {
        return Err(PlenoraError::Contract("bins deve essere tra 2 e 100".into()));
    }
    let values = numeric
        .iter()
        .flatten()
        .copied()
        .filter(|value| value.is_finite())
        .collect::<Vec<_>>();
    let min = values
        .iter()
        .copied()
        .reduce(f64::min)
        .ok_or_else(|| PlenoraError::Schema("bin senza valori numerici".into()))?;
    let max = values.iter().copied().reduce(f64::max).unwrap_or(min);
    let count_f64 = count
        .to_f64()
        .ok_or_else(|| PlenoraError::Contract("numero bin non rappresentabile".into()))?;
    if max.total_cmp(&min) == std::cmp::Ordering::Equal {
        // pandas.cut expands a constant range by 0.1% on both sides
        // (or by 0.001 around zero) before computing equal-width bins.
        let adjustment = if min == 0.0 { 0.001 } else { min.abs() * 0.001 };
        let lower = min - adjustment;
        // Niente mul_add/FMA: la fusione cambia l'arrotondamento IEEE e
        // violerebbe il determinismo bit-esatto (ADR-0001); la forma non
        // fusa e' il contratto numerico.
        #[allow(clippy::suboptimal_flops)]
        let width = (adjustment * 2.0 + 0.0) / count_f64;
        return (0..=count)
            .map(|index| {
                index
                    .to_f64()
                    .map(|position| {
                        // Come sopra: forma non fusa (niente mul_add/FMA).
                        #[allow(clippy::suboptimal_flops)]
                        let edge = width * position + lower;
                        edge
                    })
                    .ok_or_else(|| PlenoraError::Contract("indice bin non rappresentabile".into()))
            })
            .collect();
    }
    let width = (max - min) / count_f64;
    let mut edges = (0..=count)
        .map(|index| {
            index
                .to_f64()
                .map(|position| {
                    if index == count {
                        max
                    } else {
                        // Come sopra: forma non fusa (niente mul_add/FMA).
                        #[allow(clippy::suboptimal_flops)]
                        let edge = width * position + min;
                        edge
                    }
                })
                .ok_or_else(|| PlenoraError::Contract("indice bin non rappresentabile".into()))
        })
        .collect::<Result<Vec<_>>>()?;
    // pandas.cut uses right-closed intervals and expands only the open side.
    edges[0] -= (max - min) * 0.001;
    Ok(edges)
}

/// Discretizza `column` in bin equal-width (`Bins::Count`) o su bordi
/// espliciti (`Bins::Edges`), con etichette opzionali.
///
/// # Errors
///
/// - `Contract`: numero di bin fuori da 2..=100 o non rappresentabile;
///   bordi non strettamente crescenti (o in numero fuori da 3..=101);
///   numero di `labels` diverso dal numero di bin;
/// - `Schema`: colonna `column` assente dallo schema; nessun valore
///   numerico su cui calcolare i bordi; in piu' gli errori di
///   `scalar_as_f64` e `replace_or_append`.
pub fn bin(batch: &RecordBatch, config: &Bin) -> Result<RecordBatch> {
    let index = column_index(batch, &config.column)?;
    let source = batch.column(index);
    let numeric = (0..batch.num_rows())
        .map(|row| scalar_as_f64(source.as_ref(), row))
        .collect::<Result<Vec<_>>>()?;
    let edges = match &config.bins {
        Bins::Count(count) => equal_width_edges(&numeric, *count)?,
        Bins::Edges(edges) => {
            if edges.len() < 3
                || edges.len() > 101
                || edges
                    .windows(2)
                    .any(|v| !matches!(v[0].partial_cmp(&v[1]), Some(std::cmp::Ordering::Less)))
            {
                return Err(PlenoraError::Contract(
                    "bordi bin non strettamente crescenti".into(),
                ));
            }
            edges.clone()
        }
    };
    let count = edges.len() - 1;
    if config
        .labels
        .as_ref()
        .is_some_and(|labels| labels.len() != count)
    {
        return Err(PlenoraError::Contract(
            "numero labels diverso dai bin".into(),
        ));
    }
    let values = numeric
        .into_iter()
        .map(|value| {
            value.and_then(|value| {
                (0..count)
                    .find(|index| {
                        (value > edges[*index] || (*index == 0 && value >= edges[*index]))
                            && value <= edges[*index + 1]
                    })
                    .map(|index| {
                        config.labels.as_ref().map_or_else(
                            || format!("({}, {}]", edges[index], edges[index + 1]),
                            |labels| labels[index].clone(),
                        )
                    })
            })
        })
        .collect::<Vec<_>>();
    let output = config
        .output_column
        .clone()
        .unwrap_or_else(|| format!("{}_bin", config.column));
    replace_or_append(
        batch,
        &output,
        DataType::Utf8,
        true,
        Arc::new(StringArray::from(values)),
    )
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FlattenJson {
    pub column: String,
    #[serde(default)]
    pub prefix: String,
    #[serde(default = "default_level")]
    pub max_level: usize,
    #[serde(default)]
    pub output_columns: Vec<String>,
}
const fn default_level() -> usize {
    1
}

fn flatten(
    value: &Value,
    path: &str,
    depth: usize,
    max: usize,
    out: &mut BTreeMap<String, String>,
) {
    if let Value::Object(object) = value {
        for (key, value) in object {
            let key = if path.is_empty() {
                key.clone()
            } else {
                format!("{path}.{key}")
            };
            if value.is_object() && depth < max {
                flatten(value, &key, depth + 1, max, out);
            } else if !value.is_object() {
                out.insert(key, value_text(value));
            }
        }
    }
}

/// Insiemi di path per il parsing selettivo (`output_columns` esplicite).
///
/// `capture` sono i path esatti da emettere, `ancestors` tutti i prefissi
/// propri a passi di punto (un sotto-albero fuori da entrambi gli insiemi
/// non puo' contenere alcun path richiesto e viene saltato con
/// `IgnoredAny`, senza allocazioni).
#[derive(Default)]
struct JsonTargets {
    capture: HashSet<String>,
    ancestors: HashSet<String>,
}

impl JsonTargets {
    fn from_outputs(outputs: &[String], prefix: &str) -> Self {
        let mut targets = Self::default();
        for output in outputs {
            // Come il codice a valle: gli output senza prefix producono un
            // errore dopo il parsing delle righe; qui bastano path mai
            // presenti nei documenti per non alterare quel comportamento.
            let path = output.strip_prefix(prefix).unwrap_or(output);
            targets.capture.insert(path.to_owned());
            for (index, _) in path.match_indices('.') {
                targets.ancestors.insert(path[..index].to_owned());
            }
        }
        targets
    }
}

/// Colonne path -> valori dense per riga (null dove il path manca).
///
/// Costruite direttamente durante la scansione: evita la `BTreeMap` per
/// riga, la raccolta/ordinamento delle chiavi per cella e la copia dei
/// valori nelle colonne di output dell'implementazione originale.
#[derive(Default)]
struct PathColumns {
    paths: Vec<String>,
    index: HashMap<String, usize>,
    columns: Vec<Vec<Option<String>>>,
}

impl PathColumns {
    fn insert(&mut self, row: usize, path: &str, text: String) {
        let id = if let Some(&id) = self.index.get(path) { id } else {
            let id = self.columns.len();
            self.index.insert(path.to_owned(), id);
            self.paths.push(path.to_owned());
            self.columns.push(Vec::new());
            id
        };
        let column = &mut self.columns[id];
        if column.len() > row {
            // Chiave duplicata nello stesso oggetto: come serde_json,
            // vince l'ultima occorrenza.
            column[row] = Some(text);
        } else {
            column.resize(row, None);
            column.push(Some(text));
        }
    }
}

/// Stato della scansione JSON di una riga (fast path).
///
/// Buffer delle celle emesse dalla riga (path, testo), buffer riusato per
/// il path corrente e flag per le chiavi "ambigue" (vuote o contenenti
/// '.'). Le celle sono riversate nelle colonne solo a riga valida: JSON
/// invalido o radice non oggetto scartano il buffer (l'originale produce
/// una riga vuota). Con chiavi ambigue due derivazioni diverse possono
/// produrre lo stesso path appiattito e l'originale risolve il conflitto
/// iterando le chiavi in ordine lessicografico (`BTreeMap` di `serde_json`:
/// non riproducibile in streaming ordine-documento, quindi il driver
/// ricade sul parsing completo per quella riga.
struct RowFlatten<'a> {
    cells: &'a mut Vec<(String, String)>,
    path: String,
    max: usize,
    targets: Option<&'a JsonTargets>,
    weird_key: bool,
}

impl RowFlatten<'_> {
    fn emit(&mut self, text: String) {
        // Le chiavi duplicate nello stesso oggetto restano nel buffer in
        // ordine di documento: al riversamento vince l'ultima, come in
        // serde_json.
        self.cells.push((self.path.clone(), text));
    }

    fn walk_map<'de, A: MapAccess<'de>>(&mut self, mut map: A, depth: usize) -> std::result::Result<(), A::Error> {
        if depth > self.max {
            // Oggetto oltre max_level: l'originale lo scarta senza
            // emettere nulla (ma il parser valida comunque il contenuto).
            while map.next_entry::<IgnoredAny, IgnoredAny>()?.is_some() {}
            return Ok(());
        }
        let base = self.path.len();
        let empty = self.path.is_empty();
        while let Some(key) = map.next_key::<String>()? {
            if key.is_empty() || key.contains('.') {
                self.weird_key = true;
            }
            // Stessa regola di join dell'originale: niente punto se il
            // path corrente e' vuoto.
            if !empty {
                self.path.push('.');
            }
            self.path.push_str(&key);
            let capture = self.targets.is_none_or(|targets| {
                targets.capture.contains(&self.path[..])
            });
            let descend = capture
                || self.targets.is_none_or(|targets| {
                    targets.ancestors.contains(&self.path[..])
                });
            if descend {
                map.next_value_seed(LeafSeed {
                    row: &mut *self,
                    depth: depth + 1,
                    capture,
                })?;
            } else {
                map.next_value::<IgnoredAny>()?;
            }
            self.path.truncate(base);
        }
        Ok(())
    }
}

/// Seed per il valore associato a una chiave: scalari e array sono emessi
/// (se richiesti) con la stessa resa testuale di `value_text`, gli oggetti
/// sono attraversati da `walk_map`.
struct LeafSeed<'a, 'b> {
    row: &'a mut RowFlatten<'b>,
    depth: usize,
    capture: bool,
}

impl<'de> DeserializeSeed<'de> for LeafSeed<'_, '_> {
    type Value = ();

    fn deserialize<D>(self, deserializer: D) -> std::result::Result<(), D::Error>
    where
        D: de::Deserializer<'de>,
    {
        deserializer.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for LeafSeed<'_, '_> {
    type Value = ();

    fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
        formatter.write_str("un valore JSON")
    }

    fn visit_map<A: MapAccess<'de>>(self, map: A) -> std::result::Result<Self::Value, A::Error> {
        // Un oggetto non e' mai emesso direttamente: si attraversa (il
        // limite di livello e' applicato da walk_map).
        self.row.walk_map(map, self.depth)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, seq: A) -> std::result::Result<Self::Value, A::Error> {
        if self.capture {
            // Stesso Value dell'originale: la serializzazione compatta e'
            // identica per costruzione.
            let value = Value::deserialize(de::value::SeqAccessDeserializer::new(seq))?;
            self.row.emit(value_text(&value));
        } else {
            IgnoredAny::deserialize(de::value::SeqAccessDeserializer::new(seq))?;
        }
        Ok(())
    }

    fn visit_str<E: de::Error>(self, value: &str) -> std::result::Result<Self::Value, E> {
        if self.capture {
            self.row.emit(value.to_owned());
        }
        Ok(())
    }

    fn visit_bool<E: de::Error>(self, value: bool) -> std::result::Result<Self::Value, E> {
        if self.capture {
            self.row.emit(value.to_string());
        }
        Ok(())
    }

    fn visit_i64<E: de::Error>(self, value: i64) -> std::result::Result<Self::Value, E> {
        if self.capture {
            self.row.emit(value.to_string());
        }
        Ok(())
    }

    fn visit_u64<E: de::Error>(self, value: u64) -> std::result::Result<Self::Value, E> {
        if self.capture {
            self.row.emit(value.to_string());
        }
        Ok(())
    }

    fn visit_f64<E: de::Error>(self, value: f64) -> std::result::Result<Self::Value, E> {
        if self.capture {
            // Stessa formattazione di serde_json::Number (ryu), usata da
            // value_text sull'albero Value.
            self.row.emit(Value::from(value).to_string());
        }
        Ok(())
    }

    fn visit_unit<E: de::Error>(self) -> std::result::Result<Self::Value, E> {
        if self.capture {
            self.row.emit(String::new());
        }
        Ok(())
    }
}

/// Seed di radice: solo un oggetto produce path; qualsiasi altra radice
/// (scalare, array, null) da' mappa vuota come l'originale.
struct RootSeed<'a, 'b> {
    row: &'a mut RowFlatten<'b>,
}

impl<'de> DeserializeSeed<'de> for RootSeed<'_, '_> {
    type Value = ();

    fn deserialize<D>(self, deserializer: D) -> std::result::Result<(), D::Error>
    where
        D: de::Deserializer<'de>,
    {
        deserializer.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for RootSeed<'_, '_> {
    type Value = ();

    fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
        formatter.write_str("un valore JSON")
    }

    fn visit_map<A: MapAccess<'de>>(self, map: A) -> std::result::Result<Self::Value, A::Error> {
        self.row.walk_map(map, 0)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> std::result::Result<Self::Value, A::Error> {
        while seq.next_element::<IgnoredAny>()?.is_some() {}
        Ok(())
    }

    fn visit_bool<E: de::Error>(self, _value: bool) -> std::result::Result<Self::Value, E> {
        Ok(())
    }

    fn visit_i64<E: de::Error>(self, _value: i64) -> std::result::Result<Self::Value, E> {
        Ok(())
    }

    fn visit_u64<E: de::Error>(self, _value: u64) -> std::result::Result<Self::Value, E> {
        Ok(())
    }

    fn visit_f64<E: de::Error>(self, _value: f64) -> std::result::Result<Self::Value, E> {
        Ok(())
    }

    fn visit_str<E: de::Error>(self, _value: &str) -> std::result::Result<Self::Value, E> {
        Ok(())
    }

    fn visit_unit<E: de::Error>(self) -> std::result::Result<Self::Value, E> {
        Ok(())
    }
}

/// Appiattisce una riga JSON nelle colonne path -> testo.
///
/// Fast path: parsing streaming con serde (stessa validazione di
/// `from_str::<Value>`, quindi errori, limiti di annidamento e resa
/// testuale identici), senza costruire l'albero `Value` e saltando i
/// sotto-alberi non richiesti. JSON invalido o radice non-oggetto =>
/// nessuna emissione (come l'originale); chiavi vuote o con '.' =>
/// fallback al parsing completo della riga.
fn flatten_row(
    text: &str,
    max: usize,
    targets: Option<&JsonTargets>,
    row_index: usize,
    cells: &mut Vec<(String, String)>,
    out: &mut PathColumns,
) {
    cells.clear();
    let mut row = RowFlatten {
        cells,
        path: String::new(),
        max,
        targets,
        weird_key: false,
    };
    let valid = {
        let mut deserializer = serde_json::Deserializer::from_str(text);
        let parsed = RootSeed { row: &mut row }.deserialize(&mut deserializer);
        parsed.is_ok() && deserializer.end().is_ok()
    };
    if row.weird_key {
        // Fallback: algoritmo originale (ordine di emissione sulle chiavi
        // ordinate, risoluzione dei conflitti inclusa); sostituisce le
        // emissioni del fast path per questa riga.
        let mut map = BTreeMap::new();
        let parsed = serde_json::from_str::<Value>(text)
            .ok()
            .unwrap_or_else(|| Value::Object(serde_json::Map::new()));
        flatten(&parsed, "", 0, max, &mut map);
        for (path, text) in map {
            out.insert(row_index, &path, text);
        }
    } else if valid {
        for (path, text) in row.cells.drain(..) {
            out.insert(row_index, &path, text);
        }
    }
}

/// Appiattisce i documenti JSON di `column` in colonne `prefix + path`
/// (profondita' massima `max_level`).
///
/// # Errors
///
/// - `Contract`: `max_level` oltre 5; totale colonne oltre
///   `limits.max_columns`; nome di output non valido o privo del `prefix`
///   atteso;
/// - `Schema`: colonna `column` assente dallo schema; in piu' gli errori
///   di `scalar_as_string` (colonne non Utf8) e `replace_or_append`.
pub fn flatten_json(
    batch: &RecordBatch,
    config: &FlattenJson,
    limits: &Limits,
) -> Result<RecordBatch> {
    if config.max_level > 5 {
        return Err(PlenoraError::Contract("max_level oltre 5".into()));
    }
    let index = column_index(batch, &config.column)?;
    let source = batch.column(index);
    let prefix = if config.prefix.is_empty() {
        format!("{}_", config.column)
    } else {
        config.prefix.clone()
    };
    let targets = if config.output_columns.is_empty() {
        None
    } else {
        Some(JsonTargets::from_outputs(&config.output_columns, &prefix))
    };
    let mut columns = PathColumns::default();
    let mut cells: Vec<(String, String)> = Vec::new();
    if let Some(values) = source.as_any().downcast_ref::<StringArray>() {
        // Caso comune: documenti utf8 prestati senza copia (identico al
        // to_owned di scalar_as_string, compresa la gestione dei null).
        for (row, text) in values.iter().enumerate() {
            if let Some(text) = text {
                flatten_row(text, config.max_level, targets.as_ref(), row, &mut cells, &mut columns);
            }
        }
    } else {
        for row in 0..batch.num_rows() {
            if let Some(text) = scalar_as_string(source.as_ref(), row)? {
                flatten_row(&text, config.max_level, targets.as_ref(), row, &mut cells, &mut columns);
            }
        }
    }
    // Colonne dense: null dove il path manca nella riga.
    for column in &mut columns.columns {
        column.resize(batch.num_rows(), None);
    }
    let keys: Vec<String> = if config.output_columns.is_empty() {
        let mut paths = columns.paths.clone();
        paths.sort();
        paths.into_iter().map(|key| format!("{prefix}{key}")).collect()
    } else {
        config.output_columns.clone()
    };
    if batch.num_columns().saturating_add(keys.len()) > limits.max_columns {
        return Err(PlenoraError::Contract(
            "flatten_json supera max_columns".into(),
        ));
    }
    let mut result = batch.clone();
    for output in keys {
        validate_output_name(&output)?;
        let path = output.strip_prefix(&prefix).ok_or_else(|| {
            PlenoraError::Contract(format!("output {output} non inizia con prefix"))
        })?;
        let values: Vec<Option<&str>> = match columns.index.get(path) {
            Some(&id) => columns.columns[id]
                .iter()
                .map(|value| value.as_deref())
                .collect(),
            None => vec![None; batch.num_rows()],
        };
        result = replace_or_append(
            &result,
            &output,
            DataType::Utf8,
            true,
            Arc::new(StringArray::from(values)),
        )?;
    }
    Ok(result)
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stat {
    Count,
    Min,
    Max,
    Sum,
    Mean,
    Median,
    Std,
    Var,
    Q25,
    Q75,
}

fn default_stats() -> Vec<Stat> {
    vec![
        Stat::Count,
        Stat::Min,
        Stat::Max,
        Stat::Mean,
        Stat::Median,
        Stat::Std,
    ]
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Statistics {
    pub column: String,
    pub group_by: Option<String>,
    #[serde(default = "default_stats")]
    pub stats: Vec<Stat>,
    #[serde(default)]
    pub output_prefix: String,
}

fn quantile(sorted: &[f64], q: f64) -> Option<f64> {
    if sorted.is_empty() {
        return None;
    }
    let position = q * (sorted.len() - 1).to_f64()?;
    let low = position.floor().to_usize()?;
    let high = position.ceil().to_usize()?;
    Some(if low == high {
        sorted[low]
    } else {
        // Niente mul_add/FMA: la fusione cambia l'arrotondamento IEEE e
        // violerebbe il determinismo bit-esatto (ADR-0001); la forma non
        // fusa e' il contratto numerico.
        #[allow(clippy::suboptimal_flops)]
        let interpolated = (sorted[high] - sorted[low]) * (position - low.to_f64()?) + sorted[low];
        interpolated
    })
}

/// Statistiche di un gruppo calcolate una sola volta: un sort unico (se
/// richiesto da min/max/quantili) e momenti (somma, media, varianza) in
/// singola passata.
///
/// Replica bit per bit la semantica dell'implementazione originale, che
/// ricalcolava sort e momenti per ogni coppia (riga, statistica): stesse
/// operazioni f64 nello stesso ordine (somme con `Iterator::sum`, varianza
/// su `mean`, quantili su copia ordinata con `f64::total_cmp`).
fn group_statistics(values: &[f64], stats: &[Stat]) -> Vec<Option<f64>> {
    let count = values.len().to_f64();
    if values.is_empty() {
        return stats
            .iter()
            .map(|stat| {
                if matches!(stat, Stat::Count) {
                    count
                } else {
                    None
                }
            })
            .collect();
    }
    // L'originale valuta `values.len().to_f64()?` prima di ogni statistica
    // diversa da Count: se fallisce, tutte (tranne Count) sono null.
    let Some(len) = count else {
        return stats.iter().map(|_| None).collect();
    };
    let sum: f64 = values.iter().sum();
    let mean = sum / len;
    let sorted = stats
        .iter()
        .any(|stat| {
            matches!(
                stat,
                Stat::Min | Stat::Max | Stat::Median | Stat::Q25 | Stat::Q75
            )
        })
        .then(|| {
            let mut sorted = values.to_vec();
            sorted.sort_by(f64::total_cmp);
            sorted
        });
    let variance = if values.len() >= 2
        && stats
            .iter()
            .any(|stat| matches!(stat, Stat::Var | Stat::Std))
    {
        (values.len() - 1)
            .to_f64()
            .map(|denominator| {
                values.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / denominator
            })
    } else {
        None
    };
    stats
        .iter()
        .map(|stat| match stat {
            Stat::Count => Some(len),
            Stat::Min => sorted.as_ref().and_then(|sorted| sorted.first().copied()),
            Stat::Max => sorted.as_ref().and_then(|sorted| sorted.last().copied()),
            Stat::Sum => Some(sum),
            Stat::Mean => Some(mean),
            Stat::Median => sorted.as_ref().and_then(|sorted| quantile(sorted, 0.5)),
            Stat::Q25 => sorted.as_ref().and_then(|sorted| quantile(sorted, 0.25)),
            Stat::Q75 => sorted.as_ref().and_then(|sorted| quantile(sorted, 0.75)),
            Stat::Var => variance,
            Stat::Std => variance.map(f64::sqrt),
        })
        .collect()
}

/// Aggiunge le colonne di statistiche `prefix + nome` (count, min, max,
/// ...) sui valori di `column`, opzionalmente per gruppi di `group_by`.
///
/// # Errors
///
/// - `Schema`: colonna `column` o `group_by` assente dallo schema; in
///   piu' gli errori di `scalar_as_string`/`scalar_as_f64` e
///   `replace_or_append`.
pub fn statistics(batch: &RecordBatch, config: &Statistics) -> Result<RecordBatch> {
    let value_index = column_index(batch, &config.column)?;
    let group_index = config
        .group_by
        .as_deref()
        .map(|name| column_index(batch, name))
        .transpose()?;
    // Passata unica con interning delle chiavi: l'originale ricalcolava la
    // chiave (con allocazione) di ogni riga per ciascuna statistica e
    // rieseguiva sort + momenti per ogni coppia (riga, statistica); qui
    // ogni gruppo e' aggregato una sola volta.
    let mut groups: Vec<Vec<f64>> = Vec::new();
    let mut row_group: Vec<usize> = Vec::with_capacity(batch.num_rows());
    if let Some(group_index) = group_index {
        let mut intern: HashMap<Option<String>, usize> = HashMap::new();
        let group_column = batch.column(group_index);
        let value_column = batch.column(value_index);
        for row in 0..batch.num_rows() {
            let key = scalar_as_string(group_column.as_ref(), row)?;
            let value = scalar_as_f64(value_column.as_ref(), row)?;
            let id = *intern.entry(key).or_insert_with(|| {
                groups.push(Vec::new());
                groups.len() - 1
            });
            row_group.push(id);
            if let Some(value) = value {
                groups[id].push(value);
            }
        }
    } else {
        groups.push(Vec::new());
        let value_column = batch.column(value_index);
        for row in 0..batch.num_rows() {
            if let Some(value) = scalar_as_f64(value_column.as_ref(), row)? {
                groups[0].push(value);
            }
            row_group.push(0);
        }
    }
    // Gruppi con tutti i valori null: l'originale non li inserisce nella
    // mappa, quindi ogni statistica (Count incluso) e' null.
    let group_stats: Vec<Vec<Option<f64>>> = groups
        .iter()
        .map(|values| {
            if values.is_empty() {
                vec![None; config.stats.len()]
            } else {
                group_statistics(values, &config.stats)
            }
        })
        .collect();
    let prefix = if config.output_prefix.is_empty() {
        format!("{}_", config.column)
    } else {
        config.output_prefix.clone()
    };
    let mut result = batch.clone();
    for (position, stat) in config.stats.iter().enumerate() {
        let suffix = match stat {
            Stat::Count => "count",
            Stat::Min => "min",
            Stat::Max => "max",
            Stat::Sum => "sum",
            Stat::Mean => "mean",
            Stat::Median => "median",
            Stat::Std => "std",
            Stat::Var => "var",
            Stat::Q25 => "q25",
            Stat::Q75 => "q75",
        };
        let name = format!("{prefix}{suffix}");
        let values = row_group
            .iter()
            .map(|&id| group_stats[id][position])
            .collect::<Vec<_>>();
        result = replace_or_append(
            &result,
            &name,
            DataType::Float64,
            true,
            Arc::new(Float64Array::from(values)),
        )?;
    }
    Ok(result)
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Sample {
    #[serde(default = "default_n")]
    pub n: usize,
    pub fraction: Option<f64>,
    pub random_state: Option<u64>,
    pub stratify_column: Option<String>,
}
const fn default_n() -> usize {
    100
}

fn shuffle(rows: &mut [usize], seed: u64) {
    let mut state = seed.max(1);
    for index in (1..rows.len()).rev() {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        let bound = u64::try_from(index + 1).unwrap_or(u64::MAX);
        let target = usize::try_from(state % bound).unwrap_or(0);
        rows.swap(index, target);
    }
}

/// Campione casuale deterministico delle righe (dimensione `n` o
/// `fraction`, opzionalmente stratificato su `stratify_column`).
///
/// # Errors
///
/// - `Contract`: `fraction` fuori da 0..=1; dimensioni di gruppo, dataset
///   o campione non rappresentabili;
/// - `Schema`: colonna `stratify_column` assente dallo schema; in piu' gli
///   errori di `scalar_as_string` e `select_rows`.
pub fn sample(batch: &RecordBatch, config: &Sample) -> Result<RecordBatch> {
    if config
        .fraction
        .is_some_and(|fraction| !(0.0..=1.0).contains(&fraction))
    {
        return Err(PlenoraError::Contract("fraction fuori 0..1".into()));
    }
    let seed = config.random_state.unwrap_or(0x9e37_79b9_7f4a_7c15);
    let mut selected = Vec::new();
    if let Some(column) = &config.stratify_column {
        let index = column_index(batch, column)?;
        let mut groups: BTreeMap<Option<String>, Vec<usize>> = BTreeMap::new();
        for row in 0..batch.num_rows() {
            groups
                .entry(scalar_as_string(batch.column(index).as_ref(), row)?)
                .or_default()
                .push(row);
        }
        for (offset, rows) in groups.values_mut().enumerate() {
            shuffle(
                rows,
                seed.wrapping_add(u64::try_from(offset).unwrap_or(u64::MAX)),
            );
            let count = match config.fraction {
                None => config.n.saturating_mul(rows.len()) / batch.num_rows().max(1),
                Some(fraction) => (rows
                    .len()
                    .to_f64()
                    .ok_or_else(|| PlenoraError::Contract("gruppo troppo grande".into()))?
                    * fraction)
                    .floor()
                    .to_usize()
                    .ok_or_else(|| PlenoraError::Contract("campione non rappresentabile".into()))?,
            }
            .max(1)
            .min(rows.len());
            selected.extend_from_slice(&rows[..count]);
        }
    } else {
        selected.extend(0..batch.num_rows());
        shuffle(&mut selected, seed);
        let count = match config.fraction {
            None => config.n,
            Some(fraction) => (batch
                .num_rows()
                .to_f64()
                .ok_or_else(|| PlenoraError::Contract("dataset troppo grande".into()))?
                * fraction)
                .round()
                .to_usize()
                .ok_or_else(|| PlenoraError::Contract("campione non rappresentabile".into()))?,
        }
        .min(batch.num_rows());
        selected.truncate(count);
    }
    select_rows(batch, &selected)
}


#[cfg(test)]
mod tests {
    use plenora_core::arrow::array::{Array, Int64Array};
    use plenora_core::arrow::schema::{Field, Schema};

    use super::*;

    // ------------------------------------------------------------------
    // Oracoli: implementazioni pre-ottimizzazione, copiate verbatim per i
    // test di equivalenza byte-identica (schema, valori, null, bit f64,
    // ordine righe, errori).
    // ------------------------------------------------------------------

    fn oracle_statistic(values: &[f64], stat: &Stat) -> Option<f64> {
        if matches!(stat, Stat::Count) {
            return values.len().to_f64();
        }
        if values.is_empty() {
            return None;
        }
        let sum: f64 = values.iter().sum();
        let mean = sum / values.len().to_f64()?;
        let mut sorted = values.to_vec();
        sorted.sort_by(f64::total_cmp);
        Some(match stat {
            Stat::Min => sorted[0],
            Stat::Max => *sorted.last()?,
            Stat::Sum => sum,
            Stat::Mean => mean,
            Stat::Median => quantile(&sorted, 0.5)?,
            Stat::Q25 => quantile(&sorted, 0.25)?,
            Stat::Q75 => quantile(&sorted, 0.75)?,
            Stat::Var | Stat::Std => {
                if values.len() < 2 {
                    return None;
                }
                let variance = values.iter().map(|v| (v - mean).powi(2)).sum::<f64>()
                    / (values.len() - 1).to_f64()?;
                if matches!(stat, Stat::Std) {
                    variance.sqrt()
                } else {
                    variance
                }
            }
            Stat::Count => unreachable!(),
        })
    }

    fn oracle_statistics(batch: &RecordBatch, config: &Statistics) -> Result<RecordBatch> {
        let value_index = column_index(batch, &config.column)?;
        let group_index = config
            .group_by
            .as_deref()
            .map(|name| column_index(batch, name))
            .transpose()?;
        let mut groups: HashMap<Option<String>, Vec<f64>> = HashMap::new();
        for row in 0..batch.num_rows() {
            let key = group_index
                .map(|index| scalar_as_string(batch.column(index).as_ref(), row))
                .transpose()?
                .flatten();
            if let Some(value) = scalar_as_f64(batch.column(value_index).as_ref(), row)? {
                groups.entry(key).or_default().push(value);
            }
        }
        let prefix = if config.output_prefix.is_empty() {
            format!("{}_", config.column)
        } else {
            config.output_prefix.clone()
        };
        let mut result = batch.clone();
        for stat in &config.stats {
            let suffix = match stat {
                Stat::Count => "count",
                Stat::Min => "min",
                Stat::Max => "max",
                Stat::Sum => "sum",
                Stat::Mean => "mean",
                Stat::Median => "median",
                Stat::Std => "std",
                Stat::Var => "var",
                Stat::Q25 => "q25",
                Stat::Q75 => "q75",
            };
            let name = format!("{prefix}{suffix}");
            let values = (0..batch.num_rows())
                .map(|row| {
                    let key = group_index
                        .map(|index| scalar_as_string(batch.column(index).as_ref(), row))
                        .transpose()?
                        .flatten();
                    Ok(groups.get(&key).and_then(|values| oracle_statistic(values, stat)))
                })
                .collect::<Result<Vec<_>>>()?;
            result = replace_or_append(
                &result,
                &name,
                DataType::Float64,
                true,
                Arc::new(Float64Array::from(values)),
            )?;
        }
        Ok(result)
    }

    fn oracle_flatten_json(
        batch: &RecordBatch,
        config: &FlattenJson,
        limits: &Limits,
    ) -> Result<RecordBatch> {
        if config.max_level > 5 {
            return Err(PlenoraError::Contract("max_level oltre 5".into()));
        }
        let index = column_index(batch, &config.column)?;
        let source = batch.column(index);
        let prefix = if config.prefix.is_empty() {
            format!("{}_", config.column)
        } else {
            config.prefix.clone()
        };
        let rows = (0..batch.num_rows())
            .map(|row| {
                let parsed = scalar_as_string(source.as_ref(), row)?
                    .and_then(|text| serde_json::from_str::<Value>(&text).ok())
                    .unwrap_or_else(|| Value::Object(serde_json::Map::new()));
                let mut values = BTreeMap::new();
                flatten(&parsed, "", 0, config.max_level, &mut values);
                Ok(values)
            })
            .collect::<Result<Vec<BTreeMap<String, String>>>>()?;
        let keys: Vec<String> = if config.output_columns.is_empty() {
            let mut keys = rows
                .iter()
                .flat_map(|row| row.keys().cloned())
                .collect::<Vec<_>>();
            keys.sort();
            keys.dedup();
            keys.into_iter()
                .map(|key| format!("{prefix}{key}"))
                .collect()
        } else {
            config.output_columns.clone()
        };
        if batch.num_columns().saturating_add(keys.len()) > limits.max_columns {
            return Err(PlenoraError::Contract(
                "flatten_json supera max_columns".into(),
            ));
        }
        let mut result = batch.clone();
        for output in keys {
            validate_output_name(&output)?;
            let path = output.strip_prefix(&prefix).ok_or_else(|| {
                PlenoraError::Contract(format!("output {output} non inizia con prefix"))
            })?;
            let values = rows
                .iter()
                .map(|row| row.get(path).cloned())
                .collect::<Vec<_>>();
            result = replace_or_append(
                &result,
                &output,
                DataType::Utf8,
                true,
                Arc::new(StringArray::from(values)),
            )?;
        }
        Ok(result)
    }

    // ------------------------------------------------------------------
    // Helper di equivalenza rigorosa: schema, null mask, bit f64, ordine.
    // ------------------------------------------------------------------

    fn assert_batch_identical(fast: &RecordBatch, oracle: &RecordBatch) {
        assert_eq!(fast.schema(), oracle.schema(), "schema diverso");
        assert_eq!(fast.num_rows(), oracle.num_rows(), "righe diverse");
        for (index, field) in fast.schema().fields().iter().enumerate() {
            let left = fast.column(index);
            let right = oracle.column(index);
            if field.data_type() == &DataType::Float64 {
                let left = left
                    .as_any()
                    .downcast_ref::<Float64Array>()
                    .expect("colonna f64");
                let right = right
                    .as_any()
                    .downcast_ref::<Float64Array>()
                    .expect("colonna f64");
                assert_eq!(left.len(), right.len(), "lunghezza colonna {}", field.name());
                for row in 0..left.len() {
                    assert_eq!(
                        left.is_null(row),
                        right.is_null(row),
                        "null diverso: colonna {} riga {row}",
                        field.name()
                    );
                    if !left.is_null(row) {
                        assert_eq!(
                            left.value(row).to_bits(),
                            right.value(row).to_bits(),
                            "bit f64 diversi: colonna {} riga {row}",
                            field.name()
                        );
                    }
                }
            } else {
                assert_eq!(left, right, "colonna {} diversa", field.name());
            }
        }
    }

    fn assert_statistics_equiv(batch: &RecordBatch, config: &Statistics) {
        let fast = statistics(batch, config).map_err(|err| format!("{err:?}"));
        let oracle = oracle_statistics(batch, config).map_err(|err| format!("{err:?}"));
        match (fast, oracle) {
            (Ok(fast), Ok(oracle)) => assert_batch_identical(&fast, &oracle),
            (Err(fast), Err(oracle)) => assert_eq!(fast, oracle),
            (fast, oracle) => panic!(
                "statistics: fast/oracolo divergono (fast ok={}, oracolo ok={})",
                fast.is_ok(),
                oracle.is_ok()
            ),
        }
    }

    fn assert_flatten_equiv(batch: &RecordBatch, config: &FlattenJson, limits: &Limits) {
        let fast = flatten_json(batch, config, limits).map_err(|err| format!("{err:?}"));
        let oracle = oracle_flatten_json(batch, config, limits).map_err(|err| format!("{err:?}"));
        match (fast, oracle) {
            (Ok(fast), Ok(oracle)) => assert_batch_identical(&fast, &oracle),
            (Err(fast), Err(oracle)) => assert_eq!(fast, oracle),
            (fast, oracle) => panic!(
                "flatten_json: fast/oracolo divergono (fast ok={}, oracolo ok={})",
                fast.is_ok(),
                oracle.is_ok()
            ),
        }
    }

    fn stats_batch(values: Vec<Option<f64>>, groups: Vec<Option<&str>>) -> RecordBatch {
        RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("num", DataType::Float64, true),
                Field::new("grp", DataType::Utf8, true),
            ])),
            vec![
                Arc::new(Float64Array::from(values)),
                Arc::new(StringArray::from(groups)),
            ],
        )
        .expect("batch statistics")
    }

    fn all_stats() -> Vec<Stat> {
        vec![
            Stat::Count,
            Stat::Min,
            Stat::Max,
            Stat::Sum,
            Stat::Mean,
            Stat::Median,
            Stat::Std,
            Stat::Var,
            Stat::Q25,
            Stat::Q75,
        ]
    }

    #[test]
    fn statistics_oracle_grouped_edge_values() {
        // NaN, -0.0/+0.0 (bit della somma), gruppi di un solo elemento
        // (var/std null, quantili banali), valori null, chiave null,
        // gruppo con tutti i valori null.
        let batch = stats_batch(
            vec![
                Some(1.0),
                None,
                Some(2.5),
                Some(f64::NAN),
                Some(-0.0),
                Some(0.0),
                Some(7.5),
                Some(-0.0),
                Some(42.0),
                None,
                Some(f64::INFINITY),
                Some(f64::NEG_INFINITY),
            ],
            vec![
                Some("a"),
                Some("a"),
                Some("a"),
                Some("b"),
                Some("b"),
                Some("b"),
                None,
                Some("solo"),
                Some("solo"),
                Some("vuoto"),
                Some("inf"),
                Some("inf"),
            ],
        );
        for stats in [
            all_stats(),
            vec![Stat::Sum, Stat::Q25],
            vec![Stat::Var, Stat::Std],
            vec![Stat::Mean, Stat::Mean],
            vec![],
            vec![Stat::Count],
        ] {
            for (group_by, prefix) in [(Some("grp"), ""), (None, "s_")] {
                assert_statistics_equiv(
                    &batch,
                    &Statistics {
                        column: "num".into(),
                        group_by: group_by.map(str::to_owned),
                        stats: stats.clone(),
                        output_prefix: prefix.into(),
                    },
                );
            }
        }
    }

    #[test]
    fn statistics_oracle_types_and_errors() {
        // Colonna valori int64 e chiave int64 (chiave via to_string).
        let integers = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("n", DataType::Int64, true),
                Field::new("k", DataType::Int64, true),
            ])),
            vec![
                Arc::new(Int64Array::from(vec![Some(1), None, Some(3), Some(-2)])),
                Arc::new(Int64Array::from(vec![Some(7), Some(7), None, Some(8)])),
            ],
        )
        .expect("batch int");
        assert_statistics_equiv(
            &integers,
            &Statistics {
                column: "n".into(),
                group_by: Some("k".into()),
                stats: all_stats(),
                output_prefix: String::new(),
            },
        );
        // Colonna valori utf8 numerica (parsing con virgola) e errori.
        let strings = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("s", DataType::Utf8, true),
                Field::new("g", DataType::Utf8, true),
            ])),
            vec![
                Arc::new(StringArray::from(vec![Some("1,5"), Some("x"), None])),
                Arc::new(StringArray::from(vec![Some("a"), Some("a"), Some("a")])),
            ],
        )
        .expect("batch utf8");
        assert_statistics_equiv(
            &strings,
            &Statistics {
                column: "s".into(),
                group_by: Some("g".into()),
                stats: all_stats(),
                output_prefix: String::new(),
            },
        );
        // Batch vuoto e colonna mancante.
        let empty = stats_batch(vec![], vec![]);
        assert_statistics_equiv(
            &empty,
            &Statistics {
                column: "num".into(),
                group_by: Some("grp".into()),
                stats: all_stats(),
                output_prefix: String::new(),
            },
        );
        assert_statistics_equiv(
            &empty,
            &Statistics {
                column: "manca".into(),
                group_by: Some("grp".into()),
                stats: all_stats(),
                output_prefix: String::new(),
            },
        );
    }

    #[test]
    fn statistics_group_statistics_matches_oracle_directly() {
        // Confronto diretto per gruppo: stesse sequenze f64, stessi bit.
        let groups: Vec<Vec<f64>> = vec![
            vec![],
            vec![-0.0],
            vec![0.0],
            vec![-0.0, -0.0],
            vec![1.0],
            vec![f64::NAN, 1.0],
            vec![1.0, 2.0, 3.0, 4.0],
            vec![f64::INFINITY, f64::NEG_INFINITY, 5.0],
            vec![1e308, 1e308, -1e308],
        ];
        for values in groups {
            let fast = group_statistics(&values, &all_stats());
            let oracle = all_stats()
                .iter()
                .map(|stat| oracle_statistic(&values, stat))
                .collect::<Vec<_>>();
            assert_eq!(fast.len(), oracle.len());
            for (index, (fast, oracle)) in fast.iter().zip(&oracle).enumerate() {
                match (fast, oracle) {
                    (Some(fast), Some(oracle)) => assert_eq!(
                        fast.to_bits(),
                        oracle.to_bits(),
                        "stat {index} gruppo {values:?}"
                    ),
                    (fast, oracle) => assert_eq!(fast, oracle, "stat {index} gruppo {values:?}"),
                }
            }
        }
    }

    // ------------------------------------------------------------------
    // flatten_json
    // ------------------------------------------------------------------

    fn docs_batch(docs: Vec<Option<&str>>) -> RecordBatch {
        RecordBatch::try_new(
            Arc::new(Schema::new(vec![Field::new("doc", DataType::Utf8, true)])),
            vec![Arc::new(StringArray::from(docs))],
        )
        .expect("batch json")
    }

    fn flatten_config(max_level: usize, output_columns: Vec<String>) -> FlattenJson {
        FlattenJson {
            column: "doc".into(),
            prefix: String::new(),
            max_level,
            output_columns,
        }
    }

    fn nested_doc(depth: usize) -> String {
        let mut doc = "1".to_owned();
        for _ in 0..depth {
            doc = format!("{{\"a\":{doc}}}");
        }
        doc
    }

    #[test]
    fn flatten_json_oracle_document_matrix() {
        let limits = Limits::default();
        let docs = docs_batch(vec![
            Some(r#"{"a":1,"b":{"c":2.5,"d":{"e":"x","f":[1,2,3]}},"g":"s"}"#),
            Some(r#"{"a":null}"#),
            None,
            Some(r#"{"a":"#),          // JSON invalido
            Some(r#""radice stringa""#), // radice scalare
            Some("[1,2]"),            // radice array
            Some("42"),
            Some("null"),
            Some(r#"{"a":1} junk"#),  // trailing garbage
            Some(r#"{"n":1.0,"m":1e2,"big":18446744073709551615,"neg":-0.0,"f":0.1}"#),
            Some(r#"{"s":"a\nbé","t":"\ud83d\ude00"}"#),
            Some(r#"{"a":1,"a":2}"#), // chiave duplicata: vince l'ultima
            Some(r#"{"a.b":1,"a":{"b":2}}"#), // chiave con punto: fallback
            Some(r#"{"":{"x":1},"y":2}"#),    // chiave vuota: fallback
            Some(r#"{"arr":[{"k":1,"z":2},{"k":3}]}"#), // oggetti in array
            Some("{}"),
            Some(r#"  { "a" : 1 }  "#),
            Some(&nested_doc(127)),
            Some(&nested_doc(128)),
            Some(&nested_doc(129)),
            Some(&nested_doc(200)),
        ]);
        for max_level in 0..=5 {
            assert_flatten_equiv(&docs, &flatten_config(max_level, vec![]), &limits);
        }
        for output_columns in [
            vec!["doc_a".to_owned()],
            vec!["doc_b.d.e".to_owned(), "doc_g".to_owned(), "doc_manca".to_owned()],
            vec!["doc_b".to_owned()],       // path che punta a un oggetto
            vec!["doc_arr".to_owned()],     // path che punta a un array
            vec!["doc_a.a".to_owned(), "doc_a.a.a".to_owned()], // path annidati
            vec!["doc_a".to_owned(), "doc_a".to_owned()], // output duplicati
        ] {
            for max_level in 0..=3 {
                assert_flatten_equiv(&docs, &flatten_config(max_level, output_columns.clone()), &limits);
            }
        }
        // batch vuoto, discovery e selettivo
        let empty = docs_batch(vec![]);
        assert_flatten_equiv(&empty, &flatten_config(2, vec![]), &limits);
        assert_flatten_equiv(&empty, &flatten_config(2, vec!["doc_a".to_owned()]), &limits);
    }

    #[test]
    #[allow(clippy::cast_precision_loss)] // valori grezzi del generatore, la precisione non conta
    fn flatten_json_oracle_generated_corpus() {
        // Corpus deterministico (xorshift, seed 42) con forme miste,
        // valori numerici vari, escape e qualche chiave ambigua per
        // esercitare anche il fallback.
        let limits = Limits::default();
        let mut state = 42_u64;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        let docs = (0..2_000)
            .map(|row| {
                let doc = match next() % 8 {
                    0 => format!(
                        "{{\"a\":{},\"b\":{{\"c\":{:.3},\"d\":{{\"e\":\"{:x}\",\"f\":[1,2,3]}}}},\"g\":\"{:x}\"}}",
                        next() % 1000,
                        (next() % 1000) as f64 / 7.0,
                        next(),
                        next()
                    ),
                    1 => format!("{{\"a\":1e{},\"n\":-0.0,\"u\":{}}}", next() % 300, next()),
                    2 => format!("{{\"s\":\"\\t\\u00e8{:x}\",\"v\":null}}", next()),
                    3 => format!("{{\"a.b\":{},\"a\":{{\"b\":{}}}}}", next() % 10, next() % 10),
                    4 => format!("{{\"dup\":1,\"dup\":{},\"arr\":[{{\"y\":1,\"x\":2}}]}}", next() % 10),
                    5 if row % 17 == 0 => "non json".to_owned(),
                    5 => format!("{{\"liv1\":{{\"liv2\":{{\"liv3\":{{\"liv4\":{}}}}}}}}}", next() % 100),
                    6 => format!("\"{}\"" , next()),
                    _ => "{}".to_owned(),
                };
                Some(doc)
            })
            .collect::<Vec<_>>();
        let docs = docs_batch(docs.iter().map(|doc| doc.as_deref()).collect());
        for max_level in 0..=5 {
            assert_flatten_equiv(&docs, &flatten_config(max_level, vec![]), &limits);
        }
        assert_flatten_equiv(
            &docs,
            &flatten_config(
                3,
                vec![
                    "doc_a".into(),
                    "doc_b.c".into(),
                    "doc_b.d.e".into(),
                    "doc_liv1.liv2.liv3".into(),
                    "doc_assente".into(),
                ],
            ),
            &limits,
        );
    }

    #[test]
    fn flatten_json_oracle_errors_and_limits() {
        let docs = docs_batch(vec![Some(r#"{"a":1}"#)]);
        let limits = Limits::default();
        // max_level oltre 5
        assert_flatten_equiv(&docs, &flatten_config(6, vec![]), &limits);
        // output senza prefix
        assert_flatten_equiv(
            &docs,
            &FlattenJson {
                column: "doc".into(),
                prefix: "j_".into(),
                max_level: 1,
                output_columns: vec!["altro".into()],
            },
            &limits,
        );
        // prefix custom
        assert_flatten_equiv(
            &docs,
            &FlattenJson {
                column: "doc".into(),
                prefix: "j_".into(),
                max_level: 1,
                output_columns: vec!["j_a".into()],
            },
            &limits,
        );
        // max_columns superato
        let tight = Limits {
            max_columns: 1,
            ..Limits::default()
        };
        assert_flatten_equiv(&docs, &flatten_config(1, vec![]), &tight);
        // colonna mancante
        assert_flatten_equiv(
            &docs,
            &FlattenJson {
                column: "manca".into(),
                prefix: String::new(),
                max_level: 1,
                output_columns: vec![],
            },
            &limits,
        );
        // colonna non utf8 (f64): scalar_as_string la rende testo,
        // il parsing fallisce e la riga e' vuota in entrambe le versioni
        let floats = RecordBatch::try_new(
            Arc::new(Schema::new(vec![Field::new("doc", DataType::Float64, true)])),
            vec![Arc::new(Float64Array::from(vec![Some(1.5), None]))],
        )
        .expect("batch f64");
        assert_flatten_equiv(&floats, &flatten_config(1, vec![]), &limits);
    }
}
