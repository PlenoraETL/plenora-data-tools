//! Dimensioni dichiarate da un Parquet contro i metadati del column chunk
//! (`patches/parquet-eof.patch`, `vendor/parquet-60.0.0-eof/PROVENANCE.md`).
//!
//! Il budget di lettura si controlla sui metadati dei column chunk prima di
//! leggere ([`plenora_io::stima_decodificata`]); un header di pagina che
//! dichiara di più (dimensione non compressa, valori, voci del dizionario,
//! conteggi delta, figli dello schema) farebbe riservare a `parquet` memoria
//! fuori da quel budget. Ogni prova prende un file valido scritto da
//! `parquet-rs`, sostituisce un solo intero dichiarato con uno più grande
//! codificato nello stesso numero di byte (il resto del file resta allineato)
//! e si aspetta un errore. Le codifiche valide si rileggono uguali.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod comune;

use std::fs::File;
use std::sync::Arc;

use parquet::arrow::arrow_reader::{ArrowReaderOptions, ParquetRecordBatchReaderBuilder};
use parquet::arrow::ArrowWriter;
use parquet::basic::{Compression, Encoding};
use parquet::file::metadata::PageIndexPolicy;
use parquet::file::properties::{WriterProperties, WriterVersion};
use parquet::file::reader::{FileReader, SerializedFileReader};
use parquet::schema::types::ColumnPath;
use plenora_core::arrow::array::{
    ArrayRef, FixedSizeBinaryArray, Float64Array, Int64Array, RecordBatch, StringArray,
};
use plenora_core::arrow::schema::{DataType, Field, Schema};
use plenora_core::ErrorCategory;
use plenora_io::{leggi_tabella, Formato};

use comune::{cartella, identiche};

/// Una colonna `x` non nullabile.
fn colonna(valori: ArrayRef) -> RecordBatch {
    let campo = Field::new("x", valori.data_type().clone(), false);
    RecordBatch::try_new(Arc::new(Schema::new(vec![campo])), vec![valori]).unwrap()
}

fn interi(righe: i64, distinti: i64) -> RecordBatch {
    colonna(Arc::new(Int64Array::from_iter_values(
        (0..righe).map(|i| (i % distinti) * 7),
    )))
}

fn testi(righe: usize) -> RecordBatch {
    colonna(Arc::new(StringArray::from_iter_values(
        (0..righe).map(|i| format!("valore-{:04}", i % 13)),
    )))
}

/// Byte del Parquet di `tabella` scritto con `proprieta`.
fn scrivi(tabella: &RecordBatch, proprieta: WriterProperties) -> Vec<u8> {
    let dir = cartella();
    let percorso = dir.path().join("p.parquet");
    let mut scrittore = ArrowWriter::try_new(
        File::create(&percorso).unwrap(),
        tabella.schema(),
        Some(proprieta),
    )
    .unwrap();
    scrittore.write(tabella).unwrap();
    scrittore.close().unwrap();
    std::fs::read(&percorso).unwrap()
}

fn proprieta(compressione: Compression, dizionario: bool) -> WriterProperties {
    WriterProperties::builder()
        .set_compression(compressione)
        .set_dictionary_enabled(dizionario)
        .set_writer_version(WriterVersion::PARQUET_1_0)
        .build()
}

fn con_codifica(codifica: Encoding) -> WriterProperties {
    WriterProperties::builder()
        .set_compression(Compression::UNCOMPRESSED)
        .set_dictionary_enabled(false)
        .set_writer_version(WriterVersion::PARQUET_1_0)
        .set_column_encoding(ColumnPath::from("x"), codifica)
        .build()
}

fn leggi(byte: &[u8]) -> plenora_core::Result<RecordBatch> {
    let dir = cartella();
    let percorso = dir.path().join("f.parquet");
    std::fs::write(&percorso, byte).unwrap();
    leggi_tabella(&percorso, Some(Formato::Parquet), u64::MAX)
}

/// Rifiutato da dt2 (`DataMapping`, subito) e, letto con `parquet`
/// direttamente, dalla verifica della patch: il messaggio di `parquet`
/// contiene `motivo` (il messaggio di dt2 non porta quello di `parquet`).
fn rifiutato(byte: &[u8], motivo: &str) {
    let inizio = std::time::Instant::now();
    let errore = leggi(byte).expect_err("il file doveva essere rifiutato");
    assert_eq!(errore.category(), ErrorCategory::DataMapping, "{errore}");
    assert!(inizio.elapsed() < std::time::Duration::from_secs(5));
    let messaggio = messaggio_parquet(byte, false);
    assert!(messaggio.contains(motivo), "{messaggio}");
}

/// Come [`rifiutato`], e anche letto con il page index: `parquet` legge
/// allora le pagine dalle posizioni dell'offset index (l'altro stato del
/// lettore di pagine), e la verifica deve valere lì.
fn rifiutato_nei_due_stati(byte: &[u8], motivo: &str) {
    rifiutato(byte, motivo);
    let messaggio = messaggio_parquet(byte, true);
    assert!(messaggio.contains(motivo), "{messaggio}");
}

/// L'errore di `parquet` sullo stesso file, letto fino in fondo; con
/// `indice` l'offset index è obbligatorio e deve essere caricato.
fn messaggio_parquet(byte: &[u8], indice: bool) -> String {
    let dir = cartella();
    let percorso = dir.path().join("q.parquet");
    std::fs::write(&percorso, byte).unwrap();
    let opzioni = if indice {
        ArrowReaderOptions::new().with_page_index_policy(PageIndexPolicy::Required)
    } else {
        ArrowReaderOptions::new()
    };
    let costruttore = match ParquetRecordBatchReaderBuilder::try_new_with_options(
        File::open(&percorso).unwrap(),
        opzioni,
    ) {
        Ok(costruttore) => costruttore,
        Err(errore) => return errore.to_string(),
    };
    if indice {
        assert!(
            costruttore
                .metadata()
                .page_index_for_row_group(0)
                .offset_index(0)
                .is_some(),
            "offset index non caricato"
        );
    }
    for batch in costruttore.build().unwrap() {
        if let Err(errore) = batch {
            return errore.to_string();
        }
    }
    panic!("parquet ha letto il file senza errori");
}

/// Metadati del primo column chunk: inizio della prima pagina dati, del
/// dizionario, dimensione non compressa e valori.
struct Chunk {
    pagina_dati: usize,
    compressi: i64,
    dizionario: Option<usize>,
    non_compressi: i64,
    valori: i64,
}

fn chunk(byte: &[u8]) -> Chunk {
    let dir = cartella();
    let percorso = dir.path().join("m.parquet");
    std::fs::write(&percorso, byte).unwrap();
    let lettore = SerializedFileReader::new(File::open(&percorso).unwrap()).unwrap();
    let colonna = lettore.metadata().row_group(0).column(0);
    Chunk {
        pagina_dati: usize::try_from(colonna.data_page_offset()).unwrap(),
        compressi: colonna.compressed_size(),
        dizionario: colonna
            .dictionary_page_offset()
            .map(|o| usize::try_from(o).unwrap()),
        non_compressi: colonna.uncompressed_size(),
        valori: colonna.num_values(),
    }
}

/// Un varint (LEB128) a `posizione`: `(posizione, lunghezza, valore)`.
fn varint(byte: &[u8], posizione: usize) -> (usize, usize, u64) {
    let mut valore = 0_u64;
    for (i, b) in byte[posizione..].iter().enumerate() {
        valore |= u64::from(b & 0x7f) << (7 * i);
        if b & 0x80 == 0 {
            return (posizione, i + 1, valore);
        }
    }
    panic!("varint senza fine");
}

/// Il valore massimo di un i32 zigzag in `lunghezza` byte.
const fn massimo_zigzag(lunghezza: usize) -> i64 {
    (1_i64 << (7 * lunghezza - 1)) - 1
}

/// Scrive `valore` (zigzag) nei `lunghezza` byte del campo, con byte di
/// continuazione ridondanti se servono: un varint LEB128 resta valido.
fn riscrivi_zigzag(byte: &mut [u8], (posizione, lunghezza, _): (usize, usize, u64), valore: i64) {
    let mut resto = u64::try_from(valore << 1).unwrap();
    for i in 0..lunghezza {
        let mut b = (resto & 0x7f) as u8;
        resto >>= 7;
        if i + 1 < lunghezza {
            b |= 0x80;
        }
        byte[posizione + i] = b;
    }
    assert_eq!(resto, 0, "il valore non sta nel campo");
}

/// Campi di un header di pagina (Thrift compact) a `inizio`: dimensione non
/// compressa e primo campo dell'header specifico (i valori della pagina).
struct Header {
    non_compressi: (usize, usize, u64),
    valori: (usize, usize, u64),
}

fn header(byte: &[u8], inizio: usize) -> Header {
    let mut p = inizio;
    let i32_successivo = |p: &mut usize| {
        assert_eq!(byte[*p], 0x15, "atteso un campo i32 consecutivo");
        let campo = varint(byte, *p + 1);
        *p = campo.0 + campo.1;
        campo
    };
    let _tipo = i32_successivo(&mut p);
    let non_compressi = i32_successivo(&mut p);
    let _compressi = i32_successivo(&mut p);
    assert_eq!(byte[p] & 0x0f, 0x0c, "atteso l'header specifico (struct)");
    p += 1;
    let valori = i32_successivo(&mut p);
    Header {
        non_compressi,
        valori,
    }
}

fn trova(byte: &[u8], da: usize, cerca: &[u8]) -> usize {
    da + byte[da..]
        .windows(cerca.len())
        .position(|finestra| finestra == cerca)
        .expect("sequenza non trovata")
}

#[test]
fn una_pagina_piu_grande_del_suo_chunk_e_rifiutata() {
    for compressione in [Compression::UNCOMPRESSED, Compression::SNAPPY] {
        let originale = scrivi(&interi(2000, 2000), proprieta(compressione, false));
        assert!(leggi(&originale).is_ok());
        let chunk = chunk(&originale);
        let campi = header(&originale, chunk.pagina_dati);
        let nuovo = massimo_zigzag(campi.non_compressi.1);
        assert!(nuovo > chunk.non_compressi);
        let mut byte = originale.clone();
        riscrivi_zigzag(&mut byte, campi.non_compressi, nuovo);
        rifiutato_nei_due_stati(&byte, "exceeds the column chunk uncompressed size");
    }
}

#[test]
fn una_pagina_con_piu_valori_del_chunk_e_rifiutata() {
    let originale = scrivi(
        &interi(2000, 2000),
        proprieta(Compression::UNCOMPRESSED, false),
    );
    let chunk = chunk(&originale);
    let campi = header(&originale, chunk.pagina_dati);
    let nuovo = massimo_zigzag(campi.valori.1);
    assert!(nuovo > chunk.valori);
    let mut byte = originale;
    riscrivi_zigzag(&mut byte, campi.valori, nuovo);
    rifiutato_nei_due_stati(&byte, "exceeds the column chunk value count");
}

#[test]
fn un_dizionario_con_piu_voci_dei_suoi_byte_e_rifiutato() {
    let originale = scrivi(
        &interi(2000, 100),
        proprieta(Compression::UNCOMPRESSED, true),
    );
    assert!(leggi(&originale).is_ok());
    let chunk = chunk(&originale);
    let inizio = chunk.dizionario.expect("pagina del dizionario");
    let campi = header(&originale, inizio);
    // 100 voci da 8 byte: al più 8 voci per byte sono 6400.
    let nuovo = massimo_zigzag(campi.valori.1);
    assert!(nuovo > 6400);
    let mut byte = originale;
    riscrivi_zigzag(&mut byte, campi.valori, nuovo);
    rifiutato(&byte, "more values than its bytes can encode");
}

/// Il conteggio dell'header delta (`block_size` 128 o 256, 4 miniblocchi, 10
/// valori) portato a 100 in una pagina da 10 valori: tre lettori diversi
/// (interi, lunghezze delta, prefissi delta).
#[test]
fn un_header_delta_con_piu_valori_della_pagina_e_rifiutato() {
    let casi = [
        (interi(10, 10), Encoding::DELTA_BINARY_PACKED),
        (testi(10), Encoding::DELTA_LENGTH_BYTE_ARRAY),
        (testi(10), Encoding::DELTA_BYTE_ARRAY),
    ];
    for (tabella, codifica) in casi {
        let originale = scrivi(&tabella, con_codifica(codifica));
        identiche(&tabella, &leggi(&originale).unwrap());
        let chunk = chunk(&originale);
        // `block_size` 128 o 256 (due byte), poi 4 miniblocchi e 10 valori.
        let posizione = (chunk.pagina_dati..originale.len() - 3)
            .find(|&i| {
                originale[i] == 0x80
                    && matches!(originale[i + 1], 0x01 | 0x02)
                    && originale[i + 2..i + 4] == [0x04, 0x0a]
            })
            .expect("header delta");
        let mut byte = originale;
        byte[posizione + 3] = 100;
        rifiutato(&byte, "more values than the page holds");
    }
}

#[test]
fn uno_schema_con_piu_figli_degli_elementi_e_rifiutato() {
    let originale = scrivi(&interi(10, 10), proprieta(Compression::UNCOMPRESSED, false));
    // La radice dello schema di `parquet-rs` si chiama `arrow_schema`: dopo
    // il nome viene `num_children`.
    let nome = trova(&originale, 4, b"\x0carrow_schema\x15");
    let campo = varint(&originale, nome + 14);
    let mut byte = originale;
    riscrivi_zigzag(&mut byte, campo, massimo_zigzag(campo.1));
    rifiutato(&byte, "more children than elements remain");
}

/// Le codifiche e le compressioni valide si rileggono uguali: le verifiche
/// non rifiutano file ben formati (pagine v1 e v2).
#[test]
fn le_codifiche_valide_si_rileggono_uguali() {
    let binari: Vec<[u8; 4]> = (0_u32..300).map(|i| (i % 17).to_le_bytes()).collect();
    let tabelle = [
        (interi(3000, 50), None),
        (interi(3000, 3000), Some(Encoding::DELTA_BINARY_PACKED)),
        (
            colonna(Arc::new(Float64Array::from_iter_values(
                (0..3000).map(|i| f64::from(i) / 3.0),
            ))),
            Some(Encoding::BYTE_STREAM_SPLIT),
        ),
        (testi(3000), Some(Encoding::DELTA_LENGTH_BYTE_ARRAY)),
        (testi(3000), Some(Encoding::DELTA_BYTE_ARRAY)),
        (testi(3000), None),
        (
            colonna(Arc::new(
                FixedSizeBinaryArray::try_from_iter(binari.iter()).unwrap(),
            )),
            None,
        ),
        (
            colonna(Arc::new(
                FixedSizeBinaryArray::try_from_iter(binari.iter()).unwrap(),
            )),
            Some(Encoding::DELTA_BYTE_ARRAY),
        ),
    ];
    for (tabella, codifica) in &tabelle {
        assert!(matches!(
            tabella.schema().field(0).data_type(),
            DataType::Int64 | DataType::Float64 | DataType::Utf8 | DataType::FixedSizeBinary(4)
        ));
        for compressione in [
            Compression::UNCOMPRESSED,
            Compression::SNAPPY,
            Compression::ZSTD(parquet::basic::ZstdLevel::default()),
        ] {
            for versione in [WriterVersion::PARQUET_1_0, WriterVersion::PARQUET_2_0] {
                let mut costruttore = WriterProperties::builder()
                    .set_compression(compressione)
                    .set_writer_version(versione)
                    .set_data_page_row_count_limit(700);
                costruttore = match codifica {
                    Some(codifica) => costruttore
                        .set_dictionary_enabled(false)
                        .set_column_encoding(ColumnPath::from("x"), *codifica),
                    None => costruttore.set_dictionary_enabled(true),
                };
                let byte = scrivi(tabella, costruttore.build());
                let letta = leggi(&byte).unwrap_or_else(|errore| {
                    panic!("{codifica:?} {compressione:?} {versione:?}: {errore}")
                });
                identiche(tabella, &letta);
            }
        }
    }
}

/// Un intervallo oltre la fine del file è un EOF prima di riservare la sua
/// lunghezza: la lunghezza viene da un header di pagina o dal footer.
#[test]
fn byte_oltre_la_fine_del_file_sono_un_errore_senza_riserva() {
    use parquet::file::reader::ChunkReader;
    let dir = cartella();
    let percorso = dir.path().join("corto");
    std::fs::write(&percorso, [0_u8; 16]).unwrap();
    let file = File::open(&percorso).unwrap();
    assert_eq!(file.get_bytes(4, 12).unwrap().len(), 12);
    for (inizio, lunghezza) in [(4, 13), (0, usize::MAX / 2), (u64::MAX, 1)] {
        let errore = file
            .get_bytes(inizio, lunghezza)
            .expect_err("oltre la fine");
        assert!(
            errore.to_string().contains("past the end of the file"),
            "{errore}"
        );
    }
}

/// Righe lette da `parquet` con l'offset index caricato (le pagine dalle
/// sue posizioni).
fn righe_con_indice(byte: &[u8]) -> usize {
    let dir = cartella();
    let percorso = dir.path().join("i.parquet");
    std::fs::write(&percorso, byte).unwrap();
    let opzioni = ArrowReaderOptions::new().with_page_index_policy(PageIndexPolicy::Required);
    let costruttore = ParquetRecordBatchReaderBuilder::try_new_with_options(
        File::open(&percorso).unwrap(),
        opzioni,
    )
    .unwrap();
    assert!(costruttore
        .metadata()
        .page_index_for_row_group(0)
        .offset_index(0)
        .is_some());
    costruttore
        .build()
        .unwrap()
        .map(|batch| batch.unwrap().num_rows())
        .sum()
}

/// Scrittore minimo del protocollo Thrift compact, per un header di pagina
/// costruito a mano.
#[derive(Default)]
struct Compact(Vec<u8>, Vec<i16>);

impl Compact {
    fn campo(&mut self, id: i16, tipo: u8) {
        let ultimo = self.1.last().copied().unwrap_or(0);
        let delta = id - ultimo;
        assert!((1..=15).contains(&delta));
        self.0.push((u8::try_from(delta).unwrap() << 4) | tipo);
        *self.1.last_mut().unwrap() = id;
    }
    fn varint(&mut self, mut valore: u64) {
        loop {
            let b = (valore & 0x7f) as u8;
            valore >>= 7;
            if valore == 0 {
                self.0.push(b);
                return;
            }
            self.0.push(b | 0x80);
        }
    }
    fn i32(&mut self, id: i16, valore: i32) {
        self.campo(id, 5);
        self.varint(u64::from(((valore << 1) ^ (valore >> 31)).cast_unsigned()));
    }
    fn bool(&mut self, id: i16, valore: bool) {
        self.campo(id, if valore { 1 } else { 2 });
    }
    fn inizio(&mut self, id: i16) {
        self.campo(id, 12);
        self.1.push(0);
    }
    fn fine(&mut self) {
        self.0.push(0);
        self.1.pop();
    }
    fn binario(&mut self, id: i16, lunghezza: usize) {
        self.campo(id, 8);
        self.varint(lunghezza as u64);
        self.0.extend(std::iter::repeat_n(b'x', lunghezza));
    }
}

/// Una pagina con l'header v1 e l'header v2 insieme, di tipo v2: l'header
/// v1 dichiara i valori del chunk, quello v2 (che la decodifica usa) un
/// miliardo. Il controllo deve guardare l'header che il tipo seleziona.
/// L'header costruito prende il posto di quello originale, alla stessa
/// lunghezza grazie a un campo sconosciuto di riempimento (Thrift lo salta).
#[test]
fn il_controllo_guarda_l_header_del_tipo_di_pagina() {
    // Valori lunghi con le statistiche nell'header: l'header originale ha
    // lo spazio per i due header costruiti.
    let lunghi = colonna(Arc::new(StringArray::from_iter_values(
        (0..10).map(|i| format!("{i:0>60}")),
    )));
    let proprieta = WriterProperties::builder()
        .set_compression(Compression::UNCOMPRESSED)
        .set_dictionary_enabled(false)
        .set_writer_version(WriterVersion::PARQUET_1_0)
        .set_write_page_header_statistics(true)
        .set_column_encoding(ColumnPath::from("x"), Encoding::DELTA_LENGTH_BYTE_ARRAY)
        .build();
    let originale = scrivi(&lunghi, proprieta);
    let chunk = chunk(&originale);
    let campi = header(&originale, chunk.pagina_dati);
    let compressi = varint(
        &originale,
        campi.non_compressi.0 + campi.non_compressi.1 + 1,
    )
    .2;
    let compressi = i32::try_from(compressi >> 1).unwrap();
    let lunghezza_header =
        usize::try_from(chunk.compressi).unwrap() - usize::try_from(compressi).unwrap();
    let costruisci = |riempimento: Option<usize>| {
        let mut c = Compact::default();
        c.1.push(0);
        c.i32(1, 3); // DATA_PAGE_V2
        c.i32(2, compressi);
        c.i32(3, compressi);
        c.inizio(5);
        c.i32(1, 10);
        c.i32(2, 6); // DELTA_LENGTH_BYTE_ARRAY
        c.i32(3, 3);
        c.i32(4, 3);
        c.fine();
        c.inizio(8);
        c.i32(1, 1_000_000_000);
        c.i32(2, 0);
        c.i32(3, 10);
        c.i32(4, 6);
        c.i32(5, 0);
        c.i32(6, 0);
        c.bool(7, false);
        c.fine();
        if let Some(n) = riempimento {
            c.binario(9, n);
        }
        c.0.push(0);
        c.0
    };
    let senza = costruisci(None).len();
    // Campo di riempimento: un byte di intestazione, un varint, i dati.
    let n = (0..lunghezza_header)
        .find(|&n| senza + 1 + (n.max(1).ilog2() as usize / 7 + 1) + n == lunghezza_header)
        .expect("l'header originale e' troppo corto per il riempimento");
    let nuovo = costruisci(Some(n));
    assert_eq!(nuovo.len(), lunghezza_header);
    let mut byte = originale;
    byte[chunk.pagina_dati..chunk.pagina_dati + lunghezza_header].copy_from_slice(&nuovo);
    rifiutato(&byte, "exceeds the column chunk value count");
}

/// Una pagina v1 compressa con un header v2 aggiunto che dichiara
/// `is_compressed = false`: l'header v2 non conta su una pagina v1, la pagina
/// si decomprime e si rilegge uguale. Scelto per presenza, l'header v2
/// spegneva la decompressione (e la verifica della dimensione) e i byte
/// compressi arrivavano al decodificatore come valori.
#[test]
fn un_header_v2_non_conta_su_una_pagina_v1() {
    let lunghi = colonna(Arc::new(StringArray::from_iter_values(
        (0..10).map(|i| format!("{i:0>60}")),
    )));
    let proprieta = WriterProperties::builder()
        .set_compression(Compression::SNAPPY)
        .set_dictionary_enabled(false)
        .set_writer_version(WriterVersion::PARQUET_1_0)
        .set_write_page_header_statistics(true)
        .build();
    let originale = scrivi(&lunghi, proprieta);
    identiche(&lunghi, &leggi(&originale).unwrap());
    let chunk = chunk(&originale);
    let campi = header(&originale, chunk.pagina_dati);
    let non_compressi = i32::try_from(campi.non_compressi.2 >> 1).unwrap();
    let compressi = varint(
        &originale,
        campi.non_compressi.0 + campi.non_compressi.1 + 1,
    )
    .2;
    let compressi = i32::try_from(compressi >> 1).unwrap();
    let lunghezza_header =
        usize::try_from(chunk.compressi).unwrap() - usize::try_from(compressi).unwrap();
    let costruisci = |riempimento: Option<usize>| {
        let mut c = Compact::default();
        c.1.push(0);
        c.i32(1, 0); // DATA_PAGE
        c.i32(2, non_compressi);
        c.i32(3, compressi);
        c.inizio(5);
        c.i32(1, 10);
        c.i32(2, 0); // PLAIN
        c.i32(3, 3);
        c.i32(4, 3);
        c.fine();
        c.inizio(8);
        c.i32(1, 10);
        c.i32(2, 0);
        c.i32(3, 10);
        c.i32(4, 0);
        c.i32(5, 0);
        c.i32(6, 0);
        c.bool(7, false);
        c.fine();
        if let Some(n) = riempimento {
            c.binario(9, n);
        }
        c.0.push(0);
        c.0
    };
    let senza = costruisci(None).len();
    let n = (0..lunghezza_header)
        .find(|&n| senza + 1 + (n.max(1).ilog2() as usize / 7 + 1) + n == lunghezza_header)
        .expect("l'header originale e' troppo corto per il riempimento");
    let nuovo = costruisci(Some(n));
    assert_eq!(nuovo.len(), lunghezza_header);
    let mut byte = originale;
    byte[chunk.pagina_dati..chunk.pagina_dati + lunghezza_header].copy_from_slice(&nuovo);
    identiche(&lunghi, &leggi(&byte).unwrap());
}

/// Nulli, colonne tutte nulle e liste (più valori per riga) si rileggono
/// uguali con pagine v1 e v2 e con le codifiche delta: sono i casi in cui
/// livelli, valori e righe di una pagina differiscono.
#[test]
fn nulli_e_ripetizioni_si_rileggono_uguali() {
    use plenora_core::arrow::array::builder::{Int64Builder, ListBuilder};
    let nullabile = |valori: ArrayRef| {
        let campo = Field::new("x", valori.data_type().clone(), true);
        RecordBatch::try_new(Arc::new(Schema::new(vec![campo])), vec![valori]).unwrap()
    };
    let mut liste = ListBuilder::new(Int64Builder::new());
    for riga in 0..500_i64 {
        if riga % 7 == 0 {
            liste.append_null();
            continue;
        }
        for valore in 0..(riga % 5) {
            if valore == 2 {
                liste.values().append_null();
            } else {
                liste.values().append_value(riga * 10 + valore);
            }
        }
        liste.append(true);
    }
    let casi: Vec<(RecordBatch, Option<Encoding>)> = vec![
        (
            nullabile(Arc::new(Int64Array::from_iter(
                (0..2000_i64).map(|i| (i % 3 != 0).then_some(i)),
            ))),
            Some(Encoding::DELTA_BINARY_PACKED),
        ),
        (
            nullabile(Arc::new(Int64Array::from_iter(
                (0..2000).map(|_| None::<i64>),
            ))),
            Some(Encoding::DELTA_BINARY_PACKED),
        ),
        (
            nullabile(Arc::new(StringArray::from_iter(
                (0..2000).map(|i| (i % 4 != 0).then(|| format!("v{}", i % 9))),
            ))),
            Some(Encoding::DELTA_LENGTH_BYTE_ARRAY),
        ),
        (
            nullabile(Arc::new(StringArray::from_iter(
                (0..2000).map(|i| (i % 4 != 0).then(|| format!("prefisso-{}", i % 9))),
            ))),
            Some(Encoding::DELTA_BYTE_ARRAY),
        ),
        (
            nullabile(Arc::new(liste.finish())),
            Some(Encoding::DELTA_BINARY_PACKED),
        ),
        (
            nullabile(Arc::new(StringArray::from_iter(
                (0..2000).map(|i| (i % 2 == 0).then_some("d")),
            ))),
            None,
        ),
    ];
    for (tabella, codifica) in &casi {
        for versione in [WriterVersion::PARQUET_1_0, WriterVersion::PARQUET_2_0] {
            let mut costruttore = WriterProperties::builder()
                .set_writer_version(versione)
                .set_data_page_row_count_limit(300);
            costruttore = match codifica {
                Some(codifica) => costruttore
                    .set_dictionary_enabled(false)
                    .set_encoding(*codifica),
                None => costruttore.set_dictionary_enabled(true),
            };
            let byte = scrivi(tabella, costruttore.build());
            let letta =
                leggi(&byte).unwrap_or_else(|errore| panic!("{codifica:?} {versione:?}: {errore}"));
            identiche(tabella, &letta);
            // Anche dalle posizioni dell'offset index.
            assert_eq!(
                righe_con_indice(&byte),
                tabella.num_rows(),
                "{codifica:?} {versione:?}"
            );
        }
    }
}
