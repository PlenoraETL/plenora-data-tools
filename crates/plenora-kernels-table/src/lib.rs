//! plenora-kernels-table — kernel tabellari puri `&RecordBatch -> Result<RecordBatch>`
//! (architettura.md).
//!
//! I moduli kernel — `columns`, `strings`, `cleansing`, `filtering`,
//! `dates`, `utility`, `analysis`, `aggregation`, `reshape`, `joins`,
//! `setops`, `security`, `quality`, `governance`, `formula`, `expressions`,
//! `spill` — con gli helper che condividono.

use serde::{Deserialize, Serialize};

/// Limiti dei kernel tabellari.
///
/// Non coincide con `plenora_core::limits::Limits` (D19,
/// errori-e-limiti.md): quello non ha `max_columns` e `max_split_columns` e
/// sostituisce `max_rows` con `RowLimits`. La mappatura fra i due e' una
/// decisione semantica dell'engine, non un adattamento meccanico.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Limits {
    pub max_rows: usize,
    pub max_columns: usize,
    pub max_string_bytes: usize,
    pub max_regex_bytes: usize,
    pub max_split_columns: usize,
    pub max_governed_memory_bytes: usize,
    pub max_temp_bytes: u64,
    pub spill_partitions: usize,
}

/// Limiti **interni ai kernel**: non sono dichiarabili in un piano, e nessuna
/// conversione dai limiti del piano puo' produrli.
///
/// Proteggono invarianti dei kernel — quante colonne puo' generare una
/// `flatten_json` o uno `split` — che il formato del piano non nomina, e per
/// questo mancano da `plenora_core::limits::Limits`. Sono dichiarati dove
/// sono imposti, perche' non sembrino ereditati dal piano.
pub mod limiti_interni {
    /// Colonne totali che un batch puo' raggiungere dopo un'espansione.
    pub const MAX_COLUMNS: usize = 4_096;

    /// Colonne che una singola operazione di split puo' produrre.
    pub const MAX_SPLIT_COLUMNS: usize = 256;
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_rows: 10_000_000,
            max_columns: limiti_interni::MAX_COLUMNS,
            max_string_bytes: 16 * 1024 * 1024,
            max_regex_bytes: 4_096,
            max_split_columns: limiti_interni::MAX_SPLIT_COLUMNS,
            // Stessa autorita' di `plenora_core::Limits::default()`: i due
            // default non possono divergere.
            max_governed_memory_bytes:
                plenora_core::limits::DEFAULT_MAX_GOVERNED_MEMORY_BYTES_USIZE,
            max_temp_bytes: plenora_core::limits::DEFAULT_MAX_TEMP_BYTES,
            // 64 sta in qualunque `usize` che questo progetto supporti; la
            // conversione e' esatta per il VALORE, non per i tipi.
            spill_partitions: plenora_core::limits::DEFAULT_SPILL_PARTITIONS as usize,
        }
    }
}

pub mod aggregation;
pub mod analysis;
pub mod analyze;
pub mod cleansing;
pub mod columns;
pub mod dates;
pub mod exact_compare;
pub mod expressions;
pub mod filtering;
mod float64_source;
pub mod formula;
pub mod fuzzy;
pub mod governance;
pub mod hashing;
pub mod joins;
pub mod quality;
pub mod reshape;
pub mod security;
pub mod setops;
pub mod spill;
pub mod strings;
pub mod utility;

use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::sync::Arc;

use plenora_core::arrow::array::{
    types::Int32Type, Array, ArrayRef, BinaryArray, BooleanArray, Date32Array, Decimal128Array,
    DictionaryArray, Float64Array, Int64Array, RecordBatch, StringArray, TimestampMillisecondArray,
    UInt32Array, UInt64Array,
};
use plenora_core::arrow::schema::{DataType, Field, Schema};

use plenora_core::diagnostics::{
    RowDiagnosticExample, RowDiagnosticScope, RowDiagnostics, RowDiagnosticsCompleteness,
    ROW_DIAGNOSTICS_CONTRACT, ROW_DIAGNOSTICS_INDEX_BASIS,
};
use plenora_core::{ErrorPhase, PlenoraError, Result};

pub(crate) struct RowRejection<'a> {
    pub row: usize,
    pub cause: &'static str,
    pub column: Option<&'a str>,
}

/// Messaggi degli errori di valutazione attribuibili alla riga (dato, non
/// piano): costanti uniche condivise fra i siti di costruzione
/// (`expressions`, `formula`) e la classificazione — mai dati di riga.
pub(crate) const DIVISION_BY_ZERO_MESSAGE: &str = "divisione per zero";
pub(crate) const NON_FINITE_INPUT_MESSAGE: &str = "expression non accetta numeri non finiti";
pub(crate) const NON_FINITE_RESULT_MESSAGE: &str = "risultato expression non finito";

/// Classifica un errore di valutazione per riga: `Some(causa)` solo se il
/// difetto dipende dal valore della cella (divisione per zero, numero non
/// finito in ingresso o in uscita). Gli errori di tipo/piano (`Schema` con
/// altri messaggi, `InvalidPlan`, `Internal`) restano non row-scoped e
/// propagano invariati.
pub(crate) fn row_eval_failure_cause(error: &PlenoraError) -> Option<&'static str> {
    let PlenoraError::Schema(message) = error else {
        return None;
    };
    match message.as_str() {
        DIVISION_BY_ZERO_MESSAGE => Some("evaluation.division_by_zero"),
        NON_FINITE_INPUT_MESSAGE => Some("evaluation.non_finite_input"),
        NON_FINITE_RESULT_MESSAGE => Some("evaluation.non_finite_result"),
        _ => None,
    }
}

/// Chiude fail-closed una batch quando una validazione per-riga trova difetti.
/// Conteggi ed esempi sono deterministici, bounded e non contengono valori.
pub(crate) fn reject_rows(rejections: &[RowRejection<'_>], message: &'static str) -> Result<()> {
    const EXAMPLES_LIMIT: u64 = 10;
    if rejections.is_empty() {
        return Ok(());
    }
    let mut rows = BTreeMap::new();
    for rejection in rejections {
        rows.entry(rejection.row).or_insert(rejection);
    }
    let observed_total = u64::try_from(rows.len())
        .map_err(|_| PlenoraError::Internal("troppe rejection row-scoped".into()))?;
    let mut counts = BTreeMap::new();
    let mut examples = Vec::new();
    for rejection in rows.values() {
        let count = counts.entry(rejection.cause.to_owned()).or_insert(0_u64);
        *count = count.checked_add(1).ok_or_else(|| {
            PlenoraError::Internal("overflow del conteggio causa row-scoped".into())
        })?;
        if u64::try_from(examples.len())
            .map_err(|_| PlenoraError::Internal("troppi esempi row-scoped".into()))?
            < EXAMPLES_LIMIT
        {
            examples.push(RowDiagnosticExample {
                source_index: u64::try_from(rejection.row).map_err(|_| {
                    PlenoraError::Internal("indice sorgente non rappresentabile".into())
                })?,
                cause: rejection.cause.to_owned(),
                column: rejection.column.map(ToOwned::to_owned),
                key: None,
                write_state: None,
            });
        }
    }
    let report = RowDiagnostics {
        contract: ROW_DIAGNOSTICS_CONTRACT.to_owned(),
        scope: RowDiagnosticScope::Read,
        index_basis: ROW_DIAGNOSTICS_INDEX_BASIS.to_owned(),
        completeness: RowDiagnosticsCompleteness::Complete,
        knowledge_limits: None,
        observed_total,
        total: Some(observed_total),
        input_total: None,
        counts,
        examples_limit: EXAMPLES_LIMIT,
        examples_truncated: observed_total > EXAMPLES_LIMIT,
        examples,
        diagnostic_state_counts: None,
        write_outcome: None,
    };
    Err(PlenoraError::DataMapping(message.into())
        .with_phase(ErrorPhase::Read)
        .with_row_diagnostics(report))
}

/// Indice della colonna `name` nel batch.
///
/// # Errors
///
/// - `Schema`: colonna assente dallo schema.
pub fn column_index(batch: &RecordBatch, name: &str) -> Result<usize> {
    batch
        .schema()
        .index_of(name)
        .map_err(|_| PlenoraError::Schema(format!("colonna non trovata: {name}")))
}

/// Colonna `name` del batch come `StringArray` (Utf8).
///
/// # Errors
///
/// - `Schema`: colonna assente o non di tipo Utf8.
pub fn utf8_column<'a>(
    batch: &'a RecordBatch,
    name: &str,
) -> Result<&'a plenora_core::arrow::array::StringArray> {
    let index = column_index(batch, name)?;
    batch
        .column(index)
        .as_any()
        .downcast_ref::<plenora_core::arrow::array::StringArray>()
        .ok_or_else(|| PlenoraError::Schema(format!("la colonna {name} deve essere Utf8")))
}

/// Batch con la colonna `name` sostituita da `array` (o aggiunta in coda se
/// assente), preservando i metadati dello schema.
///
/// # Errors
///
/// - `Schema`: `array` ha un numero di righe diverso dal batch, oppure lo
///   schema risultante non e' coerente con le colonne.
pub fn replace_or_append(
    batch: &RecordBatch,
    name: &str,
    data_type: DataType,
    nullable: bool,
    array: ArrayRef,
) -> Result<RecordBatch> {
    if array.len() != batch.num_rows() {
        return Err(PlenoraError::Schema(format!(
            "lunghezza output {} diversa dalle righe {}",
            array.len(),
            batch.num_rows()
        )));
    }
    let mut fields: Vec<Field> = batch
        .schema()
        .fields()
        .iter()
        .map(|field| field.as_ref().clone())
        .collect();
    let mut columns = batch.columns().to_vec();
    if let Ok(index) = batch.schema().index_of(name) {
        fields[index] = Field::new(name, data_type, nullable);
        columns[index] = array;
    } else {
        fields.push(Field::new(name, data_type, nullable));
        columns.push(array);
    }
    let schema = Schema::new_with_metadata(fields, batch.schema().metadata().clone());
    batch_with_rows(Arc::new(schema), columns, batch.num_rows())
}

/// Byte per riga di una singola colonna, con un PAVIMENTO per le colonne
/// prive di righe da misurare.
///
/// Una colonna vuota non si puo' misurare ma occupa comunque spazio
/// nell'output: senza pavimento un input vuoto con molte colonne peserebbe
/// zero nella stima.
#[must_use]
pub fn column_bytes_per_row(array: &dyn Array) -> usize {
    let rows = array.len();
    if rows == 0 {
        return type_bytes_floor(array.data_type());
    }
    array.get_array_memory_size().div_ceil(rows)
}

/// Pavimento per riga di un tipo Arrow, quando non ci sono righe da misurare.
#[must_use]
pub fn type_bytes_floor(data_type: &DataType) -> usize {
    // Tipi a lunghezza variabile: otto byte, il minimo fra offset e
    // puntatore. Piu' un byte di validita'. La somma non puo' saturare:
    // `saturating_add` evita solo un `Result` senza errore possibile.
    data_type.primitive_width().unwrap_or(8).saturating_add(1)
}

/// Larghezza per riga di un valore CONVERTITO IN TESTO, per tipo.
///
/// Per le operazioni che producono `Utf8` da altri tipi (`melt` con
/// `type_policy = "string"`): la larghezza binaria della sorgente
/// sottostima il testo, che e' memoria del risultato.
///
/// Valori: massimo della forma decimale dei formattatori del progetto, piu'
/// offset e validita' della colonna `Utf8`. Per i tipi gia' testuali o
/// binari resta il pavimento: la larghezza reale la misura il chiamante.
#[must_use]
pub fn text_bytes_floor(data_type: &DataType) -> usize {
    let cifre: usize = match data_type {
        // "-9223372036854775808" e "18446744073709551615": venti caratteri
        // entrambi, il primo per il segno e il secondo per la magnitudine.
        DataType::Int64 | DataType::UInt64 => 20,
        // notazione esponenziale di un double, con segno ed esponente
        DataType::Float64 => 24,
        // 38 cifre, segno, separatore decimale
        DataType::Decimal128(_, _) => 40,
        // "YYYY-MM-DD", con margine per gli anni fuori dalle quattro cifre
        DataType::Date32 => 16,
        // "YYYY-MM-DDTHH:MM:SS.sssZ" con margine per la timezone
        DataType::Timestamp(_, _) => 32,
        // "false"
        DataType::Boolean => 5,
        // Gia' testo o byte: nessuna conversione, la misura la fa il
        // chiamante sulla colonna reale.
        _ => 0,
    };
    cifre.saturating_add(type_bytes_floor(&DataType::Utf8))
}

/// Larghezza per riga della forma TESTUALE, misurata sull'array reale.
///
/// Una `DictionaryArray` conta ogni testo una volta, nel dizionario, mentre
/// l'output `Utf8` lo materializza per ogni riga: misurata sull'input, una
/// stringa lunga ripetuta da molte chiavi sottostima di ordini di grandezza
/// e il preflight autorizzerebbe l'allocazione che deve impedire.
///
/// Il limite superiore esatto e' la voce piu' lunga del dizionario; si
/// scorre una volta per colonna, non per riga.
#[must_use]
pub fn text_bytes_per_row(array: &dyn Array) -> usize {
    if let Some(values) = array.as_any().downcast_ref::<DictionaryArray<Int32Type>>() {
        let piu_lunga = values
            .values()
            .as_any()
            .downcast_ref::<StringArray>()
            .map_or(0, |testi| {
                (0..testi.len())
                    .filter(|riga| !testi.is_null(*riga))
                    .map(|riga| testi.value(riga).len())
                    .max()
                    .unwrap_or(0)
            });
        return piu_lunga.saturating_add(type_bytes_floor(&DataType::Utf8));
    }
    // Per i tipi gia' testuali o binari la misura sulla colonna e' la stima
    // migliore; per i tipi a larghezza fissa la forma decimale la conosce
    // `text_bytes_floor`. Si prende il maggiore: nessuno dei due sottostima
    // l'altro in modo sistematico.
    column_bytes_per_row(array).max(text_bytes_floor(array.data_type()))
}

/// `true` se [`scalar_as_string`] sa convertire questo tipo in testo.
///
/// Serve a rifiutare PRIMA di allocare (per esempio `melt` con
/// `type_policy = "string"`), invece che a meta' scansione.
///
/// Il predicato segue esattamente il formatter: di `Timestamp` accetta solo
/// `Millisecond`, l'unico array su cui fa downcast; di `Decimal128` solo le
/// scale `0..=38`, perche' il formatter rifiuta le scale negative (valide in
/// Arrow) e `10^scala` trabocca oltre 38.
///
/// E' una prevalidazione di tipo, non di valore: un `Binary` non UTF-8, una
/// data o un istante fuori intervallo, una timezone non valida, una chiave
/// dictionary fuori dal dizionario falliscono ancora durante la scansione.
#[must_use]
pub fn text_convertible(data_type: &DataType) -> bool {
    match data_type {
        DataType::Utf8
        | DataType::Int64
        | DataType::Float64
        | DataType::Boolean
        | DataType::UInt64
        | DataType::Date32
        | DataType::Binary => true,
        // Solo i millisecondi: e' l'unico array su cui il formatter fa
        // downcast.
        DataType::Timestamp(unit, _) => {
            matches!(unit, plenora_core::arrow::schema::TimeUnit::Millisecond)
        }
        // Scala non negativa e dentro il dominio di `10^scala` in `i128`.
        DataType::Decimal128(_, scale) => (0..=38).contains(scale),
        DataType::Dictionary(key, value) => {
            matches!(**key, DataType::Int32) && matches!(**value, DataType::Utf8)
        }
        _ => false,
    }
}

/// Byte per riga di un intero batch: somma delle colonne.
///
/// Chi stima un output la compone secondo la propria operazione (somma dei
/// lati per un prodotto cartesiano, massimo per un impilamento).
///
/// # Errors
///
/// [`PlenoraError::ResourceLimit`] se la somma non e' rappresentabile.
/// L'aritmetica e' controllata, non saturante: con
/// `max_governed_memory_bytes` a fondo scala una somma saturata passerebbe il
/// confronto.
pub fn batch_bytes_per_row(batch: &RecordBatch) -> Result<usize> {
    batch.columns().iter().try_fold(0_usize, |totale, column| {
        totale
            .checked_add(column_bytes_per_row(column.as_ref()))
            .ok_or_else(|| {
                PlenoraError::ResourceLimit(
                    "larghezza di riga non rappresentabile: stima del budget non affidabile".into(),
                )
            })
    })
}

/// Rifiuto PREVENTIVO di un output troppo grande, prima di allocarlo.
///
/// Un tetto verificato dopo la costruzione non protegge i byte: un
/// `cross_join` entro `max_rows` puo' allocare molto oltre
/// `max_governed_memory_bytes` ed esaurire la memoria invece di fallire.
///
/// `bytes_per_row` e' la larghezza di una riga di OUTPUT e la calcola il
/// chiamante: solo il kernel sa come si compone la propria riga.
///
/// E' una stima, non una misura: resta fuori cio' che l'implementazione
/// alloca oltre il risultato (indici, tabelle hash, temporanei) se il
/// chiamante non lo include. Impedisce le esplosioni di ordini di grandezza,
/// non rende `max_governed_memory_bytes` un tetto duro; vedi
/// errori-e-limiti.md#che-cosa-la-memoria-governata-non-garantisce.
///
/// # Errors
///
/// [`PlenoraError::ResourceLimit`] se la stima supera `max_governed_memory_bytes`, o
/// se il prodotto non e' rappresentabile — un numero che ha perso il conto
/// non puo' autorizzare un'allocazione.
pub fn preflight_output_bytes(
    operation: &str,
    output_rows: usize,
    bytes_per_row: usize,
    limits: &Limits,
) -> Result<()> {
    if output_rows == 0 || bytes_per_row == 0 {
        return Ok(());
    }
    let stima = bytes_per_row.checked_mul(output_rows).ok_or_else(|| {
        PlenoraError::ResourceLimit(format!(
            "{operation}: stima dell'output non rappresentabile \
             ({output_rows} righe x {bytes_per_row} byte)"
        ))
    })?;
    if stima > limits.max_governed_memory_bytes {
        return Err(PlenoraError::ResourceLimit(format!(
            "{operation}: l'output stimato ({stima} byte: {output_rows} righe x \
             {bytes_per_row} byte) supera max_governed_memory_bytes ({})",
            limits.max_governed_memory_bytes
        )));
    }
    Ok(())
}

/// Costruttore di `RecordBatch` che DICHIARA la cardinalita'.
///
/// Ri-esportato da [`plenora_core::batch_with_rows`]: l'invariante «un batch
/// a zero colonne puo' avere righe» vale per tutto il workspace.
pub use plenora_core::batch_with_rows;

/// Verifica sullo SCHEMA tutto cio' che la conversione in testo puo'
/// rifiutare senza guardare i valori.
///
/// Oltre al tipo ([`text_convertible`]) verifica la timezone, che sta nello
/// schema ma `scalar_as_string` risolve a ogni riga. Resta fuori solo cio'
/// che dipende dal contenuto della cella.
///
/// # Errors
///
/// `PlenoraError::Schema` con il nome della colonna: tipo non convertibile,
/// oppure timezone Arrow non risolvibile.
pub fn validate_text_convertible(data_type: &DataType, column: &str) -> Result<()> {
    if !text_convertible(data_type) {
        return Err(PlenoraError::Schema(format!(
            "colonna `{column}` di tipo {data_type:?} non convertibile in testo"
        )));
    }
    if let DataType::Timestamp(_, Some(timezone)) = data_type {
        if timezone.parse::<chrono_tz::Tz>().is_err() {
            return Err(PlenoraError::Schema(format!(
                "colonna `{column}`: timezone Arrow `{timezone}` non valida"
            )));
        }
    }
    Ok(())
}

/// Suffissi provati da [`resolve_output_names`] per evitare una collisione:
/// da `_1` a `_99`.
pub const MAX_SUFFISSI_COLLISIONE: u32 = 99;

/// Risolve PIU' nomi di output evitando le collisioni, in sequenza.
///
/// Ogni nome si confronta con lo schema di input **e con i nomi risolti
/// prima di lui**; se occupato riceve il primo suffisso libero fra i
/// [`MAX_SUFFISSI_COLLISIONE`], e poi viene riservato.
///
/// La sequenza e' necessaria: con input `v` e richiesta `["v", "v_1"]`, una
/// risoluzione indipendente darebbe due colonne `v_1`, cioe' uno schema con
/// nomi duplicati. Per questo la funzione prende tutti i nomi insieme.
///
/// Ogni candidato generato passa [`validate_output_name`], perche' il
/// suffisso allunga il nome oltre il limite; un candidato non valido conta
/// come occupato.
///
/// # Errors
///
/// - `InvalidPlan`: un nome richiesto non e' valido (vedi
///   [`validate_output_name`]), oppure nessuno dei suffissi ammessi produce
///   un nome che sia insieme libero e valido.
pub fn resolve_output_names<'a>(
    occupati: impl IntoIterator<Item = &'a str>,
    richiesti: &[&str],
) -> Result<Vec<String>> {
    let mut presi: std::collections::BTreeSet<String> =
        occupati.into_iter().map(str::to_owned).collect();
    let mut risolti = Vec::with_capacity(richiesti.len());
    for nome in richiesti {
        validate_output_name(nome)?;
        let scelto = if presi.contains(*nome) {
            (1..=MAX_SUFFISSI_COLLISIONE)
                .map(|indice| format!("{nome}_{indice}"))
                .find(|candidato| {
                    !presi.contains(candidato) && validate_output_name(candidato).is_ok()
                })
                .ok_or_else(|| {
                    PlenoraError::InvalidPlan(format!(
                        "impossibile evitare collisione {nome}: nessuno \
                         dei {MAX_SUFFISSI_COLLISIONE} suffissi produce \
                         un nome insieme libero e valido"
                    ))
                })?
        } else {
            (*nome).to_owned()
        };
        // La riserva e' il punto: senza, il nome successivo potrebbe
        // scegliere lo stesso candidato.
        presi.insert(scelto.clone());
        risolti.push(scelto);
    }
    Ok(risolti)
}

/// Valida il nome di una colonna di output (non vuoto, <= 1024 byte).
///
/// # Errors
///
/// - `InvalidPlan`: nome vuoto (o solo spazi) oppure oltre 1024 byte.
pub fn validate_output_name(name: &str) -> Result<()> {
    if name.trim().is_empty() {
        return Err(PlenoraError::InvalidPlan(
            "il nome della colonna di output e' vuoto".into(),
        ));
    }
    if name.len() > 1_024 {
        return Err(PlenoraError::InvalidPlan(
            "il nome della colonna supera 1024 byte".into(),
        ));
    }
    Ok(())
}

/// Valore testuale di una cella `Dictionary(Int32, Utf8)`, in prestito, con
/// il **null logico** risolto.
///
/// `Ok(None)` se la chiave e' nulla **oppure** punta a una entry nulla del
/// dizionario: `DictionaryArray::is_null` vede solo la prima, e la seconda
/// verrebbe letta come stringa vuota.
///
/// Il controllo dei limiti della chiave sta qui: `value()` fuori intervallo
/// va in panico (gate R6).
///
/// # Errors
///
/// `Schema` se il dizionario non contiene `Utf8`, se la chiave e' negativa o
/// se punta oltre la fine del dizionario.
pub fn dictionary_utf8_value(
    values: &DictionaryArray<Int32Type>,
    row: usize,
) -> Result<Option<&str>> {
    if values.is_null(row) {
        return Ok(None);
    }
    let dictionary = values
        .values()
        .as_any()
        .downcast_ref::<StringArray>()
        .ok_or_else(|| PlenoraError::Schema("dictionary non contiene Utf8".into()))?;
    let key = usize::try_from(values.keys().value(row))
        .map_err(|_| PlenoraError::Schema("chiave dictionary negativa".into()))?;
    if key >= dictionary.len() {
        return Err(PlenoraError::Schema(
            "chiave dictionary oltre il dizionario".into(),
        ));
    }
    if dictionary.is_null(key) {
        // Chiave valida che punta a una entry nulla: la riga e' nulla.
        return Ok(None);
    }
    Ok(Some(dictionary.value(key)))
}

/// `true` se la riga e' nulla **logicamente**, non solo nella bitmap di
/// primo livello.
///
/// Differisce da `Array::is_null` solo per `Dictionary(Int32, Utf8)` con
/// chiave valida su entry nulla. Ogni percorso che decide la nullita' di una
/// riga passa di qui, perche' due percorsi non diano due risposte.
///
/// Una chiave malformata non e' null: risponde `false`, e l'errore lo da'
/// [`dictionary_utf8_value`].
#[must_use]
pub fn is_logically_null(array: &dyn Array, row: usize) -> bool {
    if array.is_null(row) {
        return true;
    }
    array
        .as_any()
        .downcast_ref::<DictionaryArray<Int32Type>>()
        .is_some_and(|values| matches!(dictionary_utf8_value(values, row), Ok(None)))
}

/// Valore scalare della riga come `String` (profilo scalare testuale).
/// `None` se la riga e' null.
///
/// # Errors
///
/// - `InvalidPlan`: epoch date32 non valida (guardia interna);
/// - `Schema`: valore date32/timestamp fuori intervallo, timezone Arrow non
///   valida, decimal128 incoerente o con scala non supportata, binary non
///   UTF-8, dictionary non Utf8, tipo non supportato dal profilo scalare.
pub fn scalar_as_string(array: &dyn Array, row: usize) -> Result<Option<String>> {
    if array.is_null(row) {
        return Ok(None);
    }
    if let Some(values) = array.as_any().downcast_ref::<StringArray>() {
        return Ok(Some(values.value(row).to_owned()));
    }
    if let Some(values) = array.as_any().downcast_ref::<Int64Array>() {
        return Ok(Some(values.value(row).to_string()));
    }
    if let Some(values) = array.as_any().downcast_ref::<Float64Array>() {
        return Ok(Some(values.value(row).to_string()));
    }
    if let Some(values) = array.as_any().downcast_ref::<BooleanArray>() {
        return Ok(Some(values.value(row).to_string()));
    }
    if let Some(values) = array.as_any().downcast_ref::<UInt64Array>() {
        return Ok(Some(values.value(row).to_string()));
    }
    if let Some(values) = array.as_any().downcast_ref::<Date32Array>() {
        let epoch = chrono::NaiveDate::from_ymd_opt(1970, 1, 1)
            .ok_or_else(|| PlenoraError::InvalidPlan("epoch date32 non valida".into()))?;
        let date = epoch
            .checked_add_signed(chrono::TimeDelta::days(i64::from(values.value(row))))
            .ok_or_else(|| PlenoraError::Schema("date32 fuori intervallo".into()))?;
        return Ok(Some(date.format("%Y-%m-%d").to_string()));
    }
    if let Some(values) = array.as_any().downcast_ref::<TimestampMillisecondArray>() {
        let timestamp = chrono::DateTime::<chrono::Utc>::from_timestamp_millis(values.value(row))
            .ok_or_else(|| PlenoraError::Schema("timestamp fuori intervallo".into()))?;
        if let DataType::Timestamp(_, Some(timezone)) = values.data_type() {
            let timezone = timezone
                .parse::<chrono_tz::Tz>()
                .map_err(|_| PlenoraError::Schema("timezone Arrow non valida".into()))?;
            return Ok(Some(timestamp.with_timezone(&timezone).to_rfc3339()));
        }
        return Ok(Some(timestamp.to_rfc3339()));
    }
    if let Some(values) = array.as_any().downcast_ref::<Decimal128Array>() {
        let DataType::Decimal128(_, scale) = values.data_type() else {
            return Err(PlenoraError::Schema("decimal128 incoerente".into()));
        };
        let value = values.value(row);
        let scale = u32::try_from(*scale)
            .map_err(|_| PlenoraError::Schema("scala decimal negativa non supportata".into()))?;
        let factor = 10_i128
            .checked_pow(scale)
            .ok_or_else(|| PlenoraError::Schema("scala decimal fuori intervallo".into()))?;
        let magnitude = value.unsigned_abs();
        let whole = magnitude / factor.unsigned_abs();
        let fraction = magnitude % factor.unsigned_abs();
        let sign = if value < 0 { "-" } else { "" };
        return if scale == 0 {
            Ok(Some(format!("{sign}{whole}")))
        } else {
            Ok(Some(format!(
                "{sign}{whole}.{fraction:0width$}",
                width = usize::try_from(scale).unwrap_or_default()
            )))
        };
    }
    if let Some(values) = array.as_any().downcast_ref::<BinaryArray>() {
        return std::str::from_utf8(values.value(row))
            .map(|value| Some(value.to_owned()))
            .map_err(|_| PlenoraError::Schema("binary non contiene UTF-8 valido".into()));
    }
    if let Some(values) = array.as_any().downcast_ref::<DictionaryArray<Int32Type>>() {
        // Null logico e limiti della chiave nel risolutore condiviso.
        return Ok(dictionary_utf8_value(values, row)?.map(ToOwned::to_owned));
    }
    Err(PlenoraError::Schema(format!(
        "tipo {:?} non supportato dal profilo scalare",
        array.data_type()
    )))
}

// ---------------------------------------------------------------------------
// Conversioni intero -> f64 con verifica di rappresentabilita' esatta.
//
// `ToPrimitive::to_f64()` non e' un test: per i64/u64/i128 rende sempre
// `Some`, arrotondando, e sopra 2^53 due interi distinti diventano lo stesso
// double. Nemmeno il round-trip `value as f64 as i64 == value` lo e': il
// cast satura, e `i64::MAX` lo supera.
//
// Criterio esatto: un intero e' rappresentabile se i bit fra il primo e
// l'ultimo 1 stanno in 53 (2^54 resta accettato).
// ---------------------------------------------------------------------------

/// Bit di mantissa di un `f64`, bit implicito compreso.
const F64_SIGNIFICAND_BITS: u32 = 53;

/// `true` se `magnitude` ha al piu' 53 bit significativi, cioe' se esiste un
/// `f64` che lo rappresenta esattamente.
const fn magnitude_fits_f64(magnitude: u128) -> bool {
    if magnitude == 0 {
        return true;
    }
    let significant = u128::BITS - magnitude.leading_zeros() - magnitude.trailing_zeros();
    significant <= F64_SIGNIFICAND_BITS
}

/// Conversione `i64` -> `f64` esatta, oppure `None`.
#[allow(clippy::cast_precision_loss)] // Guardato da `magnitude_fits_f64`: nessuna perdita possibile.
#[must_use]
pub fn exact_f64_from_i64(value: i64) -> Option<f64> {
    magnitude_fits_f64(u128::from(value.unsigned_abs())).then_some(value as f64)
}

/// Conversione `u64` -> `f64` esatta, oppure `None`.
#[allow(clippy::cast_precision_loss)] // Guardato da `magnitude_fits_f64`.
#[must_use]
pub fn exact_f64_from_u64(value: u64) -> Option<f64> {
    magnitude_fits_f64(u128::from(value)).then_some(value as f64)
}

/// Conversione `i128` -> `f64` esatta, oppure `None`.
#[allow(clippy::cast_precision_loss)] // Guardato da `magnitude_fits_f64`.
#[must_use]
pub fn exact_f64_from_i128(value: i128) -> Option<f64> {
    magnitude_fits_f64(value.unsigned_abs()).then_some(value as f64)
}

/// Conversione `Decimal128` -> `f64` esatta, oppure `None`.
///
/// Non basta che `unscaled` sia rappresentabile: `1` a scala `1` vale `0.1`,
/// che nessun double rappresenta.
///
/// Criterio: `10^scale = 2^scale * 5^scale`, quindi `5^scale` deve dividere
/// `unscaled` e il quoziente stare in 53 bit; la divisione per `2^scale` e'
/// esatta.
#[allow(clippy::cast_precision_loss)] // Guardato da `magnitude_fits_f64`.
#[must_use]
pub fn exact_f64_from_decimal128(unscaled: i128, scale: i8) -> Option<f64> {
    // Lo zero e' esatto a qualunque scala: deciso prima, perche' `10^|scale|`
    // oltre +-38 fallirebbe, e Arrow ammette scale negative illimitate.
    if unscaled == 0 {
        return Some(0.0);
    }
    if scale == 0 {
        return exact_f64_from_i128(unscaled);
    }
    if scale < 0 {
        // Scala negativa: `unscaled * 2^a * 5^a` con `a = -scale`. Serve
        // `5^a <= 2^53`: oltre nessun valore non nullo e' esatto.
        let a = u32::from(scale.unsigned_abs());
        if a > MAX_EXACT_POW5 {
            return None;
        }
        // Le potenze di due di `unscaled` passano all'esponente prima di
        // moltiplicare per `5^a`: altrimenti `2^126 * 10`, esatto in `f64`,
        // traboccherebbe `i128` nel prodotto intermedio.
        let due_estratti = unscaled.trailing_zeros();
        let dispari = unscaled >> due_estratti;
        let five = i128::checked_pow(5, a)?;
        let scaled = dispari.checked_mul(five)?;
        let base = exact_f64_from_i128(scaled)?;
        let esponente = i32::try_from(a)
            .ok()?
            .checked_add(i32::try_from(due_estratti).ok()?)?;
        let valore = base * 2_f64.powi(esponente);
        // La potenza di due puo' portare fuori dalla gamma dei double: un
        // valore infinito non e' una rappresentazione esatta.
        return valore.is_finite().then_some(valore);
    }
    // Scala positiva: `unscaled / (2^s * 5^s)`. Serve che `5^s` divida
    // `unscaled`; per `s >= 55` nessun `i128` non nullo e' divisibile per
    // `5^s` (5^55 supera gia' `i128::MAX`), quindi il `?` che segue nega
    // l'esattezza per la stessa ragione per cui la negherebbe la divisione.
    let exponent = u32::from(scale.unsigned_abs());
    let five = i128::checked_pow(5, exponent)?;
    if unscaled % five != 0 {
        return None;
    }
    let quotient = unscaled / five;
    if !magnitude_fits_f64(quotient.unsigned_abs()) {
        return None;
    }
    // `2^-scale` e' esatto e la moltiplicazione per una potenza di due non
    // arrotonda (scale <= 38, nessun subnormale in gioco).
    Some((quotient as f64) * 2_f64.powi(-i32::from(scale)))
}

/// Valore scalare della riga come `f64`. `None` se la riga e' null.
///
/// Le conversioni da intero sono **esatte o errore** `Schema`. Dove
/// l'arrotondamento e' voluto si usa [`scalar_as_f64_rounded`].
///
/// # Errors
///
/// - `Schema`: intero/timestamp/decimal128 non rappresentabile come f64,
///   decimal128 incoerente, testo non convertibile in numero, tipo non
///   convertibile in numero.
pub fn scalar_as_f64(array: &dyn Array, row: usize) -> Result<Option<f64>> {
    if array.is_null(row) {
        return Ok(None);
    }
    if let Some(values) = array.as_any().downcast_ref::<Float64Array>() {
        return Ok(Some(values.value(row)));
    }
    if let Some(values) = array.as_any().downcast_ref::<Int64Array>() {
        return exact_f64_from_i64(values.value(row))
            .map(Some)
            .ok_or_else(|| PlenoraError::Schema("intero non rappresentabile come f64".into()));
    }
    if let Some(values) = array.as_any().downcast_ref::<UInt64Array>() {
        return exact_f64_from_u64(values.value(row))
            .map(Some)
            .ok_or_else(|| PlenoraError::Schema("uint64 non rappresentabile come f64".into()));
    }
    if let Some(values) = array.as_any().downcast_ref::<Date32Array>() {
        return Ok(Some(f64::from(values.value(row))));
    }
    if let Some(values) = array.as_any().downcast_ref::<TimestampMillisecondArray>() {
        return exact_f64_from_i64(values.value(row))
            .map(Some)
            .ok_or_else(|| PlenoraError::Schema("timestamp non rappresentabile come f64".into()));
    }
    if let Some(values) = array.as_any().downcast_ref::<Decimal128Array>() {
        let DataType::Decimal128(_, scale) = values.data_type() else {
            return Err(PlenoraError::Schema("decimal128 incoerente".into()));
        };
        // La verifica riguarda il valore DOPO la divisione: `unscaled`
        // rappresentabile non implica che lo sia `unscaled / 10^scale`.
        return exact_f64_from_decimal128(values.value(row), *scale)
            .map(Some)
            .ok_or_else(|| PlenoraError::Schema("decimal128 non rappresentabile come f64".into()));
    }
    if let Some(values) = array.as_any().downcast_ref::<StringArray>() {
        return values
            .value(row)
            .trim()
            .replace(',', ".")
            .parse::<f64>()
            .map(Some)
            .map_err(|_| PlenoraError::Schema("valore non convertibile in numero".into()));
    }
    Err(PlenoraError::Schema(format!(
        "tipo {:?} non convertibile in numero",
        array.data_type()
    )))
}

/// Valore scalare della riga come `f64`, con **arrotondamento dichiarato**.
///
/// Per i kernel il cui risultato e' un `Float64` per contratto (medie,
/// statistiche, formule): li' l'esattezza rifiuterebbe input legittimi come
/// un decimale `0.1`. Dove il valore serve a decidere (confronti, chiavi,
/// vincoli) valgono [`scalar_as_f64`] o [`scalar_compare`].
///
/// Deroga registrata in errori-e-limiti.md#limiti-dichiarati.
///
/// # Errors
///
/// - `Schema`: testo non convertibile in numero, tipo non convertibile in
///   numero, decimal128 incoerente con il proprio schema.
#[allow(clippy::cast_precision_loss)] // Arrotondamento voluto: vedi doc.
pub fn scalar_as_f64_rounded(array: &dyn Array, row: usize) -> Result<Option<f64>> {
    if array.is_null(row) {
        return Ok(None);
    }
    if let Some(values) = array.as_any().downcast_ref::<Int64Array>() {
        return Ok(Some(values.value(row) as f64));
    }
    if let Some(values) = array.as_any().downcast_ref::<UInt64Array>() {
        return Ok(Some(values.value(row) as f64));
    }
    if let Some(values) = array.as_any().downcast_ref::<TimestampMillisecondArray>() {
        return Ok(Some(values.value(row) as f64));
    }
    if let Some(values) = array.as_any().downcast_ref::<Decimal128Array>() {
        let DataType::Decimal128(_, scale) = values.data_type() else {
            return Err(PlenoraError::Schema("decimal128 incoerente".into()));
        };
        let factor = 10_f64.powi(i32::from(*scale));
        return Ok(Some((values.value(row) as f64) / factor));
    }
    // Gli altri tipi non perdono nulla nella conversione: si riusa il
    // percorso esatto, che per loro non fallisce mai.
    scalar_as_f64(array, row)
}

// ---------------------------------------------------------------------------
// Confronti scalari tipizzati (filtri, regole di governance, assert_range).
//
// Esatti per costruzione: nessuna conversione a f64 quando un lato e' un
// intero, perche' oltre 2^53 interi distinti collassano sullo stesso double.
//
// Il valore di configurazione e' un letterale JSON reso testo: un intero
// resta intero esatto, un decimale posizionale resta decimale esatto, ogni
// altra forma e' `F64`. Contro `F64`: un double frazionario non eguaglia mai
// un intero e ordina per floor, uno intero fuori gamma ordina per segno, NaN
// rende falso ogni confronto (`None`), come in IEEE 754.
// ---------------------------------------------------------------------------

/// Estremo di un confronto scalare, parsato dal valore di configurazione.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum NumericBound {
    /// Letterale intero in gamma i64: confronto nativo esatto.
    I64(i64),
    /// Letterale intero oltre `i64::MAX` (gamma u64): confronto nativo esatto.
    U64(u64),
    /// Letterale DECIMALE esatto (`10.5`, `-0.001`): conservato come intero
    /// non scalato piu' scala, quindi confrontabile esattamente con una
    /// colonna Decimal128 senza passare da `f64`.
    Decimal {
        /// Valore intero non scalato.
        unscaled: i128,
        /// Cifre decimali: il valore e' `unscaled * 10^(-scale)`.
        scale: i8,
    },
    /// Qualunque altra forma numerica (esponenziale, inf, NaN, o un decimale
    /// con piu' cifre di quante ne tenga un `i128`).
    F64(f64),
}

/// Massimo `a` per cui `5^a <= 2^53`, cioe' per cui un valore con scala
/// `-a` puo' avere un `f64` esatto: `5^22 < 2^52 < 5^23`.
const MAX_EXACT_POW5: u32 = 22;

/// Cifre decimali massime conservate in forma esatta.
///
/// Coincide con la precisione di `Decimal128` di Arrow: oltre, il letterale
/// non e' comunque confrontabile esattamente con una colonna decimale.
const MAX_DECIMAL_DIGITS: usize = 38;

impl NumericBound {
    /// Parse del valore atteso: intero, poi decimale esatto, poi `f64`.
    ///
    /// `None` se il testo non e' numerico. La forma decimale rende esatti i
    /// confronti con le colonne `Decimal128`, dove un double farebbe
    /// collassare decimali distinti.
    pub fn parse(text: &str) -> Option<Self> {
        if let Ok(value) = text.parse::<i64>() {
            return Some(Self::I64(value));
        }
        if let Ok(value) = text.parse::<u64>() {
            return Some(Self::U64(value));
        }
        // Interi oltre `u64` (o negativi oltre `i64`): restano esatti come
        // decimali a scala zero. Ricadendo su `f64`,
        // `18446744073709551617` — perfettamente rappresentabile in `i128` —
        // verrebbe arrotondato.
        if let Ok(value) = text.parse::<i128>() {
            return Some(Self::Decimal {
                unscaled: value,
                scale: 0,
            });
        }
        if let Some(decimal) = Self::parse_decimal(text) {
            return Some(decimal);
        }
        text.parse::<f64>().ok().map(Self::F64)
    }

    /// Letterale decimale in notazione posizionale (niente esponente): segno
    /// opzionale, cifre, al piu' un punto. `None` per ogni altra forma.
    fn parse_decimal(text: &str) -> Option<Self> {
        let negative = text.starts_with('-');
        let digits = text
            .strip_prefix('-')
            .or_else(|| text.strip_prefix('+'))
            .unwrap_or(text);
        let (intero, frazione) = digits.split_once('.')?;
        // Almeno una cifra in tutto, e solo cifre: `.5` e `5.` sono ammessi,
        // `1.2.3`, `1e5` e `abc` no.
        if frazione.contains('.')
            || (intero.is_empty() && frazione.is_empty())
            || !intero.bytes().all(|byte| byte.is_ascii_digit())
            || !frazione.bytes().all(|byte| byte.is_ascii_digit())
        {
            return None;
        }
        // Zeri non significativi via prima di contare le cifre, altrimenti
        // `0000…0.1` sforerebbe il tetto e ricadrebbe su `f64`. Gli zeri
        // iniziali della parte frazionaria restano: sono la scala.
        let intero = intero.trim_start_matches('0');
        let frazione = frazione.trim_end_matches('0');
        if intero.len() + frazione.len() > MAX_DECIMAL_DIGITS {
            return None;
        }
        let scale = i8::try_from(frazione.len()).ok()?;
        let testo = format!("{intero}{frazione}");
        // Tutto zero (`0.0`, `-0.000`, `.0`): la stringa concatenata e'
        // vuota e `parse` fallirebbe. Il valore e' lo zero, a scala zero.
        if testo.is_empty() {
            return Some(Self::Decimal {
                unscaled: 0,
                scale: 0,
            });
        }
        let mut unscaled: i128 = testo.parse().ok()?;
        if negative {
            unscaled = -unscaled;
        }
        Some(Self::Decimal { unscaled, scale })
    }
}

/// Confronto esatto i64 <-> bound. `None` solo con bound NaN (ogni confronto
/// falso, come IEEE): i chiamanti lo trattano come "confronto non soddisfatto".
#[allow(clippy::cast_possible_wrap, clippy::cast_sign_loss)]
// I cast intero<->intero sono guardati dai rami precedenti (gamma verificata).
#[must_use]
pub fn compare_i64(actual: i64, bound: NumericBound) -> Option<Ordering> {
    match bound {
        NumericBound::I64(expected) => Some(actual.cmp(&expected)),
        NumericBound::U64(expected) => Some(if expected > i64::MAX as u64 {
            Ordering::Less // ogni i64 e' minore di un u64 oltre i64::MAX
        } else {
            actual.cmp(&(expected as i64))
        }),
        // Un intero contro un decimale: si confronta nel dominio scalato,
        // esatto in `i128`.
        NumericBound::Decimal { unscaled, scale } => Some(compare_decimal128_values(
            i128::from(actual),
            0,
            unscaled,
            scale,
        )),
        NumericBound::F64(expected) => compare_i64_f64(actual, expected),
    }
}

/// Confronto esatto u64 <-> bound. `None` solo con bound NaN.
#[allow(clippy::cast_sign_loss)] // cast i64->u64 guardato dal ramo `expected < 0`
#[must_use]
pub fn compare_u64(actual: u64, bound: NumericBound) -> Option<Ordering> {
    match bound {
        NumericBound::U64(expected) => Some(actual.cmp(&expected)),
        NumericBound::I64(expected) => Some(if expected < 0 {
            Ordering::Greater // ogni u64 e' maggiore di un intero negativo
        } else {
            actual.cmp(&(expected as u64))
        }),
        NumericBound::Decimal { unscaled, scale } => Some(compare_decimal128_values(
            i128::from(actual),
            0,
            unscaled,
            scale,
        )),
        NumericBound::F64(expected) => compare_u64_f64(actual, expected),
    }
}

/// Confronto esatto f64 <-> bound, duale di `compare_i64`/`compare_u64`.
///
/// Contro letterali interi coincide con IEEE entro 2^53 e resta esatto
/// oltre; con bound `F64` vale `partial_cmp` (NaN -> `None`).
///
/// Con bound `Decimal` il confronto e' razionale esatto: la soglia scritta
/// non viene arrotondata (`1e-1 > 0.100000000000000001` e' falso, non
/// uguale).
pub fn compare_f64(actual: f64, bound: NumericBound) -> Option<Ordering> {
    match bound {
        NumericBound::I64(expected) => compare_i64_f64(expected, actual).map(Ordering::reverse),
        NumericBound::U64(expected) => compare_u64_f64(expected, actual).map(Ordering::reverse),
        // `compare_decimal_with_f64` ordina il decimale rispetto al double:
        // qui l'attore e' il double, quindi il verso si rovescia.
        NumericBound::Decimal { unscaled, scale } => {
            exact_compare::compare_decimal_with_f64(unscaled, scale, actual).map(Ordering::reverse)
        }
        NumericBound::F64(expected) => actual.partial_cmp(&expected),
    }
}

#[allow(clippy::float_cmp, clippy::cast_possible_truncation)]
// I confronti con inf e i cast f64->i64 sono esatti per costruzione: i rami
// sopra garantiscono finitezza, gamma e (dove richiesto) integrita'.
fn compare_i64_f64(actual: i64, expected: f64) -> Option<Ordering> {
    if expected.is_nan() {
        return None;
    }
    if expected == f64::INFINITY {
        return Some(Ordering::Less);
    }
    if expected == f64::NEG_INFINITY {
        return Some(Ordering::Greater);
    }
    // Oltre 2^63 (in valore assoluto) il double e' certamente intero (non
    // esistono doppi frazionari oltre 2^52) e fuori gamma i64: ordina per segno.
    if expected >= 9_223_372_036_854_775_808.0 {
        return Some(Ordering::Less);
    }
    if expected < -9_223_372_036_854_775_808.0 {
        return Some(Ordering::Greater);
    }
    if expected.fract() == 0.0 {
        // Double intero in gamma i64 (2^63 negativo incluso): cast esatto.
        return Some(actual.cmp(&(expected as i64)));
    }
    // Double frazionario (qui |expected| < 2^52, floor esatto in i64): mai
    // uguale a un intero, ordina per floor.
    let floor = expected.floor() as i64;
    Some(if actual <= floor {
        Ordering::Less
    } else {
        Ordering::Greater
    })
}

// Come `compare_i64_f64`: guardie di finitezza, segno, gamma e integrita'.
// I cast f64 -> u64 nel corpo sono esatti per costruzione: NaN e infiniti
// sono esclusi dalle guardie iniziali, il segno negativo dalla guardia
// `expected < 0.0`, l'overflow dalla guardia `expected >= 2^64`.
#[allow(
    clippy::float_cmp,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
fn compare_u64_f64(actual: u64, expected: f64) -> Option<Ordering> {
    if expected.is_nan() {
        return None;
    }
    if expected == f64::INFINITY {
        return Some(Ordering::Less);
    }
    if expected == f64::NEG_INFINITY || expected < 0.0 {
        return Some(Ordering::Greater);
    }
    if expected >= 18_446_744_073_709_551_616.0 {
        return Some(Ordering::Less);
    }
    if expected.fract() == 0.0 {
        // Double intero in [0, 2^64): cast esatto.
        return Some(actual.cmp(&(expected as u64)));
    }
    let floor = expected.floor() as u64;
    Some(if actual <= floor {
        Ordering::Less
    } else {
        Ordering::Greater
    })
}

/// `2^127`: primo double oltre la gamma `i128`.
const I128_UPPER_BOUND_F64: f64 = 170_141_183_460_469_231_731_687_303_715_884_105_728.0;

/// Confronto esatto `i128` <-> bound (base dei confronti Decimal128).
#[must_use]
pub fn compare_i128(actual: i128, bound: NumericBound) -> Option<Ordering> {
    match bound {
        NumericBound::I64(expected) => Some(actual.cmp(&i128::from(expected))),
        NumericBound::U64(expected) => Some(actual.cmp(&i128::from(expected))),
        NumericBound::Decimal { unscaled, scale } => {
            Some(compare_decimal128_values(actual, 0, unscaled, scale))
        }
        NumericBound::F64(expected) => compare_i128_f64(actual, expected),
    }
}

// Come `compare_i64_f64`: guardie di NaN, infiniti, gamma e integrita' prima
// di ogni cast, che risulta quindi esatto per costruzione.
#[allow(clippy::float_cmp, clippy::cast_possible_truncation)]
fn compare_i128_f64(actual: i128, expected: f64) -> Option<Ordering> {
    if expected.is_nan() {
        return None;
    }
    if expected == f64::INFINITY || expected >= I128_UPPER_BOUND_F64 {
        return Some(Ordering::Less);
    }
    if expected == f64::NEG_INFINITY || expected < -I128_UPPER_BOUND_F64 {
        return Some(Ordering::Greater);
    }
    if expected.fract() == 0.0 {
        return Some(actual.cmp(&(expected as i128)));
    }
    // Double frazionario: mai uguale a un intero, ordina per floor.
    let floor = expected.floor() as i128;
    Some(if actual <= floor {
        Ordering::Less
    } else {
        Ordering::Greater
    })
}

/// Confronto esatto Decimal128 <-> bound.
///
/// Il valore logico e' `unscaled * 10^(-scale)`. Contro un letterale intero o
/// decimale il confronto e' interamente in `i128`. Il bound `F64` (forme non
/// posizionali: esponenziale, infiniti, NaN, oltre 38 cifre) e' un double
/// per natura, e si confronta come tale.
#[must_use]
pub fn compare_decimal128(unscaled: i128, scale: i8, bound: NumericBound) -> Option<Ordering> {
    let (expected, expected_scale) = match bound {
        NumericBound::I64(value) => (i128::from(value), 0_i8),
        NumericBound::U64(value) => (i128::from(value), 0_i8),
        NumericBound::Decimal {
            unscaled: value,
            scale: value_scale,
        } => (value, value_scale),
        NumericBound::F64(value) => return compare_decimal128_f64(unscaled, scale, value),
    };
    Some(compare_decimal128_values(
        unscaled,
        scale,
        expected,
        expected_scale,
    ))
}

/// Confronto di `unscaled * 10^(-scale)` con l'intero `expected`, interamente
/// in `i128`: il fattore di scala si applica al lato che non trabocca.
fn compare_scaled_i128(unscaled: i128, scale: i8, expected: i128) -> Ordering {
    if unscaled == 0 || expected == 0 {
        // Con un lato a zero il confronto e' il segno dell'altro, qualunque
        // sia la scala (e nessun fattore va calcolato).
        return if unscaled == 0 {
            0_i128.cmp(&expected)
        } else {
            unscaled.cmp(&0)
        };
    }
    let factor = i128::checked_pow(10, u32::from(scale.unsigned_abs()));
    if scale >= 0 {
        // Il bound si porta nel dominio dell'unscaled. Se il prodotto esce da
        // `i128` supera in valore assoluto qualunque decimal rappresentabile,
        // e decide il suo segno.
        factor
            .and_then(|factor| expected.checked_mul(factor))
            .map_or_else(
                || {
                    if expected > 0 {
                        Ordering::Less
                    } else {
                        Ordering::Greater
                    }
                },
                |scaled| unscaled.cmp(&scaled),
            )
    } else {
        // Scala negativa: il valore logico e' `unscaled * 10^(-scale)`, quindi
        // e' l'unscaled a essere portato nel dominio del bound.
        factor
            .and_then(|factor| unscaled.checked_mul(factor))
            .map_or_else(
                || {
                    if unscaled > 0 {
                        Ordering::Greater
                    } else {
                        Ordering::Less
                    }
                },
                |value| value.cmp(&expected),
            )
    }
}

#[allow(
    clippy::float_cmp,
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss
)]
// Le guardie sopra ogni cast ne garantiscono gamma e integrita'; la
// conversione del decimal a double avviene solo nel ramo frazionario, dove
// l'approssimazione e' gia' nel contratto del letterale.
fn compare_decimal128_f64(unscaled: i128, scale: i8, expected: f64) -> Option<Ordering> {
    if expected.is_nan() {
        return None;
    }
    if expected == f64::INFINITY {
        return Some(Ordering::Less);
    }
    if expected == f64::NEG_INFINITY {
        return Some(Ordering::Greater);
    }
    // Un double a valore intero in gamma i128 si confronta esattamente nel
    // dominio scalato: nessuna divisione, nessun arrotondamento.
    if expected.fract() == 0.0 && expected.abs() < I128_UPPER_BOUND_F64 {
        return Some(compare_scaled_i128(unscaled, scale, expected as i128));
    }
    // Double frazionario: confronto RAZIONALE esatto, senza convertire il
    // decimal. La conversione collasserebbe valori distinti — `1e-1` e
    // `0.100000000000000001` risulterebbero uguali — perche' porta il decimal
    // sul reticolo dei double invece di confrontare i due razionali.
    exact_compare::compare_decimal_with_f64(unscaled, scale, expected)
}

/// Confronto esatto fra due valori Decimal128, anche con scale diverse.
///
/// Nessun passaggio per `f64` e nessuna forma testuale: il confronto sulla
/// rappresentazione decimale stampata ordina "10" prima di "9" e sbaglia
/// sistematicamente sui negativi.
#[must_use]
pub fn compare_decimal128_values(
    left: i128,
    left_scale: i8,
    right: i128,
    right_scale: i8,
) -> Ordering {
    if left_scale == right_scale {
        return left.cmp(&right);
    }
    // Uno zero va deciso prima: il ramo di traboccamento del fattore lo
    // scambierebbe per un valore enorme.
    if left == 0 || right == 0 {
        return left.signum().cmp(&right.signum());
    }
    // Il lato con scala minore va portato alla scala maggiore: scalare in su
    // e' esatto, scalare in giu' perderebbe cifre.
    let left_is_lower = left_scale < right_scale;
    let (lower, target, lower_scale) = if left_is_lower {
        (left, right_scale, left_scale)
    } else {
        (right, left_scale, right_scale)
    };
    let delta = u32::try_from(i16::from(target) - i16::from(lower_scale)).unwrap_or(u32::MAX);
    i128::checked_pow(10, delta)
        .and_then(|factor| lower.checked_mul(factor))
        .map_or_else(
            // Il riscalaggio esce da `i128`: quel lato supera in modulo
            // l'altro, che invece ci sta. Decide il suo segno (`lower` non e'
            // zero, perche' zero non trabocca mai).
            || {
                let verso = if lower > 0 {
                    Ordering::Greater
                } else {
                    Ordering::Less
                };
                if left_is_lower {
                    verso
                } else {
                    verso.reverse()
                }
            },
            |scaled| {
                if left_is_lower {
                    scaled.cmp(&right)
                } else {
                    left.cmp(&scaled)
                }
            },
        )
}

/// Confronto fra due estremi gia' parsati (colonne testuali numeriche).
#[must_use]
pub fn compare_bounds(actual: NumericBound, expected: NumericBound) -> Option<Ordering> {
    match actual {
        NumericBound::I64(value) => compare_i64(value, expected),
        NumericBound::U64(value) => compare_u64(value, expected),
        // Due decimali si confrontano esattamente, anche con scale diverse:
        // "10.50" e "10.5" sono lo stesso valore, "0.1" e "0.10000000001" no.
        NumericBound::Decimal { unscaled, scale } => compare_decimal128(unscaled, scale, expected),
        NumericBound::F64(value) => compare_f64(value, expected),
    }
}

/// Confronto esatto fra il valore scalare della riga e un estremo di
/// configurazione: comparatore condiviso di filtri, regole di governance e
/// vincoli di qualita'.
///
/// Ogni tipo si confronta nel proprio dominio nativo, mai attraverso `f64`.
/// Il chiamante ha gia' escluso le righe null; `None` significa estremo NaN
/// (semantica IEEE: ogni operatore ordinato e' falso).
///
/// # Errors
///
/// `Schema` se il tipo non e' confrontabile numericamente o se il testo di
/// una colonna Utf8 non e' un numero.
pub fn scalar_compare(
    array: &dyn Array,
    row: usize,
    bound: NumericBound,
) -> Result<Option<Ordering>> {
    let any = array.as_any();
    if let Some(values) = any.downcast_ref::<Int64Array>() {
        return Ok(compare_i64(values.value(row), bound));
    }
    if let Some(values) = any.downcast_ref::<UInt64Array>() {
        return Ok(compare_u64(values.value(row), bound));
    }
    if let Some(values) = any.downcast_ref::<Float64Array>() {
        return Ok(compare_f64(values.value(row), bound));
    }
    if let Some(values) = any.downcast_ref::<Date32Array>() {
        // Stesso dominio numerico di `scalar_as_f64`: giorni dall'epoch.
        return Ok(compare_i64(i64::from(values.value(row)), bound));
    }
    if let Some(values) = any.downcast_ref::<TimestampMillisecondArray>() {
        // Stesso dominio numerico di `scalar_as_f64`: millisecondi dall'epoch.
        return Ok(compare_i64(values.value(row), bound));
    }
    if let Some(values) = any.downcast_ref::<Decimal128Array>() {
        let DataType::Decimal128(_, scale) = values.data_type() else {
            return Err(PlenoraError::Schema("decimal128 incoerente".into()));
        };
        return Ok(compare_decimal128(values.value(row), *scale, bound));
    }
    if let Some(values) = any.downcast_ref::<StringArray>() {
        // Stessa normalizzazione di `scalar_as_f64` (trim, virgola decimale),
        // ma il letterale resta intero quando lo e': nessun arrotondamento.
        let text = values.value(row).trim().replace(',', ".");
        let actual = NumericBound::parse(&text)
            .ok_or_else(|| PlenoraError::Schema("valore non convertibile in numero".into()))?;
        return Ok(compare_bounds(actual, bound));
    }
    Err(PlenoraError::Schema(format!(
        "tipo {:?} non confrontabile numericamente",
        array.data_type()
    )))
}

/// Batch con le sole righe indicate, nell'ordine dato.
///
/// # Errors
///
/// - `ResourceLimit`: indice di riga oltre `u32::MAX` (cresce col numero di
///   righe: e' un volume, non un piano sbagliato);
/// - `Schema`: errore Arrow nella `take` o nella costruzione del batch.
pub fn select_rows(batch: &RecordBatch, rows: &[usize]) -> Result<RecordBatch> {
    let indices: UInt32Array = rows
        .iter()
        .map(|row| {
            u32::try_from(*row)
                .map_err(|_| PlenoraError::ResourceLimit("indice riga oltre u32".into()))
        })
        .collect::<Result<Vec<_>>>()?
        .into();
    let columns = batch
        .columns()
        .iter()
        .map(|column| {
            plenora_core::arrow::select::take::take(column.as_ref(), &indices, None)
                .map_err(PlenoraError::from)
        })
        .collect::<Result<Vec<_>>>()?;
    // Righe DICHIARATE: `select_rows` e' attraversata da molti kernel, e su
    // un batch a zero colonne il numero di righe selezionate non e' deducibile
    // da alcuna colonna.
    batch_with_rows(batch.schema(), columns, rows.len())
}

#[cfg(test)]
mod tests {
    use plenora_core::arrow::array::{Int64Array, StringArray};
    use plenora_core::arrow::schema::{DataType, Field, Schema};

    use super::*;

    #[test]
    fn il_default_governato_e_lo_stesso_di_plenora_core() {
        // Due default divergenti darebbero budget diversi allo stesso piano:
        // si verificano entrambi i lati contro l'autorita' di `plenora-core`.
        assert_eq!(
            Limits::default().max_governed_memory_bytes as u64,
            plenora_core::DEFAULT_MAX_GOVERNED_MEMORY_BYTES,
            "il default dei kernel deve venire dall'autorita' di plenora-core"
        );
        assert_eq!(
            Limits::default().max_governed_memory_bytes as u64,
            plenora_core::limits::Limits::default().max_governed_memory_bytes,
            "e coincidere con quello del contenitore dei limiti del piano"
        );
    }

    #[test]
    fn anche_gli_altri_due_default_condivisi_vengono_dall_autorita() {
        // Anche `max_temp_bytes` e `spill_partitions` vengono dalle costanti
        // di `plenora-core`: finiscono negli override del piano, e una
        // divergenza darebbe limiti che nessuno ha dichiarato.
        let nostri = Limits::default();
        let del_piano = plenora_core::limits::Limits::default();
        assert_eq!(nostri.max_temp_bytes, del_piano.max_temp_bytes);
        assert_eq!(
            nostri.max_temp_bytes,
            plenora_core::limits::DEFAULT_MAX_TEMP_BYTES
        );
        // Il confronto si fa nel tipo LARGO: restringere `usize` a `u32` per
        // confrontarli introdurrebbe qui la stessa troncatura che il codice
        // di produzione evita.
        let nostre_partizioni = u64::try_from(nostri.spill_partitions).expect("partizioni");
        assert_eq!(
            nostre_partizioni,
            u64::from(del_piano.spill_partitions),
            "i due crate li tengono in tipi diversi, non in valori diversi"
        );
        assert_eq!(
            nostre_partizioni,
            u64::from(plenora_core::limits::DEFAULT_SPILL_PARTITIONS)
        );
    }

    fn batch() -> RecordBatch {
        RecordBatch::try_new(
            Arc::new(Schema::new(vec![Field::new("a", DataType::Utf8, true)])),
            vec![Arc::new(StringArray::from(vec![Some("x"), None]))],
        )
        .expect("fixture")
    }

    #[test]
    fn helper_guards_cover_type_length_and_names() {
        let input = batch();
        assert!(replace_or_append(
            &input,
            "bad",
            DataType::Utf8,
            true,
            Arc::new(StringArray::from(vec![Some("only one")]))
        )
        .is_err());
        assert!(validate_output_name(" ").is_err());
        assert!(validate_output_name(&"x".repeat(1_025)).is_err());
        assert!(column_index(&input, "missing").is_err());

        let integers = RecordBatch::try_new(
            Arc::new(Schema::new(vec![Field::new("n", DataType::Int64, false)])),
            vec![Arc::new(Int64Array::from(vec![1]))],
        )
        .expect("integers");
        assert!(utf8_column(&integers, "n").is_err());
    }

    #[test]
    fn numeric_bound_parse_prefers_exact_integers() {
        assert_eq!(NumericBound::parse("42"), Some(NumericBound::I64(42)));
        assert_eq!(NumericBound::parse("-7"), Some(NumericBound::I64(-7)));
        assert_eq!(
            NumericBound::parse("9007199254740993"),
            Some(NumericBound::I64(9_007_199_254_740_993))
        );
        assert_eq!(
            NumericBound::parse("18446744073709551615"),
            Some(NumericBound::U64(u64::MAX))
        );
        assert_eq!(
            NumericBound::parse("9223372036854775808"),
            Some(NumericBound::U64(9_223_372_036_854_775_808))
        );
        // I letterali decimali posizionali restano DECIMALI esatti: e' cio'
        // che rende esatto il confronto con una colonna Decimal128.
        assert_eq!(
            NumericBound::parse("1.5"),
            Some(NumericBound::Decimal {
                unscaled: 15,
                scale: 1
            })
        );
        // Gli zeri finali della frazione non sono cifre: la forma e'
        // normalizzata (stesso VALORE, scala minima), cosi' il tetto di 38
        // cifre misura le cifre significative e non la scrittura.
        assert_eq!(
            NumericBound::parse("64.0"),
            Some(NumericBound::Decimal {
                unscaled: 64,
                scale: 0
            })
        );
        assert_eq!(
            NumericBound::parse("-0.001"),
            Some(NumericBound::Decimal {
                unscaled: -1,
                scale: 3
            })
        );
        // Le forme che un decimale non e' restano double.
        assert_eq!(NumericBound::parse("1e3"), Some(NumericBound::F64(1_000.0)));
        assert!(matches!(
            NumericBound::parse("1.5e2"),
            Some(NumericBound::F64(_))
        ));
        // Oltre 38 cifre non c'e' forma decimale esatta: si ricade su f64.
        assert!(matches!(
            NumericBound::parse("1.000000000000000000000000000000000000000001"),
            Some(NumericBound::F64(_))
        ));
        assert!(NumericBound::parse("x").is_none());
        assert!(NumericBound::parse("").is_none());
        assert!(NumericBound::parse("1.2.3").is_none());
    }

    #[test]
    fn i_decimal_frazionari_si_confrontano_esattamente() {
        // `0.1` non ha alcun double esatto: passando per f64, decimal
        // distinti collassano sullo stesso valore. Con la forma decimale il
        // confronto e' un confronto di interi scalati.
        let bound = NumericBound::parse("0.1").expect("decimale");
        // 0.10 (scala 2) e' uguale a 0.1 (scala 1).
        assert_eq!(compare_decimal128(10, 2, bound), Some(Ordering::Equal));
        // 0.11 e' maggiore, 0.09 minore: entrambi indistinguibili da 0.1 se
        // il confronto passasse per la conversione a double di un decimal a
        // scala alta.
        assert_eq!(compare_decimal128(11, 2, bound), Some(Ordering::Greater));
        assert_eq!(compare_decimal128(9, 2, bound), Some(Ordering::Less));
        // Un decimal a 30 cifre appena sopra 0.1 resta sopra.
        let appena_sopra = 100_000_000_000_000_000_000_000_000_001_i128;
        assert_eq!(
            compare_decimal128(appena_sopra, 30, bound),
            Some(Ordering::Greater)
        );
    }

    #[test]
    fn la_conversione_decimal_f64_e_esatta_o_errore() {
        // `1` con scala `1` vale 0.1: nessun double lo rappresenta, e
        // verificare il solo `unscaled` lo lascerebbe passare.
        assert_eq!(exact_f64_from_decimal128(1, 1), None);
        // `5` con scala `1` vale 0.5: potenza di due, esatto.
        assert_eq!(exact_f64_from_decimal128(5, 1), Some(0.5));
        assert_eq!(exact_f64_from_decimal128(25, 2), Some(0.25));
        assert_eq!(exact_f64_from_decimal128(-125, 3), Some(-0.125));
        // Scala zero: vale la regola degli interi.
        assert_eq!(exact_f64_from_decimal128(3, 0), Some(3.0));
        assert_eq!(exact_f64_from_decimal128(i128::MAX, 0), None);
    }

    #[test]
    fn compare_i64_is_exact_beyond_2_pow_53() {
        let lo = 9_007_199_254_740_992_i64; // 2^53
        let hi = 9_007_199_254_740_993_i64; // 2^53 + 1: stesso double di lo
        assert_eq!(
            compare_i64(hi, NumericBound::I64(lo)),
            Some(Ordering::Greater)
        );
        assert_eq!(compare_i64(lo, NumericBound::I64(hi)), Some(Ordering::Less));
        assert_eq!(
            compare_i64(hi, NumericBound::I64(hi)),
            Some(Ordering::Equal)
        );
        // Bound f64: lo e hi collassano sullo stesso double, il confronto
        // resta esatto.
        let collapsed = NumericBound::F64(9_007_199_254_740_992.0);
        assert_eq!(compare_i64(lo, collapsed), Some(Ordering::Equal));
        assert_eq!(compare_i64(hi, collapsed), Some(Ordering::Greater));
        assert_eq!(compare_i64(-hi, collapsed), Some(Ordering::Less));
    }

    #[test]
    fn compare_i64_mixed_covers_fraction_inf_nan_and_ranges() {
        assert_eq!(compare_i64(5, NumericBound::F64(5.5)), Some(Ordering::Less));
        assert_eq!(
            compare_i64(5, NumericBound::F64(4.5)),
            Some(Ordering::Greater)
        );
        assert_eq!(
            compare_i64(-5, NumericBound::F64(-5.5)),
            Some(Ordering::Greater)
        );
        assert_eq!(
            compare_i64(-6, NumericBound::F64(-5.5)),
            Some(Ordering::Less)
        );
        assert_eq!(
            compare_i64(0, NumericBound::F64(-0.0)),
            Some(Ordering::Equal)
        );
        assert_eq!(
            compare_i64(i64::MAX, NumericBound::F64(f64::INFINITY)),
            Some(Ordering::Less)
        );
        assert_eq!(
            compare_i64(i64::MIN, NumericBound::F64(f64::NEG_INFINITY)),
            Some(Ordering::Greater)
        );
        assert_eq!(compare_i64(0, NumericBound::F64(f64::NAN)), None);
        assert_eq!(
            compare_i64(i64::MAX, NumericBound::F64(1e30)),
            Some(Ordering::Less)
        );
        assert_eq!(
            compare_i64(i64::MIN, NumericBound::F64(-1e30)),
            Some(Ordering::Greater)
        );
        // -2^63 e' un double intero esatto in gamma i64.
        assert_eq!(
            compare_i64(i64::MIN, NumericBound::F64(-9_223_372_036_854_775_808.0)),
            Some(Ordering::Equal)
        );
        // Bound u64 oltre i64::MAX: maggiore di ogni i64.
        assert_eq!(
            compare_i64(i64::MAX, NumericBound::U64(9_223_372_036_854_775_808)),
            Some(Ordering::Less)
        );
        assert_eq!(
            compare_i64(-1, NumericBound::U64(u64::MAX)),
            Some(Ordering::Less)
        );
    }

    #[test]
    fn compare_u64_orders_natively_not_textually() {
        assert_eq!(
            compare_u64(10, NumericBound::U64(9)),
            Some(Ordering::Greater)
        );
        assert_eq!(compare_u64(9, NumericBound::U64(10)), Some(Ordering::Less));
        assert_eq!(
            compare_u64(u64::MAX, NumericBound::U64(u64::MAX)),
            Some(Ordering::Equal)
        );
        assert_eq!(
            compare_u64(0, NumericBound::I64(-1)),
            Some(Ordering::Greater)
        );
        assert_eq!(
            compare_u64(10, NumericBound::I64(9)),
            Some(Ordering::Greater)
        );
        // Bound f64 oltre 2^53: 2^64 e' maggiore di ogni u64.
        let top = NumericBound::F64(18_446_744_073_709_551_616.0); // 2^64
        assert_eq!(compare_u64(u64::MAX, top), Some(Ordering::Less));
        assert_eq!(compare_u64(0, NumericBound::F64(0.5)), Some(Ordering::Less));
        assert_eq!(
            compare_u64(1, NumericBound::F64(0.5)),
            Some(Ordering::Greater)
        );
        assert_eq!(compare_u64(0, NumericBound::F64(f64::NAN)), None);
    }

    #[test]
    fn compare_f64_is_the_exact_dual_for_float_columns() {
        // Letterale intero oltre 2^53 contro colonna Float64: il double
        // 9007199254740992.0 e' minore dell'intero 9007199254740993.
        assert_eq!(
            compare_f64(
                9_007_199_254_740_992.0,
                NumericBound::I64(9_007_199_254_740_993)
            ),
            Some(Ordering::Less)
        );
        assert_eq!(
            compare_f64(
                9_007_199_254_740_992.0,
                NumericBound::I64(9_007_199_254_740_992)
            ),
            Some(Ordering::Equal)
        );
        assert_eq!(compare_f64(f64::NAN, NumericBound::I64(1)), None);
        assert_eq!(
            compare_f64(1.5, NumericBound::F64(1.5)),
            Some(Ordering::Equal)
        );
    }
}
