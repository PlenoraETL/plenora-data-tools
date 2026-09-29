//! Fixture geo dei test del runner: tabelle con una colonna geometria WKB
//! (`GeoArrow`, CRS nel metadato `geo`), piani di un passo, oracoli sui kernel.

#![allow(dead_code)] // Ogni file di test usa solo una parte delle fixture.

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
    let schema = Arc::new(Schema::new(vec![
        Field::new("id", DataType::Int64, false),
        Field::new("label", DataType::Utf8, true),
        geometry_output_field("geometry", crs).expect("campo geometria"),
    ]));
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
    runner.category() == analisi.category() && runner.to_string().contains(interno)
}
