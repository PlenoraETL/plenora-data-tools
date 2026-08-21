//! plenora-kernels-table — kernel tabellari puri `&RecordBatch -> Result<RecordBatch>`
//! (architettura.md).
//!
//! Fase 1 "coesistenza": trasloco meccanico dei 17 moduli kernel da
//! `plenora-nogeo-tools/src/kernels/` (`columns`, `strings`, `cleansing`,
//! `filtering`, `dates`, `utility`, `analysis`, `aggregation`, `reshape`,
//! `joins`, `setops`, `security`, `quality`, `governance`, `formula`,
//! `expressions`, `spill`) con gli helper condivisi, senza modifiche di
//! comportamento.

use serde::{Deserialize, Serialize};

/// Limiti dei kernel tabellari, traslocati identici da
/// `plenora-nogeo-tools/src/contract.rs` (Fase 1, zero modifiche di
/// comportamento).
///
/// NOTA (punto aperto per la fase engine): il `Limits` unificato di
/// `plenora_core::limits` (decisione D19, errori-e-limiti.md) non copre `max_columns` e
/// `max_split_columns`, e sostituisce il singolo `max_rows` con la famiglia
/// semantica `RowLimits` (`max_input_rows` / `max_output_rows` /
/// `max_rows_per_edge`). La mappatura di questa struct su
/// `plenora_core::limits::Limits` e' una decisione semantica demandata alla
/// fase engine, non un adattamento meccanico.
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

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_rows: 10_000_000,
            max_columns: 4_096,
            max_string_bytes: 16 * 1024 * 1024,
            max_regex_bytes: 4_096,
            max_split_columns: 256,
            max_governed_memory_bytes: 512 * 1024 * 1024,
            max_temp_bytes: 8 * 1024 * 1024 * 1024,
            spill_partitions: 64,
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
/// propagano invariati, come prima.
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
/// Una colonna vuota non si puo' misurare, ma nell'output ne occupera'
/// comunque: il pavimento e' la larghezza del tipo (o otto byte per i tipi a
/// lunghezza variabile) piu' un byte di validita'. Senza il pavimento un
/// input vuoto che porta molte colonne nello schema di uscita pesava zero
/// nella stima — che e' esattamente il caso in cui la stima serve.
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
    // `primitive_width` copre i tipi a larghezza fissa; per i tipi a
    // lunghezza variabile (Utf8, Binary, List...) non esiste una larghezza,
    // e otto byte sono il minimo fra offset e puntatore. Piu' un byte di
    // validita', che c'e' sempre.
    // `saturating_add` su una larghezza di tipo (al massimo poche decine di
    // byte) piu' uno: non puo' saturare, e resta saturante solo per non
    // introdurre un `Result` dove non c'e' un errore possibile.
    data_type.primitive_width().unwrap_or(8).saturating_add(1)
}

/// Larghezza per riga di un valore CONVERTITO IN TESTO, per tipo.
///
/// Serve alle operazioni che producono una colonna `Utf8` a partire da
/// colonne di altro tipo — `melt` con `type_policy = "string"` e' il caso —
/// dove misurare la larghezza BINARIA della sorgente sottostima il
/// risultato: un `Int64` occupa otto byte come numero e fino a venti come
/// testo, un `Decimal128` fino a quaranta. E' memoria del RISULTATO, non un
/// temporaneo che il modello puo' permettersi di ignorare.
///
/// I valori sono i massimi della rappresentazione decimale prodotta dai
/// formattatori del progetto, piu' l'overhead di offset e validita' della
/// colonna `Utf8` che li contiene. Per i tipi gia' testuali o binari non
/// c'e' conversione e si torna al pavimento del tipo: la larghezza reale la
/// misura il chiamante sulla colonna.
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
/// [`text_bytes_floor`] guarda solo il `DataType` e per i tipi a lunghezza
/// variabile non ha nulla da dire. Per le `Dictionary` questo non basta, ed
/// e' un caso concreto che vanifica il preflight:
///
/// una `DictionaryArray` conta il valore testuale **una volta sola**, nel
/// dizionario, e le righe ne portano solo la chiave. L'output `Utf8` lo
/// **materializza per ogni riga**. Una singola stringa lunga referenziata da
/// centomila chiavi occupa pochi byte per riga nell'input e centinaia di
/// megabyte nell'output: la misura sull'input sottostima di ordini di
/// grandezza, e la stima autorizza l'allocazione che doveva impedire.
///
/// Il limite superiore esatto e' la **voce piu' lunga del dizionario**: ogni
/// chiave puo' puntarci. Il dizionario ha pochi elementi per costruzione —
/// e' il senso della codifica — quindi scorrerlo costa poco e si fa una
/// volta per colonna, non per riga.
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
/// Serve a chi deve rifiutare PRIMA di allocare: `melt` con
/// `type_policy = "string"` costruiva gli indici e la colonna dei nomi e solo
/// dopo scopriva, a meta' scansione, che una colonna valore non era
/// convertibile.
///
/// # I due tipi parametrici NON sono accettati per intero
///
/// La prima versione di questo predicato accettava qualunque
/// `Timestamp(_, _)` e qualunque `Decimal128(_, _)`. Il formatter e' piu'
/// stretto, e la differenza non era teorica:
///
/// - **Timestamp**: `scalar_as_string` fa `downcast_ref::<TimestampMillisecondArray>`,
///   quindi gestisce SOLO `Millisecond`. Un `Timestamp(Second, _)` o
///   `Timestamp(Nanosecond, _)` passava la prevalidazione e falliva dopo
///   l'allocazione — cioe' esattamente cio' che la prevalidazione esiste per
///   evitare.
/// - **Decimal128**: il formatter converte la scala con `u32::try_from`, che
///   rifiuta le scale NEGATIVE, e calcola `10^scala` con `checked_pow`, che
///   trabocca oltre 38. Arrow considera `Decimal128(38, -1)` un tipo valido,
///   quindi il caso e' raggiungibile.
///
/// # Che cosa questo predicato NON garantisce
///
/// E' una prevalidazione di TIPO, non di valore. Restano possibili i
/// fallimenti che dipendono dal contenuto della cella e che nessuna lettura
/// dello schema puo' anticipare: un `Binary` non UTF-8, un `Date32` o un
/// `Timestamp` fuori dall'intervallo rappresentabile, una timezone Arrow non
/// valida, una chiave dictionary fuori dal dizionario. Su quelli l'errore
/// arriva ancora durante la scansione.
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
/// E' la larghezza di UNA riga di quel batch. Chi stima un output la compone
/// secondo la propria operazione — sommando i lati per un prodotto
/// cartesiano, prendendo il massimo per un impilamento — invece di ricevere
/// una formula unica che non puo' essere giusta per tutte.
///
/// # Errors
///
/// [`PlenoraError::ResourceLimit`] se la somma delle larghezze non e'
/// rappresentabile. Aritmetica CONTROLLATA, non saturante: una stima che ha
/// perso il conto non puo' autorizzare un'allocazione, e con
/// `max_governed_memory_bytes` a fondo scala una somma saturata passerebbe il
/// confronto. E' la stessa regola del picco di memoria della CLI e dei
/// contatori dello spill.
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
/// I tetti dei kernel erano tutti *post*: si costruiva l'output e poi lo si
/// confrontava con `max_rows`. Per le righe va bene — il conteggio si sa
/// prima — ma per i BYTE no: un `cross_join` che rispetta `max_rows` puo'
/// comunque allocare molto oltre `max_governed_memory_bytes`, e l'unico esito
/// possibile diventa l'esaurimento della memoria, non un errore. Un tetto che
/// si puo' verificare solo dopo aver superato il tetto non e' un tetto.
///
/// `bytes_per_row` e' la larghezza di una riga di OUTPUT, e la calcola il
/// chiamante: solo il kernel sa come si compone la propria riga. La prima
/// versione di questa funzione la derivava da sola, prendendo il massimo
/// fra le sorgenti — corretto per un impilamento, SBAGLIATO per un prodotto
/// cartesiano (dove le righe si affiancano e i byte si sommano), per uno
/// schema unione (dove compaiono colonne che nessuna sorgente misurava) e
/// per un `melt` (dove una colonna nuova ripete i nomi delle colonne). Una
/// formula unica per operazioni diverse e' una formula sbagliata per quasi
/// tutte.
///
/// # Che cosa questa funzione NON garantisce
///
/// Non e' una misura: e' una stima, e la sua qualita' e' quella del modello
/// che il chiamante le passa. Anche con un buon modello resta fuori tutto
/// cio' che l'implementazione alloca oltre i buffer del risultato — vettori
/// di indici, tabelle hash, copie temporanee — a meno che il chiamante non lo
/// includa esplicitamente in `bytes_per_row`. Serve a impedire le esplosioni
/// di ordini di grandezza; NON rende `max_governed_memory_bytes` un tetto duro sulla
/// memoria del processo. Vedi `docs/errori-e-limiti.md`, errori-e-limiti.md#che-cosa-la-memoria-governata-non-garantisce.
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
/// a zero colonne puo' avere righe» vale per tutto il workspace, non solo per
/// i kernel tabellari, quindi la funzione vive nel crate comune. Il
/// ri-export tiene i chiamanti esistenti.
pub use plenora_core::batch_with_rows;

/// Verifica sullo SCHEMA tutto cio' che la conversione in testo puo'
/// rifiutare senza guardare i valori.
///
/// [`text_convertible`] copre il tipo; questa copre anche la **timezone**,
/// che sta nello schema e non nei dati: `scalar_as_string` la risolve con
/// `chrono_tz` a ogni riga, e una timezone non valida faceva fallire la
/// conversione durante la scansione — dopo le allocazioni — pur essendo
/// conoscibile prima di cominciare.
///
/// Resta fuori solo cio' che dipende dal CONTENUTO della cella: un `Binary`
/// non UTF-8, un istante o una data fuori intervallo, una chiave dictionary
/// fuori dal dizionario.
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
/// Ogni nome viene confrontato con i nomi gia' occupati — quelli dello schema
/// di input **piu' quelli risolti prima di lui** — e, se occupato, riceve il
/// primo suffisso `_1`, `_2`, ... libero. Il nome risolto viene poi
/// RISERVATO, cosi' il successivo non puo' finirci sopra.
///
/// # Perche' in sequenza
///
/// Risolvere i nomi in modo indipendente sembra equivalente e non lo e'. Con
/// una colonna di input `v` e la richiesta `["v", "v_1"]`:
///
/// - `v` collide con l'input e diventa `v_1`;
/// - `v_1` non collide con l'input — che contiene solo `v` — e resta `v_1`.
///
/// I due nomi RICHIESTI erano distinti, quindi nessun controllo sulla
/// configurazione poteva accorgersene, e il risultato erano due colonne di
/// output con lo stesso nome. Uno schema con nomi duplicati e' un contratto
/// rotto: chi legge per nome non sa quale colonna riceve.
///
/// La versione sequenziale rende il caso impossibile per costruzione, ed e'
/// il motivo per cui la funzione prende TUTTI i nomi insieme invece di
/// essere chiamata una volta per nome.
///
/// # Il nome GENERATO e' validato quanto quello richiesto
///
/// Il suffisso allunga il nome: `validate_output_name` sul solo nome
/// richiesto lasciava passare un nome di 1024 byte che, collidendo, diventava
/// `nome_1` — 1026 byte, cioe' oltre il limite che la validazione esiste per
/// imporre. Un invariante che si perde proprio nel caso che lo mette alla
/// prova non e' un invariante. Ogni candidato viene quindi validato prima di
/// essere accettato, e un candidato non valido e' scartato come se fosse
/// occupato: se nessuno regge, il nome non e' risolvibile.
///
/// # Quanti suffissi
///
/// [`MAX_SUFFISSI_COLLISIONE`], cioe' da `_1` a `_99`. Il numero e' una
/// costante e non un letterale sparso perche' il messaggio d'errore lo
/// riporta: la versione precedente provava novantanove suffissi e ne
/// dichiarava cento.
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
/// `Ok(None)` significa che la riga e' nulla, e la nullita' ha DUE sorgenti:
/// la chiave puo' essere nulla, oppure una chiave valida puo' puntare a una
/// entry nulla del dizionario. La seconda sfuggiva a tutti i chiamanti,
/// perche' `DictionaryArray::is_null` guarda solo la validita' delle CHIAVI:
/// una riga logicamente nulla veniva letta come stringa vuota, e quindi
/// ordinata, raggruppata, filtrata e scritta come se fosse un valore.
///
/// Il controllo dei limiti della chiave sta qui una volta sola: `value()` su
/// un indice fuori intervallo va in panico, e un panico su input non fidato
/// e' cio' che il gate R6 vieta.
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
/// Per ogni tipo coincide con `Array::is_null`, tranne per
/// `Dictionary(Int32, Utf8)`, dove una chiave valida puo' puntare a una entry
/// nulla: li' `is_null` risponde `false` su una riga che e' nulla. Ogni
/// percorso che decide «questa riga e' nulla» — filtri `isnull`/`notnull`,
/// salto delle righe nulle nei confronti, vincoli di qualita' — deve passare
/// di qui, altrimenti due percorsi danno due risposte sulla stessa riga.
///
/// Una chiave malformata (negativa o fuori intervallo) NON e' trattata come
/// null: la riga non e' nulla, e' l'array a essere incoerente. Il chiamante
/// che deve produrre un errore lo ottiene da [`dictionary_utf8_value`]; qui
/// si risponde `false` per non trasformare un difetto in un silenzio.
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
        // Null logico incluso: una chiave valida che punta a una entry nulla
        // non e' la stringa vuota. Il controllo dei limiti della chiave, che
        // qui mancava, vive nel risolutore condiviso.
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
// Classe di bug chiusa: `ToPrimitive::to_f64()` NON e' un test di
// rappresentabilita'. Per i64/u64/i128 restituisce sempre `Some`,
// arrotondando al double piu' vicino. Ogni sito che scriveva
// `value.to_f64().ok_or_else(|| "non rappresentabile")` dichiarava un
// controllo che non esisteva: il ramo di errore era codice morto e il valore
// arrotondato proseguiva nella pipeline. Sopra 2^53 due interi distinti
// diventano lo stesso double — chiavi di join che collassano, filtri che
// cambiano esito, confronti che invertono l'ordine.
//
// Gli helper qui sotto fanno il test vero: un intero e' esattamente
// rappresentabile in `f64` se e solo se la sua parte significativa (bit fra
// il primo e l'ultimo bit a 1) sta in 53 bit. E' un criterio esatto e non
// conservativo: 2^54, che ha un solo bit significativo, resta accettato.
//
// Nota sul round-trip ingenuo (`value as f64 as i64 == value`): non funziona
// agli estremi, perche' il cast f64 -> intero satura. `i64::MAX` arrotonda a
// 2^63, che risaturando torna `i64::MAX` e supererebbe il controllo pur non
// essendo rappresentabile.
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
/// Il valore e' `unscaled / 10^scale`, e verificare la sola
/// rappresentabilita' di `unscaled` non basta: e' la DIVISIONE a introdurre
/// l'errore. `1` con scala `1` vale `0.1`, che nessun double rappresenta,
/// eppure `unscaled = 1` supera qualunque controllo sull'intero.
///
/// Il criterio esatto: `10^scale = 2^scale * 5^scale`, e una frazione e'
/// rappresentabile in binario solo se il denominatore, ridotto ai minimi
/// termini, e' una potenza di due. Serve quindi che `5^scale` divida
/// esattamente `unscaled`; il quoziente deve poi stare in 53 bit, e la
/// divisione residua per `2^scale` e' esatta perche' e' una potenza di due.
#[allow(clippy::cast_precision_loss)] // Guardato da `magnitude_fits_f64`.
#[must_use]
pub fn exact_f64_from_decimal128(unscaled: i128, scale: i8) -> Option<f64> {
    // Lo zero e' esatto a QUALUNQUE scala e non richiede alcun fattore.
    // Calcolarlo prima faceva fallire `10^|scale|` per scale oltre +-38 e
    // rispondeva «non rappresentabile» sullo zero, che e' il valore piu'
    // rappresentabile che esista. Arrow non pone un limite inferiore alla
    // scala (`validate_decimal_precision_and_scale` rifiuta solo
    // `scale > 38`), quindi quelle scale arrivano davvero.
    if unscaled == 0 {
        return Some(0.0);
    }
    if scale == 0 {
        return exact_f64_from_i128(unscaled);
    }
    if scale < 0 {
        // Scala negativa: il valore e' `unscaled * 2^a * 5^a` con `a = -scale`.
        // La parte dispari contiene `5^a`, quindi l'esattezza in `f64`
        // richiede `5^a <= 2^53`, cioe' `a <= 22`: oltre, nessun valore non
        // nullo e' esatto e la risposta e' `None` per costruzione, non per
        // traboccamento del fattore.
        let a = u32::from(scale.unsigned_abs());
        if a > MAX_EXACT_POW5 {
            return None;
        }
        // Si moltiplica per `5^a` e si delega la potenza di due, che non
        // arrotonda. **Prima** pero' si estraggono le potenze di due gia'
        // presenti in `unscaled`: senza, `2^126 * 10` — che vale `5 * 2^127`
        // ed e' perfettamente esatto in `f64` — falliva perche' il prodotto
        // intermedio `2^126 * 5` esce da `i128`. Spostare quei fattori
        // dall'intero all'esponente non cambia il valore e toglie di mezzo
        // il traboccamento.
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
/// Le conversioni da intero (`Int64`, `UInt64`, `Timestamp`, `Decimal128`) sono
/// **esatte o errore**: un valore che non ha un `f64` esatto e' un errore
/// `Schema`, mai un arrotondamento silenzioso. Dove l'arrotondamento e'
/// invece voluto — statistiche, medie, misure che per contratto producono
/// `Float64` — il chiamante lo dichiara sul proprio sito, non qui.
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
/// E' la forma da usare nei kernel il cui risultato e' un `Float64` per
/// contratto — medie, varianze, quantili, aggregazioni, statistiche, valori
/// di una formula: li' il double e' il tipo del risultato, non un passaggio
/// intermedio, e pretendere l'esattezza rifiuterebbe input legittimi (un
/// `Decimal(10,2)` con valore `0.1` non ha alcun double esatto, eppure la sua
/// media e' una richiesta ragionevole).
///
/// Ovunque il valore serva a DECIDERE — confronti, chiavi, vincoli — vale
/// invece [`scalar_as_f64`], esatto o errore, o meglio ancora
/// [`scalar_compare`], che non converte affatto.
///
/// La deroga e' registrata in `docs/errori-e-limiti.md` (errori-e-limiti.md#limiti-dichiarati).
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
// Classe di bug chiusa (review 2026-07-27, stessa classe dei comparatori di
// `table.sort`): i confronti fatti via `scalar_as_f64` collassano interi
// distinti oltre 2^53 sullo stesso double (9007199254740992 e
// 9007199254740993 risultavano uguali) e il confronto testuale disordinava
// gli UInt64 ("10" < "9"). Il predicato condiviso qui sotto e' esatto per
// costruzione: nessuna conversione a f64 quando un lato e' un intero.
//
// Regola mista interi <-> float (decisione documentata): il valore di
// configurazione e' un letterale JSON reso testo; un letterale INTERO resta
// un intero esatto (`I64`, poi `U64`), ogni altra forma numerica
// (frazionaria, esponenziale, inf, NaN) e' `F64`. Il confronto intero <-> F64
// e' esatto: un double frazionario non e' mai uguale a un intero e ordina
// per floor; un double intero fuori gamma ordina per segno (2^63 > ogni
// i64); NaN rende falso ogni confronto (`None`), come in IEEE 754. Quindi
// 9007199254740993 (intero) > 9007199254740992.0 (double), mentre un
// letterale frazionario JSON (es. 9007199254740993.0) e' un double gia'
// arrotondato in deserializzazione e vale come tale.
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
    /// Parse del valore atteso: intero esatto se il testo e' un letterale
    /// intero, DECIMALE esatto se e' un letterale con virgola, altrimenti
    /// f64. `None` se il testo non e' numerico: il chiamante lo traduce nello
    /// stesso errore di contratto del percorso storico (che parsava solo f64,
    /// un sottoinsieme stretto di questi casi).
    ///
    /// La forma decimale e' quella che rende esatti i confronti con le
    /// colonne `Decimal128`. Parsare `10.5` come double e poi confrontarlo
    /// con un decimal significa decidere l'ordine su un valore che il double
    /// non rappresenta: due decimal distinti possono collassare sullo stesso
    /// float e risultare uguali.
    pub fn parse(text: &str) -> Option<Self> {
        if let Ok(value) = text.parse::<i64>() {
            return Some(Self::I64(value));
        }
        if let Ok(value) = text.parse::<u64>() {
            return Some(Self::U64(value));
        }
        // Interi oltre `u64` (o negativi oltre `i64`): restano esatti come
        // decimali a scala zero. Prima cadevano su `f64`, e
        // `18446744073709551617` — perfettamente rappresentabile in `i128` —
        // veniva arrotondato.
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
        // Zeri NON significativi via prima di misurare precisione e scala.
        // Contarli faceva ricadere su `f64` letterali perfettamente esatti:
        // `0000…0.1` (quaranta zeri) e' `unscaled = 1, scale = 1`, ma le sue
        // 41 cifre sforavano il tetto e il confronto con una colonna
        // `Decimal128` finiva nel dominio dei double — corretto rispetto al
        // double, sbagliato rispetto al letterale scritto.
        //
        // Gli zeri iniziali della PARTE FRAZIONARIA restano: sono la scala,
        // non un abbellimento (`0.001` non e' `0.1`).
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
/// Usato per colonne Float64 contro letterali interi di configurazione:
/// entro 2^53 coincide col confronto IEEE storico, oltre resta esatto.
/// Con bound `F64` vale la semantica IEEE (`partial_cmp`: NaN -> `None`).
///
/// Con bound `Decimal` il confronto e' RAZIONALE ESATTO, non nel dominio
/// dei double: il dato resta il double della colonna, ma la soglia scritta
/// dall'utente non viene arrotondata per confrontarla. Con
/// `column > 0.100000000000000001` e la colonna a `1e-1` la conversione
/// rendeva i due uguali, escludendo una riga che il letterale include.
pub fn compare_f64(actual: f64, bound: NumericBound) -> Option<Ordering> {
    match bound {
        NumericBound::I64(expected) => compare_i64_f64(expected, actual).map(Ordering::reverse),
        NumericBound::U64(expected) => compare_u64_f64(expected, actual).map(Ordering::reverse),
        // Il valore della colonna e' un double e resta tale; il LETTERALE di
        // configurazione e' pero' un decimale scritto dall'utente, e
        // convertirlo con `10^-scale` introduce un errore che non e' nel
        // dato ma nella lettura della soglia. `compare_decimal_with_f64`
        // ordina il decimale rispetto al double in aritmetica intera: qui
        // l'attore e' il double, quindi il verso si rovescia.
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
/// Il valore logico e' `unscaled * 10^(-scale)`. Contro un letterale INTERO o
/// DECIMALE il confronto e' interamente in `i128`, senza mai passare per
/// `f64`: e' cio' che impedisce a due decimal distinti di collassare sullo
/// stesso double e risultare uguali o invertiti.
///
/// Resta sul profilo IEEE solo il bound `F64`, cioe' una forma che il
/// letterale assume unicamente quando NON e' un decimale posizionale
/// (notazione esponenziale, infiniti, NaN, o piu' di 38 cifre): li' il valore
/// atteso e' un double per sua natura, non un decimale arrotondato.
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
    // decimal. La conversione collassava valori distinti — `1e-1` e
    // `0.100000000000000001` risultavano uguali — perche' portava il decimal
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
    // Uno zero non cambia valore riscalando: va deciso PRIMA, altrimenti il
    // ramo di traboccamento del fattore lo scambia per un valore enorme
    // (`0` a scala -38 contro `0` a scala 38 dava `Less` invece di `Equal`).
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
/// Ogni tipo e' confrontato nel proprio dominio nativo — `i64` per interi,
/// date e timestamp, `i128` scalato per i decimal, `NumericBound` per il
/// testo numerico — e mai attraverso `f64`, che sopra 2^53 collassa interi
/// distinti e sui decimal cambia l'esito del confronto.
///
/// Il chiamante ha gia' escluso le righe null. `None` significa confronto non
/// definito (estremo NaN): semantica IEEE, ogni operatore ordinato e' falso.
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
        // verificare il solo `unscaled` lo lasciava passare.
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
