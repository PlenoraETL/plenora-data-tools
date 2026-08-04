//! Payload Arrow IPC del trasporto v3: pre-validazione strutturale del
//! framing e dei metadati flatbuffer, decodifica e codifica entro i limiti
//! di risorse.

use plenora_core::arrow::array::RecordBatch;
use plenora_core::arrow::ipc::reader::StreamReader;
use plenora_core::arrow::ipc::writer::StreamWriter;
use plenora_core::arrow::schema::SchemaRef;

use super::error::ArrowTransportError;
use super::protocol::{MAX_ROWS, MAX_STREAM_BYTES};
use super::transport::{MAX_BATCHES, MAX_COLUMNS, MAX_IPC_METADATA_BYTES};

const fn align8(value: usize) -> usize {
    value.saturating_add(7) & !7
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
fn fb_field_table(buf: &[u8], table: usize, depth: usize) -> Result<(), ArrowTransportError> {
    if depth > MAX_FLATBUFFER_DEPTH {
        return Err(ArrowTransportError::IpcTruncated);
    }
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
            fb_field_table(buf, child, depth + 1)?;
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
        for index in 0..count {
            let field = fb_indirect(buf, vector + 4, index * 4)?;
            fb_field_table(buf, field, 0)?;
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
fn validate_ipc_message_metadata(metadata: &[u8]) -> Result<usize, ArrowTransportError> {
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
    if let Some(header_table) = header_table {
        match header_type {
            1 => fb_schema(metadata, header_table)?,
            2 => {
                // DictionaryBatch: data (RecordBatch) al campo 1.
                let (dict_vtable, dict_vtable_len) = fb_table(metadata, header_table)?;
                let data = fb_field(metadata, dict_vtable, dict_vtable_len, 1)?;
                if data != 0 {
                    let batch = fb_indirect(metadata, header_table, data)?;
                    fb_record_batch(metadata, batch, metadata.len())?;
                }
            }
            3 => {
                // body_len verificato dopo la lettura di bodyLength.
                let _ = fb_table(metadata, header_table)?;
            }
            _ => {
                return Err(ArrowTransportError::Arrow(
                    "header IPC Tensor/SparseTensor non supportato".to_owned(),
                ))
            }
        }
    }

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

    // Con il body noto, i buffer del RecordBatch devono starci dentro.
    if let (Some(header_table), 3) = (header_table, header_type) {
        fb_record_batch(metadata, header_table, body_len)?;
    }

    let custom = fb_field(metadata, vtable, vtable_len, 4)?;
    fb_custom_metadata(metadata, table, custom)?;
    Ok(body_len)
}

/// Pre-validazione del framing IPC prima che arrow-rs allochi: ogni messaggio
/// dichiara la lunghezza dei propri metadati e il flatbuffer dichiara il
/// body; entrambi devono stare dentro il payload, i metadati entro un tetto
/// assoluto e la struttura flatbuffer entro i propri limiti. Senza questo
/// controllo un payload malevolo induce allocazioni enormi dentro arrow-rs
/// (OOM, trovato via fuzzing).
fn validate_ipc_framing(payload: &[u8]) -> Result<(), ArrowTransportError> {
    let mut offset = 0_usize;
    loop {
        let prefix_bytes: [u8; 4] = payload
            .get(offset..offset + 4)
            .and_then(|bytes| bytes.try_into().ok())
            .ok_or(ArrowTransportError::IpcTruncated)?;
        let prefix = u32::from_le_bytes(prefix_bytes);
        let (metadata_len, header) = if prefix == 0xFFFF_FFFF {
            let length_bytes: [u8; 4] = payload
                .get(offset + 4..offset + 8)
                .and_then(|bytes| bytes.try_into().ok())
                .ok_or(ArrowTransportError::IpcTruncated)?;
            (u32::from_le_bytes(length_bytes) as usize, 8)
        } else {
            (prefix as usize, 4)
        };
        if metadata_len == 0 {
            return Ok(());
        }
        if metadata_len > MAX_IPC_METADATA_BYTES {
            return Err(ArrowTransportError::IpcMetadataTooLarge(metadata_len));
        }
        let metadata_start = offset + header;
        let metadata = payload
            .get(metadata_start..metadata_start + metadata_len)
            .ok_or(ArrowTransportError::IpcTruncated)?;
        let body_len = validate_ipc_message_metadata(metadata)?;
        let end = align8(
            align8(metadata_start + metadata_len)
                .checked_add(body_len)
                .ok_or(ArrowTransportError::IpcTruncated)?,
        );
        if end > payload.len() {
            return Err(ArrowTransportError::IpcTruncated);
        }
        offset = end;
    }
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
