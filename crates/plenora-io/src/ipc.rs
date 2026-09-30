//! Arrow IPC: lettura di file (Feather v2) e stream, scrittura di file.
//!
//! La lettura riconosce file e stream dal contenuto (il file comincia con
//! `ARROW1`), legge tutti i blocchi e li ricompone in un solo `RecordBatch`
//! con lo schema del file, metadati di schema e di campo compresi. Con un
//! solo blocco le colonne restano viste del buffer letto (nessuna copia);
//! con più blocchi `concat_batches` li copia.
//!
//! **Confine** ([`crate::confine`]). Il file si legge intero in un solo
//! buffer allineato e sugli stessi byte, prima di Arrow, si percorre la
//! struttura: prefissi e lunghezze dei messaggi (o footer e blocchi),
//! metadati entro il tetto, corpi e blocchi dentro il file, blocchi del
//! footer non ripetuti né sovrapposti (moltiplicherebbero le righe senza
//! errore), il marcatore di fine dello stream (senza, uno stream tagliato
//! darebbe meno righe senza errore), l'endianness. Poi
//! `FileDecoder`/`StreamDecoder` decodificano per viste dello stesso
//! buffer, dentro la barriera anti-panico. Il contenuto dei messaggi
//! (buffer, nodi, dizionari) resta ad Arrow: i casi che lì allocano oltre il
//! file sono un limite dichiarato (README, «File»).
//!
//! Memoria: i buffer letti sono i byte del file (i dati IPC senza
//! compressione si leggono per viste), e la ricomposizione di più blocchi
//! ne tiene insieme due copie. Prima di leggere si verifica che la
//! dimensione del file stia nel budget residuo, prima di decodificare il
//! doppio se i blocchi sono più di uno, prima di ricomporre i byte dei
//! blocchi decodificati e della loro copia; dopo, i byte vivi esatti.
//!
//! La scrittura procede a blocchi di righe di circa [`BYTE_PER_BLOCCO`]
//! byte, come lo sfratto del runner: `FileWriter` codifica ogni blocco in un
//! vettore prima di scriverlo, e blocchi limitati limitano quel transitorio.
//! Nessuna compressione: i crate Arrow del workspace non la abilitano, e un
//! file IPC compresso si rifiuta in lettura con l'errore di Arrow.

use std::fs::File;
use std::io::{BufReader, Read, Write};
use std::path::Path;
use std::sync::Arc;

use arrow_buffer::{Buffer, MutableBuffer};
use plenora_core::arrow::array::RecordBatch;
use plenora_core::arrow::ipc::reader::{FileDecoder, FileReader, StreamDecoder};
use plenora_core::arrow::ipc::writer::FileWriter;
use plenora_core::arrow::ipc::MessageHeader;
use plenora_core::arrow::schema::SchemaRef;
use plenora_core::arrow::select::concat::concat_batches;
use plenora_core::contract::arrow_schema::verifica_tipi_supportati;
use plenora_core::memoria::byte_vivi;
use plenora_core::{PlenoraError, Result};

use crate::confine::{
    barriera, malformato, oltre_il_limite, verifica_metadati_custom, LimitiLettura,
};
use crate::memoria::{oltre_il_budget, stima_byte};

/// Byte di dati per blocco scritto.
pub const BYTE_PER_BLOCCO: u64 = 8 * 1024 * 1024;

const MAGIA_FILE: &[u8; 6] = b"ARROW1";
const MAGIA_FEATHER_V1: &[u8; 4] = b"FEA1";
const CONTINUAZIONE: [u8; 4] = [0xFF; 4];
/// `ARROW1` più due byte di allineamento, in testa al formato file.
const TESTA_FILE: usize = 8;
const IPC: &str = "file Arrow IPC";

/// Legge un file Arrow IPC (file o stream) in un solo `RecordBatch`.
///
/// `residuo` è il budget che la tabella può occupare; `limiti` quelli del
/// confine di lettura.
///
/// # Errors
///
/// `ResourceLimit` se il file, la tabella letta, i metadati o i blocchi non
/// stanno in `residuo` o in `limiti`; `Unsupported` per Feather v1 e per
/// un'endianness diversa; `DataMapping` per un file malformato (anche
/// quando Arrow va in panico); `Io` dalla lettura.
pub fn leggi(percorso: &Path, residuo: u64, limiti: &LimitiLettura) -> Result<RecordBatch> {
    let mut file = File::open(percorso)?;
    let lunghezza = file.metadata()?.len();
    if lunghezza > residuo {
        return Err(oltre_il_budget(lunghezza, residuo));
    }
    let n = usize::try_from(lunghezza)
        .map_err(|_| oltre_il_limite("byte del file", lunghezza, usize::MAX as u64))?;
    // Il file intero, in un buffer allineato: le colonne ne saranno viste.
    let mut letto = MutableBuffer::from_len_zeroed(n);
    file.read_exact(letto.as_slice_mut())?;
    let byte: Buffer = letto.into();
    if byte.starts_with(MAGIA_FEATHER_V1) {
        return Err(PlenoraError::Unsupported(
            "Feather v1 non supportato: solo Feather v2 (Arrow IPC)".to_owned(),
        ));
    }
    let tetto = limiti.metadati_entro(residuo);
    let (schema, blocchi) = if byte.starts_with(MAGIA_FILE) {
        decodifica_file(&byte, residuo, tetto, limiti)?
    } else {
        decodifica_stream(&byte, residuo, tetto, limiti)?
    };
    let tabella = match blocchi.len() {
        0 => RecordBatch::new_empty(schema),
        1 => blocchi
            .into_iter()
            .next()
            .ok_or_else(|| PlenoraError::Internal("blocco atteso e assente".to_owned()))?,
        _ => {
            // Blocchi decodificati (viste) più la copia di `concat_batches`.
            let stima = byte_vivi(blocchi.iter())?.saturating_add(stima_byte(&blocchi));
            if stima > residuo {
                return Err(oltre_il_budget(stima, residuo));
            }
            barriera("arrow-select", || Ok(concat_batches(&schema, &blocchi)?))?
        }
    };
    let vivi = byte_vivi(std::iter::once(&tabella))?;
    if vivi > residuo {
        return Err(oltre_il_budget(vivi, residuo));
    }
    Ok(tabella)
}

/// Blocchi entro il massimo, e il file due volte nel budget se sono più di
/// uno (ricomposizione).
fn verifica_blocchi(n: usize, blocchi: u64, residuo: u64, limiti: &LimitiLettura) -> Result<()> {
    if blocchi > limiti.max_blocchi {
        return Err(oltre_il_limite(
            "blocchi del file IPC",
            blocchi,
            limiti.max_blocchi,
        ));
    }
    let servono = u64::try_from(n).unwrap_or(u64::MAX).saturating_mul(2);
    if blocchi > 1 && servono > residuo {
        return Err(oltre_il_budget(servono, residuo));
    }
    Ok(())
}

/// Lo schema del file dentro la barriera, con endianness, tipi e metadati
/// verificati. `StreamDecoder` non guarda l'endianness: un file big-endian
/// si decodificherebbe come little-endian, con altri valori.
fn schema_verificato(
    schema: plenora_core::arrow::ipc::Schema<'_>,
    limiti: &LimitiLettura,
) -> Result<SchemaRef> {
    if !schema.endianness().equals_to_target_endianness() {
        return Err(PlenoraError::Unsupported(
            "Arrow IPC con endianness diversa da quella del sistema".to_owned(),
        ));
    }
    let schema = barriera("arrow-ipc", || {
        Ok(plenora_core::arrow::ipc::convert::fb_to_schema(schema))
    })?;
    verifica_tipi_supportati(&schema)?;
    verifica_metadati_custom(&schema, 0, limiti.max_byte_metadati_custom)?;
    Ok(Arc::new(schema))
}

/// Intero con segno a 32 bit little-endian in `byte[da..da + 4]`.
fn i32_in(byte: &[u8], da: usize) -> Result<i32> {
    da.checked_add(4)
        .and_then(|fine| byte.get(da..fine))
        .and_then(|fetta| <[u8; 4]>::try_from(fetta).ok())
        .map(i32::from_le_bytes)
        .ok_or_else(|| malformato(IPC, "messaggio troncato"))
}

/// Formato stream: percorre prefissi e lunghezze dei messaggi fino al
/// marcatore di fine, poi decodifica con `StreamDecoder`.
fn decodifica_stream(
    byte: &Buffer,
    residuo: u64,
    tetto: u64,
    limiti: &LimitiLettura,
) -> Result<(SchemaRef, Vec<RecordBatch>)> {
    let mut schema = None;
    let mut blocchi = 0_u64;
    let mut posizione = 0_usize;
    loop {
        // Continuazione e lunghezza, o solo la lunghezza (formato
        // precedente); lunghezza zero: fine dello stream.
        let continua = byte
            .get(posizione..)
            .is_some_and(|resto| resto.starts_with(&CONTINUAZIONE));
        let prefisso = if continua { 8 } else { 4 };
        let lunghezza = usize::try_from(i32_in(byte, posizione + prefisso - 4)?)
            .map_err(|_| malformato(IPC, "lunghezza dei metadati negativa"))?;
        let inizio = posizione + prefisso;
        if lunghezza == 0 {
            if inizio != byte.len() {
                return Err(malformato(IPC, "byte dopo la fine dello stream"));
            }
            break;
        }
        let dichiarati = u64::try_from(lunghezza).unwrap_or(u64::MAX);
        if dichiarati > tetto {
            return Err(oltre_il_limite(
                "metadati di un messaggio IPC (byte)",
                dichiarati,
                tetto,
            ));
        }
        let metadati = byte
            .get(inizio..inizio.saturating_add(lunghezza))
            .ok_or_else(|| malformato(IPC, "metadati oltre la fine del file"))?;
        let messaggio = plenora_core::arrow::ipc::root_as_message(metadati)
            .map_err(|_| malformato(IPC, "metadati del messaggio non verificabili"))?;
        match (messaggio.header_type(), &schema) {
            (MessageHeader::Schema, None) => {
                let intestazione = messaggio
                    .header_as_schema()
                    .ok_or_else(|| malformato(IPC, "schema senza intestazione"))?;
                schema = Some(schema_verificato(intestazione, limiti)?);
            }
            (MessageHeader::Schema, Some(_)) => {
                return Err(malformato(IPC, "schema ripetuto nello stream"));
            }
            (_, None) => return Err(malformato(IPC, "messaggio prima dello schema")),
            (MessageHeader::RecordBatch, Some(_)) => blocchi = blocchi.saturating_add(1),
            (MessageHeader::DictionaryBatch, Some(_)) => {}
            // `StreamDecoder` salta in silenzio un messaggio `NONE`: un blocco
            // col tipo cambiato sparirebbe con le sue righe.
            (_, Some(_)) => {
                return Err(malformato(
                    IPC,
                    "messaggio di tipo non ammesso nello stream",
                ));
            }
        }
        posizione = usize::try_from(messaggio.bodyLength())
            .ok()
            .and_then(|corpo| (inizio + lunghezza).checked_add(corpo))
            .filter(|fine| *fine <= byte.len())
            .ok_or_else(|| malformato(IPC, "corpo del messaggio oltre la fine del file"))?;
        if posizione == byte.len() {
            // Arrow accetta uno stream senza marcatore di fine; qui no: uno
            // stream tagliato fra due messaggi darebbe meno righe.
            return Err(malformato(IPC, "stream senza marcatore di fine (troncato)"));
        }
    }
    let schema = schema.ok_or_else(|| malformato(IPC, "stream senza schema"))?;
    verifica_blocchi(byte.len(), blocchi, residuo, limiti)?;
    let mut decodificatore = StreamDecoder::new();
    let mut resto = byte.clone();
    let mut letti = Vec::new();
    barriera("arrow-ipc", || {
        while !resto.is_empty() {
            if let Some(blocco) = decodificatore.decode(&mut resto)? {
                letti.push(blocco);
            }
        }
        Ok(decodificatore.finish()?)
    })?;
    Ok((schema, letti))
}

/// Un blocco del footer: dentro la zona dei dati, metadati entro il tetto,
/// e coerente col suo messaggio. Rende la zona del blocco.
///
/// Il blocco deve dire del suo messaggio quello che il messaggio dice di sé:
/// `FileDecoder` prende il corpo dall'offset del blocco e ignora il prefisso,
/// e un blocco spostato leggerebbe metadati come valori.
fn verifica_blocco(
    byte: &Buffer,
    blocco: &plenora_core::arrow::ipc::Block,
    dizionario: bool,
    fine_dati: usize,
    tetto: u64,
) -> Result<(usize, usize)> {
    let fuori = || malformato(IPC, "blocco del footer fuori dalla zona dei dati");
    let inizio = usize::try_from(blocco.offset()).map_err(|_| fuori())?;
    let metadati = usize::try_from(blocco.metaDataLength()).map_err(|_| fuori())?;
    let corpo = usize::try_from(blocco.bodyLength()).map_err(|_| fuori())?;
    let fine = inizio
        .checked_add(metadati)
        .and_then(|fine| fine.checked_add(corpo))
        .filter(|fine| inizio >= TESTA_FILE && metadati >= 8 && *fine <= fine_dati)
        .ok_or_else(fuori)?;
    let dichiarati = u64::try_from(metadati).unwrap_or(u64::MAX);
    if dichiarati > tetto {
        return Err(oltre_il_limite(
            "metadati di un messaggio IPC (byte)",
            dichiarati,
            tetto,
        ));
    }
    let prefisso = if byte[inizio..].starts_with(&CONTINUAZIONE) {
        8
    } else {
        4
    };
    let interni = usize::try_from(i32_in(byte, inizio + prefisso - 4)?).map_err(|_| fuori())?;
    if prefisso.checked_add(interni) != Some(metadati) {
        return Err(malformato(
            IPC,
            "metadati del blocco diversi dal suo messaggio",
        ));
    }
    let messaggio =
        plenora_core::arrow::ipc::root_as_message(&byte[inizio + prefisso..inizio + metadati])
            .map_err(|_| malformato(IPC, "metadati del messaggio non verificabili"))?;
    let atteso = if dizionario {
        MessageHeader::DictionaryBatch
    } else {
        MessageHeader::RecordBatch
    };
    if messaggio.header_type() != atteso || messaggio.bodyLength() != blocco.bodyLength() {
        return Err(malformato(
            IPC,
            "messaggio diverso dal suo blocco del footer",
        ));
    }
    Ok((inizio, fine))
}

/// Formato file: footer entro il tetto, blocchi dentro la zona dei dati e
/// disgiunti, poi `FileDecoder` sulle fette dei blocchi.
fn decodifica_file(
    byte: &Buffer,
    residuo: u64,
    tetto: u64,
    limiti: &LimitiLettura,
) -> Result<(SchemaRef, Vec<RecordBatch>)> {
    let n = byte.len();
    if n < TESTA_FILE + 10 || !byte.ends_with(MAGIA_FILE) {
        return Err(malformato(IPC, "coda ARROW1 assente o file troppo corto"));
    }
    let fine_footer = n - 10;
    let lunghezza = usize::try_from(i32_in(byte, fine_footer)?)
        .map_err(|_| malformato(IPC, "lunghezza del footer negativa"))?;
    let dichiarati = u64::try_from(lunghezza).unwrap_or(u64::MAX);
    if dichiarati > tetto {
        return Err(oltre_il_limite("footer IPC (byte)", dichiarati, tetto));
    }
    let fine_dati = fine_footer
        .checked_sub(lunghezza)
        .filter(|inizio| *inizio >= TESTA_FILE)
        .ok_or_else(|| malformato(IPC, "footer più lungo del file"))?;
    let footer = plenora_core::arrow::ipc::root_as_footer(&byte[fine_dati..fine_footer])
        .map_err(|_| malformato(IPC, "footer non verificabile"))?;
    let intestazione = footer
        .schema()
        .ok_or_else(|| malformato(IPC, "footer senza schema"))?;
    let lotti = footer
        .recordBatches()
        .ok_or_else(|| malformato(IPC, "footer senza elenco dei blocchi"))?;
    verifica_blocchi(
        n,
        u64::try_from(lotti.len()).unwrap_or(u64::MAX),
        residuo,
        limiti,
    )?;
    let schema = schema_verificato(intestazione, limiti)?;
    // Ogni blocco (dizionari, poi dati) in una zona sua della zona dei dati:
    // un footer che ripete o sovrappone i blocchi moltiplicherebbe le righe.
    let dizionari = footer.dictionaries().into_iter().flatten();
    let mut fette = Vec::new();
    let mut zone = Vec::new();
    for (dizionario, blocco) in dizionari
        .map(|b| (true, b))
        .chain(lotti.iter().map(|b| (false, b)))
    {
        let (inizio, fine) = verifica_blocco(byte, blocco, dizionario, fine_dati, tetto)?;
        zone.push((inizio, fine));
        fette.push((
            dizionario,
            *blocco,
            byte.slice_with_length(inizio, fine - inizio),
        ));
    }
    zone.sort_unstable();
    if zone.windows(2).any(|coppia| coppia[1].0 < coppia[0].1) {
        return Err(malformato(IPC, "blocchi del footer ripetuti o sovrapposti"));
    }
    let mut decodificatore = FileDecoder::new(Arc::clone(&schema), footer.version());
    let mut decodificati = Vec::with_capacity(lotti.len());
    barriera("arrow-ipc", || {
        for (dizionario, blocco, fetta) in &fette {
            if *dizionario {
                decodificatore.read_dictionary(blocco, fetta)?;
            } else {
                let letto = decodificatore
                    .read_record_batch(blocco, fetta)?
                    .ok_or_else(|| malformato(IPC, "blocco senza dati"))?;
                decodificati.push(letto);
            }
        }
        Ok(())
    })?;
    Ok((schema, decodificati))
}

/// Righe per blocco: circa [`BYTE_PER_BLOCCO`] byte di dati ciascuno.
fn righe_per_blocco(tabella: &RecordBatch) -> usize {
    let righe = tabella.num_rows();
    let dati = u64::try_from(plenora_core::memoria::byte_dati(tabella))
        .unwrap_or(u64::MAX)
        .max(1);
    let per_blocco = u128::from(BYTE_PER_BLOCCO)
        * u128::from(u64::try_from(righe).unwrap_or(u64::MAX).max(1))
        / u128::from(dati);
    usize::try_from(per_blocco).unwrap_or(usize::MAX).max(1)
}

/// Scrive la tabella come file Arrow IPC.
///
/// # Errors
///
/// `DataMapping` (Arrow) e `Io` dalla codifica e dalla scrittura.
pub fn scrivi(tabella: &RecordBatch, uscita: impl Write) -> Result<()> {
    let mut scrittore = FileWriter::try_new(uscita, &tabella.schema())?;
    let righe = tabella.num_rows();
    if tabella.num_columns() == 0 {
        // Senza colonne il blocco porta solo il numero di righe.
        if righe > 0 {
            scrittore.write(tabella)?;
        }
    } else {
        let per_blocco = righe_per_blocco(tabella);
        let mut inizio = 0;
        while inizio < righe {
            let lunghezza = per_blocco.min(righe - inizio);
            scrittore.write(&tabella.slice(inizio, lunghezza))?;
            inizio += lunghezza;
        }
    }
    scrittore.finish()?;
    Ok(())
}

/// Transitorio previsto della scrittura: il doppio del blocco più grande
/// (il vettore di codifica cresce per raddoppi), misurato sui blocchi veri
/// con i valori dei dizionari interi, più un margine.
#[must_use]
pub fn transitorio_scrittura(tabella: &RecordBatch) -> u64 {
    crate::memoria::fetta_massima(tabella, righe_per_blocco(tabella))
        .saturating_mul(2)
        .saturating_add(crate::memoria::MARGINE)
}

/// Rilegge il footer di un file appena scritto: lo schema deve essere quello
/// scritto.
///
/// # Errors
///
/// `Schema` se lo schema del file è diverso; `Io`, `DataMapping` dalla
/// lettura del footer.
pub fn verifica_schema(percorso: &Path, scritto: &SchemaRef) -> Result<()> {
    let lettore = FileReader::try_new(BufReader::new(File::open(percorso)?), None)?;
    if lettore.schema() != *scritto {
        return Err(PlenoraError::Schema(
            "schema del file IPC scritto diverso da quello della tabella".to_owned(),
        ));
    }
    Ok(())
}
