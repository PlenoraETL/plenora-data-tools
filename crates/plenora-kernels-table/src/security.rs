use std::collections::HashSet;
use std::fmt::Write as _;
use std::ops::Range;
use std::sync::Arc;

use md5::{Digest, Md5};
use plenora_core::arrow::array::{
    builder::StringBuilder, Array, BooleanArray, Float64Array, Int64Array, RecordBatch,
    StringArray, UInt64Array,
};
use plenora_core::arrow::schema::DataType;
use rayon::prelude::{IntoParallelRefIterator, ParallelIterator};
use serde::Deserialize;
use sha2::Sha256;

use crate::{
    column_index, reject_rows, replace_or_append, scalar_as_string, validate_output_name,
    RowRejection,
};
use plenora_core::{PlenoraError, Result};

/// Config di `table.md5_hash`: MD5 esadecimale dei valori di alcune
/// colonne.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Md5Hash {
    /// Colonne del messaggio, lette come testo; l'ordine non conta (si
    /// ordinano per nome).
    pub columns: Vec<String>,
    /// Colonna d'uscita (`Utf8` non nullable); default `md5_hash`.
    #[serde(default = "default_hash_name")]
    pub output_column: String,
    /// `trim` e `to_lowercase` di ogni valore e di `null_literal`; default
    /// `true`.
    #[serde(default = "default_true")]
    pub normalize: bool,
    /// Come entra una cella nulla; default `empty`.
    #[serde(default = "default_null_policy")]
    pub null_policy: HashNullPolicy,
    /// Testo di una cella nulla con `null_policy` `literal`; default
    /// `<null>` ([`letterale_nullo`]). Con le altre politiche non avrebbe
    /// effetto: scritto si rifiuta ([`verifica_null_literal`]).
    #[serde(default)]
    pub null_literal: Option<String>,
}

/// Politica sui null di `md5_hash` e `sha256_hash`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HashNullPolicy {
    /// La cella nulla vale il testo vuoto (come una cella vuota).
    Empty,
    /// La cella nulla vale `null_literal`.
    Literal,
    /// Una cella nulla rifiuta la riga, con diagnostica per riga.
    Error,
}

const fn default_null_policy() -> HashNullPolicy {
    HashNullPolicy::Empty
}

const DEFAULT_NULL_LITERAL: &str = "<null>";

/// Il testo di una cella nulla con `null_policy` `literal`: `null_literal`,
/// o `<null>` se assente.
#[must_use]
pub fn letterale_nullo(null_literal: Option<&String>) -> &str {
    null_literal.map_or(DEFAULT_NULL_LITERAL, String::as_str)
}

/// `null_literal` vale solo con `null_policy` `literal`.
///
/// Con `empty` e `error` una cella nulla non diventa mai quel testo, quindi
/// scritto si rifiuta. La chiamano i kernel `md5_hash` e `sha256_hash` e
/// l'analisi dei contratti.
///
/// # Errors
///
/// `InvalidPlan` se `null_literal` e' scritto con un'altra politica.
pub fn verifica_null_literal(
    null_policy: &HashNullPolicy,
    null_literal: Option<&String>,
) -> Result<()> {
    if null_literal.is_some() && !matches!(null_policy, HashNullPolicy::Literal) {
        return Err(PlenoraError::InvalidPlan(
            "null_literal ammesso solo con null_policy=literal".into(),
        ));
    }
    Ok(())
}

fn default_hash_name() -> String {
    "md5_hash".into()
}
const fn default_true() -> bool {
    true
}

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
/// Il messaggio non e' delimitato: un valore con U+001F, o un null con
/// `empty` e un testo vuoto, possono dare lo stesso hash.
///
/// # Errors
///
/// - `InvalidPlan`: nome della colonna di output non valido, `columns` vuoto,
///   `null_literal` scritto senza `null_policy` `literal`;
/// - `DataMapping`: null sorgente con `null_policy` `error`, con row
///   diagnostics (`validation.required_value_missing`);
/// - `Schema`: colonna assente dal batch, valore non rappresentabile come
///   testo o tipo non coperto dal profilo scalare (gli errori di
///   `scalar_as_string`).
pub fn md5_hash(batch: &RecordBatch, config: &Md5Hash) -> Result<RecordBatch> {
    validate_output_name(&config.output_column)?;
    verifica_null_literal(&config.null_policy, config.null_literal.as_ref())?;
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
    // Il pre-rifiuto row-scoped vale solo per null_policy=error: empty
    // (default) e literal danno un valore alla cella nulla.
    if matches!(config.null_policy, HashNullPolicy::Error) {
        reject_null_hash_rows(batch, &columns, &indices)?;
    }
    // Accesso tipizzato e letterale normalizzato risolti una volta per
    // batch; per riga un solo buffer riusato, con gli stessi byte di
    // `parts.join("\u{1f}")` dell'oracolo (`security_hash_oracolo.rs`).
    let accessi = indices
        .iter()
        .map(|index| column_access(batch.column(*index).as_ref()))
        .collect::<Vec<_>>();
    let letterale = letterale_nullo(config.null_literal.as_ref());
    let letterale = if config.normalize {
        letterale.trim().to_lowercase()
    } else {
        letterale.to_owned()
    };
    let values = colonna_digest(batch.num_rows(), 32, |row, appunti, uscita| {
        let Appunti { messaggio, testo } = appunti;
        messaggio.clear();
        for (posizione, accesso) in accessi.iter().enumerate() {
            if posizione > 0 {
                messaggio.push(0x1f);
            }
            match testo_cella(accesso, row, testo)? {
                Some(valore) if config.normalize => accoda_normalizzato(messaggio, valore),
                Some(valore) => messaggio.extend_from_slice(valore.as_bytes()),
                None => match config.null_policy {
                    HashNullPolicy::Empty => {}
                    HashNullPolicy::Literal => messaggio.extend_from_slice(letterale.as_bytes()),
                    HashNullPolicy::Error => {
                        return Err(PlenoraError::Internal(
                            "prevalidazione null md5_hash incoerente".into(),
                        ));
                    }
                },
            }
        }
        accoda_esadecimale(uscita, &Md5::digest(messaggio.as_slice()));
        Ok(false)
    })?;
    replace_or_append(
        batch,
        &config.output_column,
        DataType::Utf8,
        false,
        Arc::new(values),
    )
}

/// Config di `table.sha256_hash`: SHA-256 esadecimale di un messaggio
/// delimitato (nome, tipo e valore di ogni colonna).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Sha256Hash {
    /// Colonne del messaggio, lette come testo; l'ordine non conta (si
    /// ordinano per nome).
    pub columns: Vec<String>,
    /// Colonna d'uscita (`Utf8` non nullable); default `sha256_hash`.
    #[serde(default = "default_sha256_name")]
    pub output_column: String,
    /// `trim` e `to_lowercase` di ogni valore e di `null_literal`; default
    /// `true`.
    #[serde(default = "default_true")]
    pub normalize: bool,
    /// Come entra una cella nulla; default `empty`.
    #[serde(default = "default_null_policy")]
    pub null_policy: HashNullPolicy,
    /// Testo di una cella nulla con `null_policy` `literal`; default
    /// `<null>` ([`letterale_nullo`]). Con le altre politiche non avrebbe
    /// effetto: scritto si rifiuta ([`verifica_null_literal`]).
    #[serde(default)]
    pub null_literal: Option<String>,
}

fn default_sha256_name() -> String {
    "sha256_hash".into()
}

/// Colonna con l'hash SHA-256 (esadecimale) delle colonne configurate.
///
/// Dopo il separatore di dominio `plenora-sha256-v1\0`, per ogni colonna
/// in ordine di nome: `framed(nome)`, `framed(tipo Arrow)`, il byte 1 e
/// `framed(valore)` (lunghezza u64 big-endian + byte). Il byte di presenza
/// e' sempre 1: una cella nulla entra come testo vuoto o `null_literal`.
/// Nessuna collisione per concatenazione; null e testo vuoto (`empty`), o
/// null e `null_literal` (`literal`), coincidono.
///
/// # Errors
///
/// - `InvalidPlan`: nome della colonna di output non valido, `null_literal`
///   scritto senza `null_policy` `literal`;
/// - `ResourceLimit`: lunghezza di un valore oltre `u64` nel framing;
/// - `DataMapping`: null sorgente con `null_policy` `error`, con row
///   diagnostics (`validation.required_value_missing`);
/// - `Schema`: colonna assente dal batch, valore non rappresentabile come
///   testo o tipo non coperto dal profilo scalare (gli errori di
///   `scalar_as_string`).
pub fn sha256_hash(batch: &RecordBatch, config: &Sha256Hash) -> Result<RecordBatch> {
    validate_output_name(&config.output_column)?;
    verifica_null_literal(&config.null_policy, config.null_literal.as_ref())?;
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
    // Il framing di nome e tipo con il byte di presenza (sempre 1: anche un
    // null diventa un valore, vuoto o letterale) e' costante per colonna; il
    // separatore di dominio e la prima intestazione sono assorbiti una volta
    // in `base`, clonato per riga. Byte assorbiti identici a quelli
    // dell'oracolo (`security_hash_oracolo.rs`).
    let accessi = indices
        .iter()
        .map(|index| column_access(batch.column(*index).as_ref()))
        .collect::<Vec<_>>();
    let intestazioni = names
        .iter()
        .zip(&indices)
        .map(|(name, index)| {
            let mut intestazione = Vec::new();
            framed_vec(&mut intestazione, name.as_bytes(), "sha256_hash")?;
            framed_vec(
                &mut intestazione,
                batch.column(*index).data_type().to_string().as_bytes(),
                "sha256_hash",
            )?;
            intestazione.push(1);
            Ok(intestazione)
        })
        .collect::<Result<Vec<_>>>()?;
    let mut base = Sha256::new();
    base.update(b"plenora-sha256-v1\0");
    if let Some(prima) = intestazioni.first() {
        base.update(prima);
    }
    let letterale = letterale_nullo(config.null_literal.as_ref());
    let letterale = if config.normalize {
        letterale.trim().to_lowercase()
    } else {
        letterale.to_owned()
    };
    let values = colonna_digest(batch.num_rows(), 64, |row, appunti, uscita| {
        let Appunti { messaggio, testo } = appunti;
        messaggio.clear();
        for (posizione, (intestazione, accesso)) in intestazioni.iter().zip(&accessi).enumerate() {
            if posizione > 0 {
                messaggio.extend_from_slice(intestazione);
            }
            match testo_cella(accesso, row, testo)? {
                Some(valore) if config.normalize => {
                    framed_normalizzato(messaggio, valore, "sha256_hash")?;
                }
                Some(valore) => framed_vec(messaggio, valore.as_bytes(), "sha256_hash")?,
                None => match config.null_policy {
                    HashNullPolicy::Empty => framed_vec(messaggio, b"", "sha256_hash")?,
                    HashNullPolicy::Literal => {
                        framed_vec(messaggio, letterale.as_bytes(), "sha256_hash")?;
                    }
                    HashNullPolicy::Error => {
                        return Err(PlenoraError::Internal(
                            "prevalidazione null sha256_hash incoerente".into(),
                        ));
                    }
                },
            }
        }
        let mut digest = base.clone();
        digest.update(messaggio.as_slice());
        accoda_esadecimale(uscita, &digest.finalize());
        Ok(false)
    })?;
    replace_or_append(
        batch,
        &config.output_column,
        DataType::Utf8,
        false,
        Arc::new(values),
    )
}

/// Funzione di hash di `stable_fingerprint`.
#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FingerprintAlgorithm {
    /// SHA-256, 64 cifre esadecimali (default).
    #[default]
    Sha256,
    /// MD5, 32 cifre esadecimali; non resistente alle collisioni costruite.
    Md5,
}

/// Config di `table.stable_fingerprint`: impronta canonica per riga, senza
/// normalizzazione, con null distinto dal testo vuoto.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StableFingerprint {
    /// Colonne hashate, nell'ordine dato. Vuoto (default) = tutte le colonne
    /// nell'ordine dello schema.
    #[serde(default)]
    pub columns: Vec<String>,
    /// Colonna d'uscita (`Utf8` non nullable); default `fingerprint`.
    #[serde(default = "default_fingerprint_name")]
    pub output_column: String,
    /// Funzione di hash; default `sha256`.
    #[serde(default)]
    pub algorithm: FingerprintAlgorithm,
}

fn default_fingerprint_name() -> String {
    "fingerprint".into()
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

/// Valore testuale canonico della cella, in prestito dall'array (Utf8) o dal
/// buffer `testo` riusato (gli altri tipi).
///
/// Stessi byte, stessi null e stessi errori di `scalar_as_string` (`write!`
/// usa lo stesso `Display` di `to_string`; i tipi non tipizzati qui passano
/// proprio da `scalar_as_string`).
fn testo_cella<'r>(
    accesso: &'r ColumnAccess<'_>,
    row: usize,
    testo: &'r mut String,
) -> Result<Option<&'r str>> {
    match accesso {
        ColumnAccess::Utf8(values) => Ok((!values.is_null(row)).then(|| values.value(row))),
        ColumnAccess::Int64(values) => Ok((!values.is_null(row)).then(|| {
            testo.clear();
            let _ = write!(testo, "{}", values.value(row));
            testo.as_str()
        })),
        ColumnAccess::Float64(values) => Ok((!values.is_null(row)).then(|| {
            testo.clear();
            let _ = write!(testo, "{}", values.value(row));
            testo.as_str()
        })),
        ColumnAccess::Boolean(values) => Ok((!values.is_null(row)).then(|| {
            testo.clear();
            let _ = write!(testo, "{}", values.value(row));
            testo.as_str()
        })),
        ColumnAccess::UInt64(values) => Ok((!values.is_null(row)).then(|| {
            testo.clear();
            let _ = write!(testo, "{}", values.value(row));
            testo.as_str()
        })),
        ColumnAccess::Scalar(array) => Ok(scalar_as_string(*array, row)?.map(|valore| {
            *testo = valore;
            testo.as_str()
        })),
    }
}

/// Accoda `valore.trim().to_lowercase()` senza allocare sul testo ASCII.
///
/// Su un testo ASCII `to_lowercase` coincide con `to_ascii_lowercase` (la
/// sola regola di contesto di `to_lowercase`, il sigma finale, riguarda un
/// carattere non ASCII); `trim` resta quello di `str`, con gli spazi
/// Unicode. Il testo non ASCII passa da `to_lowercase`.
fn accoda_normalizzato(messaggio: &mut Vec<u8>, valore: &str) {
    let ridotto = valore.trim();
    if ridotto.is_ascii() {
        messaggio.extend(ridotto.bytes().map(|byte| byte.to_ascii_lowercase()));
    } else {
        messaggio.extend_from_slice(ridotto.to_lowercase().as_bytes());
    }
}

/// Frame di `valore.trim().to_lowercase()`: sul testo ASCII la lunghezza del
/// testo normalizzato e' quella di `trim` (vedi `accoda_normalizzato`).
fn framed_normalizzato(messaggio: &mut Vec<u8>, valore: &str, op: &str) -> Result<()> {
    let ridotto = valore.trim();
    if ridotto.is_ascii() {
        let length = u64::try_from(ridotto.len())
            .map_err(|_| PlenoraError::ResourceLimit(format!("{op}: valore troppo grande")))?;
        messaggio.extend_from_slice(&length.to_be_bytes());
        messaggio.extend(ridotto.bytes().map(|byte| byte.to_ascii_lowercase()));
        Ok(())
    } else {
        framed_vec(messaggio, ridotto.to_lowercase().as_bytes(), op)
    }
}

// ---------------------------------------------------------------------------
// Digest per riga a chunk paralleli
// ---------------------------------------------------------------------------

/// Righe per chunk nel calcolo dei digest.
///
/// Nei test un valore piccolo, perche' anche i batch degli oracoli
/// attraversino piu' chunk e il percorso rayon; l'uscita non dipende dal
/// valore (vedi `colonna_digest`).
const RIGHE_PER_CHUNK: usize = if cfg!(test) { 16 } else { 32_768 };

/// Le 256 coppie di cifre esadecimali minuscole, una per valore di byte:
/// gli stessi byte di `plenora_core::esadecimale` (provato nei test).
const COPPIE_ESADECIMALI: [[u8; 2]; 256] = {
    const CIFRE: &[u8; 16] = b"0123456789abcdef";
    let mut tabella = [[0_u8; 2]; 256];
    let mut byte = 0;
    while byte < 256 {
        tabella[byte] = [CIFRE[byte >> 4], CIFRE[byte & 0x0F]];
        byte += 1;
    }
    tabella
};

/// Accoda il digest in esadecimale minuscolo, due cifre per byte.
fn accoda_esadecimale(uscita: &mut Vec<u8>, digest: &[u8]) {
    for &byte in digest {
        uscita.extend_from_slice(&COPPIE_ESADECIMALI[usize::from(byte)]);
    }
}

/// Buffer riusati da tutte le righe di un chunk.
#[derive(Default)]
struct Appunti {
    /// Byte della riga che seguono il prefisso costante gia' assorbito.
    messaggio: Vec<u8>,
    /// Testo di una cella non Utf8 (numero formattato o fallback scalare).
    testo: String,
}

/// Uscita di un chunk: le cifre delle righe non nulle, concatenate, e la
/// nullita' di ogni riga.
struct ChunkDigest {
    esadecimale: Vec<u8>,
    nulle: Vec<bool>,
}

fn digest_incoerente() -> PlenoraError {
    PlenoraError::Internal("digest per riga incoerente".into())
}

/// Colonna Utf8 dei digest per riga, calcolati per chunk contigui di righe.
///
/// `riga(row, appunti, uscita)` accoda a `uscita` le `cifre` esadecimali del
/// digest della riga e rende `false`, oppure rende `true` per una riga nulla
/// senza accodare nulla. Ogni riga dipende solo da se stessa; i chunk, in
/// parallelo (rayon) se piu' di uno, si concatenano nell'ordine delle righe.
/// L'errore reso e' quello del primo chunk che fallisce, cioe' della prima
/// riga che fallisce, come nella scansione sequenziale.
fn colonna_digest<F>(righe: usize, cifre: usize, riga: F) -> Result<StringArray>
where
    F: Fn(usize, &mut Appunti, &mut Vec<u8>) -> Result<bool> + Sync,
{
    let intervalli: Vec<Range<usize>> = (0..righe.div_ceil(RIGHE_PER_CHUNK))
        .map(|indice| {
            let inizio = indice * RIGHE_PER_CHUNK;
            inizio..inizio.saturating_add(RIGHE_PER_CHUNK).min(righe)
        })
        .collect();
    let calcola = |intervallo: &Range<usize>| -> Result<ChunkDigest> {
        let mut appunti = Appunti::default();
        let mut esadecimale = Vec::with_capacity(intervallo.len().saturating_mul(cifre));
        let mut nulle = Vec::with_capacity(intervallo.len());
        for row in intervallo.clone() {
            nulle.push(riga(row, &mut appunti, &mut esadecimale)?);
        }
        Ok(ChunkDigest { esadecimale, nulle })
    };
    let esiti: Vec<Result<ChunkDigest>> = if intervalli.len() > 1 {
        intervalli.par_iter().map(calcola).collect()
    } else {
        intervalli.iter().map(calcola).collect()
    };
    let mut builder = StringBuilder::with_capacity(righe, righe.saturating_mul(cifre));
    for esito in esiti {
        let ChunkDigest { esadecimale, nulle } = esito?;
        let testo = std::str::from_utf8(&esadecimale).map_err(|_| digest_incoerente())?;
        let mut inizio = 0_usize;
        for nulla in nulle {
            if nulla {
                builder.append_null();
                continue;
            }
            let fine = inizio.saturating_add(cifre);
            builder.append_value(testo.get(inizio..fine).ok_or_else(digest_incoerente)?);
            inizio = fine;
        }
        if inizio != testo.len() {
            return Err(digest_incoerente());
        }
    }
    Ok(builder.finish())
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
fn fingerprint_rows<D: Digest + Clone + Sync>(
    batch: &RecordBatch,
    names: &[String],
    indices: &[usize],
) -> Result<StringArray> {
    // Framing per colonna e accesso tipizzato precomputati una volta per
    // batch (`data_type().to_string()` alloca). Il separatore di dominio e
    // la prima intestazione, costanti, sono assorbiti una volta in `base`,
    // clonato per riga; il resto della riga in un buffer riusato. Il byte
    // stream e' l'encoding canonico documentato sopra.
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
    let mut base = D::new();
    base.update(b"plenora-fingerprint-v1\0");
    if let Some(prima) = headers.first() {
        base.update(prima);
    }
    let cifre = <D as Digest>::output_size() * 2;
    colonna_digest(batch.num_rows(), cifre, |row, appunti, uscita| {
        let Appunti { messaggio, testo } = appunti;
        messaggio.clear();
        for (posizione, (header, access)) in headers.iter().zip(&accesses).enumerate() {
            if posizione > 0 {
                messaggio.extend_from_slice(header);
            }
            match testo_cella(access, row, testo)? {
                Some(value) => {
                    messaggio.push(1);
                    framed_vec(messaggio, value.as_bytes(), "stable_fingerprint")?;
                }
                None => messaggio.push(0),
            }
        }
        let mut digest = base.clone();
        digest.update(messaggio.as_slice());
        accoda_esadecimale(uscita, &digest.finalize());
        Ok(false)
    })
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
///   colonne);
/// - `ResourceLimit`: lunghezza di un valore oltre `u64` nel framing;
/// - `Schema`: colonna assente dal batch, valore non rappresentabile come
///   testo o tipo non coperto dal profilo scalare (gli errori di
///   `scalar_as_string`).
pub fn stable_fingerprint(batch: &RecordBatch, config: &StableFingerprint) -> Result<RecordBatch> {
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

// ---------------------------------------------------------------------------
// table.hmac_sha256
// ---------------------------------------------------------------------------

/// Politica sui null per `hmac_sha256`.
///
/// Nessun null sorgente e' rifiutato: `empty` lo hasha come valore vuoto,
/// `null` rende nulla la riga d'uscita, `skip` omette la colonna dal
/// messaggio (vedi `hmac_sha256`).
#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HmacNullPolicy {
    /// La cella nulla entra come testo vuoto, con intestazione e byte di
    /// presenza 1 (default).
    #[default]
    Empty,
    /// Una cella nulla rende nulla l'uscita della riga.
    Null,
    /// La colonna nulla manca dal messaggio, intestazione compresa.
    Skip,
}

/// Config di `table.hmac_sha256`: HMAC-SHA256 per riga con chiave da
/// variabile d'ambiente.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HmacSha256 {
    /// Colonne hashate, nell'ordine dato (come `stable_fingerprint`).
    pub columns: Vec<String>,
    /// NOME della variabile d'ambiente che contiene la chiave. La chiave non
    /// compare mai nel piano, negli errori o nei log: solo il nome.
    pub key_env: String,
    /// Colonna d'uscita (`Utf8`, nullable solo con `null_policy` `null`);
    /// default `hmac`.
    #[serde(default = "default_hmac_name")]
    pub output_column: String,
    /// Come entra una cella nulla; default `empty`.
    #[serde(default)]
    pub null_policy: HmacNullPolicy,
}

fn default_hmac_name() -> String {
    "hmac".into()
}

/// HMAC-SHA256 (RFC 2104) implementato sopra `sha2` — due round di hash con
/// ipad/opad su blocco da 64 byte, nessuna dipendenza aggiuntiva.
///
/// Gli stati Sha256 dopo l'assorbimento di ipad/opad dipendono SOLO dalla
/// chiave: sono precomputati una volta per batch e clonati per riga (il
/// byte stream assorbito e' identico a quello dell'oracolo byte per byte).
/// `hmac_sha256` assorbe nello stato interno anche il separatore di dominio.
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

/// Legge la chiave di `table.hmac_sha256` (i byte UTF-8 del valore) dalla
/// variabile d'ambiente `key_env`.
///
/// E' l'unica lettura della chiave: la chiamano il kernel, il suo oracolo e
/// il controllo d'ambiente del runner in validazione, cosi' non possono dare
/// verdetti diversi sulla stessa variabile. Gli errori non rivelano ne' il
/// nome della variabile ne' alcun frammento del valore.
///
/// # Errors
///
/// `InvalidPlan`, distinto per causa: variabile assente, vuota, o con un
/// valore che non e' UTF-8 (su Windows, UTF-16 non valido). Un valore non
/// UTF-8 non diventa in silenzio un'altra chiave, ne' una variabile assente.
pub fn carica_chiave_hmac(key_env: &str) -> Result<Vec<u8>> {
    match std::env::var_os(key_env) {
        None => Err(PlenoraError::InvalidPlan(
            "hmac_sha256: chiave HMAC non disponibile (variabile assente)".into(),
        )),
        Some(value) if value.is_empty() => Err(PlenoraError::InvalidPlan(
            "hmac_sha256: chiave HMAC non disponibile (variabile vuota)".into(),
        )),
        Some(value) => value.into_string().map(String::into_bytes).map_err(|_| {
            PlenoraError::InvalidPlan(
                "hmac_sha256: chiave HMAC non valida (valore non UTF-8)".into(),
            )
        }),
    }
}

fn load_hmac_key(key_env: &str) -> Result<Vec<u8>> {
    carica_chiave_hmac(key_env)
}

/// HMAC-SHA256 per riga della concatenazione canonica dei valori.
///
/// Stesso framing di `stable_fingerprint` (separatore di dominio,
/// `framed(nome)`, `framed(tipo)`, byte di presenza, `framed(valore)`), con
/// separatore `b"plenora-hmac-sha256-v1\0"`. La chiave arriva SOLO dalla
/// variabile d'ambiente il cui nome e' `key_env`.
///
/// Nessun null sorgente e' rifiutato; la politica decide l'uscita:
/// `empty` (default) hasha il null come valore vuoto, con intestazione e
/// byte di presenza 1; `null` rende nulla la riga d'uscita alla prima
/// colonna nulla; `skip` omette la colonna nulla (intestazione compresa)
/// dal messaggio.
///
/// # Errors
///
/// - `InvalidPlan`: nome della colonna di output non valido, `key_env` vuoto,
///   `columns` vuoto, colonna ripetuta, chiave HMAC non disponibile
///   (variabile d'ambiente assente, vuota o non UTF-8);
/// - `ResourceLimit`: lunghezza di un valore oltre `u64` nel framing;
/// - `Schema`: colonna assente dal batch, valore non rappresentabile come
///   testo o tipo non coperto dal profilo scalare (gli errori di
///   `scalar_as_string`, per le colonne lette: con `null` quelle dopo la
///   prima nulla di una riga non si leggono).
pub fn hmac_sha256(batch: &RecordBatch, config: &HmacSha256) -> Result<RecordBatch> {
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
    // hmac non ha null_policy=error: Empty (default), Null e Skip danno
    // sempre un'uscita, nessun pre-rifiuto.
    // Stati ipad/opad dalla chiave, con il separatore di dominio gia'
    // assorbito nello stato interno: e' il prefisso costante di ogni
    // messaggio (con `skip` l'intestazione di una colonna nulla manca, e il
    // prefisso costante si ferma qui).
    let (mut inner_base, outer_base) = hmac_sha256_states(&key);
    inner_base.update(b"plenora-hmac-sha256-v1\0");
    // Framing costante per colonna e accesso tipizzato ai valori,
    // precomputati una volta per batch come in `fingerprint_rows`.
    let mut headers = Vec::with_capacity(config.columns.len());
    let mut accesses = Vec::with_capacity(config.columns.len());
    for (name, index) in config.columns.iter().zip(&indices) {
        let column = batch.column(*index);
        let data_type = column.data_type().to_string();
        let mut header = Vec::with_capacity(name.len() + data_type.len() + 16);
        framed_vec(&mut header, name.as_bytes(), "hmac_sha256")?;
        framed_vec(&mut header, data_type.as_bytes(), "hmac_sha256")?;
        headers.push(header);
        accesses.push(column_access(column.as_ref()));
    }
    // Con `null` la riga e' nulla alla prima colonna nulla, e le colonne
    // seguenti non si leggono (quindi nessun loro errore di conversione).
    let values = colonna_digest(batch.num_rows(), 64, |row, appunti, uscita| {
        let Appunti { messaggio, testo } = appunti;
        messaggio.clear();
        for (header, access) in headers.iter().zip(&accesses) {
            match testo_cella(access, row, testo)? {
                Some(value) => {
                    messaggio.extend_from_slice(header);
                    messaggio.push(1);
                    framed_vec(messaggio, value.as_bytes(), "hmac_sha256")?;
                }
                None => match config.null_policy {
                    HmacNullPolicy::Empty => {
                        messaggio.extend_from_slice(header);
                        messaggio.push(1);
                        framed_vec(messaggio, b"", "hmac_sha256")?;
                    }
                    HmacNullPolicy::Null => return Ok(true),
                    HmacNullPolicy::Skip => {}
                },
            }
        }
        accoda_esadecimale(
            uscita,
            &hmac_sha256_with_states(&inner_base, &outer_base, messaggio),
        );
        Ok(false)
    })?;
    replace_or_append(
        batch,
        &config.output_column,
        DataType::Utf8,
        matches!(config.null_policy, HmacNullPolicy::Null),
        Arc::new(values),
    )
}

/// Forma della maschera di `table.mask_data`; i caratteri si contano come
/// caratteri Unicode.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MaskType {
    /// Codice fiscale: 3 caratteri iniziali e 3 finali in chiaro, `*`.
    Cf,
    /// Email: della parte prima dell'ultima `@` resta il primo carattere,
    /// gli altri diventano `*` (un solo `*` se ha al piu' un carattere); il
    /// dominio resta. Senza `@` il testo resta com'e'.
    Email,
    /// Telefono: si tengono cifre e `+`; con almeno 6 caratteri, 3 iniziali
    /// e 4 finali in chiaro, altrimenti il testo originale resta com'e'.
    Phone,
    /// IBAN: 4 caratteri iniziali e 4 finali in chiaro, `*`.
    Iban,
    /// `chars_start` iniziali e `chars_end` finali in chiaro, `mask_char`
    /// al posto degli altri (default).
    Custom,
}

const fn default_mask_type() -> MaskType {
    MaskType::Custom
}

/// Una mascheratura di `table.mask_data`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Masking {
    /// Colonna da mascherare, letta come testo.
    pub column: String,
    /// Forma della maschera; default `custom`.
    #[serde(default = "default_mask_type")]
    pub mask_type: MaskType,
    /// Caratteri iniziali lasciati in chiaro (`custom`; assente: 3).
    #[serde(default)]
    pub chars_start: Option<usize>,
    /// Caratteri finali lasciati in chiaro (`custom`; assente: 3).
    #[serde(default)]
    pub chars_end: Option<usize>,
    /// Carattere di maschera (`custom`; assente: `*`).
    #[serde(default)]
    pub mask_char: Option<String>,
}

impl Masking {
    /// Caratteri iniziali in chiaro.
    #[must_use]
    pub fn chars_start(&self) -> usize {
        self.chars_start.unwrap_or(3)
    }

    /// Caratteri finali in chiaro.
    #[must_use]
    pub fn chars_end(&self) -> usize {
        self.chars_end.unwrap_or(3)
    }

    /// Carattere di maschera.
    #[must_use]
    pub fn mask_char(&self) -> &str {
        self.mask_char.as_deref().unwrap_or("*")
    }

    /// `chars_start`, `chars_end` e `mask_char` valgono solo per
    /// `mask_type = custom`: gli altri tipi hanno una forma fissa, e un
    /// parametro scritto per loro si rifiuta invece di essere ignorato. La
    /// chiamano il kernel e l'analisi dei contratti.
    ///
    /// # Errors
    ///
    /// `InvalidPlan` se un parametro di `custom` accompagna un altro tipo.
    pub fn verifica_parametri(&self) -> Result<()> {
        if !matches!(self.mask_type, MaskType::Custom)
            && (self.chars_start.is_some() || self.chars_end.is_some() || self.mask_char.is_some())
        {
            return Err(PlenoraError::InvalidPlan(
                "chars_start, chars_end e mask_char ammessi solo con mask_type=custom".into(),
            ));
        }
        Ok(())
    }
}

/// Config di `table.mask_data`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MaskData {
    /// Mascherature, applicate in sequenza; almeno una.
    pub maskings: Vec<Masking>,
    /// `true` sovrascrive la colonna, `false` (default) scrive
    /// `<colonna>_masked`.
    #[serde(default)]
    pub overwrite: bool,
}

impl MaskData {
    /// Senza `overwrite` ogni voce scrive `<colonna>_masked` leggendo la
    /// colonna originale: due voci sulla stessa colonna scriverebbero la
    /// stessa uscita, e la prima non avrebbe effetto. Si rifiuta. Con
    /// `overwrite` le voci si applicano in sequenza e hanno effetto
    /// entrambe. La chiamano il kernel e l'analisi dei contratti.
    ///
    /// # Errors
    ///
    /// `InvalidPlan` per una colonna ripetuta senza `overwrite`.
    pub fn verifica_colonne(&self) -> Result<()> {
        if self.overwrite {
            return Ok(());
        }
        for (posizione, masking) in self.maskings.iter().enumerate() {
            if self.maskings[..posizione]
                .iter()
                .any(|prima| prima.column == masking.column)
            {
                return Err(PlenoraError::InvalidPlan(format!(
                    "colonna {} ripetuta in maskings senza overwrite: la prima voce \
                     non avrebbe effetto",
                    masking.column
                )));
            }
        }
        Ok(())
    }
}

/// Maschera i caratteri centrali di `value` mantenendo `start` caratteri
/// iniziali ed `end` finali.
///
/// Lavora su indici di byte (via `char_indices`) senza materializzare un
/// `Vec<char>`; l'oracolo nei test e' `mask_middle_reference`.
fn mask_middle(value: &str, start: usize, end: usize, mask: char) -> String {
    let char_count = value.chars().count();
    if char_count <= start.saturating_add(end) {
        return value.to_owned();
    }
    // Offset di byte dopo i primi `start` caratteri e all'inizio degli
    // ultimi `end` caratteri.
    let start_byte = value
        .char_indices()
        .nth(start)
        .map_or(value.len(), |(index, _)| index);
    let end_byte = if end == 0 {
        value.len()
    } else {
        value
            .char_indices()
            .rev()
            .nth(end - 1)
            .map_or(0, |(index, _)| index)
    };
    let mask_count = char_count - start - end;
    let mut out =
        String::with_capacity(start_byte + mask_count * mask.len_utf8() + (value.len() - end_byte));
    out.push_str(&value[..start_byte]);
    out.extend(std::iter::repeat_n(mask, mask_count));
    out.push_str(&value[end_byte..]);
    out
}

fn mask(value: &str, config: &Masking) -> Result<String> {
    Ok(match config.mask_type {
        MaskType::Cf => mask_middle(value, 3, 3, '*'),
        MaskType::Iban => mask_middle(value, 4, 4, '*'),
        MaskType::Email => {
            if let Some((local, domain)) = value.rsplit_once('@') {
                let first = match local.chars().count() {
                    0 | 1 => "*".into(),
                    count => format!(
                        "{}{}",
                        local.chars().next().unwrap_or_default(),
                        "*".repeat(count - 1)
                    ),
                };
                format!("{first}@{domain}")
            } else {
                value.to_owned()
            }
        }
        MaskType::Phone => {
            let compact: String = value
                .chars()
                .filter(|ch| ch.is_ascii_digit() || *ch == '+')
                .collect();
            if compact.chars().count() < 6 {
                value.to_owned()
            } else {
                mask_middle(&compact, 3, 4, '*')
            }
        }
        MaskType::Custom => {
            let mut chars = config.mask_char().chars();
            let character = chars
                .next()
                .ok_or_else(|| PlenoraError::InvalidPlan("mask_char vuoto".into()))?;
            if chars.next().is_some() {
                return Err(PlenoraError::InvalidPlan(
                    "mask_char deve essere un carattere".into(),
                ));
            }
            mask_middle(value, config.chars_start(), config.chars_end(), character)
        }
    })
}

/// Colonne mascherate secondo le configurazioni di `config.maskings`.
///
/// Con `overwrite` la colonna originale e' sostituita, altrimenti il
/// risultato va in `<colonna>_masked` (`Utf8` nullable). I null restano
/// null. Le voci si applicano in sequenza sul batch gia' mascherato dalle
/// precedenti. Con `Limits::default()`: [`mask_data_con_limiti`] con i
/// limiti del chiamante.
///
/// # Errors
///
/// Come [`mask_data_con_limiti`].
pub fn mask_data(batch: &RecordBatch, config: &MaskData) -> Result<RecordBatch> {
    mask_data_con_limiti(batch, config, &crate::Limits::default())
}

/// [`mask_data`] con i limiti del chiamante (il runner passa i suoi).
///
/// Ogni valore mascherato non supera `limits.max_string_bytes`: una
/// maschera puo' allungare la cella (un carattere di un byte coperto da un
/// `mask_char` di quattro byte).
///
/// # Errors
///
/// - `InvalidPlan`: `maskings` vuoto, nome della colonna di output non valido,
///   `mask_char` vuoto o piu' di un carattere (tipo `custom`),
///   `chars_start`/`chars_end`/`mask_char` con un altro tipo, colonna
///   ripetuta senza `overwrite` ([`MaskData::verifica_colonne`]);
/// - `ResourceLimit`: un valore mascherato oltre `limits.max_string_bytes`;
/// - `Schema`: colonna assente dal batch; valore non rappresentabile come
///   testo o tipo non coperto dal profilo scalare (gli errori di
///   `scalar_as_string`).
pub fn mask_data_con_limiti(
    batch: &RecordBatch,
    config: &MaskData,
    limits: &crate::Limits,
) -> Result<RecordBatch> {
    let mascherato = |value: &str, masking: &Masking| -> Result<String> {
        let testo = mask(value, masking)?;
        crate::verifica_testo_prodotto("mask_data", testo.len(), limits)?;
        Ok(testo)
    };
    if config.maskings.is_empty() {
        return Err(PlenoraError::InvalidPlan(
            "mask_data richiede configurazioni".into(),
        ));
    }
    for masking in &config.maskings {
        masking.verifica_parametri()?;
    }
    config.verifica_colonne()?;
    let mut result = batch.clone();
    for masking in &config.maskings {
        let index = column_index(&result, &masking.column)?;
        let output = if config.overwrite {
            masking.column.clone()
        } else {
            format!("{}_masked", masking.column)
        };
        validate_output_name(&output)?;
        let column = result.column(index).clone();
        // Fast path Utf8: valori in prestito dallo `StringArray`, senza
        // `scalar_as_string` per riga. Stessi byte, null ed errori del
        // percorso scalare, su cui ricadono gli altri tipi.
        let values = if let Some(strings) = column.as_any().downcast_ref::<StringArray>() {
            let mut builder = StringBuilder::with_capacity(
                result.num_rows(),
                result.num_rows().saturating_mul(8).min(64 * 1024 * 1024),
            );
            for row in 0..result.num_rows() {
                if strings.is_null(row) {
                    builder.append_null();
                } else {
                    builder.append_value(&mascherato(strings.value(row), masking)?);
                }
            }
            Arc::new(builder.finish())
        } else {
            let values = (0..result.num_rows())
                .map(|row| {
                    scalar_as_string(column.as_ref(), row).and_then(|value| {
                        value.map(|value| mascherato(&value, masking)).transpose()
                    })
                })
                .collect::<Result<Vec<_>>>()?;
            Arc::new(StringArray::from(values))
        };
        result = replace_or_append(&result, &output, DataType::Utf8, true, values)?;
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    // -------------------------------------------------------------------
    // Test-oracolo di `mask_data`: implementazioni di riferimento di
    // `mask_middle`/`mask`/`mask_data`, indipendenti dal percorso ottimizzato.
    // -------------------------------------------------------------------

    use super::*;
    use crate::test_support::{assert_batches_identical, single_column_batch};
    use plenora_core::arrow::array::{
        Array, ArrayRef, BooleanArray, Date32Array, Float64Array, Int64Array, UInt64Array,
    };
    use plenora_core::arrow::schema::{Field, Schema};
    use serde_json::json;

    #[test]
    fn hash_error_policy_rejects_every_null_source_row() {
        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("a", DataType::Utf8, true),
                Field::new("b", DataType::Utf8, true),
            ])),
            vec![
                Arc::new(StringArray::from(vec![Some("x"), None, Some("z")])),
                Arc::new(StringArray::from(vec![Some("y"), Some("q"), None])),
            ],
        )
        .expect("fixture");
        let errors = [
            md5_hash(
                &batch,
                &Md5Hash {
                    columns: vec!["a".into(), "b".into()],
                    output_column: "digest".into(),
                    normalize: true,
                    null_policy: HashNullPolicy::Error,
                    null_literal: None,
                },
            )
            .expect_err("md5 ha accettato null vietati"),
            sha256_hash(
                &batch,
                &Sha256Hash {
                    columns: vec!["a".into(), "b".into()],
                    output_column: "digest".into(),
                    normalize: true,
                    null_policy: HashNullPolicy::Error,
                    null_literal: None,
                },
            )
            .expect_err("sha256 ha accettato null vietati"),
        ];
        for error in errors {
            let report = error
                .row_diagnostics()
                .expect("diagnostica hash row-scoped mancante");
            assert_eq!(report.observed_total, 2);
            assert_eq!(report.total, Some(2));
            assert_eq!(
                report
                    .examples
                    .iter()
                    .map(|example| (example.source_index, example.column.as_deref()))
                    .collect::<Vec<_>>(),
                vec![(1, Some("a")), (2, Some("b"))]
            );
        }
    }

    #[test]
    fn hash_null_policy_empty_and_literal_preserve_historic_output() {
        // Il rifiuto row-scoped e' solo per null_policy=error; empty
        // (default) e literal sostituiscono il null con il valore che la
        // politica dichiara (testo vuoto o letterale).
        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("a", DataType::Utf8, true),
                Field::new("b", DataType::Utf8, true),
            ])),
            vec![
                Arc::new(StringArray::from(vec![Some("x"), None, Some("z")])),
                Arc::new(StringArray::from(vec![Some("y"), Some("q"), None])),
            ],
        )
        .expect("fixture");
        let digest_column = |output: &RecordBatch, name: &str| {
            output
                .column_by_name(name)
                .expect("colonna digest")
                .as_any()
                .downcast_ref::<StringArray>()
                .expect("utf8")
                .iter()
                .map(|cell| cell.map(str::to_owned))
                .collect::<Vec<_>>()
        };
        // Empty: null -> "" nella concatenazione U+001F.
        let md5_empty = md5_hash(
            &batch,
            &Md5Hash {
                columns: vec!["a".into(), "b".into()],
                output_column: "digest".into(),
                normalize: false,
                null_policy: HashNullPolicy::Empty,
                null_literal: None,
            },
        )
        .expect("null_policy=empty non rifiuta (output storico)");
        let expected_empty: Vec<Option<String>> = ["x\u{1f}y", "\u{1f}q", "z\u{1f}"]
            .iter()
            .map(|parts| {
                let mut digest = Md5::new();
                digest.update(parts.as_bytes());
                // Oracolo indipendente da `push_hex`: se lo usasse, il test
                // confronterebbe l'implementazione con se stessa.
                let mut hex = String::new();
                for byte in digest.finalize() {
                    let _ = write!(hex, "{byte:02x}");
                }
                Some(hex)
            })
            .collect();
        assert_eq!(digest_column(&md5_empty, "digest"), expected_empty);
        // Literal: null -> letterale dichiarato. Oracolo: sostituire i null
        // col letterale in una colonna tutta valida deve dare lo STESSO
        // digest (la sostituzione e' la semantica dichiarata).
        let sha_literal = sha256_hash(
            &batch,
            &Sha256Hash {
                columns: vec!["a".into(), "b".into()],
                output_column: "digest".into(),
                normalize: false,
                null_policy: HashNullPolicy::Literal,
                null_literal: Some("<NULL>".to_owned()),
            },
        )
        .expect("null_policy=literal non rifiuta (output storico)");
        let substituted = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("a", DataType::Utf8, false),
                Field::new("b", DataType::Utf8, false),
            ])),
            vec![
                Arc::new(StringArray::from(vec!["x", "<NULL>", "z"])),
                Arc::new(StringArray::from(vec!["y", "q", "<NULL>"])),
            ],
        )
        .expect("oracolo");
        let oracle = sha256_hash(
            &substituted,
            &Sha256Hash {
                columns: vec!["a".into(), "b".into()],
                output_column: "digest".into(),
                normalize: false,
                null_policy: HashNullPolicy::Empty,
                null_literal: None,
            },
        )
        .expect("oracolo senza null");
        assert_eq!(
            digest_column(&sha_literal, "digest"),
            digest_column(&oracle, "digest"),
            "null->letterale diverso dalla sostituzione storica"
        );
    }

    /// Oracolo indipendente di `mask_middle`.
    fn mask_middle_reference(value: &str, start: usize, end: usize, mask: char) -> String {
        let chars: Vec<char> = value.chars().collect();
        if chars.len() <= start.saturating_add(end) {
            return value.to_owned();
        }
        let mut out: String = chars[..start].iter().collect();
        out.extend(std::iter::repeat_n(mask, chars.len() - start - end));
        out.extend(chars[chars.len() - end..].iter());
        out
    }

    /// Oracolo indipendente di `mask`: compone `mask_middle_reference`, mai
    /// il percorso di produzione.
    fn mask_reference(value: &str, config: &Masking) -> Result<String> {
        Ok(match config.mask_type {
            MaskType::Cf => mask_middle_reference(value, 3, 3, '*'),
            MaskType::Iban => mask_middle_reference(value, 4, 4, '*'),
            MaskType::Email => {
                if let Some((local, domain)) = value.rsplit_once('@') {
                    let first = match local.chars().count() {
                        0 | 1 => "*".into(),
                        count => format!(
                            "{}{}",
                            local.chars().next().unwrap_or_default(),
                            "*".repeat(count - 1)
                        ),
                    };
                    format!("{first}@{domain}")
                } else {
                    value.to_owned()
                }
            }
            MaskType::Phone => {
                let compact: String = value
                    .chars()
                    .filter(|ch| ch.is_ascii_digit() || *ch == '+')
                    .collect();
                if compact.chars().count() < 6 {
                    value.to_owned()
                } else {
                    mask_middle_reference(&compact, 3, 4, '*')
                }
            }
            MaskType::Custom => {
                let mut chars = config.mask_char().chars();
                let character = chars
                    .next()
                    .ok_or_else(|| PlenoraError::InvalidPlan("mask_char vuoto".into()))?;
                if chars.next().is_some() {
                    return Err(PlenoraError::InvalidPlan(
                        "mask_char deve essere un carattere".into(),
                    ));
                }
                mask_middle_reference(value, config.chars_start(), config.chars_end(), character)
            }
        })
    }

    /// Oracolo indipendente di `mask_data`.
    fn mask_data_reference(batch: &RecordBatch, config: &MaskData) -> Result<RecordBatch> {
        if config.maskings.is_empty() {
            return Err(PlenoraError::InvalidPlan(
                "mask_data richiede configurazioni".into(),
            ));
        }
        let mut result = batch.clone();
        for masking in &config.maskings {
            let index = column_index(&result, &masking.column)?;
            let output = if config.overwrite {
                masking.column.clone()
            } else {
                format!("{}_masked", masking.column)
            };
            validate_output_name(&output)?;
            let values = (0..result.num_rows())
                .map(|row| {
                    scalar_as_string(result.column(index).as_ref(), row).and_then(|value| {
                        value
                            .map(|value| mask_reference(&value, masking))
                            .transpose()
                    })
                })
                .collect::<Result<Vec<_>>>()?;
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

    fn masking(column: &str, mask_type: MaskType) -> Masking {
        Masking {
            column: column.into(),
            mask_type,
            chars_start: None,
            chars_end: None,
            mask_char: None,
        }
    }

    // -------------------------------------------------------------------
    // Test di `stable_fingerprint`: known-answer sull'encoding canonico,
    // sensibilita' e determinismo.
    // -------------------------------------------------------------------

    fn fingerprint_fixture() -> RecordBatch {
        RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("a", DataType::Utf8, true),
                Field::new("b", DataType::Int64, true),
            ])),
            vec![
                Arc::new(StringArray::from(vec![Some("x"), None, Some("")])),
                Arc::new(Int64Array::from(vec![Some(42), Some(7), Some(7)])),
            ],
        )
        .expect("fixture")
    }

    fn fingerprint_values(batch: &RecordBatch, config: &StableFingerprint) -> Vec<String> {
        let output = stable_fingerprint(batch, config).expect("fingerprint");
        let column = output
            .column_by_name(&config.output_column)
            .expect("colonna fingerprint")
            .as_any()
            .downcast_ref::<StringArray>()
            .expect("utf8");
        (0..column.len())
            .map(|row| column.value(row).to_owned())
            .collect()
    }

    fn fingerprint_config(columns: &[&str]) -> StableFingerprint {
        StableFingerprint {
            columns: columns.iter().map(|name| (*name).to_owned()).collect(),
            output_column: "fingerprint".into(),
            algorithm: FingerprintAlgorithm::Sha256,
        }
    }

    #[test]
    fn stable_fingerprint_known_answer_sha256_and_md5() {
        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("a", DataType::Utf8, true),
                Field::new("b", DataType::Int64, true),
            ])),
            vec![
                Arc::new(StringArray::from(vec![Some("x")])),
                Arc::new(Int64Array::from(vec![Some(42)])),
            ],
        )
        .expect("fixture");
        // Known-answer sull'encoding: "plenora-fingerprint-v1\0" +
        // framed("a") framed("Utf8") 0x01 framed("x") +
        // framed("b") framed("Int64") 0x01 framed("42").
        let sha = fingerprint_values(&batch, &fingerprint_config(&["a", "b"]));
        assert_eq!(
            sha,
            vec!["4e1f09e49d945536920b917446d005d90e62d4bb2cc231557ed6a8bef6c4f21f"]
        );
        let md5 = fingerprint_values(
            &batch,
            &StableFingerprint {
                algorithm: FingerprintAlgorithm::Md5,
                ..fingerprint_config(&["a", "b"])
            },
        );
        assert_eq!(md5, vec!["79a680c00d6257fa3364bb4b40e7963d"]);
    }

    #[test]
    fn stable_fingerprint_is_deterministic_and_sensitive() {
        let batch = fingerprint_fixture();
        let config = fingerprint_config(&["a", "b"]);
        let first = fingerprint_values(&batch, &config);
        let second = fingerprint_values(&batch, &config);
        assert_eq!(first, second, "stesso input -> stesso hash");
        assert!(first.iter().all(|value| value.len() == 64));
        // Righe diverse -> hash diversi; null ed empty string distinti.
        assert_ne!(first[0], first[1]);
        assert_ne!(first[1], first[2], "null e stringa vuota devono differire");
        // L'ordine delle colonne di config cambia il digest.
        let swapped = fingerprint_values(&batch, &fingerprint_config(&["b", "a"]));
        assert_ne!(first, swapped);
        // Un subset di colonne ignora le altre.
        let subset = fingerprint_values(&batch, &fingerprint_config(&["b"]));
        assert_eq!(subset[1], subset[2], "stessa colonna b -> stesso hash");
        assert_ne!(subset[0], subset[1]);
    }

    #[test]
    fn stable_fingerprint_defaults_and_validation() {
        let batch = fingerprint_fixture();
        // Default: tutte le colonne in ordine di schema, output "fingerprint",
        // algoritmo sha256.
        let decoded: StableFingerprint = serde_json::from_value(json!({})).expect("defaults");
        assert!(decoded.columns.is_empty());
        assert_eq!(decoded.output_column, "fingerprint");
        assert!(matches!(decoded.algorithm, FingerprintAlgorithm::Sha256));
        let output = stable_fingerprint(&batch, &decoded).expect("fingerprint");
        assert_eq!(output.num_columns(), 3);
        assert_eq!(output.num_rows(), batch.num_rows());
        let schema = output.schema();
        let field = schema
            .field_with_name("fingerprint")
            .expect("colonna output");
        assert_eq!(field.data_type(), &DataType::Utf8);
        assert!(!field.is_nullable());
        // Tutte le colonne di default = stesso hash di columns esplicite nello
        // stesso ordine.
        assert_eq!(
            fingerprint_values(&batch, &decoded),
            fingerprint_values(&batch, &fingerprint_config(&["a", "b"]))
        );
        // Errori: colonna mancante, duplicata, campo config sconosciuto.
        assert!(stable_fingerprint(&batch, &fingerprint_config(&["missing"])).is_err());
        assert!(stable_fingerprint(&batch, &fingerprint_config(&["a", "a"])).is_err());
        assert!(serde_json::from_value::<StableFingerprint>(json!({"algo": "sha256"})).is_err());
        assert!(stable_fingerprint(
            &batch,
            &StableFingerprint {
                output_column: " ".into(),
                ..fingerprint_config(&["a"])
            },
        )
        .is_err());
    }

    // -------------------------------------------------------------------
    // Test di `hmac_sha256`: known-answer RFC 2104 sul framing canonico, null
    // policy, e la chiave che non compare mai negli errori.
    // -------------------------------------------------------------------

    fn hmac_fixture() -> RecordBatch {
        RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("a", DataType::Utf8, true),
                Field::new("b", DataType::Int64, true),
            ])),
            vec![
                Arc::new(StringArray::from(vec![Some("x"), None])),
                Arc::new(Int64Array::from(vec![Some(42), Some(7)])),
            ],
        )
        .expect("fixture")
    }

    fn hmac_config(columns: &[&str], key_env: &str) -> HmacSha256 {
        HmacSha256 {
            columns: columns.iter().map(|name| (*name).to_owned()).collect(),
            key_env: key_env.into(),
            output_column: "hmac".into(),
            null_policy: HmacNullPolicy::Empty,
        }
    }

    fn hmac_values(batch: &RecordBatch, config: &HmacSha256) -> Vec<Option<String>> {
        let output = hmac_sha256(batch, config).expect("hmac");
        let column = output
            .column_by_name(&config.output_column)
            .expect("colonna hmac")
            .as_any()
            .downcast_ref::<StringArray>()
            .expect("utf8");
        (0..column.len())
            .map(|row| (!column.is_null(row)).then(|| column.value(row).to_owned()))
            .collect()
    }

    #[test]
    fn hmac_sha256_known_answers() {
        // Known-answer (Python hmac/hashlib) su: "plenora-hmac-sha256-v1\0" +
        // framed("a") framed("Utf8") 0x01 framed("x") + framed("b")
        // framed("Int64") 0x01 framed("42"), chiave da env.
        std::env::set_var("PLENORA_HMAC_KAT_KEY", "plenora-hmac-test-key");
        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("a", DataType::Utf8, true),
                Field::new("b", DataType::Int64, true),
            ])),
            vec![
                Arc::new(StringArray::from(vec![Some("x")])),
                Arc::new(Int64Array::from(vec![Some(42)])),
            ],
        )
        .expect("fixture");
        let values = hmac_values(&batch, &hmac_config(&["a", "b"], "PLENORA_HMAC_KAT_KEY"));
        assert_eq!(
            values,
            vec![Some(
                "46424a38201bbe2cf03b90d3d48444bd54dc41a70002f4991a146138ca4e0d10".to_owned()
            )]
        );
        // Chiave piu' lunga del blocco (100 byte): percorso key = H(key).
        std::env::set_var("PLENORA_HMAC_KAT_LONG", "k".repeat(100));
        let values = hmac_values(&batch, &hmac_config(&["a", "b"], "PLENORA_HMAC_KAT_LONG"));
        assert_eq!(
            values,
            vec![Some(
                "d9893ec92c4162280504db2c21c83dd75ae2106033f716baa1fcc6804a2b1a82".to_owned()
            )]
        );
    }

    #[test]
    fn hmac_sha256_null_policies() {
        // hmac non ha null_policy=error: Empty (default), Null e Skip danno
        // sempre un'uscita, nessun rifiuto.
        std::env::set_var("PLENORA_HMAC_NULL_KEY", "plenora-hmac-test-key");
        let batch = hmac_fixture();
        // Empty: il null di riga 1 entra nel framing come campo vuoto.
        let empty = hmac_values(
            &batch,
            &HmacSha256 {
                null_policy: HmacNullPolicy::Empty,
                ..hmac_config(&["a"], "PLENORA_HMAC_NULL_KEY")
            },
        );
        assert!(
            empty.iter().all(Option::is_some),
            "empty policy: digest atteso su ogni riga"
        );
        // Null: riga con null -> digest null in output.
        let nulled = hmac_values(
            &batch,
            &HmacSha256 {
                null_policy: HmacNullPolicy::Null,
                ..hmac_config(&["a"], "PLENORA_HMAC_NULL_KEY")
            },
        );
        assert_eq!(
            nulled.iter().map(Option::is_some).collect::<Vec<_>>(),
            vec![true, false]
        );
        // Skip: colonna null esclusa dal messaggio, digest presente.
        let skipped = hmac_values(
            &batch,
            &HmacSha256 {
                null_policy: HmacNullPolicy::Skip,
                ..hmac_config(&["a"], "PLENORA_HMAC_NULL_KEY")
            },
        );
        assert!(
            skipped.iter().all(Option::is_some),
            "skip policy: digest atteso su ogni riga"
        );
    }

    #[test]
    fn hmac_sha256_is_deterministic_and_sensitive() {
        std::env::set_var("PLENORA_HMAC_DET_KEY", "plenora-hmac-test-key");
        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("a", DataType::Utf8, false),
                Field::new("b", DataType::Int64, false),
            ])),
            vec![
                Arc::new(StringArray::from(vec!["x", "y"])),
                Arc::new(Int64Array::from(vec![42, 7])),
            ],
        )
        .expect("fixture senza null");
        let config = hmac_config(&["a", "b"], "PLENORA_HMAC_DET_KEY");
        let first = hmac_values(&batch, &config);
        let second = hmac_values(&batch, &config);
        assert_eq!(first, second, "stesso input -> stesso hmac");
        // Ordine delle colonne e chiave diversa cambiano il digest.
        let swapped = hmac_values(&batch, &hmac_config(&["b", "a"], "PLENORA_HMAC_DET_KEY"));
        assert_ne!(first, swapped);
        std::env::set_var("PLENORA_HMAC_DET_KEY2", "altra-chiave");
        let other_key = hmac_values(&batch, &hmac_config(&["a", "b"], "PLENORA_HMAC_DET_KEY2"));
        assert_ne!(first, other_key);
    }

    #[test]
    fn hmac_sha256_key_never_leaks_in_errors() {
        let secret = "chiave-segreta-DA-NON-RIVELARE-12345";
        std::env::set_var("PLENORA_HMAC_LEAK_NAME", secret);
        let batch = hmac_fixture();
        // 1. Variabile assente: l'errore non rivela ne' il NOME della
        //    variabile ne' (ovviamente) il valore.
        std::env::remove_var("PLENORA_HMAC_LEAK_MISSING");
        let error = hmac_sha256(&batch, &hmac_config(&["a"], "PLENORA_HMAC_LEAK_MISSING"))
            .expect_err("variabile assente");
        let display = error.to_string();
        let debug = format!("{error:?}");
        assert!(
            !display.contains("PLENORA_HMAC_LEAK_MISSING"),
            "nome variabile in {display}"
        );
        assert!(
            !debug.contains("PLENORA_HMAC_LEAK_MISSING"),
            "nome variabile in {debug}"
        );
        // 2. Variabile vuota: stesso errore generico, niente valore/nome.
        std::env::set_var("PLENORA_HMAC_LEAK_EMPTY", "");
        let error = hmac_sha256(&batch, &hmac_config(&["a"], "PLENORA_HMAC_LEAK_EMPTY"))
            .expect_err("variabile vuota");
        assert!(!error.to_string().contains("PLENORA_HMAC_LEAK_EMPTY"));
        // 3. Errori successivi alla lettura della chiave (colonna mancante):
        //    il valore segreto non compare in nessuna forma.
        let error = hmac_sha256(
            &batch,
            &hmac_config(&["missing_column"], "PLENORA_HMAC_LEAK_NAME"),
        )
        .expect_err("colonna mancante");
        let display = error.to_string();
        let debug = format!("{error:?}");
        assert!(!display.contains(secret), "chiave in {display}");
        assert!(!debug.contains(secret), "chiave in {debug}");
        // 4. Errori di validazione config: idem.
        let config = HmacSha256 {
            columns: vec!["a".into(), "a".into()],
            ..hmac_config(&["a"], "PLENORA_HMAC_LEAK_NAME")
        };
        let error = hmac_sha256(&batch, &config).expect_err("colonna ripetuta");
        assert!(!error.to_string().contains(secret));
    }

    /// Un valore che non e' UTF-8 (byte `0xff` su Unix, surrogato isolato su
    /// Windows).
    fn valore_non_utf8() -> std::ffi::OsString {
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStringExt;
            std::ffi::OsString::from_vec(vec![b'k', 0xff])
        }
        #[cfg(windows)]
        {
            use std::os::windows::ffi::OsStringExt;
            std::ffi::OsString::from_wide(&[u16::from(b'k'), 0xD800])
        }
    }

    #[test]
    fn hmac_sha256_chiave_non_utf8_e_un_errore_esplicito() {
        // Prima la lettura con `env::var` la trattava come variabile assente,
        // mentre il runner (`var_os`) la accettava: verdetti diversi sulla
        // stessa variabile. Ora una sola funzione, con una causa propria.
        std::env::set_var("PLENORA_HMAC_NON_UTF8", valore_non_utf8());
        let errore = carica_chiave_hmac("PLENORA_HMAC_NON_UTF8").expect_err("non UTF-8");
        assert!(matches!(errore, PlenoraError::InvalidPlan(_)), "{errore:?}");
        assert!(errore.to_string().contains("non UTF-8"), "{errore}");
        assert!(!errore.to_string().contains("PLENORA_HMAC_NON_UTF8"));
        let errore = hmac_sha256(
            &hmac_fixture(),
            &hmac_config(&["a"], "PLENORA_HMAC_NON_UTF8"),
        )
        .expect_err("kernel");
        assert!(errore.to_string().contains("non UTF-8"), "{errore}");
        std::env::remove_var("PLENORA_HMAC_NON_UTF8");
        // Assente e vuota restano distinte.
        std::env::remove_var("PLENORA_HMAC_NON_UTF8_ASSENTE");
        assert!(carica_chiave_hmac("PLENORA_HMAC_NON_UTF8_ASSENTE")
            .expect_err("assente")
            .to_string()
            .contains("variabile assente"));
        std::env::set_var("PLENORA_HMAC_NON_UTF8_VUOTA", "");
        assert!(carica_chiave_hmac("PLENORA_HMAC_NON_UTF8_VUOTA")
            .expect_err("vuota")
            .to_string()
            .contains("variabile vuota"));
        std::env::set_var("PLENORA_HMAC_NON_UTF8_BUONA", "k\u{e9}");
        assert_eq!(
            carica_chiave_hmac("PLENORA_HMAC_NON_UTF8_BUONA").expect("utf-8"),
            "k\u{e9}".as_bytes()
        );
    }

    #[test]
    fn hmac_sha256_config_validation() {
        std::env::set_var("PLENORA_HMAC_CFG_KEY", "k");
        let batch = hmac_fixture();
        // Config strict: campo sconosciuto rifiutato.
        assert!(serde_json::from_value::<HmacSha256>(
            json!({"columns": ["a"], "key_env": "PLENORA_HMAC_CFG_KEY", "surprise": 1})
        )
        .is_err());
        // Defaults: output "hmac", null_policy empty.
        let decoded: HmacSha256 =
            serde_json::from_value(json!({"columns": ["a"], "key_env": "PLENORA_HMAC_CFG_KEY"}))
                .expect("defaults");
        assert_eq!(decoded.output_column, "hmac");
        assert!(matches!(decoded.null_policy, HmacNullPolicy::Empty));
        // Colonne vuote, ripetute, key_env vuoto, nome output non valido.
        assert!(hmac_sha256(&batch, &hmac_config(&[], "PLENORA_HMAC_CFG_KEY")).is_err());
        assert!(hmac_sha256(&batch, &hmac_config(&["a", "a"], "PLENORA_HMAC_CFG_KEY")).is_err());
        assert!(hmac_sha256(&batch, &hmac_config(&["a"], " ")).is_err());
        let config = HmacSha256 {
            output_column: " ".into(),
            ..hmac_config(&["a"], "PLENORA_HMAC_CFG_KEY")
        };
        assert!(hmac_sha256(&batch, &config).is_err());
    }

    #[test]
    fn mask_middle_unicode_e_casi_limite() {
        let cases: &[(&str, usize, usize, char)] = &[
            ("🦀🦀🦀🦀🦀🦀🦀🦀", 2, 2, '*'),
            ("héllo", 3, 3, '*'),   // corta: 5 <= 3+3
            ("héllow", 3, 3, '*'),  // estremo: 6 > 3+3 falso -> invariata
            ("héllowo", 3, 3, '*'), // 7 > 6: un solo carattere mascherato
            ("", 3, 3, '*'),
            ("abcdefgh", 0, 0, '*'),
            ("abcdefgh", 0, 3, '*'),
            ("abcdefgh", 3, 0, '*'),
            ("abcdefgh", 10, 0, '*'),
            ("abcdefgh", 0, 10, '*'),
            ("abcdefgh", 2, 2, '•'), // mask multi-byte
            ("RSSRA85M01H501Z", 3, 3, '*'),
            ("IT60X0542811101000000123456", 4, 4, '*'),
        ];
        for (value, start, end, mask) in cases {
            assert_eq!(
                mask_middle(value, *start, *end, *mask),
                mask_middle_reference(value, *start, *end, *mask),
                "mask_middle({value:?}, {start}, {end}, {mask:?})"
            );
        }
    }

    #[test]
    fn mask_data_tutti_i_tipi_oracle() {
        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("cf", DataType::Utf8, true),
                Field::new("email", DataType::Utf8, true),
                Field::new("phone", DataType::Utf8, true),
                Field::new("iban", DataType::Utf8, true),
                Field::new("text", DataType::Utf8, true),
                Field::new("num", DataType::Int64, true),
                // Non mascherata: deve attraversare intatta, bit per bit.
                Field::new("misura", DataType::Float64, true),
            ])),
            vec![
                Arc::new(StringArray::from(vec![
                    Some("RSSRA85M01H501Z"),
                    Some("corto"),
                    None,
                    Some("🦀🦀🦀🦀🦀🦀🦀🦀🦀"),
                ])),
                Arc::new(StringArray::from(vec![
                    Some("mario.rossi@example.com"),
                    Some("a@b.it"),
                    Some("@dominio.it"),
                    Some("senza-chiocciola"),
                ])),
                Arc::new(StringArray::from(vec![
                    Some("+39 333 123 4567"),
                    Some("12345"),
                    Some("12 34"),
                    None,
                ])),
                Arc::new(StringArray::from(vec![
                    Some("IT60X0542811101000000123456"),
                    Some("IT60X0"),
                    Some("IT60X"),
                    None,
                ])),
                Arc::new(StringArray::from(vec![
                    Some("héllo wörld"),
                    Some(""),
                    None,
                    Some("x"),
                ])),
                Arc::new(Int64Array::from(vec![
                    Some(123_456),
                    None,
                    Some(-7),
                    Some(0),
                ])),
                Arc::new(Float64Array::from(vec![
                    Some(f64::from_bits(0x7ff8_0000_0000_0001)), // NaN con payload
                    Some(-0.0),
                    None,
                    Some(1.5),
                ])),
            ],
        )
        .expect("fixture");
        let config = MaskData {
            maskings: vec![
                masking("cf", MaskType::Cf),
                masking("email", MaskType::Email),
                masking("phone", MaskType::Phone),
                masking("iban", MaskType::Iban),
                masking("text", MaskType::Custom),
                masking("num", MaskType::Custom), // percorso generico non-Utf8
            ],
            overwrite: true,
        };
        let fast = mask_data(&batch, &config).expect("fast");
        let reference = mask_data_reference(&batch, &config).expect("ref");
        assert_batches_identical(&fast, &reference);
        // overwrite=false: colonne *_masked aggiunte.
        let config = MaskData {
            maskings: config.maskings,
            overwrite: false,
        };
        let fast = mask_data(&batch, &config).expect("fast");
        let reference = mask_data_reference(&batch, &config).expect("ref");
        assert_batches_identical(&fast, &reference);
    }

    #[test]
    #[should_panic(expected = "bit riga 0 colonna x")]
    fn il_confronto_distingue_i_payload_nan() {
        // Due NaN con payload diversi hanno lo stesso testo: il confronto
        // condiviso di `test_support` deve comunque separarli.
        let batch = |bits: u64| {
            single_column_batch(
                "x",
                Arc::new(Float64Array::from(vec![Some(f64::from_bits(bits))])),
                DataType::Float64,
                true,
            )
        };
        assert_batches_identical(&batch(0x7ff8_0000_0000_0001), &batch(0x7ff8_0000_0000_0002));
    }

    #[test]
    fn mask_data_input_vuoto_oracle() {
        let batch = single_column_batch(
            "text",
            Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
            DataType::Utf8,
            true,
        );
        let config = MaskData {
            maskings: vec![masking("text", MaskType::Custom)],
            overwrite: true,
        };
        let fast = mask_data(&batch, &config).expect("fast");
        let reference = mask_data_reference(&batch, &config).expect("ref");
        assert_batches_identical(&fast, &reference);
    }

    #[test]
    fn mask_data_errori_identici() {
        let batch = single_column_batch(
            "text",
            Arc::new(StringArray::from(vec![Some("abcdefgh")])),
            DataType::Utf8,
            true,
        );
        // mask_char vuoto e multi-carattere: stesso errore.
        for mask_char in ["", "**"] {
            let config = MaskData {
                maskings: vec![Masking {
                    mask_char: Some(mask_char.into()),
                    ..masking("text", MaskType::Custom)
                }],
                overwrite: true,
            };
            let fast = mask_data(&batch, &config);
            let reference = mask_data_reference(&batch, &config);
            assert_eq!(
                format!("{:?}", fast.expect_err("fast deve fallire")),
                format!("{:?}", reference.expect_err("ref deve fallire"))
            );
        }
        // Nessuna configurazione e colonna mancante: stesso errore.
        let config = MaskData {
            maskings: Vec::new(),
            overwrite: true,
        };
        let fast = mask_data(&batch, &config);
        let reference = mask_data_reference(&batch, &config);
        assert_eq!(
            format!("{:?}", fast.expect_err("fast deve fallire")),
            format!("{:?}", reference.expect_err("ref deve fallire"))
        );
        let config = MaskData {
            maskings: vec![masking("manca", MaskType::Custom)],
            overwrite: true,
        };
        let fast = mask_data(&batch, &config);
        let reference = mask_data_reference(&batch, &config);
        assert_eq!(
            format!("{:?}", fast.expect_err("fast deve fallire")),
            format!("{:?}", reference.expect_err("ref deve fallire"))
        );
    }

    // -------------------------------------------------------------------
    // Test-oracolo di `stable_fingerprint` e `hmac_sha256`: implementazioni
    // di riferimento indipendenti dal percorso ottimizzato, digest confrontati
    // riga per riga su una fixture con null, NaN, -0.0, unicode e tipi su
    // entrambi i percorsi.
    // -------------------------------------------------------------------

    /// Oracolo indipendente di `framed_digest`.
    fn framed_digest_reference<D: Digest>(digest: &mut D, value: &[u8]) -> Result<()> {
        let length = u64::try_from(value.len()).map_err(|_| {
            PlenoraError::InvalidPlan("stable_fingerprint: valore troppo grande".into())
        })?;
        digest.update(length.to_be_bytes());
        digest.update(value);
        Ok(())
    }

    /// Oracolo indipendente di `fingerprint_rows`.
    fn fingerprint_rows_reference<D: Digest>(
        batch: &RecordBatch,
        names: &[String],
        indices: &[usize],
    ) -> Result<Vec<String>> {
        (0..batch.num_rows())
            .map(|row| {
                let mut digest = D::new();
                digest.update(b"plenora-fingerprint-v1\0");
                for (name, index) in names.iter().zip(indices) {
                    framed_digest_reference(&mut digest, name.as_bytes())?;
                    framed_digest_reference(
                        &mut digest,
                        batch.column(*index).data_type().to_string().as_bytes(),
                    )?;
                    match scalar_as_string(batch.column(*index).as_ref(), row)? {
                        Some(value) => {
                            digest.update([1]);
                            framed_digest_reference(&mut digest, value.as_bytes())?;
                        }
                        None => digest.update([0]),
                    }
                }
                let digest = digest.finalize();
                let mut hex = String::with_capacity(digest.len() * 2);
                for byte in digest {
                    let _ = write!(hex, "{byte:02x}");
                }
                Ok(hex)
            })
            .collect()
    }

    /// Oracolo indipendente di `stable_fingerprint`.
    fn stable_fingerprint_reference(
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
            FingerprintAlgorithm::Sha256 => {
                fingerprint_rows_reference::<Sha256>(batch, &names, &indices)?
            }
            FingerprintAlgorithm::Md5 => {
                fingerprint_rows_reference::<Md5>(batch, &names, &indices)?
            }
        };
        replace_or_append(
            batch,
            &config.output_column,
            DataType::Utf8,
            false,
            Arc::new(StringArray::from(values)),
        )
    }

    /// Frame dell'oracolo HMAC, con l'errore di `hmac_sha256`.
    fn framed_bytes(message: &mut Vec<u8>, value: &[u8]) -> Result<()> {
        framed_vec(message, value, "hmac_sha256")
    }

    /// Oracolo indipendente di `hmac_sha256_digest`.
    fn hmac_sha256_digest_reference(key: &[u8], message: &[u8]) -> [u8; 32] {
        const BLOCK: usize = 64;
        let mut block = [0_u8; BLOCK];
        if key.len() > BLOCK {
            let hashed = Sha256::digest(key);
            block[..hashed.len()].copy_from_slice(&hashed);
        } else {
            block[..key.len()].copy_from_slice(key);
        }
        let mut inner = Sha256::new();
        for byte in block {
            inner.update([byte ^ 0x36]);
        }
        inner.update(message);
        let inner = inner.finalize();
        let mut outer = Sha256::new();
        for byte in block {
            outer.update([byte ^ 0x5c]);
        }
        outer.update(inner);
        let digest = outer.finalize();
        let mut output = [0_u8; 32];
        output.copy_from_slice(&digest);
        output
    }

    /// Oracolo indipendente di `hmac_sha256`.
    fn hmac_sha256_reference(batch: &RecordBatch, config: &HmacSha256) -> Result<RecordBatch> {
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
        let values = (0..batch.num_rows())
            .map(|row| {
                let mut message = b"plenora-hmac-sha256-v1\0".to_vec();
                for (name, index) in config.columns.iter().zip(&indices) {
                    let array = batch.column(*index).as_ref();
                    match scalar_as_string(array, row)? {
                        Some(value) => {
                            framed_bytes(&mut message, name.as_bytes())?;
                            framed_bytes(&mut message, array.data_type().to_string().as_bytes())?;
                            message.push(1);
                            framed_bytes(&mut message, value.as_bytes())?;
                        }
                        None => match config.null_policy {
                            HmacNullPolicy::Empty => {
                                framed_bytes(&mut message, name.as_bytes())?;
                                framed_bytes(
                                    &mut message,
                                    array.data_type().to_string().as_bytes(),
                                )?;
                                message.push(1);
                                framed_bytes(&mut message, b"")?;
                            }
                            HmacNullPolicy::Null => return Ok(None),
                            HmacNullPolicy::Skip => {}
                        },
                    }
                }
                let digest = hmac_sha256_digest_reference(&key, &message);
                let mut hex = String::with_capacity(digest.len() * 2);
                for byte in digest {
                    let _ = write!(hex, "{byte:02x}");
                }
                Ok(Some(hex))
            })
            .collect::<Result<Vec<_>>>()?;
        replace_or_append(
            batch,
            &config.output_column,
            DataType::Utf8,
            matches!(config.null_policy, HmacNullPolicy::Null),
            Arc::new(StringArray::from(values)),
        )
    }

    /// Fixture oracolo: null, NaN, -0.0, unicode, stringa vuota; tipi su
    /// percorso tipizzato (Utf8/Int64/Float64/Boolean/UInt64) e su percorso
    /// scalare (Date32).
    fn oracle_fixture() -> RecordBatch {
        let columns: Vec<ArrayRef> = vec![
            Arc::new(StringArray::from(vec![
                Some("héllo 🦀"),
                None,
                Some(""),
                Some("x"),
            ])),
            Arc::new(Int64Array::from(vec![
                Some(-42),
                Some(0),
                None,
                Some(i64::MAX),
            ])),
            Arc::new(Float64Array::from(vec![
                Some(f64::NAN),
                Some(-0.0),
                Some(3.5),
                None,
            ])),
            Arc::new(BooleanArray::from(vec![
                Some(true),
                None,
                Some(false),
                Some(true),
            ])),
            Arc::new(UInt64Array::from(vec![
                Some(u64::MAX),
                Some(0),
                None,
                Some(7),
            ])),
            Arc::new(Date32Array::from(vec![
                Some(0),
                Some(19_000),
                None,
                Some(-1),
            ])),
        ];
        RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("utf8", DataType::Utf8, true),
                Field::new("int64", DataType::Int64, true),
                Field::new("float64", DataType::Float64, true),
                Field::new("boolean", DataType::Boolean, true),
                Field::new("uint64", DataType::UInt64, true),
                Field::new("date32", DataType::Date32, true),
            ])),
            columns,
        )
        .expect("fixture oracle")
    }

    /// Valori (con null) della colonna digest di un batch di output.
    fn output_strings(output: &RecordBatch, name: &str) -> Vec<Option<String>> {
        let column = output
            .column_by_name(name)
            .expect("colonna output")
            .as_any()
            .downcast_ref::<StringArray>()
            .expect("utf8");
        (0..column.len())
            .map(|row| (!column.is_null(row)).then(|| column.value(row).to_owned()))
            .collect()
    }

    #[test]
    fn stable_fingerprint_oracle_tutti_i_percorsi() {
        let batch = oracle_fixture();
        let subsets: Vec<Vec<&str>> = vec![
            vec!["utf8", "int64", "float64", "boolean", "uint64", "date32"],
            vec!["float64", "utf8"],
            vec!["int64"],
            vec!["date32", "boolean"],
        ];
        for columns in subsets {
            for algorithm in [FingerprintAlgorithm::Sha256, FingerprintAlgorithm::Md5] {
                let config = StableFingerprint {
                    columns: columns.iter().map(|name| (*name).to_owned()).collect(),
                    output_column: "fingerprint".into(),
                    algorithm,
                };
                let fast = output_strings(
                    &stable_fingerprint(&batch, &config).expect("fast"),
                    "fingerprint",
                );
                let reference = output_strings(
                    &stable_fingerprint_reference(&batch, &config).expect("ref"),
                    "fingerprint",
                );
                assert_eq!(
                    fast, reference,
                    "subset {columns:?} algoritmo {algorithm:?}"
                );
            }
        }
        // Default (colonne omesse = tutte, ordine di schema), sha256 e md5.
        for algorithm in [FingerprintAlgorithm::Sha256, FingerprintAlgorithm::Md5] {
            let config = StableFingerprint {
                columns: Vec::new(),
                output_column: "fingerprint".into(),
                algorithm,
            };
            let fast = output_strings(
                &stable_fingerprint(&batch, &config).expect("fast"),
                "fingerprint",
            );
            let reference = output_strings(
                &stable_fingerprint_reference(&batch, &config).expect("ref"),
                "fingerprint",
            );
            assert_eq!(fast, reference, "default algoritmo {algorithm:?}");
        }
    }

    #[test]
    fn hmac_sha256_oracle_tutti_i_percorsi() {
        std::env::set_var("PLENORA_HMAC_ORACLE_KEY", "chiave-oracolo-🦀-unicode");
        let batch = oracle_fixture();
        let subsets: Vec<Vec<&str>> = vec![
            vec!["utf8", "int64", "float64", "boolean", "uint64", "date32"],
            vec!["float64", "utf8"],
            vec!["date32"],
        ];
        for columns in subsets {
            for null_policy in [
                HmacNullPolicy::Empty,
                HmacNullPolicy::Null,
                HmacNullPolicy::Skip,
            ] {
                let config = HmacSha256 {
                    columns: columns.iter().map(|name| (*name).to_owned()).collect(),
                    key_env: "PLENORA_HMAC_ORACLE_KEY".into(),
                    output_column: "hmac".into(),
                    null_policy,
                };
                // Nessuna politica null rifiuta; il percorso veloce deve
                // coincidere con l'oracolo indipendente.
                let fast = output_strings(&hmac_sha256(&batch, &config).expect("fast"), "hmac");
                let reference = output_strings(
                    &hmac_sha256_reference(&batch, &config).expect("ref"),
                    "hmac",
                );
                assert_eq!(fast, reference, "subset {columns:?} policy {null_policy:?}");
            }
        }
    }
}

#[cfg(test)]
#[path = "security_hash_oracolo.rs"]
mod hash_oracolo;
