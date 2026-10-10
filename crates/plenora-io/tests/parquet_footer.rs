//! Limiti della decodifica del footer Parquet: profondità dello schema e
//! tetto di memoria (fork `vendor/parquet-60.0.0-eof`,
//! `patches/parquet-footer-budget.patch`, lo stesso delta del fork di
//! plenora-IO-tools).
//!
//! * `parquet` converte lo schema del footer per ricorsione, senza un tetto
//!   di profondità: uno schema **valido** di qualche migliaio di livelli
//!   esauriva lo stack (`STATUS_STACK_OVERFLOW`, un aborto che nessuna
//!   barriera ferma). Ora oltre [`MAX_PROFONDITA_SCHEMA`] è un errore prima
//!   della conversione, e dal confine `ResourceLimit`.
//! * `SchemaDescriptor::new` dà a ogni foglia il percorso intero dalla
//!   radice: il costo cresce col quadrato dei byte del footer. Ora si stima
//!   prima di convertire e si addebita al tetto di memoria
//!   ([`budget_del_footer`]), come ogni prenotazione del decoder: elenchi,
//!   row group e la loro capacità di colonne, i testi copiati.
//! * Column index e offset index hanno decoder senza tetto: con un tetto si
//!   rifiutano prima di leggerne un byte (questo crate non li chiede).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod comune;

use std::sync::Arc;

use parquet::arrow::arrow_reader::{ArrowReaderOptions, ParquetRecordBatchReaderBuilder};
use parquet::arrow::ArrowWriter;
use parquet::basic::{Repetition, Type as Fisico};
use parquet::file::metadata::{PageIndexPolicy, ParquetMetaDataOptions, ParquetMetaDataReader};
use parquet::file::properties::WriterProperties;
use parquet::file::writer::SerializedFileWriter;
use parquet::schema::types::Type;
use plenora_core::arrow::array::{ArrayRef, Int32Array, RecordBatch};
use plenora_core::arrow::schema::{DataType, Field, Schema};
use plenora_core::ErrorCategory;
use plenora_io::parquet_io::{budget_del_footer, MAX_PROFONDITA_SCHEMA};
use plenora_io::{leggi_tabella_con_limiti, Formato, LimitiLettura};

use comune::cartella;

// --- File validi scritti con lo schema Parquet ---------------------------

/// Un Parquet senza righe con uno schema dato (radice `schema`).
fn con_schema(campi: Vec<Arc<Type>>) -> Vec<u8> {
    let radice = Arc::new(
        Type::group_type_builder("schema")
            .with_fields(campi)
            .build()
            .unwrap(),
    );
    let mut byte = Vec::new();
    let scrittore = SerializedFileWriter::new(
        &mut byte,
        radice,
        Arc::new(WriterProperties::builder().build()),
    )
    .unwrap();
    scrittore.close().unwrap();
    byte
}

fn foglia(nome: &str) -> Arc<Type> {
    Arc::new(
        Type::primitive_type_builder(nome, Fisico::INT32)
            .with_repetition(Repetition::OPTIONAL)
            .build()
            .unwrap(),
    )
}

/// Una catena di `livelli` gruppi annidati (radice esclusa) con `foglie`
/// colonne nel gruppo più interno; ogni gruppo ha il nome `nome_gruppo(i)`.
fn catena(livelli: usize, foglie: usize, nome_gruppo: impl Fn(usize) -> String) -> Vec<u8> {
    let mut campi: Vec<Arc<Type>> = (0..foglie).map(|i| foglia(&format!("f{i}"))).collect();
    for livello in (0..livelli).rev() {
        campi = vec![Arc::new(
            Type::group_type_builder(&nome_gruppo(livello))
                .with_repetition(Repetition::OPTIONAL)
                .with_fields(campi)
                .build()
                .unwrap(),
        )];
    }
    con_schema(campi)
}

fn dal_confine(byte: &[u8], limiti: &LimitiLettura) -> plenora_core::Result<usize> {
    let dir = cartella();
    let percorso = dir.path().join("f.parquet");
    std::fs::write(&percorso, byte).unwrap();
    leggi_tabella_con_limiti(&percorso, Some(Formato::Parquet), u64::MAX, limiti)
        .map(|tabella| tabella.num_rows())
}

fn diretto(byte: &[u8], opzioni: ArrowReaderOptions) -> Result<(), String> {
    let dir = cartella();
    let percorso = dir.path().join("d.parquet");
    std::fs::write(&percorso, byte).unwrap();
    let file = std::fs::File::open(&percorso).unwrap();
    ParquetRecordBatchReaderBuilder::try_new_with_options(file, opzioni)
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// Lo schema profondo: 3000 gruppi annidati, un file valido di qualche
/// decina di KiB (ben sotto il tetto dei metadati). Senza il limite la
/// lettura esauriva lo stack (`STATUS_STACK_OVERFLOW` su Windows,
/// `SIGSEGV` da stack overflow su Linux: il processo di test muore). Ora è
/// un errore tipizzato, dal decoder e dal confine.
#[test]
fn uno_schema_profondo_e_un_errore_e_non_uno_stack_overflow() {
    // In un thread con uno stack ampio: è lo **scrittore** di `parquet` a
    // ricorrere anche lui sullo schema, e la prova non deve morire prima
    // di arrivare al lettore.
    let byte = std::thread::Builder::new()
        .stack_size(256 * 1024 * 1024)
        .spawn(|| catena(3000, 1, |i| format!("g{i}")))
        .unwrap()
        .join()
        .unwrap();
    assert!(byte.len() < 1024 * 1024, "{} byte", byte.len());
    let testo = diretto(&byte, ArrowReaderOptions::new()).expect_err("schema profondo");
    assert!(
        testo.contains(&format!("deeper than {MAX_PROFONDITA_SCHEMA} levels")),
        "{testo}"
    );
    let errore = dal_confine(&byte, &LimitiLettura::default()).expect_err("schema profondo");
    assert_eq!(errore.category(), ErrorCategory::ResourceLimit, "{errore}");
}

/// Il confine del limite, esatto: un elemento dello schema può avere al più
/// `MAX_PROFONDITA_SCHEMA - 1` antenati (la radice compresa). Una foglia
/// sotto `livelli` gruppi ne ha `livelli + 1`: si legge fino a
/// `MAX_PROFONDITA_SCHEMA - 2` livelli, e un annidamento reale (otto) sta
/// lontano dal limite.
#[test]
fn la_profondita_massima_e_un_confine_esatto() {
    for livelli in [8, MAX_PROFONDITA_SCHEMA - 2] {
        let byte = catena(livelli, 1, |i| format!("g{i}"));
        assert_eq!(
            dal_confine(&byte, &LimitiLettura::default()).map_err(|e| e.to_string()),
            Ok(0),
            "{livelli} livelli"
        );
    }
    let byte = catena(MAX_PROFONDITA_SCHEMA - 1, 1, |i| format!("g{i}"));
    let errore = dal_confine(&byte, &LimitiLettura::default()).expect_err("oltre il limite");
    assert_eq!(errore.category(), ErrorCategory::ResourceLimit, "{errore}");
}

/// Il costo quadratico dei percorsi delle foglie. Venti gruppi con nomi di
/// 4 KiB e 512 foglie nel più interno: un footer di circa 100 KiB, ma
/// `SchemaDescriptor::new` copia 80 KiB di percorso per ognuna delle 512
/// foglie, circa 40 MiB. Con un tetto dei metadati di 1 MiB (tetto del
/// footer 16 MiB) il costo stimato si rifiuta prima di convertire; con il
/// tetto predefinito (256 MiB) lo stesso file si legge. Senza il tetto
/// del footer si leggeva in entrambi i casi, a qualunque costo.
#[test]
fn il_costo_quadratico_dei_percorsi_e_addebitato_al_tetto() {
    let byte = catena(20, 512, |i| format!("{i:0>4096}"));
    assert!(byte.len() < 1024 * 1024, "{} byte", byte.len());
    let stretti = LimitiLettura {
        max_byte_metadati: 1024 * 1024,
        ..LimitiLettura::default()
    };
    assert_eq!(budget_del_footer(u64::MAX, &stretti), 16 * 1024 * 1024);
    let errore = dal_confine(&byte, &stretti).expect_err("costo oltre il tetto");
    assert_eq!(errore.category(), ErrorCategory::ResourceLimit, "{errore}");
    assert_eq!(
        dal_confine(&byte, &LimitiLettura::default()).map_err(|e| e.to_string()),
        Ok(0)
    );
}

/// I file dello schema profondo e del costo quadratico sono anche semi del
/// target di fuzz `lettura_parquet` (`scripts/genera_corpus_fuzz.py` li
/// prende da `tests/dati/fuzz-footer/`). Con `PLENORA_RIGENERA_SEMI=1` la
/// prova li riscrive; senza, pretende che i file versionati siano questi.
#[test]
fn i_semi_del_fuzz_sono_quelli_delle_prove() {
    let cartella = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("dati")
        .join("fuzz-footer");
    let profondo = std::thread::Builder::new()
        .stack_size(256 * 1024 * 1024)
        .spawn(|| catena(3000, 1, |i| format!("g{i}")))
        .unwrap()
        .join()
        .unwrap();
    let semi = [
        ("schema-profondo", profondo),
        (
            "percorsi-quadratici",
            catena(20, 512, |i| format!("{i:0>4096}")),
        ),
    ];
    let rigenera = std::env::var_os("PLENORA_RIGENERA_SEMI").is_some();
    if rigenera {
        std::fs::create_dir_all(&cartella).unwrap();
    }
    for (nome, byte) in &semi {
        let percorso = cartella.join(format!("{nome}.parquet"));
        if rigenera {
            std::fs::write(&percorso, byte).unwrap();
        } else {
            let versionato = std::fs::read(&percorso).unwrap_or_else(|_| {
                panic!("{nome}: seme assente, rigenerare con PLENORA_RIGENERA_SEMI=1")
            });
            assert_eq!(&versionato, byte, "{nome}: seme diverso dalla prova");
        }
    }
    assert_eq!(std::fs::read_dir(&cartella).unwrap().count(), semi.len());
}

/// Il tetto di memoria viene anche dal budget residuo, non solo dai limiti.
#[test]
fn il_tetto_del_footer_non_supera_il_budget_residuo() {
    let limiti = LimitiLettura::default();
    assert_eq!(budget_del_footer(u64::MAX, &limiti), 256 * 1024 * 1024);
    assert_eq!(budget_del_footer(1000, &limiti), 1000);
}

// --- Thrift compatto scritto a mano (dal fork di plenora-IO-tools) -------
//
// Un row group senza `columns` non lo scrive nessun writer: il footer si
// scrive campo per campo, con le sole forme che servono.

fn varint(mut n: u64, out: &mut Vec<u8>) {
    loop {
        let sette = u8::try_from(n & 0x7F).unwrap();
        n >>= 7;
        if n == 0 {
            out.push(sette);
            return;
        }
        out.push(sette | 0x80);
    }
}

const fn zigzag(n: i64) -> u64 {
    ((n << 1) ^ (n >> 63)).cast_unsigned()
}

/// Intestazione di campo con delta corto: `delta << 4 | tipo`.
fn campo(delta: u8, tipo: u8, out: &mut Vec<u8>) {
    out.push((delta << 4) | tipo);
}

fn intero(delta: u8, tipo: u8, valore: i64, out: &mut Vec<u8>) {
    campo(delta, tipo, out);
    varint(zigzag(valore), out);
}

fn testo(delta: u8, valore: &str, out: &mut Vec<u8>) {
    campo(delta, 8, out);
    varint(valore.len() as u64, out);
    out.extend_from_slice(valore.as_bytes());
}

/// Intestazione di un elenco di struct (tipo elemento 12).
fn elenco_di_struct(quanti: u64, out: &mut Vec<u8>) {
    if quanti < 15 {
        out.push(u8::try_from(quanti << 4).unwrap() | 0x0C);
    } else {
        out.push(0xFC);
        varint(quanti, out);
    }
}

/// `FileMetaData` con uno schema di `foglie` colonne INT32 e `gruppi` row
/// group **senza** il campo `columns`.
fn footer(foglie: u64, gruppi: u64) -> Vec<u8> {
    let mut out = Vec::new();
    intero(1, 5, 2, &mut out); // 1: version i32
    campo(1, 9, &mut out); // 2: schema list<SchemaElement>
    elenco_di_struct(foglie + 1, &mut out);
    // la radice: 4: name, 5: num_children
    testo(4, "schema", &mut out);
    intero(1, 5, i64::try_from(foglie).unwrap(), &mut out);
    out.push(0);
    for i in 0..foglie {
        intero(1, 5, 1, &mut out); // 1: type INT32
        intero(2, 5, 0, &mut out); // 3: repetition REQUIRED
        testo(1, &format!("c{i}"), &mut out); // 4: name
        out.push(0);
    }
    intero(1, 6, 0, &mut out); // 3: num_rows i64
    campo(1, 9, &mut out); // 4: row_groups list<RowGroup>
    elenco_di_struct(gruppi, &mut out);
    for _ in 0..gruppi {
        intero(2, 6, 0, &mut out); // 2: total_byte_size (columns assente)
        intero(1, 6, 0, &mut out); // 3: num_rows
        out.push(0);
    }
    out.push(0);
    out
}

fn decodifica(byte: &[u8], tetto: u64) -> Result<(), String> {
    let opzioni = ParquetMetaDataOptions::new().with_footer_memory_budget(tetto);
    ParquetMetaDataReader::decode_metadata_with_options(byte, Some(&opzioni))
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// Il tetto minimo con cui `byte` si decodifica, cercato per bisezione.
fn tetto_minimo(byte: &[u8]) -> u64 {
    let (mut basso, mut alto) = (0_u64, 1_u64 << 32);
    assert!(
        decodifica(byte, alto).is_ok(),
        "il footer si decodifica con 4 GiB"
    );
    while basso + 1 < alto {
        let medio = basso + (alto - basso) / 2;
        if decodifica(byte, medio).is_ok() {
            alto = medio;
        } else {
            basso = medio;
        }
    }
    alto
}

/// La capacità del row group si addebita prima di essere prenotata.
///
/// `RowGroupMetaDataBuilder::new` prenota un `ColumnChunkMetaData` per
/// foglia prima di leggere un solo chunk. Senza row group lo schema di 512
/// foglie si decodifica con il tetto `minimo`; un row group senza
/// `columns` arriva al rifiuto strutturale solo dopo quella prenotazione, e
/// il tetto per arrivarci deve superare `minimo` di almeno 512 chunk.
#[test]
fn la_capacita_del_row_group_si_addebita_prima_del_costruttore() {
    let minimo = tetto_minimo(&footer(512, 0));
    let con_gruppo = footer(512, 1);
    let arriva_al_rifiuto_strutturale = |tetto: u64| match decodifica(&con_gruppo, tetto) {
        Ok(()) => panic!("un row group senza `columns` si decodifica"),
        Err(messaggio) => !messaggio.contains("exceeds the decoding memory budget"),
    };
    let (mut basso, mut alto) = (minimo, 1_u64 << 32);
    assert!(arriva_al_rifiuto_strutturale(alto));
    while basso + 1 < alto {
        let medio = basso + (alto - basso) / 2;
        if arriva_al_rifiuto_strutturale(medio) {
            alto = medio;
        } else {
            basso = medio;
        }
    }
    // 64 byte per chunk è un minorante largo: `ColumnChunkMetaData` ne
    // occupa alcune centinaia.
    assert!(
        alto - minimo >= 512 * 64,
        "il row group arriva al decoder con {} byte di tetto oltre lo schema: la \
         prenotazione delle 512 colonne non e' stata addebitata",
        alto - minimo
    );
    let messaggio = decodifica(&con_gruppo, alto).expect_err("columns manca");
    assert!(messaggio.contains("columns"), "{messaggio}");
}

fn scrivi_colonne(colonne: usize, righe: i32) -> Vec<u8> {
    let campi: Vec<Field> = (0..colonne)
        .map(|i| Field::new(format!("c{i}"), DataType::Int32, false))
        .collect();
    let schema = Arc::new(Schema::new(campi));
    let array: Vec<ArrayRef> = (0..colonne)
        .map(|_| Arc::new(Int32Array::from((0..righe).collect::<Vec<i32>>())) as ArrayRef)
        .collect();
    let batch = RecordBatch::try_new(Arc::clone(&schema), array).unwrap();
    let mut byte = Vec::new();
    let mut scrittore = ArrowWriter::try_new(&mut byte, schema, None).unwrap();
    scrittore.write(&batch).unwrap();
    scrittore.close().unwrap();
    byte
}

/// Niente doppio conteggio: un file vero con molte colonne si legge con il
/// tetto minimo che il suo footer richiede, una sola volta per chunk.
#[test]
fn un_file_vero_largo_si_legge_entro_il_tetto() {
    let byte = scrivi_colonne(64, 3);
    let n = byte.len();
    let lunghezza = u32::from_le_bytes([byte[n - 8], byte[n - 7], byte[n - 6], byte[n - 5]]);
    let footer = &byte[n - 8 - usize::try_from(lunghezza).unwrap()..n - 8];
    let minimo = tetto_minimo(footer);
    assert!(decodifica(footer, minimo).is_ok());
    assert!(decodifica(footer, minimo - 1).is_err());
}

/// Indici di pagina chiesti con un tetto: un errore esplicito, prima di
/// leggerli; senza tetto si leggono come prima. Questo crate non li chiede
/// (`PageIndexPolicy::Skip`, il default): con il tetto la lettura resta
/// possibile.
#[test]
fn gli_indici_di_pagina_con_un_tetto_sono_rifiutati() {
    let byte = scrivi_colonne(1, 100);
    for politica in [PageIndexPolicy::Optional, PageIndexPolicy::Required] {
        let senza_tetto = ArrowReaderOptions::new().with_page_index_policy(politica);
        assert_eq!(diretto(&byte, senza_tetto), Ok(()), "senza tetto");
        let con_tetto = ArrowReaderOptions::new()
            .with_page_index_policy(politica)
            .with_footer_memory_budget(1 << 30);
        let Err(messaggio) = diretto(&byte, con_tetto) else {
            panic!("indici di pagina letti con un tetto che non li copre ({politica:?})");
        };
        assert!(
            messaggio.contains("page index requested with a footer memory budget"),
            "{messaggio}"
        );
    }
    let solo_offset = ArrowReaderOptions::new()
        .with_offset_index_policy(PageIndexPolicy::Required)
        .with_footer_memory_budget(1 << 30);
    assert!(diretto(&byte, solo_offset).is_err());
    let salta = ArrowReaderOptions::new()
        .with_page_index_policy(PageIndexPolicy::Skip)
        .with_footer_memory_budget(1 << 30);
    assert_eq!(diretto(&byte, salta), Ok(()));
    assert_eq!(
        dal_confine(&byte, &LimitiLettura::default()).map_err(|e| e.to_string()),
        Ok(100)
    );
}

/// La stessa regola per il lettore di file seriale.
#[test]
fn anche_il_lettore_seriale_rifiuta_gli_indici_con_un_tetto() {
    use parquet::file::serialized_reader::{ReadOptionsBuilder, SerializedFileReader};

    let dir = cartella();
    let percorso = dir.path().join("s.parquet");
    std::fs::write(&percorso, scrivi_colonne(1, 100)).unwrap();
    let apri = || std::fs::File::open(&percorso).unwrap();
    let opzioni = ReadOptionsBuilder::new()
        .with_page_index()
        .with_footer_memory_budget(1 << 30)
        .build();
    let Err(errore) = SerializedFileReader::new_with_options(apri(), opzioni).map(|_| ()) else {
        panic!("indici di pagina letti con un tetto che non li copre");
    };
    assert!(
        errore
            .to_string()
            .contains("page index requested with a footer memory budget"),
        "{errore}"
    );
    let senza_tetto = ReadOptionsBuilder::new().with_page_index().build();
    assert!(SerializedFileReader::new_with_options(apri(), senza_tetto).is_ok());
}
