//! Lettura del contratto dallo schema Arrow: chiavi di una colonna
//! geometrica, stato CRS e contratto di input.
//!
//! I casi vengono dai test della CLI di `plenora-data-tools`, che ne era il
//! primo chiamante; il codice che provano è di questo crate
//! (`contract::arrow_metadata` e `contract::arrow_schema`). Qui si usano
//! solo le API pubbliche. «Discovery» nei nomi dei test è la lettura del
//! contratto da uno schema.
//!
//! Il risolutore è quello di base di `plenora-core`: nessuno di questi casi
//! arriva alla risoluzione (le regole che li decidono vengono prima, o il CRS
//! è iniettato dal caso). I casi il cui esito dipende dal risolutore stanno
//! in `plenora-kernels-geo/tests/scoperta_contratto_con_risolutore.rs`.

use std::collections::HashMap;
use std::sync::Arc;

use plenora_core::arrow::schema::{DataType, Field, Schema, SchemaRef};
use plenora_core::contract::arrow_metadata::{
    geometry_output_field, read_geometry_contract_keys, CanonicalGeometryKeys,
    GEOARROW_EXTENSION_KEY, GEOARROW_WKB_EXTENSION, GEO_METADATA_KEY, PLENORA_CONTRACT_VERSION_KEY,
    PLENORA_GEOMETRY_AXIS_ORDER_KEY, PLENORA_GEOMETRY_CRS_DEFINITION_FORMAT_KEY,
    PLENORA_GEOMETRY_CRS_DEFINITION_KEY, PLENORA_GEOMETRY_CRS_ID_KEY,
    PLENORA_GEOMETRY_CRS_RESOLUTION_KEY, PLENORA_GEOMETRY_DIMENSIONS_KEY,
    PLENORA_GEOMETRY_ENCODING_KEY, PLENORA_GEOMETRY_SRID_KEY,
    PLENORA_GEOMETRY_TYPES_DECLARATION_KEY, PLENORA_GEOMETRY_TYPES_KEY,
};
use plenora_core::contract::arrow_schema::{
    contract_crs_from_keys, contract_from_arrow_schema as discover_input_contract_from_schema,
    crs_definition_from_metadata, geometry_contract_from_field,
};
use plenora_core::contract::{
    ContractCrs, CrsResolution, GeometryColumnContract, GeometryDimensions, GeometryEncoding,
    PropertyConfidence, PropertyScope,
};
use plenora_core::crs::{resolve_crs, CrsKind, ResolvedCrs};
use plenora_core::{ErrorPhase, PlenoraError};

fn projected_crs() -> ResolvedCrs {
    ResolvedCrs::from_resolved_parts(
        "EPSG:32632".to_owned(),
        serde_json::json!({"type": "ProjectedCRS", "name": "WGS 84 / UTM zone 32N"}),
        CrsKind::Projected,
        Some(1.0),
    )
}

/// Campo geometria GeoArrow-WKB con il metadato `geo` dato (o senza).
fn geometry_field(geo_json: Option<&str>) -> Field {
    let mut metadata = HashMap::new();
    metadata.insert(
        GEOARROW_EXTENSION_KEY.to_owned(),
        GEOARROW_WKB_EXTENSION.to_owned(),
    );
    if let Some(geo) = geo_json {
        metadata.insert(GEO_METADATA_KEY.to_owned(), geo.to_owned());
    }
    Field::new("geometry", DataType::Binary, true).with_metadata(metadata)
}

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

/// Schema con la versione del contratto nei metadati di schema.
fn schema_v1(fields: Vec<Field>) -> SchemaRef {
    Arc::new(Schema::new_with_metadata(
        fields,
        HashMap::from([(PLENORA_CONTRACT_VERSION_KEY.to_owned(), "1".to_owned())]),
    ))
}

/// Lettura di contratto del campo + costruzione del contratto, come nel
/// ciclo di `contract_from_arrow_schema` (la risoluzione CRS resta
/// iniettata dai test).
fn contract_from_field(field: &Field) -> Result<GeometryColumnContract, PlenoraError> {
    let keys = read_geometry_contract_keys(field)?;
    Ok(geometry_contract_from_field(
        field,
        ContractCrs::Resolved(projected_crs()),
        &keys,
    ))
}

/// Campo geometria canonico con le chiavi date (helper delle fixture
/// CRS: schema con versione del contratto, colonna `id` + `geometry`).
fn canonical_crs_field(pairs: &[(&str, &str)]) -> Field {
    canonical_field(DataType::Binary, pairs)
}

/// Le parti di uno stato `DeclaredUnresolved`; ogni altro stato e' un
/// fallimento del caso, con lo stato trovato nel messaggio.
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
fn discovery_reads_dimensions_and_encoding_from_metadata() {
    let contract = contract_from_field(&geometry_field(Some(
        r#"{"crs":"EPSG:32632","dimensions":"xyz","encoding":"ewkb"}"#,
    )))
    .expect("discovery");
    assert_eq!(contract.dimensions, GeometryDimensions::Xyz);
    assert_eq!(contract.encoding, Some(GeometryEncoding::Ewkb));

    // Forma scritta dai writer correnti (dimensions xy, niente encoding).
    let written = geometry_output_field("geometry", "EPSG:32632").expect("field");
    let contract = contract_from_field(&written).expect("discovery");
    assert_eq!(contract.dimensions, GeometryDimensions::Xy);
    // Il nome di estensione `geoarrow.wkb` dichiara
    // la famiglia WKB e completa l'encoding assente altrove (ultimo
    // rango della precedenza): non piu' `None`.
    assert_eq!(contract.encoding, Some(GeometryEncoding::Wkb));
}

#[test]
fn discovery_without_dimensions_metadata_propagates_unknown_never_xy() {
    // (b) Chiave `dimensions` assente -> Unknown propagato nel
    // contratto, MAI un default silenzioso Xy.
    let contract =
        contract_from_field(&geometry_field(Some(r#"{"crs":"EPSG:32632"}"#))).expect("discovery");
    assert_eq!(contract.dimensions, GeometryDimensions::Unknown);
    // Come sopra: encoding completato dal nome di estensione.
    assert_eq!(contract.encoding, Some(GeometryEncoding::Wkb));
}

#[test]
fn discovery_rejects_unreadable_dimensions_never_ignores_them() {
    // Lettura strict: un valore `dimensions` non canonico o non
    // testuale e' un errore esplicito, mai ignorato ne' mappato a
    // `Unknown` — «illeggibile» non e' «assente».
    for geo_json in [
        r#"{"crs":"EPSG:32632","dimensions":"2d"}"#,
        r#"{"crs":"EPSG:32632","dimensions":42}"#,
    ] {
        let result = read_geometry_contract_keys(&geometry_field(Some(geo_json)));
        assert!(result.is_err(), "geo: {geo_json}");
    }
}

#[test]
fn discovery_legacy_field_leaves_types_undeclared() {
    // Ingresso legacy senza la coppia types_declaration/types ->
    // «proprieta' non dichiarata» (confidence Unknown), MAI unresolved.
    let contract =
        contract_from_field(&geometry_field(Some(r#"{"crs":"EPSG:32632"}"#))).expect("discovery");
    assert!(contract.types.value().is_none());
}

#[test]
fn discovery_rejects_canonical_geometry_field_of_non_binary_type() {
    // (1c) chiavi canoniche coerenti ma tipo non Binary -> errore.
    let schema = schema_v1(vec![canonical_geometry_field(DataType::Utf8)]);
    let result = discover_input_contract_from_schema(schema, resolve_crs);
    assert!(matches!(result, Err(PlenoraError::InvalidPlan(_))));
}

#[test]
fn discovery_rejects_contract_version_newer_than_supported() {
    // (b) Versione successiva a quella nota -> fallimento
    // esplicito (Unsupported), mai interpretazione parziale.
    let schema = Arc::new(Schema::new_with_metadata(
        vec![Field::new("id", DataType::Int64, false)],
        HashMap::from([(PLENORA_CONTRACT_VERSION_KEY.to_owned(), "2".to_owned())]),
    ));
    let result = discover_input_contract_from_schema(schema, resolve_crs);
    assert!(matches!(result, Err(PlenoraError::Unsupported(_))));
}

#[test]
fn discovery_rejects_canonical_legacy_divergence() {
    // (c) Nozione divergente fra chiavi canoniche e metadato legacy
    // -> il componente fallisce, non sceglie.
    let field = geometry_field(Some(r#"{"crs":"EPSG:32632","dimensions":"xy"}"#));
    let mut metadata = field.metadata().clone();
    metadata.insert(PLENORA_GEOMETRY_DIMENSIONS_KEY.to_owned(), "xyz".to_owned());
    let field = field.with_metadata(metadata);
    let result = discover_input_contract_from_schema(schema_v1(vec![field]), resolve_crs);
    match result {
        Err(PlenoraError::InvalidPlan(message)) => {
            assert!(message.contains("divergente"), "{message}");
        }
        other => panic!("attesa divergenza R2.6, ottenuto {other:?}"),
    }
}

// -------------------------------------------------------------------
// La lettura del contratto non pretende un CRS risolvibile: lo stato
// `missing` entra nel contratto.
// -------------------------------------------------------------------

#[test]
fn discovery_geometry_with_geo_metadata_without_crs_is_missing() {
    // Metadato `geo` presente ma senza chiave `crs` (dimensions sola):
    // anche qui nessun CRS dichiarato -> `missing`, mai errore.
    let schema = Arc::new(Schema::new(vec![geometry_field(Some(
        r#"{"dimensions":"xy"}"#,
    ))]));
    let contract = discover_input_contract_from_schema(schema, resolve_crs).expect("discovery");
    assert!(matches!(contract.geometries[0].crs, ContractCrs::Missing));
    assert_eq!(contract.geometries[0].dimensions, GeometryDimensions::Xy);
}

#[test]
fn discovery_canonical_missing_resolution_is_carried() {
    // `crs_resolution = missing` dichiarato canonicamente (senza chiavi
    // CRS, come impone la coerenza delle chiavi) -> stato missing nel
    // contratto.
    let metadata = HashMap::from([
        (PLENORA_GEOMETRY_ENCODING_KEY.to_owned(), "wkb".to_owned()),
        (PLENORA_GEOMETRY_DIMENSIONS_KEY.to_owned(), "xy".to_owned()),
        (
            PLENORA_GEOMETRY_CRS_RESOLUTION_KEY.to_owned(),
            "missing".to_owned(),
        ),
    ]);
    let field = Field::new("geometry", DataType::Binary, true).with_metadata(metadata);
    let contract = discover_input_contract_from_schema(schema_v1(vec![field]), resolve_crs)
        .expect("discovery");
    assert!(matches!(contract.geometries[0].crs, ContractCrs::Missing));
}

#[test]
fn discovery_rejects_resolution_declaration_without_any_crs() {
    // Una dichiarazione `resolved` (o `declared_unresolved`) senza
    // alcuna rappresentazione CRS e' una contraddizione — MAI collassata
    // su `missing`: errore esplicito che nomina la chiave.
    for resolution in ["resolved", "declared_unresolved"] {
        let metadata = HashMap::from([
            (PLENORA_GEOMETRY_DIMENSIONS_KEY.to_owned(), "xy".to_owned()),
            (
                PLENORA_GEOMETRY_CRS_RESOLUTION_KEY.to_owned(),
                resolution.to_owned(),
            ),
        ]);
        let field = Field::new("geometry", DataType::Binary, true).with_metadata(metadata);
        let result = discover_input_contract_from_schema(schema_v1(vec![field]), resolve_crs);
        match result {
            Err(PlenoraError::InvalidPlan(message)) => {
                assert!(
                    message.contains("nessun CRS e' dichiarato in alcuna rappresentazione"),
                    "{resolution}: {message}"
                );
            }
            other => panic!("{resolution}: attesa contraddizione, ottenuto {other:?}"),
        }
    }
}

#[test]
fn discovery_rejects_malformed_geo_metadata_never_treats_it_as_missing() {
    // Un metadato `geo` illeggibile non diventa «CRS assente»:
    // «illeggibile» non e' «assente», e' un errore, e la regola dello stato
    // `missing` non lo trasforma in un CRS mancante.
    let schema = Arc::new(Schema::new(vec![geometry_field(Some("not json"))]));
    let result = discover_input_contract_from_schema(schema, resolve_crs);
    assert!(result.is_err(), "metadato geo malformato -> errore");
}

// -------------------------------------------------------------------
// `declared_unresolved`: preservato, mai risolto in assenza di una
// decisione esplicita nel piano.
// -------------------------------------------------------------------

#[test]
fn discovery_declared_unresolved_is_preserved_never_auto_resolved() {
    // Una dichiarazione `declared_unresolved` con crs_id RISOLVIBILE
    // (EPSG:32632) non e' risolta ed emessa come `resolved`: il centro
    // preserva lo stato dichiarato. Il risolutore non e' coinvolto.
    let field = canonical_crs_field(&[
        (PLENORA_GEOMETRY_CRS_RESOLUTION_KEY, "declared_unresolved"),
        (PLENORA_GEOMETRY_CRS_ID_KEY, "EPSG:32632"),
        (PLENORA_GEOMETRY_AXIS_ORDER_KEY, "unknown"),
    ]);
    let contract = discover_input_contract_from_schema(schema_v1(vec![field]), resolve_crs)
        .expect("discovery");
    let (crs_id, definition, _) = declared_unresolved_parts(&contract.geometries[0].crs);
    assert_eq!(crs_id, Some("EPSG:32632"));
    assert_eq!(definition, None);
    assert_eq!(
        contract.geometries[0].crs.resolution(),
        CrsResolution::DeclaredUnresolved
    );
}

#[test]
fn discovery_conflicting_crs_id_and_srid_become_declared_unresolved() {
    // Il caso `conflicting_crs` del corpus di conformita': crs_id=EPSG:4326
    // con srid=3003. Il centro preserva: lo stato diventa
    // DeclaredUnresolved con la dichiarazione originale. Resta preservato
    // anche con `resolved` dichiarato (test successivo).
    let field = canonical_crs_field(&[
        (PLENORA_GEOMETRY_CRS_ID_KEY, "EPSG:4326"),
        (PLENORA_GEOMETRY_AXIS_ORDER_KEY, "lon_lat"),
        (PLENORA_GEOMETRY_SRID_KEY, "3003"),
    ]);
    let contract = discover_input_contract_from_schema(schema_v1(vec![field]), resolve_crs)
        .expect("discovery");
    let (crs_id, definition, _) = declared_unresolved_parts(&contract.geometries[0].crs);
    assert_eq!(crs_id, Some("EPSG:4326"));
    assert_eq!(definition, None);
}

#[test]
fn discovery_declared_resolved_with_conflicting_crs_id_and_srid_stays_unresolved() {
    // Una dichiarazione `resolved` non puo' nascondere un conflitto
    // numerico decidibile fra identificatore e SRID.
    let field = canonical_crs_field(&[
        (PLENORA_GEOMETRY_CRS_RESOLUTION_KEY, "resolved"),
        (PLENORA_GEOMETRY_CRS_ID_KEY, "EPSG:4326"),
        (PLENORA_GEOMETRY_AXIS_ORDER_KEY, "lon_lat"),
        (PLENORA_GEOMETRY_SRID_KEY, "3003"),
    ]);
    let contract = discover_input_contract_from_schema(schema_v1(vec![field]), resolve_crs)
        .expect("discovery");
    let (crs_id, definition, _) = declared_unresolved_parts(&contract.geometries[0].crs);
    assert_eq!(crs_id, Some("EPSG:4326"));
    assert_eq!(definition, None);
}

#[test]
fn discovery_crs_id_and_definition_copresent_become_declared_unresolved() {
    // Due rappresentazioni risolvibili co-presenti: l'accordo non e'
    // decidibile testualmente, quindi `DeclaredUnresolved` con entrambe le
    // dichiarazioni. La regola (2a) di `contract_crs_from_keys` vale solo
    // per input non dichiarati, per questo la fixture non porta
    // `crs_resolution`.
    let field = canonical_crs_field(&[
        (PLENORA_GEOMETRY_CRS_ID_KEY, "EPSG:4326"),
        (
            PLENORA_GEOMETRY_CRS_DEFINITION_KEY,
            r#"{"type":"GeographicCRS"}"#,
        ),
        (PLENORA_GEOMETRY_CRS_DEFINITION_FORMAT_KEY, "projjson"),
        (PLENORA_GEOMETRY_AXIS_ORDER_KEY, "lat_lon"),
    ]);
    let contract = discover_input_contract_from_schema(schema_v1(vec![field]), resolve_crs)
        .expect("discovery");
    let (crs_id, definition, definition_format) =
        declared_unresolved_parts(&contract.geometries[0].crs);
    assert_eq!(crs_id, Some("EPSG:4326"));
    assert_eq!(definition, Some(r#"{"type":"GeographicCRS"}"#));
    assert_eq!(definition_format, Some("projjson"));
}

#[test]
fn discovery_contract_errors_keep_the_derived_validate_phase() {
    // Regressione: gli errori della lettura del contratto (coerenza
    // dei metadati) NON sono taggati: restano validazione
    // derivata per variante. Il tagging copre solo la lettura fisica.
    let field = geometry_field(Some(r#"{"crs":"EPSG:32632","dimensions":"xy"}"#));
    let mut metadata = field.metadata().clone();
    metadata.insert(PLENORA_GEOMETRY_DIMENSIONS_KEY.to_owned(), "xyz".to_owned());
    let field = field.with_metadata(metadata);
    let error = discover_input_contract_from_schema(schema_v1(vec![field]), resolve_crs)
        .expect_err("divergenza R2.6");
    assert_eq!(error.phase(), ErrorPhase::Validate);
    assert_eq!(error.phase_tag(), None, "nessun tag: fase derivata");
}

// -------------------------------------------------------------------
// Definizione CRS dalle rappresentazioni accettate (R4.x)
// -------------------------------------------------------------------

#[test]
fn crs_definition_from_metadata_accepts_objects_and_rejects_other_types() {
    // PROJJSON come oggetto: serializzato compatto, mai perso (chiavi in
    // ordine canonico: `serde_json::Value` le riordina alfabeticamente).
    let object = r#"{"crs":{"type":"GeographicCRS","name":"WGS 84"}}"#.to_owned();
    let definition =
        crs_definition_from_metadata("geometry", Some(&object)).expect("oggetto PROJJSON");
    assert_eq!(
        definition.as_deref(),
        Some(r#"{"name":"WGS 84","type":"GeographicCRS"}"#)
    );
    // Tipo non stringa/oggetto: metadato malformato -> errore.
    let invalid = r#"{"crs":32632}"#.to_owned();
    let result = crs_definition_from_metadata("geometry", Some(&invalid));
    assert!(
        matches!(result, Err(PlenoraError::InvalidPlan(_))),
        "{result:?}"
    );
    // Senza chiave `crs` e senza metadato: assenza, non errore.
    let bare = "{}".to_owned();
    assert_eq!(
        crs_definition_from_metadata("geometry", Some(&bare)).expect("nessun crs"),
        None
    );
    assert_eq!(
        crs_definition_from_metadata("geometry", None).expect("nessun metadato"),
        None
    );
}

#[test]
fn contract_crs_from_keys_srid_only_declared_unresolved_is_a_representation() {
    // Catena MySQL TLS Database→Data: il provider dichiara
    // `declared_unresolved` con solo `srid`. Lo SRID e' la terza
    // rappresentazione CRS, quindi non e' la contraddizione «stato senza
    // rappresentazioni»:
    // `DeclaredUnresolved` con crs_id/definition/format assenti, mai
    // sintetizzati.
    let keys = CanonicalGeometryKeys {
        srid: Some(4326),
        crs_resolution: Some(CrsResolution::DeclaredUnresolved),
        ..CanonicalGeometryKeys::default()
    };
    let ContractCrs::DeclaredUnresolved {
        crs_id,
        definition,
        definition_format,
    } = contract_crs_from_keys("geometry", None, &keys, resolve_crs).expect("stato")
    else {
        panic!("atteso DeclaredUnresolved");
    };
    assert_eq!(crs_id, None, "crs_id mai sintetizzato");
    assert_eq!(definition, None, "definizione mai sintetizzata");
    assert_eq!(definition_format, None, "formato mai sintetizzato");
}

#[test]
fn contract_crs_from_keys_resolved_with_srid_only_is_never_promoted() {
    // Fail-closed: lo SRID numerico da solo non identifica un'autorita'
    // risolvibile e il centro non la inventa: un `resolved` dichiarato con
    // SOLO `srid` non e' promosso ne' risolto implicitamente: resta la
    // contraddizione «stato senza rappresentazioni» (errore).
    let keys = CanonicalGeometryKeys {
        srid: Some(4326),
        crs_resolution: Some(CrsResolution::Resolved),
        ..CanonicalGeometryKeys::default()
    };
    let result = contract_crs_from_keys("geometry", None, &keys, resolve_crs);
    assert!(
        matches!(result, Err(PlenoraError::InvalidPlan(_))),
        "`resolved` srid-only non promosso: {result:?}"
    );
}

#[test]
fn discovery_rejects_incoherent_geometry_metadata() {
    // Estensione diversa da `geoarrow.wkb`: rifiuto esplicito.
    let unknown_extension =
        Field::new("geometry", DataType::Binary, true).with_metadata(HashMap::from([(
            GEOARROW_EXTENSION_KEY.to_owned(),
            "geoarrow.point".to_owned(),
        )]));
    let result = discover_input_contract_from_schema(
        Arc::new(Schema::new(vec![unknown_extension])),
        resolve_crs,
    );
    match result {
        Err(PlenoraError::InvalidPlan(message)) => {
            assert!(message.contains("non supportata"), "{message}");
        }
        other => panic!("atteso rifiuto estensione, ottenuto {other:?}"),
    }
    // Metadato `geo` senza estensione: metadati incoerenti.
    let orphan = Field::new("geometry", DataType::Binary, true).with_metadata(HashMap::from([(
        GEO_METADATA_KEY.to_owned(),
        "{}".to_owned(),
    )]));
    let result =
        discover_input_contract_from_schema(Arc::new(Schema::new(vec![orphan])), resolve_crs);
    match result {
        Err(PlenoraError::InvalidPlan(message)) => {
            assert!(message.contains("incoerenti"), "{message}");
        }
        other => panic!("attesi metadati incoerenti, ottenuto {other:?}"),
    }
}

#[test]
fn canonical_types_enter_the_contract_as_declared_with_schema_scope() {
    // Variante proj-indipendente del riconoscimento canonico: la coppia
    // types_declaration/types entra nel contratto come Declared/Schema.
    let contract =
        contract_from_field(&canonical_geometry_field(DataType::Binary)).expect("lettura");
    assert!(
        matches!(contract.types.confidence, PropertyConfidence::Declared(_)),
        "types dichiarati dalla coppia canonica"
    );
    assert_eq!(contract.types.scope, PropertyScope::Schema);
}

#[test]
fn discovery_rejects_canonical_keys_without_contract_version() {
    // Chiavi canoniche senza `plenora.contract.version` nei
    // metadati dello schema -> errore esplicito.
    let schema = Arc::new(Schema::new(vec![canonical_geometry_field(
        DataType::Binary,
    )]));
    let result = discover_input_contract_from_schema(schema, resolve_crs);
    assert!(matches!(result, Err(PlenoraError::InvalidPlan(_))));
}

#[test]
fn discovery_rejects_unrepresentable_encoding() {
    // (d) Framing fuori dall'enum chiuso -> rifiuto esplicito
    // (Unsupported), mai mappato a un encoding noto.
    for geo_json in [
        r#"{"crs":"EPSG:32632","encoding":"gpkg"}"#,
        r#"{"crs":"EPSG:32632","encoding":"twkb"}"#,
        r#"{"crs":"EPSG:32632","encoding":42}"#,
    ] {
        let result = read_geometry_contract_keys(&geometry_field(Some(geo_json)));
        assert!(
            matches!(result, Err(PlenoraError::Unsupported(_))),
            "geo: {geo_json}"
        );
    }

    // Encoding rappresentabile -> propagato nel contratto.
    let contract = contract_from_field(&geometry_field(Some(
        r#"{"crs":"EPSG:32632","encoding":"wkb"}"#,
    )))
    .expect("discovery");
    assert_eq!(contract.encoding, Some(GeometryEncoding::Wkb));
}
