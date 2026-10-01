//! Confine di lettura (README, «Confine di lettura»): file troncati,
//! corrotti, con lunghezze dichiarate enormi o metadati oltre i limiti
//! diventano errori espliciti; i limiti si applicano. Non si provano file
//! Parquet costruiti per far allocare `parquet` oltre misura: quello è un
//! limite dichiarato (aborto del processo).

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod comune;

use std::collections::HashMap;
use std::fs::File;
use std::sync::Arc;

use plenora_core::arrow::array::builder::StringDictionaryBuilder;
use plenora_core::arrow::array::types::Int32Type;
use plenora_core::arrow::array::{ArrayRef, Int64Array, RecordBatch, StringArray};
use plenora_core::arrow::ipc as fb;
use plenora_core::arrow::ipc::writer::{FileWriter, StreamWriter};
use plenora_core::arrow::schema::{DataType, Field, Schema};
use plenora_core::{ErrorCategory, PlenoraError};
use plenora_io::{
    leggi_tabella, leggi_tabella_con_limiti, scrivi_tabella, Formato, LimitiLettura,
    OpzioniScrittura,
};

use comune::{cartella, identiche};

/// Tre colonne, una a dizionario, metadati di schema.
fn piccola(righe: i64) -> RecordBatch {
    let mut dizionario = StringDictionaryBuilder::<Int32Type>::new();
    for riga in 0..righe {
        dizionario.append_value(["rosso", "verde", "blu"][usize::try_from(riga % 3).unwrap()]);
    }
    let schema = Schema::new_with_metadata(
        vec![
            Field::new("id", DataType::Int64, false),
            Field::new(
                "colore",
                DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Utf8)),
                false,
            ),
            Field::new("testo", DataType::Utf8, false),
        ],
        HashMap::from([("origine".to_owned(), "prova".to_owned())]),
    );
    let colonne: Vec<ArrayRef> = vec![
        Arc::new(Int64Array::from((0..righe).collect::<Vec<_>>())),
        Arc::new(dizionario.finish()),
        Arc::new(StringArray::from(
            (0..righe)
                .map(|riga| format!("t{riga}"))
                .collect::<Vec<_>>(),
        )),
    ];
    RecordBatch::try_new(Arc::new(schema), colonne).unwrap()
}

/// Byte di un file IPC (file o stream) con `blocchi` blocchi da due righe.
fn ipc(stream: bool, blocchi: usize) -> Vec<u8> {
    let tabella = piccola(i64::try_from(blocchi * 2).unwrap());
    let mut byte = Vec::new();
    if stream {
        let mut scrittore = StreamWriter::try_new(&mut byte, &tabella.schema()).unwrap();
        for blocco in 0..blocchi {
            scrittore.write(&tabella.slice(blocco * 2, 2)).unwrap();
        }
        scrittore.finish().unwrap();
    } else {
        let mut scrittore = FileWriter::try_new(&mut byte, &tabella.schema()).unwrap();
        for blocco in 0..blocchi {
            scrittore.write(&tabella.slice(blocco * 2, 2)).unwrap();
        }
        scrittore.finish().unwrap();
    }
    byte
}

/// Byte di un Parquet della tabella piccola.
fn parquet(righe: i64) -> Vec<u8> {
    let dir = cartella();
    let percorso = dir.path().join("p.parquet");
    scrivi_tabella(&piccola(righe), &percorso, &OpzioniScrittura::default()).unwrap();
    std::fs::read(&percorso).unwrap()
}

fn leggi_byte(
    byte: &[u8],
    formato: Formato,
    limiti: &LimitiLettura,
) -> plenora_core::Result<RecordBatch> {
    let dir = cartella();
    let percorso = dir.path().join("f");
    std::fs::write(&percorso, byte).unwrap();
    leggi_tabella_con_limiti(&percorso, Some(formato), u64::MAX, limiti)
}

fn errore(byte: &[u8], formato: Formato, limiti: &LimitiLettura) -> PlenoraError {
    leggi_byte(byte, formato, limiti).expect_err("il file doveva essere rifiutato")
}

fn categoria(byte: &[u8], formato: Formato) -> ErrorCategory {
    errore(byte, formato, &LimitiLettura::default()).category()
}

/// Categoria e motivo dell'errore con i limiti predefiniti.
fn rifiutato(byte: &[u8], formato: Formato, attesa: ErrorCategory, motivo: &str) {
    let errore = errore(byte, formato, &LimitiLettura::default());
    assert_eq!(errore.category(), attesa, "{errore}");
    assert!(errore.to_string().contains(motivo), "{errore}");
}

#[test]
fn i_file_validi_passano_dal_confine() {
    let attesa = piccola(6);
    for stream in [false, true] {
        let letta = leggi_byte(
            &ipc(stream, 3),
            Formato::ArrowIpc,
            &LimitiLettura::default(),
        )
        .expect("IPC valido");
        identiche(&attesa, &letta);
    }
    let letta = leggi_byte(&parquet(6), Formato::Parquet, &LimitiLettura::default())
        .expect("Parquet valido");
    assert_eq!(letta.num_rows(), 6);
}

/// Le posizioni da provare in un file di `n` byte: tutte con
/// `PLENORA_TEST_LUNGHI=1` (README, «Suite lunga»); senza, testa e coda
/// intere (prefissi, schema, footer, code) e una ogni 11 in mezzo.
fn posizioni(n: usize) -> Vec<usize> {
    let lunghi = match std::env::var("PLENORA_TEST_LUNGHI") {
        Err(_) => false,
        Ok(valore) if valore == "0" => false,
        Ok(valore) if valore == "1" => true,
        Ok(_) => panic!("PLENORA_TEST_LUNGHI vale 1 (suite lunga) o 0"),
    };
    (0..n)
        .filter(|&i| lunghi || i < 96 || i + 160 >= n || i % 11 == 0)
        .collect()
}

#[test]
fn file_troncati_a_ogni_lunghezza_sono_errori() {
    let limiti = LimitiLettura::default();
    for (byte, formato) in [
        (ipc(false, 2), Formato::ArrowIpc),
        (ipc(true, 2), Formato::ArrowIpc),
        (parquet(4), Formato::Parquet),
    ] {
        let dir = cartella();
        let percorso = dir.path().join("t");
        for lunghezza in posizioni(byte.len()) {
            std::fs::write(&percorso, &byte[..lunghezza]).unwrap();
            let esito = leggi_tabella_con_limiti(&percorso, Some(formato), u64::MAX, &limiti);
            assert!(esito.is_err(), "{formato:?} troncato a {lunghezza} letto");
        }
    }
}

/// Arrow IPC, ogni byte invertito, uno alla volta ([`posizioni`]): la
/// lettura può riuscire (un valore cambiato non si vede senza checksum) o
/// fallire, mai andare in panico. Parquet non c'è: un byte invertito in una
/// lunghezza Thrift del footer può far abortire `parquet` (limite
/// dichiarato).
#[test]
fn byte_corrotti_non_mandano_in_panico() {
    let limiti = LimitiLettura::default();
    for (byte, formato) in [
        (ipc(false, 2), Formato::ArrowIpc),
        (ipc(true, 2), Formato::ArrowIpc),
    ] {
        let dir = cartella();
        let percorso = dir.path().join("c");
        for posizione in posizioni(byte.len()) {
            let mut corrotto = byte.clone();
            corrotto[posizione] ^= 0xFF;
            std::fs::write(&percorso, &corrotto).unwrap();
            let esito = std::panic::catch_unwind(|| {
                leggi_tabella_con_limiti(&percorso, Some(formato), u64::MAX, &limiti)
            });
            assert!(esito.is_ok(), "{formato:?}: panico al byte {posizione}");
        }
    }
}

#[test]
fn ipc_lunghezze_dei_metadati_enormi() {
    // Continuazione e 2^31 - 1 byte di metadati dichiarati.
    let mut byte = vec![0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x7F];
    byte.extend_from_slice(&[0; 16]);
    rifiutato(
        &byte,
        Formato::ArrowIpc,
        ErrorCategory::ResourceLimit,
        "metadati di un messaggio IPC",
    );
    // Sotto il tetto ma oltre il file.
    let alti = LimitiLettura {
        max_byte_metadati: u64::MAX,
        ..LimitiLettura::default()
    };
    assert_eq!(
        errore(&byte, Formato::ArrowIpc, &alti).category(),
        ErrorCategory::DataMapping
    );
    // Lunghezza negativa.
    let negativa = [0xFF, 0xFF, 0xFF, 0xFF, 0x00, 0x00, 0x00, 0x80];
    assert_eq!(
        categoria(&negativa, Formato::ArrowIpc),
        ErrorCategory::DataMapping
    );
}

#[test]
fn ipc_stream_senza_fine_o_con_byte_dopo_la_fine() {
    let mut byte = ipc(true, 2);
    byte.truncate(byte.len() - 8);
    assert_eq!(
        categoria(&byte, Formato::ArrowIpc),
        ErrorCategory::DataMapping
    );
    let mut dopo = ipc(true, 2);
    dopo.extend_from_slice(&[0; 8]);
    assert_eq!(
        categoria(&dopo, Formato::ArrowIpc),
        ErrorCategory::DataMapping
    );
}

/// Posizione del corpo del primo blocco nel footer di un file IPC.
fn corpo_primo_blocco(byte: &[u8]) -> usize {
    let fine = byte.len() - 10;
    let lunghezza =
        usize::try_from(i32::from_le_bytes(byte[fine..fine + 4].try_into().unwrap())).unwrap();
    let footer = fb::root_as_footer(&byte[fine - lunghezza..fine]).unwrap();
    let blocco = footer.recordBatches().unwrap().get(0);
    let mut cercato = Vec::new();
    cercato.extend_from_slice(&blocco.offset().to_le_bytes());
    cercato.extend_from_slice(&blocco.metaDataLength().to_le_bytes());
    let zona = &byte[fine - lunghezza..fine];
    let trovati: Vec<usize> = zona
        .windows(cercato.len())
        .enumerate()
        .filter(|(_, finestra)| *finestra == cercato.as_slice())
        .map(|(posizione, _)| posizione)
        .collect();
    assert_eq!(trovati.len(), 1, "blocco da correggere non univoco");
    // offset (8), metaDataLength (4), allineamento (4), bodyLength (8).
    fine - lunghezza + trovati[0] + 16
}

#[test]
fn ipc_footer_con_blocchi_o_lunghezze_enormi() {
    // `FileReader` farebbe `from_len_zeroed(bodyLength)` del blocco.
    let mut byte = ipc(false, 1);
    let corpo = corpo_primo_blocco(&byte);
    byte[corpo..corpo + 8].copy_from_slice(&(1_i64 << 60).to_le_bytes());
    rifiutato(
        &byte,
        Formato::ArrowIpc,
        ErrorCategory::DataMapping,
        "blocco del footer fuori",
    );
    // Lunghezza del footer: oltre il tetto, poi solo oltre il file.
    let mut footer = ipc(false, 1);
    let fine = footer.len() - 10;
    footer[fine..fine + 4].copy_from_slice(&i32::MAX.to_le_bytes());
    assert_eq!(
        categoria(&footer, Formato::ArrowIpc),
        ErrorCategory::ResourceLimit
    );
    let alti = LimitiLettura {
        max_byte_metadati: u64::MAX,
        ..LimitiLettura::default()
    };
    assert_eq!(
        errore(&footer, Formato::ArrowIpc, &alti).category(),
        ErrorCategory::DataMapping
    );
}

#[test]
fn limiti_di_blocchi_e_metadati() {
    for (byte, formato) in [
        (ipc(false, 3), Formato::ArrowIpc),
        (ipc(true, 3), Formato::ArrowIpc),
    ] {
        let tre = LimitiLettura {
            max_blocchi: 3,
            ..LimitiLettura::default()
        };
        assert!(leggi_byte(&byte, formato, &tre).is_ok());
        let due = LimitiLettura {
            max_blocchi: 2,
            ..LimitiLettura::default()
        };
        assert_eq!(
            errore(&byte, formato, &due).category(),
            ErrorCategory::ResourceLimit
        );
    }
    for (byte, formato) in [
        (ipc(false, 1), Formato::ArrowIpc),
        (ipc(true, 1), Formato::ArrowIpc),
        (parquet(2), Formato::Parquet),
    ] {
        // "origine" + "prova": 12 byte di metadati di schema.
        let custom = LimitiLettura {
            max_byte_metadati_custom: 11,
            ..LimitiLettura::default()
        };
        assert_eq!(
            errore(&byte, formato, &custom).category(),
            ErrorCategory::ResourceLimit,
            "{formato:?}"
        );
        let metadati = LimitiLettura {
            max_byte_metadati: 64,
            ..LimitiLettura::default()
        };
        assert_eq!(
            errore(&byte, formato, &metadati).category(),
            ErrorCategory::ResourceLimit,
            "{formato:?}"
        );
    }
}

#[test]
fn parquet_row_group_oltre_il_massimo() {
    // Tre row group da una riga.
    let dir = cartella();
    let percorso = dir.path().join("rg.parquet");
    let tabella = piccola(3);
    let proprieta = parquet::file::properties::WriterProperties::builder()
        .set_max_row_group_row_count(Some(1))
        .build();
    let mut scrittore = parquet::arrow::ArrowWriter::try_new(
        File::create(&percorso).unwrap(),
        tabella.schema(),
        Some(proprieta),
    )
    .unwrap();
    scrittore.write(&tabella).unwrap();
    scrittore.close().unwrap();
    let limiti = |massimo| LimitiLettura {
        max_blocchi: massimo,
        ..LimitiLettura::default()
    };
    assert!(leggi_tabella_con_limiti(&percorso, None, u64::MAX, &limiti(3)).is_ok());
    assert_eq!(
        leggi_tabella_con_limiti(&percorso, None, u64::MAX, &limiti(2))
            .expect_err("row group oltre il massimo")
            .category(),
        ErrorCategory::ResourceLimit
    );
}

#[test]
fn parquet_footer_enorme_o_corrotto() {
    let mut byte = parquet(2);
    let fine = byte.len() - 8;
    byte[fine..fine + 4].copy_from_slice(&u32::MAX.to_le_bytes());
    assert_eq!(
        categoria(&byte, Formato::Parquet),
        ErrorCategory::ResourceLimit
    );
    let alti = LimitiLettura {
        max_byte_metadati: u64::MAX,
        ..LimitiLettura::default()
    };
    assert_eq!(
        errore(&byte, Formato::Parquet, &alti).category(),
        ErrorCategory::DataMapping
    );
}

/// Un blocco del footer spostato di 8 byte (metadati - 8, corpo + 8) resta
/// dentro il file e disgiunto, ma `FileDecoder` leggerebbe il riempimento
/// dei metadati come valori.
#[test]
fn ipc_blocco_del_footer_incoerente_col_messaggio() {
    let mut byte = ipc(false, 1);
    let corpo = corpo_primo_blocco(&byte);
    let metadati = corpo - 8;
    let valore = i32::from_le_bytes(byte[metadati..metadati + 4].try_into().unwrap());
    byte[metadati..metadati + 4].copy_from_slice(&(valore - 8).to_le_bytes());
    let lunghezza = i64::from_le_bytes(byte[corpo..corpo + 8].try_into().unwrap());
    byte[corpo..corpo + 8].copy_from_slice(&(lunghezza + 8).to_le_bytes());
    rifiutato(
        &byte,
        Formato::ArrowIpc,
        ErrorCategory::DataMapping,
        "metadati del blocco diversi",
    );
}

/// Un blocco dello stream col tipo cambiato in `NONE`: `StreamDecoder` lo
/// salterebbe, con le sue righe.
#[test]
fn ipc_messaggio_none_nello_stream_rifiutato() {
    let originale = ipc(true, 2);
    // Il primo messaggio di blocco: dopo schema e dizionario.
    let mut posizione = 0;
    let mut trovato = None;
    while trovato.is_none() {
        let lunghezza = usize::try_from(i32::from_le_bytes(
            originale[posizione + 4..posizione + 8].try_into().unwrap(),
        ))
        .unwrap();
        let metadati = &originale[posizione + 8..posizione + 8 + lunghezza];
        let messaggio = fb::root_as_message(metadati).unwrap();
        if messaggio.header_type() == fb::MessageHeader::RecordBatch {
            trovato = Some((posizione + 8, lunghezza));
        }
        posizione += 8 + lunghezza + usize::try_from(messaggio.bodyLength()).unwrap();
    }
    let (inizio, lunghezza) = trovato.unwrap();
    // Il byte del discriminante: l'unico 3 che, messo a 0, rende `NONE` con
    // la stessa lunghezza del corpo.
    let mut candidati = Vec::new();
    for indice in inizio..inizio + lunghezza {
        if originale[indice] != 3 {
            continue;
        }
        let mut prova = originale.clone();
        prova[indice] = 0;
        let metadati = &prova[inizio..inizio + lunghezza];
        if let Ok(messaggio) = fb::root_as_message(metadati) {
            if messaggio.header_type() == fb::MessageHeader::NONE {
                candidati.push(prova);
            }
        }
    }
    assert_eq!(candidati.len(), 1, "discriminante non univoco");
    rifiutato(
        &candidati[0],
        Formato::ArrowIpc,
        ErrorCategory::DataMapping,
        "non ammesso",
    );
}

/// `parquet` toglie `ARROW:schema` dai metadati dello schema: il limite
/// conta i metadati chiave-valore del file.
#[test]
fn parquet_metadati_del_file_contati() {
    let tabella =
        RecordBatch::try_from_iter([("v", Arc::new(Int64Array::from(vec![1, 2])) as ArrayRef)])
            .unwrap();
    let dir = cartella();
    let percorso = dir.path().join("m.parquet");
    scrivi_tabella(&tabella, &percorso, &OpzioniScrittura::default()).unwrap();
    let limiti = |massimo| LimitiLettura {
        max_byte_metadati_custom: massimo,
        ..LimitiLettura::default()
    };
    assert!(leggi_tabella_con_limiti(&percorso, None, u64::MAX, &limiti(1 << 20)).is_ok());
    let errore = leggi_tabella_con_limiti(&percorso, None, u64::MAX, &limiti(0))
        .expect_err("ARROW:schema oltre il limite");
    assert_eq!(errore.category(), ErrorCategory::ResourceLimit, "{errore}");
}

/// Un footer IPC che ripete un blocco moltiplicherebbe righe e
/// ricomposizione senza byte nel file.
#[test]
fn ipc_footer_con_blocchi_ripetuti() {
    let mut byte = ipc(false, 2);
    let corpo = corpo_primo_blocco(&byte);
    let primo = corpo - 16;
    let copia = byte[primo..primo + 24].to_vec();
    byte[primo + 24..primo + 48].copy_from_slice(&copia);
    rifiutato(
        &byte,
        Formato::ArrowIpc,
        ErrorCategory::DataMapping,
        "ripetuti o sovrapposti",
    );
}

/// Una colonna senza righe scritta da `parquet-rs`.
#[test]
fn parquet_colonna_vuota_si_legge() {
    use plenora_core::arrow::array::BooleanArray;
    let tabella = RecordBatch::try_from_iter([(
        "x",
        Arc::new(BooleanArray::from(Vec::<bool>::new())) as ArrayRef,
    )])
    .unwrap();
    let dir = cartella();
    let percorso = dir.path().join("vuota.parquet");
    let mut scrittore = parquet::arrow::ArrowWriter::try_new(
        File::create(&percorso).unwrap(),
        tabella.schema(),
        None,
    )
    .unwrap();
    scrittore.write(&tabella).unwrap();
    scrittore.close().unwrap();
    let letta = leggi_tabella(&percorso, None, u64::MAX).expect("colonna vuota");
    assert_eq!(letta.num_rows(), 0);
}

/// Checksum di pagina (fixture pyarrow con `write_page_checksum`, pagina
/// non compressa): intatto si legge; un byte di un valore invertito, che
/// senza verifica tornerebbe un altro numero, e' un errore esplicito.
#[test]
fn parquet_pagina_con_checksum_corrotta_rifiutata() {
    use plenora_core::arrow::array::cast::AsArray;
    use plenora_core::arrow::array::types::Int64Type;
    let percorso = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("dati")
        .join("pyarrow_crc.parquet");
    let byte = std::fs::read(&percorso).unwrap();
    let letta = leggi_byte(&byte, Formato::Parquet, &LimitiLettura::default()).expect("intatto");
    let valori: Vec<i64> = letta
        .column(0)
        .as_primitive::<Int64Type>()
        .values()
        .to_vec();
    assert_eq!(valori, vec![1, 2, 3, 4]);
    // Il valore 3, little-endian a 8 byte, compare una volta sola.
    let tre = 3_i64.to_le_bytes();
    let posizioni: Vec<usize> = byte
        .windows(8)
        .enumerate()
        .filter(|(_, finestra)| *finestra == tre)
        .map(|(posizione, _)| posizione)
        .collect();
    assert_eq!(posizioni.len(), 1, "valore da corrompere non univoco");
    let mut corrotto = byte;
    corrotto[posizioni[0]] = 7;
    let errore = errore(&corrotto, Formato::Parquet, &LimitiLettura::default());
    assert_eq!(errore.category(), ErrorCategory::DataMapping, "{errore}");
}

/// Byte IPC di una tabella con metadati a due chiavi nello schema, in un
/// campo e in un campo annidato; `ripeti` rinomina in tutto il file la
/// seconda chiave come la prima (stessa lunghezza, flatbuffer ancora valido).
fn ipc_con_metadati(stream: bool, ripeti: Option<&str>) -> Vec<u8> {
    let metadati = |prefisso: &str| {
        HashMap::from([
            (format!("{prefisso}1"), "u".to_owned()),
            (format!("{prefisso}2"), "v".to_owned()),
        ])
    };
    let figlio = Field::new("c", DataType::Int64, false).with_metadata(metadati("chiaveN"));
    let schema = Schema::new_with_metadata(
        vec![
            Field::new("id", DataType::Int64, false).with_metadata(metadati("chiaveC")),
            Field::new("s", DataType::Struct(vec![figlio.clone()].into()), false),
        ],
        metadati("chiaveS"),
    );
    let struttura = plenora_core::arrow::array::StructArray::from(vec![(
        Arc::new(figlio),
        Arc::new(Int64Array::from(vec![3, 4])) as ArrayRef,
    )]);
    let tabella = RecordBatch::try_new(
        Arc::new(schema),
        vec![Arc::new(Int64Array::from(vec![1, 2])), Arc::new(struttura)],
    )
    .unwrap();
    let mut byte = Vec::new();
    if stream {
        let mut scrittore = StreamWriter::try_new(&mut byte, &tabella.schema()).unwrap();
        scrittore.write(&tabella).unwrap();
        scrittore.finish().unwrap();
    } else {
        let mut scrittore = FileWriter::try_new(&mut byte, &tabella.schema()).unwrap();
        scrittore.write(&tabella).unwrap();
        scrittore.finish().unwrap();
    }
    if let Some(prefisso) = ripeti {
        let (seconda, prima) = (format!("{prefisso}2"), format!("{prefisso}1"));
        let mut trovate = 0;
        for inizio in 0..=byte.len() - seconda.len() {
            if byte[inizio..].starts_with(seconda.as_bytes()) {
                byte[inizio..inizio + prima.len()].copy_from_slice(prima.as_bytes());
                trovate += 1;
            }
        }
        assert!(trovate > 0, "chiave {seconda} non trovata");
    }
    byte
}

/// Una chiave ripetuta nei metadati di schema, di campo o di un campo
/// annidato: Arrow terrebbe l'ultima, qui è un errore.
#[test]
fn ipc_chiavi_dei_metadati_ripetute_rifiutate() {
    for stream in [false, true] {
        let letta = leggi_byte(
            &ipc_con_metadati(stream, None),
            Formato::ArrowIpc,
            &LimitiLettura::default(),
        )
        .expect("metadati validi");
        assert_eq!(letta.schema().metadata().len(), 2);
        for prefisso in ["chiaveS", "chiaveC", "chiaveN"] {
            rifiutato(
                &ipc_con_metadati(stream, Some(prefisso)),
                Formato::ArrowIpc,
                ErrorCategory::DataMapping,
                "ripetuta",
            );
        }
    }
}
