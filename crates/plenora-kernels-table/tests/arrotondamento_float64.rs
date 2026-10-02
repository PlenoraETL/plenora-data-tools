//! Le operazioni con risultato `Float64` per contratto **arrotondano**; le
//! somme di interi **no**.
//!
//! L'arrotondamento e' una deroga dichiarata alla regola «esatto o errore»
//! (vedi `scalar_as_f64_rounded`): dove il double e' il tipo del risultato
//! (medie, somme di decimali e di testo numerico) un valore oltre la
//! precisione del double la perde **senza errore**. Una somma sul dominio
//! intero (`Int64`, `UInt64`, `Date32`, `Timestamp(ms)`) invece e' esatta:
//! si accumula in `i128` ed esce `Int64`, e oltre la gamma di `Int64` e' un
//! errore (docs/limiti.md, «Somme intere esatte e tipi delle riduzioni»).
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

fn colonna_i64(batch: &RecordBatch, nome: &str) -> Vec<Option<i64>> {
    let indice = batch.schema().index_of(nome).expect("colonna presente");
    let valori = batch
        .column(indice)
        .as_any()
        .downcast_ref::<Int64Array>()
        .expect("colonna Int64")
        .clone();
    valori.iter().collect()
}

// ---------------------------------------------------------------------------
// I quattro chiamanti, ciascuno sul proprio percorso
// ---------------------------------------------------------------------------

/// Una somma sul dominio intero, dai quattro chiamanti: `Int64` esatto.
fn somme_intere(valori: &ArrayRef) -> Vec<plenora_core::Result<Vec<Option<i64>>>> {
    let ingresso = batch("v", Arc::clone(valori), true);
    let aggregata = serde_json::from_value(serde_json::json!({
        "group_by": ["g"],
        "aggregations": [{"column": "v", "function": "sum", "alias": "out"}],
    }))
    .expect("config aggregate");
    let mobile = serde_json::from_value(serde_json::json!({
        "column": "v", "function": "sum", "window": 1, "min_periods": 1, "output_column": "out",
    }))
    .expect("config rolling_window");
    let cumulata = serde_json::from_value(serde_json::json!({
        "column": "v", "function": "cumsum", "output_column": "out",
    }))
    .expect("config window_function");
    vec![
        aggregate(&ingresso, &aggregata).map(|uscita| colonna_i64(&uscita, "out")),
        rolling_window(&ingresso, &mobile).map(|uscita| colonna_i64(&uscita, "out")),
        window_function(&ingresso, &cumulata).map(|uscita| colonna_i64(&uscita, "out")),
        pivot_interi(Arc::clone(valori), "sum"),
    ]
}

fn pivot_interi(valori: ArrayRef, aggregazione: &str) -> plenora_core::Result<Vec<Option<i64>>> {
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
        "index_col": "i", "pivot_col": "p", "value_col": "v", "aggr_func": aggregazione,
    }))
    .expect("config pivot");
    pivot(&ingresso, &config, &Limits::default()).map(|uscita| colonna_i64(&uscita, "c"))
}

fn somma_aggregata(valori: ArrayRef) -> plenora_core::Result<Vec<Option<f64>>> {
    let ingresso = batch("v", valori, true);
    let config = serde_json::from_value(serde_json::json!({
        "group_by": ["g"],
        "aggregations": [{"column": "v", "function": "sum", "alias": "out"}],
    }))
    .expect("config aggregate");
    aggregate(&ingresso, &config).map(|uscita| colonna_f64(&uscita, "out"))
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
// Dominio intero: somma esatta, non arrotondata
// ---------------------------------------------------------------------------

/// Il double di 2^53 + 1: non ne ha uno proprio e cade su 2^53. Scritto per
/// esteso, non calcolato dal codice sotto prova.
const ATTESO_DUE_53_PIU_1: f64 = 9_007_199_254_740_992.0;

/// 2^53 + 1 non ha un `f64` proprio: la somma in double lo portava a 2^53.
/// Ora resta 2^53 + 1, in `Int64`, in tutti e quattro i chiamanti.
#[test]
fn la_somma_degli_interi_oltre_due_53_e_esatta() {
    let valori: ArrayRef = Arc::new(Int64Array::from(vec![DUE_53_PIU_1]));
    for esito in somme_intere(&valori) {
        assert_eq!(esito.expect("somma esatta"), vec![Some(DUE_53_PIU_1)]);
    }
}

/// Gli estremi e il confine restano quelli scritti: `i64::MAX` non sale a
/// 2^63, `i64::MIN` resta `i64::MIN`.
#[test]
fn gli_estremi_int64_restano_esatti() {
    for valore in [DUE_53, DUE_53_PIU_1, i64::MAX, i64::MIN] {
        let valori: ArrayRef = Arc::new(Int64Array::from(vec![valore]));
        for esito in somme_intere(&valori) {
            assert_eq!(esito.expect("somma esatta"), vec![Some(valore)], "{valore}");
        }
    }
}

/// Oltre la gamma di `Int64` la somma e' un errore, mai un valore saturato
/// o arrotondato.
#[test]
fn la_somma_oltre_int64_e_un_errore() {
    let valori: ArrayRef = Arc::new(Int64Array::from(vec![i64::MAX, 1]));
    let esiti = somme_intere(&valori);
    // `aggregate`, `window_function` (cumulata) e `pivot` sommano le due
    // righe; `rolling_window` con finestra 1 no.
    for (indice, esito) in esiti.into_iter().enumerate() {
        if indice == 1 {
            assert!(esito.is_ok());
            continue;
        }
        assert!(
            matches!(esito, Err(PlenoraError::DataMapping(_))),
            "chiamante {indice}: {esito:?}"
        );
    }
}

#[test]
fn int64_null_resta_null() {
    let valori: ArrayRef = Arc::new(Int64Array::from(vec![None::<i64>]));
    for esito in somme_intere(&valori) {
        assert_eq!(esito.expect("null ammesso"), vec![None]);
    }
}

// ---------------------------------------------------------------------------
// UInt64
// ---------------------------------------------------------------------------

#[test]
fn uint64_somma_esatta_e_conserva_i_null() {
    use plenora_core::arrow::array::UInt64Array;

    let oltre: ArrayRef = Arc::new(UInt64Array::from(vec![9_007_199_254_740_993_u64]));
    for esito in somme_intere(&oltre) {
        assert_eq!(esito.expect("somma esatta"), vec![Some(DUE_53_PIU_1)]);
    }
    // `u64::MAX` non sta in `Int64`, il tipo delle somme intere: errore.
    let massimo: ArrayRef = Arc::new(UInt64Array::from(vec![u64::MAX]));
    for esito in somme_intere(&massimo) {
        assert!(matches!(esito, Err(PlenoraError::DataMapping(_))));
    }
    let nullo: ArrayRef = Arc::new(UInt64Array::from(vec![None::<u64>]));
    for esito in somme_intere(&nullo) {
        assert_eq!(esito.expect("null ammesso"), vec![None]);
    }
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
fn il_percorso_generico_arrotonda_decimal_e_testo_non_il_timestamp() {
    // La somma di istanti non ha un significato: si rifiuta in tutti e
    // quattro i chiamanti, invece di rendere millisecondi sommati.
    let timestamp: ArrayRef = Arc::new(
        TimestampMillisecondArray::from(vec![DUE_53_PIU_1])
            .with_data_type(DataType::Timestamp(TimeUnit::Millisecond, None)),
    );
    for esito in somme_intere(&timestamp) {
        assert!(matches!(esito, Err(PlenoraError::InvalidPlan(_))));
    }

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

/// Confronto differenziale, **secondario**: la media di un solo intero e'
/// il suo double, cioe' cio' che rende l'accessore generico. Oltre 2^53 non
/// e' scontato: la media parte dalla somma esatta e arrotonda una volta.
#[test]
fn la_media_di_un_intero_e_il_suo_double() {
    use plenora_kernels_table::scalar_as_f64_rounded;

    let valori = Arc::new(Int64Array::from(vec![DUE_53_PIU_1, i64::MAX, i64::MIN]));
    for riga in 0..valori.len() {
        let atteso =
            scalar_as_f64_rounded(valori.as_ref(), riga).expect("il percorso generico non rifiuta");
        let ingresso = batch(
            "v",
            Arc::new(Int64Array::from(vec![valori.value(riga)])),
            true,
        );
        let config = serde_json::from_value(serde_json::json!({
            "group_by": ["g"],
            "aggregations": [{"column": "v", "function": "mean", "alias": "out"}],
        }))
        .expect("config aggregate");
        let media = aggregate(&ingresso, &config).expect("media");
        assert_eq!(colonna_f64(&media, "out"), vec![atteso], "riga {riga}");
    }
    // L'atteso della prima riga, scritto a mano: il double di 2^53 + 1.
    let atteso = scalar_as_f64_rounded(valori.as_ref(), 0)
        .expect("generico")
        .expect("valore");
    assert_eq!(atteso.to_bits(), ATTESO_DUE_53_PIU_1.to_bits());
}

// ---------------------------------------------------------------------------
// `pivot`, su tutti i rami dell'accessore
// ---------------------------------------------------------------------------
//
// Gli oracoli qui sopra passano quasi tutti da `aggregate`. Ogni operazione
// deve pero' attraversare l'accessore per conto proprio: e' l'unico modo di
// accorgersi se una di esse smettesse di usarlo, o lo usasse diversamente.
// Questi coprono `pivot` su UInt64, Float64, percorso generico ed errori.

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

// ---------------------------------------------------------------------------
// Le varianti di rango decidono: non convertono
// ---------------------------------------------------------------------------
//
// `rank`, `dense_rank`, `percent_rank` e `cume_dist` ordinano e confrontano.
// Se leggessero la colonna come `f64`, 2^53 e 2^53 + 1 diventerebbero lo
// stesso double e due valori distinti risulterebbero a pari merito. Il
// confronto avviene quindi sul dominio originale.

fn rango(valori: Vec<i64>, funzione: &str) -> plenora_core::Result<Vec<Option<f64>>> {
    let ingresso = batch("v", Arc::new(Int64Array::from(valori)), true);
    let config = serde_json::from_value(serde_json::json!({
        "column": "v",
        "function": funzione,
        "output_column": "out",
    }))
    .expect("config window_function");
    window_function(&ingresso, &config).map(|uscita| colonna_f64(&uscita, "out"))
}

/// Due interi **distinti** che condividono lo stesso `f64`.
///
/// Provati nei due ordini: se il confronto passasse dai double, l'esito
/// sarebbe lo stesso in entrambi — a pari merito — mentre il confronto esatto
/// distingue, e l'ordine dice quale viene prima.
#[test]
fn rank_distingue_due_interi_che_condividono_lo_stesso_double() {
    assert_eq!(
        rango(vec![DUE_53, DUE_53_PIU_1], "rank").expect("rank"),
        vec![Some(1.0), Some(2.0)]
    );
    assert_eq!(
        rango(vec![DUE_53_PIU_1, DUE_53], "rank").expect("rank invertito"),
        vec![Some(2.0), Some(1.0)]
    );
}

#[test]
fn dense_rank_distingue_due_interi_che_condividono_lo_stesso_double() {
    assert_eq!(
        rango(vec![DUE_53, DUE_53_PIU_1], "dense_rank").expect("dense_rank"),
        vec![Some(1.0), Some(2.0)]
    );
    assert_eq!(
        rango(vec![DUE_53_PIU_1, DUE_53], "dense_rank").expect("dense_rank invertito"),
        vec![Some(2.0), Some(1.0)]
    );
}

#[test]
fn percent_rank_distingue_due_interi_che_condividono_lo_stesso_double() {
    // Due righe: il rango 0 vale 0.0, il rango 1 vale 1/(2-1) = 1.0.
    assert_eq!(
        rango(vec![DUE_53, DUE_53_PIU_1], "percent_rank").expect("percent_rank"),
        vec![Some(0.0), Some(1.0)]
    );
    assert_eq!(
        rango(vec![DUE_53_PIU_1, DUE_53], "percent_rank").expect("percent_rank invertito"),
        vec![Some(1.0), Some(0.0)]
    );
}

#[test]
fn cume_dist_distingue_due_interi_che_condividono_lo_stesso_double() {
    // Il piu' piccolo copre 1/2 della partizione, il piu' grande 2/2.
    assert_eq!(
        rango(vec![DUE_53, DUE_53_PIU_1], "cume_dist").expect("cume_dist"),
        vec![Some(0.5), Some(1.0)]
    );
    assert_eq!(
        rango(vec![DUE_53_PIU_1, DUE_53], "cume_dist").expect("cume_dist invertito"),
        vec![Some(1.0), Some(0.5)]
    );
}

/// I valori davvero uguali restano a pari merito: il confronto esatto non
/// rende tutto distinto, distingue cio' che e' distinto.
#[test]
fn i_valori_uguali_restano_a_pari_merito() {
    assert_eq!(
        rango(vec![DUE_53, DUE_53], "rank").expect("rank"),
        vec![Some(1.5), Some(1.5)]
    );
    assert_eq!(
        rango(vec![DUE_53, DUE_53], "dense_rank").expect("dense_rank"),
        vec![Some(1.0), Some(1.0)]
    );
}

// ---------------------------------------------------------------------------
// Il dominio numerico e' uno solo, e vale per tutte le varianti
// ---------------------------------------------------------------------------
//
// «Ordinabile» non e' «numerico»: `Boolean`, `Binary` e le dictionary hanno
// un ordine ma non sono numeri, e il testo — che il contratto ammette come
// numero — ordinato per byte metterebbe "10" prima di "9", cioe' nell'ordine
// di un dominio diverso da quello dichiarato.

fn rango_testuale(valori: Vec<&str>, funzione: &str) -> plenora_core::Result<Vec<Option<f64>>> {
    let ingresso = batch("v", Arc::new(StringArray::from(valori)), true);
    let mut grezza = serde_json::json!({
        "column": "v",
        "function": funzione,
        "output_column": "out",
    });
    if funzione == "ntile" {
        grezza["buckets"] = serde_json::json!(2);
    }
    let config = serde_json::from_value(grezza).expect("config window_function");
    window_function(&ingresso, &config).map(|uscita| colonna_f64(&uscita, "out"))
}

#[test]
fn il_rango_rifiuta_il_testo_numerico() {
    // `"9007199254740992"` e `"9007199254740993"` sono numeri distinti.
    // Interpretarli come double li renderebbe a pari merito — la stessa cosa
    // che il confronto sul dominio originale evita per gli interi nativi — e
    // confrontarli esattamente richiederebbe un'aritmetica decimale che
    // questo kernel non ha. Il rango quindi li rifiuta, in entrambi gli
    // ordini e per tutte e quattro le funzioni.
    for funzione in ["rank", "dense_rank", "percent_rank", "cume_dist"] {
        for coppia in [
            vec!["9007199254740992", "9007199254740993"],
            vec!["9007199254740993", "9007199254740992"],
            vec!["10", "9"],
        ] {
            let primo = coppia[0].to_owned();
            let errore = rango_testuale(coppia, funzione)
                .expect_err(&format!("{funzione}: il testo non e' ordinabile"));
            assert!(
                matches!(errore, PlenoraError::Schema(_)),
                "{funzione}: categoria inattesa {errore:?}"
            );
            assert!(
                !errore.to_string().contains(&primo),
                "{funzione}: il messaggio riporta il valore in ingresso: {errore}"
            );
        }
    }
}

/// Il testo resta ammesso dove il risultato e' un valore, non un ordine.
#[test]
fn il_testo_numerico_resta_ammesso_per_le_varianti_di_valore() {
    assert_eq!(
        rango_testuale(vec!["9", "10"], "cumsum").expect("cumsum su testo numerico"),
        vec![Some(9.0), Some(19.0)]
    );
}

#[test]
fn il_testo_non_numerico_e_un_errore_per_ogni_variante() {
    // `cumcount` e `ntile` dipendono dalla sola posizione e non leggono i
    // valori: il contratto della colonna vale lo stesso, e una colonna
    // dichiarata numerica piena di parole resta un ingresso invalido.
    for funzione in [
        "rank",
        "dense_rank",
        "percent_rank",
        "cume_dist",
        "cumsum",
        "cumcount",
        "ntile",
    ] {
        let errore = rango_testuale(vec!["non un numero", "2"], funzione)
            .expect_err(&format!("{funzione}: il testo non numerico e' un errore"));
        assert!(
            matches!(errore, PlenoraError::Schema(_)),
            "{funzione}: categoria inattesa {errore:?}"
        );
        assert!(
            !errore.to_string().contains("non un numero"),
            "{funzione}: il messaggio riporta il valore in ingresso: {errore}"
        );
    }
}

#[test]
fn il_booleano_resta_fuori_dal_dominio_numerico() {
    // `Boolean` e' ordinabile ma non numerico: nessuna variante lo accetta,
    // nemmeno quelle che dipendono dalla sola posizione — il contratto e'
    // della colonna, non di cio' che il kernel poi ne fa.
    for funzione in [
        "rank",
        "dense_rank",
        "percent_rank",
        "cume_dist",
        "cumsum",
        "cumcount",
        "ntile",
    ] {
        let ingresso = batch("v", Arc::new(BooleanArray::from(vec![true, false])), true);
        let mut config = serde_json::json!({
            "column": "v",
            "function": funzione,
            "output_column": "out",
        });
        if funzione == "ntile" {
            config["buckets"] = serde_json::json!(2);
        }
        let config = serde_json::from_value(config).expect("config");
        let errore = window_function(&ingresso, &config)
            .expect_err(&format!("{funzione}: il booleano non e' numerico"));
        assert!(
            matches!(errore, PlenoraError::Schema(_)),
            "{funzione}: categoria inattesa {errore:?}"
        );
    }
}

/// L'analizzatore e il kernel devono dire la stessa cosa.
///
/// Due elenchi di tipi numerici divergerebbero come un piano accettato in
/// analisi e rifiutato in esecuzione — o accettato da entrambi e calcolato su
/// un ordine che nessuno dei due aveva inteso.
#[test]
fn analisi_ed_esecuzione_concordano_sul_dominio() {
    use plenora_core::arrow::schema::Schema as ArrowSchema;
    use plenora_core::contract::{ContractProperties, DataContract, FieldAllocator};
    use plenora_kernels_table::analyze::analyze_table_contract;

    for (tipo, array, numerico) in [
        (
            DataType::Int64,
            Arc::new(Int64Array::from(vec![1_i64, 2])) as ArrayRef,
            true,
        ),
        // Il testo e' nel dominio numerico, ma non ha un ordine esatto: per
        // il rango lo rifiutano **entrambi**, ed e' il punto della prova.
        (
            DataType::Utf8,
            Arc::new(StringArray::from(vec!["1", "2"])) as ArrayRef,
            false,
        ),
        (
            DataType::Boolean,
            Arc::new(BooleanArray::from(vec![true, false])) as ArrayRef,
            false,
        ),
    ] {
        let schema = Arc::new(ArrowSchema::new(vec![
            Field::new("g", DataType::Utf8, false),
            Field::new("v", tipo.clone(), true),
        ]));
        let contratto = DataContract::new(
            Arc::clone(&schema),
            Vec::new(),
            None,
            ContractProperties::default(),
        )
        .expect("contratto");
        // Tutte e quattro le funzioni di rango, non una sola: un
        // analizzatore che ne classificasse solo alcune concorderebbe sul
        // caso provato e divergerebbe sugli altri.
        for funzione in ["rank", "dense_rank", "percent_rank", "cume_dist"] {
            let config = serde_json::json!({
                "column": "v",
                "function": funzione,
                "output_column": "out",
            });
            let mut campi = FieldAllocator::default();
            let analisi = analyze_table_contract(
                "table.window_function",
                std::slice::from_ref(&contratto),
                &config,
                &mut campi,
                &plenora_kernels_table::Limits::default(),
            );

            let ingresso = RecordBatch::try_new(
                Arc::clone(&schema),
                vec![
                    Arc::new(StringArray::from(vec!["g", "g"])) as ArrayRef,
                    Arc::clone(&array),
                ],
            )
            .expect("batch");
            let config = serde_json::from_value(config).expect("config");
            let esecuzione = window_function(&ingresso, &config);

            assert_eq!(
                analisi.is_ok(),
                numerico,
                "{tipo:?}/{funzione}: l'analizzatore non concorda col contratto atteso"
            );
            assert_eq!(
                esecuzione.is_ok(),
                analisi.is_ok(),
                "{tipo:?}/{funzione}: analisi ed esecuzione in disaccordo"
            );
        }
    }
}
