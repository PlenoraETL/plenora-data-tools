#![no_main]

//! Runner geo: righe WKT arbitrarie (una per riga del payload) lette con
//! `geo.from_wkt`, poi un'operazione unaria, collettiva o binaria. Le binarie
//! ricevono due tabelle con le stesse righe: la seconda è la prima ruotata
//! di una riga.
//!
//! Il primo byte sceglie operazione, config e CRS di piano (proiettato o
//! geografico); il resto è il testo WKT.
//!
//! Invarianti: mai panico, mai `Internal` (salvo quello documentato di una
//! dipendenza in barriera: validazione OGC non conclusa), nessun errore che
//! contenga il valore sentinella del corpus, due esecuzioni dello stesso
//! piano danno lo stesso esito (determinismo), e ogni cella di una colonna
//! geometria in uscita si rilegge con il decoder dei kernel: un passo dopo
//! deve poterla usare.

use std::sync::Arc;

use libfuzzer_sys::fuzz_target;
use plenora_core::arrow::array::{Array, BinaryArray, RecordBatch, StringArray};
use plenora_core::arrow::schema::{DataType, Field, Schema, SchemaRef};
use plenora_core::contract::arrow_metadata::{GEOARROW_EXTENSION_KEY, GEOARROW_WKB_EXTENSION};
use plenora_core::{ErrorCategory, Result};
use plenora_pipeline::{Esito, Passo, Pipeline};
use serde_json::{json, Value};

#[path = "comune/aggancio.rs"]
mod aggancio;
#[path = "comune/esiti.rs"]
mod esiti;

/// Righe massime: le booleane e le collettive sono superlineari.
const MAX_RIGHE: usize = 8;
/// Byte massimi del testo WKT.
const MAX_TESTO: usize = 4096;

const CRS: &[&str] = &["EPSG:32632", "OGC:CRS84"];

fn operazioni() -> Vec<(&'static str, Value, bool)> {
    vec![
        ("geo.area", json!({}), false),
        ("geo.length", json!({}), false),
        ("geo.perimeter", json!({}), false),
        ("geo.centroid", json!({}), false),
        ("geo.point_on_surface", json!({}), false),
        ("geo.convex_hull", json!({}), false),
        ("geo.envelope", json!({}), false),
        ("geo.boundary", json!({}), false),
        ("geo.make_valid", json!({}), false),
        ("geo.vertex_count", json!({}), false),
        // `wkt` c'è già nella tabella: l'uscita vuole un nome libero.
        ("geo.to_wkt", json!({"output_column": "wkt_out"}), false),
        ("geo.geometry_diagnostics", json!({}), false),
        ("geo.geodesic_area", json!({}), false),
        ("geo.buffer", json!({"distance": 1.0}), false),
        ("geo.buffer", json!({"distance": -0.5}), false),
        ("geo.buffer", json!({"distance": 0.004}), false),
        ("geo.buffer", json!({"distance": 5.0, "cap": "flat"}), false),
        ("geo.simplify", json!({"tolerance": 1.0}), false),
        (
            "geo.simplify",
            json!({"min_area": 1.0, "policy": "preserve_topology"}),
            false,
        ),
        ("geo.dissolve", json!({}), false),
        ("geo.polygonize", json!({}), false),
        ("geo.intersection", json!({}), true),
        ("geo.union", json!({}), true),
        ("geo.difference", json!({}), true),
        ("geo.symmetric_difference", json!({}), true),
        ("geo.distance", json!({}), true),
        ("geo.predicate_intersects", json!({}), true),
        ("geo.predicate_contains", json!({}), true),
        ("geo.predicate_touches", json!({}), true),
    ]
}

fn testi(righe: &[Option<&str>]) -> RecordBatch {
    RecordBatch::try_new(
        Arc::new(Schema::new(vec![Field::new("wkt", DataType::Utf8, true)])),
        vec![Arc::new(StringArray::from(righe.to_vec()))],
    )
    .expect("tabella WKT")
}

fn lettura(out: &str, ingresso: &str) -> Passo {
    Passo {
        out: out.to_owned(),
        op: "geo.from_wkt".to_owned(),
        inputs: vec![ingresso.to_owned()],
        config: json!({"wkt_column": "wkt"}),
    }
}

fn esegui(
    crs: &str,
    op: &str,
    config: &Value,
    a: &RecordBatch,
    b: Option<&RecordBatch>,
) -> Result<Esito> {
    let mut inputs = vec!["a".to_owned()];
    let mut steps = vec![lettura("ga", "a")];
    let mut tabelle = vec![("a".to_owned(), a.clone())];
    let mut operandi = vec!["ga".to_owned()];
    if let Some(b) = b {
        inputs.push("b".to_owned());
        steps.push(lettura("gb", "b"));
        tabelle.push(("b".to_owned(), b.clone()));
        operandi.push("gb".to_owned());
    }
    steps.push(Passo {
        out: "uscita".to_owned(),
        op: op.to_owned(),
        inputs: operandi,
        config: config.clone(),
    });
    let piano = Pipeline {
        version: 1,
        inputs,
        crs: Some(crs.to_owned()),
        limits: None,
        steps,
        outputs: vec!["uscita".to_owned()],
    };
    let schemi: Vec<(&str, SchemaRef)> = tabelle
        .iter()
        .map(|(nome, tabella)| (nome.as_str(), tabella.schema()))
        .collect();
    piano.validate(&schemi)?.run(tabelle)
}

/// Ogni cella non null di ogni colonna GeoArrow-WKB si rilegge (o la
/// validazione OGC non conclude, per un panico di `geo` in barriera).
fn celle_rileggibili(op: &str, tabella: &RecordBatch, barriere: u64) {
    for (campo, colonna) in tabella.schema().fields().iter().zip(tabella.columns()) {
        if campo
            .metadata()
            .get(GEOARROW_EXTENSION_KEY)
            .map(String::as_str)
            != Some(GEOARROW_WKB_EXTENSION)
        {
            continue;
        }
        let celle = colonna
            .as_any()
            .downcast_ref::<BinaryArray>()
            .expect("colonna geometria Binary");
        for riga in 0..celle.len() {
            if celle.is_valid(riga) {
                if let Err(errore) = plenora_kernels_geo::geometry_from_wkb(celle.value(riga)) {
                    let non_conclusa = errore.category() == ErrorCategory::Internal
                        && aggancio::panici_in_barriera() > barriere;
                    assert!(
                        non_conclusa,
                        "{op}: cella geometria in uscita non rileggibile: {errore}"
                    );
                }
            }
        }
    }
}

fuzz_target!(init: aggancio::installa(), |dati: &[u8]| {
    let barriere = aggancio::panici_in_barriera();
    let Some((&scelta, testo)) = dati.split_first() else {
        return;
    };
    if testo.len() > MAX_TESTO {
        return;
    }
    let Ok(testo) = std::str::from_utf8(testo) else {
        return;
    };
    let operazioni = operazioni();
    let (op, config, binaria) = &operazioni[usize::from(scelta) % operazioni.len()];
    let crs = CRS[usize::from(scelta / 128)];

    // Una riga vuota è una cella null.
    let righe: Vec<Option<&str>> = testo
        .split('\n')
        .take(MAX_RIGHE)
        .map(|riga| (!riga.is_empty()).then_some(riga))
        .collect();
    let a = testi(&righe);
    let b = binaria.then(|| {
        let mut ruotate = righe.clone();
        ruotate.rotate_left(1);
        testi(&ruotate)
    });

    let primo = esegui(crs, op, config, &a, b.as_ref());
    let secondo = esegui(crs, op, config, &a, b.as_ref());
    match (&primo, &secondo) {
        (Ok(primo), Ok(secondo)) => {
            assert_eq!(primo.outputs, secondo.outputs, "{op}");
            assert_eq!(primo.report, secondo.report, "{op}");
            for (_, tabella) in &primo.outputs {
                celle_rileggibili(op, tabella, barriere);
            }
        }
        (Err(primo), Err(secondo)) => {
            esiti::stesso_errore(op, primo, secondo);
            esiti::errore_ammesso(op, primo, barriere);
        }
        _ => panic!("{op}: esecuzione non deterministica"),
    }
});
