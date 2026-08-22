//! Payload Arrow IPC del trasporto v3: pre-validazione strutturale del
//! framing e dei metadati flatbuffer, decodifica e codifica entro i limiti
//! di risorse.

use std::io::{Read, Seek, SeekFrom};

use plenora_core::arrow::array::RecordBatch;
use plenora_core::arrow::ipc::reader::StreamReader;
use plenora_core::arrow::ipc::writer::{FileWriter, StreamWriter};
use plenora_core::arrow::schema::SchemaRef;

use super::error::ArrowTransportError;
use super::protocol::{MAX_ROWS, MAX_STREAM_BYTES};
use super::transport::{MAX_BATCHES, MAX_COLUMNS, MAX_IPC_METADATA_BYTES};

/// Allineamento a 8 byte degli offset del framing IPC, su 64 bit: un file
/// puo' superare `usize` su piattaforme a 32 bit e gli offset non vanno mai
/// troncati.
const fn align8_u64(value: u64) -> u64 {
    value.saturating_add(7) & !7
}

/// Prefisso di continuazione dei messaggi IPC incapsulati.
const CONTINUATION_MARKER: u32 = 0xFFFF_FFFF;

/// Valore di `MessageHeader` per un `RecordBatch` (union dello standard IPC).
const IPC_HEADER_RECORD_BATCH: u8 = 3;

/// Magic del **file format** Arrow IPC, in testa e in coda al file.
const ARROW_FILE_MAGIC: &[u8; 6] = b"ARROW1";

/// Magic iniziale piu' il padding a 8 byte che lo segue.
const ARROW_FILE_HEADER_BYTES: u64 = 8;

/// Trailer del file format: lunghezza del footer (i32) piu' magic finale.
const ARROW_FILE_TRAILER_BYTES: u64 = 10;

fn le_u32(bytes: &[u8]) -> Result<u32, ArrowTransportError> {
    bytes
        .get(..4)
        .and_then(|slice| <[u8; 4]>::try_from(slice).ok())
        .map(u32::from_le_bytes)
        .ok_or(ArrowTransportError::IpcTruncated)
}

fn to_u64(value: usize) -> Result<u64, ArrowTransportError> {
    u64::try_from(value).map_err(|_| ArrowTransportError::IpcTruncated)
}

// --- Validazione strutturale dei metadati flatbuffer `Message` -------------
//
// arrow-format alloca `Vec::with_capacity(count)` per vettori e stringhe
// dichiarati nei metadati senza un tetto proprio: un payload malevolo puo'
// indurre allocazioni enormi (OOM, trovato via fuzzing). Questo validatore
// percorre la struttura `Message`/`Schema`/`RecordBatch` dello standard IPC
// e verifica che ogni vettore, stringa e buffer stia dentro i byte
// disponibili, prima che arrow-rs veda i metadati. Non e' un parser
// completo: copre solo la struttura che puo' allocare.

const MAX_FLATBUFFER_DEPTH: usize = 64;

/// Nodi totali (campi, figli compresi) ammessi in uno Schema IPC.
///
/// `MAX_COLUMNS` limita i soli campi di primo livello: i vettori `children`
/// venivano invece percorsi senza alcun tetto. Milioni di figli stanno
/// comodamente dentro il tetto sui metadati, e un `FlatBuffer` costruito a mano
/// puo' far puntare piu' entry allo STESSO sottoalbero — il validatore lo
/// visiterebbe una volta per riferimento, con crescita esponenziale fino alla
/// profondita' 64, e arrow espanderebbe poi lo stesso schema.
const MAX_SCHEMA_NODES: usize = 64 * 1024;

/// Budget di visita di uno Schema: conta i nodi e rifiuta i sottoalberi
/// condivisi.
///
/// Il conteggio da solo limita il LAVORO di questa validazione; il rifiuto
/// dei riferimenti ripetuti serve ad arrow, che sullo stesso schema farebbe
/// l'espansione vera. Un produttore onesto non emette mai un DAG: i `FlatBuffer`
/// di arrow-rs scrivono ogni campo una volta.
struct SchemaBudget {
    remaining: usize,
    visited: std::collections::HashSet<usize>,
}

impl SchemaBudget {
    fn new() -> Self {
        Self {
            remaining: MAX_SCHEMA_NODES,
            visited: std::collections::HashSet::new(),
        }
    }

    /// Consuma un nodo e registra la tabella visitata.
    fn enter(&mut self, table: usize) -> Result<(), ArrowTransportError> {
        self.remaining = self
            .remaining
            .checked_sub(1)
            .ok_or(ArrowTransportError::IpcSchemaTooComplex(MAX_SCHEMA_NODES))?;
        if !self.visited.insert(table) {
            return Err(ArrowTransportError::IpcSchemaTooComplex(MAX_SCHEMA_NODES));
        }
        Ok(())
    }
}

fn fb_u16(buf: &[u8], pos: usize) -> Result<u16, ArrowTransportError> {
    buf.get(pos..pos + 2)
        .and_then(|bytes| <[u8; 2]>::try_from(bytes).ok())
        .map(u16::from_le_bytes)
        .ok_or(ArrowTransportError::IpcTruncated)
}

fn fb_u32(buf: &[u8], pos: usize) -> Result<u32, ArrowTransportError> {
    buf.get(pos..pos + 4)
        .and_then(|bytes| <[u8; 4]>::try_from(bytes).ok())
        .map(u32::from_le_bytes)
        .ok_or(ArrowTransportError::IpcTruncated)
}

fn fb_i32(buf: &[u8], pos: usize) -> Result<i32, ArrowTransportError> {
    buf.get(pos..pos + 4)
        .and_then(|bytes| <[u8; 4]>::try_from(bytes).ok())
        .map(i32::from_le_bytes)
        .ok_or(ArrowTransportError::IpcTruncated)
}

fn fb_i64(buf: &[u8], pos: usize) -> Result<i64, ArrowTransportError> {
    buf.get(pos..pos + 8)
        .and_then(|bytes| <[u8; 8]>::try_from(bytes).ok())
        .map(i64::from_le_bytes)
        .ok_or(ArrowTransportError::IpcTruncated)
}

/// Tabella flatbuffer in `pos`: ritorna (`vtable_start`, `vtable_len`).
/// A `pos` c'e' l'`soffset` (i32, distanza alla vtable); `vtable_len` e
/// `table_len` stanno nella vtable stessa. L'`soffset` puo' essere NEGATIVO:
/// con vtable deduplicate il writer puo' piazzare la vtable dopo la tabella.
fn fb_table(buf: &[u8], pos: usize) -> Result<(usize, usize), ArrowTransportError> {
    let soffset = buf
        .get(pos..pos + 4)
        .and_then(|bytes| <[u8; 4]>::try_from(bytes).ok())
        .map(i32::from_le_bytes)
        .ok_or(ArrowTransportError::IpcTruncated)?;
    if soffset == 0 {
        return Err(ArrowTransportError::IpcTruncated);
    }
    // Conversioni totali: un offset che non entra in i64/usize e' un
    // riferimento malformato, mai un troncamento silenzioso (R5.4).
    let vtable_signed =
        i64::try_from(pos).map_err(|_| ArrowTransportError::IpcTruncated)? - i64::from(soffset);
    let vtable = usize::try_from(vtable_signed).map_err(|_| ArrowTransportError::IpcTruncated)?;
    let vtable_len = fb_u16(buf, vtable)? as usize;
    let table_len = fb_u16(buf, vtable + 2)? as usize;
    if vtable_len < 4
        || !vtable_len.is_multiple_of(2)
        || vtable + vtable_len > buf.len()
        || pos + table_len > buf.len()
    {
        return Err(ArrowTransportError::IpcTruncated);
    }
    Ok((vtable, vtable_len))
}

/// Offset del campo `index` dalla vtable (0 se assente).
fn fb_field(
    buf: &[u8],
    vtable: usize,
    vtable_len: usize,
    index: usize,
) -> Result<usize, ArrowTransportError> {
    let entry = 4 + index * 2;
    if entry + 2 > vtable_len {
        return Ok(0);
    }
    let bytes: [u8; 2] = buf
        .get(vtable + entry..vtable + entry + 2)
        .and_then(|slice| slice.try_into().ok())
        .ok_or(ArrowTransportError::IpcTruncated)?;
    Ok(u16::from_le_bytes(bytes) as usize)
}

/// Posizione assoluta di un campo indiretto (tabella, vettore, stringa).
fn fb_indirect(buf: &[u8], table: usize, offset: usize) -> Result<usize, ArrowTransportError> {
    let relative = fb_u32(buf, table + offset)? as usize;
    let target = (table + offset)
        .checked_add(relative)
        .ok_or(ArrowTransportError::IpcTruncated)?;
    if target + 4 > buf.len() {
        return Err(ArrowTransportError::IpcTruncated);
    }
    Ok(target)
}

/// Conteggio di un vettore flatbuffer con elementi da `elem_size` byte:
/// il contenuto deve stare interamente nel buffer.
fn fb_vector(buf: &[u8], pos: usize, elem_size: usize) -> Result<usize, ArrowTransportError> {
    let count = fb_u32(buf, pos)? as usize;
    let bytes = count
        .checked_mul(elem_size)
        .and_then(|bytes| bytes.checked_add(4))
        .ok_or(ArrowTransportError::IpcTruncated)?;
    if pos + bytes > buf.len() {
        return Err(ArrowTransportError::IpcTruncated);
    }
    Ok(count)
}

/// Stringa flatbuffer (vettore di byte con terminatore).
fn fb_string(buf: &[u8], pos: usize) -> Result<(), ArrowTransportError> {
    let count = fb_vector(buf, pos, 1)?;
    if pos + 4 + count >= buf.len() {
        return Err(ArrowTransportError::IpcTruncated);
    }
    Ok(())
}

fn fb_key_value(buf: &[u8], table: usize) -> Result<(), ArrowTransportError> {
    let (vtable, vtable_len) = fb_table(buf, table)?;
    for index in [0, 1] {
        let offset = fb_field(buf, vtable, vtable_len, index)?;
        if offset != 0 {
            fb_string(buf, fb_indirect(buf, table, offset)?)?;
        }
    }
    Ok(())
}

fn fb_custom_metadata(buf: &[u8], table: usize, offset: usize) -> Result<(), ArrowTransportError> {
    if offset == 0 {
        return Ok(());
    }
    let vector = fb_indirect(buf, table, offset)?;
    let count = fb_vector(buf, vector, 4)?;
    for index in 0..count {
        let entry = fb_indirect(buf, vector + 4, index * 4)?;
        fb_key_value(buf, entry)?;
    }
    Ok(())
}

/// Tabella `Field` di uno Schema IPC.
fn fb_field_table(
    buf: &[u8],
    table: usize,
    depth: usize,
    budget: &mut SchemaBudget,
) -> Result<(), ArrowTransportError> {
    if depth > MAX_FLATBUFFER_DEPTH {
        return Err(ArrowTransportError::IpcTruncated);
    }
    budget.enter(table)?;
    let (vtable, vtable_len) = fb_table(buf, table)?;
    // name: stringa.
    let name = fb_field(buf, vtable, vtable_len, 0)?;
    if name != 0 {
        fb_string(buf, fb_indirect(buf, table, name)?)?;
    }
    // type (union): la tabella e' verificata nei limiti; il solo tipo con
    // vettori (Union.typeIds) e' controllato esplicitamente.
    let type_type_offset = fb_field(buf, vtable, vtable_len, 2)?;
    let type_offset = fb_field(buf, vtable, vtable_len, 3)?;
    if type_offset != 0 {
        let union_table = fb_indirect(buf, table, type_offset)?;
        let (type_vtable, type_vtable_len) = fb_table(buf, union_table)?;
        if type_type_offset != 0 {
            let type_type = *buf
                .get(table + type_type_offset)
                .ok_or(ArrowTransportError::IpcTruncated)?;
            if type_type == 14 {
                let type_ids = fb_field(buf, type_vtable, type_vtable_len, 3)?;
                if type_ids != 0 {
                    fb_vector(buf, fb_indirect(buf, union_table, type_ids)?, 4)?;
                }
            }
        }
    }
    // dictionary: DictionaryEncoding (scalari + tabella Int).
    let dictionary = fb_field(buf, vtable, vtable_len, 4)?;
    if dictionary != 0 {
        let dictionary_table = fb_indirect(buf, table, dictionary)?;
        let (dict_vtable, dict_vtable_len) = fb_table(buf, dictionary_table)?;
        let index_type = fb_field(buf, dict_vtable, dict_vtable_len, 1)?;
        if index_type != 0 {
            fb_table(buf, fb_indirect(buf, dictionary_table, index_type)?)?;
        }
    }
    // children: vettore di Field.
    let children = fb_field(buf, vtable, vtable_len, 5)?;
    if children != 0 {
        let vector = fb_indirect(buf, table, children)?;
        let count = fb_vector(buf, vector, 4)?;
        for index in 0..count {
            let child = fb_indirect(buf, vector + 4, index * 4)?;
            fb_field_table(buf, child, depth + 1, budget)?;
        }
    }
    // custom_metadata.
    let custom = fb_field(buf, vtable, vtable_len, 6)?;
    fb_custom_metadata(buf, table, custom)?;
    Ok(())
}

/// Tabella `RecordBatch`: nodi, buffer (entro il body dichiarato), variadic.
fn fb_record_batch(buf: &[u8], table: usize, body_len: usize) -> Result<(), ArrowTransportError> {
    let (vtable, vtable_len) = fb_table(buf, table)?;
    let nodes = fb_field(buf, vtable, vtable_len, 1)?;
    if nodes != 0 {
        fb_vector(buf, fb_indirect(buf, table, nodes)?, 16)?;
    }
    let buffers = fb_field(buf, vtable, vtable_len, 2)?;
    if buffers != 0 {
        let vector = fb_indirect(buf, table, buffers)?;
        let count = fb_vector(buf, vector, 16)?;
        for index in 0..count {
            let entry = vector + 4 + index * 16;
            let buffer_offset = fb_i64(buf, entry)?;
            let length = fb_i64(buf, entry + 8)?;
            // Conversione totale: negativi o oltre usize (target a 32 bit)
            // sono offset malformati, rifiutati invece che troncati.
            let end = usize::try_from(buffer_offset)
                .ok()
                .zip(usize::try_from(length).ok())
                .and_then(|(offset, len)| offset.checked_add(len))
                .ok_or(ArrowTransportError::IpcTruncated)?;
            if end > body_len {
                return Err(ArrowTransportError::IpcTruncated);
            }
        }
    }
    let compression = fb_field(buf, vtable, vtable_len, 3)?;
    if compression != 0 {
        fb_table(buf, fb_indirect(buf, table, compression)?)?;
        // Con `bodyCompression` la dimensione che arrow allochera' NON e' il
        // `bodyLength` del messaggio ma la somma delle lunghezze decompresse,
        // dichiarate nei prefissi a 8 byte dei singoli buffer — dentro il
        // body, cioe' proprio la regione che la pre-validazione non legge.
        // Un tetto sul body compresso non limiterebbe quindi nulla, e
        // fidarsi dei prefissi dichiarati sarebbe lo stesso schema
        // "lunghezza dichiarata" da cui il confine difende.
        //
        // Il confine rifiuta quindi la classe intera: i writer di questo
        // progetto non emettono mai IPC compresso (le `IpcWriteOptions` di
        // default non comprimono), quindi non rifiuta nulla di nostro, e per
        // un input esterno un rifiuto esplicito e' meglio di
        // un'allocazione non misurata.
        return Err(ArrowTransportError::IpcUnsupportedFeature(
            "body compresso (bodyCompression)",
        ));
    }
    let variadic = fb_field(buf, vtable, vtable_len, 4)?;
    if variadic != 0 {
        fb_vector(buf, fb_indirect(buf, table, variadic)?, 8)?;
    }
    Ok(())
}

/// Tabella `Schema`: fields, `custom_metadata` e feature.
fn fb_schema(buf: &[u8], table: usize) -> Result<(), ArrowTransportError> {
    let (vtable, vtable_len) = fb_table(buf, table)?;
    let fields = fb_field(buf, vtable, vtable_len, 1)?;
    if fields != 0 {
        let vector = fb_indirect(buf, table, fields)?;
        let count = fb_vector(buf, vector, 4)?;
        if count > MAX_COLUMNS {
            return Err(ArrowTransportError::TooManyColumns(count));
        }
        // Budget CUMULATIVO sull'intero schema: i figli annidati consumano lo
        // stesso conto dei campi di primo livello.
        let mut budget = SchemaBudget::new();
        for index in 0..count {
            let field = fb_indirect(buf, vector + 4, index * 4)?;
            fb_field_table(buf, field, 0, &mut budget)?;
        }
    }
    let custom = fb_field(buf, vtable, vtable_len, 2)?;
    fb_custom_metadata(buf, table, custom)?;
    let features = fb_field(buf, vtable, vtable_len, 3)?;
    if features != 0 {
        fb_vector(buf, fb_indirect(buf, table, features)?, 8)?;
    }
    Ok(())
}

/// Valida i metadati flatbuffer di un messaggio IPC e ritorna la lunghezza
/// del body dichiarata (`bodyLength`). Header Tensor/SparseTensor sono
/// rifiutati: il trasporto non li usa e nessun produttore onesto li emette.
fn validate_ipc_message_metadata(metadata: &[u8]) -> Result<(usize, u8), ArrowTransportError> {
    if metadata.len() < 8 {
        return Err(ArrowTransportError::IpcTruncated);
    }
    let table = fb_u32(metadata, 0)? as usize;
    let (vtable, vtable_len) = fb_table(metadata, table)?;

    // version (0) e header_type (1) sono scalari; header (2) e' la tabella
    // del messaggio; bodyLength (3) uno scalare i64; custom_metadata (4).
    let header_type_offset = fb_field(metadata, vtable, vtable_len, 1)?;
    let header_type = if header_type_offset == 0 {
        0
    } else {
        *metadata
            .get(table + header_type_offset)
            .ok_or(ArrowTransportError::IpcTruncated)?
    };
    let header_offset = fb_field(metadata, vtable, vtable_len, 2)?;
    let header_table = if header_offset == 0 {
        None
    } else {
        Some(fb_indirect(metadata, table, header_offset)?)
    };
    // `bodyLength` si legge PRIMA di validare l'header: e' il solo metro con
    // cui verificare i buffer, sia di un RecordBatch sia del RecordBatch
    // interno a un DictionaryBatch. La versione precedente validava il
    // dictionary contro `metadata.len()` — la lunghezza dei METADATI, che con
    // il body non ha alcun rapporto: rifiutava dictionary legittime con
    // metadati corti e accettava buffer ben oltre il body dichiarato.
    let body_len_offset = fb_field(metadata, vtable, vtable_len, 3)?;
    let body_len = if body_len_offset == 0 {
        0
    } else {
        let value = fb_i64(metadata, table + body_len_offset)?;
        if value < 0 {
            return Err(ArrowTransportError::IpcTruncated);
        }
        usize::try_from(value).map_err(|_| ArrowTransportError::IpcTruncated)?
    };

    if let Some(header_table) = header_table {
        match header_type {
            1 => fb_schema(metadata, header_table)?,
            2 => {
                // DictionaryBatch: data (RecordBatch) al campo 1.
                let (dict_vtable, dict_vtable_len) = fb_table(metadata, header_table)?;
                let data = fb_field(metadata, dict_vtable, dict_vtable_len, 1)?;
                if data != 0 {
                    let batch = fb_indirect(metadata, header_table, data)?;
                    fb_record_batch(metadata, batch, body_len)?;
                }
            }
            3 => fb_record_batch(metadata, header_table, body_len)?,
            _ => {
                return Err(ArrowTransportError::Arrow(
                    "header IPC Tensor/SparseTensor non supportato".to_owned(),
                ))
            }
        }
    }

    let custom = fb_field(metadata, vtable, vtable_len, 4)?;
    fb_custom_metadata(metadata, table, custom)?;
    Ok((body_len, header_type))
}

// --- Sorgente di byte per la pre-validazione del framing -------------------

/// Sorgente di byte su cui gira la pre-validazione del framing IPC.
///
/// La stessa procedura serve due ingressi con vincoli di memoria opposti: il
/// payload del trasporto, che e' gia' interamente in memoria, e i file aperti
/// dagli ingressi pubblici, che NON vanno caricati per intero. Leggere per
/// offset e' quindi l'unica interfaccia comune possibile: si materializzano
/// solo i metadati di ogni messaggio — tetto [`MAX_IPC_METADATA_BYTES`],
/// verificato PRIMA della lettura — e il body si salta per offset.
pub trait IpcSource {
    /// Byte totali disponibili nella sorgente.
    fn total_len(&self) -> u64;

    /// Copia in `out` esattamente `len` byte a partire da `offset`.
    ///
    /// # Errors
    ///
    /// `IpcTruncated` se la finestra esce dalla sorgente, `Io` sugli errori
    /// di lettura.
    fn read_at(
        &mut self,
        offset: u64,
        len: usize,
        out: &mut Vec<u8>,
    ) -> Result<(), ArrowTransportError>;
}

impl IpcSource for &[u8] {
    fn total_len(&self) -> u64 {
        // Su ogni target supportato `usize` entra in `u64`; il saturante
        // evita un panico teorico invece di introdurre un unwrap.
        u64::try_from(self.len()).unwrap_or(u64::MAX)
    }

    fn read_at(
        &mut self,
        offset: u64,
        len: usize,
        out: &mut Vec<u8>,
    ) -> Result<(), ArrowTransportError> {
        let start = usize::try_from(offset).map_err(|_| ArrowTransportError::IpcTruncated)?;
        let end = start
            .checked_add(len)
            .ok_or(ArrowTransportError::IpcTruncated)?;
        let window = self
            .get(start..end)
            .ok_or(ArrowTransportError::IpcTruncated)?;
        out.clear();
        out.extend_from_slice(window);
        Ok(())
    }
}

/// Sorgente su un lettore posizionabile (tipicamente un `File`): legge solo
/// le finestre richieste, mai il file intero.
pub struct SeekSource<R> {
    reader: R,
    total_len: u64,
}

impl<R: Read + Seek> SeekSource<R> {
    pub const fn new(reader: R, total_len: u64) -> Self {
        Self { reader, total_len }
    }

    /// Restituisce il lettore riportandolo all'inizio, pronto per arrow.
    ///
    /// # Errors
    ///
    /// `Io` se il riposizionamento fallisce.
    pub fn rewind(mut self) -> Result<R, ArrowTransportError> {
        self.reader.seek(SeekFrom::Start(0))?;
        Ok(self.reader)
    }
}

impl<R: Read + Seek> IpcSource for SeekSource<R> {
    fn total_len(&self) -> u64 {
        self.total_len
    }

    fn read_at(
        &mut self,
        offset: u64,
        len: usize,
        out: &mut Vec<u8>,
    ) -> Result<(), ArrowTransportError> {
        // Il controllo di range precede l'allocazione: `len` e' gia' limitato
        // dal chiamante a MAX_IPC_METADATA_BYTES, ma la finestra deve stare
        // nella sorgente prima che si tocchi memoria.
        let end = offset
            .checked_add(to_u64(len)?)
            .ok_or(ArrowTransportError::IpcTruncated)?;
        if end > self.total_len {
            return Err(ArrowTransportError::IpcTruncated);
        }
        self.reader.seek(SeekFrom::Start(offset))?;
        out.clear();
        out.resize(len, 0);
        self.reader.read_exact(out)?;
        Ok(())
    }
}

/// Cosa fare quando la regione dei messaggi finisce senza marcatore di fine
/// stream (EOS, `metadata_len == 0`).
#[derive(Clone, Copy)]
pub enum EndOfData {
    /// L'EOS e' obbligatorio: il payload del trasporto dichiara la propria
    /// lunghezza nell'envelope, quindi una regione che finisce senza EOS e'
    /// troncata.
    RequireEos,
    /// La fine della regione vale come terminatore: nel file format il
    /// footer delimita gia' i messaggi e l'EOS e' opzionale.
    Accept,
}

/// Limiti che il confine applica PRIMA che arrow allochi.
///
/// Esistono perche' i limiti del piano arrivano troppo tardi:
/// `max_batch_bytes` misura un `RecordBatch` gia' materializzato, cioe' dopo
/// l'allocazione che dovrebbe impedire. Questi si applicano sulle lunghezze
/// DICHIARATE, prima che una sola pagina venga allocata.
#[derive(Debug, Clone, Copy)]
pub struct IpcLimits {
    /// Tetto sui metadati di un singolo messaggio (e sul footer del file).
    pub max_metadata_bytes: usize,
    /// Tetto sul `bodyLength` dichiarato di un singolo messaggio.
    pub max_body_bytes: u64,
    /// Numero massimo di RECORD BATCH.
    ///
    /// E' il limite semantico del piano (`max_batches`): conta i soli
    /// messaggi che portano dati.
    pub max_record_batches: usize,
    /// Numero massimo di messaggi TOTALI, dati e ausiliari.
    ///
    /// Uno stream con un solo record batch contiene almeno lo schema e il
    /// batch, e con le colonne dictionary anche un `DictionaryBatch` per
    /// campo: confondere i due conteggi — come faceva la versione precedente,
    /// che assegnava `max_messages = max_batches` — rifiutava qualunque
    /// stream non vuoto con `max_batches = 1`.
    pub max_messages: usize,
}

impl Default for IpcLimits {
    fn default() -> Self {
        Self {
            max_metadata_bytes: MAX_IPC_METADATA_BYTES,
            // Il default coincide con `BatchTarget::max_batch_bytes`: il body
            // di un messaggio e' esattamente il batch che ne uscira'.
            max_body_bytes: DEFAULT_MAX_BODY_BYTES,
            max_record_batches: MAX_BATCHES,
            max_messages: MAX_TOTAL_IPC_MESSAGES,
        }
    }
}

impl IpcLimits {
    /// Profilo del payload del trasporto v3.
    ///
    /// Il trasporto ha limiti PROPRI e gia' dichiarati — l'envelope porta la
    /// lunghezza del payload, `MAX_CELL_BYTES` limita la singola cella WKB —
    /// e un batch con una cella al massimo consentito ha per costruzione un
    /// body maggiore del tetto per-batch degli ingressi file. Applicare qui
    /// il profilo stretto trasformerebbe un `CellTooLarge` diagnostico in un
    /// generico "body troppo grande", peggiorando l'errore senza aggiungere
    /// protezione: il tetto vero e' `MAX_STREAM_BYTES`.
    #[must_use]
    pub const fn transport() -> Self {
        Self {
            max_metadata_bytes: MAX_IPC_METADATA_BYTES,
            max_body_bytes: MAX_STREAM_BYTES,
            max_record_batches: MAX_BATCHES,
            max_messages: MAX_TOTAL_IPC_MESSAGES,
        }
    }
}

/// Default del tetto sul body: stesso valore di `BatchTarget::max_batch_bytes`
/// (64 MiB), che e' il limite con cui l'executor misura il batch risultante.
pub const DEFAULT_MAX_BODY_BYTES: u64 = 64 * 1024 * 1024;

/// Tetto sui messaggi TOTALI di uno stream: i record batch piu' lo schema e i
/// `DictionaryBatch`. Il fattore rispetto a `MAX_BATCHES` copre uno schema e
/// un dizionario per colonna.
pub const MAX_TOTAL_IPC_MESSAGES: usize = MAX_BATCHES * 4;

/// Pre-validazione del framing IPC prima che arrow-rs allochi: ogni messaggio
/// dichiara la lunghezza dei propri metadati e il flatbuffer dichiara il
/// body; entrambi devono stare dentro la regione ED entro i limiti, i
/// metadati entro un tetto assoluto e la struttura flatbuffer entro i propri
/// limiti. Senza questo controllo un payload malevolo induce allocazioni
/// enormi dentro arrow-rs (OOM, trovato via fuzzing).
fn validate_framing_region<S: IpcSource + ?Sized>(
    source: &mut S,
    start: u64,
    end_limit: u64,
    end_of_data: EndOfData,
    limits: &IpcLimits,
) -> Result<(), ArrowTransportError> {
    let mut scratch = Vec::new();
    let mut offset = start;
    let mut messages = 0_usize;
    let mut record_batches = 0_usize;
    loop {
        if offset >= end_limit {
            return match end_of_data {
                EndOfData::Accept => Ok(()),
                EndOfData::RequireEos => Err(ArrowTransportError::IpcTruncated),
            };
        }
        source.read_at(offset, 4, &mut scratch)?;
        let prefix = le_u32(&scratch)?;
        let (metadata_len, header) = if prefix == CONTINUATION_MARKER {
            source.read_at(
                offset
                    .checked_add(4)
                    .ok_or(ArrowTransportError::IpcTruncated)?,
                4,
                &mut scratch,
            )?;
            (le_u32(&scratch)? as usize, 8_u64)
        } else {
            (prefix as usize, 4_u64)
        };
        if metadata_len == 0 {
            // Fine stream: il marcatore deve CHIUDERE la regione. Uscire qui
            // senza guardare cosa segue lasciava passare byte e messaggi
            // interi dopo l'EOS — validati da nessuno e ignorati dal reader,
            // che e' esattamente la forma dello smuggling.
            let after = offset
                .checked_add(header)
                .ok_or(ArrowTransportError::IpcTruncated)?;
            if after == end_limit {
                return Ok(());
            }
            return Err(ArrowTransportError::IpcTrailingAfterEos);
        }
        messages = messages.saturating_add(1);
        if messages > limits.max_messages {
            return Err(ArrowTransportError::IpcTooManyMessages(
                messages,
                limits.max_messages,
            ));
        }
        let (body_len, header_type) =
            validate_message_at(source, offset, header, metadata_len, end_limit, limits)?;
        if header_type == IPC_HEADER_RECORD_BATCH {
            record_batches = record_batches.saturating_add(1);
            if record_batches > limits.max_record_batches {
                return Err(ArrowTransportError::IpcTooManyRecordBatches(
                    record_batches,
                    limits.max_record_batches,
                ));
            }
        }
        let metadata_bytes = to_u64(metadata_len)?;
        let metadata_end = offset
            .checked_add(header)
            .and_then(|start| start.checked_add(metadata_bytes))
            .ok_or(ArrowTransportError::IpcTruncated)?;
        let end = align8_u64(
            align8_u64(metadata_end)
                .checked_add(body_len)
                .ok_or(ArrowTransportError::IpcTruncated)?,
        );
        if end > end_limit {
            return Err(ArrowTransportError::IpcTruncated);
        }
        offset = end;
    }
}

/// Valida il messaggio incapsulato che comincia a `offset` e ne restituisce
/// il `bodyLength` dichiarato, applicando i tetti su metadati e body PRIMA di
/// leggere o di lasciar procedere.
fn validate_message_at<S: IpcSource + ?Sized>(
    source: &mut S,
    offset: u64,
    header: u64,
    metadata_len: usize,
    end_limit: u64,
    limits: &IpcLimits,
) -> Result<(u64, u8), ArrowTransportError> {
    let metadata_start = offset
        .checked_add(header)
        .ok_or(ArrowTransportError::IpcTruncated)?;
    // Una lunghezza DICHIARATA che non e' nemmeno contenuta nella sorgente
    // descrive un file ROTTO, non un file troppo grande: nessuno supera un
    // budget con byte che non esistono. La verifica di disponibilita' viene
    // quindi prima del tetto, altrimenti diciannove byte di spazzatura — i
    // cui primi quattro si leggono come una lunghezza enorme — uscivano come
    // `resource_limit`, cioe' «rilancia con piu' budget» per un file che non
    // e' un file IPC. E' pura aritmetica su `end_limit`: nessun byte viene
    // letto prima del tetto, quindi la proprieta' di non materializzare una
    // finestra arbitraria resta intatta.
    let metadata_end = metadata_start
        .checked_add(to_u64(metadata_len)?)
        .ok_or(ArrowTransportError::IpcTruncated)?;
    if metadata_end > end_limit {
        return Err(ArrowTransportError::IpcTruncated);
    }
    // Il tetto sui metadati precede qualunque lettura: e' il controllo che
    // impedisce di materializzare una finestra arbitraria.
    if metadata_len > limits.max_metadata_bytes {
        return Err(ArrowTransportError::IpcMetadataTooLarge(
            metadata_len,
            limits.max_metadata_bytes,
        ));
    }
    let mut metadata = Vec::new();
    source.read_at(metadata_start, metadata_len, &mut metadata)?;
    let (body_len, header_type) = validate_ipc_message_metadata(&metadata)?;
    let body_len = to_u64(body_len)?;
    // Stesso criterio per il corpo: se i byte dichiarati non stanno nella
    // regione, il messaggio e' troncato, non oltre budget.
    let fine = align8_u64(
        align8_u64(metadata_end)
            .checked_add(body_len)
            .ok_or(ArrowTransportError::IpcTruncated)?,
    );
    if fine > end_limit {
        return Err(ArrowTransportError::IpcTruncated);
    }
    // Il tetto sul body si applica alla lunghezza DICHIARATA, cioe' prima che
    // arrow legga un solo byte del corpo. `max_batch_bytes` dell'executor
    // misura invece il `RecordBatch` gia' costruito: troppo tardi per
    // impedire l'allocazione che dovrebbe limitare.
    if body_len > limits.max_body_bytes {
        return Err(ArrowTransportError::IpcBodyTooLarge {
            declared: body_len,
            limit: limits.max_body_bytes,
        });
    }
    Ok((body_len, header_type))
}

/// Pre-validazione del framing di un payload IPC **stream format** gia' in
/// memoria: l'EOS e' obbligatorio.
fn validate_ipc_framing(payload: &[u8]) -> Result<(), ArrowTransportError> {
    let mut source = payload;
    let end = source.total_len();
    validate_framing_region(
        &mut source,
        0,
        end,
        EndOfData::RequireEos,
        &IpcLimits::transport(),
    )
}

/// Pre-validazione del framing di uno **stream** IPC letto da una sorgente
/// posizionabile. La fine dei dati vale come terminatore: a differenza del
/// payload del trasporto — la cui lunghezza e' dichiarata dall'envelope — un
/// file non porta con se' la propria lunghezza attesa, e `StreamReader`
/// tratta l'EOF come fine dello stream.
///
/// # Errors
///
/// `IpcTruncated`, `IpcMetadataTooLarge`, `IpcBodyTooLarge`,
/// `IpcTooManyMessages`, `TooManyColumns` o `Io`.
pub fn validate_ipc_stream_framing<S: IpcSource + ?Sized>(
    source: &mut S,
    limits: &IpcLimits,
) -> Result<(), ArrowTransportError> {
    let end = source.total_len();
    validate_framing_region(source, 0, end, EndOfData::Accept, limits)
}

/// Un blocco del footer del file format: dove arrow andra' DAVVERO a leggere.
#[derive(Clone, Copy)]
struct FooterBlock {
    offset: u64,
    metadata_len: u64,
    body_len: u64,
}

impl FooterBlock {
    /// Primo byte oltre il blocco.
    fn end(self) -> Result<u64, ArrowTransportError> {
        self.offset
            .checked_add(self.metadata_len)
            .and_then(|end| end.checked_add(self.body_len))
            .ok_or(ArrowTransportError::IpcFooterInvalid(
                "blocco con lunghezze fuori intervallo",
            ))
    }
}

/// Byte di un `Block` del footer (flatbuffer struct: offset i64,
/// `metaDataLength` i32 + padding, `bodyLength` i64).
const FOOTER_BLOCK_BYTES: usize = 24;

/// Legge il vettore di `Block` in `field` della tabella `Footer`.
fn fb_footer_blocks(
    footer: &[u8],
    table: usize,
    vtable: usize,
    vtable_len: usize,
    field: usize,
    blocks: &mut Vec<FooterBlock>,
    limits: &IpcLimits,
) -> Result<(), ArrowTransportError> {
    let offset = fb_field(footer, vtable, vtable_len, field)?;
    if offset == 0 {
        return Ok(());
    }
    let vector = fb_indirect(footer, table, offset)?;
    let count = fb_vector(footer, vector, FOOTER_BLOCK_BYTES)?;
    if blocks.len().saturating_add(count) > limits.max_messages {
        return Err(ArrowTransportError::IpcTooManyMessages(
            blocks.len().saturating_add(count),
            limits.max_messages,
        ));
    }
    for index in 0..count {
        let entry = vector + 4 + index * FOOTER_BLOCK_BYTES;
        // Layout dello struct flatbuffer `Block`: offset (i64), poi
        // metaDataLength (i32) con quattro byte di padding, poi bodyLength
        // (i64) — 24 byte in tutto, allineati a 8.
        let block_offset = fb_i64(footer, entry)?;
        let metadata_len = fb_i32(footer, entry + 8)?;
        let body_len = fb_i64(footer, entry + 16)?;
        let (Ok(offset), Ok(metadata_len), Ok(body_len)) = (
            u64::try_from(block_offset),
            u64::try_from(metadata_len),
            u64::try_from(body_len),
        ) else {
            return Err(ArrowTransportError::IpcFooterInvalid(
                "blocco con offset o lunghezze negative",
            ));
        };
        if metadata_len == 0 {
            return Err(ArrowTransportError::IpcFooterInvalid(
                "blocco senza metadati",
            ));
        }
        blocks.push(FooterBlock {
            offset,
            metadata_len,
            body_len,
        });
    }
    Ok(())
}

/// Pre-validazione del **file format** IPC, guidata dal FOOTER.
///
/// `FileReader` non percorre i messaggi in sequenza: legge il footer e salta
/// direttamente agli `offset` dei suoi blocchi. Una scansione sequenziale che
/// si ferma al primo EOS valida quindi una regione che arrow potrebbe non
/// leggere mai, e lascia non validata quella che leggera' davvero: bastava un
/// footer che puntasse altrove per aggirare l'intero confine.
///
/// La validazione segue percio' la stessa mappa di arrow — magic, trailer,
/// footer, blocchi — e per ogni blocco verifica offset, allineamento,
/// lunghezze dichiarate, contenimento nella regione dati e non
/// sovrapposizione con gli altri blocchi, poi valida il messaggio che ci
/// trova. Lo schema dentro il footer e' percorso come quello di un messaggio.
///
/// # Errors
///
/// `IpcTruncated` per magic o trailer non validi, `IpcFooterInvalid` per
/// blocchi incoerenti, `IpcMetadataTooLarge` / `IpcBodyTooLarge` /
/// `IpcTooManyMessages` al superamento dei limiti, `Io` sugli errori di
/// lettura.
pub fn validate_ipc_file_framing<S: IpcSource + ?Sized>(
    source: &mut S,
    limits: &IpcLimits,
) -> Result<(), ArrowTransportError> {
    let total = source.total_len();
    if total < ARROW_FILE_HEADER_BYTES + ARROW_FILE_TRAILER_BYTES {
        return Err(ArrowTransportError::IpcTruncated);
    }
    let mut scratch = Vec::new();
    source.read_at(0, ARROW_FILE_MAGIC.len(), &mut scratch)?;
    if scratch.as_slice() != ARROW_FILE_MAGIC.as_slice() {
        return Err(ArrowTransportError::IpcTruncated);
    }
    let trailer_start = total
        .checked_sub(ARROW_FILE_TRAILER_BYTES)
        .ok_or(ArrowTransportError::IpcTruncated)?;
    source.read_at(
        trailer_start,
        usize::try_from(ARROW_FILE_TRAILER_BYTES).unwrap_or(usize::MAX),
        &mut scratch,
    )?;
    if scratch.get(4..) != Some(ARROW_FILE_MAGIC.as_slice()) {
        return Err(ArrowTransportError::IpcTruncated);
    }
    let footer_len = i32::from_le_bytes(
        scratch
            .get(..4)
            .and_then(|slice| <[u8; 4]>::try_from(slice).ok())
            .ok_or(ArrowTransportError::IpcTruncated)?,
    );
    let footer_len = u64::try_from(footer_len).map_err(|_| ArrowTransportError::IpcTruncated)?;
    if footer_len == 0 {
        return Err(ArrowTransportError::IpcTruncated);
    }
    // Come nello stream: prima si verifica che il footer dichiarato esista
    // davvero nel file, poi lo si confronta con il tetto. Un footer che
    // sfora l'inizio del file e' un file rotto, non un file troppo grande.
    let footer_start = trailer_start
        .checked_sub(footer_len)
        .ok_or(ArrowTransportError::IpcTruncated)?;
    if footer_start < ARROW_FILE_HEADER_BYTES {
        return Err(ArrowTransportError::IpcTruncated);
    }
    if footer_len > to_u64(limits.max_metadata_bytes)? {
        return Err(ArrowTransportError::IpcMetadataTooLarge(
            usize::try_from(footer_len).unwrap_or(usize::MAX),
            limits.max_metadata_bytes,
        ));
    }

    // Il footer entro il tetto sui metadati: e' quanto arrow allochera' per
    // leggerlo, e ora anche quanto alloca questa validazione.
    let mut footer = Vec::new();
    source.read_at(
        footer_start,
        usize::try_from(footer_len).map_err(|_| ArrowTransportError::IpcTruncated)?,
        &mut footer,
    )?;
    let blocks = parse_footer(&footer, limits)?;
    validate_footer_blocks(source, &blocks, footer_start, limits)
}

/// Percorre il footer: lo Schema (che `fb_to_schema` leggera') e i vettori di
/// `Block` dei dizionari e dei record batch.
fn parse_footer(
    footer: &[u8],
    limits: &IpcLimits,
) -> Result<Vec<FooterBlock>, ArrowTransportError> {
    let root = fb_u32(footer, 0)? as usize;
    let (vtable, vtable_len) = fb_table(footer, root)?;
    // Campo 1: lo Schema del footer.
    let schema_offset = fb_field(footer, vtable, vtable_len, 1)?;
    if schema_offset != 0 {
        fb_schema(footer, fb_indirect(footer, root, schema_offset)?)?;
    }
    let mut blocks: Vec<FooterBlock> = Vec::new();
    // Campo 2: dizionari. Campo 3: record batch. Arrow legge entrambi.
    for field in [2_usize, 3] {
        fb_footer_blocks(footer, root, vtable, vtable_len, field, &mut blocks, limits)?;
    }
    Ok(blocks)
}

/// Verifica i blocchi del footer: contenimento nella regione dati,
/// allineamento, tetti e messaggio effettivamente presente all'offset; poi la
/// non sovrapposizione fra blocchi.
fn validate_footer_blocks<S: IpcSource + ?Sized>(
    source: &mut S,
    blocks: &[FooterBlock],
    footer_start: u64,
    limits: &IpcLimits,
) -> Result<(), ArrowTransportError> {
    let mut record_batches = 0_usize;
    for block in blocks {
        if block.offset < ARROW_FILE_HEADER_BYTES {
            return Err(ArrowTransportError::IpcFooterInvalid(
                "blocco prima della regione dati",
            ));
        }
        if !block.offset.is_multiple_of(8) {
            return Err(ArrowTransportError::IpcFooterInvalid(
                "blocco non allineato a 8 byte",
            ));
        }
        if block.end()? > footer_start {
            return Err(ArrowTransportError::IpcFooterInvalid(
                "blocco oltre la regione dati",
            ));
        }
        // Il tetto sui metadati si applica alla lunghezza del BLOCCO, perche'
        // e' quella che arrow legge e alloca. Limitare solo il prefisso
        // lasciava fuori proprio il numero usato.
        if block.metadata_len > to_u64(limits.max_metadata_bytes)? {
            return Err(ArrowTransportError::IpcMetadataTooLarge(
                usize::try_from(block.metadata_len).unwrap_or(usize::MAX),
                limits.max_metadata_bytes,
            ));
        }
        if block.body_len > limits.max_body_bytes {
            return Err(ArrowTransportError::IpcBodyTooLarge {
                declared: block.body_len,
                limit: limits.max_body_bytes,
            });
        }
        // Il tetto semantico sui RECORD BATCH vale anche qui. Nel file
        // format i blocchi di dizionari e di record batch confluiscono in un
        // vettore solo, e fermarsi a `max_messages` non applicava
        // `max_batches` del piano: un file con cento batch e un piano che ne
        // ammette uno superava il confine, e veniva fermato solo dopo la
        // materializzazione del secondo. Si conta per TIPO DI HEADER letto
        // dal messaggio, non per campo del footer: e' lo stesso criterio del
        // percorso stream, quindi i due non possono divergere.
        if validate_footer_block(source, *block, footer_start, limits)? == IPC_HEADER_RECORD_BATCH {
            record_batches = record_batches.saturating_add(1);
            if record_batches > limits.max_record_batches {
                return Err(ArrowTransportError::IpcTooManyRecordBatches(
                    record_batches,
                    limits.max_record_batches,
                ));
            }
        }
    }

    // Blocchi sovrapposti: arrow leggerebbe la stessa regione come due
    // messaggi diversi. Nessun produttore onesto li emette, e accettarli
    // significherebbe validare una regione con un'interpretazione e lasciarla
    // usare con un'altra.
    let mut ordered: Vec<(u64, u64)> = blocks
        .iter()
        .map(|block| block.end().map(|end| (block.offset, end)))
        .collect::<Result<_, _>>()?;
    ordered.sort_unstable();
    for pair in ordered.windows(2) {
        let [(_, previous_end), (next_start, _)] = pair else {
            continue;
        };
        if next_start < previous_end {
            return Err(ArrowTransportError::IpcFooterInvalid("blocchi sovrapposti"));
        }
    }
    Ok(())
}

/// Valida il messaggio incapsulato che il blocco dichiara.
fn validate_footer_block<S: IpcSource + ?Sized>(
    source: &mut S,
    block: FooterBlock,
    footer_start: u64,
    limits: &IpcLimits,
) -> Result<u8, ArrowTransportError> {
    let mut scratch = Vec::new();
    source.read_at(block.offset, 4, &mut scratch)?;
    let prefix = le_u32(&scratch)?;
    let (metadata_len, header) = if prefix == CONTINUATION_MARKER {
        source.read_at(
            block
                .offset
                .checked_add(4)
                .ok_or(ArrowTransportError::IpcTruncated)?,
            4,
            &mut scratch,
        )?;
        (le_u32(&scratch)? as usize, 8_u64)
    } else {
        (prefix as usize, 4_u64)
    };
    if metadata_len == 0 {
        return Err(ArrowTransportError::IpcFooterInvalid(
            "blocco che punta a un marcatore di fine stream",
        ));
    }
    // Le due lunghezze devono COINCIDERE, non solo starci dentro.
    //
    // Arrow legge il blocco usando `metaDataLength` e `bodyLength` del Block,
    // NON le lunghezze del prefisso: con la relazione `<=` un file poteva
    // dichiarare un prefisso piccolo — che il validatore limitava — e un
    // `Block.metaDataLength` enorme, che arrow avrebbe letto e allocato. Il
    // tetto valeva quindi su un numero diverso da quello effettivamente usato.
    //
    // Per la specifica del formato incapsulato `metaDataLength` comprende il
    // prefisso e il padding a 8 byte, quindi l'uguaglianza esatta e'
    // `align8(prefisso + metadata_len)`.
    let declared = align8_u64(
        header
            .checked_add(to_u64(metadata_len)?)
            .ok_or(ArrowTransportError::IpcTruncated)?,
    );
    if declared != block.metadata_len {
        return Err(ArrowTransportError::IpcFooterInvalid(
            "metadati del messaggio diversi dalla lunghezza dichiarata dal blocco",
        ));
    }
    let (body_len, header_type) = validate_message_at(
        source,
        block.offset,
        header,
        metadata_len,
        footer_start,
        limits,
    )?;
    if body_len != block.body_len {
        return Err(ArrowTransportError::IpcFooterInvalid(
            "body del messaggio diverso dalla lunghezza dichiarata dal blocco",
        ));
    }
    Ok(header_type)
}

/// Decodifica il payload Arrow IPC applicando i limiti di risorse prima di
/// accumulare i batch.
///
/// # Errors
///
/// `ArrowTransportError::IpcTruncated` o `ArrowTransportError::Arrow` per
/// stream malformati, `ArrowTransportError::TooManyColumns` /
/// `TooManyBatches` / `TooManyRows` / `StreamTooLarge` al superamento dei
/// limiti di risorse.
pub fn decode_ipc(payload: &[u8]) -> Result<(SchemaRef, Vec<RecordBatch>), ArrowTransportError> {
    validate_ipc_framing(payload)?;
    // `arrow-ipc` 59.1.0 va in panico dentro `convert::fb_to_schema` su schemi
    // che il decoder FlatBuffer accetta: `fields` e' opzionale e viene scartato
    // con `unwrap()` (convert.rs:198), e la conversione dei tipi ha una
    // ventina fra `panic!` e `unimplemented!` sui valori di enum che non
    // riconosce. Ogni reader chiama quella funzione. Le API che la avvolgono
    // si chiamano `try_*` ma sono fallibili solo sul parsing esterno: appena
    // ottengono lo schema fanno `.map(fb_to_schema)`. Non esiste quindi un
    // percorso per leggere Arrow IPC che non possa abortire il processo su
    // input ostile.
    //
    // Segnalato a monte: apache/arrow-rs#10575. Questa barriera va rimossa
    // quando quella issue e' chiusa e il pin di arrow sale a una versione che
    // rende fallibile la conversione dello schema.
    //
    // Il confine e' qui perche' e' l'unico punto in cui `&[u8]` non fidati
    // entrano nel trasporto: catturare piu' in profondita' significherebbe
    // sparpagliare `catch_unwind` sui call site, piu' in alto significherebbe
    // avvolgere anche codice nostro, dove un panico e' un difetto da non
    // nascondere.
    //
    // Correttezza dell'unwind safety: il payload e' un `&[u8]` immutabile e
    // tutto lo stato costruito qui dentro viene scartato se il panico avviene,
    // perche' la funzione ritorna `Err` e non espone nulla di parzialmente
    // costruito. Nessun invariante osservabile puo' restare rotto.
    //
    // Nota sull'hook globale: questa barriera converte il panico in errore,
    // ma NON impedisce all'hook di processo di averlo gia' stampato su
    // stderr — l'hook di `std` corre prima dell'unwinding e pubblica il
    // payload tale e quale, cioe' potenzialmente dati della riga. Una
    // libreria non puo' sostituire l'hook di nascosto (romperebbe quello di
    // chi la ospita), quindi la politica e' esplicita e vive in
    // `plenora_core::panic_policy`: la CLI installa `Silent`, un embedder —
    // il binding PyO3 compreso — deve installare `Sanitized`. Chi non
    // installa nulla resta con l'hook di `std`: residuo dichiarato in
    // docs/errori-e-limiti.md.
    //
    // ATTENZIONE per chi legge in futuro: il fuzz target `arrow_transform` e'
    // in quarantena e resta rosso anche con questa barriera attiva. Non e' un
    // segno che non funzioni. `libfuzzer-sys` installa un hook che chiama
    // `std::process::abort()` prima che l'unwinding cominci (0.4.10,
    // src/lib.rs:92-95), apposta perche' un `catch_unwind` nel codice sotto
    // test non possa nascondere difetti al fuzzer. La barriera e' verificata
    // dal test `ipc_decode_converte_il_panico_di_arrow_in_errore` in
    // transport.rs, che e' l'unica copertura possibile.
    let esito = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        decode_ipc_unguarded(payload)
    }));
    match esito {
        Ok(risultato) => risultato,
        Err(panico) => Err(ArrowTransportError::ArrowPanic(
            descrivi_panico(&panico).to_owned(),
        )),
    }
}

/// Descrizione PUBBLICA e sanitizzata del payload di un panico.
///
/// Il testo di un panico non e' controllato da noi: un `assert_eq!` dentro
/// arrow — o dentro qualunque dipendenza — puo' includere nel messaggio i
/// VALORI che ha confrontato, cioe' dati della riga. Pubblicarlo in un errore
/// viola la regola «errori senza dati» del progetto e puo' esfiltrare
/// contenuto dell'input in un log.
///
/// Si riporta quindi solo la FORMA del payload, che e' una proprieta' del
/// panico e non del dato: utile a distinguere un panico con messaggio da uno
/// senza, inutile a chi volesse leggerne il contenuto.
#[must_use]
pub fn descrivi_panico(panico: &Box<dyn std::any::Any + Send>) -> &'static str {
    plenora_core::panic_policy::forma_payload(panico.as_ref())
}

/// Corpo storico di [`decode_ipc`], senza la rete di protezione.
fn decode_ipc_unguarded(
    payload: &[u8],
) -> Result<(SchemaRef, Vec<RecordBatch>), ArrowTransportError> {
    let reader = StreamReader::try_new(payload, None)
        .map_err(|error| ArrowTransportError::Arrow(error.to_string()))?;
    let schema = reader.schema();
    if schema.fields().len() > MAX_COLUMNS {
        return Err(ArrowTransportError::TooManyColumns(schema.fields().len()));
    }
    let mut batches = Vec::new();
    let mut rows = 0_u64;
    for batch in reader {
        let batch = batch.map_err(|error| ArrowTransportError::Arrow(error.to_string()))?;
        if batches.len() >= MAX_BATCHES {
            return Err(ArrowTransportError::TooManyBatches(batches.len() + 1));
        }
        rows = rows
            .checked_add(batch.num_rows() as u64)
            .ok_or(ArrowTransportError::StreamTooLarge)?;
        if rows > MAX_ROWS {
            return Err(ArrowTransportError::TooManyRows(rows));
        }
        batches.push(batch);
    }
    Ok((schema, batches))
}

/// Codifica i batch in un payload Arrow IPC stream entro i limiti di risorse.
///
/// # Errors
///
/// `ArrowTransportError::TooManyBatches` se i batch superano il limite,
/// `ArrowTransportError::Arrow` per errori di codifica IPC,
/// `ArrowTransportError::StreamTooLarge` se il payload supera
/// `MAX_STREAM_BYTES`.
pub fn encode_ipc(
    schema: &SchemaRef,
    batches: &[RecordBatch],
) -> Result<Vec<u8>, ArrowTransportError> {
    if batches.len() > MAX_BATCHES {
        return Err(ArrowTransportError::TooManyBatches(batches.len()));
    }
    let mut payload = Vec::new();
    {
        let mut writer = StreamWriter::try_new(&mut payload, schema)
            .map_err(|error| ArrowTransportError::Arrow(error.to_string()))?;
        for batch in batches {
            writer
                .write(batch)
                .map_err(|error| ArrowTransportError::Arrow(error.to_string()))?;
        }
        writer
            .finish()
            .map_err(|error| ArrowTransportError::Arrow(error.to_string()))?;
    }
    if payload.len() as u64 > MAX_STREAM_BYTES {
        return Err(ArrowTransportError::StreamTooLarge);
    }
    Ok(payload)
}

/// Codifica gli stessi batch come Arrow IPC **file**, il formato ammesso dai
/// consumer path-based (nessun envelope o unwrap privato necessario).
///
/// # Errors
///
/// Restituisce `TooManyBatches` o `StreamTooLarge` quando vengono superati i
/// limiti del trasporto; propaga come `Arrow` gli errori del writer IPC.
pub fn encode_ipc_file(
    schema: &SchemaRef,
    batches: &[RecordBatch],
) -> Result<Vec<u8>, ArrowTransportError> {
    if batches.len() > MAX_BATCHES {
        return Err(ArrowTransportError::TooManyBatches(batches.len()));
    }
    let mut payload = Vec::new();
    {
        let mut writer = FileWriter::try_new(&mut payload, schema)
            .map_err(|error| ArrowTransportError::Arrow(error.to_string()))?;
        for batch in batches {
            writer
                .write(batch)
                .map_err(|error| ArrowTransportError::Arrow(error.to_string()))?;
        }
        writer
            .finish()
            .map_err(|error| ArrowTransportError::Arrow(error.to_string()))?;
    }
    if payload.len() as u64 > MAX_STREAM_BYTES {
        return Err(ArrowTransportError::StreamTooLarge);
    }
    Ok(payload)
}
