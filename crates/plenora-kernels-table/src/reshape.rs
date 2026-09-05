use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;
use std::hash::BuildHasherDefault;
use std::sync::Arc;

use num_traits::ToPrimitive;
use plenora_core::arrow::array::{
    builder::StringBuilder, Array, ArrayRef, BooleanArray, Float64Array, Int64Array, ListArray,
    RecordBatch, StringArray, StructArray, UInt32Array, UInt64Array,
};
use plenora_core::arrow::schema::{DataType, Field, Schema};
use serde::Deserialize;

use crate::float64_source::Float64Source;
use crate::hashing::FastHasher;
use crate::Limits;
use crate::{column_index, replace_or_append, scalar_as_string, select_rows, validate_output_name};
use plenora_core::{PlenoraError, Result};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Melt {
    pub id_columns: Vec<String>,
    #[serde(default)]
    pub value_columns: Vec<String>,
    #[serde(default = "default_variable")]
    pub var_name: String,
    #[serde(default = "default_value")]
    pub value_name: String,
    #[serde(default = "default_type_policy")]
    pub type_policy: HeterogeneousTypePolicy,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HeterogeneousTypePolicy {
    Reject,
    String,
}

const fn default_type_policy() -> HeterogeneousTypePolicy {
    HeterogeneousTypePolicy::Reject
}
fn default_variable() -> String {
    "variable".into()
}
fn default_value() -> String {
    "value".into()
}

// ---------------------------------------------------------------------------
// Fast path di `melt`/`pivot`.
//
// `TextColumn` e `Float64Source` (quest'ultimo condiviso col resto del
// crate) preparano una sola volta il downcast Arrow per colonna e iterano sui
// valori nativi, producendo gli STESSI byte dei
// percorsi scalari originali (`scalar_as_string` / `scalar_as_f64_rounded`):
// stesso formato Display per numerici e booleani (NaN -> "NaN",
// -0.0 -> "-0" distinto da "0"), stessi null, stessi errori. I tipi fuori
// dal fast path ricadono sul percorso generico, invariato.
// ---------------------------------------------------------------------------

/// Sorgente testuale tipizzata per `melt` (policy string) e `pivot`
/// (chiavi composte, valori pivot, concat).
enum TextColumn<'a> {
    Utf8(&'a StringArray),
    Int64(&'a Int64Array),
    Float64(&'a Float64Array),
    Boolean(&'a BooleanArray),
    UInt64(&'a UInt64Array),
    Generic(&'a ArrayRef),
}

impl<'a> TextColumn<'a> {
    fn new(array: &'a ArrayRef) -> Self {
        if let Some(values) = array.as_any().downcast_ref::<StringArray>() {
            return Self::Utf8(values);
        }
        if let Some(values) = array.as_any().downcast_ref::<Int64Array>() {
            return Self::Int64(values);
        }
        if let Some(values) = array.as_any().downcast_ref::<Float64Array>() {
            return Self::Float64(values);
        }
        if let Some(values) = array.as_any().downcast_ref::<BooleanArray>() {
            return Self::Boolean(values);
        }
        if let Some(values) = array.as_any().downcast_ref::<UInt64Array>() {
            return Self::UInt64(values);
        }
        Self::Generic(array)
    }

    /// Scrive in `out` gli stessi byte di `scalar_as_string` per `row`;
    /// restituisce `false` (senza scrivere) se il valore e' null.
    fn write_value(&self, row: usize, out: &mut String) -> Result<bool> {
        match self {
            Self::Utf8(values) => {
                if values.is_null(row) {
                    return Ok(false);
                }
                out.push_str(values.value(row));
            }
            Self::Int64(values) => {
                if values.is_null(row) {
                    return Ok(false);
                }
                // `fmt::Write` su `String` e' infallibile; l'errore e'
                // comunque propagato come Internal, mai ignorato (R6.5).
                write!(out, "{}", values.value(row))
                    .map_err(|_| PlenoraError::Internal("fmt su String".into()))?;
            }
            Self::Float64(values) => {
                if values.is_null(row) {
                    return Ok(false);
                }
                // `fmt::Write` su `String` e' infallibile; l'errore e'
                // comunque propagato come Internal, mai ignorato (R6.5).
                write!(out, "{}", values.value(row))
                    .map_err(|_| PlenoraError::Internal("fmt su String".into()))?;
            }
            Self::Boolean(values) => {
                if values.is_null(row) {
                    return Ok(false);
                }
                // `fmt::Write` su `String` e' infallibile; l'errore e'
                // comunque propagato come Internal, mai ignorato (R6.5).
                write!(out, "{}", values.value(row))
                    .map_err(|_| PlenoraError::Internal("fmt su String".into()))?;
            }
            Self::UInt64(values) => {
                if values.is_null(row) {
                    return Ok(false);
                }
                // `fmt::Write` su `String` e' infallibile; l'errore e'
                // comunque propagato come Internal, mai ignorato (R6.5).
                write!(out, "{}", values.value(row))
                    .map_err(|_| PlenoraError::Internal("fmt su String".into()))?;
            }
            Self::Generic(array) => {
                let Some(value) = scalar_as_string(array.as_ref(), row)? else {
                    return Ok(false);
                };
                out.push_str(&value);
            }
        }
        Ok(true)
    }
}

/// Colonna di chiave composta (usata da `pivot` e `table_diff`): tag di
/// tipo precomputato una sola volta + valore tipizzato.
///
/// Scrive gli STESSI byte di `composite_key` (mantenuta come oracolo dei
/// test).
struct PivotKeyColumn<'a> {
    tag: String,
    source: TextColumn<'a>,
}

impl<'a> PivotKeyColumn<'a> {
    fn new(array: &'a ArrayRef) -> Self {
        Self {
            tag: array.data_type().to_string(),
            source: TextColumn::new(array),
        }
    }

    fn write_key(&self, row: usize, key: &mut String, value: &mut String) -> Result<()> {
        key.push_str(&self.tag);
        key.push('\u{1e}');
        value.clear();
        if self.source.write_value(row, value)? {
            key.push('1');
            // `fmt::Write` su `String` e' infallibile; l'errore e'
            // comunque propagato come Internal, mai ignorato (R6.5).
            write!(key, "{}", value.len())
                .map_err(|_| PlenoraError::Internal("fmt su String".into()))?;
            key.push(':');
            key.push_str(value);
        } else {
            key.push('0');
        }
        key.push('\u{1f}');
        Ok(())
    }
}

/// Risolve i due nomi di output di `melt` (`variable` e `value`) evitando le
/// collisioni con lo schema di input **e fra loro**.
///
/// Punto d'ingresso unico per il kernel e per l'analisi del contratto: se le
/// due parti risolvessero i nomi con algoritmi separati, il contratto
/// dichiarato e lo schema prodotto potrebbero divergere. E' gia' successo con
/// una copia dell'algoritmo per ciascuna.
///
/// # Errors
///
/// Come [`crate::resolve_output_names`]: nome richiesto non valido, oppure
/// nessuno dei suffissi ammessi produce un nome insieme libero e valido.
pub fn resolve_melt_names<'a>(
    occupati: impl IntoIterator<Item = &'a str>,
    config: &Melt,
) -> Result<[String; 2]> {
    let risolti = crate::resolve_output_names(
        occupati,
        &[config.var_name.as_str(), config.value_name.as_str()],
    )?;
    <[String; 2]>::try_from(risolti)
        .map_err(|_| PlenoraError::Internal("melt: il risolutore deve restituire due nomi".into()))
}

/// Trasforma le `value_columns` da wide a long: per ogni riga, una riga per
/// colonna valore.
///
/// Con `value_columns` vuoto si usano tutte le colonne non id. Tipi omogenei:
/// concatenazione nativa; tipi eterogenei solo con `type_policy='string'`.
///
/// # Errors
///
/// - `InvalidPlan`: nessuna colonna valore; colonne valore eterogenee senza
///   `type_policy='string'`; nome di output non valido o collisione di
///   naming non risolvibile. Tutti decisi in PREPARAZIONE, prima di
///   qualunque controllo di risorsa: un piano invalido resta tale
///   qualunque sia il budget.
/// - `Schema`: colonna id/value assente; sul percorso testuale, tipo non
///   convertibile o timezone Arrow non valida.
/// - `ResourceLimit`: overflow o righe di output oltre `max_rows`, stima
///   dell'output oltre `max_governed_memory_bytes`, valore testuale oltre
///   `max_string_bytes`.
// Pipeline lineare wide->long (preparazione, indici, take sulle colonne id,
// costruzione delle colonne variable/value con fast path per tipo): lunga
// per costruzione, uno spezzone artificiale peggiorerebbe la leggibilita'.
#[allow(clippy::too_many_lines)]
pub fn melt(batch: &RecordBatch, config: &Melt, limits: &Limits) -> Result<RecordBatch> {
    let id_indices = config
        .id_columns
        .iter()
        .map(|name| column_index(batch, name))
        .collect::<Result<Vec<_>>>()?;
    let value_indices = if config.value_columns.is_empty() {
        (0..batch.num_columns())
            .filter(|index| !id_indices.contains(index))
            .collect::<Vec<_>>()
    } else {
        config
            .value_columns
            .iter()
            .map(|name| column_index(batch, name))
            .collect::<Result<Vec<_>>>()?
    };
    if value_indices.is_empty() {
        return Err(PlenoraError::InvalidPlan("melt senza value_columns".into()));
    }

    // ---------------------------------------------------------------------
    // PREPARAZIONE. Tutto cio' che si decide guardando SOLO configurazione e
    // schema si decide qui, prima di ogni controllo di risorsa e prima delle
    // allocazioni PROPORZIONALI ai dati o all'output — gli indici di
    // ripetizione, i `take`, la colonna dei nomi. Non «prima di qualunque
    // allocazione»: gli indici delle colonne, l'insieme dei nomi occupati e
    // le stringhe dei nomi risolti sono gia' stati allocati, e sono
    // proporzionali allo SCHEMA, non alle righe.
    //
    // L'ordine conta due volte:
    //
    // 1. un piano invalido deve restare `invalid_plan` QUALUNQUE sia il
    //    budget. Mettendo il rifiuto delle colonne eterogenee con
    //    `type_policy = "reject"` in fondo, dopo il tetto sulle righe e
    //    dopo la stima, con un budget stretto uscirebbe prima un
    //    `resource_limit`, e chi lo legge andrebbe ad alzare un budget per
    //    un piano che non sarebbe comunque eseguibile;
    // 2. rifiutare dopo aver allocato costa la memoria che il rifiuto deve
    //    risparmiare.
    let tipo_valore = batch.column(value_indices[0]).data_type().clone();
    let omogeneo = value_indices
        .iter()
        .all(|index| batch.column(*index).data_type() == &tipo_valore);
    if !omogeneo {
        match config.type_policy {
            HeterogeneousTypePolicy::Reject => {
                return Err(PlenoraError::InvalidPlan(
                    "melt: value_columns eterogenee; impostare type_policy='string' per la conversione esplicita".into(),
                ));
            }
            HeterogeneousTypePolicy::String => {
                // Tipo E timezone: entrambi stanno nello schema. Risolvere la
                // timezone a ogni riga durante la scansione fa fallire
                // l'operazione dopo le allocazioni, quando la timezone non e'
                // valida, pur essendo nota fin da qui.
                for index in &value_indices {
                    crate::validate_text_convertible(
                        batch.column(*index).data_type(),
                        batch.schema().field(*index).name(),
                    )
                    .map_err(|errore| PlenoraError::Schema(format!("melt: {errore}")))?;
                }
            }
        }
    }
    // I due nomi di output si risolvono INSIEME e in sequenza: risolverli
    // indipendentemente lascia passare `var_name = "v"` e
    // `value_name = "v_1"` su un input che contiene `v`, perche' il primo
    // diventa `v_1` e il secondo non collide con nulla. I nomi RICHIESTI sono
    // distinti — il controllo di contratto non se ne accorge — e l'output
    // esce con due colonne omonime.
    let schema_ingresso = batch.schema();
    let [var_name, value_name] = resolve_melt_names(
        schema_ingresso
            .fields()
            .iter()
            .map(|campo| campo.name().as_str()),
        config,
    )?;

    let output_rows = batch
        .num_rows()
        .checked_mul(value_indices.len())
        .ok_or_else(|| PlenoraError::ResourceLimit("overflow righe melt".into()))?;
    if output_rows > limits.max_rows {
        return Err(PlenoraError::ResourceLimit("melt supera max_rows".into()));
    }
    // MODELLO: l'output ha le colonne id RIPETUTE una volta per colonna
    // valore, piu' due colonne nuove — `variable`, che ripete il NOME della
    // colonna di provenienza, e `value`, larga quanto la piu' larga delle
    // colonne valore. Misurare la larghezza media dell'intero batch
    // d'ingresso ignorerebbe entrambe: con nomi di colonna lunghi la sola
    // `variable` puo' superare tutto il resto.
    let byte_id: usize = id_indices
        .iter()
        .map(|index| crate::column_bytes_per_row(batch.column(*index).as_ref()))
        .try_fold(0_usize, |totale, larghezza| {
            totale.checked_add(larghezza).ok_or_else(|| {
                PlenoraError::ResourceLimit(
                    "melt: larghezza di riga non rappresentabile: stima non affidabile".into(),
                )
            })
        })?;
    // La larghezza della colonna `value` dipende dal PERCORSO. Con colonne
    // omogenee l'output conserva il tipo e la larghezza e' quella misurata;
    // con colonne eterogenee e `type_policy = "string"` i valori vengono
    // CONVERTITI IN TESTO, e la rappresentazione decimale puo' essere molto
    // piu' larga di quella binaria — un `Int64` da otto byte diventa fino a
    // venti caratteri. E' memoria del risultato, non un temporaneo: il
    // modello deve conoscerla, altrimenti sottostima proprio il caso peggiore.
    //
    // `tipo_valore` e `omogeneo` sono gia' stati decisi in preparazione: qui
    // si riusano, cosi' la scelta del percorso e quella della stima non
    // possono divergere.
    let byte_valore = value_indices
        .iter()
        .map(|index| {
            let colonna = batch.column(*index);
            let misurato = crate::column_bytes_per_row(colonna.as_ref());
            if omogeneo {
                misurato
            } else {
                // Larghezza testuale MISURATA sull'array, non dedotta dal
                // tipo: per una `Dictionary` il valore vive una volta sola
                // nel dizionario e l'output `Utf8` lo materializza per ogni
                // riga. Guardare il `DataType` non basta.
                crate::text_bytes_per_row(colonna.as_ref())
            }
        })
        .max()
        .unwrap_or(0);
    let byte_nome = value_indices
        .iter()
        .map(|index| batch.schema().field(*index).name().len())
        .max()
        .unwrap_or(0)
        // Offset piu' validita' della colonna Utf8 che li contiene.
        .saturating_add(crate::type_bytes_floor(&DataType::Utf8));
    let byte_per_riga = byte_id
        .checked_add(byte_valore)
        .and_then(|somma| somma.checked_add(byte_nome))
        .ok_or_else(|| {
            PlenoraError::ResourceLimit(
                "melt: larghezza di riga non rappresentabile: stima non affidabile".into(),
            )
        })?;
    crate::preflight_output_bytes("melt", output_rows, byte_per_riga, limits)?;
    // Fast path: indici di ripetizione materializzati una sola volta come
    // UInt32 e `take` SOLO sulle colonne id: replicare via `select_rows`
    // l'intero batch includerebbe le value_columns, poi scartate. Stesso controllo di overflow di `select_rows`.
    let row_count = u32::try_from(batch.num_rows())
        .map_err(|_| PlenoraError::ResourceLimit("indice riga oltre u32".into()))?;
    let mut repeated_indices = Vec::with_capacity(output_rows);
    for _ in 0..value_indices.len() {
        repeated_indices.extend(0..row_count);
    }
    let repeated_indices = UInt32Array::from(repeated_indices);
    let mut fields = id_indices
        .iter()
        .map(|index| batch.schema().field(*index).as_ref().clone())
        .collect::<Vec<_>>();
    let mut columns = id_indices
        .iter()
        .map(|index| {
            plenora_core::arrow::select::take::take(
                batch.column(*index).as_ref(),
                &repeated_indices,
                None,
            )
            .map_err(PlenoraError::from)
        })
        .collect::<Result<Vec<_>>>()?;
    fields.push(Field::new(&var_name, DataType::Utf8, false));
    // Fast path: nomi di colonna presi in prestito dallo schema, senza un
    // clone di String per riga.
    let schema = batch.schema();
    let mut variables: Vec<&str> = Vec::with_capacity(output_rows);
    for index in &value_indices {
        variables.extend(std::iter::repeat_n(
            schema.field(*index).name().as_str(),
            batch.num_rows(),
        ));
    }
    columns.push(Arc::new(StringArray::from(variables)));
    // Gia' calcolati sopra per il modello della stima: qui si riusano, cosi'
    // la decisione del percorso e quella della stima non possono divergere.
    let value_type = tipo_valore;
    if omogeneo {
        let arrays = value_indices
            .iter()
            .map(|index| batch.column(*index).as_ref())
            .collect::<Vec<_>>();
        fields.push(Field::new(&value_name, value_type, true));
        columns.push(plenora_core::arrow::select::concat::concat(&arrays)?);
    } else if matches!(config.type_policy, HeterogeneousTypePolicy::String) {
        // Fast path: downcast una volta per colonna e loop tipizzato sulle
        // righe; stessi byte, stessi null, stesso ordine di scansione e
        // stesso controllo max_string_bytes del percorso scalare originale.
        let mut builder = StringBuilder::with_capacity(
            output_rows,
            output_rows.saturating_mul(8).min(64 * 1024 * 1024),
        );
        let mut text = String::new();
        for index in &value_indices {
            let source = TextColumn::new(batch.column(*index));
            for row in 0..batch.num_rows() {
                text.clear();
                if source.write_value(row, &mut text)? {
                    if text.len() > limits.max_string_bytes {
                        return Err(PlenoraError::ResourceLimit(
                            "melt: valore testuale oltre max_string_bytes".into(),
                        ));
                    }
                    builder.append_value(&text);
                } else {
                    builder.append_null();
                }
            }
        }
        fields.push(Field::new(&value_name, DataType::Utf8, true));
        columns.push(Arc::new(builder.finish()));
    } else {
        // Irraggiungibile: la preparazione rifiuta le colonne eterogenee con
        // `type_policy = "reject"` prima delle allocazioni proporzionali
        // ai dati. Resta come
        // difesa esplicita — il gate R6 vieta `unreachable!` — e come
        // invariante nostra, non come piano sbagliato del chiamante.
        return Err(PlenoraError::Internal(
            "melt: percorso eterogeneo raggiunto dopo la preparazione".into(),
        ));
    }
    Ok(RecordBatch::try_new(
        Arc::new(Schema::new(fields)),
        columns,
    )?)
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PivotAgg {
    First,
    Last,
    Max,
    Min,
    Sum,
    Mean,
    Count,
    Concat,
}
const fn default_pivot_agg() -> PivotAgg {
    PivotAgg::First
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pivot {
    pub index_col: String,
    #[serde(rename = "pivot_col")]
    pub column: String,
    pub value_col: String,
    #[serde(default = "default_pivot_agg")]
    pub aggr_func: PivotAgg,
    #[serde(default)]
    pub mapping: BTreeMap<String, String>,
}

// Dispatcher esaustivo per funzione di aggregazione: ogni braccio tiene
// insieme riduzione e costruzione della colonna di output.
#[allow(clippy::too_many_lines)]
fn pivot_column(
    source: &ArrayRef,
    groups: &[Option<&Vec<usize>>],
    function: &PivotAgg,
) -> Result<(DataType, ArrayRef)> {
    Ok(match function {
        PivotAgg::First | PivotAgg::Last => {
            let indices = groups
                .iter()
                .map(|rows| {
                    rows.and_then(|rows| {
                        if matches!(function, PivotAgg::First) {
                            rows.first()
                        } else {
                            rows.last()
                        }
                    })
                    .copied()
                    .map(u32::try_from)
                    .transpose()
                    .map_err(|_| PlenoraError::ResourceLimit("indice pivot oltre u32".into()))
                })
                .collect::<Result<Vec<_>>>()?;
            (
                source.data_type().clone(),
                plenora_core::arrow::select::take::take(
                    source.as_ref(),
                    &UInt32Array::from(indices),
                    None,
                )?,
            )
        }
        PivotAgg::Count => {
            let values = groups
                .iter()
                .map(|rows| {
                    rows.map(|rows| {
                        i64::try_from(
                            rows.iter()
                                .filter(|row| !crate::is_logically_null(source.as_ref(), **row))
                                .count(),
                        )
                        .map_err(|_| {
                            PlenoraError::ResourceLimit("conteggio pivot oltre i64".into())
                        })
                    })
                    .transpose()
                })
                .collect::<Result<Vec<_>>>()?;
            (DataType::Int64, Arc::new(Int64Array::from(values)))
        }
        PivotAgg::Concat => {
            // Fast path: downcast una volta per colonna valore; stessi byte
            // (valori uniti da ",", null saltati) del percorso scalare.
            let text = TextColumn::new(source);
            let mut value = String::new();
            let mut joined = String::new();
            let values = groups
                .iter()
                .map(|rows| {
                    rows.map(|rows| {
                        joined.clear();
                        let mut first = true;
                        for row in rows {
                            value.clear();
                            if text.write_value(*row, &mut value)? {
                                if first {
                                    first = false;
                                } else {
                                    joined.push(',');
                                }
                                joined.push_str(&value);
                            }
                        }
                        Ok(joined.clone())
                    })
                    .transpose()
                })
                .collect::<Result<Vec<_>>>()?;
            (DataType::Utf8, Arc::new(StringArray::from(values)))
        }
        PivotAgg::Sum | PivotAgg::Mean | PivotAgg::Min | PivotAgg::Max => {
            // Fast path: riduzione in streaming sulle righe del gruppo nello
            // STESSO ordine del Vec<f64> originale. Il seme della somma e'
            // -0.0, identico a `Iterator::sum` per f64 (fold(-0.0, +)):
            // cosi' anche i segni di zero restano bit-identici
            // ([-0.0] -> -0.0, [+0.0] -> +0.0). f64::min/f64::max
            // passo-passo danno gli stessi bit di reduce, NaN incluso.
            let numeric = Float64Source::new(source);
            let values = groups
                .iter()
                .map(|rows| {
                    let Some(rows) = rows else { return Ok(None) };
                    let mut sum = -0.0_f64;
                    let mut extremum = 0.0_f64;
                    let mut count = 0_usize;
                    for row in *rows {
                        let Some(value) = numeric.value(*row)? else {
                            continue;
                        };
                        if count == 0 {
                            extremum = value;
                        } else if matches!(function, PivotAgg::Min) {
                            extremum = f64::min(extremum, value);
                        } else {
                            extremum = f64::max(extremum, value);
                        }
                        sum += value;
                        count += 1;
                    }
                    if count == 0 {
                        return Ok(None);
                    }
                    Ok(Some(match function {
                        PivotAgg::Sum => sum,
                        PivotAgg::Mean => {
                            sum / count.to_f64().ok_or_else(|| {
                                PlenoraError::ResourceLimit(
                                    "gruppo pivot non rappresentabile".into(),
                                )
                            })?
                        }
                        PivotAgg::Min | PivotAgg::Max => extremum,
                        _ => {
                            return Err(PlenoraError::Internal(
                                "funzione pivot non numerica nel ramo numerico".into(),
                            ));
                        }
                    }))
                })
                .collect::<Result<Vec<_>>>()?;
            (DataType::Float64, Arc::new(Float64Array::from(values)))
        }
    })
}

/// Pivot delle righe in colonne: una riga per chiave indice, una colonna per
/// valore pivot, celle aggregate con `aggr_func`.
///
/// `index_col` accetta piu' colonne separate da virgola; `mapping` rinomina
/// (e filtra) i valori pivot nelle colonne di output.
///
/// # Errors
///
/// - `Schema`: colonna indice/pivot/valore assente;
/// - `ResourceLimit`: chiavi o colonne di output oltre i limiti `max_rows`/
///   `max_columns`, nome di colonna di output non valido, oppure gli errori
///   di conversione/indice di `pivot_column`.
///
/// Un intero oltre 2^53 **non** e' un errore: le aggregazioni numeriche di
/// `pivot` producono un `Float64` per contratto, quindi la conversione
/// arrotonda
/// (errori-e-limiti.md#arrotondamento-nelle-operazioni-a-risultato-float64).
// Pipeline lineare (una passata di raggruppamento, ordinamento di chiavi e
// pivot, take sulle colonne indice, materializzazione delle colonne pivot):
// lunga per costruzione, uno spezzone artificiale peggiorerebbe la
// leggibilita'.
#[allow(clippy::too_many_lines)]
pub fn pivot(batch: &RecordBatch, config: &Pivot, limits: &Limits) -> Result<RecordBatch> {
    let index_names = config
        .index_col
        .split(',')
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .collect::<Vec<_>>();
    let index_indices = index_names
        .iter()
        .map(|name| column_index(batch, name))
        .collect::<Result<Vec<_>>>()?;
    let pivot_index = column_index(batch, &config.column)?;
    let value_index = column_index(batch, &config.value_col)?;
    // Fast path: UNA passata sulle
    // righe (il percorso generico ne fa due, ricalcolando le chiavi
    // composte riga per riga), con chiavi scritte in un buffer riusato e
    // gruppi di celle indicizzati dagli interi (chiave, pivot) invece che da
    // stringhe formattate ad ogni lookup. Ordini di output identici:
    // chiavi e pivot ordinati lessicograficamente come nel BTreeMap del
    // percorso generico, righe di gruppo in ordine crescente,
    // rappresentante = prima riga incontrata per chiave.
    let key_columns = index_indices
        .iter()
        .map(|index| PivotKeyColumn::new(batch.column(*index)))
        .collect::<Vec<_>>();
    let pivot_source = TextColumn::new(batch.column(pivot_index));
    let mut key = String::new();
    let mut scratch = String::new();
    let mut key_ids: HashMap<String, usize> = HashMap::new();
    let mut representatives: Vec<usize> = Vec::new();
    let mut pivot_ids: HashMap<String, usize> = HashMap::new();
    let mut cells: HashMap<(usize, usize), Vec<usize>> = HashMap::new();
    for row in 0..batch.num_rows() {
        key.clear();
        for column in &key_columns {
            column.write_key(row, &mut key, &mut scratch)?;
        }
        // `copied()` chiude subito il prestito di `key_ids`: la closure di
        // default deve poterlo mutare (insert) — `unwrap_or_else` diretto su
        // `get` non passerebbe il borrow check.
        let key_id = key_ids.get(key.as_str()).copied().unwrap_or_else(|| {
            let id = representatives.len();
            key_ids.insert(key.clone(), id);
            representatives.push(row);
            id
        });
        scratch.clear();
        if !pivot_source.write_value(row, &mut scratch)? {
            continue;
        }
        if config.mapping.is_empty() || config.mapping.contains_key(scratch.as_str()) {
            // Come sopra: `copied()` chiude il prestito prima dell'insert.
            let pivot_id = pivot_ids.get(scratch.as_str()).copied().unwrap_or_else(|| {
                let id = pivot_ids.len();
                pivot_ids.insert(scratch.clone(), id);
                id
            });
            cells.entry((key_id, pivot_id)).or_default().push(row);
        }
    }
    let mut sorted_keys = key_ids.into_iter().collect::<Vec<_>>();
    sorted_keys.sort_by(|left, right| left.0.cmp(&right.0));
    let mut sorted_pivots = pivot_ids.into_iter().collect::<Vec<_>>();
    sorted_pivots.sort_by(|left, right| left.0.cmp(&right.0));
    if sorted_keys.len() > limits.max_rows
        || index_indices.len().saturating_add(sorted_pivots.len()) > limits.max_columns
    {
        return Err(PlenoraError::ResourceLimit(
            "pivot supera i limiti di output".into(),
        ));
    }
    // Fast path: `take` SOLO sulle colonne indice, invece di replicare
    // l'intero batch. Stesso controllo di overflow di
    // `select_rows`.
    let representative_indices = sorted_keys
        .iter()
        .map(|(_, id)| {
            u32::try_from(representatives[*id])
                .map_err(|_| PlenoraError::ResourceLimit("indice riga oltre u32".into()))
        })
        .collect::<Result<Vec<_>>>()?;
    let representative_indices = UInt32Array::from(representative_indices);
    let mut fields = index_indices
        .iter()
        .map(|index| batch.schema().field(*index).as_ref().clone())
        .collect::<Vec<_>>();
    let mut columns = index_indices
        .iter()
        .map(|index| {
            plenora_core::arrow::select::take::take(
                batch.column(*index).as_ref(),
                &representative_indices,
                None,
            )
            .map_err(PlenoraError::from)
        })
        .collect::<Result<Vec<_>>>()?;
    let value_source = batch.column(value_index);
    for (pivot_value, pivot_id) in &sorted_pivots {
        let output = config
            .mapping
            .get(pivot_value)
            .cloned()
            .unwrap_or_else(|| pivot_value.clone());
        validate_output_name(&output)?;
        let grouped_rows = sorted_keys
            .iter()
            .map(|(_, key_id)| cells.get(&(*key_id, *pivot_id)))
            .collect::<Vec<_>>();
        let (data_type, values) = pivot_column(value_source, &grouped_rows, &config.aggr_func)?;
        fields.push(Field::new(&output, data_type, true));
        columns.push(values);
    }
    Ok(RecordBatch::try_new(
        Arc::new(Schema::new(fields)),
        columns,
    )?)
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Transpose {
    pub id_column: Option<String>,
    #[serde(default)]
    pub output_columns: Vec<String>,
    #[serde(default = "default_type_policy")]
    pub type_policy: HeterogeneousTypePolicy,
}

/// Traspone il batch: le colonne dati diventano righe, le righe colonne.
///
/// La prima colonna elenca i nomi delle colonne dati; con `id_column` i nomi
/// delle colonne di output arrivano dai suoi valori. Tipi eterogenei solo
/// con `type_policy='string'`; batch senza righe restituito invariato.
///
/// # Errors
///
/// - `Schema`: colonna `id_column` assente;
/// - `ResourceLimit`: righe/colonne di output oltre i limiti, colonne dati
///   eterogenee senza `type_policy='string'`, valore testuale oltre
///   `max_string_bytes`, indice oltre u64, nome di colonna di output non
///   valido.
// Pipeline lineare (schema di output, fast path omogeneo via concat+take o
// ripiego testuale, una colonna per riga): lunga per costruzione, uno
// spezzone artificiale peggiorerebbe la leggibilita'.
#[allow(clippy::too_many_lines)]
pub fn transpose(batch: &RecordBatch, config: &Transpose, limits: &Limits) -> Result<RecordBatch> {
    if batch.num_rows() == 0 {
        return Ok(batch.clone());
    }
    let id_index = config
        .id_column
        .as_deref()
        .map(|name| column_index(batch, name))
        .transpose()?;
    let data_indices = (0..batch.num_columns())
        .filter(|index| Some(*index) != id_index)
        .collect::<Vec<_>>();
    let output_columns = batch.num_rows().saturating_add(1);
    if data_indices.len() > limits.max_rows || output_columns > limits.max_columns {
        return Err(PlenoraError::ResourceLimit(
            "transpose supera i limiti".into(),
        ));
    }
    let first_name = config.id_column.clone().unwrap_or_else(|| "col_0".into());
    let mut fields = vec![Field::new(&first_name, DataType::Utf8, false)];
    let mut columns: Vec<Arc<dyn plenora_core::arrow::array::Array>> =
        vec![Arc::new(StringArray::from(
            data_indices
                .iter()
                .map(|index| batch.schema().field(*index).name().clone())
                .collect::<Vec<_>>(),
        ))];
    let data_type = data_indices.first().map_or(DataType::Utf8, |index| {
        batch.column(*index).data_type().clone()
    });
    let homogeneous = data_indices
        .iter()
        .all(|index| batch.column(*index).data_type() == &data_type);
    if !homogeneous && matches!(config.type_policy, HeterogeneousTypePolicy::Reject) {
        return Err(PlenoraError::InvalidPlan(
            "transpose: colonne eterogenee; impostare type_policy='string' per la conversione esplicita".into(),
        ));
    }
    let combined = if homogeneous && !data_indices.is_empty() {
        let arrays = data_indices
            .iter()
            .map(|index| batch.column(*index).as_ref())
            .collect::<Vec<_>>();
        Some(plenora_core::arrow::select::concat::concat(&arrays)?)
    } else {
        None
    };
    for row in 0..batch.num_rows() {
        let default_name = id_index
            .map(|index| scalar_as_string(batch.column(index).as_ref(), row))
            .transpose()?
            .flatten()
            .unwrap_or_else(|| format!("col_{}", row + 1));
        let name = config
            .output_columns
            .get(row)
            .filter(|name| !name.is_empty())
            .cloned()
            .unwrap_or(default_name);
        validate_output_name(&name)?;
        if let Some(combined) = &combined {
            fields.push(Field::new(&name, data_type.clone(), true));
            let indices = data_indices
                .iter()
                .enumerate()
                .map(|(position, _)| {
                    position
                        .checked_mul(batch.num_rows())
                        .and_then(|base| base.checked_add(row))
                        .and_then(|index| u64::try_from(index).ok())
                        .map(Some)
                        .ok_or_else(|| {
                            PlenoraError::ResourceLimit("indice transpose oltre u64".into())
                        })
                })
                .collect::<Result<Vec<_>>>()?;
            columns.push(plenora_core::arrow::select::take::take(
                combined.as_ref(),
                &UInt64Array::from(indices),
                None,
            )?);
        } else {
            fields.push(Field::new(&name, DataType::Utf8, true));
            let values = data_indices
                .iter()
                .map(|index| scalar_as_string(batch.column(*index).as_ref(), row))
                .collect::<Result<Vec<_>>>()?;
            if values
                .iter()
                .flatten()
                .any(|value| value.len() > limits.max_string_bytes)
            {
                return Err(PlenoraError::ResourceLimit(
                    "transpose: valore testuale oltre max_string_bytes".into(),
                ));
            }
            columns.push(Arc::new(StringArray::from(values)));
        }
    }
    Ok(RecordBatch::try_new(
        Arc::new(Schema::new(fields)),
        columns,
    )?)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EmptyListPolicy {
    Drop,
    Null,
}

const fn default_empty_list_policy() -> EmptyListPolicy {
    EmptyListPolicy::Null
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Explode {
    pub column: String,
    pub output_column: Option<String>,
    #[serde(default = "default_empty_list_policy")]
    pub empty_policy: EmptyListPolicy,
}

/// Espande una colonna `List` in una riga per elemento (stile pandas
/// explode).
///
/// Liste null o vuote producono una riga con valore null con `empty_policy`
/// `Null`. Il token legacy `Drop` e' rifiutato: l'omissione richiede un nodo
/// esplicito di selezione o partizione. Il nome di output e' `output_column` o
/// la colonna stessa.
///
/// # Errors
///
/// - `Schema`: colonna assente o non di tipo List, offset List negativo;
/// - `ResourceLimit`: righe di output oltre `max_rows`, indice elemento oltre
///   u32;
/// - `InvalidPlan`: nome di output non valido, `empty_policy=drop`; inoltre
///   gli errori di `replace_or_append`.
pub fn explode(batch: &RecordBatch, config: &Explode, limits: &Limits) -> Result<RecordBatch> {
    if matches!(config.empty_policy, EmptyListPolicy::Drop) {
        return Err(PlenoraError::InvalidPlan(
            "explode.empty_policy=drop e' remediation implicita; usare un nodo esplicito di selezione o partizione accepted/rejected".into(),
        ));
    }
    let index = column_index(batch, &config.column)?;
    let list = batch
        .column(index)
        .as_any()
        .downcast_ref::<ListArray>()
        .ok_or_else(|| PlenoraError::Schema("explode richiede una colonna List".into()))?;
    let output_name = config.output_column.as_deref().unwrap_or(&config.column);
    validate_output_name(output_name)?;
    let offsets = list.value_offsets();
    let mut rows = Vec::new();
    let mut values = Vec::new();
    for row in 0..batch.num_rows() {
        let start = usize::try_from(offsets[row])
            .map_err(|_| PlenoraError::Schema("offset List negativo".into()))?;
        let end = usize::try_from(offsets[row + 1])
            .map_err(|_| PlenoraError::Schema("offset List negativo".into()))?;
        if list.is_null(row) || start == end {
            if matches!(config.empty_policy, EmptyListPolicy::Null) {
                rows.push(row);
                values.push(None);
            }
        } else {
            for value in start..end {
                rows.push(row);
                values.push(Some(u32::try_from(value).map_err(|_| {
                    PlenoraError::ResourceLimit("indice explode oltre u32".into())
                })?));
            }
        }
        if rows.len() > limits.max_rows {
            return Err(PlenoraError::ResourceLimit(
                "explode supera max_rows".into(),
            ));
        }
    }
    let repeated = select_rows(batch, &rows)?;
    let output = plenora_core::arrow::select::take::take(
        list.values().as_ref(),
        &UInt32Array::from(values),
        None,
    )?;
    replace_or_append(&repeated, output_name, list.value_type(), true, output)
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Unnest {
    pub column: String,
    #[serde(default)]
    pub prefix: String,
    #[serde(default = "default_true")]
    pub drop_source: bool,
}

const fn default_true() -> bool {
    true
}

/// Espande una colonna `Struct` in colonne figlie (con prefisso `prefix`).
///
/// Con `drop_source` la colonna sorgente e' rimossa; le colonne figlie sono
/// nullable e replicate sugli indici delle righe in cui lo struct e' valido.
///
/// # Errors
///
/// - `Schema`: colonna assente o non di tipo Struct, collisione tra il nome
///   di una colonna figlia e una colonna esistente;
/// - `ResourceLimit`: colonne di output oltre `max_columns`.
/// - `InvalidPlan`: nome di colonna
///   figlia non valido.
pub fn unnest(batch: &RecordBatch, config: &Unnest, limits: &Limits) -> Result<RecordBatch> {
    let index = column_index(batch, &config.column)?;
    let structure = batch
        .column(index)
        .as_any()
        .downcast_ref::<StructArray>()
        .ok_or_else(|| PlenoraError::Schema("unnest richiede una colonna Struct".into()))?;
    let projected_columns = batch
        .num_columns()
        .saturating_sub(usize::from(config.drop_source))
        .saturating_add(structure.num_columns());
    if projected_columns > limits.max_columns {
        return Err(PlenoraError::ResourceLimit(
            "unnest supera max_columns".into(),
        ));
    }
    let mut fields = Vec::with_capacity(projected_columns);
    let mut columns = Vec::with_capacity(projected_columns);
    let mut names = std::collections::HashSet::new();
    for (position, field) in batch.schema().fields().iter().enumerate() {
        if position == index && config.drop_source {
            continue;
        }
        names.insert(field.name().clone());
        fields.push(field.as_ref().clone());
        columns.push(batch.column(position).clone());
    }
    // L'indice di riga si converte in modo FALLIBILE.
    //
    // `u32::try_from(row).ok()` renderebbe `None` oltre `u32::MAX`, e `None`
    // in un array di indici e' un indice NULLO: i figli di una struct non
    // nulla sarebbero sostituiti da null senza che nulla lo segnali.
    // I limiti di riga sono `u64` e niente vincola un batch a `u32::MAX`
    // righe, quindi il caso non e' escluso per costruzione. E' la stessa
    // classe della conversione silenziosa dei conteggi di gruppo.
    let mut parent_indices = Vec::with_capacity(batch.num_rows());
    for row in 0..batch.num_rows() {
        if structure.is_null(row) {
            parent_indices.push(None);
            continue;
        }
        let indice = u32::try_from(row).map_err(|_| {
            PlenoraError::ResourceLimit(format!(
                "unnest: il batch supera {} righe, oltre il dominio degli indici \
                 di riga a 32 bit",
                u32::MAX
            ))
        })?;
        parent_indices.push(Some(indice));
    }
    let parent_indices = UInt32Array::from(parent_indices);
    for (child, field) in structure.columns().iter().zip(structure.fields()) {
        let name = format!("{}{}", config.prefix, field.name());
        validate_output_name(&name)?;
        if !names.insert(name.clone()) {
            return Err(PlenoraError::Schema(format!(
                "unnest: collisione colonna {name}"
            )));
        }
        fields.push(field.as_ref().clone().with_name(name).with_nullable(true));
        columns.push(plenora_core::arrow::select::take::take(
            child.as_ref(),
            &parent_indices,
            None,
        )?);
    }
    Ok(RecordBatch::try_new(
        Arc::new(Schema::new_with_metadata(
            fields,
            batch.schema().metadata().clone(),
        )),
        columns,
    )?)
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TableDiff {
    pub left_keys: Vec<String>,
    pub right_keys: Vec<String>,
    #[serde(default)]
    pub compare_columns: Vec<String>,
    #[serde(default = "default_no")]
    pub include_unchanged: String,
    #[serde(default = "default_separator")]
    pub separator: String,
}

struct DiffRow {
    old_row: Option<usize>,
    new_row: Option<usize>,
    status: &'static str,
    changed: Option<String>,
    old_values: Option<String>,
}
fn default_no() -> String {
    "no".into()
}
fn default_separator() -> String {
    "#".into()
}

/// Codifica la chiave composta di `table_diff` per `row` in `key` (buffer
/// riusato tra le righe), con gli stessi byte di `composite_key`.
fn encode_diff_key(
    columns: &[PivotKeyColumn],
    row: usize,
    key: &mut String,
    value: &mut String,
) -> Result<()> {
    key.clear();
    for column in columns {
        column.write_key(row, key, value)?;
    }
    Ok(())
}

type DiffKeyMap = HashMap<String, usize, FastHasher>;

/// Chiave composta in forma testuale: e' l'oracolo dei test di equivalenza
/// di `table_diff` e `pivot`, e non e' il percorso che quei due usano.
#[cfg(test)]
fn composite_key(batch: &RecordBatch, indices: &[usize], row: usize) -> Result<String> {
    let mut key = String::new();
    for index in indices {
        key.push_str(batch.column(*index).data_type().to_string().as_str());
        key.push('\u{1e}');
        match scalar_as_string(batch.column(*index).as_ref(), row)? {
            Some(value) => {
                key.push('1');
                key.push_str(&value.len().to_string());
                key.push(':');
                key.push_str(&value);
            }
            None => key.push('0'),
        }
        key.push('\u{1f}');
    }
    Ok(key)
}

fn diff_values(left: &ArrayRef, right: &ArrayRef, rows: &[DiffRow]) -> Result<ArrayRef> {
    if left.data_type() != right.data_type() {
        return Err(PlenoraError::Schema(format!(
            "table_diff richiede tipi Arrow identici, trovati {} e {}",
            left.data_type(),
            right.data_type()
        )));
    }
    let combined = plenora_core::arrow::select::concat::concat(&[left.as_ref(), right.as_ref()])?;
    let indices = rows
        .iter()
        .map(|row| {
            let index = if let Some(new_row) = row.new_row {
                left.len().saturating_add(new_row)
            } else {
                row.old_row.ok_or_else(|| {
                    PlenoraError::InvalidPlan("riga table_diff senza sorgente".into())
                })?
            };
            u32::try_from(index)
                .map(Some)
                .map_err(|_| PlenoraError::ResourceLimit("indice table_diff oltre u32".into()))
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(plenora_core::arrow::select::take::take(
        combined.as_ref(),
        &UInt32Array::from(indices),
        None,
    )?)
}

/// Confronto riga per riga tra due batch allineati sulle chiavi: stati
/// ADDED/DELETED/MODIFIED/UNCHANGED.
///
/// Le colonne confrontate sono `compare_columns` o, se vuota, le colonne
/// sinistre non chiave presenti anche a destra. Output: colonne chiave e
/// confrontate, piu' `_diff_status`, `_diff_columns`, `_diff_old_values`.
///
/// # Errors
///
/// - `InvalidPlan`: chiavi vuote o cardinalita' diversa tra i due lati, chiavi
///   duplicate in uno dei due input, righe/colonne di output oltre i limiti;
/// - `Schema`: colonna chiave o di confronto assente, tipi Arrow non
///   identici tra i due lati su una stessa colonna.
#[allow(clippy::too_many_lines)] // Diff phases remain adjacent to preserve auditability.
pub fn table_diff(
    left: &RecordBatch,
    right: &RecordBatch,
    config: &TableDiff,
    limits: &Limits,
) -> Result<RecordBatch> {
    if config.left_keys.is_empty() || config.left_keys.len() != config.right_keys.len() {
        return Err(PlenoraError::InvalidPlan(
            "chiavi table_diff non valide".into(),
        ));
    }
    let left_keys = config
        .left_keys
        .iter()
        .map(|name| column_index(left, name))
        .collect::<Result<Vec<_>>>()?;
    let right_keys = config
        .right_keys
        .iter()
        .map(|name| column_index(right, name))
        .collect::<Result<Vec<_>>>()?;
    let compare = if config.compare_columns.is_empty() {
        left.schema()
            .fields()
            .iter()
            .map(|field| field.name().clone())
            .filter(|name| {
                !config.left_keys.contains(name) && right.schema().index_of(name).is_ok()
            })
            .collect::<Vec<_>>()
    } else {
        config.compare_columns.clone()
    };
    let left_compare = compare
        .iter()
        .map(|name| column_index(left, name))
        .collect::<Result<Vec<_>>>()?;
    let right_compare = compare
        .iter()
        .map(|name| column_index(right, name))
        .collect::<Result<Vec<_>>>()?;
    // Fast path: chiavi codificate una sola
    // volta per colonna (`PivotKeyColumn`, stessi byte di `composite_key`) in
    // un buffer riusato; mappe hash con hasher FxHash-style (mai iterate,
    // solo lookup/insert, quindi equivalenti alle BTreeMap originali);
    // confronto valori via `TextColumn` senza `scalar_as_string` per cella.
    // Stesso ordine righe in output (sorgente sinistra, poi righe solo a
    // destra), stessi errori.
    let left_key_columns = left_keys
        .iter()
        .map(|index| PivotKeyColumn::new(left.column(*index)))
        .collect::<Vec<_>>();
    let right_key_columns = right_keys
        .iter()
        .map(|index| PivotKeyColumn::new(right.column(*index)))
        .collect::<Vec<_>>();
    let left_text_columns = left_compare
        .iter()
        .map(|index| TextColumn::new(left.column(*index)))
        .collect::<Vec<_>>();
    let right_text_columns = right_compare
        .iter()
        .map(|index| TextColumn::new(right.column(*index)))
        .collect::<Vec<_>>();
    let mut key = String::new();
    let mut text = String::new();
    let mut old =
        DiffKeyMap::with_capacity_and_hasher(left.num_rows(), BuildHasherDefault::default());
    let mut new =
        DiffKeyMap::with_capacity_and_hasher(right.num_rows(), BuildHasherDefault::default());
    for row in 0..left.num_rows() {
        encode_diff_key(&left_key_columns, row, &mut key, &mut text)?;
        if old.insert(key.clone(), row).is_some() {
            return Err(PlenoraError::InvalidPlan(
                "chiavi duplicate nella tabella sinistra".into(),
            ));
        }
    }
    for row in 0..right.num_rows() {
        encode_diff_key(&right_key_columns, row, &mut key, &mut text)?;
        if new.insert(key.clone(), row).is_some() {
            return Err(PlenoraError::InvalidPlan(
                "chiavi duplicate nella tabella destra".into(),
            ));
        }
    }
    // Preserve source order: old rows first, then new-only rows. Sorting the
    // encoded key would place nulls first and reorder otherwise stable data.
    let mut matched = Vec::with_capacity(old.len().saturating_add(new.len()));
    for row in 0..left.num_rows() {
        encode_diff_key(&left_key_columns, row, &mut key, &mut text)?;
        matched.push((Some(row), new.get(key.as_str()).copied()));
    }
    for row in 0..right.num_rows() {
        encode_diff_key(&right_key_columns, row, &mut key, &mut text)?;
        if !old.contains_key(key.as_str()) {
            matched.push((None, Some(row)));
        }
    }
    let mut rows = Vec::new();
    let mut before = String::new();
    let mut after = String::new();
    for (old_row, new_row) in matched {
        let (status, changed, old_values) = match (old_row, new_row) {
            (None, Some(_)) => ("ADDED", None, None),
            (Some(_), None) => ("DELETED", None, None),
            (Some(old_row), Some(new_row)) => {
                let mut changed = Vec::new();
                let mut old_values = Vec::new();
                for ((name, left_column), right_column) in compare
                    .iter()
                    .zip(&left_text_columns)
                    .zip(&right_text_columns)
                {
                    before.clear();
                    after.clear();
                    let has_before = left_column.write_value(old_row, &mut before)?;
                    let has_after = right_column.write_value(new_row, &mut after)?;
                    if has_before != has_after || (has_before && before != after) {
                        changed.push(name.clone());
                        old_values.push(before.clone());
                    }
                }
                if changed.is_empty() {
                    ("UNCHANGED", None, None)
                } else {
                    (
                        "MODIFIED",
                        Some(changed.join(&config.separator)),
                        Some(old_values.join(&config.separator)),
                    )
                }
            }
            (None, None) => {
                // `matched` e' costruito solo con una sorgente `Some` a
                // sinistra o a destra: la coppia (None, None) non e'
                // producibile; invariante interna, errore esplicito (R6).
                return Err(PlenoraError::Internal(
                    "table_diff: riga senza sorgente in nessuno dei due batch".into(),
                ));
            }
        };
        if status != "UNCHANGED" || config.include_unchanged == "yes" {
            rows.push(DiffRow {
                old_row,
                new_row,
                status,
                changed,
                old_values,
            });
        }
    }
    if rows.len() > limits.max_rows {
        return Err(PlenoraError::ResourceLimit(
            "table_diff supera max_rows".into(),
        ));
    }
    let output_count = config
        .right_keys
        .len()
        .saturating_add(compare.len())
        .saturating_add(3);
    if output_count > limits.max_columns {
        return Err(PlenoraError::ResourceLimit(
            "table_diff supera max_columns".into(),
        ));
    }
    let mut fields = Vec::new();
    let mut columns: Vec<Arc<dyn plenora_core::arrow::array::Array>> = Vec::new();
    for (position, name) in config.left_keys.iter().enumerate() {
        let left_column = left.column(left_keys[position]);
        let right_column = right.column(right_keys[position]);
        fields.push(
            left.schema()
                .field(left_keys[position])
                .as_ref()
                .clone()
                .with_name(name)
                .with_nullable(true),
        );
        columns.push(diff_values(left_column, right_column, &rows)?);
    }
    for (position, name) in compare.iter().enumerate() {
        let left_column = left.column(left_compare[position]);
        let right_column = right.column(right_compare[position]);
        fields.push(
            right
                .schema()
                .field(right_compare[position])
                .as_ref()
                .clone()
                .with_name(name)
                .with_nullable(true),
        );
        columns.push(diff_values(left_column, right_column, &rows)?);
    }
    for (name, selector) in [
        ("_diff_status", 0_usize),
        ("_diff_columns", 1),
        ("_diff_old_values", 2),
    ] {
        fields.push(Field::new(
            name,
            DataType::Utf8,
            selector == 1 || selector == 2,
        ));
        // StringBuilder diretto: stessi valori e stessi null della costruzione
        // originale via Vec<Option<String>>, senza un clone di String per riga.
        let mut builder = StringBuilder::with_capacity(
            rows.len(),
            rows.len().saturating_mul(8).min(64 * 1024 * 1024),
        );
        for row in &rows {
            let value = match selector {
                0 => Some(row.status),
                1 => row.changed.as_deref(),
                _ => row.old_values.as_deref(),
            };
            match value {
                Some(value) => builder.append_value(value),
                None => builder.append_null(),
            }
        }
        columns.push(Arc::new(builder.finish()));
    }
    Ok(RecordBatch::try_new(
        Arc::new(Schema::new(fields)),
        columns,
    )?)
}

#[cfg(test)]
mod tests {
    // -------------------------------------------------------------------
    // Test-oracolo del fast path di `melt`/`pivot`: l'output dev'essere
    // byte-identico a quello delle implementazioni di riferimento qui
    // sotto, che non passano dal percorso ottimizzato.
    // -------------------------------------------------------------------

    use super::*;
    // L'oracolo qui sotto ricalcola il percorso generico originale, e quello
    // usa `scalar_as_f64_rounded` direttamente: la produzione ora ci arriva
    // tramite `Float64Source`, quindi l'importazione serve solo qui.
    use crate::scalar_as_f64_rounded;
    use std::collections::BTreeSet;

    use plenora_core::arrow::array::Date32Array;

    /// Oracolo indipendente di `melt`: stesso contratto, percorso diverso.
    fn melt_reference(batch: &RecordBatch, config: &Melt, limits: &Limits) -> Result<RecordBatch> {
        let id_indices = config
            .id_columns
            .iter()
            .map(|name| column_index(batch, name))
            .collect::<Result<Vec<_>>>()?;
        let value_indices = if config.value_columns.is_empty() {
            (0..batch.num_columns())
                .filter(|index| !id_indices.contains(index))
                .collect::<Vec<_>>()
        } else {
            config
                .value_columns
                .iter()
                .map(|name| column_index(batch, name))
                .collect::<Result<Vec<_>>>()?
        };
        if value_indices.is_empty() {
            return Err(PlenoraError::InvalidPlan("melt senza value_columns".into()));
        }
        let output_rows = batch
            .num_rows()
            .checked_mul(value_indices.len())
            .ok_or_else(|| PlenoraError::ResourceLimit("overflow righe melt".into()))?;
        if output_rows > limits.max_rows {
            return Err(PlenoraError::ResourceLimit("melt supera max_rows".into()));
        }
        let row_indices = value_indices
            .iter()
            .flat_map(|_| 0..batch.num_rows())
            .collect::<Vec<_>>();
        let repeated = select_rows(batch, &row_indices)?;
        let mut fields = id_indices
            .iter()
            .map(|index| repeated.schema().field(*index).as_ref().clone())
            .collect::<Vec<_>>();
        let mut columns = id_indices
            .iter()
            .map(|index| repeated.column(*index).clone())
            .collect::<Vec<_>>();
        // Stessa funzione del kernel, non una copia: quando l'oracolo replica
        // l'algoritmo, replica anche i suoi difetti — e la parita' concorda
        // su un risultato sbagliato. La correttezza dei nomi la verificano
        // asserzioni MANUALI, non questa parita'.
        let schema_ingresso = batch.schema();
        let [var_name, value_name] = super::resolve_melt_names(
            schema_ingresso
                .fields()
                .iter()
                .map(|campo| campo.name().as_str()),
            config,
        )?;
        fields.push(Field::new(&var_name, DataType::Utf8, false));
        let variables = value_indices
            .iter()
            .flat_map(|index| {
                std::iter::repeat_n(
                    batch.schema().field(*index).name().clone(),
                    batch.num_rows(),
                )
            })
            .collect::<Vec<_>>();
        columns.push(Arc::new(StringArray::from(variables)));
        let value_type = batch.column(value_indices[0]).data_type().clone();
        let homogeneous = value_indices
            .iter()
            .all(|index| batch.column(*index).data_type() == &value_type);
        if homogeneous {
            let arrays = value_indices
                .iter()
                .map(|index| batch.column(*index).as_ref())
                .collect::<Vec<_>>();
            fields.push(Field::new(&value_name, value_type, true));
            columns.push(plenora_core::arrow::select::concat::concat(&arrays)?);
        } else if matches!(config.type_policy, HeterogeneousTypePolicy::String) {
            let mut values = Vec::with_capacity(output_rows);
            for index in value_indices {
                for row in 0..batch.num_rows() {
                    let value = scalar_as_string(batch.column(index).as_ref(), row)?;
                    if value
                        .as_ref()
                        .is_some_and(|value| value.len() > limits.max_string_bytes)
                    {
                        return Err(PlenoraError::ResourceLimit(
                            "melt: valore testuale oltre max_string_bytes".into(),
                        ));
                    }
                    values.push(value);
                }
            }
            fields.push(Field::new(&value_name, DataType::Utf8, true));
            columns.push(Arc::new(StringArray::from(values)));
        } else {
            return Err(PlenoraError::InvalidPlan(
                "melt: value_columns eterogenee; impostare type_policy='string' per la conversione esplicita".into(),
            ));
        }
        Ok(RecordBatch::try_new(
            Arc::new(Schema::new(fields)),
            columns,
        )?)
    }

    /// Oracolo indipendente di `pivot_column`.
    // Oracolo: dispatcher esaustivo per funzione di aggregazione, specchio del
    // percorso veloce; la lunghezza e' nei casi, non nella logica.
    #[allow(clippy::too_many_lines)]
    fn pivot_column_reference(
        batch: &RecordBatch,
        index: usize,
        groups: &[Option<&Vec<usize>>],
        function: &PivotAgg,
    ) -> Result<(DataType, ArrayRef)> {
        let source = batch.column(index);
        Ok(match function {
            PivotAgg::First | PivotAgg::Last => {
                let indices = groups
                    .iter()
                    .map(|rows| {
                        rows.and_then(|rows| {
                            if matches!(function, PivotAgg::First) {
                                rows.first()
                            } else {
                                rows.last()
                            }
                        })
                        .copied()
                        .map(u32::try_from)
                        .transpose()
                        .map_err(|_| PlenoraError::ResourceLimit("indice pivot oltre u32".into()))
                    })
                    .collect::<Result<Vec<_>>>()?;
                (
                    source.data_type().clone(),
                    plenora_core::arrow::select::take::take(
                        source.as_ref(),
                        &UInt32Array::from(indices),
                        None,
                    )?,
                )
            }
            PivotAgg::Count => {
                let values = groups
                    .iter()
                    .map(|rows| {
                        rows.map(|rows| {
                            i64::try_from(
                                rows.iter()
                                    .filter(|row| !crate::is_logically_null(source.as_ref(), **row))
                                    .count(),
                            )
                            .map_err(|_| {
                                PlenoraError::ResourceLimit("conteggio pivot oltre i64".into())
                            })
                        })
                        .transpose()
                    })
                    .collect::<Result<Vec<_>>>()?;
                (DataType::Int64, Arc::new(Int64Array::from(values)))
            }
            PivotAgg::Concat => {
                let values = groups
                    .iter()
                    .map(|rows| {
                        rows.map(|rows| {
                            rows.iter()
                                .filter_map(|row| {
                                    scalar_as_string(source.as_ref(), *row).transpose()
                                })
                                .collect::<Result<Vec<_>>>()
                                .map(|values| values.join(","))
                        })
                        .transpose()
                    })
                    .collect::<Result<Vec<_>>>()?;
                (DataType::Utf8, Arc::new(StringArray::from(values)))
            }
            PivotAgg::Sum | PivotAgg::Mean | PivotAgg::Min | PivotAgg::Max => {
                let values = groups
                    .iter()
                    .map(|rows| {
                        let Some(rows) = rows else { return Ok(None) };
                        let values = rows
                            .iter()
                            .filter_map(|row| {
                                scalar_as_f64_rounded(source.as_ref(), *row).transpose()
                            })
                            .collect::<Result<Vec<_>>>()?;
                        if values.is_empty() {
                            return Ok(None);
                        }
                        Ok(Some(match function {
                            PivotAgg::Sum => values.iter().sum(),
                            PivotAgg::Mean => {
                                values.iter().sum::<f64>()
                                    / values.len().to_f64().ok_or_else(|| {
                                        PlenoraError::ResourceLimit(
                                            "gruppo pivot non rappresentabile".into(),
                                        )
                                    })?
                            }
                            PivotAgg::Min => {
                                values.into_iter().reduce(f64::min).unwrap_or_default()
                            }
                            PivotAgg::Max => {
                                values.into_iter().reduce(f64::max).unwrap_or_default()
                            }
                            _ => unreachable!(),
                        }))
                    })
                    .collect::<Result<Vec<_>>>()?;
                (DataType::Float64, Arc::new(Float64Array::from(values)))
            }
        })
    }

    /// Oracolo indipendente di `pivot`: stesso contratto, percorso diverso.
    fn pivot_reference(
        batch: &RecordBatch,
        config: &Pivot,
        limits: &Limits,
    ) -> Result<RecordBatch> {
        let index_names = config
            .index_col
            .split(',')
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .collect::<Vec<_>>();
        let index_indices = index_names
            .iter()
            .map(|name| column_index(batch, name))
            .collect::<Result<Vec<_>>>()?;
        let pivot_index = column_index(batch, &config.column)?;
        let value_index = column_index(batch, &config.value_col)?;
        let mut groups: BTreeMap<String, Vec<usize>> = BTreeMap::new();
        let mut pivot_values = BTreeSet::new();
        for row in 0..batch.num_rows() {
            let key = composite_key(batch, &index_indices, row)?;
            let Some(pivot) = scalar_as_string(batch.column(pivot_index).as_ref(), row)? else {
                continue;
            };
            if config.mapping.is_empty() || config.mapping.contains_key(&pivot) {
                pivot_values.insert(pivot.clone());
                groups
                    .entry(format!("{key}{}:{pivot}", pivot.len()))
                    .or_default()
                    .push(row);
            }
        }
        let mut index_rows: BTreeMap<String, usize> = BTreeMap::new();
        for row in 0..batch.num_rows() {
            let key = composite_key(batch, &index_indices, row)?;
            index_rows.entry(key).or_insert(row);
        }
        if index_rows.len() > limits.max_rows
            || index_indices.len().saturating_add(pivot_values.len()) > limits.max_columns
        {
            return Err(PlenoraError::ResourceLimit(
                "pivot supera i limiti di output".into(),
            ));
        }
        let representatives = index_rows.values().copied().collect::<Vec<_>>();
        let selected = select_rows(batch, &representatives)?;
        let mut fields = index_indices
            .iter()
            .map(|index| selected.schema().field(*index).as_ref().clone())
            .collect::<Vec<_>>();
        let mut columns = index_indices
            .iter()
            .map(|index| selected.column(*index).clone())
            .collect::<Vec<_>>();
        for pivot_value in pivot_values {
            let output = config
                .mapping
                .get(&pivot_value)
                .cloned()
                .unwrap_or_else(|| pivot_value.clone());
            validate_output_name(&output)?;
            let grouped_rows = index_rows
                .keys()
                .map(|key| groups.get(&format!("{key}{}:{pivot_value}", pivot_value.len())))
                .collect::<Vec<_>>();
            let (data_type, values) =
                pivot_column_reference(batch, value_index, &grouped_rows, &config.aggr_func)?;
            fields.push(Field::new(&output, data_type, true));
            columns.push(values);
        }
        Ok(RecordBatch::try_new(
            Arc::new(Schema::new(fields)),
            columns,
        )?)
    }

    /// Confronto rigoroso: schema (nomi, tipi, nullabilita'), numero righe,
    /// maschera null e valori (bit a bit per i Float64, NaN incluso).
    // Asserzioni esplicite colonna per colonna e valore per valore: test di
    // parita', lungo per costruzione (niente logica da fattorizzare).
    #[allow(clippy::too_many_lines)]
    fn assert_batches_identical(fast: &RecordBatch, reference: &RecordBatch) {
        assert_eq!(fast.num_rows(), reference.num_rows(), "righe");
        assert_eq!(fast.num_columns(), reference.num_columns(), "colonne");
        let fast_schema = fast.schema();
        let reference_schema = reference.schema();
        for index in 0..fast.num_columns() {
            let fast_field = fast_schema.field(index);
            let reference_field = reference_schema.field(index);
            assert_eq!(
                fast_field.name(),
                reference_field.name(),
                "nome colonna {index}"
            );
            assert_eq!(
                fast_field.data_type(),
                reference_field.data_type(),
                "tipo colonna {}",
                fast_field.name()
            );
            assert_eq!(
                fast_field.is_nullable(),
                reference_field.is_nullable(),
                "nullabilita' colonna {}",
                fast_field.name()
            );
            let fast_column = fast.column(index);
            let reference_column = reference.column(index);
            for row in 0..fast.num_rows() {
                assert_eq!(
                    fast_column.is_null(row),
                    reference_column.is_null(row),
                    "null riga {row} colonna {}",
                    fast_field.name()
                );
            }
            match fast_field.data_type() {
                DataType::Float64 => {
                    let fast_values = fast_column
                        .as_any()
                        .downcast_ref::<Float64Array>()
                        .expect("float64");
                    let reference_values = reference_column
                        .as_any()
                        .downcast_ref::<Float64Array>()
                        .expect("float64");
                    for row in 0..fast.num_rows() {
                        if !fast_values.is_null(row) {
                            assert_eq!(
                                fast_values.value(row).to_bits(),
                                reference_values.value(row).to_bits(),
                                "bits riga {row} colonna {}",
                                fast_field.name()
                            );
                        }
                    }
                }
                DataType::Int64 => {
                    let fast_values = fast_column
                        .as_any()
                        .downcast_ref::<Int64Array>()
                        .expect("int64");
                    let reference_values = reference_column
                        .as_any()
                        .downcast_ref::<Int64Array>()
                        .expect("int64");
                    for row in 0..fast.num_rows() {
                        if !fast_values.is_null(row) {
                            assert_eq!(
                                fast_values.value(row),
                                reference_values.value(row),
                                "valore riga {row} colonna {}",
                                fast_field.name()
                            );
                        }
                    }
                }
                DataType::Utf8 => {
                    let fast_values = fast_column
                        .as_any()
                        .downcast_ref::<StringArray>()
                        .expect("utf8");
                    let reference_values = reference_column
                        .as_any()
                        .downcast_ref::<StringArray>()
                        .expect("utf8");
                    for row in 0..fast.num_rows() {
                        if !fast_values.is_null(row) {
                            assert_eq!(
                                fast_values.value(row),
                                reference_values.value(row),
                                "testo riga {row} colonna {}",
                                fast_field.name()
                            );
                        }
                    }
                }
                DataType::Boolean => {
                    let fast_values = fast_column
                        .as_any()
                        .downcast_ref::<BooleanArray>()
                        .expect("bool");
                    let reference_values = reference_column
                        .as_any()
                        .downcast_ref::<BooleanArray>()
                        .expect("bool");
                    for row in 0..fast.num_rows() {
                        if !fast_values.is_null(row) {
                            assert_eq!(
                                fast_values.value(row),
                                reference_values.value(row),
                                "bool riga {row} colonna {}",
                                fast_field.name()
                            );
                        }
                    }
                }
                _ => {
                    // Tipi senza confronto nativo dedicato (es. Date32 in
                    // first/last): stessi byte via profilo scalare.
                    for row in 0..fast.num_rows() {
                        assert_eq!(
                            scalar_as_string(fast_column.as_ref(), row).expect("fast"),
                            scalar_as_string(reference_column.as_ref(), row).expect("ref"),
                            "valore riga {row} colonna {}",
                            fast_field.name()
                        );
                    }
                }
            }
        }
    }

    fn batch_of(fields: Vec<Field>, columns: Vec<ArrayRef>) -> RecordBatch {
        RecordBatch::try_new(Arc::new(Schema::new(fields)), columns).expect("fixture")
    }

    fn melt_config(
        id_columns: &[&str],
        value_columns: &[&str],
        type_policy: HeterogeneousTypePolicy,
    ) -> Melt {
        Melt {
            id_columns: id_columns.iter().map(|name| (*name).into()).collect(),
            value_columns: value_columns.iter().map(|name| (*name).into()).collect(),
            var_name: "variable".into(),
            value_name: "value".into(),
            type_policy,
        }
    }

    fn pivot_config(index: &str, column: &str, value: &str, aggr_func: PivotAgg) -> Pivot {
        Pivot {
            index_col: index.into(),
            column: column.into(),
            value_col: value.into(),
            aggr_func,
            mapping: BTreeMap::new(),
        }
    }

    fn schema_names(batch: &RecordBatch) -> Vec<String> {
        batch
            .schema()
            .fields()
            .iter()
            .map(|field| field.name().clone())
            .collect()
    }

    #[test]
    fn melt_homogeneous_float64_oracle() {
        let batch = batch_of(
            vec![
                Field::new("id", DataType::Int64, false),
                Field::new("a", DataType::Float64, true),
                Field::new("b", DataType::Float64, true),
            ],
            vec![
                Arc::new(Int64Array::from(vec![1, 2, 3, 4])),
                Arc::new(Float64Array::from(vec![
                    Some(1.5),
                    None,
                    Some(f64::NAN),
                    Some(-0.0),
                ])),
                Arc::new(Float64Array::from(vec![
                    Some(-2.25),
                    Some(0.0),
                    None,
                    Some(f64::INFINITY),
                ])),
            ],
        );
        let config = melt_config(&["id"], &["a", "b"], HeterogeneousTypePolicy::Reject);
        let fast = melt(&batch, &config, &Limits::default()).expect("melt fast");
        let reference = melt_reference(&batch, &config, &Limits::default()).expect("melt ref");
        assert_batches_identical(&fast, &reference);
    }

    #[test]
    fn melt_default_value_columns_e_naming_collision_oracle() {
        // Quirk di naming: la colonna id si chiama gia' "value", quindi la
        // colonna valore di output diventa "value_1"; "variable" resta libero.
        let batch = batch_of(
            vec![
                Field::new("value", DataType::Int64, false),
                Field::new("x", DataType::Int64, true),
                Field::new("y", DataType::Int64, true),
            ],
            vec![
                Arc::new(Int64Array::from(vec![10, 20])),
                Arc::new(Int64Array::from(vec![Some(1), None])),
                Arc::new(Int64Array::from(vec![None, Some(2)])),
            ],
        );
        let config = melt_config(&["value"], &[], HeterogeneousTypePolicy::Reject);
        let fast = melt(&batch, &config, &Limits::default()).expect("melt fast");
        assert_eq!(
            schema_names(&fast),
            vec!["value", "variable", "value_1"],
            "quirk value->value_1"
        );
        let reference = melt_reference(&batch, &config, &Limits::default()).expect("melt ref");
        assert_batches_identical(&fast, &reference);

        // Collisione doppia: anche "variable" occupata -> "variable_1".
        let batch = batch_of(
            vec![
                Field::new("variable", DataType::Int64, false),
                Field::new("value", DataType::Int64, false),
                Field::new("x", DataType::Int64, true),
            ],
            vec![
                Arc::new(Int64Array::from(vec![1, 2])),
                Arc::new(Int64Array::from(vec![3, 4])),
                Arc::new(Int64Array::from(vec![Some(5), None])),
            ],
        );
        let config = melt_config(
            &["variable", "value"],
            &["x"],
            HeterogeneousTypePolicy::Reject,
        );
        let fast = melt(&batch, &config, &Limits::default()).expect("melt fast");
        assert_eq!(
            schema_names(&fast),
            vec!["variable", "value", "variable_1", "value_1"],
            "doppia collisione"
        );
        let reference = melt_reference(&batch, &config, &Limits::default()).expect("melt ref");
        assert_batches_identical(&fast, &reference);
    }

    #[test]
    fn melt_heterogeneous_string_policy_oracle() {
        let batch = batch_of(
            vec![
                Field::new("id", DataType::Int64, false),
                Field::new("n", DataType::Int64, true),
                Field::new("t", DataType::Utf8, true),
                Field::new("f", DataType::Float64, true),
                Field::new("b", DataType::Boolean, true),
            ],
            vec![
                Arc::new(Int64Array::from(vec![1, 2, 3])),
                Arc::new(Int64Array::from(vec![Some(-7), None, Some(42)])),
                Arc::new(StringArray::from(vec![Some("alfa"), Some(""), None])),
                Arc::new(Float64Array::from(vec![
                    Some(-0.0),
                    Some(f64::NAN),
                    Some(2.5e300),
                ])),
                Arc::new(BooleanArray::from(vec![Some(true), None, Some(false)])),
            ],
        );
        let config = melt_config(
            &["id"],
            &["n", "t", "f", "b"],
            HeterogeneousTypePolicy::String,
        );
        let fast = melt(&batch, &config, &Limits::default()).expect("melt fast");
        let reference = melt_reference(&batch, &config, &Limits::default()).expect("melt ref");
        assert_batches_identical(&fast, &reference);
    }

    #[test]
    fn melt_heterogeneous_generic_fallback_oracle() {
        // Date32 non ha fast path: ricade sul percorso scalare generico.
        let batch = batch_of(
            vec![
                Field::new("id", DataType::Int64, false),
                Field::new("d", DataType::Date32, true),
                Field::new("n", DataType::Int64, true),
            ],
            vec![
                Arc::new(Int64Array::from(vec![1, 2, 3])),
                Arc::new(Date32Array::from(vec![Some(0), None, Some(19_700)])),
                Arc::new(Int64Array::from(vec![Some(9), Some(-1), None])),
            ],
        );
        let config = melt_config(&["id"], &["d", "n"], HeterogeneousTypePolicy::String);
        let fast = melt(&batch, &config, &Limits::default()).expect("melt fast");
        let reference = melt_reference(&batch, &config, &Limits::default()).expect("melt ref");
        assert_batches_identical(&fast, &reference);
    }

    #[test]
    fn melt_errori_identici() {
        let batch = batch_of(
            vec![
                Field::new("id", DataType::Int64, false),
                Field::new("n", DataType::Int64, true),
                Field::new("t", DataType::Utf8, true),
            ],
            vec![
                Arc::new(Int64Array::from(vec![1])),
                Arc::new(Int64Array::from(vec![Some(7)])),
                Arc::new(StringArray::from(vec![Some("x")])),
            ],
        );
        // Eterogenee senza policy string: stesso errore.
        let config = melt_config(&["id"], &["n", "t"], HeterogeneousTypePolicy::Reject);
        let fast = melt(&batch, &config, &Limits::default());
        let reference = melt_reference(&batch, &config, &Limits::default());
        assert_eq!(
            format!("{:?}", fast.expect_err("fast deve fallire")),
            format!("{:?}", reference.expect_err("ref deve fallire"))
        );
        // max_string_bytes: stesso errore.
        let limits = Limits {
            max_string_bytes: 0,
            ..Limits::default()
        };
        let config = melt_config(&["id"], &["n", "t"], HeterogeneousTypePolicy::String);
        let fast = melt(&batch, &config, &limits);
        let reference = melt_reference(&batch, &config, &limits);
        assert_eq!(
            format!("{:?}", fast.expect_err("fast deve fallire")),
            format!("{:?}", reference.expect_err("ref deve fallire"))
        );
        // Batch senza colonne valore: stesso errore.
        let only_ids = batch_of(
            vec![Field::new("id", DataType::Int64, false)],
            vec![Arc::new(Int64Array::from(vec![1]))],
        );
        let config = melt_config(&["id"], &[], HeterogeneousTypePolicy::Reject);
        let fast = melt(&only_ids, &config, &Limits::default());
        let reference = melt_reference(&only_ids, &config, &Limits::default());
        assert_eq!(
            format!("{:?}", fast.expect_err("fast deve fallire")),
            format!("{:?}", reference.expect_err("ref deve fallire"))
        );
    }

    #[test]
    fn melt_input_vuoto_oracle() {
        let batch = batch_of(
            vec![
                Field::new("id", DataType::Int64, false),
                Field::new("a", DataType::Float64, true),
            ],
            vec![
                Arc::new(Int64Array::from(Vec::<i64>::new())),
                Arc::new(Float64Array::from(Vec::<Option<f64>>::new())),
            ],
        );
        let config = melt_config(&["id"], &["a"], HeterogeneousTypePolicy::Reject);
        let fast = melt(&batch, &config, &Limits::default()).expect("melt fast");
        let reference = melt_reference(&batch, &config, &Limits::default()).expect("melt ref");
        assert_batches_identical(&fast, &reference);
    }

    /// Fixture pivot: chiavi utf8 (con null), pivot utf8 (con null),
    /// valori float64 (con null, NaN, -0.0). Copre first/last/count/concat
    /// e tutte le riduzioni numeriche.
    fn pivot_fixture_utf8() -> RecordBatch {
        let keys: Vec<Option<&str>> = vec![
            Some("b"),
            Some("a"),
            None,
            Some("a"),
            Some("b"),
            None,
            Some("a"),
            Some("b"),
        ];
        let pivots: Vec<Option<&str>> = vec![
            Some("y"),
            Some("x"),
            Some("x"),
            Some("y"),
            Some("x"),
            None,
            Some("x"),
            Some("y"),
        ];
        let values: Vec<Option<f64>> = vec![
            Some(1.0),
            Some(-0.0),
            Some(10.0),
            Some(f64::NAN),
            Some(3.5),
            Some(20.0),
            None,
            Some(-7.25),
        ];
        batch_of(
            vec![
                Field::new("k", DataType::Utf8, true),
                Field::new("p", DataType::Utf8, true),
                Field::new("v", DataType::Float64, true),
            ],
            vec![
                Arc::new(StringArray::from(keys)),
                Arc::new(StringArray::from(pivots)),
                Arc::new(Float64Array::from(values)),
            ],
        )
    }

    #[test]
    fn pivot_tutte_le_aggr_oracle() {
        let batch = pivot_fixture_utf8();
        for function in [
            PivotAgg::First,
            PivotAgg::Last,
            PivotAgg::Min,
            PivotAgg::Max,
            PivotAgg::Sum,
            PivotAgg::Mean,
            PivotAgg::Count,
            PivotAgg::Concat,
        ] {
            let config = pivot_config("k", "p", "v", function.clone());
            let fast = pivot(&batch, &config, &Limits::default()).expect("pivot fast");
            let reference =
                pivot_reference(&batch, &config, &Limits::default()).expect("pivot ref");
            assert_batches_identical(&fast, &reference);
        }
    }

    #[test]
    fn pivot_valori_int64_e_testo_oracle() {
        let batch = batch_of(
            vec![
                Field::new("k", DataType::Int64, true),
                Field::new("p", DataType::Utf8, true),
                Field::new("n", DataType::Int64, true),
                Field::new("t", DataType::Utf8, true),
            ],
            vec![
                Arc::new(Int64Array::from(vec![
                    Some(2),
                    Some(1),
                    Some(2),
                    None,
                    Some(1),
                    Some(2),
                ])),
                Arc::new(StringArray::from(vec![
                    Some("u"),
                    Some("u"),
                    Some("v"),
                    Some("u"),
                    Some("v"),
                    Some("u"),
                ])),
                Arc::new(Int64Array::from(vec![
                    Some(5),
                    None,
                    Some(-3),
                    Some(100),
                    Some(8),
                    Some(2),
                ])),
                Arc::new(StringArray::from(vec![
                    Some("a"),
                    Some("b"),
                    None,
                    Some("z"),
                    Some("c"),
                    Some("d"),
                ])),
            ],
        );
        for (value, function) in [
            ("n", PivotAgg::Sum),
            ("n", PivotAgg::Mean),
            ("n", PivotAgg::Min),
            ("n", PivotAgg::Max),
            ("n", PivotAgg::Count),
            ("n", PivotAgg::First),
            ("n", PivotAgg::Last),
            ("n", PivotAgg::Concat),
            ("t", PivotAgg::Concat),
            ("t", PivotAgg::First),
            ("t", PivotAgg::Last),
        ] {
            let config = pivot_config("k", "p", value, function.clone());
            let fast = pivot(&batch, &config, &Limits::default()).expect("pivot fast");
            let reference =
                pivot_reference(&batch, &config, &Limits::default()).expect("pivot ref");
            assert_batches_identical(&fast, &reference);
        }
    }

    #[test]
    fn pivot_mapping_rinomina_e_filtra_oracle() {
        let batch = pivot_fixture_utf8();
        let mut mapping = BTreeMap::new();
        mapping.insert("x".to_owned(), "col_x".to_owned());
        // "y" escluso dal mapping -> non produce colonne.
        let config = Pivot {
            mapping,
            ..pivot_config("k", "p", "v", PivotAgg::Sum)
        };
        let fast = pivot(&batch, &config, &Limits::default()).expect("pivot fast");
        assert_eq!(
            schema_names(&fast),
            vec!["k", "col_x"],
            "mapping rinomina e filtra"
        );
        let reference = pivot_reference(&batch, &config, &Limits::default()).expect("pivot ref");
        assert_batches_identical(&fast, &reference);
    }

    #[test]
    fn pivot_multi_indice_e_float_keys_oracle() {
        // Indice composto + pivot float64: -0.0 e 0.0 restano chiavi
        // distinte ("-0" vs "0"), NaN e' una chiave ("NaN").
        let batch = batch_of(
            vec![
                Field::new("a", DataType::Int64, true),
                Field::new("b", DataType::Utf8, true),
                Field::new("p", DataType::Float64, true),
                Field::new("v", DataType::Float64, true),
            ],
            vec![
                Arc::new(Int64Array::from(vec![
                    Some(1),
                    Some(1),
                    None,
                    Some(1),
                    None,
                    Some(2),
                ])),
                Arc::new(StringArray::from(vec![
                    Some("s"),
                    Some("s"),
                    None,
                    Some("t"),
                    None,
                    Some("s"),
                ])),
                Arc::new(Float64Array::from(vec![
                    Some(-0.0),
                    Some(0.0),
                    Some(f64::NAN),
                    Some(-0.0),
                    Some(1.5),
                    Some(0.0),
                ])),
                Arc::new(Float64Array::from(vec![
                    Some(1.0),
                    Some(2.0),
                    Some(3.0),
                    Some(4.0),
                    Some(5.0),
                    None,
                ])),
            ],
        );
        for function in [
            PivotAgg::Sum,
            PivotAgg::First,
            PivotAgg::Count,
            PivotAgg::Concat,
        ] {
            let config = pivot_config("a, b", "p", "v", function.clone());
            let fast = pivot(&batch, &config, &Limits::default()).expect("pivot fast");
            let reference =
                pivot_reference(&batch, &config, &Limits::default()).expect("pivot ref");
            assert_batches_identical(&fast, &reference);
        }
    }

    #[test]
    fn pivot_tipici_generici_oracle() {
        // Date32 (indice e valore) e Boolean (pivot): fuori dai fast path
        // chiave/numerico, ricadono sul percorso scalare generico.
        let batch = batch_of(
            vec![
                Field::new("d", DataType::Date32, true),
                Field::new("p", DataType::Boolean, true),
                Field::new("v", DataType::Date32, true),
            ],
            vec![
                Arc::new(Date32Array::from(vec![
                    Some(0),
                    Some(1),
                    Some(0),
                    None,
                    Some(1),
                ])),
                Arc::new(BooleanArray::from(vec![
                    Some(true),
                    Some(false),
                    Some(true),
                    Some(true),
                    Some(true),
                ])),
                Arc::new(Date32Array::from(vec![
                    Some(10),
                    Some(20),
                    Some(30),
                    Some(40),
                    None,
                ])),
            ],
        );
        for function in [
            PivotAgg::Sum,
            PivotAgg::Mean,
            PivotAgg::Count,
            PivotAgg::First,
        ] {
            let config = pivot_config("d", "p", "v", function.clone());
            let fast = pivot(&batch, &config, &Limits::default()).expect("pivot fast");
            let reference =
                pivot_reference(&batch, &config, &Limits::default()).expect("pivot ref");
            assert_batches_identical(&fast, &reference);
        }
    }

    #[test]
    fn pivot_molti_distinti_oracle() {
        // 500 chiavi x 30 pivot distinti, valori con null sparsi.
        let rows = 15_000;
        let keys = (0..rows)
            .map(|row| format!("k{:04}", row % 500))
            .collect::<Vec<_>>();
        let pivots = (0..rows)
            .map(|row| format!("p{:02}", (row / 500) % 30))
            .collect::<Vec<_>>();
        let values = (0..rows)
            .map(|row| {
                if row % 7 == 0 {
                    None
                } else {
                    #[allow(clippy::cast_precision_loss)]
                    Some(f64::from(row % 997) / 3.0)
                }
            })
            .collect::<Vec<_>>();
        let batch = batch_of(
            vec![
                Field::new("k", DataType::Utf8, false),
                Field::new("p", DataType::Utf8, false),
                Field::new("v", DataType::Float64, true),
            ],
            vec![
                Arc::new(StringArray::from(keys)),
                Arc::new(StringArray::from(pivots)),
                Arc::new(Float64Array::from(values)),
            ],
        );
        for function in [PivotAgg::Sum, PivotAgg::First, PivotAgg::Count] {
            let config = pivot_config("k", "p", "v", function.clone());
            let fast = pivot(&batch, &config, &Limits::default()).expect("pivot fast");
            let reference =
                pivot_reference(&batch, &config, &Limits::default()).expect("pivot ref");
            assert_batches_identical(&fast, &reference);
        }
    }

    #[test]
    fn pivot_un_pivot_solo_oracle() {
        // Estremo opposto: un solo valore pivot distinto su molte chiavi.
        let rows = 1_000;
        let keys = (0..rows).map(i64::from).collect::<Vec<_>>();
        let pivots = (0..rows).map(|_| "solo").collect::<Vec<_>>();
        let values = (0..rows)
            .map(|row| Some(i64::from(row % 50)))
            .collect::<Vec<_>>();
        let batch = batch_of(
            vec![
                Field::new("k", DataType::Int64, false),
                Field::new("p", DataType::Utf8, false),
                Field::new("v", DataType::Int64, true),
            ],
            vec![
                Arc::new(Int64Array::from(keys)),
                Arc::new(StringArray::from(pivots)),
                Arc::new(Int64Array::from(values)),
            ],
        );
        for function in [PivotAgg::Sum, PivotAgg::Last, PivotAgg::Concat] {
            let config = pivot_config("k", "p", "v", function.clone());
            let fast = pivot(&batch, &config, &Limits::default()).expect("pivot fast");
            let reference =
                pivot_reference(&batch, &config, &Limits::default()).expect("pivot ref");
            assert_batches_identical(&fast, &reference);
        }
    }

    #[test]
    fn pivot_input_vuoto_oracle() {
        let batch = batch_of(
            vec![
                Field::new("k", DataType::Utf8, true),
                Field::new("p", DataType::Utf8, true),
                Field::new("v", DataType::Float64, true),
            ],
            vec![
                Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
                Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
                Arc::new(Float64Array::from(Vec::<Option<f64>>::new())),
            ],
        );
        let config = pivot_config("k", "p", "v", PivotAgg::Sum);
        let fast = pivot(&batch, &config, &Limits::default()).expect("pivot fast");
        let reference = pivot_reference(&batch, &config, &Limits::default()).expect("pivot ref");
        assert_batches_identical(&fast, &reference);
    }

    #[test]
    fn pivot_errori_identici() {
        let batch = pivot_fixture_utf8();
        // Colonna inesistente: stesso errore.
        let config = pivot_config("k", "p", "manca", PivotAgg::Sum);
        let fast = pivot(&batch, &config, &Limits::default());
        let reference = pivot_reference(&batch, &config, &Limits::default());
        assert_eq!(
            format!("{:?}", fast.expect_err("fast deve fallire")),
            format!("{:?}", reference.expect_err("ref deve fallire"))
        );
        // Limiti di output: stesso errore.
        let limits = Limits {
            max_columns: 2,
            ..Limits::default()
        };
        let config = pivot_config("k", "p", "v", PivotAgg::Sum);
        let fast = pivot(&batch, &config, &limits);
        let reference = pivot_reference(&batch, &config, &limits);
        assert_eq!(
            format!("{:?}", fast.expect_err("fast deve fallire")),
            format!("{:?}", reference.expect_err("ref deve fallire"))
        );
    }

    // -------------------------------------------------------------------
    // Test-oracolo di `table_diff`: qui sotto un'implementazione di
    // riferimento indipendente, che non passa dal percorso ottimizzato e
    // costruisce le chiavi con `composite_key`.
    // -------------------------------------------------------------------

    /// La forma di riga dell'oracolo (`status: String`), usata da
    /// `diff_values_reference` e `table_diff_reference`.
    struct DiffRowRef {
        old_row: Option<usize>,
        new_row: Option<usize>,
        status: String,
        changed: Option<String>,
        old_values: Option<String>,
    }

    /// Implementazione indipendente di riferimento dello stesso contratto di
    /// `diff_values`. Non condivide il percorso ottimizzato, cosi' una
    /// deviazione del prodotto resta osservabile.
    fn diff_values_reference(
        left: &ArrayRef,
        right: &ArrayRef,
        rows: &[DiffRowRef],
    ) -> Result<ArrayRef> {
        if left.data_type() != right.data_type() {
            return Err(PlenoraError::Schema(format!(
                "table_diff richiede tipi Arrow identici, trovati {} e {}",
                left.data_type(),
                right.data_type()
            )));
        }
        let combined =
            plenora_core::arrow::select::concat::concat(&[left.as_ref(), right.as_ref()])?;
        let indices = rows
            .iter()
            .map(|row| {
                let index = if let Some(new_row) = row.new_row {
                    left.len().saturating_add(new_row)
                } else {
                    row.old_row.ok_or_else(|| {
                        PlenoraError::InvalidPlan("riga table_diff senza sorgente".into())
                    })?
                };
                u32::try_from(index)
                    .map(Some)
                    .map_err(|_| PlenoraError::ResourceLimit("indice table_diff oltre u32".into()))
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(plenora_core::arrow::select::take::take(
            combined.as_ref(),
            &UInt32Array::from(indices),
            None,
        )?)
    }

    /// Oracolo indipendente di `table_diff`: stesso contratto, percorso
    /// diverso.
    #[allow(clippy::too_many_lines)]
    fn table_diff_reference(
        left: &RecordBatch,
        right: &RecordBatch,
        config: &TableDiff,
        limits: &Limits,
    ) -> Result<RecordBatch> {
        if config.left_keys.is_empty() || config.left_keys.len() != config.right_keys.len() {
            return Err(PlenoraError::InvalidPlan(
                "chiavi table_diff non valide".into(),
            ));
        }
        let left_keys = config
            .left_keys
            .iter()
            .map(|name| column_index(left, name))
            .collect::<Result<Vec<_>>>()?;
        let right_keys = config
            .right_keys
            .iter()
            .map(|name| column_index(right, name))
            .collect::<Result<Vec<_>>>()?;
        let compare = if config.compare_columns.is_empty() {
            left.schema()
                .fields()
                .iter()
                .map(|field| field.name().clone())
                .filter(|name| {
                    !config.left_keys.contains(name) && right.schema().index_of(name).is_ok()
                })
                .collect::<Vec<_>>()
        } else {
            config.compare_columns.clone()
        };
        let left_compare = compare
            .iter()
            .map(|name| column_index(left, name))
            .collect::<Result<Vec<_>>>()?;
        let right_compare = compare
            .iter()
            .map(|name| column_index(right, name))
            .collect::<Result<Vec<_>>>()?;
        let mut old = BTreeMap::new();
        let mut new = BTreeMap::new();
        for row in 0..left.num_rows() {
            let key = composite_key(left, &left_keys, row)?;
            if old.insert(key, row).is_some() {
                return Err(PlenoraError::InvalidPlan(
                    "chiavi duplicate nella tabella sinistra".into(),
                ));
            }
        }
        for row in 0..right.num_rows() {
            let key = composite_key(right, &right_keys, row)?;
            if new.insert(key, row).is_some() {
                return Err(PlenoraError::InvalidPlan(
                    "chiavi duplicate nella tabella destra".into(),
                ));
            }
        }
        // Preserve source order: old rows first, then new-only rows. Sorting the
        // encoded key would place nulls first and reorder otherwise stable data.
        let mut all_keys = Vec::with_capacity(old.len().saturating_add(new.len()));
        for row in 0..left.num_rows() {
            all_keys.push(composite_key(left, &left_keys, row)?);
        }
        for row in 0..right.num_rows() {
            let key = composite_key(right, &right_keys, row)?;
            if !old.contains_key(&key) {
                all_keys.push(key);
            }
        }
        let mut rows = Vec::new();
        for key in all_keys {
            let old_row = old.get(&key).copied();
            let new_row = new.get(&key).copied();
            let (status, changed, old_values) = match (old_row, new_row) {
                (None, Some(_)) => ("ADDED".to_owned(), None, None),
                (Some(_), None) => ("DELETED".to_owned(), None, None),
                (Some(old_row), Some(new_row)) => {
                    let mut changed = Vec::new();
                    let mut old_values = Vec::new();
                    for ((name, left_index), right_index) in
                        compare.iter().zip(&left_compare).zip(&right_compare)
                    {
                        let before = scalar_as_string(left.column(*left_index).as_ref(), old_row)?;
                        let after = scalar_as_string(right.column(*right_index).as_ref(), new_row)?;
                        if before != after {
                            changed.push(name.clone());
                            old_values.push(before.unwrap_or_default());
                        }
                    }
                    if changed.is_empty() {
                        ("UNCHANGED".to_owned(), None, None)
                    } else {
                        (
                            "MODIFIED".to_owned(),
                            Some(changed.join(&config.separator)),
                            Some(old_values.join(&config.separator)),
                        )
                    }
                }
                (None, None) => unreachable!(),
            };
            if status != "UNCHANGED" || config.include_unchanged == "yes" {
                rows.push(DiffRowRef {
                    old_row,
                    new_row,
                    status,
                    changed,
                    old_values,
                });
            }
        }
        if rows.len() > limits.max_rows {
            return Err(PlenoraError::ResourceLimit(
                "table_diff supera max_rows".into(),
            ));
        }
        let output_count = config
            .right_keys
            .len()
            .saturating_add(compare.len())
            .saturating_add(3);
        if output_count > limits.max_columns {
            return Err(PlenoraError::ResourceLimit(
                "table_diff supera max_columns".into(),
            ));
        }
        let mut fields = Vec::new();
        let mut columns: Vec<Arc<dyn plenora_core::arrow::array::Array>> = Vec::new();
        for (position, name) in config.left_keys.iter().enumerate() {
            let left_column = left.column(left_keys[position]);
            let right_column = right.column(right_keys[position]);
            fields.push(
                left.schema()
                    .field(left_keys[position])
                    .as_ref()
                    .clone()
                    .with_name(name)
                    .with_nullable(true),
            );
            columns.push(diff_values_reference(left_column, right_column, &rows)?);
        }
        for (position, name) in compare.iter().enumerate() {
            let left_column = left.column(left_compare[position]);
            let right_column = right.column(right_compare[position]);
            fields.push(
                right
                    .schema()
                    .field(right_compare[position])
                    .as_ref()
                    .clone()
                    .with_name(name)
                    .with_nullable(true),
            );
            columns.push(diff_values_reference(left_column, right_column, &rows)?);
        }
        for (name, selector) in [
            ("_diff_status", 0_usize),
            ("_diff_columns", 1),
            ("_diff_old_values", 2),
        ] {
            fields.push(Field::new(
                name,
                DataType::Utf8,
                selector == 1 || selector == 2,
            ));
            columns.push(Arc::new(StringArray::from(
                rows.iter()
                    .map(|row| match selector {
                        0 => Some(row.status.clone()),
                        1 => row.changed.clone(),
                        _ => row.old_values.clone(),
                    })
                    .collect::<Vec<_>>(),
            )));
        }
        Ok(RecordBatch::try_new(
            Arc::new(Schema::new(fields)),
            columns,
        )?)
    }

    /// Fixture diff: chiavi composite `id` int64 + `grp` utf8 nullable,
    /// colonne confrontate `num` float64 (con NaN, -0.0 e null) e `txt`.
    /// Stati coperti: UNCHANGED (anche NaN==NaN), MODIFIED (valore, null e
    /// -0.0 vs 0.0), DELETED, ADDED.
    // Fixture con molti stati espliciti riga per riga su due batch: lunga per
    // costruzione (dati di test, niente logica).
    #[allow(clippy::too_many_lines)]
    fn diff_fixtures() -> (RecordBatch, RecordBatch) {
        let left = batch_of(
            vec![
                Field::new("id", DataType::Int64, true),
                Field::new("grp", DataType::Utf8, true),
                Field::new("num", DataType::Float64, true),
                Field::new("txt", DataType::Utf8, true),
                Field::new("solo_sinistra", DataType::Int64, true),
            ],
            vec![
                Arc::new(Int64Array::from(vec![
                    Some(1),
                    Some(2),
                    Some(3),
                    Some(4),
                    Some(5),
                    Some(6),
                ])),
                Arc::new(StringArray::from(vec![
                    Some("a"),
                    Some("b"),
                    None,
                    Some("d"),
                    Some("e"),
                    Some("f"),
                ])),
                Arc::new(Float64Array::from(vec![
                    Some(1.5),
                    Some(f64::NAN),
                    Some(-0.0),
                    None,
                    Some(5.0),
                    Some(6.0),
                ])),
                Arc::new(StringArray::from(vec![
                    Some("uno"),
                    Some("due"),
                    Some("tre"),
                    Some("quattro"),
                    Some("cinque"),
                    Some("sei"),
                ])),
                Arc::new(Int64Array::from(vec![
                    Some(10),
                    Some(20),
                    Some(30),
                    Some(40),
                    Some(50),
                    Some(60),
                ])),
            ],
        );
        let right = batch_of(
            vec![
                Field::new("id", DataType::Int64, true),
                Field::new("grp", DataType::Utf8, true),
                Field::new("num", DataType::Float64, true),
                Field::new("txt", DataType::Utf8, true),
                Field::new("solo_destra", DataType::Utf8, true),
            ],
            vec![
                Arc::new(Int64Array::from(vec![
                    Some(1),
                    Some(2),
                    Some(3),
                    Some(4),
                    Some(7),
                    None,
                ])),
                Arc::new(StringArray::from(vec![
                    Some("a"),
                    Some("b"),
                    None,
                    Some("d"),
                    Some("g"),
                    None,
                ])),
                Arc::new(Float64Array::from(vec![
                    Some(1.5),
                    Some(f64::NAN),
                    Some(0.0),
                    Some(4.0),
                    Some(7.0),
                    None,
                ])),
                Arc::new(StringArray::from(vec![
                    Some("uno"),
                    Some("due"),
                    Some("tre"),
                    None,
                    Some("sette"),
                    Some("null-key"),
                ])),
                Arc::new(StringArray::from(vec![
                    Some("r1"),
                    Some("r2"),
                    Some("r3"),
                    Some("r4"),
                    Some("r7"),
                    Some("rn"),
                ])),
            ],
        );
        (left, right)
    }

    fn diff_config(include_unchanged: &str) -> TableDiff {
        TableDiff {
            left_keys: vec!["id".into(), "grp".into()],
            right_keys: vec!["id".into(), "grp".into()],
            compare_columns: vec!["num".into(), "txt".into()],
            include_unchanged: include_unchanged.into(),
            separator: ", ".into(),
        }
    }

    #[test]
    fn table_diff_stati_misti_oracle() {
        let (left, right) = diff_fixtures();
        for include_unchanged in ["no", "yes"] {
            let config = diff_config(include_unchanged);
            let fast = table_diff(&left, &right, &config, &Limits::default()).expect("fast");
            let reference =
                table_diff_reference(&left, &right, &config, &Limits::default()).expect("ref");
            assert_batches_identical(&fast, &reference);
        }
        // Verifica puntuale degli stati attesi (ordine: sinistra, poi
        // solo-destra): id 1 UNCHANGED, id 2 UNCHANGED (NaN==NaN),
        // id 3 MODIFIED (-0.0 vs 0.0), id 4 MODIFIED (null vs valore),
        // id 5 DELETED, id 6 DELETED, id 7 ADDED, chiave null ADDED.
        let config = diff_config("no");
        let fast = table_diff(&left, &right, &config, &Limits::default()).expect("fast");
        let status_column = fast
            .column(fast.schema().index_of("_diff_status").expect("status"))
            .as_any()
            .downcast_ref::<StringArray>()
            .expect("status utf8");
        let states: Vec<&str> = (0..status_column.len())
            .map(|row| status_column.value(row))
            .collect();
        assert_eq!(
            states,
            vec!["MODIFIED", "MODIFIED", "DELETED", "DELETED", "ADDED", "ADDED"]
        );
    }

    #[test]
    fn table_diff_compare_default_oracle() {
        let (left, right) = diff_fixtures();
        // compare_columns vuoto: confronta tutte le colonne condivise non
        // chiave (num, txt); le colonne non condivise restano fuori.
        let config = TableDiff {
            compare_columns: Vec::new(),
            separator: " | ".into(),
            ..diff_config("yes")
        };
        let fast = table_diff(&left, &right, &config, &Limits::default()).expect("fast");
        let reference =
            table_diff_reference(&left, &right, &config, &Limits::default()).expect("ref");
        assert_batches_identical(&fast, &reference);
    }

    #[test]
    fn table_diff_input_vuoti_oracle() {
        let (left, right) = diff_fixtures();
        let empty_left = left.slice(0, 0);
        let empty_right = right.slice(0, 0);
        let config = diff_config("yes");
        for (l, r) in [
            (&empty_left, &right),
            (&left, &empty_right),
            (&empty_left, &empty_right),
        ] {
            let fast = table_diff(l, r, &config, &Limits::default()).expect("fast");
            let reference = table_diff_reference(l, r, &config, &Limits::default()).expect("ref");
            assert_batches_identical(&fast, &reference);
        }
    }

    #[test]
    fn table_diff_errori_identici() {
        let (left, right) = diff_fixtures();
        // Chiavi duplicate a sinistra e a destra: stesso messaggio.
        let dup_left = batch_of(
            vec![
                Field::new("id", DataType::Int64, true),
                Field::new("grp", DataType::Utf8, true),
                Field::new("num", DataType::Float64, true),
                Field::new("txt", DataType::Utf8, true),
            ],
            vec![
                Arc::new(Int64Array::from(vec![Some(1), Some(1)])),
                Arc::new(StringArray::from(vec![Some("a"), Some("a")])),
                Arc::new(Float64Array::from(vec![Some(1.0), Some(2.0)])),
                Arc::new(StringArray::from(vec![Some("x"), Some("y")])),
            ],
        );
        let config = diff_config("no");
        let fast = table_diff(&dup_left, &right, &config, &Limits::default());
        let reference = table_diff_reference(&dup_left, &right, &config, &Limits::default());
        assert_eq!(
            format!("{:?}", fast.expect_err("fast deve fallire")),
            format!("{:?}", reference.expect_err("ref deve fallire"))
        );
        let fast = table_diff(&left, &dup_left, &config, &Limits::default());
        let reference = table_diff_reference(&left, &dup_left, &config, &Limits::default());
        assert_eq!(
            format!("{:?}", fast.expect_err("fast deve fallire")),
            format!("{:?}", reference.expect_err("ref deve fallire"))
        );
        // Configurazioni non valide: stesso errore.
        for config in [
            TableDiff {
                left_keys: Vec::new(),
                ..diff_config("no")
            },
            TableDiff {
                left_keys: vec!["id".into()],
                right_keys: vec!["id".into(), "grp".into()],
                ..diff_config("no")
            },
            TableDiff {
                left_keys: vec!["manca".into(), "grp".into()],
                ..diff_config("no")
            },
            TableDiff {
                compare_columns: vec!["manca".into()],
                ..diff_config("no")
            },
        ] {
            let fast = table_diff(&left, &right, &config, &Limits::default());
            let reference = table_diff_reference(&left, &right, &config, &Limits::default());
            assert_eq!(
                format!("{:?}", fast.expect_err("fast deve fallire")),
                format!("{:?}", reference.expect_err("ref deve fallire"))
            );
        }
        // Tipi diversi sulle colonne confrontate: stesso errore di
        // diff_values.
        let typed_right = batch_of(
            vec![
                Field::new("id", DataType::Int64, true),
                Field::new("grp", DataType::Utf8, true),
                Field::new("num", DataType::Utf8, true),
                Field::new("txt", DataType::Utf8, true),
            ],
            vec![
                Arc::new(Int64Array::from(vec![Some(1)])),
                Arc::new(StringArray::from(vec![Some("a")])),
                Arc::new(StringArray::from(vec![Some("1.5")])),
                Arc::new(StringArray::from(vec![Some("uno")])),
            ],
        );
        let config = diff_config("no");
        let fast = table_diff(&left, &typed_right, &config, &Limits::default());
        let reference = table_diff_reference(&left, &typed_right, &config, &Limits::default());
        assert_eq!(
            format!("{:?}", fast.expect_err("fast deve fallire")),
            format!("{:?}", reference.expect_err("ref deve fallire"))
        );
        // Limiti: stesso errore.
        let limits = Limits {
            max_rows: 1,
            ..Limits::default()
        };
        let fast = table_diff(&left, &right, &config, &limits);
        let reference = table_diff_reference(&left, &right, &config, &limits);
        assert_eq!(
            format!("{:?}", fast.expect_err("fast deve fallire")),
            format!("{:?}", reference.expect_err("ref deve fallire"))
        );
        let limits = Limits {
            max_columns: 2,
            ..Limits::default()
        };
        let fast = table_diff(&left, &right, &config, &limits);
        let reference = table_diff_reference(&left, &right, &config, &limits);
        assert_eq!(
            format!("{:?}", fast.expect_err("fast deve fallire")),
            format!("{:?}", reference.expect_err("ref deve fallire"))
        );
    }
}
