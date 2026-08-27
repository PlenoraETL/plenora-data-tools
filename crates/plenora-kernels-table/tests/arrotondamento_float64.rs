//! Le operazioni con risultato `Float64` per contratto **arrotondano**.
//!
//! L'autorita' e' `docs/errori-e-limiti.md#arrotondamento-nelle-operazioni-a-risultato-float64`:
//! in quelle operazioni il double e' il tipo del risultato, non un passaggio
//! intermedio, e pretendere l'esattezza rifiuterebbe input legittimi. Un
//! valore oltre 2^53 perde precisione **senza errore**.
//!
//! Gli attesi qui sono letterali, calcolati dalla regola IEEE 754 e non da
//! `scalar_as_f64_rounded`: un oracolo che chiedesse al codice quale sia la
//! risposta giusta direbbe soltanto che il codice concorda con se stesso.
//! `scalar_as_f64_rounded` compare in una prova sola, come confronto
//! differenziale fra i due percorsi dello stesso accessore.
//!
//! Le quattro operazioni provate sono i chiamanti diretti dell'accessore:
//! `aggregate`, `rolling_window`, `window_function`, `pivot`. Ciascuna ha un
//! oracolo che passa **davvero** dal proprio uso, perche' provare il solo
//! accessore interno lascerebbe scoperto chi lo chiama.

use std::sync::Arc;

use plenora_core::arrow::array::{
    Array, ArrayRef, BooleanArray, Decimal128Array, Float64Array, Int64Array, RecordBatch,
    StringArray, TimestampMillisecondArray,
};
use plenora_core::arrow::schema::{DataType, Field, Schema, TimeUnit};
use plenora_core::PlenoraError;
use plenora_kernels_table::aggregation::{aggregate, rolling_window, window_function};
use plenora_kernels_table::reshape::pivot;
use plenora_kernels_table::Limits;

/// 2^53: l'ultimo intero con un `f64` tutto suo.
const DUE_53: i64 = 9_007_199_254_740_992;
/// 2^53 + 1: il primo che non ce l'ha. In `f64` diventa 2^53.
const DUE_53_PIU_1: i64 = 9_007_199_254_740_993;

fn batch(nome: &str, valori: ArrayRef, nullable: bool) -> RecordBatch {
    let gruppi: ArrayRef = Arc::new(StringArray::from(vec!["g"; valori.len()]));
    RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("g", DataType::Utf8, false),
            Field::new(nome, valori.data_type().clone(), nullable),
        ])),
        vec![gruppi, valori],
    )
    .expect("batch di prova")
}

fn colonna_f64(batch: &RecordBatch, nome: &str) -> Vec<Option<f64>> {
    let indice = batch.schema().index_of(nome).expect("colonna presente");
    let array = batch.column(indice);
    let valori = array
        .as_any()
        .downcast_ref::<Float64Array>()
        .expect("colonna Float64");
    (0..valori.len())
        .map(|riga| {
            if valori.is_null(riga) {
                None
            } else {
                Some(valori.value(riga))
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// I quattro chiamanti, ciascuno sul proprio percorso
// ---------------------------------------------------------------------------

fn somma_aggregata(valori: ArrayRef) -> plenora_core::Result<Vec<Option<f64>>> {
    let ingresso = batch("v", valori, true);
    let config = serde_json::from_value(serde_json::json!({
        "group_by": ["g"],
        "aggregations": [{"column": "v", "function": "sum", "alias": "out"}],
    }))
    .expect("config aggregate");
    aggregate(&ingresso, &config).map(|uscita| colonna_f64(&uscita, "out"))
}

fn somma_mobile(valori: ArrayRef) -> plenora_core::Result<Vec<Option<f64>>> {
    let ingresso = batch("v", valori, true);
    let config = serde_json::from_value(serde_json::json!({
        "column": "v",
        "function": "sum",
        "window": 1,
        "min_periods": 1,
        "output_column": "out",
    }))
    .expect("config rolling_window");
    rolling_window(&ingresso, &config).map(|uscita| colonna_f64(&uscita, "out"))
}

fn somma_cumulata(valori: ArrayRef) -> plenora_core::Result<Vec<Option<f64>>> {
    let ingresso = batch("v", valori, true);
    let config = serde_json::from_value(serde_json::json!({
        "column": "v",
        "function": "cumsum",
        "output_column": "out",
    }))
    .expect("config window_function");
    window_function(&ingresso, &config).map(|uscita| colonna_f64(&uscita, "out"))
}

fn somma_pivotata(valori: ArrayRef) -> plenora_core::Result<Vec<Option<f64>>> {
    pivotata_con(valori, "sum")
}

/// `pivot` con la riduzione dichiarata.
///
/// Serve parametrizzata perche' `min` su una riga sola rende quel valore
/// senza sommarlo: e' l'unico modo di guardare `-0.0` e `NaN` come li vede
/// **l'accessore**, senza che la riduzione ci metta del suo.
fn pivotata_con(valori: ArrayRef, aggregazione: &str) -> plenora_core::Result<Vec<Option<f64>>> {
    let righe = valori.len();
    let ingresso = RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("i", DataType::Utf8, false),
            Field::new("p", DataType::Utf8, false),
            Field::new("v", valori.data_type().clone(), true),
        ])),
        vec![
            Arc::new(StringArray::from(vec!["r"; righe])) as ArrayRef,
            Arc::new(StringArray::from(vec!["c"; righe])) as ArrayRef,
            valori,
        ],
    )
    .expect("batch pivot");
    let config = serde_json::from_value(serde_json::json!({
        "index_col": "i",
        "pivot_col": "p",
        "value_col": "v",
        "aggr_func": aggregazione,
    }))
    .expect("config pivot");
    pivot(&ingresso, &config, &Limits::default()).map(|uscita| colonna_f64(&uscita, "c"))
}

// ---------------------------------------------------------------------------
// Int64 oltre 2^53: arrotondamento, non rifiuto
// ---------------------------------------------------------------------------

/// L'attesa: 2^53 + 1 non ha un `f64` proprio e cade su 2^53. Il valore e'
/// scritto per esteso, non calcolato dal codice sotto prova.
const ATTESO_DUE_53_PIU_1: f64 = 9_007_199_254_740_992.0;

fn interi_oltre_soglia() -> ArrayRef {
    Arc::new(Int64Array::from(vec![DUE_53_PIU_1]))
}

#[test]
fn aggregate_arrotonda_gli_interi_oltre_due_53() {
    assert_eq!(
        somma_aggregata(interi_oltre_soglia()).expect("aggregate non deve rifiutare"),
        vec![Some(ATTESO_DUE_53_PIU_1)]
    );
}

#[test]
fn rolling_window_arrotonda_gli_interi_oltre_due_53() {
    assert_eq!(
        somma_mobile(interi_oltre_soglia()).expect("rolling_window non deve rifiutare"),
        vec![Some(ATTESO_DUE_53_PIU_1)]
    );
}

#[test]
fn window_function_arrotonda_gli_interi_oltre_due_53() {
    assert_eq!(
        somma_cumulata(interi_oltre_soglia()).expect("window_function non deve rifiutare"),
        vec![Some(ATTESO_DUE_53_PIU_1)]
    );
}

#[test]
fn pivot_arrotonda_gli_interi_oltre_due_53() {
    assert_eq!(
        somma_pivotata(interi_oltre_soglia()).expect("pivot non deve rifiutare"),
        vec![Some(ATTESO_DUE_53_PIU_1)]
    );
}

/// Gli estremi e il confine, con gli attesi scritti a mano.
///
/// `i64::MAX` non e' rappresentabile e sale a 2^63; `i64::MIN` e' una potenza
/// di due e resta esatto; 2^53 e' l'ultimo intero con un double proprio.
#[test]
fn gli_estremi_int64_arrotondano_verso_il_double_piu_vicino() {
    for (valore, atteso) in [
        (DUE_53, 9_007_199_254_740_992.0_f64),
        (DUE_53_PIU_1, 9_007_199_254_740_992.0),
        (i64::MAX, 9_223_372_036_854_775_808.0),
        (i64::MIN, -9_223_372_036_854_775_808.0),
    ] {
        let ottenuto = somma_aggregata(Arc::new(Int64Array::from(vec![valore])))
            .unwrap_or_else(|errore| panic!("{valore} rifiutato: {errore}"));
        assert_eq!(ottenuto, vec![Some(atteso)], "valore {valore}");
    }
}

#[test]
fn int64_null_resta_null() {
    let valori: ArrayRef = Arc::new(Int64Array::from(vec![None::<i64>]));
    assert_eq!(somma_aggregata(valori).expect("null ammesso"), vec![None]);
}

// ---------------------------------------------------------------------------
// UInt64
// ---------------------------------------------------------------------------

#[test]
fn uint64_arrotonda_e_conserva_i_null() {
    use plenora_core::arrow::array::UInt64Array;

    let oltre: ArrayRef = Arc::new(UInt64Array::from(vec![9_007_199_254_740_993_u64]));
    assert_eq!(
        somma_aggregata(oltre).expect("uint64 oltre 2^53 non deve essere rifiutato"),
        vec![Some(9_007_199_254_740_992.0)]
    );

    let massimo: ArrayRef = Arc::new(UInt64Array::from(vec![u64::MAX]));
    assert_eq!(
        somma_aggregata(massimo).expect("u64::MAX non deve essere rifiutato"),
        vec![Some(18_446_744_073_709_551_616.0)]
    );

    let nullo: ArrayRef = Arc::new(UInt64Array::from(vec![None::<u64>]));
    assert_eq!(somma_aggregata(nullo).expect("null ammesso"), vec![None]);
}

// ---------------------------------------------------------------------------
// Float64: null, -0.0, NaN
// ---------------------------------------------------------------------------

#[test]
fn float64_conserva_null_zero_negativo_e_nan() {
    let nullo: ArrayRef = Arc::new(Float64Array::from(vec![None::<f64>]));
    assert_eq!(somma_aggregata(nullo).expect("null ammesso"), vec![None]);

    // `Iterator::sum` parte da -0.0 per conservare il segno: la somma di un
    // solo -0.0 resta -0.0, e la differenza da +0.0 si vede solo sui bit.
    let meno_zero: ArrayRef = Arc::new(Float64Array::from(vec![-0.0_f64]));
    let somma = somma_aggregata(meno_zero).expect("-0.0 ammesso");
    let valore = somma[0].expect("un valore");
    assert_eq!(
        valore.to_bits(),
        (-0.0_f64).to_bits(),
        "lo zero ha perso il segno: {valore}"
    );

    let nan: ArrayRef = Arc::new(Float64Array::from(vec![f64::NAN]));
    let somma = somma_aggregata(nan).expect("NaN ammesso");
    assert!(somma[0].expect("un valore").is_nan(), "NaN non conservato");
}

// ---------------------------------------------------------------------------
// Percorso generico: timestamp, decimal, testo numerico
// ---------------------------------------------------------------------------

#[test]
fn il_percorso_generico_arrotonda_timestamp_decimal_e_testo() {
    let timestamp: ArrayRef = Arc::new(
        TimestampMillisecondArray::from(vec![DUE_53_PIU_1])
            .with_data_type(DataType::Timestamp(TimeUnit::Millisecond, None)),
    );
    assert_eq!(
        somma_aggregata(timestamp).expect("timestamp oltre 2^53 non deve essere rifiutato"),
        vec![Some(ATTESO_DUE_53_PIU_1)]
    );

    // 12345 con scala 2 vale 123.45, che in binario non e' esatto: e' il
    // caso che la semantica dichiarata ammette.
    let decimale: ArrayRef = Arc::new(
        Decimal128Array::from(vec![12_345_i128])
            .with_precision_and_scale(10, 2)
            .expect("decimal valido"),
    );
    let somma = somma_aggregata(decimale).expect("decimal ammesso");
    assert_eq!(somma, vec![Some(123.45)]);

    let testo: ArrayRef = Arc::new(StringArray::from(vec!["123.5"]));
    assert_eq!(
        somma_aggregata(testo).expect("testo numerico ammesso"),
        vec![Some(123.5)]
    );
}

// ---------------------------------------------------------------------------
// Rifiuti che restano rifiuti
// ---------------------------------------------------------------------------

#[test]
fn testo_non_numerico_e_tipo_non_convertibile_restano_errori_di_schema() {
    let testo: ArrayRef = Arc::new(StringArray::from(vec!["non un numero"]));
    let errore = somma_aggregata(testo).expect_err("il testo non numerico e' un errore");
    assert!(
        matches!(errore, PlenoraError::Schema(_)),
        "categoria inattesa: {errore:?}"
    );
    // Il valore che ha causato l'errore non entra nel messaggio: lo sceglie
    // chi manda i dati, e finirebbe nel log di chi indaga.
    assert!(
        !errore.to_string().contains("non un numero"),
        "il messaggio riporta il valore in ingresso: {errore}"
    );

    let booleani: ArrayRef = Arc::new(BooleanArray::from(vec![true]));
    let errore = somma_aggregata(booleani).expect_err("il booleano non e' convertibile");
    assert!(
        matches!(errore, PlenoraError::Schema(_)),
        "categoria inattesa: {errore:?}"
    );
}

// ---------------------------------------------------------------------------
// I due percorsi dello stesso accessore devono concordare
// ---------------------------------------------------------------------------

/// Confronto differenziale, **secondario**: gli attesi qui sopra non vengono
/// da qui. Serve a dire che il percorso veloce (Int64/UInt64 nativi) e quello
/// generico dello stesso accessore rispondono la stessa cosa, cosa che prima
/// della correzione non era vera oltre 2^53.
#[test]
fn il_percorso_veloce_e_quello_generico_concordano() {
    use plenora_kernels_table::scalar_as_f64_rounded;

    let valori = Arc::new(Int64Array::from(vec![DUE_53_PIU_1, i64::MAX, i64::MIN]));
    let generico: Vec<Option<f64>> = (0..valori.len())
        .map(|riga| {
            scalar_as_f64_rounded(valori.as_ref(), riga).expect("il percorso generico non rifiuta")
        })
        .collect();
    for (riga, atteso) in generico.iter().enumerate() {
        let una: ArrayRef = Arc::new(Int64Array::from(vec![valori.value(riga)]));
        assert_eq!(
            somma_aggregata(una).expect("il percorso veloce non deve rifiutare"),
            vec![*atteso],
            "riga {riga}: i due percorsi divergono"
        );
    }
}

// ---------------------------------------------------------------------------
// La seconda copia dell'accessore, provata su tutti i suoi rami
// ---------------------------------------------------------------------------
//
// L'accessore di `pivot` e' una copia di quello delle aggregazioni, e finche'
// sono due copie il contratto va provato su entrambe: gli oracoli qui sopra
// passano quasi tutti da `aggregate`, e lascerebbero scoperti i rami UInt64,
// Float64, generico ed errori della copia di `reshape`.

#[test]
fn pivot_arrotonda_gli_uint64_e_conserva_i_null() {
    use plenora_core::arrow::array::UInt64Array;

    let oltre: ArrayRef = Arc::new(UInt64Array::from(vec![9_007_199_254_740_993_u64]));
    assert_eq!(
        somma_pivotata(oltre).expect("uint64 oltre 2^53 non deve essere rifiutato"),
        vec![Some(9_007_199_254_740_992.0)]
    );

    let massimo: ArrayRef = Arc::new(UInt64Array::from(vec![u64::MAX]));
    assert_eq!(
        somma_pivotata(massimo).expect("u64::MAX non deve essere rifiutato"),
        vec![Some(18_446_744_073_709_551_616.0)]
    );

    let nullo: ArrayRef = Arc::new(UInt64Array::from(vec![None::<u64>]));
    assert_eq!(somma_pivotata(nullo).expect("null ammesso"), vec![None]);
}

#[test]
fn pivot_conserva_null_zero_negativo_e_nan() {
    let nullo: ArrayRef = Arc::new(Float64Array::from(vec![None::<f64>]));
    assert_eq!(somma_pivotata(nullo).expect("null ammesso"), vec![None]);

    // `min` su una riga sola rende quella riga: cio' che si guarda e' il
    // segno che l'accessore ha letto, non cosa fa la somma.
    let meno_zero: ArrayRef = Arc::new(Float64Array::from(vec![-0.0_f64]));
    let valore = pivotata_con(meno_zero, "min").expect("-0.0 ammesso")[0].expect("un valore");
    assert_eq!(
        valore.to_bits(),
        (-0.0_f64).to_bits(),
        "lo zero ha perso il segno: {valore}"
    );

    let nan: ArrayRef = Arc::new(Float64Array::from(vec![f64::NAN]));
    let valore = pivotata_con(nan, "min").expect("NaN ammesso")[0].expect("un valore");
    assert!(valore.is_nan(), "NaN non conservato");
}

#[test]
fn pivot_passa_dal_percorso_generico_per_decimal_e_testo() {
    let decimale: ArrayRef = Arc::new(
        Decimal128Array::from(vec![12_345_i128])
            .with_precision_and_scale(10, 2)
            .expect("decimal valido"),
    );
    assert_eq!(
        somma_pivotata(decimale).expect("decimal ammesso"),
        vec![Some(123.45)]
    );

    let testo: ArrayRef = Arc::new(StringArray::from(vec!["123.5"]));
    assert_eq!(
        somma_pivotata(testo).expect("testo numerico ammesso"),
        vec![Some(123.5)]
    );
}

#[test]
fn pivot_rifiuta_il_testo_non_numerico_e_i_tipi_non_convertibili() {
    let testo: ArrayRef = Arc::new(StringArray::from(vec!["non un numero"]));
    let errore = somma_pivotata(testo).expect_err("il testo non numerico e' un errore");
    assert!(
        matches!(errore, PlenoraError::Schema(_)),
        "categoria inattesa: {errore:?}"
    );
    assert!(
        !errore.to_string().contains("non un numero"),
        "il messaggio riporta il valore in ingresso: {errore}"
    );

    let booleani: ArrayRef = Arc::new(BooleanArray::from(vec![true]));
    let errore = somma_pivotata(booleani).expect_err("il booleano non e' convertibile");
    assert!(
        matches!(errore, PlenoraError::Schema(_)),
        "categoria inattesa: {errore:?}"
    );
}
