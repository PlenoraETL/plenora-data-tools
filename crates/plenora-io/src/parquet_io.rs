//! Parquet e `GeoParquet`: lettura in un solo `RecordBatch`, scrittura
//! deterministica.
//!
//! **Lettura.** Prima di decodificare si controllano i codec di tutti i
//! column chunk (solo `UNCOMPRESSED`, `SNAPPY`, `ZSTD` sono compilati) e lo
//! schema Arrow incorporato (`ARROW:schema`): se c'è, lo schema che
//! `parquet` applica deve coincidere campo per campo (nome, tipo,
//! nullabilità, metadati) con quello incorporato, perché `parquet` ricade in
//! silenzio sul tipo Parquet quando il tipo incorporato non è applicabile.
//! La lettura usa un batch grande quanto il file (un solo `RecordBatch`
//! anche con più row group: niente `concat_batches`, niente seconda copia);
//! poi i byte vivi esatti contro il budget residuo, e con il metadato di
//! file `geo` la mappatura `GeoParquet` ([`crate::geoparquet`]).
//!
//! **Scrittura.** Proprietà fisse: `created_by` costante (quello di
//! `parquet-rs` alla versione pinnata), formato 1.0 delle pagine, row group
//! di al più [`RIGHE_PER_ROW_GROUP`] righe, statistiche di pagina, niente
//! bloom filter; lo stesso batch dà gli stessi byte. Dopo la scrittura si
//! rilegge il footer del file e se ne verifica lo schema incorporato
//! ([`verifica_schema`]).

use std::collections::BTreeMap;
use std::fs::File;
use std::io::Write;
use std::path::Path;
use std::sync::Arc;

use base64::Engine;
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use parquet::arrow::{ArrowWriter, ARROW_SCHEMA_META_KEY};
use parquet::basic::{Compression, Type as TipoFisico, ZstdLevel};
use parquet::errors::ParquetError;
use parquet::file::metadata::{KeyValue, ParquetMetaData};
use parquet::file::properties::{
    EnabledStatistics, WriterProperties, WriterVersion, DEFAULT_CREATED_BY,
};
use plenora_core::arrow::array::cast::AsArray;
use plenora_core::arrow::array::types::{
    Decimal128Type, Decimal256Type, Decimal32Type, Decimal64Type,
};
use plenora_core::arrow::array::{make_array, Array, ArrayRef, RecordBatch};
use plenora_core::arrow::schema::DataType;
use plenora_core::arrow::schema::{Schema, SchemaRef};
use plenora_core::arrow::select::concat::concat_batches;
use plenora_core::contract::arrow_metadata::GEO_METADATA_KEY;
use plenora_core::memoria::byte_vivi;
use plenora_core::{PlenoraError, Result};

use crate::formato::CompressioneParquet;
use crate::geoparquet;
use crate::memoria::{oltre_il_budget, stima_byte};

/// Righe massime per row group in scrittura.
pub const RIGHE_PER_ROW_GROUP: usize = 1024 * 1024;

/// Errore di `parquet` con un codice nostro, mai il testo della dipendenza
/// (che può contenere valori): la stessa regola di `arrow_error_code`.
#[must_use]
pub fn da_parquet(errore: &ParquetError) -> PlenoraError {
    let codice = match errore {
        ParquetError::General(_) => "general",
        ParquetError::NYI(_) => "not_yet_implemented",
        ParquetError::EOF(_) => "eof",
        ParquetError::ArrowError(_) => "arrow",
        ParquetError::IndexOutOfBound(_, _) => "index_out_of_bound",
        ParquetError::External(_) => "external",
        ParquetError::NeedMoreData(_) | ParquetError::NeedMoreDataRange(_) => "need_more_data",
        _ => "other",
    };
    PlenoraError::DataMapping(format!("parquet error: {codice}"))
}

#[allow(clippy::needless_pass_by_value)] // Per `map_err`, che passa l'errore per valore.
fn da_parquet_valore(errore: ParquetError) -> PlenoraError {
    da_parquet(&errore)
}

/// I codec di tutti i column chunk devono essere fra quelli compilati, e
/// nessuna colonna è `INT96`: `parquet` la converte in nanosecondi con
/// aritmetica che avvolge, e le date oltre ±292 anni dal 1970 diventano
/// altre date senza errore.
fn verifica_colonne(metadati: &ParquetMetaData) -> Result<()> {
    for gruppo in metadati.row_groups() {
        for chunk in gruppo.columns() {
            if chunk.column_descr().physical_type() == TipoFisico::INT96 {
                return Err(PlenoraError::Unsupported(format!(
                    "colonna `{}` INT96 (timestamp legacy): la conversione di parquet-rs \
                     avvolge in silenzio le date fuori da ±292 anni",
                    chunk.column_descr().path()
                )));
            }
            let codec = chunk.compression();
            if !matches!(
                codec,
                Compression::UNCOMPRESSED | Compression::SNAPPY | Compression::ZSTD(_)
            ) {
                return Err(PlenoraError::Unsupported(format!(
                    "codec Parquet {codec} non abilitato (abilitati: UNCOMPRESSED, SNAPPY, ZSTD)"
                )));
            }
        }
    }
    Ok(())
}

/// Le chiavi dei metadati chiave-valore sono uniche, e non contraddicono i
/// metadati dello schema incorporato: `parquet` terrebbe l'ultima di due
/// chiavi uguali, e una chiave del file prevale in silenzio su quella dello
/// schema.
fn verifica_chiavi(metadati: &ParquetMetaData, incorporato: Option<&Schema>) -> Result<()> {
    let mut viste: BTreeMap<&str, Option<&str>> = BTreeMap::new();
    for voce in metadati
        .file_metadata()
        .key_value_metadata()
        .map_or(&[][..], Vec::as_slice)
    {
        if viste
            .insert(voce.key.as_str(), voce.value.as_deref())
            .is_some()
        {
            return Err(PlenoraError::DataMapping(format!(
                "metadati del file con la chiave `{}` ripetuta: documento ambiguo",
                voce.key
            )));
        }
    }
    if let Some(incorporato) = incorporato {
        for (chiave, valore) in incorporato.metadata() {
            if let Some(nel_file) = viste.get(chiave.as_str()) {
                if *nel_file != Some(valore.as_str()) {
                    return Err(PlenoraError::DataMapping(format!(
                        "metadato `{chiave}` diverso fra il file e lo schema Arrow incorporato"
                    )));
                }
            }
        }
    }
    Ok(())
}

/// Ogni decimale sta nella precisione dichiarata del suo tipo: `parquet`
/// scrive i decimali con precisione piccola restringendo il valore
/// (`as i32`/`as i64`, byte troncati), e un valore fuori precisione
/// tornerebbe un altro numero senza errore. Arrow non impone la precisione
/// sui valori, quindi si verifica qui, a ogni profondità (figli di liste e
/// strutture, valori dei dizionari).
fn verifica_decimali(nome: &str, array: &ArrayRef) -> Result<()> {
    let valida = match *array.data_type() {
        DataType::Decimal32(precisione, _) => array
            .as_primitive_opt::<Decimal32Type>()
            .map(|a| a.validate_decimal_precision(precisione).is_ok()),
        DataType::Decimal64(precisione, _) => array
            .as_primitive_opt::<Decimal64Type>()
            .map(|a| a.validate_decimal_precision(precisione).is_ok()),
        DataType::Decimal128(precisione, _) => array
            .as_primitive_opt::<Decimal128Type>()
            .map(|a| a.validate_decimal_precision(precisione).is_ok()),
        DataType::Decimal256(precisione, _) => array
            .as_primitive_opt::<Decimal256Type>()
            .map(|a| a.validate_decimal_precision(precisione).is_ok()),
        _ => Some(true),
    };
    match valida {
        Some(true) => {}
        Some(false) => {
            return Err(PlenoraError::DataMapping(format!(
                "colonna `{nome}`: decimale oltre la precisione del suo tipo"
            )))
        }
        None => {
            return Err(PlenoraError::Internal(
                "array decimale con un tipo incoerente".to_owned(),
            ))
        }
    }
    array
        .to_data()
        .child_data()
        .iter()
        .try_for_each(|figlio| verifica_decimali(nome, &make_array(figlio.clone())))
}

/// Lo schema Arrow incorporato nei metadati chiave-valore, se c'è.
fn schema_incorporato(metadati: &ParquetMetaData) -> Result<Option<Schema>> {
    let Some(voce) = metadati
        .file_metadata()
        .key_value_metadata()
        .and_then(|voci| voci.iter().find(|voce| voce.key == ARROW_SCHEMA_META_KEY))
    else {
        return Ok(None);
    };
    let illeggibile =
        || PlenoraError::DataMapping("schema Arrow incorporato illeggibile".to_owned());
    let testo = voce.value.as_deref().ok_or_else(illeggibile)?;
    let byte = base64::engine::general_purpose::STANDARD
        .decode(testo)
        .map_err(|_| illeggibile())?;
    // Messaggio IPC con prefisso di continuazione (0xFFFFFFFF + lunghezza)
    // o senza, come in `parquet`.
    let fetta = if byte.len() > 8 && byte[0..4] == [255_u8; 4] {
        &byte[8..]
    } else {
        byte.as_slice()
    };
    let messaggio = plenora_core::arrow::ipc::root_as_message(fetta).map_err(|_| illeggibile())?;
    let schema = messaggio
        .header_as_schema()
        .map(plenora_core::arrow::ipc::convert::fb_to_schema)
        .ok_or_else(illeggibile)?;
    Ok(Some(schema))
}

/// Lo schema che `parquet` applica coincide con quello incorporato, campo
/// per campo.
fn verifica_applicato(applicato: &Schema, incorporato: Option<&Schema>) -> Result<()> {
    if let Some(incorporato) = incorporato {
        if applicato.fields() != incorporato.fields() {
            return Err(PlenoraError::Schema(
                "lo schema Arrow incorporato nel file non e' applicabile ai tipi Parquet: \
                 la lettura cambierebbe i tipi"
                    .to_owned(),
            ));
        }
    }
    Ok(())
}

/// Stima dei byte Arrow della tabella decodificata, dal solo footer.
///
/// È il maggiore fra i byte non compressi dei column chunk e i valori (null
/// compresi) per la larghezza fisica di ogni colonna foglia; per
/// `BYTE_ARRAY`, 4 byte di offset per valore più i byte decodificati che il
/// footer dichiara (`unencoded_byte_array_data_bytes`, scritto da
/// `parquet-rs` e da Arrow C++ recenti). È una previsione, non un limite:
/// senza quella statistica un `BYTE_ARRAY` codificato a dizionario si
/// decodifica in valori ripetuti che nessuno dei due termini vede.
#[must_use]
pub fn stima_decodificata(metadati: &ParquetMetaData) -> u64 {
    let mut non_compressi = 0_u64;
    let mut larghezze = 0_u64;
    for gruppo in metadati.row_groups() {
        for chunk in gruppo.columns() {
            non_compressi =
                non_compressi.saturating_add(u64::try_from(chunk.uncompressed_size()).unwrap_or(0));
            let descrittore = chunk.column_descr();
            let per_valore = match descrittore.physical_type() {
                TipoFisico::BOOLEAN => 1,
                TipoFisico::INT32 | TipoFisico::FLOAT | TipoFisico::BYTE_ARRAY => 4,
                TipoFisico::INT64 | TipoFisico::DOUBLE => 8,
                TipoFisico::INT96 => 12,
                TipoFisico::FIXED_LEN_BYTE_ARRAY => {
                    u64::try_from(descrittore.type_length()).unwrap_or(0)
                }
            };
            let decodificati = chunk
                .unencoded_byte_array_data_bytes()
                .and_then(|byte| u64::try_from(byte).ok())
                .unwrap_or(0);
            larghezze = larghezze
                .saturating_add(
                    u64::try_from(chunk.num_values())
                        .unwrap_or(0)
                        .saturating_mul(per_valore),
                )
                .saturating_add(decodificati);
        }
    }
    non_compressi.max(larghezze)
}

/// Picco previsto della lettura: [`FATTORE_LETTURA`] volte
/// [`stima_decodificata`] più [`crate::memoria::MARGINE`] (README, «File»,
/// per le misure).
///
/// # Errors
///
/// `Io`, `DataMapping` dalla lettura del footer.
pub fn picco_lettura_previsto(percorso: &Path) -> Result<u64> {
    let costruttore = ParquetRecordBatchReaderBuilder::try_new(File::open(percorso)?)
        .map_err(da_parquet_valore)?;
    Ok(picco_previsto(costruttore.metadata()))
}

fn picco_previsto(metadati: &ParquetMetaData) -> u64 {
    stima_decodificata(metadati)
        .saturating_mul(FATTORE_LETTURA)
        .saturating_add(crate::memoria::MARGINE)
}

/// Fattore del picco di decodifica sulla stima dal footer.
pub const FATTORE_LETTURA: u64 = 5;

/// Legge un file Parquet (o `GeoParquet`) in un solo `RecordBatch`.
///
/// # Errors
///
/// `Unsupported` per un codec non abilitato; `Schema` per uno schema
/// incorporato non applicabile; `ResourceLimit` se la tabella non sta in
/// `residuo`; quelli di [`geoparquet::applica`]; `DataMapping`, `Io` dalla
/// lettura.
pub fn leggi(percorso: &Path, residuo: u64) -> Result<RecordBatch> {
    let file = File::open(percorso)?;
    let costruttore = ParquetRecordBatchReaderBuilder::try_new(file).map_err(da_parquet_valore)?;
    let metadati = Arc::clone(costruttore.metadata());
    verifica_colonne(&metadati)?;
    let previsto = picco_previsto(&metadati);
    if previsto > residuo {
        return Err(oltre_il_budget(previsto, residuo));
    }
    let schema: SchemaRef = Arc::clone(costruttore.schema());
    let incorporato = schema_incorporato(&metadati)?;
    verifica_chiavi(&metadati, incorporato.as_ref())?;
    verifica_applicato(&schema, incorporato.as_ref())?;
    let righe = usize::try_from(metadati.file_metadata().num_rows())
        .map_err(|_| PlenoraError::DataMapping("numero di righe non valido".to_owned()))?;
    let lettore = costruttore
        .with_batch_size(righe.max(1))
        .build()
        .map_err(da_parquet_valore)?;
    let blocchi = lettore.collect::<std::result::Result<Vec<_>, _>>()?;
    let tabella = match blocchi.len() {
        0 => RecordBatch::new_empty(Arc::clone(&schema)),
        1 => blocchi
            .into_iter()
            .next()
            .ok_or_else(|| PlenoraError::Internal("blocco atteso e assente".to_owned()))?,
        _ => {
            // Non accade con un batch grande quanto il file; se accadesse la
            // ricomposizione conta nel budget.
            let stima = byte_vivi(blocchi.iter())?.saturating_add(stima_byte(&blocchi));
            if stima > residuo {
                return Err(oltre_il_budget(stima, residuo));
            }
            concat_batches(&schema, &blocchi)?
        }
    };
    if tabella.num_rows() != righe {
        return Err(PlenoraError::DataMapping(
            "righe lette diverse da quelle del footer".to_owned(),
        ));
    }
    // Il lettore rende i campi senza i metadati di schema: si rimettono,
    // dopo aver verificato che i campi siano quelli del footer.
    if tabella.schema().fields() != schema.fields() {
        return Err(PlenoraError::Internal(
            "campi letti diversi da quelli del footer".to_owned(),
        ));
    }
    let tabella = tabella.with_schema(Arc::clone(&schema))?;
    let vivi = byte_vivi(std::iter::once(&tabella))?;
    if vivi > residuo {
        return Err(oltre_il_budget(vivi, residuo));
    }
    match schema.metadata().get(GEO_METADATA_KEY).cloned() {
        Some(geo) => geoparquet::applica(&tabella, &geo),
        None => Ok(tabella),
    }
}

fn proprieta(compressione: CompressioneParquet, geo: Option<String>) -> Result<WriterProperties> {
    let codec = match compressione {
        CompressioneParquet::Nessuna => Compression::UNCOMPRESSED,
        CompressioneParquet::Snappy => Compression::SNAPPY,
        CompressioneParquet::Zstd => {
            Compression::ZSTD(ZstdLevel::try_new(3).map_err(da_parquet_valore)?)
        }
    };
    Ok(WriterProperties::builder()
        .set_created_by(DEFAULT_CREATED_BY.to_owned())
        .set_writer_version(WriterVersion::PARQUET_1_0)
        .set_compression(codec)
        .set_max_row_group_row_count(Some(RIGHE_PER_ROW_GROUP))
        .set_statistics_enabled(EnabledStatistics::Page)
        .set_bloom_filter_enabled(false)
        .set_key_value_metadata(
            geo.map(|testo| vec![KeyValue::new(GEO_METADATA_KEY.to_owned(), testo)]),
        )
        .build())
}

/// La tabella che si scriverà davvero (per `GeoParquet`, con lo schema
/// preparato) e le proprietà.
fn da_scrivere(
    tabella: &RecordBatch,
    compressione: CompressioneParquet,
) -> Result<(RecordBatch, WriterProperties)> {
    if tabella.num_columns() == 0 {
        return Err(PlenoraError::Unsupported(
            "Parquet non rappresenta una tabella senza colonne (il numero di righe andrebbe \
             perso): usare Arrow IPC"
                .to_owned(),
        ));
    }
    for (campo, colonna) in tabella.schema().fields().iter().zip(tabella.columns()) {
        verifica_decimali(campo.name(), colonna)?;
    }
    match geoparquet::prepara(tabella)? {
        Some(preparata) => {
            let proprieta = proprieta(compressione, Some(preparata.geo))?;
            Ok((preparata.tabella, proprieta))
        }
        None => Ok((tabella.clone(), proprieta(compressione, None)?)),
    }
}

/// Scrive la tabella come Parquet (`GeoParquet` se ha colonne geometriche) e
/// rende lo schema Arrow scritto, da verificare con [`verifica_schema`].
///
/// # Errors
///
/// `Unsupported` per una tabella senza colonne; quelli di
/// [`geoparquet::prepara`]; `DataMapping` e `Io` dalla codifica.
pub fn scrivi(
    tabella: &RecordBatch,
    uscita: impl Write + Send,
    compressione: CompressioneParquet,
) -> Result<SchemaRef> {
    let (tabella, proprieta) = da_scrivere(tabella, compressione)?;
    let schema = tabella.schema();
    let mut scrittore = ArrowWriter::try_new(uscita, Arc::clone(&schema), Some(proprieta))
        .map_err(da_parquet_valore)?;
    scrittore.write(&tabella).map_err(da_parquet_valore)?;
    scrittore.close().map_err(da_parquet_valore)?;
    Ok(schema)
}

/// Rilegge il footer di un file appena scritto: lo schema incorporato deve
/// essere quello scritto e `parquet` deve applicarlo senza cambiarlo.
///
/// # Errors
///
/// `Schema` se lo schema incorporato manca, è diverso da `scritto` o non si
/// applica; `DataMapping`, `Io` dalla lettura del footer.
pub fn verifica_schema(percorso: &Path, scritto: &Schema) -> Result<()> {
    let costruttore = ParquetRecordBatchReaderBuilder::try_new(File::open(percorso)?)
        .map_err(da_parquet_valore)?;
    let incorporato = schema_incorporato(costruttore.metadata())?.ok_or_else(|| {
        PlenoraError::Schema("schema Arrow incorporato assente nel file scritto".to_owned())
    })?;
    if incorporato.fields() != scritto.fields() || incorporato.metadata() != scritto.metadata() {
        return Err(PlenoraError::Schema(
            "schema Arrow incorporato diverso da quello scritto".to_owned(),
        ));
    }
    verifica_applicato(costruttore.schema(), Some(&incorporato))
}

/// Transitorio previsto della scrittura Parquet.
///
/// Vale [`FATTORE_SCRITTURA`] volte i byte Arrow del row group più grande (il row group in corso si tiene
/// codificato in memoria, con i livelli e i dizionari delle colonne), più
/// [`MARGINE_SCRITTURA`] per i buffer fissi di pagina e dei codec (README,
/// «File», per le misure).
#[must_use]
pub fn transitorio_scrittura(tabella: &RecordBatch) -> u64 {
    crate::memoria::fetta_massima(tabella, RIGHE_PER_ROW_GROUP)
        .saturating_mul(FATTORE_SCRITTURA)
        .saturating_add(MARGINE_SCRITTURA)
}

/// Fattore del transitorio di scrittura sul row group.
pub const FATTORE_SCRITTURA: u64 = 4;

/// Parte fissa del transitorio di scrittura.
pub const MARGINE_SCRITTURA: u64 = 8 * 1024 * 1024;
