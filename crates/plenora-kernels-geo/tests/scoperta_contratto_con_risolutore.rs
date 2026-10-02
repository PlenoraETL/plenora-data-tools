//! Scoperta del contratto quando l'esito dipende dal risolutore CRS.
//!
//! La conversione schema -> contratto e' di `plenora-core`, ma riceve il
//! risolutore come parametro. Qui (Rust puro, niente PROJ) e' quello di base
//! di core, che risolve solo i CRS della tabella integrata e rifiuta tutto il
//! resto: i casi verificano che la risoluzione arrivi DOPO il riconoscimento
//! e che un rifiuto sia un errore `Crs`, mai un degrado silenzioso.
//!
//! I casi il cui esito non dipende dal risolutore stanno in
//! `plenora-core/tests/contratto_da_schema_arrow.rs`.

use std::collections::HashMap;
use std::sync::Arc;

use plenora_core::arrow::schema::{DataType, Field, Schema, SchemaRef};
use plenora_core::contract::arrow_metadata::{
    CanonicalGeometryKeys, PLENORA_CONTRACT_VERSION_KEY, PLENORA_GEOMETRY_AXIS_ORDER_KEY,
    PLENORA_GEOMETRY_CRS_DEFINITION_FORMAT_KEY, PLENORA_GEOMETRY_CRS_DEFINITION_KEY,
    PLENORA_GEOMETRY_CRS_ID_KEY, PLENORA_GEOMETRY_CRS_RESOLUTION_KEY,
    PLENORA_GEOMETRY_DIMENSIONS_KEY, PLENORA_GEOMETRY_ENCODING_KEY,
    PLENORA_GEOMETRY_TYPES_DECLARATION_KEY, PLENORA_GEOMETRY_TYPES_KEY,
};
use plenora_core::contract::arrow_schema::{
    contract_crs_from_keys, contract_from_arrow_schema as discover_input_contract_from_schema,
};
use plenora_core::contract::ContractCrs;
use plenora_core::crs::resolve_crs;
use plenora_core::PlenoraError;

/// WKT1 realistico di Monte Mario / Italy zone 1 con `AUTHORITY` e
/// `TOWGS84` (EPSG:3003): la forma di uno shapefile catastale.
///
/// Stesso testo dei test della CLI del progetto d'origine, da cui i casi
/// provengono.
const MONTE_MARIO_WKT: &str = concat!(
    r#"PROJCS["Monte Mario / Italy zone 1",GEOGCS["Monte Mario","#,
    r#"DATUM["Monte_Mario",SPHEROID["International 1924",6378388,297],"#,
    r#"TOWGS84[-104.1,-49.1,-9.9,0.971,-2.917,0.714,-11.68]],"#,
    r#"PRIMEM["Greenwich",0],UNIT["degree",0.0174532925199433]],"#,
    r#"PROJECTION["Transverse_Mercator"],PARAMETER["latitude_of_origin",0],"#,
    r#"PARAMETER["central_meridian",9],PARAMETER["scale_factor",0.9996],"#,
    r#"PARAMETER["false_easting",1500000],PARAMETER["false_northing",0],"#,
    r#"UNIT["metre",1],AXIS["Easting",EAST],AXIS["Northing",NORTH],"#,
    r#"AUTHORITY["EPSG","3003"]]"#
);

/// Campo geometria con SOLE chiavi canoniche (niente `GeoArrow` legacy).
fn canonical_geometry_field(data_type: DataType) -> Field {
    canonical_field(
        data_type,
        &[
            (PLENORA_GEOMETRY_DIMENSIONS_KEY, "xyz"),
            (PLENORA_GEOMETRY_TYPES_DECLARATION_KEY, "exact"),
            (PLENORA_GEOMETRY_TYPES_KEY, "point"),
            (PLENORA_GEOMETRY_CRS_RESOLUTION_KEY, "resolved"),
            (PLENORA_GEOMETRY_CRS_ID_KEY, "EPSG:32632"),
            (PLENORA_GEOMETRY_AXIS_ORDER_KEY, "unknown"),
        ],
    )
}

/// Campo `geometry` del tipo dato con l'estensione `geoarrow.wkb`,
/// `encoding = wkb`, `dimensions = xy` e poi le coppie date, che
/// prevalgono sulle due di base.
fn canonical_field(data_type: DataType, pairs: &[(&str, &str)]) -> Field {
    let mut metadata = HashMap::from([
        ("ARROW:extension:name".to_owned(), "geoarrow.wkb".to_owned()),
        (PLENORA_GEOMETRY_ENCODING_KEY.to_owned(), "wkb".to_owned()),
        (PLENORA_GEOMETRY_DIMENSIONS_KEY.to_owned(), "xy".to_owned()),
    ]);
    for (key, value) in pairs {
        metadata.insert((*key).to_owned(), (*value).to_owned());
    }
    Field::new("geometry", data_type, true).with_metadata(metadata)
}

/// Schema con la versione di protocollo (`plenora.contract.version`) nei
/// metadati di schema.
fn schema_v1(fields: Vec<Field>) -> SchemaRef {
    Arc::new(Schema::new_with_metadata(
        fields,
        HashMap::from([(PLENORA_CONTRACT_VERSION_KEY.to_owned(), "1".to_owned())]),
    ))
}

/// Campo geometria canonico con le chiavi date (helper delle fixture
/// CRS: schema con versione di protocollo, colonna `id` + `geometry`).
fn canonical_crs_field(pairs: &[(&str, &str)]) -> Field {
    canonical_field(DataType::Binary, pairs)
}

#[test]
fn discovery_recognizes_canonical_geometry_field() {
    // Chiavi canoniche ed estensione `geoarrow.wkb`, senza metadato `geo`:
    // il campo e' una geometria, con types Declared/Schema dalla coppia
    // canonica. Senza l'estensione le stesse chiavi si rifiutano
    // (vocabolario Arrow 1.0, sezione 4).
    let schema = schema_v1(vec![
        Field::new("id", DataType::Int64, false),
        canonical_geometry_field(DataType::Binary),
    ]);
    let contract = discover_input_contract_from_schema(schema, resolve_crs)
        .expect("EPSG:32632 e' nella tabella dei CRS integrati");
    let senza_estensione = {
        let field = canonical_geometry_field(DataType::Binary);
        let mut metadata = field.metadata().clone();
        metadata.remove("ARROW:extension:name");
        field.with_metadata(metadata)
    };
    let rifiutato =
        discover_input_contract_from_schema(schema_v1(vec![senza_estensione]), resolve_crs);
    assert!(
        matches!(rifiutato, Err(PlenoraError::Schema(_))),
        "{rifiutato:?}"
    );
    // Il campo canonico e' riconosciuto come geometria e il CRS
    // dichiarato si risolve dalla tabella integrata.
    assert_eq!(contract.geometries.len(), 1);
    let ContractCrs::Resolved(crs) = &contract.geometries[0].crs else {
        panic!("atteso CRS risolto: {:?}", contract.geometries[0].crs);
    };
    assert_eq!(crs.authority_identifier(), Some(("EPSG", 32632)));
    // Un codice fuori tabella fallisce chiuso DOPO il riconoscimento: un
    // errore `Crs` (non `InvalidPlan`).
    let schema = schema_v1(vec![
        Field::new("id", DataType::Int64, false),
        canonical_field(
            DataType::Binary,
            &[
                (PLENORA_GEOMETRY_CRS_RESOLUTION_KEY, "resolved"),
                (PLENORA_GEOMETRY_CRS_ID_KEY, "EPSG:99999"),
                (PLENORA_GEOMETRY_AXIS_ORDER_KEY, "unknown"),
            ],
        ),
    ]);
    let result = discover_input_contract_from_schema(schema, resolve_crs);
    assert!(
        matches!(
            result,
            Err(PlenoraError::Crs(_) | PlenoraError::CrsCoded { .. })
        ),
        "atteso fallimento di risoluzione CRS, ottenuto {result:?}"
    );
}

// -------------------------------------------------------------------
// `resolved` dichiarato con doppia rappresentazione: risoluzione e
// verifica di coerenza decidibile.
// -------------------------------------------------------------------

/// Campo canonico del caso Monte Mario: `resolved` dichiarato, doppia
/// rappresentazione (`crs_id` + definizione WKT) con formato `wkt`.
fn monte_mario_field(crs_id: &str) -> Field {
    canonical_crs_field(&[
        (PLENORA_GEOMETRY_CRS_RESOLUTION_KEY, "resolved"),
        (PLENORA_GEOMETRY_CRS_ID_KEY, crs_id),
        (PLENORA_GEOMETRY_CRS_DEFINITION_KEY, MONTE_MARIO_WKT),
        (PLENORA_GEOMETRY_CRS_DEFINITION_FORMAT_KEY, "wkt"),
        (PLENORA_GEOMETRY_AXIS_ORDER_KEY, "easting_northing"),
    ])
}

#[test]
fn discovery_resolved_with_double_representation_needs_the_backend() {
    // Senza backend PROJ un input `resolved` con doppia rappresentazione
    // non degrada a `DeclaredUnresolved`: la dichiarazione `resolved` si
    // onora risolvendo, quindi la risoluzione impossibile (senza PROJ una
    // definizione WKT non si risolve) fallisce con errore `Crs` — coerente con quel che fa un
    // `resolved` a rappresentazione singola.
    let field = monte_mario_field("EPSG:3003");
    let result = discover_input_contract_from_schema(schema_v1(vec![field]), resolve_crs);
    assert!(
        matches!(
            result,
            Err(PlenoraError::Crs(_) | PlenoraError::CrsCoded { .. })
        ),
        "atteso errore Crs senza backend: {result:?}"
    );
}

#[test]
fn contract_crs_from_keys_legacy_fallback_feeds_the_resolution() {
    // Nessuna forma canonica: il legacy `geo.crs` alimenta la
    // risoluzione (CRS della tabella integrata -> Resolved; altrimenti
    // errore `Crs`, mai `Missing` inventato).
    let legacy = r#"{"crs":"EPSG:32632"}"#.to_owned();
    let keys = CanonicalGeometryKeys::default();
    let result = contract_crs_from_keys("geometry", Some(&legacy), &keys, resolve_crs);
    assert!(
        matches!(&result, Ok(ContractCrs::Resolved(crs)) if crs.authority_srid() == Some(32632)),
        "{result:?}"
    );
    let legacy_wkt = format!(r#"{{"crs":{}}}"#, serde_json::Value::from(MONTE_MARIO_WKT));
    let result = contract_crs_from_keys("geometry", Some(&legacy_wkt), &keys, resolve_crs);
    assert!(
        matches!(
            result,
            Err(PlenoraError::Crs(_) | PlenoraError::CrsCoded { .. })
        ),
        "{result:?}"
    );
    // Nessuna rappresentazione: `Missing`, mai errore (il rifiuto spetta
    // all'analisi delle op che chiedono un CRS).
    let missing = contract_crs_from_keys("geometry", None, &keys, resolve_crs).expect("assente");
    assert!(matches!(missing, ContractCrs::Missing));
}
