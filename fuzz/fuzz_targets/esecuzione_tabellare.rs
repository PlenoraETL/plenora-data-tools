#![no_main]

//! Runner tabellare: un'operazione del catalogo (le config rappresentative di
//! `crates/plenora-pipeline/tests/comune`) su tabelle con gli schemi delle
//! fixture dei test e i valori generati dal payload: estremi interi, `NaN`,
//! zeri con segno, date impossibili, JSON malformato, testo non ASCII.
//!
//! Due modalità, scelte dal payload:
//! - **config del catalogo**: il runner e la chiamata diretta del kernel
//!   concordano sull'esito; se riescono, righe, nomi, tipi e valori sono
//!   uguali (tranne i valori di `table.uuid_generator`, casuali per
//!   contratto);
//! - **config dal payload**: il runner valida più del kernel, quindi può
//!   rifiutare ciò che il kernel accetta; se il runner riesce, il kernel
//!   riesce con lo stesso risultato.
//!
//! In entrambe: mai panico, mai `Internal` (salvo quello documentato di una
//! dipendenza in barriera), due esecuzioni dello stesso piano danno lo
//! stesso esito su ogni asse (diagnostica per riga compresa), e nessun
//! errore contiene il valore sentinella che le celle di testo portano.
//!
//! Il confronto con il kernel non prova l'ordinamento: runner e chiamata
//! diretta usano lo stesso `sort`. Lo prova il target `ordinamento`, con un
//! oracolo indipendente.

use std::sync::Arc;

use libfuzzer_sys::fuzz_target;
use plenora_core::arrow::array::builder::{Int64Builder, ListBuilder, StringBuilder};
use plenora_core::arrow::array::{
    Array, ArrayRef, BinaryArray, BooleanArray, Float64Array, Int64Array, RecordBatch, StringArray,
    StructArray,
};
use plenora_core::arrow::schema::{DataType, Field, Fields, Schema, SchemaRef};
use plenora_core::Result;
use plenora_pipeline::{Esito, Passo, Pipeline};
use serde_json::Value;

#[path = "comune/aggancio.rs"]
mod aggancio;
#[path = "../../crates/plenora-pipeline/tests/comune/mod.rs"]
mod comune;
#[path = "comune/esiti.rs"]
mod esiti;

use comune::{chiamata_diretta, nomi_input, Fixture, CASI, CHIAVE_HMAC};

/// Righe massime di una tabella generata.
const MAX_RIGHE: usize = 24;

/// Legge il payload un byte alla volta; finito il payload, restituisce zeri.
struct Byte<'a> {
    dati: &'a [u8],
}

impl Byte<'_> {
    fn prossimo(&mut self) -> u8 {
        match self.dati.split_first() {
            Some((primo, resto)) => {
                self.dati = resto;
                *primo
            }
            None => 0,
        }
    }

    fn scegli<T: Copy>(&mut self, valori: &[T]) -> T {
        valori[usize::from(self.prossimo()) % valori.len()]
    }
}

const INTERI: &[i64] = &[0, 1, 2, 3, -1, 7, 42, i64::MAX, i64::MIN, i64::MAX - 1];
const NUMERI: &[f64] = &[
    0.5,
    1.0,
    -0.0,
    0.0,
    2.5,
    -3.25,
    f64::NAN,
    f64::INFINITY,
    f64::NEG_INFINITY,
    f64::MAX,
    f64::MIN_POSITIVE,
    1e-320,
    9_007_199_254_740_993.0,
];
const TESTI: &[&str] = &[
    "a",
    "b",
    "",
    "A",
    " a ",
    "é",
    "e\u{301}",
    "ß",
    "a,b",
    "日本",
    esiti::SENTINELLA,
];
const DATE: &[&str] = &[
    "2024-01-02",
    "2024-01-03",
    "2024-02-29",
    "2023-02-29",
    "1970-01-01",
    "9999-12-31",
    "0000-01-01",
    "2024-01-02T10:00:00",
    "2024-01-02 10:00:00+02:00",
    "",
    "x",
];
const JSON: &[&str] = &[
    "{\"a\":1}",
    "{\"a\":{\"b\":[1,2]}}",
    "{\"a\":1e400}",
    "{\"a\":\"x\",\"a\":2}",
    "[]",
    "null",
    "{",
    "{\"a\":null}",
    "{\"\":1}",
];

fn righe(byte: &mut Byte<'_>) -> usize {
    usize::from(byte.prossimo()) % (MAX_RIGHE + 1)
}

/// Stesso schema di `comune::wide`, valori dal payload.
fn wide(byte: &mut Byte<'_>, con_null: bool) -> RecordBatch {
    let n = righe(byte);
    let mut id = Vec::with_capacity(n);
    let mut name = Vec::with_capacity(n);
    let mut value = Vec::with_capacity(n);
    let mut flag = Vec::with_capacity(n);
    let mut date = Vec::with_capacity(n);
    let mut date2 = Vec::with_capacity(n);
    let mut json = Vec::with_capacity(n);
    let mut geom: Vec<Vec<u8>> = Vec::with_capacity(n);
    for _ in 0..n {
        id.push(byte.scegli(INTERI));
        let testo = byte.scegli(TESTI);
        name.push((!con_null || byte.prossimo() % 4 != 0).then_some(testo));
        value.push(byte.scegli(NUMERI));
        flag.push(byte.prossimo() % 2 == 0);
        date.push(byte.scegli(DATE));
        date2.push(byte.scegli(DATE));
        json.push(byte.scegli(JSON));
        let lunghezza = usize::from(byte.prossimo() % 8);
        geom.push((0..lunghezza).map(|_| byte.prossimo()).collect());
    }
    let metadati = plenora_core::arrow::Metadata::from([("origine", "test")]);
    RecordBatch::try_new(
        Arc::new(Schema::new_with_metadata(
            vec![
                Field::new("id", DataType::Int64, false),
                Field::new("name", DataType::Utf8, con_null),
                Field::new("value", DataType::Float64, false),
                Field::new("flag", DataType::Boolean, false),
                Field::new("date", DataType::Utf8, false),
                Field::new("date2", DataType::Utf8, false),
                Field::new("json", DataType::Utf8, false),
                Field::new("geom", DataType::Binary, false),
            ],
            metadati,
        )),
        vec![
            Arc::new(Int64Array::from(id)),
            Arc::new(StringArray::from(name)),
            Arc::new(Float64Array::from(value)),
            Arc::new(BooleanArray::from(flag)),
            Arc::new(StringArray::from(date)),
            Arc::new(StringArray::from(date2)),
            Arc::new(StringArray::from(json)),
            Arc::new(BinaryArray::from_iter_values(geom)),
        ],
    )
    .expect("tabella wide")
}

/// Stesso schema di `comune::destra`.
fn destra(byte: &mut Byte<'_>) -> RecordBatch {
    let n = righe(byte);
    let mut rid = Vec::with_capacity(n);
    let mut rname = Vec::with_capacity(n);
    let mut rvalue = Vec::with_capacity(n);
    for _ in 0..n {
        rid.push(byte.scegli(INTERI));
        rname.push(byte.scegli(TESTI));
        rvalue.push(byte.scegli(NUMERI));
    }
    RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("rid", DataType::Int64, false),
            Field::new("rname", DataType::Utf8, false),
            Field::new("rvalue", DataType::Float64, false),
        ])),
        vec![
            Arc::new(Int64Array::from(rid)),
            Arc::new(StringArray::from(rname)),
            Arc::new(Float64Array::from(rvalue)),
        ],
    )
    .expect("tabella destra")
}

/// Stesso schema di `comune::nested`.
fn nested(byte: &mut Byte<'_>) -> RecordBatch {
    let n = righe(byte);
    let mut id = Vec::with_capacity(n);
    let mut liste = ListBuilder::new(Int64Builder::new());
    let mut nested_id = Vec::with_capacity(n);
    let mut testi = StringBuilder::new();
    for _ in 0..n {
        id.push(byte.scegli(INTERI));
        for _ in 0..byte.prossimo() % 4 {
            liste.values().append_value(byte.scegli(INTERI));
        }
        liste.append(true);
        nested_id.push(byte.scegli(INTERI));
        testi.append_value(byte.scegli(TESTI));
    }
    let colonna_lista: ArrayRef = Arc::new(liste.finish());
    let struttura: ArrayRef = Arc::new(StructArray::new(
        Fields::from(vec![
            Field::new("nested_id", DataType::Int64, false),
            Field::new("nested_name", DataType::Utf8, false),
        ]),
        vec![
            Arc::new(Int64Array::from(nested_id)),
            Arc::new(testi.finish()),
        ],
        None,
    ));
    RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int64, false),
            Field::new("lst", colonna_lista.data_type().clone(), false),
            Field::new("st", struttura.data_type().clone(), false),
        ])),
        vec![Arc::new(Int64Array::from(id)), colonna_lista, struttura],
    )
    .expect("tabella nested")
}

fn tabelle(fixture: Fixture, byte: &mut Byte<'_>) -> Vec<RecordBatch> {
    match fixture {
        Fixture::Wide => vec![wide(byte, false)],
        Fixture::Nullable => vec![wide(byte, true)],
        Fixture::Nested => vec![nested(byte)],
        Fixture::Binary => vec![wide(byte, false), destra(byte)],
        Fixture::Set => vec![wide(byte, false), wide(byte, false)],
    }
}

fn esegui(op: &str, config: &Value, tavole: &[RecordBatch]) -> Result<Esito> {
    let nomi = nomi_input(tavole.len());
    let piano = Pipeline {
        version: 1,
        inputs: nomi.clone(),
        crs: None,
        limits: None,
        steps: vec![Passo {
            out: "uscita".to_owned(),
            op: op.to_owned(),
            inputs: nomi.clone(),
            config: config.clone(),
        }],
        outputs: vec!["uscita".to_owned()],
    };
    let schemi: Vec<(&str, SchemaRef)> = nomi
        .iter()
        .zip(tavole)
        .map(|(nome, tavola)| (nome.as_str(), tavola.schema()))
        .collect();
    piano
        .validate(&schemi)?
        .run(nomi.iter().cloned().zip(tavole.iter().cloned()).collect())
}

fn uscita(esito: &Esito) -> &RecordBatch {
    let [(nome, tabella)] = esito.outputs.as_slice() else {
        panic!("un'uscita per un piano di un passo");
    };
    assert_eq!(nome, "uscita");
    tabella
}

fn nomi_e_tipi(tabella: &RecordBatch) -> Vec<(String, DataType)> {
    tabella
        .schema()
        .fields()
        .iter()
        .map(|campo| (campo.name().clone(), campo.data_type().clone()))
        .collect()
}

/// La colonna casuale per contratto: l'uscita di `table.uuid_generator`.
fn colonna_casuale<'a>(op: &str, config: &'a Value) -> Option<&'a str> {
    (op == "table.uuid_generator").then(|| {
        config
            .get("output_column")
            .and_then(Value::as_str)
            .unwrap_or("uuid")
    })
}

/// Un UUID v4 in forma canonica minuscola.
fn uuid_v4(testo: &str) -> bool {
    let byte = testo.as_bytes();
    byte.len() == 36
        && byte.iter().enumerate().all(|(i, b)| match i {
            8 | 13 | 18 | 23 => *b == b'-',
            _ => b.is_ascii_digit() || (b'a'..=b'f').contains(b),
        })
        && byte[14] == b'4'
        && matches!(byte[19], b'8' | b'9' | b'a' | b'b')
}

/// Stesso risultato, campo per campo; la colonna casuale si controlla solo
/// nella forma (non null, UUID v4).
fn uguali(op: &str, casuale: Option<&str>, a: &RecordBatch, b: &RecordBatch) {
    assert_eq!(a.num_rows(), b.num_rows(), "{op}");
    assert_eq!(nomi_e_tipi(a), nomi_e_tipi(b), "{op}");
    for (indice, campo) in a.schema().fields().iter().enumerate() {
        if Some(campo.name().as_str()) != casuale {
            assert_eq!(a.column(indice), b.column(indice), "{op}: {}", campo.name());
            continue;
        }
        for tabella in [a, b] {
            let valori = tabella
                .column(indice)
                .as_any()
                .downcast_ref::<StringArray>()
                .expect("uuid Utf8");
            assert_eq!(valori.null_count(), 0, "{op}: uuid null");
            assert!(valori.iter().flatten().all(uuid_v4), "{op}: uuid non v4");
        }
    }
}

fuzz_target!(
    init: {
        aggancio::installa();
        std::env::set_var(CHIAVE_HMAC, "chiave-del-fuzz");
    },
    |dati: &[u8]| {
        let barriere = aggancio::panici_in_barriera();
        let mut byte = Byte { dati };
        let caso = &CASI[usize::from(byte.prossimo()) % CASI.len()];
        if caso.op == "table.transpose" {
            // Rifiutata in validazione per contratto: lo schema d'uscita
            // dipende dai dati (anche il test del runner la salta).
            return;
        }
        let config_dal_payload = byte.prossimo() % 4 == 0;
        let config: Value = if config_dal_payload {
            // Config fino al primo byte zero, dati dopo.
            let fine = byte.dati.iter().position(|b| *b == 0).unwrap_or(byte.dati.len());
            let (testo, resto) = byte.dati.split_at(fine);
            byte.dati = resto.get(1..).unwrap_or_default();
            let Ok(config) = serde_json::from_slice(testo) else {
                return;
            };
            config
        } else {
            serde_json::from_str(caso.config).expect("config del catalogo")
        };
        let tavole = tabelle(caso.fixture, &mut byte);
        let casuale = colonna_casuale(caso.op, &config);
        let op = caso.op;

        let runner = esegui(caso.op, &config, &tavole);
        let di_nuovo = esegui(caso.op, &config, &tavole);
        match (&runner, &di_nuovo) {
            (Ok(a), Ok(b)) => {
                uguali(op, casuale, uscita(a), uscita(b));
                assert_eq!(a.report, b.report, "{op}");
            }
            (Err(a), Err(b)) => esiti::stesso_errore(op, a, b),
            _ => panic!("{op}: esecuzione non deterministica"),
        }

        let diretta = chiamata_diretta(caso.op, &config, &tavole);
        match (&runner, &diretta) {
            (Ok(esito), Ok(diretta)) => uguali(op, casuale, uscita(esito), diretta),
            (Ok(_), Err(errore)) => panic!("{op}: il runner riesce, il kernel no: {errore}"),
            (Err(errore), Ok(_)) => {
                esiti::errore_ammesso(op, errore, barriere);
                assert!(
                    config_dal_payload,
                    "{op}: il kernel riesce, il runner no: {errore}"
                );
            }
            (Err(runner), Err(diretta)) => {
                esiti::errore_ammesso(op, runner, barriere);
                esiti::errore_ammesso(op, diretta, barriere);
            }
        }
    }
);
