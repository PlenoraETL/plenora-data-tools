//! Scoperta del contratto quando l'esito dipende dal risolutore CRS.
//!
//! La conversione schema -> contratto e' di `plenora-core`, ma riceve il
//! risolutore come parametro: senza `proj-backend` e' quello di base di core,
//! che rifiuta sempre; con la feature e' quello PROJ di questo crate. I casi
//! qui sotto stavano nei test unitari del binario `plenora-cli`, che sceglie
//! il risolutore allo stesso modo; stanno in questo crate perche' e' il solo,
//! fra quelli che dipendono da core, che possiede il risolutore PROJ e si
//! prova anche con `proj-backend` (il job `full-backends` della CI).
//!
//! I casi il cui esito non dipende dal risolutore stanno in
//! `plenora-core/tests/contratto_da_schema_arrow.rs`.

use std::collections::HashMap;
use std::sync::Arc;

use plenora_core::arrow::schema::{DataType, Field, Schema, SchemaRef};
#[cfg(feature = "proj-backend")]
use plenora_core::contract::arrow_metadata::PLENORA_GEOMETRY_SRID_KEY;
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
#[cfg(feature = "proj-backend")]
use plenora_core::contract::{
    GeometryDimensions, GeometryEncoding, GeometryType, PropertyConfidence, PropertyScope,
    TypesDeclaration,
};
#[cfg(not(feature = "proj-backend"))]
use plenora_core::PlenoraError;

#[cfg(not(feature = "proj-backend"))]
use plenora_core::crs::resolve_crs;
#[cfg(feature = "proj-backend")]
use plenora_kernels_geo::crs::resolve_crs;

/// WKT1 realistico di Monte Mario / Italy zone 1 con `AUTHORITY` e
/// `TOWGS84` (EPSG:3003): la forma dello shapefile catastale owner.
///
/// Stesso testo di `plenora-cli/tests/comune/costanti.rs`, da cui i casi
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

/// Campo `geometry` del tipo dato con `encoding = wkb`, `dimensions = xy`
/// e poi le coppie date, che prevalgono sulle due di base.
fn canonical_field(data_type: DataType, pairs: &[(&str, &str)]) -> Field {
    let mut metadata = HashMap::from([
        (PLENORA_GEOMETRY_ENCODING_KEY.to_owned(), "wkb".to_owned()),
        (PLENORA_GEOMETRY_DIMENSIONS_KEY.to_owned(), "xy".to_owned()),
    ]);
    for (key, value) in pairs {
        metadata.insert((*key).to_owned(), (*value).to_owned());
    }
    Field::new("geometry", data_type, true).with_metadata(metadata)
}

/// Schema con la versione di protocollo R2.5 nei metadati di schema.
fn schema_v1(fields: Vec<Field>) -> SchemaRef {
    Arc::new(Schema::new_with_metadata(
        fields,
        HashMap::from([(PLENORA_CONTRACT_VERSION_KEY.to_owned(), "1".to_owned())]),
    ))
}

/// Campo geometria canonico con le chiavi date (helper delle fixture
/// CRS: schema con versione R2.5, colonna `id` + `geometry`).
fn canonical_crs_field(pairs: &[(&str, &str)]) -> Field {
    canonical_field(DataType::Binary, pairs)
}

/// Le parti di uno stato `DeclaredUnresolved`; ogni altro stato e' un
/// fallimento del caso, con lo stato trovato nel messaggio.
#[cfg(feature = "proj-backend")]
fn declared_unresolved_parts(
    crs: &ContractCrs,
) -> (Option<&str>, Option<&str>, Option<&'static str>) {
    let ContractCrs::DeclaredUnresolved {
        crs_id,
        definition,
        definition_format,
    } = crs
    else {
        panic!("atteso DeclaredUnresolved: {crs:?}");
    };
    (
        crs_id.as_deref(),
        definition.as_deref(),
        definition_format.map(plenora_core::contract::CrsDefinitionFormat::as_str),
    )
}

#[test]
fn discovery_recognizes_canonical_only_geometry_field() {
    // (a) tabella §2: le chiavi canoniche sono autosufficienti — il campo
    // e' riconosciuto come geometria anche senza estensione `geoarrow.wkb`
    // e metadato `geo`, con types Declared/Schema dalla coppia canonica.
    let schema = schema_v1(vec![
        Field::new("id", DataType::Int64, false),
        canonical_geometry_field(DataType::Binary),
    ]);
    let result = discover_input_contract_from_schema(schema, resolve_crs);
    #[cfg(feature = "proj-backend")]
    {
        let contract = result.expect("discovery canonica");
        assert_eq!(contract.geometries.len(), 1);
        let geometry = &contract.geometries[0];
        assert_eq!(geometry.dimensions, GeometryDimensions::Xyz);
        assert_eq!(geometry.encoding, Some(GeometryEncoding::Wkb));
        assert!(
            matches!(geometry.types.confidence, PropertyConfidence::Declared(_)),
            "types dichiarati dalla coppia canonica"
        );
        assert_eq!(geometry.types.scope, PropertyScope::Schema);
        let types = geometry.types.value().expect("types");
        assert_eq!(types.declaration(), TypesDeclaration::Exact);
        assert_eq!(types.types(), &[GeometryType::Point]);
    }
    #[cfg(not(feature = "proj-backend"))]
    {
        // Senza backend PROJ la risoluzione CRS fallisce chiusa DOPO il
        // riconoscimento: un errore `Crs` (non `InvalidPlan`) dimostra che
        // il campo canonico-only e' stato riconosciuto come geometria e
        // le chiavi lette senza errori.
        assert!(
            matches!(result, Err(PlenoraError::Crs(_))),
            "atteso fallimento di risoluzione CRS, ottenuto {result:?}"
        );
    }
}

// -------------------------------------------------------------------
// Emendamento 2026-07-31 (classe A): `resolved` dichiarato con doppia
// rappresentazione — risoluzione + verifica di coerenza decidibile.
// -------------------------------------------------------------------

/// Campo canonico del caso owner: `resolved` dichiarato, doppia
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

#[cfg(feature = "proj-backend")]
#[test]
fn discovery_resolved_with_coherent_wkt_resolves_with_authority_srid() {
    // Il caso owner: `resolved` + crs_id=EPSG:3003 + WKT Monte Mario
    // coerente. La (2a) NON rovescia la dichiarazione: il WKT risolve
    // contro PROJ e la verifica di coerenza (crs_id 3003 == srid del
    // canonical) conferma — `Resolved`, con `authority_srid` 3003.
    let field = monte_mario_field("EPSG:3003");
    let contract = discover_input_contract_from_schema(schema_v1(vec![field]), resolve_crs)
        .expect("discovery");
    let ContractCrs::Resolved(resolved) = &contract.geometries[0].crs else {
        panic!("atteso Resolved: {:?}", contract.geometries[0].crs);
    };
    assert_eq!(resolved.authority_srid(), Some(3003));
}

#[cfg(feature = "proj-backend")]
#[test]
fn discovery_resolved_with_divergent_crs_id_becomes_declared_unresolved() {
    // Stessa fixture ma crs_id=EPSG:4326: il WKT risolve a 3003, il
    // confronto decidibile smentisce il `resolved` dichiarato —
    // `DeclaredUnresolved` con le dichiarazioni ORIGINALI preservate
    // (non passa e nulla si perde).
    let field = monte_mario_field("EPSG:4326");
    let contract = discover_input_contract_from_schema(schema_v1(vec![field]), resolve_crs)
        .expect("discovery");
    let (crs_id, definition, definition_format) =
        declared_unresolved_parts(&contract.geometries[0].crs);
    assert_eq!(crs_id, Some("EPSG:4326"));
    assert_eq!(definition, Some(MONTE_MARIO_WKT));
    assert_eq!(definition_format, Some("wkt"));
}

#[cfg(feature = "proj-backend")]
#[test]
fn discovery_resolved_with_same_code_but_different_authority_stays_unresolved() {
    let field = monte_mario_field("FOO:3003");
    let contract = discover_input_contract_from_schema(schema_v1(vec![field]), resolve_crs)
        .expect("discovery");
    assert!(
        matches!(
            contract.geometries[0].crs,
            ContractCrs::DeclaredUnresolved { .. }
        ),
        "un'autorita' diversa non puo' essere certificata dal solo codice numerico"
    );
}

#[cfg(not(feature = "proj-backend"))]
#[test]
fn discovery_resolved_with_double_representation_needs_the_backend() {
    // Conseguenza DICHIARATA dell'emendamento 2026-07-31 (classe A):
    // senza `proj-backend` un input `resolved` con doppia
    // rappresentazione non degrada a `DeclaredUnresolved`. La
    // dichiarazione si onora con la regola (3), quindi la risoluzione
    // impossibile fallisce con errore `Crs` — coerente con quel che fa un
    // `resolved` a rappresentazione singola.
    let field = monte_mario_field("EPSG:3003");
    let result = discover_input_contract_from_schema(schema_v1(vec![field]), resolve_crs);
    assert!(
        matches!(result, Err(PlenoraError::Crs(_))),
        "atteso errore Crs senza backend: {result:?}"
    );
}

#[cfg(feature = "proj-backend")]
#[test]
fn discovery_coherent_crs_id_and_srid_still_resolves() {
    // srid coerente con il codice dell'identificatore (come il caso
    // `multipolygon_xyzm_srid` del corpus): nessun conflitto, la
    // risoluzione avviene come sempre.
    let field = canonical_crs_field(&[
        (PLENORA_GEOMETRY_CRS_RESOLUTION_KEY, "resolved"),
        (PLENORA_GEOMETRY_CRS_ID_KEY, "EPSG:32632"),
        (PLENORA_GEOMETRY_AXIS_ORDER_KEY, "easting_northing"),
        (PLENORA_GEOMETRY_SRID_KEY, "32632"),
    ]);
    let contract = discover_input_contract_from_schema(schema_v1(vec![field]), resolve_crs)
        .expect("discovery");
    assert!(
        matches!(contract.geometries[0].crs, ContractCrs::Resolved(_)),
        "srid coerente -> risoluzione"
    );
}

#[test]
fn contract_crs_from_keys_legacy_fallback_feeds_the_resolution() {
    // Nessuna forma canonica: il legacy `geo.crs` alimenta la
    // risoluzione (con backend -> Resolved; senza -> errore `Crs` di
    // backend, mai `Missing` inventato).
    let legacy = r#"{"crs":"EPSG:32632"}"#.to_owned();
    let keys = CanonicalGeometryKeys::default();
    let result = contract_crs_from_keys("geometry", Some(&legacy), &keys, resolve_crs);
    #[cfg(feature = "proj-backend")]
    assert!(matches!(result, Ok(ContractCrs::Resolved(_))), "{result:?}");
    #[cfg(not(feature = "proj-backend"))]
    assert!(matches!(result, Err(PlenoraError::Crs(_))), "{result:?}");
    // Nessuna rappresentazione: `Missing`, mai errore (R4.6.3).
    let missing = contract_crs_from_keys("geometry", None, &keys, resolve_crs).expect("assente");
    assert!(matches!(missing, ContractCrs::Missing));
}
