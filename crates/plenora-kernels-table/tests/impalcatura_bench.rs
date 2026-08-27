//! Il generatore delle fixture di benchmark non deve cambiare in silenzio.
//!
//! `examples/comune/rng.rs` non e' raggiungibile da un test come modulo del
//! crate — gli esempi sono target a se' — quindi lo si include per percorso.
//! E' l'unico modo per far girare questa prova nella suite invece di
//! affidarla a chi si ricorda di lanciare un benchmark.

#[path = "../examples/comune/rng.rs"]
mod rng;

use rng::Rng;

/// I primi otto valori con seme 42.
///
/// **Non** sono stati letti dall'implementazione: sono stati calcolati a
/// parte dalla definizione di xorshift64 — `x ^= x << 13; x ^= x >> 7;
/// x ^= x << 17` su aritmetica a 64 bit — e trascritti qui. Se fossero
/// stati generati dal codice che devono sorvegliare, direbbero soltanto che
/// il codice concorda con se stesso.
const ATTESI: [u64; 8] = [
    45_454_805_674,
    11_532_217_803_599_905_471,
    10_021_416_941_527_320_954,
    2_899_061_411_254_629_736,
    5_661_411_637_479_084_162,
    10_094_803_945_545_275_427,
    11_439_985_393_877_029_075,
    10_624_261_412_083_704_114,
];

#[test]
fn la_sequenza_delle_fixture_e_quella_dichiarata() {
    let mut rng = Rng::seeded();
    let ottenuti: Vec<u64> = (0..ATTESI.len()).map(|_| rng.next()).collect();
    assert_eq!(
        ottenuti,
        ATTESI.to_vec(),
        "la sequenza del generatore e' cambiata: le fixture di benchmark non \
         sono piu' quelle su cui la baseline e' stata raccolta"
    );
}

/// Due generatori appena costruiti partono dallo stesso punto.
///
/// Senza questo, un seme preso dall'orologio passerebbe il test qui sopra
/// alla prima esecuzione e renderebbe irriproducibile la seconda.
#[test]
fn due_generatori_partono_uguali() {
    let mut uno = Rng::seeded();
    let mut due = Rng::seeded();
    for passo in 0..16 {
        assert_eq!(uno.next(), due.next(), "divergenza al passo {passo}");
    }
}

/// Il secondo seme, quello dell'insieme che non deve sovrapporsi al primo.
///
/// Calcolati fuori da qui come gli altri. Senza questa prova, `con_seme`
/// potrebbe ignorare l'argomento e ripartire da 42: le due meta' di
/// `setop_right_fixture` coinciderebbero, e le set operation misurerebbero un
/// caso degenere invece di quello dichiarato.
#[test]
fn il_seme_dichiarato_e_quello_usato() {
    const ATTESI_43: [u64; 4] = [
        46_537_075_435,
        12_685_210_767_805_585_150,
        1_156_789_259_011_059_539,
        15_956_110_530_553_476_429,
    ];
    let mut rng = Rng::con_seme(43);
    let ottenuti: Vec<u64> = (0..ATTESI_43.len()).map(|_| rng.next()).collect();
    assert_eq!(ottenuti, ATTESI_43.to_vec(), "seme 43 ignorato o alterato");

    let mut quarantadue = Rng::seeded();
    let mut quarantatre = Rng::con_seme(43);
    assert_ne!(
        quarantadue.next(),
        quarantatre.next(),
        "i due semi producono la stessa sequenza"
    );
}

// ---------------------------------------------------------------------------
// Le fixture condivise, contro attese scritte a mano
// ---------------------------------------------------------------------------

#[path = "../examples/comune/fixture.rs"]
mod fixture;

use std::sync::Arc;

use fixture::{base_fixture, json_fixture, right_fixture, setop_right_fixture, struct_fixture};
use plenora_core::arrow::array::{
    ArrayRef, Float64Array, Int64Array, RecordBatch, StringArray, StructArray,
};
use plenora_core::arrow::schema::{DataType, Field, Fields, Schema};

/// Confronta due batch su **tutto** cio' che un benchmark puo' misurare.
///
/// Lo schema copre metadata, ordine e nullability dei campi; `to_data()`
/// copre valori, offset e bitmap dei null. `output_rows` di un benchmark non
/// vede niente di tutto questo: due fixture con lo stesso numero di righe e
/// dati diversi lo lascerebbero identico.
fn assert_batch_identico(caso: &str, ottenuto: &RecordBatch, atteso: &RecordBatch) {
    assert_eq!(
        ottenuto.schema(),
        atteso.schema(),
        "{caso}: schema, metadata o ordine delle colonne diversi"
    );
    for (indice, campo) in atteso.schema().fields().iter().enumerate() {
        assert_eq!(
            ottenuto.column(indice).to_data(),
            atteso.column(indice).to_data(),
            "{caso}: colonna `{}` diversa nei dati, negli offset o nei null",
            campo.name()
        );
    }
    assert_eq!(ottenuto, atteso, "{caso}: batch diversi");
}

/// Lo schema a sei colonne con il metadata `source`.
fn schema_base() -> Arc<Schema> {
    Arc::new(Schema::new_with_metadata(
        vec![
            Field::new("id", DataType::Int64, false),
            Field::new("num", DataType::Float64, false),
            Field::new("grp", DataType::Utf8, false),
            Field::new("text", DataType::Utf8, false),
            Field::new("key", DataType::Int64, false),
            Field::new("path", DataType::Utf8, false),
        ],
        std::iter::once(("source".to_owned(), "bench_sweep".to_owned())).collect(),
    ))
}

fn batch_sei_colonne(
    ids: Vec<i64>,
    nums: Vec<f64>,
    grp: Vec<&str>,
    text: Vec<&str>,
    key: Vec<i64>,
    path: Vec<&str>,
) -> RecordBatch {
    RecordBatch::try_new(
        schema_base(),
        vec![
            Arc::new(Int64Array::from(ids)),
            Arc::new(Float64Array::from(nums)),
            Arc::new(StringArray::from(grp)),
            Arc::new(StringArray::from(text)),
            Arc::new(Int64Array::from(key)),
            Arc::new(StringArray::from(path)),
        ],
    )
    .expect("batch atteso")
}

#[test]
fn base_fixture_produce_i_dati_attesi() {
    let atteso = batch_sei_colonne(
        vec![0, 1],
        vec![8056.74, 8035.57],
        vec!["g703", "g415"],
        vec![
            "8b13399cd1d1497a283b88fe5fdff5688b341082",
            "cf0f325839a9f7dc531907be7b3ad33387b40a55",
        ],
        vec![275_427, 33_665],
        vec!["p075/q114/r036", "p011/q472/r117"],
    );
    assert_batch_identico("base_fixture(2)", &base_fixture(2), &atteso);
}

#[test]
fn setop_right_fixture_attraversa_le_due_meta() {
    // Le prime due righe vengono dal seme 43, le ultime due dallo stream del
    // seme 42: il confine sta a `rows / 2`, e con quattro righe lo si vede.
    let atteso = batch_sei_colonne(
        vec![0, 1, 2, 3],
        vec![754.35, 2345.71, 945.91, 7208.34],
        vec!["g766", "g891", "g396", "g850"],
        vec![
            "100dbdb3bf576f53dd6f7dfd0a82754d1b6d82e7",
            "20a256a00e3260551d33a6b1ed16b1d58eb649f6",
            "111bd47c6ac23a97886fba01938ffb22549a6c54",
            "22523dcb284d7af479c68a48b81fdd01d0ce82fb",
        ],
        vec![625_570, 85_861, 870_732, 384_446],
        vec![
            "p094/q455/r187",
            "p076/q045/r476",
            "p195/q229/r405",
            "p216/q042/r252",
        ],
    );
    assert_batch_identico("setop_right_fixture(4)", &setop_right_fixture(4), &atteso);
}

#[test]
fn right_fixture_produce_i_dati_attesi() {
    // La riga 0 e' l'unica con `+ 1.0`: il confine `row % 10 == 0` cade li'.
    let atteso = RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int64, false),
            Field::new("num", DataType::Float64, false),
            Field::new("rval", DataType::Utf8, false),
        ])),
        vec![
            Arc::new(Int64Array::from(vec![0_i64, 1])),
            Arc::new(Float64Array::from(vec![8057.74, 3209.54])),
            Arc::new(StringArray::from(vec![
                "ra00aaafdf80202bf",
                "r283b88fe5fdff568",
            ])),
        ],
    )
    .expect("batch atteso");
    assert_batch_identico("right_fixture(2)", &right_fixture(2), &atteso);
}

#[test]
fn json_fixture_produce_i_dati_attesi() {
    let atteso = RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int64, false),
            Field::new("doc", DataType::Utf8, false),
        ])),
        vec![
            Arc::new(Int64Array::from(vec![0_i64, 1])),
            Arc::new(StringArray::from(vec![
                r#"{"a":674,"b":{"c":471,"d":{"e":"d1d1497a","f":[1,2,3]}},"g":"5fdff568"}"#,
                r#"{"a":162,"b":{"c":427,"d":{"e":"a5b290d3","f":[1,2,3]}},"g":"ec23a132"}"#,
            ])),
        ],
    )
    .expect("batch atteso");
    assert_batch_identico("json_fixture(2)", &json_fixture(2), &atteso);
}

#[test]
fn struct_fixture_produce_i_dati_attesi() {
    // I tre figli sono nullable e nessuno e' null; lo struct non ha bitmap
    // propria. Sono tre proprieta' diverse, e `to_data()` le confronta tutte.
    let fields = Fields::from(vec![
        Field::new("a", DataType::Int64, true),
        Field::new("b", DataType::Float64, true),
        Field::new("c", DataType::Utf8, true),
    ]);
    let structure = StructArray::new(
        fields.clone(),
        vec![
            Arc::new(Int64Array::from(vec![
                45_454_805_674_i64,
                -6_914_526_270_109_646_145,
            ])) as ArrayRef,
            Arc::new(Float64Array::from(vec![954.0, 9736.0])) as ArrayRef,
            Arc::new(StringArray::from(vec![
                "4e915fe38b341082",
                "8c17f2b433701823",
            ])) as ArrayRef,
        ],
        None,
    );
    let atteso = RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int64, false),
            Field::new("payload", DataType::Struct(fields), false),
        ])),
        vec![
            Arc::new(Int64Array::from(vec![0_i64, 1])),
            Arc::new(structure),
        ],
    )
    .expect("batch atteso");
    assert_batch_identico("struct_fixture(2)", &struct_fixture(2), &atteso);
}
