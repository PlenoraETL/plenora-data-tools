//! Oracolo dei digest per riga (`md5_hash`, `sha256_hash`,
//! `stable_fingerprint`, `hmac_sha256`): le funzioni com'erano prima del
//! calcolo a chunk paralleli, copiate alla lettera (solo i nomi pubblici
//! hanno il suffisso `_riferimento`), e i confronti degli esiti completi
//! (batch per byte, errori per categoria, messaggio e diagnostica).

use std::collections::HashSet;
use std::fmt::Write as _;
use std::sync::Arc;

use md5::{Digest, Md5};
use plenora_core::arrow::array::{
    builder::StringBuilder, Array, BooleanArray, Float64Array, Int64Array, RecordBatch,
    StringArray, UInt64Array,
};
use plenora_core::arrow::schema::DataType;
use sha2::Sha256;

use super::{
    FingerprintAlgorithm, HashNullPolicy, HmacNullPolicy, HmacSha256, Md5Hash, Sha256Hash,
    StableFingerprint,
};
use crate::{
    column_index, reject_rows, replace_or_append, scalar_as_string, validate_output_name,
    RowRejection,
};
use plenora_core::{PlenoraError, Result};

// ---------------------------------------------------------------------------
// Copia letterale del percorso precedente.
// ---------------------------------------------------------------------------

fn reject_null_hash_rows(batch: &RecordBatch, columns: &[String], indices: &[usize]) -> Result<()> {
    let mut rejections = Vec::new();
    for row in 0..batch.num_rows() {
        if let Some((column, _)) = columns
            .iter()
            .zip(indices)
            .find(|(_, index)| crate::is_logically_null(batch.column(**index).as_ref(), row))
        {
            rejections.push(RowRejection {
                row,
                cause: "validation.required_value_missing",
                column: Some(column),
            });
        }
    }
    reject_rows(
        &rejections,
        "righe non conformi; consultare row_diagnostics",
    )
}

/// Colonna con l'hash MD5 (esadecimale) delle colonne di `config.columns`.
///
/// I nomi sono ordinati e deduplicati; i valori sono concatenati con
/// separatore U+001F e, con `normalize`, trimmati e portati in minuscolo.
///
/// # Errors
///
/// - `InvalidPlan`: nome della colonna di output non valido, `columns` vuoto,
///   valore non rappresentabile come testo (come `scalar_as_string`);
/// - `DataMapping`: null sorgente con `null_policy` `error`, con row
///   diagnostics;
/// - `Schema`: colonna assente dal batch o tipo non coperto dal profilo
///   scalare.
pub fn md5_hash_riferimento(batch: &RecordBatch, config: &Md5Hash) -> Result<RecordBatch> {
    validate_output_name(&config.output_column)?;
    if config.columns.is_empty() {
        return Err(PlenoraError::InvalidPlan(
            "md5_hash richiede colonne".into(),
        ));
    }
    let mut columns = config.columns.clone();
    columns.sort();
    columns.dedup();
    let indices = columns
        .iter()
        .map(|name| column_index(batch, name))
        .collect::<Result<Vec<_>>>()?;
    // Il pre-rifiuto row-scoped vale solo per null_policy=error;
    // empty (default) e literal hanno semantica storica dichiarata.
    if matches!(config.null_policy, HashNullPolicy::Error) {
        reject_null_hash_rows(batch, &columns, &indices)?;
    }
    let values = (0..batch.num_rows())
        .map(|row| {
            let parts = indices
                .iter()
                .map(|index| {
                    let value = scalar_as_string(batch.column(*index).as_ref(), row)?;
                    let value = match (value, &config.null_policy) {
                        (Some(value), _) => value,
                        (None, HashNullPolicy::Empty) => String::new(),
                        (None, HashNullPolicy::Literal) => config.null_literal.clone(),
                        (None, HashNullPolicy::Error) => {
                            return Err(PlenoraError::Internal(
                                "prevalidazione null md5_hash incoerente".into(),
                            ));
                        }
                    };
                    Ok(if config.normalize {
                        value.trim().to_lowercase()
                    } else {
                        value
                    })
                })
                .collect::<Result<Vec<_>>>()?;
            let mut digest = Md5::new();
            digest.update(parts.join("\u{1f}").as_bytes());
            let mut hex = String::new();
            plenora_core::esadecimale::aggiungi_esadecimale(&mut hex, &digest.finalize());
            Ok(hex)
        })
        .collect::<Result<Vec<_>>>()?;
    replace_or_append(
        batch,
        &config.output_column,
        DataType::Utf8,
        false,
        Arc::new(StringArray::from(values)),
    )
}

fn framed_part(digest: &mut Sha256, value: &[u8]) -> Result<()> {
    let length = u64::try_from(value.len())
        .map_err(|_| PlenoraError::ResourceLimit("sha256_hash: valore troppo grande".into()))?;
    digest.update(length.to_be_bytes());
    digest.update(value);
    Ok(())
}

/// Colonna con l'hash SHA-256 (esadecimale) delle colonne configurate.
///
/// Ogni parte e' framed (lunghezza u64 big-endian + valore) dopo il
/// separatore di dominio `plenora-sha256-v1`, con byte di presenza per i
/// null: nessuna collisione per concatenazione.
///
/// # Errors
///
/// - `InvalidPlan`: nome della colonna di output non valido, valore oltre
///   `u64` nel framing, valore non rappresentabile come testo (come
///   `scalar_as_string`);
/// - `DataMapping`: null sorgente con `null_policy` `error`, con row
///   diagnostics;
/// - `Schema`: colonna assente dal batch o tipo non coperto dal profilo
///   scalare.
pub fn sha256_hash_riferimento(batch: &RecordBatch, config: &Sha256Hash) -> Result<RecordBatch> {
    validate_output_name(&config.output_column)?;
    let mut names = config.columns.clone();
    names.sort();
    let indices = names
        .iter()
        .map(|name| column_index(batch, name))
        .collect::<Result<Vec<_>>>()?;
    // Come `md5_hash`: pre-rifiuto solo per null_policy=error.
    if matches!(config.null_policy, HashNullPolicy::Error) {
        reject_null_hash_rows(batch, &names, &indices)?;
    }
    let values = (0..batch.num_rows())
        .map(|row| {
            let mut digest = Sha256::new();
            digest.update(b"plenora-sha256-v1\0");
            for (name, index) in names.iter().zip(&indices) {
                framed_part(&mut digest, name.as_bytes())?;
                framed_part(
                    &mut digest,
                    batch.column(*index).data_type().to_string().as_bytes(),
                )?;
                let value = scalar_as_string(batch.column(*index).as_ref(), row)?;
                match (value, &config.null_policy) {
                    (Some(value), _) => {
                        digest.update([1]);
                        let value = if config.normalize {
                            value.trim().to_lowercase()
                        } else {
                            value
                        };
                        framed_part(&mut digest, value.as_bytes())?;
                    }
                    (None, HashNullPolicy::Empty) => {
                        digest.update([1]);
                        framed_part(&mut digest, b"")?;
                    }
                    (None, HashNullPolicy::Literal) => {
                        digest.update([1]);
                        let literal = if config.normalize {
                            config.null_literal.trim().to_lowercase()
                        } else {
                            config.null_literal.clone()
                        };
                        framed_part(&mut digest, literal.as_bytes())?;
                    }
                    (None, HashNullPolicy::Error) => {
                        return Err(PlenoraError::Internal(
                            "prevalidazione null sha256_hash incoerente".into(),
                        ));
                    }
                }
            }
            let mut hex = String::new();
            plenora_core::esadecimale::aggiungi_esadecimale(&mut hex, &digest.finalize());
            Ok(hex)
        })
        .collect::<Result<Vec<_>>>()?;
    replace_or_append(
        batch,
        &config.output_column,
        DataType::Utf8,
        false,
        Arc::new(StringArray::from(values)),
    )
}

/// Frame lunghezza+valore (u64 big-endian) accumulato in un buffer di byte.
///
/// Nessuna ambiguita' di concatenazione tra parti adiacenti. Usato per i
/// frame costanti per colonna (precomputati una volta per batch) e per i
/// messaggi per riga di `stable_fingerprint`.
fn framed_vec(message: &mut Vec<u8>, value: &[u8], op: &str) -> Result<()> {
    let length = u64::try_from(value.len())
        .map_err(|_| PlenoraError::ResourceLimit(format!("{op}: valore troppo grande")))?;
    message.extend_from_slice(&length.to_be_bytes());
    message.extend_from_slice(value);
    Ok(())
}

/// Accesso tipizzato a una colonna, risolto una sola volta per batch.
///
/// Evita la catena di downcast di `scalar_as_string` a ogni cella e le sue
/// allocazioni sui tipi piu' comuni (Utf8 in prestito, numerici formattati
/// in un buffer riusato). Gli altri tipi ricadono su `scalar_as_string`,
/// invariato. I byte prodotti sono identici in tutti i percorsi.
enum ColumnAccess<'a> {
    Utf8(&'a StringArray),
    Int64(&'a Int64Array),
    Float64(&'a Float64Array),
    Boolean(&'a BooleanArray),
    UInt64(&'a UInt64Array),
    Scalar(&'a dyn Array),
}

fn column_access(array: &dyn Array) -> ColumnAccess<'_> {
    if let Some(values) = array.as_any().downcast_ref::<StringArray>() {
        return ColumnAccess::Utf8(values);
    }
    if let Some(values) = array.as_any().downcast_ref::<Int64Array>() {
        return ColumnAccess::Int64(values);
    }
    if let Some(values) = array.as_any().downcast_ref::<Float64Array>() {
        return ColumnAccess::Float64(values);
    }
    if let Some(values) = array.as_any().downcast_ref::<BooleanArray>() {
        return ColumnAccess::Boolean(values);
    }
    if let Some(values) = array.as_any().downcast_ref::<UInt64Array>() {
        return ColumnAccess::UInt64(values);
    }
    ColumnAccess::Scalar(array)
}

/// Valore testuale canonico della cella, passato in prestito a `consume`
/// (dall'array o dal buffer `scratch` riusato).
///
/// Stessi byte e stessi null di `scalar_as_string` (`write!` usa lo stesso
/// `Display` di `to_string`).
fn with_cell_value<R>(
    access: &ColumnAccess<'_>,
    row: usize,
    scratch: &mut String,
    consume: impl FnOnce(Option<&str>) -> Result<R>,
) -> Result<R> {
    match access {
        ColumnAccess::Utf8(values) => {
            if values.is_null(row) {
                consume(None)
            } else {
                consume(Some(values.value(row)))
            }
        }
        ColumnAccess::Int64(values) => {
            if values.is_null(row) {
                return consume(None);
            }
            scratch.clear();
            let _ = write!(scratch, "{}", values.value(row));
            consume(Some(scratch))
        }
        ColumnAccess::Float64(values) => {
            if values.is_null(row) {
                return consume(None);
            }
            scratch.clear();
            let _ = write!(scratch, "{}", values.value(row));
            consume(Some(scratch))
        }
        ColumnAccess::Boolean(values) => {
            if values.is_null(row) {
                return consume(None);
            }
            scratch.clear();
            let _ = write!(scratch, "{}", values.value(row));
            consume(Some(scratch))
        }
        ColumnAccess::UInt64(values) => {
            if values.is_null(row) {
                return consume(None);
            }
            scratch.clear();
            let _ = write!(scratch, "{}", values.value(row));
            consume(Some(scratch))
        }
        ColumnAccess::Scalar(array) => match scalar_as_string(*array, row)? {
            Some(value) => consume(Some(&value)),
            None => consume(None),
        },
    }
}

/// Codifica canonica di una riga, byte esatti:
///
/// - separatore di dominio `b"plenora-fingerprint-v1\0"`;
/// - per ogni colonna, nell'ordine di config (o dello schema se omessa):
///   `framed(nome)`, `framed(tipo Arrow)`, poi un byte di presenza: `0x00`
///   per null, `0x01` seguito da `framed(valore)` per un valore;
/// - il valore e' la rappresentazione testuale di `scalar_as_string`, senza
///   alcuna normalizzazione (trim/case): null e stringa vuota restano
///   distinti, e righe diverse non collidono per costruzione.
///
/// Determinismo assoluto: stessi byte in input -> stesso digest su qualunque
/// run o macchina (gli algoritmi sono sha2/md5 su un byte stream fisso).
fn fingerprint_rows<D: Digest>(
    batch: &RecordBatch,
    names: &[String],
    indices: &[usize],
) -> Result<StringArray> {
    // Framing per colonna e accesso tipizzato precomputati una volta per
    // batch (`data_type().to_string()` alloca); messaggio e hex in buffer
    // riusati, un solo `update` per riga. Il byte stream e' l'encoding
    // canonico documentato sopra.
    let mut headers = Vec::with_capacity(names.len());
    let mut accesses = Vec::with_capacity(names.len());
    for (name, index) in names.iter().zip(indices) {
        let column = batch.column(*index);
        let data_type = column.data_type().to_string();
        let mut header = Vec::with_capacity(name.len() + data_type.len() + 16);
        framed_vec(&mut header, name.as_bytes(), "stable_fingerprint")?;
        framed_vec(&mut header, data_type.as_bytes(), "stable_fingerprint")?;
        headers.push(header);
        accesses.push(column_access(column.as_ref()));
    }
    let hex_len = <D as Digest>::output_size() * 2;
    let mut builder =
        StringBuilder::with_capacity(batch.num_rows(), batch.num_rows().saturating_mul(hex_len));
    let mut message = Vec::with_capacity(256);
    let mut scratch = String::new();
    let mut hex = String::with_capacity(hex_len);
    for row in 0..batch.num_rows() {
        message.clear();
        message.extend_from_slice(b"plenora-fingerprint-v1\0");
        for (header, access) in headers.iter().zip(&accesses) {
            message.extend_from_slice(header);
            with_cell_value(access, row, &mut scratch, |value| {
                match value {
                    Some(value) => {
                        message.push(1);
                        framed_vec(&mut message, value.as_bytes(), "stable_fingerprint")?;
                    }
                    None => message.push(0),
                }
                Ok(())
            })?;
        }
        let mut digest = D::new();
        digest.update(&message);
        hex.clear();
        plenora_core::esadecimale::aggiungi_esadecimale(&mut hex, &digest.finalize());
        builder.append_value(&hex);
    }
    Ok(builder.finish())
}

/// Colonna con il fingerprint stabile per riga (sha256 o md5).
///
/// L'encoding canonico per riga e' documentato in `fingerprint_rows`;
/// `columns` vuoto usa tutte le colonne dello schema, nell'ordine dello
/// schema.
///
/// # Errors
///
/// - `InvalidPlan`: nome della colonna di output non valido, colonna ripetuta
///   in `columns`, nessuna colonna disponibile (config vuota su schema senza
///   colonne), valore oltre `u64` nel framing, valore non rappresentabile
///   come testo (come `scalar_as_string`);
/// - `Schema`: colonna assente dal batch.
pub fn stable_fingerprint_riferimento(
    batch: &RecordBatch,
    config: &StableFingerprint,
) -> Result<RecordBatch> {
    validate_output_name(&config.output_column)?;
    let names: Vec<String> = if config.columns.is_empty() {
        batch
            .schema()
            .fields()
            .iter()
            .map(|field| field.name().clone())
            .collect()
    } else {
        let mut seen = HashSet::new();
        for name in &config.columns {
            if !seen.insert(name.as_str()) {
                return Err(PlenoraError::InvalidPlan(format!(
                    "stable_fingerprint: colonna ripetuta: {name}"
                )));
            }
        }
        config.columns.clone()
    };
    if names.is_empty() {
        return Err(PlenoraError::InvalidPlan(
            "stable_fingerprint richiede almeno una colonna".into(),
        ));
    }
    let indices = names
        .iter()
        .map(|name| column_index(batch, name))
        .collect::<Result<Vec<_>>>()?;
    let values = match config.algorithm {
        FingerprintAlgorithm::Sha256 => fingerprint_rows::<Sha256>(batch, &names, &indices)?,
        FingerprintAlgorithm::Md5 => fingerprint_rows::<Md5>(batch, &names, &indices)?,
    };
    replace_or_append(
        batch,
        &config.output_column,
        DataType::Utf8,
        false,
        Arc::new(values),
    )
}

/// HMAC-SHA256 (RFC 2104) implementato sopra `sha2` — due round di hash con
/// ipad/opad su blocco da 64 byte, nessuna dipendenza aggiuntiva.
///
/// Gli stati Sha256 dopo l'assorbimento di ipad/opad dipendono SOLO dalla
/// chiave: sono precomputati una volta per batch e clonati per riga (il
/// byte stream assorbito e' identico alla versione byte-per-byte).
fn hmac_sha256_states(key: &[u8]) -> (Sha256, Sha256) {
    const BLOCK: usize = 64;
    let mut block = [0_u8; BLOCK];
    if key.len() > BLOCK {
        let hashed = Sha256::digest(key);
        block[..hashed.len()].copy_from_slice(&hashed);
    } else {
        block[..key.len()].copy_from_slice(key);
    }
    let mut ipad = [0_u8; BLOCK];
    let mut opad = [0_u8; BLOCK];
    for (index, byte) in block.iter().enumerate() {
        ipad[index] = byte ^ 0x36;
        opad[index] = byte ^ 0x5c;
    }
    let mut inner = Sha256::new();
    inner.update(ipad);
    let mut outer = Sha256::new();
    outer.update(opad);
    (inner, outer)
}

/// HMAC di un messaggio a partire dagli stati precomputati (clone per riga).
fn hmac_sha256_with_states(inner_base: &Sha256, outer_base: &Sha256, message: &[u8]) -> [u8; 32] {
    let mut inner = inner_base.clone();
    inner.update(message);
    let inner = inner.finalize();
    let mut outer = outer_base.clone();
    outer.update(inner);
    let digest = outer.finalize();
    let mut output = [0_u8; 32];
    output.copy_from_slice(&digest);
    output
}

/// Legge la chiave dalla variabile d'ambiente indicata. L'errore e'
/// volutamente generico: non rivela ne' il nome della variabile ne' alcun
/// frammento del valore.
fn load_hmac_key(key_env: &str) -> Result<Vec<u8>> {
    match std::env::var(key_env) {
        Ok(value) if !value.is_empty() => Ok(value.into_bytes()),
        _ => Err(PlenoraError::InvalidPlan(
            "hmac_sha256: chiave HMAC non disponibile".into(),
        )),
    }
}

fn framed_bytes(message: &mut Vec<u8>, value: &[u8]) -> Result<()> {
    let length = u64::try_from(value.len())
        .map_err(|_| PlenoraError::ResourceLimit("hmac_sha256: valore troppo grande".into()))?;
    message.extend_from_slice(&length.to_be_bytes());
    message.extend_from_slice(value);
    Ok(())
}

/// HMAC-SHA256 per riga della concatenazione canonica dei valori.
///
/// Stesso framing di `stable_fingerprint` (separatore di dominio,
/// `framed(nome)`, `framed(tipo)`, byte di presenza, `framed(valore)`), con
/// separatore `b"plenora-hmac-sha256-v1\0"`. La chiave arriva SOLO dalla
/// variabile d'ambiente il cui nome e' `key_env`.
///
/// # Errors
///
/// - `InvalidPlan`: nome della colonna di output non valido, `key_env` vuoto,
///   `columns` vuoto, colonna ripetuta, chiave HMAC non disponibile
///   (variabile d'ambiente assente o vuota), valore oltre `u64` nel framing,
///   valore non rappresentabile come testo (come `scalar_as_string`);
/// - `Schema`: colonna assente dal batch;
/// - `DataMapping`: qualunque null sorgente, con row diagnostics.
pub fn hmac_sha256_riferimento(batch: &RecordBatch, config: &HmacSha256) -> Result<RecordBatch> {
    validate_output_name(&config.output_column)?;
    if config.key_env.trim().is_empty() {
        return Err(PlenoraError::InvalidPlan(
            "hmac_sha256: key_env vuoto".into(),
        ));
    }
    if config.columns.is_empty() {
        return Err(PlenoraError::InvalidPlan(
            "hmac_sha256 richiede almeno una colonna".into(),
        ));
    }
    let mut seen = HashSet::new();
    for name in &config.columns {
        if !seen.insert(name.as_str()) {
            return Err(PlenoraError::InvalidPlan(format!(
                "hmac_sha256: colonna ripetuta: {name}"
            )));
        }
    }
    let indices = config
        .columns
        .iter()
        .map(|name| column_index(batch, name))
        .collect::<Result<Vec<_>>>()?;
    let key = load_hmac_key(&config.key_env)?;
    // hmac non ha null_policy=error; Empty (default), Null e Skip
    // mantengono l'output storico dichiarato, nessun pre-rifiuto.
    let (inner_base, outer_base) = hmac_sha256_states(&key);
    // Framing costante per colonna e accesso tipizzato ai valori,
    // precomputati una volta per batch come in `fingerprint_rows`.
    let mut headers = Vec::with_capacity(config.columns.len());
    let mut accesses = Vec::with_capacity(config.columns.len());
    for (name, index) in config.columns.iter().zip(&indices) {
        let column = batch.column(*index);
        let data_type = column.data_type().to_string();
        let mut header = Vec::with_capacity(name.len() + data_type.len() + 16);
        framed_bytes(&mut header, name.as_bytes())?;
        framed_bytes(&mut header, data_type.as_bytes())?;
        headers.push(header);
        accesses.push(column_access(column.as_ref()));
    }
    let mut builder =
        StringBuilder::with_capacity(batch.num_rows(), batch.num_rows().saturating_mul(64));
    let mut message = Vec::with_capacity(256);
    let mut scratch = String::new();
    let mut hex = String::with_capacity(64);
    for row in 0..batch.num_rows() {
        message.clear();
        message.extend_from_slice(b"plenora-hmac-sha256-v1\0");
        let mut null_row = false;
        for (header, access) in headers.iter().zip(&accesses) {
            if null_row {
                break;
            }
            with_cell_value(access, row, &mut scratch, |value| {
                match value {
                    Some(value) => {
                        message.extend_from_slice(header);
                        message.push(1);
                        framed_bytes(&mut message, value.as_bytes())?;
                    }
                    None => match config.null_policy {
                        HmacNullPolicy::Empty => {
                            message.extend_from_slice(header);
                            message.push(1);
                            framed_bytes(&mut message, b"")?;
                        }
                        HmacNullPolicy::Null => null_row = true,
                        HmacNullPolicy::Skip => {}
                    },
                }
                Ok(())
            })?;
        }
        if null_row {
            builder.append_null();
            continue;
        }
        hex.clear();
        plenora_core::esadecimale::aggiungi_esadecimale(
            &mut hex,
            &hmac_sha256_with_states(&inner_base, &outer_base, &message),
        );
        builder.append_value(&hex);
    }
    replace_or_append(
        batch,
        &config.output_column,
        DataType::Utf8,
        matches!(config.null_policy, HmacNullPolicy::Null),
        Arc::new(builder.finish()),
    )
}

// ---------------------------------------------------------------------------
// Confronti
// ---------------------------------------------------------------------------

mod confronti {
    use std::sync::Arc;

    use plenora_core::arrow::array::{
        types::Int32Type, ArrayRef, BinaryArray, BooleanArray, Date32Array, Decimal128Array,
        DictionaryArray, Float64Array, Int32Array, Int64Array, RecordBatch, StringArray,
        TimestampMillisecondArray, UInt64Array,
    };
    use proptest::prelude::*;

    use super::super::{
        hmac_sha256, md5_hash, sha256_hash, stable_fingerprint, FingerprintAlgorithm,
        HashNullPolicy, HmacNullPolicy, HmacSha256, Md5Hash, Sha256Hash, StableFingerprint,
        COPPIE_ESADECIMALI, RIGHE_PER_CHUNK,
    };
    use super::{
        hmac_sha256_riferimento, md5_hash_riferimento, sha256_hash_riferimento,
        stable_fingerprint_riferimento,
    };
    use crate::test_support::{assert_same_outcome_bits, nullable_batch};
    use plenora_core::arrow::array::Array;
    use plenora_core::Result;

    /// Esiti completi, e in piu' i buffer di ogni colonna Utf8 dell'uscita:
    /// offset, byte dei valori e bitmap dei null (presente o assente).
    fn confronta(veloce: Result<RecordBatch>, riferimento: Result<RecordBatch>) {
        if let (Ok(veloce), Ok(riferimento)) = (&veloce, &riferimento) {
            for (a, b) in veloce.columns().iter().zip(riferimento.columns()) {
                if let (Some(a), Some(b)) = (
                    a.as_any().downcast_ref::<StringArray>(),
                    b.as_any().downcast_ref::<StringArray>(),
                ) {
                    assert_eq!(a.value_offsets(), b.value_offsets(), "offset");
                    assert_eq!(a.value_data(), b.value_data(), "byte dei valori");
                    assert_eq!(a.nulls(), b.nulls(), "bitmap dei null");
                }
            }
        }
        assert_same_outcome_bits(veloce, riferimento);
    }

    /// Chiave HMAC dei confronti (corta) e chiave oltre il blocco da 64 byte.
    const CHIAVE: &str = "PLENORA_HMAC_ORACOLO_CHUNK";
    const CHIAVE_LUNGA: &str = "PLENORA_HMAC_ORACOLO_CHUNK_LUNGA";

    fn imposta_chiavi() {
        std::env::set_var(CHIAVE, "chiave-oracolo-chunk");
        std::env::set_var(CHIAVE_LUNGA, "κλειδί".repeat(20));
    }

    const fn copia_politica(politica: &HashNullPolicy) -> HashNullPolicy {
        match politica {
            HashNullPolicy::Empty => HashNullPolicy::Empty,
            HashNullPolicy::Literal => HashNullPolicy::Literal,
            HashNullPolicy::Error => HashNullPolicy::Error,
        }
    }

    /// Le quattro operazioni con ogni combinazione di opzioni sulle colonne
    /// date, confrontate con la copia letterale del percorso precedente.
    fn confronta_tutto(batch: &RecordBatch, colonne: &[String]) {
        imposta_chiavi();
        for normalize in [false, true] {
            for (null_policy, null_literal) in [
                (HashNullPolicy::Empty, ""),
                (HashNullPolicy::Literal, " <NULL> ΟΔΟΣ "),
                (HashNullPolicy::Literal, ""),
                (HashNullPolicy::Error, ""),
            ] {
                let md5 = || Md5Hash {
                    columns: colonne.to_vec(),
                    output_column: "h".into(),
                    normalize,
                    null_policy: copia_politica(&null_policy),
                    null_literal: null_literal.into(),
                };
                confronta(md5_hash(batch, &md5()), md5_hash_riferimento(batch, &md5()));
                let sha = || Sha256Hash {
                    columns: colonne.to_vec(),
                    output_column: "h".into(),
                    normalize,
                    null_policy: copia_politica(&null_policy),
                    null_literal: null_literal.into(),
                };
                confronta(
                    sha256_hash(batch, &sha()),
                    sha256_hash_riferimento(batch, &sha()),
                );
            }
        }
        // Colonne ripetute: md5 e sha256 le accettano, fingerprint e hmac
        // le rifiutano; i confronti coprono entrambi gli esiti.
        for algorithm in [FingerprintAlgorithm::Sha256, FingerprintAlgorithm::Md5] {
            for columns in [colonne.to_vec(), Vec::new()] {
                let config = StableFingerprint {
                    columns,
                    output_column: "fingerprint".into(),
                    algorithm,
                };
                confronta(
                    stable_fingerprint(batch, &config),
                    stable_fingerprint_riferimento(batch, &config),
                );
            }
        }
        for key_env in [CHIAVE, CHIAVE_LUNGA, "PLENORA_HMAC_ORACOLO_ASSENTE"] {
            for null_policy in [
                HmacNullPolicy::Empty,
                HmacNullPolicy::Null,
                HmacNullPolicy::Skip,
            ] {
                let config = HmacSha256 {
                    columns: colonne.to_vec(),
                    key_env: key_env.into(),
                    output_column: "hmac".into(),
                    null_policy,
                };
                confronta(
                    hmac_sha256(batch, &config),
                    hmac_sha256_riferimento(batch, &config),
                );
            }
        }
    }

    /// Testi avversari: spazi Unicode ai bordi (`trim`), maiuscole con regole
    /// di contesto (sigma finale) o espansive, il separatore U+001F dentro un
    /// valore, testo vuoto e solo spazi.
    const TESTI: [&str; 16] = [
        "",
        " ",
        "  AbC  ",
        "ÀÉÎ",
        "ΟΔΟΣ",
        "ΟΔΟΣ ΚΑΙ",
        "İstanbul",
        "\u{3000}x\u{3000}",
        "\u{85}Y\u{a0}",
        "\u{b}Z\u{b}",
        "a\u{1f}b",
        "STRASSE ß ẞ",
        "ǅ ǈ",
        "NaN",
        "\t\n\r",
        "<null>",
    ];

    const FLOAT: [f64; 12] = [
        0.0,
        -0.0,
        f64::NAN,
        f64::INFINITY,
        f64::NEG_INFINITY,
        1e21,
        1e-7,
        0.1,
        f64::MIN_POSITIVE,
        5e-324,
        f64::MAX,
        -123.456,
    ];

    /// `true` sull'ultima riga di ogni periodo: i null sparsi delle fixture.
    const fn nulla(riga: usize, periodo: usize) -> bool {
        riga % periodo == periodo - 1
    }

    /// Batch con tutti i tipi del profilo scalare, `righe` righe cicliche
    /// sui valori avversari, null sparsi. Nessun valore che fallisca.
    fn batch_tutti_i_tipi(righe: usize) -> RecordBatch {
        let testi = (0..righe)
            .map(|riga| (!nulla(riga, 7)).then(|| TESTI[riga % TESTI.len()]))
            .collect::<Vec<_>>();
        let interi = (0..righe)
            .map(|riga| {
                (!nulla(riga, 5)).then(|| match riga % 4 {
                    0 => i64::MIN,
                    1 => i64::MAX,
                    2 => -1,
                    _ => i64::try_from(riga).unwrap_or_default(),
                })
            })
            .collect::<Vec<_>>();
        let doppi = (0..righe)
            .map(|riga| (!nulla(riga, 6)).then(|| FLOAT[riga % FLOAT.len()]))
            .collect::<Vec<_>>();
        let booleani = (0..righe)
            .map(|riga| (!nulla(riga, 4)).then_some(riga % 3 == 0))
            .collect::<Vec<_>>();
        let senza_segno = (0..righe)
            .map(|riga| (!nulla(riga, 8)).then_some(if riga % 2 == 0 { u64::MAX } else { 0 }))
            .collect::<Vec<_>>();
        let date = (0..righe)
            .map(|riga| (!nulla(riga, 9)).then(|| [0, -719_162, 2_932_896, 19_000][riga % 4]))
            .collect::<Vec<_>>();
        let istanti = (0..righe)
            .map(|riga| {
                (!nulla(riga, 10))
                    .then(|| [0, -1, 1_700_000_000_123, -62_135_596_800_000][riga % 4])
            })
            .collect::<Vec<_>>();
        let decimali = (0..righe)
            .map(|riga| (!nulla(riga, 11)).then(|| [0_i128, -1, 12_345, -99_999_999][riga % 4]))
            .collect::<Vec<_>>();
        let binari = (0..righe)
            .map(|riga| (!nulla(riga, 3)).then(|| TESTI[(riga + 5) % TESTI.len()].as_bytes()))
            .collect::<Vec<_>>();
        let chiavi = (0..righe)
            .map(|riga| (!nulla(riga, 13)).then(|| i32::try_from(riga % 4).unwrap_or_default()))
            .collect::<Vec<_>>();
        let dizionario = DictionaryArray::<Int32Type>::try_new(
            Int32Array::from(chiavi),
            Arc::new(StringArray::from(vec![
                Some("Uno "),
                None,
                Some("ΣΑΣ"),
                Some(""),
            ])),
        )
        .expect("dizionario");
        nullable_batch(vec![
            ("s", Arc::new(StringArray::from(testi)) as ArrayRef),
            ("i", Arc::new(Int64Array::from(interi))),
            ("f", Arc::new(Float64Array::from(doppi))),
            ("b", Arc::new(BooleanArray::from(booleani))),
            ("u", Arc::new(UInt64Array::from(senza_segno))),
            ("d", Arc::new(Date32Array::from(date))),
            (
                "t",
                Arc::new(
                    TimestampMillisecondArray::from(istanti.clone()).with_timezone("Europe/Rome"),
                ),
            ),
            ("tn", Arc::new(TimestampMillisecondArray::from(istanti))),
            (
                "m",
                Arc::new(
                    Decimal128Array::from(decimali.clone())
                        .with_precision_and_scale(38, 3)
                        .expect("decimal"),
                ),
            ),
            (
                "m0",
                Arc::new(
                    Decimal128Array::from(decimali)
                        .with_precision_and_scale(38, 0)
                        .expect("decimal"),
                ),
            ),
            ("y", Arc::new(BinaryArray::from(binari))),
            ("k", Arc::new(dizionario)),
        ])
    }

    fn nomi(colonne: &[&str]) -> Vec<String> {
        colonne.iter().map(|nome| (*nome).to_owned()).collect()
    }

    #[test]
    fn la_tabella_esadecimale_e_quella_del_core() {
        let tutti = (0..=u8::MAX).collect::<Vec<_>>();
        let mut attesa = Vec::new();
        for byte in &tutti {
            attesa.extend_from_slice(&COPPIE_ESADECIMALI[usize::from(*byte)]);
        }
        assert_eq!(
            String::from_utf8(attesa).expect("ascii"),
            plenora_core::esadecimale::esadecimale(&tutti)
        );
    }

    #[test]
    fn tutti_i_tipi_su_piu_chunk() {
        // Piu' chunk (percorso rayon), un chunk solo, una riga, nessuna riga.
        for righe in [RIGHE_PER_CHUNK * 9 + 5, RIGHE_PER_CHUNK, 1, 0] {
            let batch = batch_tutti_i_tipi(righe);
            let tutte = nomi(&["s", "i", "f", "b", "u", "d", "t", "tn", "m", "m0", "y", "k"]);
            confronta_tutto(&batch, &tutte);
            for colonna in &tutte {
                confronta_tutto(&batch, std::slice::from_ref(colonna));
            }
            // Ordine inverso, ripetizioni, nessuna colonna, colonna assente.
            confronta_tutto(&batch, &nomi(&["k", "s", "f", "s"]));
            confronta_tutto(&batch, &[]);
            confronta_tutto(&batch, &nomi(&["s", "assente"]));
            // Nome di uscita che sostituisce una colonna d'ingresso.
            let config = Md5Hash {
                columns: nomi(&["s", "i"]),
                output_column: "s".into(),
                normalize: true,
                null_policy: HashNullPolicy::Empty,
                null_literal: String::new(),
            };
            confronta(
                md5_hash(&batch, &config),
                md5_hash_riferimento(&batch, &config),
            );
        }
    }

    /// Batch con errori in righe e colonne note: l'errore reso deve essere
    /// quello della prima riga che fallisce (e, nella riga, della prima
    /// colonna), anche quando i chunk successivi falliscono con un altro.
    #[test]
    fn primo_errore_come_nella_scansione_sequenziale() {
        let righe = RIGHE_PER_CHUNK * 6 + 3;
        let riga_binario = RIGHE_PER_CHUNK * 2 + 1;
        let riga_data = RIGHE_PER_CHUNK * 4 + 7;
        let binari = (0..righe)
            .map(|riga| {
                Some(if riga == riga_binario || riga == riga_data {
                    &[0xff_u8, 0xfe][..]
                } else {
                    b"ok".as_slice()
                })
            })
            .collect::<Vec<_>>();
        let date = (0..righe)
            .map(|riga| {
                Some(if riga == riga_data || riga + 3 == righe {
                    i32::MAX
                } else {
                    1
                })
            })
            .collect::<Vec<_>>();
        let istanti = (0..righe)
            .map(|riga| {
                Some(if riga == 5 * RIGHE_PER_CHUNK {
                    i64::MAX
                } else {
                    0
                })
            })
            .collect::<Vec<_>>();
        let batch = nullable_batch(vec![
            ("y", Arc::new(BinaryArray::from(binari)) as ArrayRef),
            ("d", Arc::new(Date32Array::from(date))),
            ("t", Arc::new(TimestampMillisecondArray::from(istanti))),
            ("s", Arc::new(StringArray::from(vec![Some("x"); righe]))),
            ("z", Arc::new(Int32Array::from(vec![Some(1); righe]))),
        ]);
        for colonne in [
            nomi(&["y", "d", "t", "s"]),
            nomi(&["d", "y", "t"]),
            nomi(&["t", "d"]),
            nomi(&["t", "s"]),
            nomi(&["s", "z"]),
            nomi(&["d"]),
        ] {
            confronta_tutto(&batch, &colonne);
        }
        // Timezone non valida: errore alla prima riga non nulla.
        let fuso = nullable_batch(vec![(
            "t",
            Arc::new(
                TimestampMillisecondArray::from(vec![None, Some(0_i64), Some(1)])
                    .with_timezone("Non/Esiste"),
            ) as ArrayRef,
        )]);
        confronta_tutto(&fuso, &nomi(&["t"]));
        // Scala decimal negativa: errore di tipo, su un batch con piu' chunk
        // tutti in errore.
        let decimali = nullable_batch(vec![(
            "m",
            Arc::new(
                Decimal128Array::from(vec![Some(1_i128); RIGHE_PER_CHUNK * 3])
                    .with_precision_and_scale(10, -2)
                    .expect("decimal"),
            ) as ArrayRef,
        )]);
        confronta_tutto(&decimali, &nomi(&["m"]));
        // hmac `null`: null in `a` ed errore in `b` sulla stessa riga; la
        // riga e' nulla senza leggere `b` (con `empty` e `skip`, errore).
        let null_prima = nullable_batch(vec![
            (
                "a",
                Arc::new(StringArray::from(vec![Some("x"), None, Some("y")])) as ArrayRef,
            ),
            (
                "b",
                Arc::new(BinaryArray::from(vec![
                    Some(b"ok".as_slice()),
                    Some(&[0xff_u8][..]),
                    Some(b"ok".as_slice()),
                ])),
            ),
        ]);
        confronta_tutto(&null_prima, &nomi(&["a", "b"]));
    }

    /// Fette con offset diverso da zero, su piu' chunk.
    #[test]
    fn fette_del_batch() {
        let batch = batch_tutti_i_tipi(RIGHE_PER_CHUNK * 5 + 9);
        let tutte = nomi(&["s", "i", "f", "b", "u", "d", "t", "tn", "m", "m0", "y", "k"]);
        for (inizio, righe) in [(3, RIGHE_PER_CHUNK * 4), (RIGHE_PER_CHUNK + 1, 7), (5, 0)] {
            let fetta = batch.slice(inizio, righe);
            confronta_tutto(&fetta, &tutte);
            confronta_tutto(&fetta, &nomi(&["s"]));
        }
    }

    /// Cella casuale di un testo: dai testi avversari o da Unicode qualunque.
    fn testo() -> impl Strategy<Value = Option<String>> {
        prop_oneof![
            1 => Just(None),
            3 => (0..TESTI.len()).prop_map(|indice| Some(TESTI[indice].to_owned())),
            3 => "\\PC{0,6}".prop_map(Some),
            1 => "[ \\t\\u{3000}\\u{85}aAΣσς]{0,5}".prop_map(Some),
        ]
    }

    fn doppio() -> impl Strategy<Value = Option<f64>> {
        prop_oneof![
            1 => Just(None),
            2 => (0..FLOAT.len()).prop_map(|indice| Some(FLOAT[indice])),
            2 => any::<f64>().prop_map(Some),
        ]
    }

    /// Una data: quasi sempre nell'intervallo, di rado oltre (errore).
    fn data() -> impl Strategy<Value = Option<i32>> {
        prop_oneof![
            1 => Just(None),
            30 => (-1_000_000_i32..1_000_000).prop_map(Some),
            1 => Just(Some(i32::MAX)),
        ]
    }

    /// Un binario: quasi sempre UTF-8, di rado no (errore).
    fn binario() -> impl Strategy<Value = Option<Vec<u8>>> {
        prop_oneof![
            1 => Just(None),
            30 => "\\PC{0,4}".prop_map(|testo| Some(testo.into_bytes())),
            1 => Just(Some(vec![0xc3_u8])),
        ]
    }

    /// Una riga delle colonne casuali.
    type Riga = (
        Option<String>,
        Option<i64>,
        Option<f64>,
        Option<bool>,
        Option<u64>,
        Option<i32>,
        Option<Vec<u8>>,
    );

    fn righe() -> impl Strategy<Value = Vec<Riga>> {
        prop::collection::vec(
            (
                testo(),
                prop::option::weighted(0.8, any::<i64>()),
                doppio(),
                prop::option::weighted(0.8, any::<bool>()),
                prop::option::weighted(0.8, any::<u64>()),
                data(),
                binario(),
            ),
            0..90,
        )
    }

    fn batch_casuale(righe: &[Riga]) -> RecordBatch {
        let chiavi = righe
            .iter()
            .enumerate()
            .map(|(indice, riga)| {
                riga.1
                    .map(|_| i32::try_from(indice % 3).unwrap_or_default())
            })
            .collect::<Vec<_>>();
        let dizionario = DictionaryArray::<Int32Type>::try_new(
            Int32Array::from(chiavi),
            Arc::new(StringArray::from(vec![Some(" Ab "), None, Some("ΣΑΣ")])),
        )
        .expect("dizionario");
        nullable_batch(vec![
            (
                "s",
                Arc::new(StringArray::from(
                    righe.iter().map(|riga| riga.0.clone()).collect::<Vec<_>>(),
                )) as ArrayRef,
            ),
            (
                "i",
                Arc::new(Int64Array::from(
                    righe.iter().map(|riga| riga.1).collect::<Vec<_>>(),
                )),
            ),
            (
                "f",
                Arc::new(Float64Array::from(
                    righe.iter().map(|riga| riga.2).collect::<Vec<_>>(),
                )),
            ),
            (
                "b",
                Arc::new(BooleanArray::from(
                    righe.iter().map(|riga| riga.3).collect::<Vec<_>>(),
                )),
            ),
            (
                "u",
                Arc::new(UInt64Array::from(
                    righe.iter().map(|riga| riga.4).collect::<Vec<_>>(),
                )),
            ),
            (
                "d",
                Arc::new(Date32Array::from(
                    righe.iter().map(|riga| riga.5).collect::<Vec<_>>(),
                )),
            ),
            (
                "y",
                Arc::new(BinaryArray::from(
                    righe
                        .iter()
                        .map(|riga| riga.6.as_deref())
                        .collect::<Vec<_>>(),
                )),
            ),
            ("k", Arc::new(dizionario)),
        ])
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(96))]

        #[test]
        fn come_il_riferimento_su_input_casuali(
            righe in righe(),
            scelta in prop::collection::vec(0_usize..8, 0..5),
        ) {
            let batch = batch_casuale(&righe);
            let tutte = ["s", "i", "f", "b", "u", "d", "y", "k"];
            let colonne = scelta
                .iter()
                .map(|indice| tutte[*indice].to_owned())
                .collect::<Vec<_>>();
            confronta_tutto(&batch, &colonne);
        }
    }
}
