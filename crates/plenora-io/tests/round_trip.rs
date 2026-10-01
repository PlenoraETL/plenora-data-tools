//! Andata e ritorno Arrow IPC (file e stream) e Parquet, bit per bit.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod comune;

use std::fs::File;
use std::sync::Arc;

use plenora_core::arrow::array::{Int64Array, RecordBatch};
use plenora_core::arrow::ipc::writer::StreamWriter;
use plenora_core::arrow::schema::{DataType, Field, Schema};
use plenora_core::ErrorCategory;
use plenora_io::{leggi_tabella, scrivi_tabella, CompressioneParquet, Formato, OpzioniScrittura};

use comune::{cartella, identiche, tabella_larga};

fn opzioni(compressione: CompressioneParquet) -> OpzioniScrittura {
    OpzioniScrittura {
        compressione,
        ..OpzioniScrittura::default()
    }
}

fn andata_e_ritorno(
    tabella: &RecordBatch,
    nome: &str,
    compressione: CompressioneParquet,
) -> RecordBatch {
    let dir = cartella();
    let percorso = dir.path().join(nome);
    scrivi_tabella(tabella, &percorso, &opzioni(compressione)).expect("scrittura");
    leggi_tabella(&percorso, None, u64::MAX).expect("lettura")
}

#[test]
fn ipc_file_e_feather_identici() {
    let tabella = tabella_larga();
    for nome in ["t.arrow", "t.feather", "t.ipc"] {
        let letta = andata_e_ritorno(&tabella, nome, CompressioneParquet::Nessuna);
        identiche(&tabella, &letta);
    }
}

#[test]
fn ipc_stream_identico() {
    let tabella = tabella_larga();
    let dir = cartella();
    let percorso = dir.path().join("t.arrows");
    let mut scrittore =
        StreamWriter::try_new(File::create(&percorso).unwrap(), &tabella.schema()).unwrap();
    // Due blocchi: la lettura li ricompone.
    scrittore.write(&tabella.slice(0, 2)).unwrap();
    scrittore.write(&tabella.slice(2, 2)).unwrap();
    scrittore.finish().unwrap();
    drop(scrittore);
    let letta = leggi_tabella(&percorso, None, u64::MAX).expect("lettura stream");
    identiche(&tabella, &letta);
}

#[test]
fn ipc_piu_blocchi_ricomposti() {
    // Una tabella oltre il blocco di scrittura si scrive in piÃ¹ blocchi.
    let righe = 3_000_000_usize;
    let schema = Arc::new(Schema::new(vec![Field::new("v", DataType::Int64, false)]));
    let valori: Vec<i64> = (0..i64::try_from(righe).unwrap()).collect();
    let tabella = RecordBatch::try_new(schema, vec![Arc::new(Int64Array::from(valori))]).unwrap();
    let letta = andata_e_ritorno(&tabella, "grande.arrow", CompressioneParquet::Nessuna);
    identiche(&tabella, &letta);
}

#[test]
fn parquet_identico_con_ogni_compressione() {
    let tabella = tabella_larga();
    for compressione in [
        CompressioneParquet::Nessuna,
        CompressioneParquet::Snappy,
        CompressioneParquet::Zstd,
    ] {
        let letta = andata_e_ritorno(&tabella, "t.parquet", compressione);
        identiche(&tabella, &letta);
    }
}

#[test]
fn tabelle_vuote() {
    let tabella = tabella_larga().slice(0, 0);
    for nome in ["v.arrow", "v.parquet"] {
        let letta = andata_e_ritorno(&tabella, nome, CompressioneParquet::Zstd);
        identiche(&tabella, &letta);
    }
}

#[test]
fn senza_colonne_solo_ipc() {
    let tabella = plenora_core::batch_with_rows(Arc::new(Schema::empty()), vec![], 5).unwrap();
    let letta = andata_e_ritorno(&tabella, "z.arrow", CompressioneParquet::Nessuna);
    assert_eq!(letta.num_rows(), 5);
    assert_eq!(letta.num_columns(), 0);
    let dir = cartella();
    let errore = scrivi_tabella(
        &tabella,
        &dir.path().join("z.parquet"),
        &OpzioniScrittura::default(),
    )
    .expect_err("Parquet senza colonne");
    assert_eq!(errore.category(), ErrorCategory::Unsupported);
    assert!(!dir.path().join("z.parquet").exists());
}

#[test]
fn parquet_deterministico() {
    let tabella = tabella_larga();
    let dir = cartella();
    let mut byte = Vec::new();
    for nome in ["a.parquet", "b.parquet"] {
        let percorso = dir.path().join(nome);
        scrivi_tabella(&tabella, &percorso, &OpzioniScrittura::default()).unwrap();
        byte.push(std::fs::read(&percorso).unwrap());
    }
    assert_eq!(byte[0], byte[1]);
    // Anche il file IPC.
    let mut ipc = Vec::new();
    for nome in ["a.arrow", "b.arrow"] {
        let percorso = dir.path().join(nome);
        scrivi_tabella(&tabella, &percorso, &OpzioniScrittura::default()).unwrap();
        ipc.push(std::fs::read(&percorso).unwrap());
    }
    assert_eq!(ipc[0], ipc[1]);
}

#[test]
fn parquet_piu_row_group_un_solo_batch() {
    let righe = plenora_io::parquet_io::RIGHE_PER_ROW_GROUP * 2 + 17;
    let schema = Arc::new(Schema::new(vec![Field::new("v", DataType::Int64, true)]));
    let valori: Vec<Option<i64>> = (0..i64::try_from(righe).unwrap())
        .map(|v| (v % 7 != 0).then_some(v))
        .collect();
    let tabella = RecordBatch::try_new(schema, vec![Arc::new(Int64Array::from(valori))]).unwrap();
    let letta = andata_e_ritorno(&tabella, "rg.parquet", CompressioneParquet::Snappy);
    identiche(&tabella, &letta);
}

#[test]
fn formato_esplicito_e_contenuto() {
    let tabella = tabella_larga();
    let dir = cartella();
    let percorso = dir.path().join("dati.bin");
    let opzioni = OpzioniScrittura {
        formato: Some(Formato::Parquet),
        ..OpzioniScrittura::default()
    };
    scrivi_tabella(&tabella, &percorso, &opzioni).unwrap();
    assert!(
        leggi_tabella(&percorso, None, u64::MAX).is_err(),
        "estensione ignota"
    );
    identiche(
        &tabella,
        &leggi_tabella(&percorso, Some(Formato::Parquet), u64::MAX).unwrap(),
    );
    // Un Parquet letto come IPC fallisce con un errore, non con una tabella.
    assert!(leggi_tabella(&percorso, Some(Formato::ArrowIpc), u64::MAX).is_err());
}

#[test]
fn budget_residuo_in_lettura() {
    let tabella = tabella_larga();
    for nome in ["b.arrow", "b.parquet"] {
        let dir = cartella();
        let percorso = dir.path().join(nome);
        scrivi_tabella(&tabella, &percorso, &OpzioniScrittura::default()).unwrap();
        let errore = leggi_tabella(&percorso, None, 64).expect_err("budget");
        assert_eq!(errore.category(), ErrorCategory::ResourceLimit, "{nome}");
    }
}

/// Tipi fuori dalla tabella larga: o tornano identici, o la scrittura
/// fallisce con un errore e senza file. Mai un tipo cambiato in silenzio.
#[test]
#[allow(clippy::too_many_lines)] // Un caso per tipo, in un posto solo.
fn tipi_rari_identici_o_rifiutati() {
    use plenora_core::arrow::array::builder::{
        Int32Builder, LargeListBuilder, MapBuilder, StringBuilder,
    };
    use plenora_core::arrow::array::types::{
        Float64Type, Int32Type, Int8Type, IntervalDayTimeType, IntervalMonthDayNanoType, UInt16Type,
    };
    use plenora_core::arrow::array::{
        ArrayRef, BinaryViewArray, Decimal32Array, Decimal64Array, DictionaryArray,
        DurationNanosecondArray, DurationSecondArray, FixedSizeListArray, IntervalDayTimeArray,
        IntervalMonthDayNanoArray, IntervalYearMonthArray, LargeStringArray, RunArray, StringArray,
        StringViewArray, Time32MillisecondArray, Time64MicrosecondArray, TimestampSecondArray,
    };

    let mut lista_grande = LargeListBuilder::new(StringBuilder::new());
    lista_grande.append_value([Some("a"), None]);
    lista_grande.append_null();
    lista_grande.append_value([Some("")]);
    let mut mappa = MapBuilder::new(None, StringBuilder::new(), Int32Builder::new());
    mappa.keys().append_value("k");
    mappa.values().append_value(1);
    mappa.append(true).unwrap();
    mappa.append(false).unwrap();
    mappa.keys().append_value("j");
    mappa.values().append_null();
    mappa.append(true).unwrap();
    let dizionario_largo: DictionaryArray<Int8Type> = DictionaryArray::try_new(
        vec![Some(0_i8), None, Some(1)].into(),
        Arc::new(LargeStringArray::from(vec!["x", "y"])),
    )
    .unwrap();
    let dizionario_u16: DictionaryArray<UInt16Type> =
        vec![Some("p"), Some("p"), None].into_iter().collect();
    let run = RunArray::<Int32Type>::try_new(
        &vec![2_i32, 3].into(),
        &(Arc::new(StringArray::from(vec!["r", "s"])) as ArrayRef),
    )
    .unwrap();

    let casi: Vec<(&str, ArrayRef)> = vec![
        (
            "fixed_list",
            Arc::new(
                FixedSizeListArray::from_iter_primitive::<Float64Type, _, _>(
                    vec![
                        Some(vec![Some(1.0), Some(-0.0)]),
                        None,
                        Some(vec![None, Some(f64::NAN)]),
                    ],
                    2,
                ),
            ),
        ),
        ("large_list", Arc::new(lista_grande.finish())),
        ("mappa", Arc::new(mappa.finish())),
        (
            "utf8_view",
            Arc::new(StringViewArray::from(vec![
                Some("breve"),
                None,
                Some("una stringa lunga oltre dodici byte"),
            ])),
        ),
        (
            "binary_view",
            Arc::new(BinaryViewArray::from(vec![
                Some(&b"a"[..]),
                None,
                Some(&b"bb"[..]),
            ])),
        ),
        (
            "intervallo_ym",
            Arc::new(IntervalYearMonthArray::from(vec![Some(-3), None, Some(14)])),
        ),
        (
            "intervallo_dt",
            Arc::new(IntervalDayTimeArray::from(vec![
                Some(IntervalDayTimeType::make_value(1, -2)),
                None,
                Some(IntervalDayTimeType::make_value(0, 0)),
            ])),
        ),
        (
            "intervallo_mdn",
            Arc::new(IntervalMonthDayNanoArray::from(vec![
                Some(IntervalMonthDayNanoType::make_value(1, 2, -3)),
                None,
                Some(IntervalMonthDayNanoType::make_value(0, 0, i64::MAX)),
            ])),
        ),
        ("dizionario_large", Arc::new(dizionario_largo)),
        ("dizionario_u16", Arc::new(dizionario_u16)),
        ("run_end", Arc::new(run)),
        (
            "ts_s",
            Arc::new(TimestampSecondArray::from(vec![
                Some(i64::MIN),
                None,
                Some(i64::MAX),
            ])),
        ),
        (
            "ora_ms",
            Arc::new(Time32MillisecondArray::from(vec![0, 1, 86_399_999])),
        ),
        (
            "ora_us",
            Arc::new(Time64MicrosecondArray::from(vec![0, 1, 86_399_999_999])),
        ),
        (
            "durata_s",
            Arc::new(DurationSecondArray::from(vec![
                Some(i64::MIN),
                None,
                Some(i64::MAX),
            ])),
        ),
        (
            "durata_ns",
            Arc::new(DurationNanosecondArray::from(vec![Some(-1), None, Some(1)])),
        ),
        (
            "dec32",
            Arc::new(
                Decimal32Array::from(vec![Some(-999_999_999), None, Some(1)])
                    .with_precision_and_scale(9, 3)
                    .unwrap(),
            ),
        ),
        (
            "dec64",
            Arc::new(
                Decimal64Array::from(vec![Some(-1), None, Some(999_999_999_999_999_999)])
                    .with_precision_and_scale(18, 0)
                    .unwrap(),
            ),
        ),
    ];
    let mut rifiutati = Vec::new();
    for (nome, colonna) in casi {
        let schema = Arc::new(Schema::new(vec![Field::new(
            nome,
            colonna.data_type().clone(),
            true,
        )]));
        let tabella = RecordBatch::try_new(schema, vec![colonna]).unwrap();
        for file in ["r.arrow", "r.parquet"] {
            let dir = cartella();
            let percorso = dir.path().join(file);
            if scrivi_tabella(&tabella, &percorso, &OpzioniScrittura::default()).is_ok() {
                let letta = leggi_tabella(&percorso, None, u64::MAX)
                    .unwrap_or_else(|errore| panic!("{nome} {file}: {errore}"));
                identiche(&tabella, &letta);
            } else {
                assert!(!percorso.exists(), "{nome} {file}: file parziale");
                rifiutati.push(format!("{nome} {file}"));
            }
        }
    }
    // L'elenco dei rifiutati e' dichiarato nel README (Â«FileÂ»): un tipo che
    // entra o esce da qui e' una modifica da dichiarare.
    assert_eq!(
        rifiutati,
        [
            "intervallo_mdn r.parquet",
            "run_end r.arrow",
            "run_end r.parquet"
        ]
    );
}

/// `parquet` restringe i decimali alla precisione del tipo (`as i32`,
/// byte troncati): un valore fuori precisione si rifiuta prima di scrivere,
/// anche annidato. Arrow IPC lo conserva com'e'.
#[test]
fn decimali_fuori_precisione_rifiutati_in_parquet() {
    use plenora_core::arrow::array::builder::{Decimal128Builder, ListBuilder};
    use plenora_core::arrow::array::{ArrayRef, Decimal128Array};

    let piatta: ArrayRef = Arc::new(
        Decimal128Array::from(vec![Some(1), None, Some(10_000_000_000)])
            .with_precision_and_scale(5, 0)
            .unwrap(),
    );
    let mut lista = ListBuilder::new(
        Decimal128Builder::new()
            .with_precision_and_scale(4, 2)
            .unwrap(),
    );
    lista.values().append_value(12);
    lista.values().append_value(123_456);
    lista.append(true);
    let annidata: ArrayRef = Arc::new(lista.finish());
    for colonna in [piatta, annidata] {
        let schema = Arc::new(Schema::new(vec![Field::new(
            "d",
            colonna.data_type().clone(),
            true,
        )]));
        let tabella = RecordBatch::try_new(schema, vec![colonna]).unwrap();
        let dir = cartella();
        let percorso = dir.path().join("d.parquet");
        let errore = scrivi_tabella(&tabella, &percorso, &OpzioniScrittura::default())
            .expect_err("fuori precisione");
        assert_eq!(errore.category(), ErrorCategory::DataMapping, "{errore}");
        assert!(
            !errore.to_string().contains("10000000000"),
            "errore con il valore"
        );
        assert!(!percorso.exists());
        let ipc = andata_e_ritorno(&tabella, "d.arrow", CompressioneParquet::Nessuna);
        identiche(&tabella, &ipc);
    }
}

/// `INT96` si converte con aritmetica che avvolge: si rifiuta.
#[test]
fn int96_rifiutato() {
    let percorso = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("dati")
        .join("pyarrow_int96.parquet");
    let errore = leggi_tabella(&percorso, None, u64::MAX).expect_err("INT96");
    assert_eq!(errore.category(), ErrorCategory::Unsupported, "{errore}");
    assert!(errore.to_string().contains("INT96"), "{errore}");
}

/// Chiavi ripetute nei metadati del file, o in conflitto con lo schema
/// incorporato: `parquet` ne terrebbe una in silenzio.
#[test]
fn metadati_ambigui_rifiutati() {
    use parquet::arrow::ArrowWriter;
    use parquet::file::metadata::KeyValue;
    use parquet::file::properties::WriterProperties;

    let scrivi = |voci: Vec<KeyValue>, metadati_schema: &[(&str, &str)]| {
        let dir = cartella();
        let percorso = dir.path().join("m.parquet");
        let tabella = comune::ordini();
        let schema = Arc::new(Schema::new_with_metadata(
            tabella.schema().fields().clone(),
            metadati_schema
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect::<plenora_core::arrow::Metadata>(),
        ));
        let tabella = tabella.with_schema(schema.clone()).unwrap();
        let proprieta = WriterProperties::builder()
            .set_key_value_metadata(Some(voci))
            .build();
        let mut w = ArrowWriter::try_new(File::create(&percorso).unwrap(), schema, Some(proprieta))
            .unwrap();
        w.write(&tabella).unwrap();
        w.close().unwrap();
        leggi_tabella(&percorso, None, u64::MAX)
    };
    let voce = |k: &str, v: &str| KeyValue::new(k.to_owned(), v.to_owned());
    let errore = scrivi(vec![voce("x", "1"), voce("x", "2")], &[]).expect_err("ripetuta");
    assert_eq!(errore.category(), ErrorCategory::DataMapping, "{errore}");
    let errore = scrivi(vec![voce("x", "1")], &[("x", "2")]).expect_err("conflitto");
    assert_eq!(errore.category(), ErrorCategory::DataMapping, "{errore}");
    // Stesso valore nei due posti (come scrive pyarrow): ammesso.
    let letta = scrivi(vec![voce("x", "1")], &[("x", "1")]).expect("coerente");
    assert_eq!(
        letta.schema().metadata().get("x").map(String::as_str),
        Some("1")
    );
}

/// Il transitorio di scrittura si misura sui blocchi veri: una riga molto
/// piu' grande della media non sfugge.
#[test]
fn transitorio_con_righe_sbilanciate() {
    use plenora_core::arrow::array::StringArray;

    let grande = "x".repeat(20 * 1024 * 1024);
    let mut valori: Vec<&str> = vec!["a"; 100_000];
    valori.push(&grande);
    let schema = Arc::new(Schema::new(vec![Field::new("s", DataType::Utf8, false)]));
    let tabella = RecordBatch::try_new(schema, vec![Arc::new(StringArray::from(valori))]).unwrap();
    let byte = u64::try_from(grande.len()).unwrap();
    assert!(plenora_io::ipc::transitorio_scrittura(&tabella) >= 2 * byte);
    assert!(
        plenora_io::parquet_io::transitorio_scrittura(&tabella)
            >= plenora_io::parquet_io::FATTORE_SCRITTURA * byte
    );
}

/// Un file Arrow IPC con una colonna run-end si rifiuta alla lettura, prima
/// di decodificare i blocchi: i kernel non trattano run-end e union.
#[test]
fn ipc_con_run_end_si_rifiuta() {
    use plenora_core::arrow::array::types::Int32Type;
    use plenora_core::arrow::array::{Array, Int32Array, RunArray, StringArray};
    let run = RunArray::<Int32Type>::try_new(
        &Int32Array::from(vec![2, 3]),
        &StringArray::from(vec![Some("a"), None]),
    )
    .expect("run-end");
    let schema = Arc::new(Schema::new(vec![Field::new(
        "r",
        run.data_type().clone(),
        true,
    )]));
    let tabella = RecordBatch::try_new(schema.clone(), vec![Arc::new(run)]).expect("tabella");
    let dir = cartella();
    let percorso = dir.path().join("r.arrows");
    let mut scrittore = StreamWriter::try_new(File::create(&percorso).unwrap(), &schema).unwrap();
    scrittore.write(&tabella).unwrap();
    scrittore.finish().unwrap();
    drop(scrittore);
    let errore = leggi_tabella(&percorso, None, u64::MAX).expect_err("run-end rifiutata");
    assert_eq!(errore.category(), ErrorCategory::Unsupported, "{errore}");
}
