//! Payload Arrow IPC del trasporto v3: pre-validazione strutturale del
//! framing e dei metadati flatbuffer, decodifica e codifica entro i limiti
//! di risorse.

use std::collections::BTreeSet;
use std::io::{Read, Seek, SeekFrom};

use plenora_core::arrow::array::RecordBatch;
use plenora_core::arrow::ipc::reader::StreamReader;
use plenora_core::arrow::ipc::writer::{FileWriter, StreamWriter};
use plenora_core::arrow::schema::SchemaRef;

use super::error::ArrowTransportError;
use super::protocol::{MAX_ROWS, MAX_STREAM_BYTES};
use super::transport::{
    MAX_BATCHES, MAX_COLUMNS, MAX_IPC_CUSTOM_METADATA_KEY_BYTES, MAX_IPC_CUSTOM_METADATA_PAIRS,
    MAX_IPC_CUSTOM_METADATA_VALUE_BYTES, MAX_IPC_METADATA_BYTES,
};

/// Allineamento a 8 byte degli offset del framing IPC, su 64 bit: un file
/// puo' superare `usize` su piattaforme a 32 bit e gli offset non vanno mai
/// troncati.
///
/// Fallisce in overflow invece di saturare: vicino a `u64::MAX` una somma
/// saturata e mascherata tornerebbe **sotto** il valore di partenza e
/// romperebbe in silenzio la monotonicita' del parsing. Un offset non
/// allineabile e' un errore di framing esplicito.
const fn align8_u64(value: u64) -> Option<u64> {
    match value.checked_add(7) {
        Some(somma) => Some(somma & !7),
        None => None,
    }
}

/// Prefisso di continuazione dei messaggi IPC incapsulati.
const CONTINUATION_MARKER: u32 = 0xFFFF_FFFF;

/// Valore di `MessageHeader` per un `RecordBatch` (union dello standard IPC).
const IPC_HEADER_RECORD_BATCH: u8 = 3;

/// Header di un `DictionaryBatch`.
const IPC_HEADER_DICTIONARY_BATCH: u8 = 2;

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
// arrow-format alloca `Vec::with_capacity(count)` per i vettori dichiarati
// nei metadati senza un tetto proprio (OOM). Questo validatore verifica che
// ogni vettore, stringa e buffer di `Message`/`Schema`/`RecordBatch` stia
// nei byte disponibili prima che arrow-rs veda i metadati. Copre solo la
// struttura che puo' allocare.

const MAX_FLATBUFFER_DEPTH: usize = 64;

/// Nodi totali (campi, figli compresi) ammessi in uno Schema IPC.
///
/// `MAX_COLUMNS` limita i soli campi di primo livello; questo tetto copre i
/// vettori `children`, che un `FlatBuffer` costruito a mano puo' far puntare
/// allo stesso sottoalbero con crescita esponenziale.
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

/// Somma di posizioni dentro il buffer.
///
/// Gli addendi arrivano dal file: un traboccamento e' un riferimento
/// malformato. Il controllo resta locale anche dove il chiamante ha gia'
/// provato il confine.
fn fb_somma(a: usize, b: usize) -> Result<usize, ArrowTransportError> {
    a.checked_add(b).ok_or(ArrowTransportError::IpcTruncated)
}

/// Prodotto di un indice per la dimensione di un elemento.
///
/// Stessa ragione della somma: il conteggio degli elementi arriva dal file.
fn fb_prodotto(indice: usize, dimensione: usize) -> Result<usize, ArrowTransportError> {
    indice
        .checked_mul(dimensione)
        .ok_or(ArrowTransportError::IpcTruncated)
}

/// Le quattro letture little-endian dal buffer flatbuffer.
///
/// Ogni posizione fuori dal buffer e' un troncamento. La fine dell'intervallo
/// passa da [`fb_somma`] perche' `pos` arriva dal file e puo' traboccare.
macro_rules! lettura_le {
    ($nome:ident, $tipo:ty, $byte:literal) => {
        fn $nome(buf: &[u8], pos: usize) -> Result<$tipo, ArrowTransportError> {
            let fine = fb_somma(pos, $byte)?;
            buf.get(pos..fine)
                .and_then(|bytes| <[u8; $byte]>::try_from(bytes).ok())
                .map(<$tipo>::from_le_bytes)
                .ok_or(ArrowTransportError::IpcTruncated)
        }
    };
}

lettura_le!(fb_u16, u16, 2);
lettura_le!(fb_u32, u32, 4);
lettura_le!(fb_i32, i32, 4);
lettura_le!(fb_i64, i64, 8);

/// Tabella flatbuffer in `pos`: ritorna (`vtable_start`, `vtable_len`).
/// A `pos` c'e' l'`soffset` (i32, distanza alla vtable); `vtable_len` e
/// `table_len` stanno nella vtable stessa. L'`soffset` puo' essere NEGATIVO:
/// con vtable deduplicate il writer puo' piazzare la vtable dopo la tabella.
fn fb_table(buf: &[u8], pos: usize) -> Result<(usize, usize), ArrowTransportError> {
    let soffset = fb_i32(buf, pos)?;
    if soffset == 0 {
        return Err(ArrowTransportError::IpcTruncated);
    }
    // Conversioni totali: un offset che non entra in i64/usize e' un
    // riferimento malformato, mai un troncamento silenzioso (R5.4).
    // `checked_sub` perche' un `soffset` negativo — le vtable deduplicate
    // stanno dopo la tabella — equivale a una somma.
    let vtable_signed = i64::try_from(pos)
        .map_err(|_| ArrowTransportError::IpcTruncated)?
        .checked_sub(i64::from(soffset))
        .ok_or(ArrowTransportError::IpcTruncated)?;
    let vtable = usize::try_from(vtable_signed).map_err(|_| ArrowTransportError::IpcTruncated)?;
    let vtable_len = fb_u16(buf, vtable)? as usize;
    let table_len = fb_u16(buf, fb_somma(vtable, 2)?)? as usize;
    if vtable_len < 4
        || !vtable_len.is_multiple_of(2)
        || fb_somma(vtable, vtable_len)? > buf.len()
        || fb_somma(pos, table_len)? > buf.len()
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
    let entry = index
        .checked_mul(2)
        .and_then(|doppio| doppio.checked_add(4))
        .ok_or(ArrowTransportError::IpcTruncated)?;
    if fb_somma(entry, 2)? > vtable_len {
        return Ok(0);
    }
    Ok(fb_u16(buf, fb_somma(vtable, entry)?)? as usize)
}

/// Posizione assoluta di un campo indiretto (tabella, vettore, stringa).
///
/// La somma con `relative` resta controllata: a 64 bit non trabocca, ma a
/// 32 bit e' l'unico controllo che separa i due addendi dall'overflow.
fn fb_indirect(buf: &[u8], table: usize, offset: usize) -> Result<usize, ArrowTransportError> {
    let campo = fb_somma(table, offset)?;
    let relative = fb_u32(buf, campo)? as usize;
    let target = fb_somma(campo, relative)?;
    if fb_somma(target, 4)? > buf.len() {
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
    if fb_somma(pos, bytes)? > buf.len() {
        return Err(ArrowTransportError::IpcTruncated);
    }
    Ok(count)
}

/// Stringa flatbuffer (vettore di byte con terminatore), e i suoi byte.
///
/// Un solo posto per il controllo dei confini, sia per chi valida soltanto
/// sia per chi legge il contenuto. L'intervallo resta controllato anche se
/// [`fb_vector`] l'ha gia' provato.
fn fb_string(buf: &[u8], pos: usize) -> Result<&[u8], ArrowTransportError> {
    let count = fb_vector(buf, pos, 1)?;
    let inizio = fb_somma(pos, 4)?;
    let fine = fb_somma(inizio, count)?;
    if fine >= buf.len() {
        return Err(ArrowTransportError::IpcTruncated);
    }
    buf.get(inizio..fine)
        .ok_or(ArrowTransportError::IpcTruncated)
}

/// Valida UNA coppia di custom metadata e **restituisce** chiave e valore.
///
/// Restituisce le stringhe perche' i duplicati sono una proprieta'
/// dell'insieme, controllata in [`fb_custom_metadata`].
///
/// Chiave e valore **assenti** sono un rifiuto: `arrow-ipc` legge i custom
/// metadata del footer con `key().unwrap()` e `value().unwrap()`, quindi una
/// voce incompleta arriverebbe a un panic dentro la dipendenza.
fn fb_key_value(buf: &[u8], table: usize) -> Result<(&str, &str), ArrowTransportError> {
    /// Che cosa pretendere da uno dei due campi di una coppia.
    ///
    /// Chiave e valore hanno tetti e diagnosi diversi.
    struct Attesa {
        indice: usize,
        limite: usize,
        assente: &'static str,
        non_utf8: &'static str,
        troppo_lungo: fn(usize, usize) -> ArrowTransportError,
    }

    const CHIAVE: Attesa = Attesa {
        indice: 0,
        limite: MAX_IPC_CUSTOM_METADATA_KEY_BYTES,
        assente: "chiave assente",
        non_utf8: "chiave non e' UTF-8 valido",
        troppo_lungo: ArrowTransportError::IpcMetadataKeyTooLarge,
    };
    const VALORE: Attesa = Attesa {
        indice: 1,
        limite: MAX_IPC_CUSTOM_METADATA_VALUE_BYTES,
        assente: "valore assente",
        non_utf8: "valore non e' UTF-8 valido",
        troppo_lungo: ArrowTransportError::IpcMetadataValueTooLarge,
    };

    fn campo<'a>(
        buf: &'a [u8],
        table: usize,
        vtable: usize,
        vtable_len: usize,
        attesa: &Attesa,
    ) -> Result<&'a str, ArrowTransportError> {
        let offset = fb_field(buf, vtable, vtable_len, attesa.indice)?;
        if offset == 0 {
            return Err(ArrowTransportError::IpcMetadataInvalid(attesa.assente));
        }
        let bytes = fb_string(buf, fb_indirect(buf, table, offset)?)?;
        // Il tetto PRIMA della validazione UTF-8: e' il controllo piu' a buon
        // mercato, e non ha senso convalidare byte che rifiuteremo comunque.
        if bytes.len() > attesa.limite {
            return Err((attesa.troppo_lungo)(bytes.len(), attesa.limite));
        }
        // UTF-8 verificato QUI: `fb_string` guarda i confini, non il
        // contenuto, e gli accessori flatbuffer non lo garantiscono.
        std::str::from_utf8(bytes)
            .map_err(|_| ArrowTransportError::IpcMetadataInvalid(attesa.non_utf8))
    }

    let (vtable, vtable_len) = fb_table(buf, table)?;
    let chiave = campo(buf, table, vtable, vtable_len, &CHIAVE)?;
    let valore = campo(buf, table, vtable, vtable_len, &VALORE)?;
    // Chiave vuota rifiutata: non ha significato, e piu' chiavi vuote sono
    // duplicati per costruzione. Il VALORE vuoto e' invece accettato —
    // rifiutarlo romperebbe file legittimi che rappresentano un campo assente
    // con la stringa vuota.
    if chiave.is_empty() {
        return Err(ArrowTransportError::IpcMetadataInvalid("chiave vuota"));
    }
    Ok((chiave, valore))
}

/// Valida una collezione di custom metadata: conteggio, forma di ogni coppia,
/// unicita' delle chiavi.
///
/// Il tetto sul conteggio si applica prima di qualunque allocazione
/// proporzionale. Le chiavi sconosciute sono accettate: si valida la
/// **forma**, non il vocabolario, per non rompere l'interoperabilita'.
fn fb_custom_metadata(buf: &[u8], table: usize, offset: usize) -> Result<(), ArrowTransportError> {
    fb_custom_metadata_estraendo(buf, table, offset, None).map(|_| ())
}

/// Come [`fb_custom_metadata`], ma **rende** il valore di una chiave cercata.
///
/// L'estrazione passa per la stessa traversata che convalida: per questo il
/// `commit_token` non si legge da `FileReader::custom_metadata`, che non fa
/// questi controlli.
fn fb_custom_metadata_estraendo<'a>(
    buf: &'a [u8],
    table: usize,
    offset: usize,
    cercata: Option<&str>,
) -> Result<Option<&'a str>, ArrowTransportError> {
    if offset == 0 {
        return Ok(None);
    }
    let vector = fb_indirect(buf, table, offset)?;
    let count = fb_vector(buf, vector, 4)?;
    // Prima di allocare: il conteggio si legge dal vettore e si confronta col
    // tetto senza costruire niente.
    if count > MAX_IPC_CUSTOM_METADATA_PAIRS {
        return Err(ArrowTransportError::IpcTooManyMetadataPairs(
            count,
            MAX_IPC_CUSTOM_METADATA_PAIRS,
        ));
    }
    let mut viste: BTreeSet<&str> = BTreeSet::new();
    let mut trovato: Option<&str> = None;
    for index in 0..count {
        let entry = fb_indirect(buf, fb_somma(vector, 4)?, fb_prodotto(index, 4)?)?;
        let (chiave, valore) = fb_key_value(buf, entry)?;
        // Duplicati rifiutati, non risolti. Chi li raccoglie in una mappa
        // applica «vince l'ultima», che per una chiave autoritativa sceglie
        // un vincitore arbitrario: qui non c'e' nulla da scegliere.
        if !viste.insert(chiave) {
            return Err(ArrowTransportError::IpcMetadataInvalid("chiave duplicata"));
        }
        if cercata == Some(chiave) {
            trovato = Some(valore);
        }
    }
    Ok(trovato)
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
                .get(fb_somma(table, type_type_offset)?)
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
        // `indexType` e' obbligatorio quando `dictionary` c'e':
        // `get_data_type` lo legge con `dictionary.indexType().unwrap()`, e
        // una codifica a dizionario senza tipo dell'indice non significa
        // comunque nulla.
        let index_type = fb_field(buf, dict_vtable, dict_vtable_len, 1)?;
        if index_type == 0 {
            return Err(ArrowTransportError::IpcSchemaInvalid(
                "dictionary senza indexType",
            ));
        }
        fb_table(buf, fb_indirect(buf, dictionary_table, index_type)?)?;
    }
    // children: vettore di Field.
    let children = fb_field(buf, vtable, vtable_len, 5)?;
    if children != 0 {
        let vector = fb_indirect(buf, table, children)?;
        let count = fb_vector(buf, vector, 4)?;
        for index in 0..count {
            let child = fb_indirect(buf, fb_somma(vector, 4)?, fb_prodotto(index, 4)?)?;
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
            let entry = fb_somma(fb_somma(vector, 4)?, fb_prodotto(index, 16)?)?;
            let buffer_offset = fb_i64(buf, entry)?;
            let length = fb_i64(buf, fb_somma(entry, 8)?)?;
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
        // Con `bodyCompression` arrow alloca le lunghezze decompresse
        // dichiarate dentro il body, che la pre-validazione non legge: un
        // tetto sul body compresso non limiterebbe nulla. Si rifiuta la
        // classe intera; i writer del progetto non comprimono.
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
    // `fields` e' OBBLIGATORIO: `fb_to_schema` lo legge con
    // `fb.fields().unwrap()`. Il writer lo emette sempre, anche per uno
    // schema senza colonne — presente con zero elementi e assente sono cose
    // diverse, e solo la seconda panica.
    let fields = fb_field(buf, vtable, vtable_len, 1)?;
    if fields == 0 {
        return Err(ArrowTransportError::IpcSchemaInvalid(
            "schema senza il campo fields",
        ));
    }
    {
        let vector = fb_indirect(buf, table, fields)?;
        let count = fb_vector(buf, vector, 4)?;
        if count > MAX_COLUMNS {
            return Err(ArrowTransportError::TooManyColumns(count));
        }
        // Budget CUMULATIVO sull'intero schema: i figli annidati consumano lo
        // stesso conto dei campi di primo livello.
        let mut budget = SchemaBudget::new();
        for index in 0..count {
            let field = fb_indirect(buf, fb_somma(vector, 4)?, fb_prodotto(index, 4)?)?;
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
/// del body dichiarata (`bodyLength`).
///
/// I tipi di header oltre il terzo — `Tensor`, `SparseTensor` e i valori che il
/// formato non ha ancora assegnato — sono rifiutati: il trasporto non li usa e
/// nessun produttore onesto li emette. `MessageHeader::NONE` senza header e'
/// invece un messaggio vuoto, e passa.
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
            .get(fb_somma(table, header_type_offset)?)
            .ok_or(ArrowTransportError::IpcTruncated)?
    };
    let header_offset = fb_field(metadata, vtable, vtable_len, 2)?;
    let header_table = if header_offset == 0 {
        None
    } else {
        Some(fb_indirect(metadata, table, header_offset)?)
    };
    // `bodyLength` si legge PRIMA di validare l'header: e' il solo metro per
    // i buffer, sia di un RecordBatch sia di quello interno a un
    // DictionaryBatch (`metadata.len()` non ha rapporto con il body).
    let body_len_offset = fb_field(metadata, vtable, vtable_len, 3)?;
    let body_len = if body_len_offset == 0 {
        0
    } else {
        let value = fb_i64(metadata, fb_somma(table, body_len_offset)?)?;
        if value < 0 {
            return Err(ArrowTransportError::IpcTruncated);
        }
        usize::try_from(value).map_err(|_| ArrowTransportError::IpcTruncated)?
    };

    // Il tipo e la presenza dell'header si decidono INSIEME, in un match
    // solo: ogni combinazione ha il suo ramo e nessuna passa in mezzo. Un
    // header assente arriverebbe ad arrow, che lo legge con `unwrap()`
    // (`header_as_schema()`, `header_as_dictionary_batch()`,
    // `header_as_record_batch()`).
    match (header_type, header_table) {
        (1, Some(header_table)) => {
            // Un messaggio Schema non ha corpo; uno dichiarato verrebbe
            // allocato da `StreamReader::try_new` prima di guardare il tipo.
            if body_len != 0 {
                return Err(ArrowTransportError::IpcSchemaInvalid(
                    "un messaggio Schema dichiara un corpo",
                ));
            }
            fb_schema(metadata, header_table)?;
        }
        (2, Some(header_table)) => {
            // DictionaryBatch: id al campo 0, data (RecordBatch) al campo
            // 1, isDelta al campo 2.
            let (dict_vtable, dict_vtable_len) = fb_table(metadata, header_table)?;

            // DEVIAZIONE dal formato: i dizionari delta si rifiutano. Arrow
            // concatena precedente e nuovo in un buffer ulteriore, il picco
            // si avvicina al doppio della somma dei `bodyLength` e la formula
            // della memoria trattenuta (`verifica.rs`,
            // isolamento.md#2-ter-la-verifica-non-può-stare-fuori-dal-limite)
            // diventerebbe falsa. Nessun nostro produttore li emette.
            // Rientro: rifare il tetto sul picco della concatenazione, non
            // sulla somma dei body.
            let is_delta = fb_field(metadata, dict_vtable, dict_vtable_len, 2)?;
            if is_delta != 0 {
                let posizione = fb_somma(header_table, is_delta)?;
                let valore = *metadata
                    .get(posizione)
                    .ok_or(ArrowTransportError::IpcTruncated)?;
                if valore != 0 {
                    return Err(ArrowTransportError::IpcSchemaInvalid(
                        "dictionary delta non supportato",
                    ));
                }
            }

            // `data` e' obbligatorio: `read_dictionary` lo legge con
            // `batch.data().unwrap()`.
            let data = fb_field(metadata, dict_vtable, dict_vtable_len, 1)?;
            if data == 0 {
                return Err(ArrowTransportError::IpcSchemaInvalid(
                    "DictionaryBatch senza data",
                ));
            }
            let batch = fb_indirect(metadata, header_table, data)?;
            fb_record_batch(metadata, batch, body_len)?;
        }
        (3, Some(header_table)) => fb_record_batch(metadata, header_table, body_len)?,
        // `MessageHeader::NONE`: un messaggio senza contenuto, legittimo per
        // il formato, che arrow attraversa senza fare niente.
        (0, None) => {}
        // NONE con un header: una dichiarazione che contraddice se stessa.
        (0, Some(_)) => {
            return Err(ArrowTransportError::IpcSchemaInvalid(
                "messaggio IPC di tipo NONE con un header",
            ))
        }
        // Il tipo si rifiuta PRIMA di guardare l'header, cosi' la diagnosi
        // nomina la causa vera. Il messaggio non nomina Tensor e SparseTensor
        // perche' il ramo prende anche i valori non assegnati dal formato.
        (4.., _) => {
            return Err(ArrowTransportError::Arrow(
                "tipo di header IPC non supportato".to_owned(),
            ))
        }
        (1..=3, None) => {
            return Err(ArrowTransportError::IpcSchemaInvalid(
                "messaggio IPC senza header",
            ))
        }
    }

    let custom = fb_field(metadata, vtable, vtable_len, 4)?;
    fb_custom_metadata(metadata, table, custom)?;
    Ok((body_len, header_type))
}

// --- Sorgente di byte per la pre-validazione del framing -------------------

/// Sorgente di byte su cui gira la pre-validazione del framing IPC.
///
/// Serve sia il payload gia' in memoria sia i file, che NON vanno caricati
/// per intero: si materializzano solo i metadati di ogni messaggio (tetto
/// [`MAX_IPC_METADATA_BYTES`], verificato PRIMA della lettura) e il body si
/// salta per offset.
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

    /// Il lettore, per chi deve duplicarne il descrittore.
    ///
    /// Non lo consuma e non lo sposta: chi lo prende puo' solo guardarlo, e la
    /// posizione resta di questa sorgente.
    pub const fn lettore(&self) -> &R {
        &self.reader
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
/// `max_batch_bytes` del piano misura un `RecordBatch` gia' materializzato;
/// questi si applicano sulle lunghezze DICHIARATE, prima dell'allocazione.
#[derive(Debug, Clone, Copy)]
// `non_exhaustive`: da fuori dal crate si parte da `IpcLimits::default()` e
// si assegnano i campi, cosi' aggiungere un limite non rompe l'API.
// Vedi errori-e-limiti.md#il-tetto-cumulativo-sui-dizionari.
#[non_exhaustive]
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
    /// Distinto da `max_record_batches`: uno stream con un batch contiene
    /// anche lo schema ed eventuali `DictionaryBatch`.
    pub max_messages: usize,
    /// Tetto sulla **somma** dei body dei dizionari.
    ///
    /// I dizionari sono l'unica cosa che il lettore trattiene tutta insieme
    /// (`FileReader` li decodifica in `try_new`, `StreamReader` li accumula),
    /// quindi il tetto per singolo body non li governa. Sta fra i limiti del
    /// confine per proteggere tutti i lettori, non un percorso solo.
    pub max_retained_dictionary_body_bytes: u64,
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
            max_retained_dictionary_body_bytes: MAX_RETAINED_DICTIONARY_BODY_BYTES,
        }
    }
}

impl IpcLimits {
    /// Profilo del payload del trasporto v3.
    ///
    /// Il trasporto ha limiti propri (lunghezza nell'envelope,
    /// `MAX_CELL_BYTES`): il profilo stretto trasformerebbe un `CellTooLarge`
    /// in un generico "body troppo grande". Il tetto vero e'
    /// `MAX_STREAM_BYTES`.
    #[must_use]
    pub const fn transport() -> Self {
        Self {
            max_metadata_bytes: MAX_IPC_METADATA_BYTES,
            max_body_bytes: MAX_STREAM_BYTES,
            max_record_batches: MAX_BATCHES,
            max_messages: MAX_TOTAL_IPC_MESSAGES,
            // Il profilo del trasporto alza `max_body_bytes` a
            // `MAX_STREAM_BYTES` per ragioni sue, ma i dizionari non c'entrano
            // con quella scelta: restano al massimale.
            max_retained_dictionary_body_bytes: MAX_RETAINED_DICTIONARY_BODY_BYTES,
        }
    }
}

/// Default del tetto sul body: stesso valore di `BatchTarget::max_batch_bytes`
/// (64 MiB), che e' il limite con cui l'executor misura il batch risultante.
pub const DEFAULT_MAX_BODY_BYTES: u64 = 64 * 1024 * 1024;

/// Massimale assoluto sulla somma dei body dei dizionari trattenuti.
///
/// Stesso ordine di grandezza di un singolo body, non un suo multiplo: i
/// dizionari restano vivi tutti insieme, i record batch uno per volta.
pub const MAX_RETAINED_DICTIONARY_BODY_BYTES: u64 = 64 * 1024 * 1024;

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
    // La somma dei body dei dizionari incontrati (vedi
    // `IpcLimits::max_retained_dictionary_body_bytes`). Sta nella traversata
    // comune a stream e file perche' la attraversano tutti i lettori.
    let mut dizionari_trattenuti = 0_u64;
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
            // Fine stream: il marcatore deve CHIUDERE la regione, altrimenti
            // byte non validati dopo l'EOS passerebbero (smuggling).
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
        if header_type == IPC_HEADER_DICTIONARY_BATCH {
            // Somma controllata: il trabocco e' un rifiuto, mai una
            // saturazione. `IpcTruncated` e non `IpcFooterInvalid` perche'
            // qui si percorre la regione dei messaggi, e lo stream non ha
            // footer.
            dizionari_trattenuti = dizionari_trattenuti
                .checked_add(body_len)
                .ok_or(ArrowTransportError::IpcTruncated)?;
            let tetto = limits.max_retained_dictionary_body_bytes;
            if dizionari_trattenuti > tetto {
                return Err(ArrowTransportError::IpcRetainedDictionariesTooLarge {
                    declared: dizionari_trattenuti,
                    limit: tetto,
                });
            }
        }
        let metadata_bytes = to_u64(metadata_len)?;
        let metadata_end = offset
            .checked_add(header)
            .and_then(|start| start.checked_add(metadata_bytes))
            .ok_or(ArrowTransportError::IpcTruncated)?;
        let end = align8_u64(metadata_end)
            .and_then(|allineato| allineato.checked_add(body_len))
            .and_then(align8_u64)
            .ok_or(ArrowTransportError::IpcTruncated)?;
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
    // Una lunghezza dichiarata oltre la sorgente descrive un file ROTTO, non
    // troppo grande: la disponibilita' si verifica prima del tetto, perche'
    // la spazzatura non esca come `resource_limit`. E' pura aritmetica su
    // `end_limit`: nessun byte e' letto prima del tetto.
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
    let fine = align8_u64(metadata_end)
        .and_then(|allineato| allineato.checked_add(body_len))
        .and_then(align8_u64)
        .ok_or(ArrowTransportError::IpcTruncated)?;
    if fine > end_limit {
        return Err(ArrowTransportError::IpcTruncated);
    }
    // Il tetto sul body si applica alla lunghezza DICHIARATA, prima che arrow
    // legga un byte del corpo.
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
        let entry = fb_somma(
            fb_somma(vector, 4)?,
            fb_prodotto(index, FOOTER_BLOCK_BYTES)?,
        )?;
        // Layout dello struct flatbuffer `Block`: offset (i64), poi
        // metaDataLength (i32) con quattro byte di padding, poi bodyLength
        // (i64) — 24 byte in tutto, allineati a 8.
        let block_offset = fb_i64(footer, entry)?;
        let metadata_len = fb_i32(footer, fb_somma(entry, 8)?)?;
        let body_len = fb_i64(footer, fb_somma(entry, 16)?)?;
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
/// `FileReader` salta direttamente agli `offset` dei blocchi del footer, quindi
/// una scansione sequenziale validerebbe la regione sbagliata. La validazione
/// segue la stessa mappa di arrow (magic, trailer, footer, blocchi) e per ogni
/// blocco verifica offset, allineamento, lunghezze, contenimento e non
/// sovrapposizione, poi il messaggio che ci trova.
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
    valida_file_ed_estrai(source, limits, None).map(|_| ())
}

/// Convalida il file **e** rende il valore di una chiave dei custom metadata
/// del footer.
///
/// Il valore esce dalla stessa traversata rinforzata di
/// [`validate_ipc_file_framing`]; `FileReader::custom_metadata` non lo e'.
///
/// Il tetto cumulativo sui dizionari di [`IpcLimits`] si applica **sempre**,
/// sui `bodyLength` dichiarati nei blocchi del footer, cioe' quelli che arrow
/// leggera' davvero, prima che ne decodifichi uno.
///
/// # Errors
///
/// Come [`validate_ipc_file_framing`];
/// [`ArrowTransportError::IpcRetainedDictionariesTooLarge`] se la somma supera
/// il tetto, [`ArrowTransportError::IpcFooterInvalid`] se la somma trabocca.
pub fn valida_file_ed_estrai<S: IpcSource + ?Sized>(
    source: &mut S,
    limits: &IpcLimits,
    chiave: Option<&str>,
) -> Result<Option<String>, ArrowTransportError> {
    valida_file(source, limits, chiave).map(|(trovato, _)| trovato)
}

/// Convalida il file **e** rende i byte del footer che la convalida ha letto.
///
/// Sono gli stessi byte che la traversata ha percorso: rileggerli dal file
/// riaprirebbe la finestra fra convalida e lettura, e un file cambiato sul
/// posto dichiarerebbe un footer che nessun tetto ha visto.
///
/// # Errors
///
/// Come [`validate_ipc_file_framing`].
pub fn valida_file_e_rendi_footer<S: IpcSource + ?Sized>(
    source: &mut S,
    limits: &IpcLimits,
) -> Result<Vec<u8>, ArrowTransportError> {
    valida_file(source, limits, None).map(|(_, footer)| footer)
}

/// Il corpo comune delle due convalide pubbliche del file format: il valore
/// della chiave richiesta, se c'e', e i byte del footer.
fn valida_file<S: IpcSource + ?Sized>(
    source: &mut S,
    limits: &IpcLimits,
    chiave: Option<&str>,
) -> Result<(Option<String>, Vec<u8>), ArrowTransportError> {
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
    let (blocks, dizionari, trovato) = parse_footer_estraendo(&footer, limits, chiave)?;
    // Il valore si copia **prima** di continuare: `footer` e' un buffer locale
    // e `trovato` lo presta.
    let trovato = trovato.map(str::to_owned);
    // **Prima si valida, poi si limita**: un file con blocchi incoerenti e'
    // rotto, e non deve uscire come `ResourceLimit`.
    validate_footer_blocks(source, &blocks, footer_start, limits)?;
    {
        let tetto = limits.max_retained_dictionary_body_bytes;
        // Nessun ripiego su una fetta vuota, che disattiverebbe il tetto
        // (fail-open). L'incoerenza e' impossibile per costruzione, quindi e'
        // un errore interno.
        let dizionari = blocks
            .get(..dizionari)
            .ok_or(ArrowTransportError::Internal(
                "indice dei blocchi dizionario oltre i blocchi del footer",
            ))?;
        verifica_tetto_dizionari(dizionari, tetto)?;
    }
    Ok((trovato, footer))
}

/// Percorre il footer: lo Schema (che `fb_to_schema` leggera') e i vettori di
/// `Block` dei dizionari e dei record batch.
///
/// **Solo sotto test**: la produzione passa per [`parse_footer_estraendo`].
#[cfg(test)]
fn parse_footer(
    footer: &[u8],
    limits: &IpcLimits,
) -> Result<Vec<FooterBlock>, ArrowTransportError> {
    parse_footer_estraendo(footer, limits, None).map(|(blocks, _, _)| blocks)
}

/// Percorre il footer — lo Schema e i vettori di `Block` — e rende anche il
/// numero di blocchi DIZIONARIO in testa a `blocks` e il valore della chiave
/// cercata fra i custom metadata del footer.
fn parse_footer_estraendo<'a>(
    footer: &'a [u8],
    limits: &IpcLimits,
    cercata: Option<&str>,
) -> Result<(Vec<FooterBlock>, usize, Option<&'a str>), ArrowTransportError> {
    let root = fb_u32(footer, 0)? as usize;
    let (vtable, vtable_len) = fb_table(footer, root)?;
    // Campo 1: lo Schema del footer, OBBLIGATORIO: `arrow-ipc` lo legge con
    // `footer.schema().unwrap()`.
    let schema_offset = fb_field(footer, vtable, vtable_len, 1)?;
    if schema_offset == 0 {
        return Err(ArrowTransportError::IpcFooterInvalid("schema assente"));
    }
    fb_schema(footer, fb_indirect(footer, root, schema_offset)?)?;
    let mut blocks: Vec<FooterBlock> = Vec::new();
    // Campo 2: dizionari. Campo 3: record batch. Arrow legge entrambi.
    //
    // Un vettore solo, con il confine conservato come indice: i dizionari
    // sono `blocks[..dizionari]`, soggetti al tetto cumulativo (§2-ter).
    fb_footer_blocks(footer, root, vtable, vtable_len, 2, &mut blocks, limits)?;
    let dizionari = blocks.len();
    fb_footer_blocks(footer, root, vtable, vtable_len, 3, &mut blocks, limits)?;
    // Campo 4: custom metadata del footer, che arrow legge con
    // `key().unwrap()` / `value().unwrap()`.
    let custom = fb_field(footer, vtable, vtable_len, 4)?;
    let trovato = fb_custom_metadata_estraendo(footer, root, custom, cercata)?;
    Ok((blocks, dizionari, trovato))
}

/// Somma controllata dei body dei blocchi dizionario, contro il tetto.
///
/// La somma e non il massimo, perche' `FileReader` trattiene tutti i
/// dizionari insieme. L'overflow e' un rifiuto con errore proprio: una somma
/// saturata non e' la somma, e non c'e' un numero onesto da riportare.
///
/// # Errors
///
/// - [`ArrowTransportError::IpcRetainedDictionariesTooLarge`] se la somma
///   supera il tetto: porta la somma **vera** e il tetto;
/// - [`ArrowTransportError::IpcFooterInvalid`] se la somma trabocca: nessun
///   numero, perche' non ce n'e' uno onesto.
fn verifica_tetto_dizionari(
    dizionari: &[FooterBlock],
    tetto: u64,
) -> Result<(), ArrowTransportError> {
    let mut somma = 0_u64;
    for block in dizionari {
        somma = somma
            .checked_add(block.body_len)
            .ok_or(ArrowTransportError::IpcFooterInvalid(
                "somma dei body dei blocchi dizionario fuori intervallo",
            ))?;
    }
    if somma > tetto {
        return Err(ArrowTransportError::IpcRetainedDictionariesTooLarge {
            declared: somma,
            limit: tetto,
        });
    }
    Ok(())
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
        // lascerebbe fuori proprio il numero usato.
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
        // Il tetto semantico sui RECORD BATCH vale anche qui, oltre a
        // `max_messages`. Si conta per TIPO DI HEADER letto dal messaggio,
        // lo stesso criterio del percorso stream.
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

    // Blocchi sovrapposti: la stessa regione, validata con
    // un'interpretazione, sarebbe letta con un'altra.
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
    // Le due lunghezze devono COINCIDERE: arrow usa `metaDataLength` e
    // `bodyLength` del Block, non quelle del prefisso, e il tetto deve valere
    // sul numero usato. `metaDataLength` comprende prefisso e padding, quindi
    // l'uguaglianza e' `align8(prefisso + metadata_len)`.
    let declared = header
        .checked_add(to_u64(metadata_len)?)
        .and_then(align8_u64)
        .ok_or(ArrowTransportError::IpcTruncated)?;
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
    // Barriera di dipendenza: `arrow-ipc` va in panico dentro
    // `convert::fb_to_schema` su schemi che il decoder FlatBuffer accetta, e
    // ogni reader la chiama. Rientro: rimuoverla quando apache/arrow-rs#10575
    // e' chiusa e il pin di arrow rende fallibile la conversione.
    //
    // Sta qui perche' e' l'unico ingresso di `&[u8]` non fidati; piu' in alto
    // nasconderebbe panici di codice nostro. Unwind safety: il payload e'
    // immutabile e lo stato parziale si scarta con l'`Err`.
    //
    // L'hook di processo stampa comunque il panico: la politica e' in
    // `plenora_core::panic_policy` (la CLI installa `Silent`, un embedder
    // `Sanitized`), residuo dichiarato in docs/errori-e-limiti.md. Il fuzz
    // target `arrow_transform` ne tollera il panico
    // (errori-e-limiti.md#panici-attesi-nel-fuzzing); la verifica e' il
    // modulo `barriera_antipanico` in fondo al file.
    let esito =
        plenora_core::panic_policy::barriera_di_dipendenza(std::panic::AssertUnwindSafe(|| {
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
/// Il testo di un panico di una dipendenza puo' contenere dati della riga
/// (regola «errori senza dati»): si riporta solo la FORMA del payload.
#[must_use]
pub fn descrivi_panico(panico: &Box<dyn std::any::Any + Send>) -> &'static str {
    plenora_core::panic_policy::forma_payload(panico.as_ref())
}

/// Corpo di [`decode_ipc`], senza la barriera antipanico.
fn decode_ipc_unguarded(
    payload: &[u8],
) -> Result<(SchemaRef, Vec<RecordBatch>), ArrowTransportError> {
    let reader =
        StreamReader::try_new(payload, None).map_err(|error| ArrowTransportError::arrow(&error))?;
    let schema = reader.schema();
    if schema.fields().len() > MAX_COLUMNS {
        return Err(ArrowTransportError::TooManyColumns(schema.fields().len()));
    }
    let mut batches = Vec::new();
    let mut rows = 0_u64;
    for batch in reader {
        let batch = batch.map_err(|error| ArrowTransportError::arrow(&error))?;
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
            .map_err(|error| ArrowTransportError::arrow(&error))?;
        for batch in batches {
            writer
                .write(batch)
                .map_err(|error| ArrowTransportError::arrow(&error))?;
        }
        writer
            .finish()
            .map_err(|error| ArrowTransportError::arrow(&error))?;
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
            .map_err(|error| ArrowTransportError::arrow(&error))?;
        for batch in batches {
            writer
                .write(batch)
                .map_err(|error| ArrowTransportError::arrow(&error))?;
        }
        writer
            .finish()
            .map_err(|error| ArrowTransportError::arrow(&error))?;
    }
    if payload.len() as u64 > MAX_STREAM_BYTES {
        return Err(ArrowTransportError::StreamTooLarge);
    }
    Ok(payload)
}

// ---------------------------------------------------------------------------
// Custom metadata: i casi strutturali del confine.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod custom_metadata {
    use super::{
        fb_custom_metadata, fb_custom_metadata_estraendo, ArrowTransportError,
        MAX_IPC_CUSTOM_METADATA_KEY_BYTES, MAX_IPC_CUSTOM_METADATA_PAIRS,
        MAX_IPC_CUSTOM_METADATA_VALUE_BYTES,
    };

    /// Che cosa mettere in una voce.
    enum Campo<'a> {
        /// Stringa presente, con questi byte esatti — anche non UTF-8.
        Byte(&'a [u8]),
        /// Campo assente dalla vtable: e' l'offset zero che una validazione
        /// distratta lascia passare, e che fa panicare `arrow-ipc` quando
        /// legge i custom metadata del footer.
        Assente,
    }

    /// Costruisce il buffer flatbuffer minimo che `fb_custom_metadata` sa
    /// percorrere, con la tabella padre in posizione zero.
    ///
    /// Non serve una vtable per la tabella padre: la funzione sotto esame non
    /// la legge, riceve l'offset del campo gia' risolto. Serve invece una
    /// vtable **vera** per ogni `KeyValue`, perche' quella viene percorsa.
    ///
    /// ```text
    ///   0   4 byte    riempimento: l'offset del campo non puo' essere zero,
    ///                   che per `fb_custom_metadata` significa «campo assente»
    ///   4   u32       offset relativo al vettore
    ///   8   u32       numero di coppie
    ///  12   u32 * n   offset relativi alle tabelle KeyValue
    ///   ..            per ogni coppia: vtable, tabella, chiave, valore
    ///   ..  1 byte    coda: le stringhe flatbuffer sono NUL-terminate
    /// ```
    ///
    /// Le stringhe stanno **dopo** la propria tabella perche' gli offset
    /// indiretti dei flatbuffer vanno in avanti.
    fn costruisci(coppie: &[(Campo<'_>, Campo<'_>)]) -> Vec<u8> {
        let mut buf: Vec<u8> = Vec::new();
        // Riempimento: il campo vive all'offset 4, perche' l'offset zero
        // significa «campo assente» e i casi positivi passerebbero a vuoto.
        buf.extend_from_slice(&0_u32.to_le_bytes());
        buf.extend_from_slice(&4_u32.to_le_bytes());
        let n = u32::try_from(coppie.len()).expect("conteggio entro u32");
        buf.extend_from_slice(&n.to_le_bytes());
        let primo_slot = buf.len();
        buf.resize(primo_slot + coppie.len() * 4, 0);

        for (indice, (chiave, valore)) in coppie.iter().enumerate() {
            let presente = |campo: &Campo<'_>| matches!(campo, Campo::Byte(_));

            // vtable: [lunghezza][lunghezza tabella][slot 0][slot 1]
            let vtable = buf.len();
            buf.extend_from_slice(&8_u16.to_le_bytes());
            buf.extend_from_slice(&12_u16.to_le_bytes());
            buf.extend_from_slice(&if presente(chiave) { 4_u16 } else { 0 }.to_le_bytes());
            buf.extend_from_slice(&if presente(valore) { 8_u16 } else { 0 }.to_le_bytes());

            // tabella: [soffset alla vtable][rel chiave][rel valore]
            let tabella = buf.len();
            let soffset = i32::try_from(tabella - vtable).expect("soffset entro i32");
            buf.extend_from_slice(&soffset.to_le_bytes());
            buf.extend_from_slice(&0_u32.to_le_bytes());
            buf.extend_from_slice(&0_u32.to_le_bytes());

            // Lo slot del vettore punta a questa tabella.
            let slot = primo_slot + indice * 4;
            let rel = u32::try_from(tabella - slot).expect("offset entro u32");
            buf[slot..slot + 4].copy_from_slice(&rel.to_le_bytes());

            // Le due stringhe, ciascuna dopo la tabella che la nomina.
            for (slot_campo, campo) in [(tabella + 4, chiave), (tabella + 8, valore)] {
                if let Campo::Byte(bytes) = campo {
                    let posizione = buf.len();
                    let lunghezza = u32::try_from(bytes.len()).expect("stringa entro u32");
                    buf.extend_from_slice(&lunghezza.to_le_bytes());
                    buf.extend_from_slice(bytes);
                    buf.push(0);
                    let rel = u32::try_from(posizione - slot_campo).expect("offset entro u32");
                    buf[slot_campo..slot_campo + 4].copy_from_slice(&rel.to_le_bytes());
                }
            }
        }
        // `fb_string` pretende almeno un byte dopo il contenuto.
        buf.push(0);
        buf
    }

    fn valida(coppie: &[(Campo<'_>, Campo<'_>)]) -> Result<(), ArrowTransportError> {
        let buf = costruisci(coppie);
        fb_custom_metadata(&buf, 0, 4)
    }

    /// Come [`valida`], ma passando dalla variante che **estrae**.
    ///
    /// I controlli vanno provati attraverso entrambe le varianti.
    fn estrai(
        coppie: &[(Campo<'_>, Campo<'_>)],
        cercata: &str,
    ) -> Result<Option<String>, ArrowTransportError> {
        let buf = costruisci(coppie);
        fb_custom_metadata_estraendo(&buf, 0, 4, Some(cercata))
            .map(|trovato| trovato.map(str::to_owned))
    }

    #[test]
    fn caso_13_l_estrazione_rende_il_valore_della_chiave_cercata() {
        assert_eq!(
            estrai(&[coppia("a", "uno"), coppia("k", "due")], "k").expect("valido"),
            Some("due".to_owned())
        );
        // Chiave assente: nessun valore, e non e' un errore.
        assert_eq!(estrai(&[coppia("a", "uno")], "k").expect("valido"), None);
    }

    #[test]
    fn caso_14_l_estrazione_rifiuta_comunque_i_duplicati() {
        // Sulla chiave cercata...
        assert!(matches!(
            estrai(&[coppia("k", "uno"), coppia("k", "due")], "k"),
            Err(ArrowTransportError::IpcMetadataInvalid("chiave duplicata"))
        ));
        // ...e su un'altra qualsiasi: il difetto e' dell'insieme, non della
        // voce che interessa a chi legge.
        assert!(matches!(
            estrai(
                &[coppia("a", "uno"), coppia("a", "due"), coppia("k", "v")],
                "k"
            ),
            Err(ArrowTransportError::IpcMetadataInvalid("chiave duplicata"))
        ));
    }

    #[test]
    fn caso_15_l_estrazione_rifiuta_comunque_le_voci_malformate() {
        assert!(matches!(
            estrai(&[(Campo::Assente, Campo::Byte(b"v"))], "k"),
            Err(ArrowTransportError::IpcMetadataInvalid("chiave assente"))
        ));
        assert!(matches!(
            estrai(&[coppia("", "v")], "k"),
            Err(ArrowTransportError::IpcMetadataInvalid("chiave vuota"))
        ));
    }

    fn coppia<'a>(chiave: &'a str, valore: &'a str) -> (Campo<'a>, Campo<'a>) {
        (
            Campo::Byte(chiave.as_bytes()),
            Campo::Byte(valore.as_bytes()),
        )
    }

    #[test]
    fn una_collezione_valida_passa() {
        assert!(valida(&[
            coppia("plenora.geometry.srid", "4326"),
            coppia("ARROW:extension:name", "geoarrow.wkb"),
        ])
        .is_ok());
    }

    // --- 1-3: i tre tetti, superati SEPARATAMENTE -------------------------
    //
    // Superarli insieme non direbbe quale ha parato, ed e' l'unica cosa che
    // questi tre test devono dire.

    #[test]
    fn tetto_1_troppe_coppie() {
        let chiavi: Vec<String> = (0..=MAX_IPC_CUSTOM_METADATA_PAIRS)
            .map(|indice| format!("k{indice}"))
            .collect();
        let coppie: Vec<(Campo<'_>, Campo<'_>)> = chiavi
            .iter()
            .map(|chiave| (Campo::Byte(chiave.as_bytes()), Campo::Byte(b"v")))
            .collect();
        assert!(matches!(
            valida(&coppie),
            Err(ArrowTransportError::IpcTooManyMetadataPairs(_, _))
        ));
    }

    #[test]
    fn tetto_2_chiave_troppo_lunga() {
        let chiave = "k".repeat(MAX_IPC_CUSTOM_METADATA_KEY_BYTES + 1);
        assert!(matches!(
            valida(&[coppia(&chiave, "v")]),
            Err(ArrowTransportError::IpcMetadataKeyTooLarge(_, _))
        ));
        // Al tetto esatto passa: il limite e' un massimo, non un divieto.
        let al_limite = "k".repeat(MAX_IPC_CUSTOM_METADATA_KEY_BYTES);
        assert!(valida(&[coppia(&al_limite, "v")]).is_ok());
    }

    #[test]
    fn tetto_3_valore_troppo_lungo() {
        let valore = "v".repeat(MAX_IPC_CUSTOM_METADATA_VALUE_BYTES + 1);
        assert!(matches!(
            valida(&[coppia("k", &valore)]),
            Err(ArrowTransportError::IpcMetadataValueTooLarge(_, _))
        ));
        let al_limite = "v".repeat(MAX_IPC_CUSTOM_METADATA_VALUE_BYTES);
        assert!(valida(&[coppia("k", &al_limite)]).is_ok());
    }

    // --- 4-5: i campi assenti, che fanno panicare arrow -------------------

    #[test]
    fn caso_4_chiave_assente() {
        assert!(matches!(
            valida(&[(Campo::Assente, Campo::Byte(b"v"))]),
            Err(ArrowTransportError::IpcMetadataInvalid("chiave assente"))
        ));
    }

    #[test]
    fn caso_5_valore_assente() {
        assert!(matches!(
            valida(&[(Campo::Byte(b"k"), Campo::Assente)]),
            Err(ArrowTransportError::IpcMetadataInvalid("valore assente"))
        ));
    }

    // --- 6-7: vuoti, con esiti OPPOSTI e voluti ---------------------------

    #[test]
    fn caso_6_chiave_vuota_rifiutata() {
        assert!(matches!(
            valida(&[coppia("", "v")]),
            Err(ArrowTransportError::IpcMetadataInvalid("chiave vuota"))
        ));
    }

    #[test]
    fn caso_7_valore_vuoto_accettato() {
        // Rifiutarlo romperebbe file legittimi che rappresentano un campo
        // assente con la stringa vuota.
        assert!(valida(&[coppia("k", "")]).is_ok());
    }

    // --- 8-9: UTF-8, verificato da noi ------------------------------------

    #[test]
    fn caso_8_chiave_non_utf8() {
        assert!(matches!(
            valida(&[(Campo::Byte(&[0xff, 0xfe]), Campo::Byte(b"v"))]),
            Err(ArrowTransportError::IpcMetadataInvalid(
                "chiave non e' UTF-8 valido"
            ))
        ));
    }

    #[test]
    fn caso_9_valore_non_utf8() {
        assert!(matches!(
            valida(&[(Campo::Byte(b"k"), Campo::Byte(&[0xff, 0xfe]))]),
            Err(ArrowTransportError::IpcMetadataInvalid(
                "valore non e' UTF-8 valido"
            ))
        ));
    }

    // --- 10-11: duplicati, rifiutati in ENTRAMBE le forme -----------------
    //
    // Anche identici: chi li raccoglie in una mappa li comprime comunque, e
    // «vince l'ultima» su una chiave autoritativa sceglie un vincitore
    // arbitrario.

    #[test]
    fn caso_10_duplicati_con_lo_stesso_valore() {
        assert!(matches!(
            valida(&[coppia("k", "v"), coppia("k", "v")]),
            Err(ArrowTransportError::IpcMetadataInvalid("chiave duplicata"))
        ));
    }

    #[test]
    fn caso_11_duplicati_con_valori_divergenti() {
        assert!(matches!(
            valida(&[coppia("k", "uno"), coppia("k", "due")]),
            Err(ArrowTransportError::IpcMetadataInvalid("chiave duplicata"))
        ));
    }

    // --- 12: le chiavi altrui ---------------------------------------------

    #[test]
    fn caso_12_chiavi_sconosciute_accettate() {
        // Il confine valida la FORMA, non il vocabolario: rifiutare le chiavi
        // altrui romperebbe l'interoperabilita' con qualunque produttore
        // Arrow che aggiunga le proprie.
        assert!(valida(&[
            coppia("qualcun.altro.chiave", "valore"),
            coppia("pandas", "{}"),
        ])
        .is_ok());
    }
}

// ---------------------------------------------------------------------------
// Il campo 4 del footer, attraversato per davvero.
// ---------------------------------------------------------------------------

/// Dimostrano che `parse_footer` **chiama** la validazione del campo 4, cosa
/// che i test diretti non vedono: file veri scritti con `FileWriter` e
/// `write_metadata`, attraverso il validatore pubblico. `FileWriter` non
/// produce voci malformate, quindi qui ci sono solo i casi di tetto.
#[cfg(test)]
mod footer_end_to_end {
    use plenora_core::arrow::array::{Int32Array, RecordBatch};
    use plenora_core::arrow::ipc::writer::FileWriter;
    use plenora_core::arrow::schema::{DataType, Field, Schema, SchemaRef};
    use std::sync::Arc;

    use super::{
        validate_ipc_file_framing, ArrowTransportError, IpcLimits,
        MAX_IPC_CUSTOM_METADATA_KEY_BYTES, MAX_IPC_CUSTOM_METADATA_PAIRS,
        MAX_IPC_CUSTOM_METADATA_VALUE_BYTES,
    };

    fn batch_minimo() -> (SchemaRef, RecordBatch) {
        let schema: SchemaRef =
            Arc::new(Schema::new(vec![Field::new("n", DataType::Int32, false)]));
        let batch = RecordBatch::try_new(
            Arc::clone(&schema),
            vec![Arc::new(Int32Array::from(vec![1, 2, 3]))],
        )
        .expect("batch minimo");
        (schema, batch)
    }

    /// File Arrow IPC completo, con le coppie richieste nei custom metadata
    /// del **footer**.
    fn file_con_metadata(coppie: &[(String, String)]) -> Vec<u8> {
        let (schema, batch) = batch_minimo();
        let mut byte = Vec::new();
        {
            let mut writer = FileWriter::try_new(&mut byte, &schema).expect("writer");
            for (chiave, valore) in coppie {
                writer.write_metadata(chiave.clone(), valore.clone());
            }
            writer.write(&batch).expect("scrittura batch");
            writer.finish().expect("chiusura file");
        }
        byte
    }

    fn valida(byte: &[u8]) -> Result<(), ArrowTransportError> {
        let mut sorgente: &[u8] = byte;
        validate_ipc_file_framing(&mut sorgente, &IpcLimits::default())
    }

    #[test]
    fn footer_con_metadata_valida_accettato() {
        let coppie = vec![
            ("plenora.commit.token".to_owned(), "0".repeat(64)),
            ("pandas".to_owned(), "{}".to_owned()),
        ];
        let byte = file_con_metadata(&coppie);
        assert!(
            valida(&byte).is_ok(),
            "un footer con custom metadata legittima deve passare"
        );
    }

    #[test]
    fn footer_con_chiave_oltre_il_tetto_respinto() {
        // Il file e' Arrow VALIDO: `pyarrow` lo leggerebbe. Il confine lo
        // rifiuta di proposito, e lo fa PRIMA di costruire un `FileReader`.
        let chiave = "k".repeat(MAX_IPC_CUSTOM_METADATA_KEY_BYTES + 1);
        let byte = file_con_metadata(&[(chiave, "v".to_owned())]);
        assert!(
            matches!(
                valida(&byte),
                Err(ArrowTransportError::IpcMetadataKeyTooLarge(_, _))
            ),
            "il campo 4 del footer non e' collegato al validatore"
        );
    }

    #[test]
    fn footer_con_valore_oltre_il_tetto_respinto() {
        let valore = "v".repeat(MAX_IPC_CUSTOM_METADATA_VALUE_BYTES + 1);
        let byte = file_con_metadata(&[("k".to_owned(), valore)]);
        assert!(matches!(
            valida(&byte),
            Err(ArrowTransportError::IpcMetadataValueTooLarge(_, _))
        ));
    }

    #[test]
    fn footer_con_troppe_coppie_respinto() {
        let coppie: Vec<(String, String)> = (0..=MAX_IPC_CUSTOM_METADATA_PAIRS)
            .map(|indice| (format!("k{indice}"), "v".to_owned()))
            .collect();
        let byte = file_con_metadata(&coppie);
        assert!(matches!(
            valida(&byte),
            Err(ArrowTransportError::IpcTooManyMetadataPairs(_, _))
        ));
    }
}

// ---------------------------------------------------------------------------
// Campi che arrow dereferenzia con `unwrap` pur essendo opzionali nel
// formato.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod campi_pretesi {
    use super::{
        fb_field_table, fb_schema, parse_footer, ArrowTransportError, IpcLimits, SchemaBudget,
    };

    /// Costruisce una tabella flatbuffer con i soli slot indicati.
    ///
    /// `slot[i] == None` significa campo **assente**, il caso che fa panicare
    /// arrow. Torna `(buf, posizione della tabella)`; la tabella e' vuota.
    fn tabella_con_slot(slot: &[Option<u16>]) -> (Vec<u8>, usize) {
        let mut buf: Vec<u8> = vec![0; 4];
        let vtable = buf.len();
        let lunghezza = u16::try_from(4 + slot.len() * 2).expect("vtable entro u16");
        buf.extend_from_slice(&lunghezza.to_le_bytes());
        // Lunghezza della tabella: il solo soffset, dato che nessuno slot
        // punta a qualcosa.
        buf.extend_from_slice(&4_u16.to_le_bytes());
        for voce in slot {
            buf.extend_from_slice(&voce.unwrap_or(0).to_le_bytes());
        }
        let tabella = buf.len();
        let soffset = i32::try_from(tabella - vtable).expect("soffset entro i32");
        buf.extend_from_slice(&soffset.to_le_bytes());
        // La radice del footer sta nei primi quattro byte.
        let radice = u32::try_from(tabella).expect("radice entro u32");
        buf[0..4].copy_from_slice(&radice.to_le_bytes());
        buf.resize(buf.len() + 16, 0);
        (buf, tabella)
    }

    #[test]
    fn footer_senza_schema_respinto() {
        // `reader.rs` lo legge con `footer.schema().unwrap()`: assente,
        // panica dentro la dipendenza. Il writer lo emette sempre.
        // Slot: 0 version, 1 schema, 2 dizionari, 3 record batch, 4 metadata.
        let (buf, _) = tabella_con_slot(&[None; 5]);
        assert!(matches!(
            parse_footer(&buf, &IpcLimits::default()),
            Err(ArrowTransportError::IpcFooterInvalid("schema assente"))
        ));
    }

    #[test]
    fn schema_senza_fields_respinto() {
        // `fb_to_schema` lo legge con `fb.fields().unwrap()`. Uno schema
        // senza colonne e' legittimo, ma allora il campo c'e' con zero
        // elementi: assente e vuoto non sono la stessa cosa.
        // Slot: 0 endianness, 1 fields, 2 metadata, 3 features.
        let (buf, tabella) = tabella_con_slot(&[None; 4]);
        assert!(matches!(
            fb_schema(&buf, tabella),
            Err(ArrowTransportError::IpcSchemaInvalid(
                "schema senza il campo fields"
            ))
        ));
    }

    #[test]
    fn dictionary_senza_index_type_respinto() {
        // `get_data_type` lo legge con `dictionary.indexType().unwrap()`, e
        // una codifica a dizionario senza tipo dell'indice non significa
        // comunque nulla.
        //
        // Il campo 4 del Field punta alla tabella DictionaryEncoding, che ha
        // lo slot 1 (`indexType`) assente.
        let mut buf: Vec<u8> = vec![0; 4];

        // DictionaryEncoding: slot 0 id, 1 indexType, 2 isOrdered.
        let dict_vtable = buf.len();
        buf.extend_from_slice(&10_u16.to_le_bytes());
        buf.extend_from_slice(&4_u16.to_le_bytes());
        buf.extend_from_slice(&[0_u8; 6]); // tre slot assenti
        let dict_tabella = buf.len();
        let dict_soffset = i32::try_from(dict_tabella - dict_vtable).expect("soffset");
        buf.extend_from_slice(&dict_soffset.to_le_bytes());

        // Field: slot 0 name, 1 nullable, 2 type_type, 3 type, 4 dictionary,
        // 5 children, 6 custom_metadata. Solo il 4 e' presente.
        let campo_vtable = buf.len();
        buf.extend_from_slice(&18_u16.to_le_bytes());
        buf.extend_from_slice(&8_u16.to_le_bytes());
        for indice in 0..7_usize {
            let valore: u16 = if indice == 4 { 4 } else { 0 };
            buf.extend_from_slice(&valore.to_le_bytes());
        }
        let campo_tabella = buf.len();
        let campo_soffset = i32::try_from(campo_tabella - campo_vtable).expect("soffset");
        buf.extend_from_slice(&campo_soffset.to_le_bytes());
        // Slot 4: offset relativo alla tabella del dizionario, all'indietro.
        // Gli offset indiretti vanno in avanti, quindi la tabella del
        // dizionario si riscrive qui dopo.
        let slot_dizionario = buf.len();
        buf.extend_from_slice(&0_u32.to_le_bytes());

        let dict_copia = buf.len();
        let copia_soffset = i32::try_from(dict_copia - dict_vtable).expect("soffset");
        buf.extend_from_slice(&copia_soffset.to_le_bytes());
        let relativo = u32::try_from(dict_copia - slot_dizionario).expect("offset");
        buf[slot_dizionario..slot_dizionario + 4].copy_from_slice(&relativo.to_le_bytes());
        buf.resize(buf.len() + 16, 0);

        let mut budget = SchemaBudget::new();
        assert!(matches!(
            fb_field_table(&buf, campo_tabella, 0, &mut budget),
            Err(ArrowTransportError::IpcSchemaInvalid(
                "dictionary senza indexType"
            ))
        ));
    }
}

// ---------------------------------------------------------------------------
// La barriera anti-panico, con una prova sua.
// ---------------------------------------------------------------------------

/// Il confine pretende i campi che `arrow-ipc` dereferenzia con `unwrap`, ma
/// la barriera resta necessaria: `convert.rs` pretende i figli dei tipi
/// annidati e panica sui codici di tipo che non riconosce
/// ([`errori-e-limiti.md`](../../../../docs/errori-e-limiti.md)). Questo
/// modulo costruisce quel caso partendo da uno stream Arrow **vero**.
#[cfg(test)]
mod barriera_antipanico {
    use std::sync::Arc;

    use plenora_core::arrow::array::{types::Int32Type, ArrayRef, ListArray, RecordBatch};
    use plenora_core::arrow::ipc::writer::StreamWriter;

    use super::{decode_ipc, ArrowTransportError};

    fn u32_a(buf: &[u8], pos: usize) -> usize {
        u32::from_le_bytes(buf[pos..pos + 4].try_into().expect("quattro byte")) as usize
    }

    fn u16_a(buf: &[u8], pos: usize) -> usize {
        u16::from_le_bytes(buf[pos..pos + 2].try_into().expect("due byte")) as usize
    }

    /// Posizione della vtable di una tabella flatbuffer.
    fn vtable_di(buf: &[u8], tabella: usize) -> usize {
        let soffset = i32::from_le_bytes(buf[tabella..tabella + 4].try_into().expect("soffset"));
        usize::try_from(i64::try_from(tabella).expect("tabella") - i64::from(soffset))
            .expect("vtable dentro il buffer")
    }

    /// Segue un offset indiretto.
    fn indiretto(buf: &[u8], tabella: usize, slot: usize) -> usize {
        tabella + slot + u32_a(buf, tabella + slot)
    }

    fn stream_con_colonna_list() -> Vec<u8> {
        let lista = ListArray::from_iter_primitive::<Int32Type, _, _>(vec![
            Some(vec![Some(1), Some(2)]),
            Some(vec![Some(3)]),
        ]);
        let batch = RecordBatch::try_from_iter(vec![("l", Arc::new(lista) as ArrayRef)])
            .expect("batch con lista");
        let mut byte = Vec::new();
        {
            let mut writer =
                StreamWriter::try_new(&mut byte, &batch.schema()).expect("writer di stream");
            writer.write(&batch).expect("scrittura");
            writer.finish().expect("chiusura");
        }
        byte
    }

    /// Toglie il campo `children` al primo `Field` dello schema, azzerando il
    /// suo slot nella vtable.
    ///
    /// Ogni passo e' verificato: se il layout di `arrow-ipc` cambiasse, questo
    /// test deve fallire dicendo **dove**, non applicare la modifica al byte
    /// sbagliato e poi passare per la ragione sbagliata.
    fn togli_children(mut payload: Vec<u8>) -> Vec<u8> {
        assert_eq!(
            &payload[0..4],
            &[0xff; 4],
            "atteso il marcatore di continuazione"
        );
        let lunghezza = u32_a(&payload, 4);
        let inizio = 8;
        let m = &payload[inizio..inizio + lunghezza];

        let radice = u32_a(m, 0);
        let vtable = vtable_di(m, radice);
        // Message: slot 2 = header dell'unione.
        let slot_header = u16_a(m, vtable + 4 + 2 * 2);
        assert!(slot_header != 0, "il messaggio non ha un header");
        let header = indiretto(m, radice, slot_header);

        let schema_vtable = vtable_di(m, header);
        // Schema: slot 1 = fields.
        let slot_fields = u16_a(m, schema_vtable + 4 + 2);
        assert!(slot_fields != 0, "lo schema non ha il campo fields");
        let vettore = indiretto(m, header, slot_fields);
        assert!(u32_a(m, vettore) >= 1, "lo schema non ha colonne");
        let campo = indiretto(m, vettore + 4, 0);

        let campo_vtable = vtable_di(m, campo);
        let lunghezza_vtable = u16_a(m, campo_vtable);
        // Field: slot 5 = children.
        let posizione = campo_vtable + 4 + 5 * 2;
        assert!(
            posizione + 2 <= campo_vtable + lunghezza_vtable,
            "la vtable del campo non arriva allo slot children: layout inatteso"
        );
        assert!(
            u16_a(m, posizione) != 0,
            "il campo List non dichiara children: non c'e' nulla da togliere"
        );

        let assoluta = inizio + posizione;
        payload[assoluta..assoluta + 2].copy_from_slice(&0_u16.to_le_bytes());
        payload
    }

    /// Il test **non** sostituisce l'hook del processo, condiviso con i test
    /// in parallelo: il panico di `arrow-ipc` finisce su stderr senza essere
    /// un fallimento.
    #[test]
    fn un_list_senza_children_esce_come_errore_invece_di_abbattere_il_processo() {
        let ostile = togli_children(stream_con_colonna_list());
        let esito = decode_ipc(&ostile);
        assert!(
            matches!(esito, Err(ArrowTransportError::ArrowPanic(_))),
            "atteso ArrowPanic dalla barriera, ottenuto {esito:?}"
        );
    }
}

/// Le posizioni che farebbero traboccare una somma non controllata.
///
/// `pos` arriva dal file e `pos + larghezza` puo' uscire da `usize`: si
/// pretende `IpcTruncated`, mai un panico.
#[cfg(test)]
mod somme_al_limite {
    use super::{
        fb_field, fb_i32, fb_i64, fb_indirect, fb_string, fb_table, fb_u16, fb_u32, fb_vector,
        ArrowTransportError,
    };

    #[test]
    fn le_quattro_letture_al_limite_di_usize_dicono_troncato() {
        let vuoto: &[u8] = &[];
        let pieno = [0_u8; 64];
        for buf in [vuoto, pieno.as_slice()] {
            for posizione in [usize::MAX, usize::MAX - 1, usize::MAX - 7] {
                assert!(
                    matches!(
                        fb_u16(buf, posizione),
                        Err(ArrowTransportError::IpcTruncated)
                    ),
                    "fb_u16 a {posizione}"
                );
                assert!(
                    matches!(
                        fb_u32(buf, posizione),
                        Err(ArrowTransportError::IpcTruncated)
                    ),
                    "fb_u32 a {posizione}"
                );
                assert!(
                    matches!(
                        fb_i32(buf, posizione),
                        Err(ArrowTransportError::IpcTruncated)
                    ),
                    "fb_i32 a {posizione}"
                );
                assert!(
                    matches!(
                        fb_i64(buf, posizione),
                        Err(ArrowTransportError::IpcTruncated)
                    ),
                    "fb_i64 a {posizione}"
                );
            }
        }
    }

    /// Le stesse posizioni sui lettori composti: ognuno somma per conto suo,
    /// e nessuna di quelle somme puo' panicare.
    #[test]
    fn i_lettori_composti_al_limite_di_usize_dicono_troncato() {
        let buf = [0_u8; 64];
        assert!(matches!(
            fb_table(&buf, usize::MAX),
            Err(ArrowTransportError::IpcTruncated)
        ));
        assert!(matches!(
            fb_field(&buf, usize::MAX, usize::MAX, usize::MAX),
            Err(ArrowTransportError::IpcTruncated)
        ));
        assert!(matches!(
            fb_indirect(&buf, usize::MAX, 4),
            Err(ArrowTransportError::IpcTruncated)
        ));
        assert!(matches!(
            fb_vector(&buf, usize::MAX, 16),
            Err(ArrowTransportError::IpcTruncated)
        ));
        assert!(matches!(
            fb_string(&buf, usize::MAX),
            Err(ArrowTransportError::IpcTruncated)
        ));
    }
}

/// Il tetto cumulativo sui body dei dizionari: l'aritmetica, provata sui
/// blocchi invece che su un file.
///
/// La somma che trabocca non e' costruibile con un artefatto reale: qui i
/// blocchi si costruiscono a mano.
#[cfg(test)]
mod tetto_dizionari {
    use std::sync::Arc;

    use plenora_core::arrow::array::RecordBatch;
    use plenora_core::arrow::ipc::writer::FileWriter;
    use plenora_core::arrow::schema::{DataType, Field, Schema};

    use super::{
        valida_file_ed_estrai, validate_ipc_message_metadata, verifica_tetto_dizionari,
        ArrowTransportError, FooterBlock, IpcLimits, ARROW_FILE_MAGIC, FOOTER_BLOCK_BYTES,
    };

    /// Un file Arrow IPC valido con **una colonna dictionary-encoded**, quindi
    /// con almeno un blocco di dizionario nel footer.
    fn artefatto_con_dizionario() -> Vec<u8> {
        use plenora_core::arrow::array::{types::Int32Type, DictionaryArray};

        let schema = Arc::new(Schema::new(vec![Field::new(
            "citta",
            DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Utf8)),
            true,
        )]));
        let colonna: DictionaryArray<Int32Type> =
            vec!["milano", "torino", "milano"].into_iter().collect();
        let batch = RecordBatch::try_new(Arc::clone(&schema), vec![Arc::new(colonna)])
            .expect("batch valido");
        let mut byte = Vec::new();
        {
            let mut writer = FileWriter::try_new(&mut byte, &schema).expect("writer");
            writer.write(&batch).expect("scrittura");
            writer.finish().expect("chiusura");
        }
        byte
    }

    /// Il primo blocco DIZIONARIO del footer, letto con lo stesso parser che il
    /// confine usa: il difetto va iniettato dove il codice lo leggera'.
    fn primo_blocco_dizionario(byte: &[u8]) -> FooterBlock {
        let (footer, _) = regione_del_footer(byte);
        let (blocchi, dizionari, _) =
            super::parse_footer_estraendo(&footer, &IpcLimits::default(), None)
                .expect("footer leggibile");
        assert!(
            dizionari >= 1,
            "l'artefatto di prova deve avere un dizionario"
        );
        blocchi[0]
    }

    /// Footer del file e offset a cui comincia.
    fn regione_del_footer(byte: &[u8]) -> (Vec<u8>, usize) {
        let fine_trailer = byte.len() - ARROW_FILE_MAGIC.len();
        let lunghezza = i32::from_le_bytes(
            byte[fine_trailer - 4..fine_trailer]
                .try_into()
                .expect("quattro byte"),
        );
        let lunghezza = usize::try_from(lunghezza).expect("lunghezza positiva");
        let inizio = fine_trailer - 4 - lunghezza;
        (byte[inizio..inizio + lunghezza].to_vec(), inizio)
    }

    /// Riscrive il `bodyLength` del blocco dato, dentro il footer del file.
    ///
    /// Il `Block` e' uno struct flatbuffer di 24 byte — offset `i64`,
    /// `metaDataLength` `i32` con quattro di padding, `bodyLength` `i64` — e si
    /// individua per il suo contenuto, che nel footer e' unico.
    fn sostituisci_body_len(byte: &mut [u8], blocco: FooterBlock, nuovo: u64) {
        let (footer, inizio_footer) = regione_del_footer(byte);
        let mut atteso = Vec::with_capacity(FOOTER_BLOCK_BYTES);
        atteso.extend_from_slice(&i64::try_from(blocco.offset).expect("offset").to_le_bytes());
        atteso.extend_from_slice(
            &i32::try_from(blocco.metadata_len)
                .expect("metadata_len")
                .to_le_bytes(),
        );
        atteso.extend_from_slice(&[0_u8; 4]);
        atteso.extend_from_slice(
            &i64::try_from(blocco.body_len)
                .expect("body_len")
                .to_le_bytes(),
        );

        let posizione = footer
            .windows(FOOTER_BLOCK_BYTES)
            .position(|finestra| finestra == atteso.as_slice())
            .expect("blocco presente nel footer");
        let inizio_body = inizio_footer + posizione + 16;
        let nuovo = i64::try_from(nuovo).expect("entro i64");
        byte[inizio_body..inizio_body + 8].copy_from_slice(&nuovo.to_le_bytes());
    }

    fn blocco(body_len: u64) -> FooterBlock {
        FooterBlock {
            offset: 8,
            metadata_len: 8,
            body_len,
        }
    }

    #[test]
    fn nessun_dizionario_non_trattiene_niente() {
        assert!(verifica_tetto_dizionari(&[], 0).is_ok());
    }

    #[test]
    fn la_somma_esattamente_al_tetto_e_accettata() {
        // Il confine e' `>`, non `>=`: un insieme che sta esattamente nel
        // budget concesso lo rispetta.
        let dizionari = [blocco(400), blocco(600)];
        assert!(verifica_tetto_dizionari(&dizionari, 1000).is_ok());
    }

    #[test]
    fn singolarmente_sotto_il_tetto_ma_cumulativamente_oltre_e_respinto() {
        // Nessuno dei tre supererebbe un tetto per singolo body: e' la somma
        // a sforare, ed e' la ragione per cui questo controllo esiste.
        let dizionari = [blocco(400), blocco(400), blocco(400)];
        let esito = verifica_tetto_dizionari(&dizionari, 1000);
        assert!(matches!(
            esito,
            Err(ArrowTransportError::IpcRetainedDictionariesTooLarge {
                declared: 1200,
                limit: 1000
            })
        ));
    }

    /// Un `DictionaryBatch` **delta** e' rifiutato in prevalidazione.
    ///
    /// Il tetto cumulativo non basta: su un delta il picco si avvicina al
    /// doppio della somma (vedi `validate_ipc_message_metadata`). Discrimina:
    /// lo stesso messaggio con `isDelta` a **false** deve passare.
    #[test]
    fn un_dictionary_delta_e_rifiutato_e_uno_normale_no() {
        for (delta, atteso_rifiuto) in [(0_u8, false), (1_u8, true)] {
            let metadata = messaggio_dictionary(Some(delta), true);
            let esito = validate_ipc_message_metadata(&metadata);
            if atteso_rifiuto {
                assert!(
                    matches!(
                        esito,
                        Err(ArrowTransportError::IpcSchemaInvalid(
                            "dictionary delta non supportato"
                        ))
                    ),
                    "isDelta=true va rifiutato: {esito:?}"
                );
            } else {
                assert!(esito.is_ok(), "isDelta=false deve passare: {esito:?}");
            }
        }

        // E senza il campo affatto: assente significa false, quindi passa.
        let esito = validate_ipc_message_metadata(&messaggio_dictionary(None, true));
        assert!(
            esito.is_ok(),
            "isDelta assente vale false e deve passare: {esito:?}"
        );
    }

    /// Un messaggio che dichiara `header_type` e **non porta l'header** e'
    /// rifiutato.
    ///
    /// Senza header il messaggio non attraversa i controlli su `data` e
    /// `isDelta` e arriva ad arrow, che legge `header_as_dictionary_batch()`
    /// con `unwrap()`. Discrimina: lo stesso messaggio **con** l'header passa.
    #[test]
    fn un_messaggio_senza_header_e_rifiutato() {
        let esito = validate_ipc_message_metadata(&messaggio_dictionary_con(Some(0), true, false));
        assert!(
            matches!(
                esito,
                Err(ArrowTransportError::IpcSchemaInvalid(
                    "messaggio IPC senza header"
                ))
            ),
            "header dichiarato e assente va rifiutato: {esito:?}"
        );

        let esito = validate_ipc_message_metadata(&messaggio_dictionary_con(Some(0), true, true));
        assert!(esito.is_ok(), "con l'header deve passare: {esito:?}");
    }

    /// `MessageHeader::NONE` senza header e' un messaggio vuoto, e passa.
    ///
    /// Con un header, invece, e' rifiutato: il tipo dichiara «nessun
    /// contenuto» e il messaggio ne porta uno.
    #[test]
    fn il_tipo_none_e_un_messaggio_vuoto_solo_se_e_davvero_vuoto() {
        let esito = validate_ipc_message_metadata(&messaggio_con_tipo(0, false));
        assert!(esito.is_ok(), "NONE senza header e' un no-op: {esito:?}");

        let esito = validate_ipc_message_metadata(&messaggio_con_tipo(0, true));
        assert!(
            matches!(
                esito,
                Err(ArrowTransportError::IpcSchemaInvalid(
                    "messaggio IPC di tipo NONE con un header"
                ))
            ),
            "NONE con un header si contraddice: {esito:?}"
        );
    }

    /// Un tipo **non supportato** senza header non passa in mezzo ai due
    /// controlli.
    ///
    /// E' la combinazione che un `if` sulla presenza e un `match` sul tipo
    /// separati lascerebbero scoperta. L'errore e' quello del tipo, che si
    /// rifiuta comunque.
    #[test]
    fn un_tipo_non_supportato_senza_header_e_rifiutato() {
        // Zero non e' qui: e' `MessageHeader::NONE`, che ha il suo caso.
        for tipo in [4_u8, 5, 6, 200] {
            for con_header in [false, true] {
                let esito = validate_ipc_message_metadata(&messaggio_con_tipo(tipo, con_header));
                assert!(
                    matches!(esito, Err(ArrowTransportError::Arrow(_))),
                    "il tipo {tipo} va rifiutato, con header={con_header}: {esito:?}"
                );
            }
        }
    }

    /// Un `DictionaryBatch` senza `data` e' rifiutato prima di arrivare ad
    /// arrow.
    ///
    /// `read_dictionary` lo legge con `batch.data().unwrap()`; la barriera
    /// anti-panico e' l'ultima difesa, non la prima. Discrimina: lo stesso
    /// messaggio **con** `data` passa.
    #[test]
    fn un_dictionary_batch_senza_data_e_rifiutato() {
        let esito = validate_ipc_message_metadata(&messaggio_dictionary(Some(0), false));
        assert!(
            matches!(
                esito,
                Err(ArrowTransportError::IpcSchemaInvalid(
                    "DictionaryBatch senza data"
                ))
            ),
            "un DictionaryBatch senza data va rifiutato: {esito:?}"
        );

        let esito = validate_ipc_message_metadata(&messaggio_dictionary(Some(0), true));
        assert!(esito.is_ok(), "con data deve passare: {esito:?}");
    }

    /// Costruisce i metadati di un messaggio `DictionaryBatch`, byte per byte.
    ///
    /// A mano perche' arrow non emette ne' delta ne' messaggi senza `data`.
    /// `delta`: `None` = slot assente (vale `false`), `Some(v)` = slot
    /// presente col valore. `con_data`: se lo slot `data` punta al
    /// `RecordBatch` interno.
    fn messaggio_dictionary(delta: Option<u8>, con_data: bool) -> Vec<u8> {
        messaggio_con(2, delta, con_data, true)
    }

    /// Un messaggio con l'`header_type` che si vuole, header presente o no.
    fn messaggio_con_tipo(tipo: u8, con_header: bool) -> Vec<u8> {
        messaggio_con(tipo, Some(0), true, con_header)
    }

    /// La forma completa: tipo dichiarato, header presente o no, e i due
    /// campi del `DictionaryBatch`.
    fn messaggio_dictionary_con(delta: Option<u8>, con_data: bool, con_header: bool) -> Vec<u8> {
        messaggio_con(2, delta, con_data, con_header)
    }

    fn messaggio_con(tipo: u8, delta: Option<u8>, con_data: bool, con_header: bool) -> Vec<u8> {
        let mut buf: Vec<u8> = vec![0; 4];

        // --- Message: slot 0 version, 1 header_type, 2 header, 3 bodyLength,
        //     4 custom_metadata. Presenti l'1, il 2 e il 3.
        let vt_messaggio = buf.len();
        buf.extend_from_slice(&14_u16.to_le_bytes()); // 4 + 5 slot x 2
        buf.extend_from_slice(&20_u16.to_le_bytes()); // soffset + 4 + 4 + 8
        for offset in [0_u16, 4, if con_header { 8 } else { 0 }, 12, 0] {
            buf.extend_from_slice(&offset.to_le_bytes());
        }
        let messaggio = buf.len();
        let soff_messaggio = i32::try_from(messaggio - vt_messaggio).expect("soffset");
        buf.extend_from_slice(&soff_messaggio.to_le_bytes());
        buf.push(tipo);
        buf.extend_from_slice(&[0_u8; 3]);
        let slot_header = buf.len();
        buf.extend_from_slice(&0_u32.to_le_bytes()); // riscritto sotto
        buf.extend_from_slice(&0_i64.to_le_bytes()); // bodyLength

        // --- DictionaryBatch: slot 0 id, 1 data, 2 isDelta.
        //
        // La vtable si accorcia quando `isDelta` e' assente: un campo oltre la
        // vtable e' un campo che non c'e', ed e' cosi' che il formato dice
        // «assente» invece di «zero».
        let vt_dizionario = buf.len();
        let slot_dict: &[u16] = if delta.is_some() {
            &[0, if con_data { 4 } else { 0 }, 8]
        } else {
            &[0, if con_data { 4 } else { 0 }]
        };
        let vtable_len = u16::try_from(4 + slot_dict.len() * 2).expect("vtable");
        buf.extend_from_slice(&vtable_len.to_le_bytes());
        buf.extend_from_slice(&12_u16.to_le_bytes()); // soffset + 4 + 4
        for offset in slot_dict {
            buf.extend_from_slice(&offset.to_le_bytes());
        }
        let dizionario = buf.len();
        let relativo = u32::try_from(dizionario - slot_header).expect("offset");
        buf[slot_header..slot_header + 4].copy_from_slice(&relativo.to_le_bytes());
        let soff_dizionario = i32::try_from(dizionario - vt_dizionario).expect("soffset");
        buf.extend_from_slice(&soff_dizionario.to_le_bytes());
        let slot_data = buf.len();
        buf.extend_from_slice(&0_u32.to_le_bytes()); // riscritto sotto
        buf.push(delta.unwrap_or(0));
        buf.extend_from_slice(&[0_u8; 3]);

        // --- RecordBatch interno: tabella vuota. Zero nodi, zero buffer,
        //     nessuna compressione: strutturalmente valida e senza body.
        let vt_batch = buf.len();
        buf.extend_from_slice(&14_u16.to_le_bytes());
        buf.extend_from_slice(&4_u16.to_le_bytes());
        buf.extend_from_slice(&[0_u8; 10]);
        let batch = buf.len();
        if con_data {
            let relativo = u32::try_from(batch - slot_data).expect("offset");
            buf[slot_data..slot_data + 4].copy_from_slice(&relativo.to_le_bytes());
        }
        let soff_batch = i32::try_from(batch - vt_batch).expect("soffset");
        buf.extend_from_slice(&soff_batch.to_le_bytes());

        let radice = u32::try_from(messaggio).expect("radice");
        buf[0..4].copy_from_slice(&radice.to_le_bytes());
        buf.resize(buf.len() + 16, 0);
        buf
    }

    /// Un messaggio Schema con un corpo e' rifiutato prima di arrow.
    ///
    /// `StreamReader::try_new` alloca il corpo prima di guardare il tipo.
    /// Discrimina: lo stesso messaggio con corpo zero passa.
    #[test]
    fn uno_schema_con_un_corpo_e_rifiutato() {
        let esito = validate_ipc_message_metadata(&messaggio_schema(0));
        assert!(
            esito.is_ok(),
            "uno Schema senza corpo deve passare: {esito:?}"
        );

        for corpo in [8_i64, 64 * 1024 * 1024] {
            let esito = validate_ipc_message_metadata(&messaggio_schema(corpo));
            assert!(
                matches!(
                    esito,
                    Err(ArrowTransportError::IpcSchemaInvalid(
                        "un messaggio Schema dichiara un corpo"
                    ))
                ),
                "uno Schema con corpo {corpo} va rifiutato: {esito:?}"
            );
        }
    }

    /// I metadati di un messaggio `Schema`, byte per byte, con il
    /// `bodyLength` che si vuole: il writer di arrow non emette mai uno Schema
    /// con un corpo, e senza il campo presente il caso non si esprime.
    ///
    /// Lo schema e' il minimo che il confine accetta: `fields` presente e
    /// vuoto, gli altri campi assenti.
    fn messaggio_schema(corpo: i64) -> Vec<u8> {
        let mut buf: Vec<u8> = vec![0; 4];

        // --- Message: slot 0 version, 1 header_type, 2 header, 3 bodyLength,
        //     4 custom_metadata. Presenti l'1, il 2 e il 3.
        let vt_messaggio = buf.len();
        buf.extend_from_slice(&14_u16.to_le_bytes());
        buf.extend_from_slice(&20_u16.to_le_bytes());
        for offset in [0_u16, 4, 8, 12, 0] {
            buf.extend_from_slice(&offset.to_le_bytes());
        }
        let messaggio = buf.len();
        let soff_messaggio = i32::try_from(messaggio - vt_messaggio).expect("soffset");
        buf.extend_from_slice(&soff_messaggio.to_le_bytes());
        buf.push(1); // MessageHeader::Schema
        buf.extend_from_slice(&[0_u8; 3]);
        let slot_header = buf.len();
        buf.extend_from_slice(&0_u32.to_le_bytes()); // riscritto sotto
        buf.extend_from_slice(&corpo.to_le_bytes());

        // --- Schema: slot 0 endianness, 1 fields. Presente il solo `fields`.
        let vt_schema = buf.len();
        buf.extend_from_slice(&8_u16.to_le_bytes());
        buf.extend_from_slice(&8_u16.to_le_bytes()); // soffset + 4
        for offset in [0_u16, 4] {
            buf.extend_from_slice(&offset.to_le_bytes());
        }
        let schema = buf.len();
        let relativo = u32::try_from(schema - slot_header).expect("offset");
        buf[slot_header..slot_header + 4].copy_from_slice(&relativo.to_le_bytes());
        let soff_schema = i32::try_from(schema - vt_schema).expect("soffset");
        buf.extend_from_slice(&soff_schema.to_le_bytes());
        let slot_fields = buf.len();
        buf.extend_from_slice(&0_u32.to_le_bytes()); // riscritto sotto

        // --- `fields`: un vettore di lunghezza zero.
        let vettore = buf.len();
        let relativo = u32::try_from(vettore - slot_fields).expect("offset");
        buf[slot_fields..slot_fields + 4].copy_from_slice(&relativo.to_le_bytes());
        buf.extend_from_slice(&0_u32.to_le_bytes());

        let radice = u32::try_from(messaggio).expect("radice");
        buf[0..4].copy_from_slice(&radice.to_le_bytes());
        buf.resize(buf.len() + 16, 0);
        buf
    }

    /// La convalida dei blocchi **precede** il tetto cumulativo, e il caso lo
    /// **discrimina**.
    ///
    /// Il `bodyLength` del dizionario portato oltre la regione dati e' rilevato
    /// da `validate_footer_blocks`, e con un tetto a zero l'errore dice quale
    /// controllo corre per primo: `IpcFooterInvalid`, non
    /// `IpcRetainedDictionariesTooLarge`.
    #[test]
    fn il_framing_invalido_vince_sul_tetto_dei_dizionari() {
        let mut byte = artefatto_con_dizionario();
        let blocco_dizionario = primo_blocco_dizionario(&byte);
        // Un `bodyLength` enorme ma positivo: il parsing lo accetta (rifiuta i
        // negativi), la convalida lo respinge perche' il blocco finirebbe oltre
        // l'inizio del footer.
        let gonfiato = 1_u64 << 40;
        sostituisci_body_len(&mut byte, blocco_dizionario, gonfiato);

        let mut sorgente: &[u8] = &byte;
        // Tetto a zero: se corresse per primo, la somma gonfiata lo
        // supererebbe e l'errore sarebbe suo.
        let esito = valida_file_ed_estrai(
            &mut sorgente,
            &IpcLimits {
                max_retained_dictionary_body_bytes: 0,
                ..IpcLimits::default()
            },
            None,
        );
        assert!(
            matches!(esito, Err(ArrowTransportError::IpcFooterInvalid(_))),
            "il framing rotto deve vincere sul tetto: {esito:?}"
        );
    }

    /// Lo stesso artefatto **senza** il difetto: qui il tetto tocca a lui, e
    /// con un tetto a zero deve essere lui a rifiutare.
    ///
    /// Dimostra che il caso qui sopra dipende dall'ordine e non da un
    /// artefatto sempre rotto.
    #[test]
    fn senza_difetti_di_framing_il_tetto_dei_dizionari_rifiuta() {
        let byte = artefatto_con_dizionario();
        let mut sorgente: &[u8] = &byte;
        let esito = valida_file_ed_estrai(
            &mut sorgente,
            &IpcLimits {
                max_retained_dictionary_body_bytes: 0,
                ..IpcLimits::default()
            },
            None,
        );
        assert!(
            matches!(
                esito,
                Err(ArrowTransportError::IpcRetainedDictionariesTooLarge { limit: 0, .. })
            ),
            "senza difetti di framing tocca al tetto: {esito:?}"
        );
    }

    #[test]
    fn la_somma_che_trabocca_e_un_rifiuto_esplicito_non_una_saturazione() {
        // Tre blocchi da `i64::MAX`: due soli non basterebbero, perche' la
        // loro somma e' `u64::MAX - 1`.
        let massimo = u64::try_from(i64::MAX).expect("i64::MAX entra in u64");
        let dizionari = [blocco(massimo), blocco(massimo), blocco(massimo)];
        // Tetto `u64::MAX`: una saturazione ACCETTEREBBE l'insieme. L'errore
        // e' quello del footer, perche' non c'e' una somma da dichiarare.
        let esito = verifica_tetto_dizionari(&dizionari, u64::MAX);
        assert!(matches!(
            esito,
            Err(ArrowTransportError::IpcFooterInvalid(_))
        ));
    }
}
