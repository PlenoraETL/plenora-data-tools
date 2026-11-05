//! Due difetti del decoder di `parquet` 60.0.0 (ancora a monte), chiusi nel
//! fork `vendor/parquet-60.0.0-eof` (`patches/parquet-flba-bss.patch`).
//! Portati dal `vendor/parquet` di plenora-IO-tools (a 1b3e264), che li
//! ha trovati adottando lo stesso fork.
//!
//! Ogni prova scrive **un** Parquet valido, controlla che si legga, altera
//! un solo campo dichiarato dal file e pretende un errore: sul decoder di
//! `parquet` chiamato direttamente (`catch_unwind` è dello strumento:
//! distingue l'errore dal panico) e attraverso il confine di lettura di
//! questo crate (`leggi_tabella`), dove un panico diventerebbe un errore
//! `internal` della barriera invece del rifiuto del file.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod comune;

#[macro_use]
#[path = "../../../tests/comune/esito.rs"]
mod esito;

use std::sync::Arc;

use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use parquet::arrow::arrow_writer::ArrowWriterOptions;
use parquet::arrow::ArrowWriter;
use parquet::basic::Encoding;
use parquet::errors::ParquetError;
use parquet::file::properties::{WriterProperties, WriterVersion};
use plenora_core::arrow::array::{ArrayRef, FixedSizeBinaryArray, Float64Array, RecordBatch};
use plenora_core::arrow::schema::{DataType, Field, Schema};
use plenora_core::arrow::ArrowError;
use plenora_core::ErrorCategory;
use plenora_io::{leggi_tabella_con_limiti, Formato, LimitiLettura};

use comune::cartella;

/// Un Parquet di una colonna `x`; `schema_arrow` incorpora `ARROW:schema`
/// (come fa il writer di `parquet`). Senza, il lettore deduce i tipi Arrow
/// dai tipi Parquet, come per un file scritto da un altro strumento.
fn scrivi(colonna: ArrayRef, proprieta: WriterProperties, schema_arrow: bool) -> Vec<u8> {
    let schema = Arc::new(Schema::new(vec![Field::new(
        "x",
        colonna.data_type().clone(),
        false,
    )]));
    let batch = RecordBatch::try_new(Arc::clone(&schema), vec![colonna]).unwrap();
    let mut byte = Vec::new();
    let opzioni = ArrowWriterOptions::new()
        .with_properties(proprieta)
        .with_skip_arrow_metadata(!schema_arrow);
    let mut scrittore = ArrowWriter::try_new_with_options(&mut byte, schema, opzioni).unwrap();
    scrittore.write(&batch).unwrap();
    scrittore.close().unwrap();
    byte
}

/// Stadio e variante originali: un errore di apertura non sostituisce
/// quello di decodifica atteso e una conversione a testo non perde il tipo.
#[derive(Debug)]
#[allow(dead_code)] // Apertura conserva la causa anche nei fallimenti inattesi.
enum ErroreDiretto {
    Apertura(ParquetError),
    Costruzione(ParquetError),
    Lettura(ArrowError),
    Colonna(ParquetError),
}

/// Lettura diretta con `parquet`: batch completi, errore, o il testo del panico.
fn diretta(byte: &[u8]) -> Result<Result<Vec<RecordBatch>, ErroreDiretto>, String> {
    // Da un file: `bytes::Bytes` non e' fra le dipendenze di questo crate.
    let dir = cartella();
    let percorso = dir.path().join("d.parquet");
    std::fs::write(&percorso, byte).unwrap();
    std::panic::catch_unwind(move || {
        let file = std::fs::File::open(&percorso).unwrap();
        let lettore = ParquetRecordBatchReaderBuilder::try_new(file)
            .map_err(ErroreDiretto::Apertura)?
            .build()
            .map_err(ErroreDiretto::Costruzione)?;
        lettore
            .map(|batch| batch.map_err(ErroreDiretto::Lettura))
            .collect()
    })
    .map_err(|carico| {
        carico
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| carico.downcast_ref::<&str>().map(|s| (*s).to_owned()))
            .unwrap_or_else(|| "(payload non testuale)".to_owned())
    })
}

/// La stessa colonna dall'API per colonne di `parquet` (decoder `PLAIN`
/// generico, non quello del lettore Arrow): questo crate non la usa, ma il
/// fork la corregge per la stessa classe (`assert!` sulla larghezza).
fn colonna_diretta(byte: &[u8]) -> Result<Result<usize, ErroreDiretto>, String> {
    use parquet::column::reader::get_typed_column_reader;
    use parquet::data_type::FixedLenByteArrayType;
    use parquet::file::reader::{FileReader, SerializedFileReader};

    let dir = cartella();
    let percorso = dir.path().join("c.parquet");
    std::fs::write(&percorso, byte).unwrap();
    std::panic::catch_unwind(move || {
        let file = std::fs::File::open(&percorso).unwrap();
        let lettore = SerializedFileReader::new(file).map_err(ErroreDiretto::Apertura)?;
        let gruppo = lettore
            .get_row_group(0)
            .map_err(ErroreDiretto::Costruzione)?;
        let colonna = gruppo
            .get_column_reader(0)
            .map_err(ErroreDiretto::Costruzione)?;
        let mut tipizzata = get_typed_column_reader::<FixedLenByteArrayType>(colonna);
        let mut valori = Vec::new();
        let (record, _, _) = tipizzata
            .read_records(16, None, None, &mut valori)
            .map_err(ErroreDiretto::Colonna)?;
        Ok(record)
    })
    .map_err(|carico| {
        carico
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| carico.downcast_ref::<&str>().map(|s| (*s).to_owned()))
            .unwrap_or_else(|| "(payload non testuale)".to_owned())
    })
}

/// Lettura attraverso il confine di questo crate.
fn dal_confine(byte: &[u8]) -> plenora_core::Result<usize> {
    let dir = cartella();
    let percorso = dir.path().join("f.parquet");
    std::fs::write(&percorso, byte).unwrap();
    leggi_tabella_con_limiti(
        &percorso,
        Some(Formato::Parquet),
        u64::MAX,
        &LimitiLettura::default(),
    )
    .map(|tabella| tabella.num_rows())
}

/// L'inizio del footer: la lunghezza sta negli otto byte finali, prima di `PAR1`.
fn inizio_del_footer(byte: &[u8]) -> usize {
    let n = byte.len();
    let lunghezza = u32::from_le_bytes([byte[n - 8], byte[n - 7], byte[n - 6], byte[n - 5]]);
    n - 8 - usize::try_from(lunghezza).unwrap()
}

/// La fixture ha una radice con un figlio `x`, REQUIRED `FIXED_LEN_BYTE_ARRAY(1)`.
/// La sequenza copre l'intero `SchemaElement`: tipo, `type_length`, repetition,
/// nome e STOP. Si identifica l'unico campo reale, non coppie di byte candidate.
#[test]
fn una_larghezza_zero_e_un_errore_e_non_una_divisione_per_zero() {
    let colonna: ArrayRef =
        Arc::new(FixedSizeBinaryArray::try_from_iter(vec![vec![7_u8]; 3].into_iter()).unwrap());
    for schema_arrow in [false, true] {
        let proprieta = WriterProperties::builder()
            .set_dictionary_enabled(false)
            .set_statistics_enabled(parquet::file::properties::EnabledStatistics::None)
            .build();
        let originale = scrivi(Arc::clone(&colonna), proprieta, schema_arrow);
        let atteso = RecordBatch::try_new(
            Arc::new(Schema::new(vec![Field::new(
                "x",
                DataType::FixedSizeBinary(1),
                false,
            )])),
            vec![Arc::clone(&colonna)],
        )
        .expect("fixture attesa");
        assert_successo!(diretta(&originale).expect("nessun panico"), vec![atteso]);
        let inizio = inizio_del_footer(&originale);
        let schema_element = [0x15, 0x0e, 0x15, 0x02, 0x15, 0x00, 0x18, 0x01, b'x', 0x00];
        let siti: Vec<_> = originale[inizio..originale.len() - 8]
            .windows(schema_element.len())
            .enumerate()
            .filter_map(|(i, bytes)| (bytes == schema_element).then_some(inizio + i + 3))
            .collect();
        assert_eq!(siti.len(), 1, "unico SchemaElement della colonna x");
        let mut alterato = originale.clone();
        alterato[siti[0]] = 0;
        assert!(matches!(
            diretta(&alterato).expect("nessun panico"),
            Err(ErroreDiretto::Costruzione(ParquetError::General(message)))
                if message == "invalid FIXED_LEN_BYTE_ARRAY width 0 for the arrow reader"
        ));
        assert!(matches!(
            colonna_diretta(&alterato).expect("nessun panico"),
            Err(ErroreDiretto::Colonna(ParquetError::General(message)))
                if message == "invalid FIXED_LEN_BYTE_ARRAY width 0 for the plain decoder"
        ));
        let categoria = if schema_arrow {
            ErrorCategory::Schema
        } else {
            ErrorCategory::DataMapping
        };
        // API diretta: fasi derivate dalle varianti; il runner da file
        // applica Read quando avvolge questa API (docs/file.md).
        let fase = if schema_arrow {
            plenora_core::ErrorPhase::Validate
        } else {
            plenora_core::ErrorPhase::Write
        };
        assert_rifiuto!(dal_confine(&alterato), categoria, fase);
    }
}

/// `BYTE_STREAM_SPLIT` non è qualificata per la lettura: un file valido
/// scritto da `parquet-rs` si rifiuta, dal fork (testo fisso) e dal confine
/// (`Unsupported`). La correzione del decoder (conteggi concordi oltre i byte
/// della pagina, `index out of bounds` in 60.0.0) resta nel fork, non
/// raggiungibile finché la codifica non è qualificata.
#[test]
fn byte_stream_split_non_e_qualificata() {
    let valori: Vec<f64> = (0..8).map(|i| f64::from(i) + 0.5).collect();
    let proprieta = WriterProperties::builder()
        .set_writer_version(WriterVersion::PARQUET_2_0)
        .set_dictionary_enabled(false)
        .set_encoding(Encoding::BYTE_STREAM_SPLIT)
        .build();
    let byte = scrivi(Arc::new(Float64Array::from(valori)), proprieta, true);
    assert!(matches!(
        diretta(&byte).expect("nessun panico"),
        Err(ErroreDiretto::Lettura(ArrowError::ParquetError(message)))
            if message == format!("NYI: {}: BYTE_STREAM_SPLIT", parquet::basic::ENCODING_NOT_QUALIFIED)
    ));
    assert_rifiuto!(
        dal_confine(&byte),
        ErrorCategory::Unsupported,
        plenora_core::ErrorPhase::Validate
    );
}
