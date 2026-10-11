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

use std::sync::Arc;

use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use parquet::arrow::arrow_writer::ArrowWriterOptions;
use parquet::arrow::ArrowWriter;
use parquet::basic::Encoding;
use parquet::file::properties::{WriterProperties, WriterVersion};
use plenora_core::arrow::array::{ArrayRef, FixedSizeBinaryArray, Float64Array, RecordBatch};
use plenora_core::arrow::schema::{Field, Schema};
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

/// Lettura diretta con `parquet`: righe lette, errore, o il testo del panico.
fn diretta(byte: &[u8]) -> Result<Result<usize, String>, String> {
    // Da un file: `bytes::Bytes` non e' fra le dipendenze di questo crate.
    let dir = cartella();
    let percorso = dir.path().join("d.parquet");
    std::fs::write(&percorso, byte).unwrap();
    std::panic::catch_unwind(move || {
        let file = std::fs::File::open(&percorso).unwrap();
        let lettore = ParquetRecordBatchReaderBuilder::try_new(file)
            .map_err(|e| e.to_string())?
            .build()
            .map_err(|e| e.to_string())?;
        let mut righe = 0;
        for batch in lettore {
            righe += batch.map_err(|e| e.to_string())?.num_rows();
        }
        Ok(righe)
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
fn colonna_diretta(byte: &[u8]) -> Result<Result<usize, String>, String> {
    use parquet::column::reader::get_typed_column_reader;
    use parquet::data_type::FixedLenByteArrayType;
    use parquet::file::reader::{FileReader, SerializedFileReader};

    let dir = cartella();
    let percorso = dir.path().join("c.parquet");
    std::fs::write(&percorso, byte).unwrap();
    std::panic::catch_unwind(move || {
        let file = std::fs::File::open(&percorso).unwrap();
        let lettore = SerializedFileReader::new(file).map_err(|e| e.to_string())?;
        let gruppo = lettore.get_row_group(0).map_err(|e| e.to_string())?;
        let colonna = gruppo.get_column_reader(0).map_err(|e| e.to_string())?;
        let mut tipizzata = get_typed_column_reader::<FixedLenByteArrayType>(colonna);
        let mut valori = Vec::new();
        let (record, _, _) = tipizzata
            .read_records(16, None, None, &mut valori)
            .map_err(|e| e.to_string())?;
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

/// Le posizioni dei campi thrift compatti con intestazione `intestazione`
/// (delta 1 e tipo) seguita dal varint zigzag di `valore` in un byte.
fn siti(byte: &[u8], intestazione: u8, valore: u8, dove: std::ops::Range<usize>) -> Vec<usize> {
    let zigzag = valore << 1;
    dove.filter(|&i| byte[i] == intestazione && byte[i + 1] == zigzag)
        .collect()
}

/// Un `FIXED_LEN_BYTE_ARRAY` di larghezza 0, in `PLAIN` e in
/// `BYTE_STREAM_SPLIT`. Lo schema Parquet ammette la larghezza 0, e il
/// lettore Arrow di `parquet` 60.0.0 ci divide sopra (`attempt to divide by
/// zero`). Il file si ottiene scrivendo una colonna di larghezza 1 e
/// riscrivendo `type_length` (i32 dopo un campo contiguo: `0x15`, zigzag(1) =
/// `0x02`) nello schema del footer; i siti candidati sono tutti quelli con
/// lo stesso valore, e nessuno deve panicare.
#[test]
fn una_larghezza_zero_e_un_errore_e_non_una_divisione_per_zero() {
    let colonna: ArrayRef =
        Arc::new(FixedSizeBinaryArray::try_from_iter(vec![vec![7_u8]; 3].into_iter()).unwrap());
    // `BYTE_STREAM_SPLIT` non è più qualificata per la lettura (prova sotto):
    // la larghezza 0 si prova in `PLAIN`, con e senza `ARROW:schema`.
    for (bss, schema_arrow) in [(false, false), (false, true)] {
        let mut proprieta = WriterProperties::builder().set_dictionary_enabled(false);
        if bss {
            proprieta = proprieta
                .set_writer_version(WriterVersion::PARQUET_2_0)
                .set_encoding(Encoding::BYTE_STREAM_SPLIT);
        }
        let originale = scrivi(Arc::clone(&colonna), proprieta.build(), schema_arrow);
        assert_eq!(diretta(&originale), Ok(Ok(3)), "bss={bss}: controfattuale");
        let footer = inizio_del_footer(&originale)..originale.len() - 9;
        let candidati = siti(&originale, 0x15, 1, footer);
        let mut rifiutati = 0;
        for sito in &candidati {
            let mut alterato = originale.clone();
            alterato[sito + 1] = 0x00;
            match diretta(&alterato) {
                Err(panico) => panic!("bss={bss}, sito {sito}: panico «{panico}»"),
                Ok(Err(testo)) if testo.contains("FIXED_LEN_BYTE_ARRAY width 0") => {
                    rifiutati += 1;
                    if !bss {
                        match colonna_diretta(&alterato) {
                            Err(panico) => {
                                panic!("sito {sito}: API per colonne in panico «{panico}»")
                            }
                            Ok(Ok(record)) => panic!("sito {sito}: {record} valori di larghezza 0"),
                            Ok(Err(testo)) => assert!(
                                testo.contains("FIXED_LEN_BYTE_ARRAY width 0"),
                                "sito {sito}: {testo}"
                            ),
                        }
                    }
                    // La categoria esatta: con `ARROW:schema` il confine
                    // rifiuta prima lo schema incorporato non applicabile
                    // (`Schema`); senza, il decoder (`DataMapping`), e mai
                    // con il testo della barriera, che converte un panico
                    // proprio in `DataMapping` («parquet in panico»).
                    let errore = dal_confine(&alterato).expect_err("larghezza 0 rifiutata");
                    let attesa = if schema_arrow {
                        ErrorCategory::Schema
                    } else {
                        ErrorCategory::DataMapping
                    };
                    assert_eq!(errore.category(), attesa, "{errore}");
                    assert!(!errore.to_string().contains("panico"), "{errore}");
                }
                Ok(_) => {}
            }
        }
        assert!(
            rifiutati > 0,
            "bss={bss}: nessuno dei siti {candidati:?} e' `type_length`: il layout \
             del writer e' cambiato, e la prova va rifatta"
        );
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
    match diretta(&byte) {
        Ok(Err(testo)) => assert!(
            testo.contains(parquet::basic::ENCODING_NOT_QUALIFIED),
            "{testo}"
        ),
        altro => panic!("atteso il rifiuto della codifica: {altro:?}"),
    }
    let errore = dal_confine(&byte).expect_err("codifica non qualificata");
    assert_eq!(errore.category(), ErrorCategory::Unsupported, "{errore}");
}
