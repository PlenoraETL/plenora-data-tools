//! Fixture geo dei test del runner: tabelle con una colonna geometria WKB
//! (`GeoArrow`, CRS nel metadato `geo`), piani di un passo, oracoli sui kernel.

#![allow(dead_code)] // Ogni file di test usa solo una parte delle fixture.

mod schema;
#[allow(unused_imports)] // Modulo condiviso da target con sottoinsiemi diversi.
pub use schema::{schema_geo_indipendente, verifica_schema_geo_indipendente};

use std::sync::Arc;

use geo::{Geometry, LineString, MultiPoint, Point, Polygon};
use plenora_core::arrow::array::{
    Array, ArrayRef, BinaryArray, Float64Array, Int64Array, RecordBatch, StringArray,
};
use plenora_core::arrow::schema::{DataType, Field, Schema, SchemaRef};
use plenora_core::contract::arrow_metadata::geometry_output_field;
use plenora_core::contract::arrow_schema::contract_from_arrow_schema;
use plenora_core::contract::{DataContract, FieldAllocator};
use plenora_core::crs::{resolve_crs, ResolvedCrs};
use plenora_core::esadecimale::esadecimale;
use plenora_core::{PlenoraError, Result};
use plenora_kernels_geo::analyze::analyze_geo_contract;
use plenora_kernels_geo::arrow_adapter::encode_geometry;
use plenora_pipeline::{Esito, LimitiParziali, Passo, Pipeline};
use serde_json::Value;

/// CRS proiettato delle fixture: UTM 32N, metri.
pub const UTM: &str = "EPSG:32632";
/// CRS geografico delle fixture lon/lat.
pub const LONLAT: &str = "OGC:CRS84";

/// Origine delle fixture proiettate, dentro il dominio di UTM 32N.
pub const X0: f64 = 500_000.0;
pub const Y0: f64 = 5_000_000.0;

pub fn passo(out: &str, op: &str, inputs: &[&str], config: Value) -> Passo {
    Passo {
        out: out.to_owned(),
        op: op.to_owned(),
        inputs: inputs.iter().map(|nome| (*nome).to_owned()).collect(),
        config,
    }
}

pub fn piano(inputs: &[&str], steps: Vec<Passo>, outputs: &[&str]) -> Pipeline {
    Pipeline {
        version: 1,
        inputs: inputs.iter().map(|nome| (*nome).to_owned()).collect(),
        crs: None,
        limits: None,
        steps,
        outputs: outputs.iter().map(|nome| (*nome).to_owned()).collect(),
    }
}

pub fn wkb(geometria: &Geometry<f64>) -> Vec<u8> {
    encode_geometry(geometria).expect("wkb")
}

pub fn esadecimale_di(geometria: &Geometry<f64>) -> String {
    esadecimale(&wkb(geometria))
}

/// Una tabella `id`, `label`, `geometry` nel CRS dato.
pub fn tabella(crs: &str, geometrie: &[Option<Geometry<f64>>]) -> RecordBatch {
    let righe = geometrie.len();
    let schema = Arc::new(Schema::new_with_metadata(
        vec![
            Field::new("id", DataType::Int64, false),
            Field::new("label", DataType::Utf8, true),
            {
                let field = geometry_output_field("geometry", crs).expect("campo geometria");
                let mut metadata = field.metadata().clone();
                let resolved = plenora_core::crs::resolve_crs(crs, "crs").expect("CRS fixture");
                metadata.insert(
                    "plenora.geometry.axis_order".to_owned(),
                    resolved.normalized_gis_axis_order().as_str().to_owned(),
                );
                field.with_metadata(metadata)
            },
        ],
        std::iter::once(("plenora.contract.version".to_owned(), "1".to_owned()))
            .collect::<plenora_core::arrow::Metadata>(),
    ));
    RecordBatch::try_new(
        schema,
        vec![
            Arc::new(Int64Array::from(
                (0..righe)
                    .map(|riga| i64::try_from(riga).expect("riga"))
                    .collect::<Vec<_>>(),
            )),
            Arc::new(StringArray::from(
                (0..righe)
                    .map(|riga| Some(if riga % 2 == 0 { "pari" } else { "dispari" }))
                    .collect::<Vec<_>>(),
            )),
            Arc::new(
                geometrie
                    .iter()
                    .map(|g| g.as_ref().map(wkb))
                    .collect::<Vec<_>>()
                    .iter()
                    .map(Option::as_deref)
                    .collect::<BinaryArray>(),
            ),
        ],
    )
    .expect("tabella geo")
}

pub fn quadrato(x: f64, y: f64, lato: f64) -> Polygon<f64> {
    Polygon::new(
        LineString::from(vec![
            (x, y),
            (x + lato, y),
            (x + lato, y + lato),
            (x, y + lato),
            (x, y),
        ]),
        vec![],
    )
}

/// Poligoni in UTM 32N, uno null, due sovrapposti.
pub fn poligoni() -> RecordBatch {
    tabella(
        UTM,
        &[
            Some(Geometry::Polygon(quadrato(X0, Y0, 100.0))),
            None,
            Some(Geometry::Polygon(quadrato(X0 + 50.0, Y0 + 50.0, 100.0))),
            Some(Geometry::Polygon(quadrato(X0 + 500.0, Y0, 30.0))),
        ],
    )
}

/// Linee in UTM 32N, una null.
pub fn linee() -> RecordBatch {
    tabella(
        UTM,
        &[
            Some(Geometry::LineString(LineString::from(vec![
                (X0, Y0),
                (X0 + 100.0, Y0 + 7.0),
                (X0 + 200.0, Y0),
            ]))),
            Some(Geometry::LineString(LineString::from(vec![
                (X0 + 200.0, Y0),
                (X0 + 300.0, Y0 + 50.0),
            ]))),
            None,
        ],
    )
}

/// Punti in UTM 32N, uno null.
pub fn punti() -> RecordBatch {
    tabella(
        UTM,
        &[
            Some(Geometry::Point(Point::new(X0, Y0))),
            Some(Geometry::Point(Point::new(X0 + 10.0, Y0 + 3.0))),
            None,
            Some(Geometry::Point(Point::new(X0 + 400.0, Y0 - 20.0))),
            Some(Geometry::Point(Point::new(X0 + 405.0, Y0 - 22.0))),
        ],
    )
}

/// Punti sparsi in un multipunto, per gli inviluppi.
pub fn multipunti() -> RecordBatch {
    tabella(
        UTM,
        &[Some(Geometry::MultiPoint(MultiPoint::from(
            (0..12_u8)
                .map(|i| {
                    (
                        f64::from(i).mul_add(10.0, X0),
                        Y0 + f64::from(i * i % 7) * 10.0,
                    )
                })
                .collect::<Vec<_>>(),
        )))],
    )
}

/// Punti lon/lat (Italia), per le misure geodetiche.
pub fn punti_lonlat() -> RecordBatch {
    tabella(
        LONLAT,
        &[
            Some(Geometry::Point(Point::new(9.19, 45.46))),
            None,
            Some(Geometry::Point(Point::new(12.49, 41.9))),
        ],
    )
}

/// Linee e poligoni lon/lat.
pub fn linee_lonlat() -> RecordBatch {
    tabella(
        LONLAT,
        &[Some(Geometry::LineString(LineString::from(vec![
            (9.0, 45.0),
            (10.0, 45.5),
            (11.0, 45.0),
        ])))],
    )
}

pub fn poligoni_lonlat() -> RecordBatch {
    tabella(LONLAT, &[Some(Geometry::Polygon(quadrato(9.0, 45.0, 0.5)))])
}

/// Valida e esegue un piano sulle tabelle date.
pub fn esegui(pipeline: &Pipeline, tabelle: &[(&str, RecordBatch)]) -> Result<Esito> {
    let schemi: Vec<(&str, SchemaRef)> = tabelle
        .iter()
        .map(|(nome, tabella)| (*nome, tabella.schema()))
        .collect();
    pipeline.validate(&schemi)?.run(
        tabelle
            .iter()
            .map(|(nome, tabella)| ((*nome).to_owned(), tabella.clone()))
            .collect(),
    )
}

/// Un passo `op` su `t` (e `u`, per le binarie), l'uscita `x`.
pub fn un_passo(op: &str, config: Value, tabelle: &[RecordBatch]) -> Result<RecordBatch> {
    let nomi: Vec<&str> = ["t", "u"].into_iter().take(tabelle.len()).collect();
    let pipeline = piano(&nomi, vec![passo("x", op, &nomi, config)], &["x"]);
    let coppie: Vec<(&str, RecordBatch)> =
        nomi.iter().copied().zip(tabelle.iter().cloned()).collect();
    let mut esito = esegui(&pipeline, &coppie)?;
    Ok(esito.outputs.remove(0).1)
}

/// Come [`un_passo`], con il CRS di piano (produttori).
pub fn un_passo_con_crs(
    op: &str,
    config: Value,
    tabella: RecordBatch,
    crs: &str,
) -> Result<RecordBatch> {
    let mut pipeline = piano(&["t"], vec![passo("x", op, &["t"], config)], &["x"]);
    pipeline.crs = Some(crs.to_owned());
    let mut esito = esegui(&pipeline, &[("t", tabella)])?;
    Ok(esito.outputs.remove(0).1)
}

/// Con un budget di memoria dato.
pub fn con_budget(mut pipeline: Pipeline, budget: u64) -> Pipeline {
    pipeline.limits = Some(LimitiParziali {
        max_governed_memory_bytes: Some(budget),
        ..LimitiParziali::default()
    });
    pipeline
}

/// Le geometrie di una colonna WKB, decodificate e validate.
pub fn geometrie(tabella: &RecordBatch, colonna: &str) -> Vec<Option<Geometry<f64>>> {
    let celle = tabella
        .column_by_name(colonna)
        .expect("colonna")
        .as_any()
        .downcast_ref::<BinaryArray>()
        .expect("binaria");
    (0..celle.len())
        .map(|riga| {
            (!celle.is_null(riga)).then(|| {
                plenora_kernels_geo::geometry_from_wkb(celle.value(riga)).expect("geometria")
            })
        })
        .collect()
}

/// La colonna attesa di geometrie, calcolata riga per riga dall'oracolo.
pub fn attese_geo(
    tabella: &RecordBatch,
    f: impl Fn(&Geometry<f64>) -> Option<Geometry<f64>>,
) -> ArrayRef {
    let celle: Vec<Option<Vec<u8>>> = geometrie(tabella, "geometry")
        .iter()
        .map(|g| g.as_ref().and_then(&f).map(|g| wkb(&g)))
        .collect();
    Arc::new(celle.iter().map(Option::as_deref).collect::<BinaryArray>())
}

pub fn attesi_f64(tabella: &RecordBatch, f: impl Fn(&Geometry<f64>) -> Option<f64>) -> ArrayRef {
    Arc::new(Float64Array::from(
        geometrie(tabella, "geometry")
            .iter()
            .map(|g| g.as_ref().and_then(&f))
            .collect::<Vec<_>>(),
    ))
}

/// Il contratto d'ingresso come lo legge la validazione.
pub fn contratto(tabella: &RecordBatch) -> DataContract {
    contract_from_arrow_schema(tabella.schema(), resolve_crs).expect("contratto")
}

/// L'analisi diretta dei kernel su quegli ingressi.
pub fn analisi(
    op: &str,
    tabelle: &[RecordBatch],
    config: &Value,
    crs_piano: Option<&str>,
) -> Result<DataContract> {
    let ingressi: Vec<DataContract> = tabelle.iter().map(contratto).collect();
    let piano: Option<ResolvedCrs> = crs_piano.map(|crs| resolve_crs(crs, "crs").expect("crs"));
    analyze_geo_contract(
        op,
        &ingressi,
        config,
        piano.as_ref(),
        &mut FieldAllocator::new(1000),
    )
}

/// La categoria e il messaggio di due errori coincidono (il runner aggiunge
/// il nome del passo in testa al messaggio).
pub fn stesso_errore(runner: &PlenoraError, analisi: &PlenoraError) -> bool {
    let testo = analisi.to_string();
    let interno = testo
        .split_once(": ")
        .map_or(testo.as_str(), |(_, resto)| resto);
    runner.category() == analisi.category()
        && runner.phase() == plenora_core::ErrorPhase::Validate
        && analisi.phase() == plenora_core::ErrorPhase::Validate
        && runner.to_string().contains(interno)
}

/// Per ogni caso la validazione del runner rifiuta esattamente cio' che
/// l'analisi rifiuta, con lo stesso errore, e accetta con lo stesso
/// contratto (nello schema emesso).
pub fn stessa_validazione(
    casi: &[(&str, Value, Vec<RecordBatch>, Option<&str>)],
    esiti: &[Option<plenora_core::ErrorCategory>],
) {
    assert_eq!(casi.len(), esiti.len());
    for ((op, config, tabelle, crs), categoria) in casi.iter().zip(esiti) {
        let atteso = analisi(op, tabelle, config, *crs);
        let nomi: Vec<&str> = ["t", "u"].into_iter().take(tabelle.len()).collect();
        let mut pipeline = piano(&nomi, vec![passo("x", op, &nomi, config.clone())], &["x"]);
        pipeline.crs = crs.map(str::to_owned);
        let schemi: Vec<(&str, SchemaRef)> = nomi
            .iter()
            .copied()
            .zip(tabelle.iter().map(RecordBatch::schema))
            .collect();
        let validata = pipeline.validate(&schemi);
        if let Some(categoria) = categoria {
            let runner = validata.expect_err("scenario rifiutato");
            let analisi = atteso.expect_err("scenario rifiutato anche direttamente");
            assert_eq!(runner.category(), *categoria, "{op} {config}");
            assert_eq!(analisi.category(), *categoria, "{op} {config}");
            assert!(stesso_errore(&runner, &analisi));
            continue;
        }
        assert!(validata.is_ok(), "scenario valido: {op} {config}");
        assert!(atteso.is_ok(), "scenario valido: {op} {config}");
        match (&validata, &atteso) {
            (Ok(validata), Ok(contratto)) => {
                assert_eq!(
                    validata.schema_uscita("x").expect("schema pubblicato"),
                    &schema_geo_indipendente(op, &tabelle[0], config, *crs),
                    "schema indipendente {op}"
                );
                let ottenuto = validata.contratto("x").expect("contratto del passo");
                let emesso =
                    plenora_core::contract::arrow_schema::arrow_schema_from_contract(contratto)
                        .expect("schema emesso");
                assert_eq!(ottenuto.schema, emesso, "{op} {config}");
            }
            (Err(runner), Err(analisi)) => {
                assert!(
                    stesso_errore(runner, analisi),
                    "{op} {config}: {runner} / {analisi}"
                );
            }
            _ => panic!(
                "{op} {config}: validazione {:?}, analisi {:?}",
                validata.as_ref().err(),
                atteso.as_ref().err()
            ),
        }
    }
}

/// Oracolo di forma per poligoni: confronta tutte le coordinate, anelli e
/// componenti, ignorando soltanto l'origine/orientamento degli anelli e
/// l'ordine delle componenti. Nessuna operazione geometrica di produzione.
pub fn assert_poligoni(ottenuta: &Geometry<f64>, attesa: &Geometry<f64>) {
    fn anello(linea: &LineString<f64>) -> Vec<(f64, f64)> {
        let mut punti: Vec<_> = linea.0.iter().map(|c| (c.x, c.y)).collect();
        assert_eq!(punti.first(), punti.last());
        punti.pop();
        let minimo = (0..punti.len())
            .min_by(|&a, &b| {
                punti[a]
                    .0
                    .total_cmp(&punti[b].0)
                    .then(punti[a].1.total_cmp(&punti[b].1))
            })
            .expect("anello");
        punti.rotate_left(minimo);
        let mut inverso = punti.clone();
        inverso[1..].reverse();
        if inverso < punti {
            inverso
        } else {
            punti
        }
    }
    fn forma(g: &Geometry<f64>) -> Vec<Vec<Vec<(f64, f64)>>> {
        let poligoni = match g {
            Geometry::Polygon(p) => vec![p],
            Geometry::MultiPolygon(p) => p.0.iter().collect(),
            _ => panic!("attesa geometria poligonale"),
        };
        let mut parti: Vec<_> = poligoni
            .into_iter()
            .map(|p| {
                let mut buchi: Vec<_> = p.interiors().iter().map(anello).collect();
                buchi.sort_by(|a, b| a.partial_cmp(b).expect("coordinate finite"));
                let mut anelli = vec![anello(p.exterior())];
                anelli.extend(buchi);
                anelli
            })
            .collect();
        parti.sort_by(|a, b| a.partial_cmp(b).expect("coordinate finite"));
        parti
    }
    let (ottenuta, attesa) = (forma(ottenuta), forma(attesa));
    assert_eq!(ottenuta.len(), attesa.len(), "componenti");
    for (a, b) in ottenuta.iter().zip(&attesa) {
        assert_eq!(a.len(), b.len(), "anelli");
        for (a, b) in a.iter().zip(b) {
            assert_eq!(a.len(), b.len(), "vertici");
            for (a, b) in a.iter().zip(b) {
                assert!(
                    (a.0 - b.0).hypot(a.1 - b.1) <= 0.01,
                    "coordinata oltre 1 cm"
                );
            }
        }
    }
}

/// Certifica una partizione del poligono convesso della fixture: ogni
/// pezzo e' convesso, senza buchi, e la somma orientata dei bordi e' il
/// bordo originale. I lati interni si cancellano anche se frazionati.
/// L'unico aggancio ammesso tra vertici/lati e' entro il centimetro.
#[allow(clippy::suboptimal_flops)] // Determinanti convenzionali, indipendenti dai kernel.
pub fn assert_partizione(originale: &Polygon<f64>, parti: &[Polygon<f64>]) {
    type P = (f64, f64);
    let mut nodi: Vec<P> = Vec::new();
    let mut anelli: Vec<Vec<usize>> = Vec::new();
    for poligono in std::iter::once(originale).chain(parti) {
        assert!(poligono.interiors().is_empty());
        let raw: Vec<P> = poligono
            .exterior()
            .0
            .iter()
            .map(|p| (p.x - X0, p.y - Y0))
            .collect();
        assert_eq!(raw.first(), raw.last());
        let area2: f64 = raw
            .windows(2)
            .map(|w| w[0].0 * w[1].1 - w[0].1 * w[1].0)
            .sum();
        assert!(area2.abs() > 0.0);
        let mut punti = raw[..raw.len() - 1].to_vec();
        if area2 < 0.0 {
            punti.reverse();
        }
        for i in 0..punti.len() {
            let (a, b, c) = (
                punti[i],
                punti[(i + 1) % punti.len()],
                punti[(i + 2) % punti.len()],
            );
            assert!(
                (b.0 - a.0) * (c.1 - b.1) - (b.1 - a.1) * (c.0 - b.0) >= -1e-6,
                "pezzo non convesso"
            );
        }
        for i in 0..punti.len() {
            let (a, b) = (punti[i], punti[(i + 1) % punti.len()]);
            for (j, c) in punti.iter().enumerate() {
                if j != i {
                    assert!((a.0 - c.0).hypot(a.1 - c.1) > 1e-9, "vertice ripetuto");
                }
                assert!(
                    (b.0 - a.0) * (c.1 - a.1) - (b.1 - a.1) * (c.0 - a.0) >= -1e-6,
                    "vertice fuori dal semipiano interno"
                );
            }
        }
        let indici = punti
            .into_iter()
            .map(|p| {
                nodi.iter()
                    .position(|q| (p.0 - q.0).hypot(p.1 - q.1) <= 0.01)
                    .unwrap_or_else(|| {
                        nodi.push(p);
                        nodi.len() - 1
                    })
            })
            .collect();
        anelli.push(indici);
    }
    let mut bordo = std::collections::BTreeMap::<(usize, usize), i32>::new();
    for (riga, anello) in anelli.iter().enumerate() {
        let segno = if riga == 0 { -1 } else { 1 };
        for i in 0..anello.len() {
            let (a, b) = (nodi[anello[i]], nodi[anello[(i + 1) % anello.len()]]);
            let (dx, dy) = (b.0 - a.0, b.1 - a.1);
            let lunghezza2 = dx * dx + dy * dy;
            assert!(lunghezza2 > 0.0);
            let mut sulla_retta: Vec<_> = nodi
                .iter()
                .enumerate()
                .filter_map(|(j, p)| {
                    let t = ((p.0 - a.0) * dx + (p.1 - a.1) * dy) / lunghezza2;
                    let distanza = ((p.0 - a.0) * dy - (p.1 - a.1) * dx).abs() / lunghezza2.sqrt();
                    ((-1e-10..=1.0 + 1e-10).contains(&t) && distanza <= 0.01).then_some((t, j))
                })
                .collect();
            sulla_retta.sort_by(|a, b| a.0.total_cmp(&b.0));
            for w in sulla_retta.windows(2) {
                let (a, b) = (w[0].1, w[1].1);
                let (chiave, verso) = if a < b { ((a, b), 1) } else { ((b, a), -1) };
                *bordo.entry(chiave).or_default() += segno * verso;
            }
        }
    }
    assert!(
        bordo.values().all(|&n| n == 0),
        "parti mancanti, duplicate o sovrapposte"
    );
}

/// Colonne passanti selezionate secondo indici dichiarati dalla fixture.
pub fn assert_attributi(
    output: &RecordBatch,
    input: &RecordBatch,
    righe: &[u32],
    con_geometria: bool,
) {
    let indices = plenora_core::arrow::array::UInt32Array::from(righe.to_vec());
    for (field, array) in input.schema().fields().iter().zip(input.columns()) {
        if !con_geometria && field.name() == "geometry" {
            continue;
        }
        let attesa = plenora_core::arrow::select::take::take(array.as_ref(), &indices, None)
            .expect("righe fixture");
        assert_eq!(
            output.column_by_name(field.name()).expect("campo passante"),
            &attesa,
            "valori passanti"
        );
    }
}
