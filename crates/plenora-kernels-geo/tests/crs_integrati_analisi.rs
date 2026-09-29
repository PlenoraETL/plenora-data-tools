//! CRS integrati attraverso l'analisi dei contratti geo.
//!
//! Prima della tabella integrata un'operazione con requisito CRS girava solo
//! con un CRS pre-risolto dal chiamante. Qui il CRS arriva come testo
//! (`EPSG:32632` nelle chiavi canoniche dello schema o nella config di un
//! produttore), si risolve dalla tabella e l'analisi arriva in fondo; il
//! dominio di validita' si applica alle geometrie che la config porta.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::Arc;

use geo::{Geometry, Point};
use geozero::{CoordDimensions, ToWkb};
use plenora_core::arrow::schema::{DataType, Field, Schema, SchemaRef};
use plenora_core::contract::arrow_metadata::{
    PLENORA_CONTRACT_VERSION_KEY, PLENORA_GEOMETRY_AXIS_ORDER_KEY, PLENORA_GEOMETRY_CRS_ID_KEY,
    PLENORA_GEOMETRY_CRS_RESOLUTION_KEY, PLENORA_GEOMETRY_DIMENSIONS_KEY,
    PLENORA_GEOMETRY_ENCODING_KEY,
};
use plenora_core::contract::arrow_schema::contract_from_arrow_schema;
use plenora_core::contract::{ContractCrs, DataContract, FieldAllocator};
use plenora_core::crs::resolve_crs;
use plenora_core::PlenoraError;
use plenora_kernels_geo::analyze::analyze_geo_contract;
use serde_json::{json, Value};

/// Schema con una colonna geometria canonica nel CRS dato, con l'ordine
/// degli assi dell'autorita'.
fn schema(crs_id: &str) -> SchemaRef {
    let assi = resolve_crs(crs_id, "crs")
        .expect("CRS integrato")
        .authority_axis_order()
        .expect("assi dell'autorita'")
        .to_string();
    let metadata = HashMap::from([
        (PLENORA_GEOMETRY_ENCODING_KEY.to_owned(), "wkb".to_owned()),
        (PLENORA_GEOMETRY_DIMENSIONS_KEY.to_owned(), "xy".to_owned()),
        (
            PLENORA_GEOMETRY_CRS_RESOLUTION_KEY.to_owned(),
            "resolved".to_owned(),
        ),
        (PLENORA_GEOMETRY_CRS_ID_KEY.to_owned(), crs_id.to_owned()),
        (PLENORA_GEOMETRY_AXIS_ORDER_KEY.to_owned(), assi),
    ]);
    Arc::new(Schema::new_with_metadata(
        vec![
            Field::new("id", DataType::Int64, false),
            Field::new("geometry", DataType::Binary, true).with_metadata(metadata),
        ],
        HashMap::from([(PLENORA_CONTRACT_VERSION_KEY.to_owned(), "1".to_owned())]),
    ))
}

fn input(crs_id: &str) -> DataContract {
    contract_from_arrow_schema(schema(crs_id), resolve_crs).expect("scoperta del contratto")
}

fn analizza(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
) -> Result<DataContract, PlenoraError> {
    analyze_geo_contract(op, inputs, config, None, &mut FieldAllocator::new(100))
}

fn punto_hex(x: f64, y: f64) -> String {
    Geometry::Point(Point::new(x, y))
        .to_wkb(CoordDimensions::xy())
        .expect("encode punto")
        .iter()
        .fold(String::new(), |mut testo, byte| {
            write!(testo, "{byte:02x}").expect("scrittura su String");
            testo
        })
}

fn crs_di(contract: &DataContract) -> Option<(&str, u32)> {
    match &contract.geometries.first()?.crs {
        ContractCrs::Resolved(crs) | ContractCrs::ResolvedByDecision(crs) => {
            crs.authority_identifier()
        }
        _ => None,
    }
}

#[test]
fn operazioni_con_requisito_proiettato_girano_con_epsg_32632() {
    let utm = input("EPSG:32632");
    assert_eq!(crs_di(&utm), Some(("EPSG", 32632)));
    for (op, config) in [
        ("geo.buffer", json!({"distance": 10.0})),
        ("geo.area", json!({})),
        ("geo.length", json!({})),
        ("geo.simplify", json!({"tolerance": 0.5})),
    ] {
        let output = analizza(op, std::slice::from_ref(&utm), &config)
            .unwrap_or_else(|errore| panic!("{op}: {errore}"));
        assert_eq!(crs_di(&output), Some(("EPSG", 32632)), "{op}");
    }
    // Un CRS geografico della tabella non soddisfa il requisito proiettato:
    // errore esplicito, non un'area in gradi quadrati.
    let wgs84 = input("EPSG:4326");
    let errore = analizza("geo.area", &[wgs84], &json!({})).expect_err("area in 4326");
    assert!(
        errore.to_string().contains("PROJECTED_CRS_REQUIRED"),
        "{errore}"
    );
}

#[test]
fn stesso_crs_in_forme_diverse_soddisfa_same_projected() {
    // Le due forme dello stesso codice sono semanticamente uguali: il join
    // spaziale (requisito `SameProjected`) le accetta; codici diversi no.
    let sinistra = input("EPSG:32632");
    let destra = input("urn:ogc:def:crs:EPSG::32632");
    analizza(
        "geo.sjoin",
        &[sinistra.clone(), destra],
        &json!({"predicate": "intersects"}),
    )
    .expect("stesso CRS in forme diverse");
    let errore = analizza(
        "geo.sjoin",
        &[sinistra, input("EPSG:32633")],
        &json!({"predicate": "intersects"}),
    )
    .expect_err("CRS diversi");
    assert!(errore.to_string().contains("CRS_MISMATCH"), "{errore}");
}

#[test]
fn geometria_da_config_nel_dominio_del_crs_dell_input() {
    let utm = input("EPSG:32632");
    // Otranto (18.52 E, 40.10 N) in UTM 32N, fuori dall'area d'uso EPSG del
    // fuso ma dentro il dominio: caso ISTAT, accettato.
    let dentro = json!({"other_wkb": punto_hex(1_312_068.675, 4_482_531.752)});
    analizza("geo.distance", std::slice::from_ref(&utm), &dentro).expect("dentro il dominio");
    // Easting di 5000 km: fuori dal dominio del fuso.
    let fuori = json!({"other_wkb": punto_hex(5_000_000.0, 4_482_531.752)});
    let errore = analizza("geo.distance", std::slice::from_ref(&utm), &fuori)
        .expect_err("fuori dal dominio");
    let testo = errore.to_string();
    assert!(testo.contains("COORDINATE_OUT_OF_CRS_DOMAIN"), "{testo}");
    assert!(testo.contains("other_wkb"), "{testo}");
    assert!(
        !testo.contains("5000000"),
        "coordinata nel messaggio: {testo}"
    );
    // Metri UTM dati come secondo operando a un input in gradi.
    // `haversine_distance` richiede un CRS geografico.
    let errore = analizza("geo.haversine_distance", &[input("OGC:CRS84")], &dentro)
        .expect_err("metri in un CRS geografico");
    assert!(
        errore.to_string().contains("COORDINATE_OUT_OF_CRS_DOMAIN"),
        "{errore}"
    );
    let gradi = json!({"other_wkb": punto_hex(18.52, 40.10)});
    analizza("geo.haversine_distance", &[input("OGC:CRS84")], &gradi).expect("gradi in CRS84");
    // Stessa regola per `snap` e `line_locate_point`.
    let snap = json!({"reference_wkb": punto_hex(5_000_000.0, 0.0), "tolerance": 1.0});
    assert!(matches!(
        analizza("geo.snap", std::slice::from_ref(&utm), &snap),
        Err(PlenoraError::Crs(_))
    ));
    let locate = json!({"point_wkb": punto_hex(500_000.0, -1.0)});
    assert!(matches!(
        analizza("geo.line_locate_point", std::slice::from_ref(&utm), &locate),
        Err(PlenoraError::Crs(_))
    ));
}

/// La lama di `geo.split` arriva dalla config (`other_wkb`) nel CRS
/// dell'input, come il secondo operando di distanze e predicati: stessa
/// regola del dominio. Prima dell'integrazione ne controllava solo la
/// struttura.
#[test]
fn lama_dello_split_nel_dominio_del_crs_dell_input() {
    let utm = input("EPSG:32632");
    let dentro = json!({"other_wkb": punto_hex(500_000.0, 4_482_531.752)});
    analizza("geo.split", std::slice::from_ref(&utm), &dentro).expect("dentro il dominio");
    let fuori = json!({"other_wkb": punto_hex(5_000_000.0, 4_482_531.752)});
    let errore =
        analizza("geo.split", std::slice::from_ref(&utm), &fuori).expect_err("fuori dal dominio");
    let testo = errore.to_string();
    assert!(testo.contains("COORDINATE_OUT_OF_CRS_DOMAIN"), "{testo}");
    assert!(testo.contains("other_wkb"), "{testo}");
    assert!(
        !testo.contains("5000000"),
        "coordinata nel messaggio: {testo}"
    );
}

#[test]
fn produttore_con_crs_da_config() {
    let trigger = DataContract::tabular(Arc::new(Schema::new(vec![Field::new(
        "id",
        DataType::Int64,
        false,
    )])));
    let griglia = |crs: &str, estensione: [f64; 4]| {
        json!({
            "extent": {
                "xmin": estensione[0], "ymin": estensione[1],
                "xmax": estensione[2], "ymax": estensione[3],
            },
            "cell_size": 1000.0,
            "crs": crs,
        })
    };
    let output = analizza(
        "geo.generate_grid",
        std::slice::from_ref(&trigger),
        &griglia(
            "EPSG:32632",
            [600_000.0, 4_900_000.0, 610_000.0, 4_910_000.0],
        ),
    )
    .expect("griglia in UTM 32N");
    assert_eq!(crs_di(&output), Some(("EPSG", 32632)));
    // Gradi dati come estensione a Gauss-Boaga: fuori dal dominio.
    let errore = analizza(
        "geo.generate_grid",
        std::slice::from_ref(&trigger),
        &griglia("EPSG:3003", [12.0, 41.0, 13.0, 42.0]),
    )
    .expect_err("gradi in Gauss-Boaga");
    assert!(errore.to_string().contains("extent"), "{errore}");
    // Un codice fuori tabella resta un errore esplicito.
    let errore = analizza(
        "geo.generate_grid",
        std::slice::from_ref(&trigger),
        &griglia("EPSG:99999", [0.0, 0.0, 1000.0, 1000.0]),
    )
    .expect_err("codice sconosciuto");
    assert!(errore.to_string().contains("CRS_NOT_BUILTIN"), "{errore}");
}
