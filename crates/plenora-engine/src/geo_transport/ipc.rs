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
/// Fallisce in overflow invece di saturare. `saturating_add(7) & !7` sembrava
/// prudente ed era il contrario: vicino a `u64::MAX` la somma satura su
/// `u64::MAX` e il mascheramento la riporta **sotto** il valore di partenza —
/// l'offset allineato risultava minore di quello da allineare, e la
/// monotonicita' su cui poggia l'avanzamento del parsing cadeva in silenzio.
/// Con `checked_add` un offset che non e' allineabile a 64 bit e' un
/// messaggio malformato, cioe' un errore di framing esplicito.
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

/// Somma di posizioni dentro il buffer.
///
/// Gli addendi arrivano dal file: un traboccamento e' un riferimento
/// malformato, non un indirizzo. Sommare senza controllo lo farebbe rientrare
/// nel buffer da capo — in release — e in debug farebbe panicare il confine
/// che esiste per non panicare.
///
/// Aritmetica totale **localmente**, anche dove il chiamante ha gia' provato
/// il confine: la garanzia altrui vale finche' resta dove sta.
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
/// Cambia la larghezza, non la regola: ogni posizione fuori dal buffer e' un
/// troncamento, mai un valore inventato. La fine dell'intervallo passa da
/// [`fb_somma`] perche' `pos` arriva dal file, quindi `pos + larghezza` puo'
/// traboccare — e una lettura che panica al posto di rifiutare non e' un
/// confine.
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
/// La somma con `relative` resta controllata anche se su un bersaglio a 64 bit
/// non puo' traboccare — `relative` viene da un `u32` e `campo` sta nel buffer
/// — e nessun test puo' quindi vederla fallire li'. A 32 bit invece i due
/// addendi ci arrivano vicini, ed e' l'unico controllo che li separa: toglierlo
/// perche' «la suite resta verde» significherebbe fidarsi della larghezza di
/// `usize` del bersaglio in cui si e' provato.
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
/// Chi valida soltanto i confini scarta il risultato; chi deve guardare il
/// contenuto — una chiave da confrontare, una lunghezza da misurare — lo usa.
/// Una funzione sola, quindi un solo posto dove il controllo dei confini puo'
/// sbagliare.
///
/// [`fb_vector`] ha gia' provato che `pos + 4 + count` stia nel buffer; la
/// costruzione dell'intervallo resta comunque controllata.
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
/// # Perche' restituisce invece di scartare
///
/// La versione precedente validava e buttava via, e non poteva fare
/// altrimenti: i duplicati sono una proprieta' dell'INSIEME, e chi vede un
/// elemento per volta non li vedra' mai. Restituire le due stringhe sposta il
/// controllo dove c'e' l'informazione per farlo ([`fb_custom_metadata`]).
///
/// # Che cosa rifiuta, e perche' non e' pedanteria
///
/// La versione precedente accettava chiave o valore **assenti**: se l'offset
/// era zero non validava e proseguiva. `arrow-ipc` legge pero' i custom
/// metadata del FOOTER con `key().unwrap()` e `value().unwrap()`, quindi una
/// voce senza chiave o senza valore raggiungeva una primitiva di panic dentro
/// la dipendenza — esattamente cio' che questo confine esiste per impedire.
/// Il percorso dello SCHEMA e' invece difensivo (`if let`), il che rendeva la
/// lacuna invisibile finche' nessuno leggeva il footer.
fn fb_key_value(buf: &[u8], table: usize) -> Result<(&str, &str), ArrowTransportError> {
    /// Che cosa pretendere da uno dei due campi di una coppia.
    ///
    /// Chiave e valore hanno tetti, diagnosi ed esiti diversi: fonderli in un
    /// ciclo con `if index == 0` li rendeva due validazioni travestite da una,
    /// con due rami irraggiungibili per convincere il compilatore.
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
/// # Chi controlla che cosa
///
/// [`fb_key_value`] valida **una** coppia; qui si vede l'intera collezione,
/// quindi qui stanno il tetto sul conteggio — applicato PRIMA del ciclo, cioe'
/// prima di qualunque allocazione proporzionale — e il rifiuto dei duplicati.
///
/// # Chiavi sconosciute
///
/// Accettate e ignorate. Questo confine valida la **forma**, non il
/// vocabolario: rifiutare le chiavi altrui romperebbe l'interoperabilita' con
/// qualunque produttore Arrow che aggiunga le proprie, e non renderebbe
/// nessuno piu' sicuro.
fn fb_custom_metadata(buf: &[u8], table: usize, offset: usize) -> Result<(), ArrowTransportError> {
    fb_custom_metadata_estraendo(buf, table, offset, None).map(|_| ())
}

/// Come [`fb_custom_metadata`], ma **rende** il valore di una chiave cercata.
///
/// Una funzione sola e non due: l'estrazione deve passare per la stessa
/// traversata che convalida, altrimenti esisterebbero due modi di leggere il
/// footer e solo uno sarebbe rinforzato. E' esattamente il motivo per cui il
/// `commit_token` **non** si legge da `FileReader::custom_metadata`: quella e'
/// una terza strada, che di questi controlli non ne fa nessuno.
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
            .get(fb_somma(table, header_type_offset)?)
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
        let value = fb_i64(metadata, fb_somma(table, body_len_offset)?)?;
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
    let fine = align8_u64(metadata_end)
        .and_then(|allineato| allineato.checked_add(body_len))
        .and_then(align8_u64)
        .ok_or(ArrowTransportError::IpcTruncated)?;
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
    valida_file_ed_estrai(source, limits, None).map(|_| ())
}

/// Convalida il file **e** rende il valore di una chiave dei custom metadata
/// del footer.
///
/// E' la stessa funzione di [`validate_ipc_file_framing`], non una seconda
/// lettura: il valore esce dalla traversata rinforzata, quindi non esiste un
/// modo di ottenerlo saltando i controlli. Era la scelta da fare — leggere il
/// token con `FileReader::custom_metadata` avrebbe aperto una terza strada nel
/// footer, e quella non e' rinforzata.
///
/// # Errors
///
/// Come [`validate_ipc_file_framing`].
pub fn valida_file_ed_estrai<S: IpcSource + ?Sized>(
    source: &mut S,
    limits: &IpcLimits,
    chiave: Option<&str>,
) -> Result<Option<String>, ArrowTransportError> {
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
    let (blocks, trovato) = parse_footer_estraendo(&footer, limits, chiave)?;
    // Il valore si copia **prima** di continuare: `footer` e' un buffer locale
    // e `trovato` lo presta.
    let trovato = trovato.map(str::to_owned);
    validate_footer_blocks(source, &blocks, footer_start, limits)?;
    Ok(trovato)
}

/// Percorre il footer: lo Schema (che `fb_to_schema` leggera') e i vettori di
/// `Block` dei dizionari e dei record batch.
///
/// **Solo sotto test.** Da quando la convalida estrae anche un custom metadata,
/// il percorso di produzione passa tutto per [`parse_footer_estraendo`] e questa
/// resta la forma breve per i test che dei soli blocchi hanno bisogno. Il `cfg`
/// lo dichiara invece di lasciare che un `dead_code` lo dica peggio.
#[cfg(test)]
fn parse_footer(
    footer: &[u8],
    limits: &IpcLimits,
) -> Result<Vec<FooterBlock>, ArrowTransportError> {
    parse_footer_estraendo(footer, limits, None).map(|(blocks, _)| blocks)
}

/// Percorre il footer — lo Schema e i vettori di `Block` — e rende anche il
/// valore della chiave cercata fra i custom metadata del footer.
fn parse_footer_estraendo<'a>(
    footer: &'a [u8],
    limits: &IpcLimits,
    cercata: Option<&str>,
) -> Result<(Vec<FooterBlock>, Option<&'a str>), ArrowTransportError> {
    let root = fb_u32(footer, 0)? as usize;
    let (vtable, vtable_len) = fb_table(footer, root)?;
    // Campo 1: lo Schema del footer, OBBLIGATORIO.
    //
    // Stessa classe dei custom metadata: `arrow-ipc` lo legge con
    // `footer.schema().unwrap()`, quindi un footer che non lo porta panica
    // dentro la dipendenza. Il writer lo emette sempre, quindi pretenderlo
    // non rifiuta nessun file legittimo.
    let schema_offset = fb_field(footer, vtable, vtable_len, 1)?;
    if schema_offset == 0 {
        return Err(ArrowTransportError::IpcFooterInvalid("schema assente"));
    }
    fb_schema(footer, fb_indirect(footer, root, schema_offset)?)?;
    let mut blocks: Vec<FooterBlock> = Vec::new();
    // Campo 2: dizionari. Campo 3: record batch. Arrow legge entrambi.
    for field in [2_usize, 3] {
        fb_footer_blocks(footer, root, vtable, vtable_len, field, &mut blocks, limits)?;
    }
    // Campo 4: custom metadata del footer. Arrow li legge, e li legge con
    // `key().unwrap()` / `value().unwrap()`: una voce senza chiave o senza
    // valore panica dentro la dipendenza. Prima di questa riga il campo non
    // era percorso affatto — non lo leggeva nessuno, quindi nessuno lo
    // vedeva.
    let custom = fb_field(footer, vtable, vtable_len, 4)?;
    let trovato = fb_custom_metadata_estraendo(footer, root, custom, cercata)?;
    Ok((blocks, trovato))
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
    // `arrow-ipc` 59.2.0 va in panico dentro `convert::fb_to_schema` su schemi
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
    // dal modulo `barriera_antipanico` in fondo a questo file, che le porta un
    // input costruito apposta: uno stream con una colonna `List` a cui viene
    // tolto il campo `children`.
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
// Custom metadata: i dodici casi strutturali del confine.
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
        /// Campo assente dalla vtable: e' l'offset zero che la versione
        /// precedente lasciava passare, e che fa panicare `arrow-ipc` quando
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
        // significa «campo assente» e la validazione tornerebbe Ok senza
        // guardare niente. La prima stesura di questo banco lo passava a
        // zero, e i dieci casi negativi lo hanno scoperto: i tre positivi da
        // soli sarebbero passati a vuoto.
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
    /// La duplicazione di superficie e' voluta: la variante estraente e' una
    /// seconda porta sullo stesso corridoio, e i controlli vanno provati
    /// attraverso entrambe. Un'estrazione che li saltasse renderebbe
    /// raggiungibile senza convalida esattamente il valore piu' autoritativo
    /// del footer.
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

    // --- 4-5: i campi assenti, che facevano panicare arrow ----------------

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

/// I test diretti su `fb_custom_metadata` non dimostrano che `parse_footer` lo
/// **chiami**: resterebbero verdi anche scollegando il campo 4. Questi
/// costruiscono un file Arrow IPC vero con `FileWriter`, gli mettono custom
/// metadata nel footer con `write_metadata`, e passano dal validatore
/// pubblico.
///
/// I casi di forma — chiave assente, duplicati — non sono costruibili con
/// `FileWriter`, che scrive una mappa e non produce voci malformate: restano
/// ai test diretti. I casi di **tetto** invece si costruiscono, e sono quelli
/// che dimostrano il collegamento.
///
/// La prova e' stata fatta: scollegando il campo 4 da `parse_footer`, tre di
/// questi test diventano rossi e i tredici diretti restano verdi. E' la
/// ragione per cui esistono.
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
// I tre fratelli della stessa classe: campi che arrow dereferenzia con
// `unwrap` e che il confine trattava come opzionali.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod campi_pretesi {
    use super::{
        fb_field_table, fb_schema, parse_footer, ArrowTransportError, IpcLimits, SchemaBudget,
    };

    /// Costruisce una tabella flatbuffer con i soli slot indicati.
    ///
    /// `slot[i] == None` significa campo **assente**, che e' precisamente il
    /// caso che questi test devono produrre: presente-con-valore-zero e
    /// assente sono cose diverse, e solo la seconda fa panicare arrow.
    ///
    /// Torna `(buf, posizione della tabella)`. La tabella e' vuota — nessuno
    /// slot punta a niente — perche' ai tre controlli sotto esame basta
    /// l'assenza.
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

/// `PR-0` ha chiuso quattro punti in cui `arrow-ipc` dereferenzia con `unwrap`
/// un campo che il confine trattava come opzionale. Nel farlo ha tolto la
/// copertura della barriera: l'artefatto di fuzz che la esercitava viene ora
/// rifiutato prima, in modo strutturato.
///
/// La barriera resta pero' necessaria, perche' un quinto punto e' aperto:
/// `convert.rs` pretende i figli dei tipi annidati e ha una ventina fra
/// `panic!` e `unimplemented!` sui codici di tipo
/// ([`errori-e-limiti.md`](../../../../docs/errori-e-limiti.md)). Questo
/// modulo costruisce esattamente quel caso, partendo da uno stream Arrow
/// **vero**.
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

    /// # Nota sull'hook di panico
    ///
    /// Questo test **non** sostituisce l'hook del processo. Il panico di
    /// `arrow-ipc` finisce quindi su stderr, e l'output della suite contiene
    /// una traccia che sembra un fallimento senza esserlo: e' il prezzo, ed e'
    /// preferibile a mutare stato globale mentre gli altri test girano in
    /// parallelo — l'hook e' del processo, non del test, e toglierlo lo
    /// toglierebbe anche a chi non c'entra.
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
/// `pos` arriva dal file: `pos + larghezza` puo' uscire da `usize`. Senza
/// controllo la somma rientrerebbe da capo nel buffer in release, e in debug
/// farebbe panicare proprio il confine che esiste per non panicare. Qui si
/// pretende `IpcTruncated`, che e' cio' che la macro promette.
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
