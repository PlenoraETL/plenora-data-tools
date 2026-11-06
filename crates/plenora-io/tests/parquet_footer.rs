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

#[macro_use]
#[path = "../../../tests/comune/esito.rs"]
mod esito;

use std::sync::Arc;

use parquet::arrow::arrow_reader::{ArrowReaderOptions, ParquetRecordBatchReaderBuilder};
use parquet::arrow::ArrowWriter;
use parquet::basic::{Repetition, Type as Fisico};
use parquet::errors::ParquetError;
use parquet::file::metadata::{
    PageIndexPolicy, ParquetMetaData, ParquetMetaDataOptions, ParquetMetaDataReader,
};
use parquet::file::properties::WriterProperties;
use parquet::file::writer::SerializedFileWriter;
use parquet::schema::types::Type;
use plenora_core::arrow::array::{ArrayRef, Int32Array, RecordBatch};
use plenora_core::arrow::schema::{DataType, Field, Schema};
use plenora_core::{ErrorCategory, ErrorPhase};
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
    tabella_dal_confine(byte, limiti).map(|tabella| tabella.num_rows())
}

fn tabella_dal_confine(byte: &[u8], limiti: &LimitiLettura) -> plenora_core::Result<RecordBatch> {
    let dir = cartella();
    let percorso = dir.path().join("f.parquet");
    std::fs::write(&percorso, byte).unwrap();
    leggi_tabella_con_limiti(&percorso, Some(Formato::Parquet), u64::MAX, limiti)
}

fn diretto(byte: &[u8], opzioni: ArrowReaderOptions) -> Result<Arc<Schema>, ParquetError> {
    let dir = cartella();
    let percorso = dir.path().join("d.parquet");
    std::fs::write(&percorso, byte).unwrap();
    let file = std::fs::File::open(&percorso).unwrap();
    ParquetRecordBatchReaderBuilder::try_new_with_options(file, opzioni)
        .map(|lettore| lettore.schema().clone())
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
    rifiuto_generale(
        diretto(&byte, ArrowReaderOptions::new()),
        &format!("Parquet schema deeper than {MAX_PROFONDITA_SCHEMA} levels"),
    );
    assert_rifiuto!(
        dal_confine(&byte, &LimitiLettura::default()),
        ErrorCategory::ResourceLimit,
        ErrorPhase::Write,
    );
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

const BUDGET_FOOTER: &str = "Parquet footer metadata exceeds the decoding memory budget";
const COLONNE_ASSENTI: &str = "Required field columns is missing";
const BUDGET_INDICI: &str = "page index requested with a footer memory budget: the page index decoders are not bounded by it";

/// Le API native Parquet non espongono una fase: questo aiuto si usa al
/// confine di decodifica/lettura, preservando il discriminante General.
#[track_caller]
fn rifiuto_generale<T>(esito: Result<T, ParquetError>, atteso: &str) {
    let Err(ParquetError::General(messaggio)) = esito else {
        panic!("atteso rifiuto General del decoder Parquet");
    };
    assert_eq!(messaggio, atteso);
}

#[track_caller]
fn rifiuto_non_qualificato<T>(esito: Result<T, ParquetError>, atteso: &str) {
    let Err(ParquetError::NYI(messaggio)) = esito else {
        panic!("atteso rifiuto NYI della funzionalita non qualificata");
    };
    assert_eq!(messaggio, atteso);
}

fn decodifica(byte: &[u8], tetto: u64) -> Result<ParquetMetaData, ParquetError> {
    let opzioni = ParquetMetaDataOptions::new().with_footer_memory_budget(tetto);
    ParquetMetaDataReader::decode_metadata_with_options(byte, Some(&opzioni))
}

/// La fixture attesa e' lo schema scritto sopra, non un'altra decodifica.
fn verifica_metadata(metadata: &ParquetMetaData, colonne: usize, righe: i64, gruppi: usize) {
    assert_eq!(metadata.num_row_groups(), gruppi);
    assert_eq!(metadata.file_metadata().num_rows(), righe);
    let radice = if gruppi == 0 {
        "schema"
    } else {
        "arrow_schema"
    };
    let campi = (0..colonne)
        .map(|i| {
            Arc::new(
                Type::primitive_type_builder(&format!("c{i}"), Fisico::INT32)
                    .with_repetition(Repetition::REQUIRED)
                    .build()
                    .unwrap(),
            )
        })
        .collect();
    let schema = Type::group_type_builder(radice)
        .with_fields(campi)
        .build()
        .unwrap();
    assert_eq!(metadata.file_metadata().schema(), &schema);
    for gruppo in metadata.row_groups() {
        assert_eq!(gruppo.num_columns(), colonne);
        assert_eq!(gruppo.num_rows(), righe);
        for colonna in gruppo.columns() {
            assert_eq!(colonna.num_values(), righe);
        }
    }
}

/// Maggiorante indipendente del costo dello schema piatto della fixture.
/// Il formato Thrift ha dieci campi: tre enum opzionali, cinque i32
/// opzionali, un nome preso in prestito e il tipo logico opzionale. Per
/// includere ogni padding si arrotonda ciascun campo all'allineamento massimo.
/// Non si interroga il decoder per trovare il budget che accetta.
fn tetto_schema_fixture(byte: &[u8], colonne: usize, radice: &str) -> u64 {
    use parquet::basic::{ConvertedType, LogicalType};
    use parquet::schema::types::ColumnDescriptor;
    use std::mem::{align_of, size_of};
    let allineamento = [
        align_of::<Option<LogicalType>>(),
        align_of::<&str>(),
        align_of::<Option<i32>>(),
    ]
    .into_iter()
    .max()
    .unwrap();
    let campo = |n: usize| n.next_multiple_of(allineamento);
    let elemento = campo(size_of::<Option<Fisico>>())
        + campo(size_of::<Option<Repetition>>())
        + campo(size_of::<Option<ConvertedType>>())
        + 5 * campo(size_of::<Option<i32>>())
        + campo(size_of::<&str>())
        + campo(size_of::<Option<LogicalType>>());
    // Due copie del footer, un nodo Type e tre puntatori per elemento;
    // un descrittore, quattro puntatori e il percorso radice/foglia per colonna.
    let nodi = (colonne + 1) * (elemento + size_of::<Type>() + 3 * size_of::<usize>());
    let percorsi: usize = (0..colonne)
        .map(|i| {
            size_of::<ColumnDescriptor>()
                + 4 * size_of::<usize>()
                + 2 * size_of::<String>()
                + radice.len()
                + format!("c{i}").len()
        })
        .sum();
    u64::try_from(2 * byte.len() + nodi + percorsi).unwrap()
}

/// Lo schema fornito dal test isola la prenotazione del row group:
/// due copie dei byte del footer, un `RowGroupMetaData` e 512 chunk.
/// Un byte sotto il costo rifiuta il budget; al costo esatto la decodifica
/// deve raggiungere il campo columns assente, senza accettare altri errori.
#[test]
fn la_capacita_del_row_group_si_addebita_prima_del_costruttore() {
    use parquet::file::metadata::{ColumnChunkMetaData, RowGroupMetaData};
    use parquet::schema::types::SchemaDescriptor;
    use std::mem::size_of;
    let campi = (0..512)
        .map(|i| {
            Arc::new(
                Type::primitive_type_builder(&format!("c{i}"), Fisico::INT32)
                    .with_repetition(Repetition::REQUIRED)
                    .build()
                    .unwrap(),
            )
        })
        .collect();
    let schema = Arc::new(SchemaDescriptor::new(Arc::new(
        Type::group_type_builder("schema")
            .with_fields(campi)
            .build()
            .unwrap(),
    )));
    let decodifica_con_schema = |byte: &[u8], tetto| {
        let opzioni = ParquetMetaDataOptions::new()
            .with_schema(Arc::clone(&schema))
            .with_footer_memory_budget(tetto);
        ParquetMetaDataReader::decode_metadata_with_options(byte, Some(&opzioni))
    };
    let senza_gruppo = footer(512, 0);
    let base = u64::try_from(2 * senza_gruppo.len()).unwrap();
    verifica_metadata(
        &decodifica_con_schema(&senza_gruppo, base).expect("sole copie del footer"),
        512,
        0,
        0,
    );
    rifiuto_generale(
        decodifica_con_schema(&senza_gruppo, base - 1),
        BUDGET_FOOTER,
    );
    let con_gruppo = footer(512, 1);
    let costo = u64::try_from(
        2 * con_gruppo.len()
            + size_of::<RowGroupMetaData>()
            + 512 * size_of::<ColumnChunkMetaData>(),
    )
    .unwrap();
    rifiuto_generale(decodifica_con_schema(&con_gruppo, costo - 1), BUDGET_FOOTER);
    rifiuto_generale(decodifica_con_schema(&con_gruppo, costo), COLONNE_ASSENTI);
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

/// Un file largo si decodifica entro un tetto fissato dalla fixture:
/// schema, una capacita' di chunk e spazio per le statistiche e i metadati.
/// A zero il rifiuto e' esclusivamente quello del budget del footer.
#[test]
fn un_file_vero_largo_si_legge_entro_il_tetto() {
    use parquet::file::metadata::{ColumnChunkMetaData, RowGroupMetaData};
    use std::mem::size_of;
    let byte = scrivi_colonne(64, 3);
    let n = byte.len();
    let lunghezza = u32::from_le_bytes(byte[n - 8..n - 4].try_into().unwrap());
    let footer = &byte[n - 8 - usize::try_from(lunghezza).unwrap()..n - 8];
    // Il writer di questa fixture emette una lista di tre encoding per
    // colonna, statistiche INT32 e il solo metadato ARROW:schema. Quattro
    // volte i byte del footer ne maggiorano copie e liste accessorie.
    let tetto = tetto_schema_fixture(footer, 64, "arrow_schema")
        + u64::try_from(
            64 * size_of::<ColumnChunkMetaData>()
                + size_of::<RowGroupMetaData>()
                + 4 * footer.len(),
        )
        .unwrap();
    let metadata = decodifica(footer, tetto).expect("fixture entro il tetto indipendente");
    verifica_metadata(&metadata, 64, 3, 1);
    rifiuto_generale(decodifica(footer, 0), BUDGET_FOOTER);
}

/// Indici di pagina chiesti con un tetto: un errore esplicito, prima di
/// leggerli. Senza tetto non sono qualificati (`PAGE_INDEX_NOT_QUALIFIED`):
/// questo crate non li chiede (`PageIndexPolicy::Skip`, il default), e la
/// lettura resta possibile.
#[test]
fn gli_indici_di_pagina_con_un_tetto_sono_rifiutati() {
    let byte = scrivi_colonne(1, 100);
    for politica in [PageIndexPolicy::Optional, PageIndexPolicy::Required] {
        let senza_tetto = ArrowReaderOptions::new().with_page_index_policy(politica);
        rifiuto_non_qualificato(
            diretto(&byte, senza_tetto),
            parquet::basic::PAGE_INDEX_NOT_QUALIFIED,
        );
        let con_tetto = ArrowReaderOptions::new()
            .with_page_index_policy(politica)
            .with_footer_memory_budget(1 << 30);
        rifiuto_generale(diretto(&byte, con_tetto), BUDGET_INDICI);
    }
    let solo_offset = ArrowReaderOptions::new()
        .with_offset_index_policy(PageIndexPolicy::Required)
        .with_footer_memory_budget(1 << 30);
    rifiuto_generale(diretto(&byte, solo_offset), BUDGET_INDICI);
    let salta = ArrowReaderOptions::new()
        .with_page_index_policy(PageIndexPolicy::Skip)
        .with_footer_memory_budget(1 << 30);
    assert_successo!(
        diretto(&byte, salta),
        Arc::new(Schema::new(vec![Field::new("c0", DataType::Int32, false)]))
    );
    verifica_valori_diretti(&byte, "c0", 100);
    assert_successo!(
        tabella_dal_confine(&byte, &LimitiLettura::default()),
        batch_interi("c0", 100)
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
    rifiuto_generale(
        SerializedFileReader::new_with_options(apri(), opzioni),
        BUDGET_INDICI,
    );
    let senza_tetto = ReadOptionsBuilder::new().with_page_index().build();
    rifiuto_non_qualificato(
        SerializedFileReader::new_with_options(apri(), senza_tetto),
        parquet::basic::PAGE_INDEX_NOT_QUALIFIED,
    );
}

// --- Indici di pagina e bloom filter non qualificati --------------------------------

/// Un Parquet di una colonna `INT32` di 300 righe in tre pagine da 100, con
/// gli indici di pagina e un bloom filter (li scrive `ArrowWriter`).
fn tre_pagine() -> Vec<u8> {
    let schema = Arc::new(Schema::new(vec![Field::new("c", DataType::Int32, false)]));
    let valori = Int32Array::from((0..300).collect::<Vec<i32>>());
    let batch =
        RecordBatch::try_new(Arc::clone(&schema), vec![Arc::new(valori) as ArrayRef]).unwrap();
    let proprieta = WriterProperties::builder()
        .set_data_page_row_count_limit(100)
        .set_write_batch_size(100)
        .set_bloom_filter_enabled(true)
        .build();
    let mut byte = Vec::new();
    let mut scrittore = ArrowWriter::try_new(&mut byte, schema, Some(proprieta)).unwrap();
    scrittore.write(&batch).unwrap();
    scrittore.close().unwrap();
    byte
}

/// Gli indici di pagina (column index e offset index) e i bloom filter non
/// sono qualificati: i loro conteggi, istogrammi, cardinalità e posizioni
/// arrivavano ad aritmetica non controllata, e data non li legge. Ogni loro
/// lettura è un errore esplicito (`PAGE_INDEX_NOT_QUALIFIED`,
/// `BLOOM_FILTER_NOT_QUALIFIED`), anche di un indice valido: dal lettore
/// Arrow, dal lettore dei metadati, dai decoder pubblici, dal lettore di
/// pagine costruito sulle posizioni dell'indice e dal lettore dei bloom
/// filter. La lettura del file senza indici resta.
#[test]
fn indici_di_pagina_e_bloom_filter_non_sono_qualificati() {
    use parquet::file::page_index::index_reader::{decode_column_index, decode_offset_index};
    use parquet::file::page_index::offset_index::PageLocation;
    use parquet::file::properties::ReaderProperties;
    use parquet::file::reader::FileReader;
    use parquet::file::serialized_reader::{
        ReadOptionsBuilder, SerializedFileReader, SerializedPageReader,
    };

    let indice = parquet::basic::PAGE_INDEX_NOT_QUALIFIED;
    let byte = tre_pagine();
    let dir = cartella();
    let percorso = dir.path().join("p.parquet");
    std::fs::write(&percorso, &byte).unwrap();
    let apri = || std::fs::File::open(&percorso).unwrap();
    // Il lettore Arrow con l'indice, e i lettori dei metadati.
    for politica in [PageIndexPolicy::Optional, PageIndexPolicy::Required] {
        let opzioni = ArrowReaderOptions::new().with_page_index_policy(politica);
        rifiuto_non_qualificato(diretto(&byte, opzioni), indice);
    }
    rifiuto_non_qualificato(
        ParquetMetaDataReader::new()
            .with_page_index_policy(PageIndexPolicy::Required)
            .parse_and_finish(&apri()),
        indice,
    );
    // I decoder pubblici, anche su byte qualunque.
    rifiuto_non_qualificato(decode_offset_index(&[0x19, 0x1C, 0x00]), indice);
    rifiuto_non_qualificato(
        decode_column_index(&[], parquet::basic::Type::INT32),
        indice,
    );
    // Il lettore di pagine sulle posizioni dell'indice.
    let costruttore = ParquetRecordBatchReaderBuilder::try_new(apri()).unwrap();
    let colonna = costruttore.metadata().row_group(0).column(0).clone();
    let (inizio, lunghezza) = colonna.byte_range();
    let posizioni = vec![PageLocation {
        offset: i64::try_from(inizio).unwrap(),
        compressed_page_size: i32::try_from(lunghezza).unwrap(),
        first_row_index: 0,
    }];
    rifiuto_non_qualificato(
        SerializedPageReader::new_with_properties(
            Arc::new(apri()),
            &colonna,
            300,
            Some(posizioni),
            Arc::new(ReaderProperties::builder().build()),
        ),
        indice,
    );
    // Il bloom filter, chiesto al lettore seriale.
    let opzioni = ReadOptionsBuilder::new()
        .with_reader_properties(
            ReaderProperties::builder()
                .set_read_bloom_filter(true)
                .build(),
        )
        .build();
    let lettore = SerializedFileReader::new_with_options(apri(), opzioni)
        .expect("il footer della fixture con bloom filter e' valido");
    rifiuto_non_qualificato(
        lettore.get_row_group(0),
        parquet::basic::BLOOM_FILTER_NOT_QUALIFIED,
    );
    // Controfattuale: senza indici e bloom filter il file si legge.
    assert_successo!(
        diretto(&byte, ArrowReaderOptions::new()),
        Arc::new(Schema::new(vec![Field::new("c", DataType::Int32, false)]))
    );
    verifica_valori_diretti(&byte, "c", 300);
    assert_successo!(
        tabella_dal_confine(&byte, &LimitiLettura::default()),
        batch_interi("c", 300)
    );
}

/// Una selezione di righe senza indici: ogni salto, dentro una pagina o di
/// pagine intere, non è qualificato (`SKIP_NOT_QUALIFIED`); una selezione
/// di tutte le righe si legge.
#[test]
fn una_selezione_di_righe_non_e_qualificata() {
    use parquet::arrow::arrow_reader::{RowSelection, RowSelector};
    let byte = tre_pagine();
    let leggi = |selezione: Vec<RowSelector>| {
        let dir = cartella();
        let percorso = dir.path().join("s.parquet");
        std::fs::write(&percorso, &byte).unwrap();
        let lettore =
            ParquetRecordBatchReaderBuilder::try_new(std::fs::File::open(&percorso).unwrap())
                .unwrap()
                .with_row_selection(RowSelection::from(selezione))
                .build()
                .unwrap();
        lettore.collect::<Result<Vec<_>, _>>()
    };
    for selezione in [
        vec![RowSelector::skip(150), RowSelector::select(150)],
        vec![RowSelector::skip(100), RowSelector::select(200)],
    ] {
        let errore = leggi(selezione).expect_err("salto non qualificato");
        let plenora_core::arrow::ArrowError::ParquetError(messaggio) = errore else {
            panic!("atteso ArrowError::ParquetError durante la lettura");
        };
        assert_eq!(
            messaggio,
            format!("NYI: {}", parquet::basic::SKIP_NOT_QUALIFIED)
        );
    }
    let atteso = batch_interi("c", 300);
    assert_successo!(leggi(vec![RowSelector::select(300)]), vec![atteso]);
}

fn batch_interi(nome: &str, righe: i32) -> RecordBatch {
    let schema = Arc::new(Schema::new(vec![Field::new(nome, DataType::Int32, false)]));
    RecordBatch::try_new(
        schema,
        vec![Arc::new(Int32Array::from((0..righe).collect::<Vec<_>>()))],
    )
    .unwrap()
}

fn verifica_valori_diretti(byte: &[u8], nome: &str, righe: i32) {
    let dir = cartella();
    let percorso = dir.path().join("valori.parquet");
    std::fs::write(&percorso, byte).unwrap();
    let lettore = ParquetRecordBatchReaderBuilder::try_new(std::fs::File::open(&percorso).unwrap())
        .unwrap()
        .build()
        .unwrap();
    assert_successo!(
        lettore.collect::<Result<Vec<_>, _>>(),
        vec![batch_interi(nome, righe)]
    );
}
