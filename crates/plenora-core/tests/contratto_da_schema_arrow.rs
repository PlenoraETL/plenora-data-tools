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

/// Campo geometria con le chiavi canoniche e l'estensione `geoarrow.wkb`
/// (niente metadato legacy `geo`).
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
        (
            GEOARROW_EXTENSION_KEY.to_owned(),
            GEOARROW_WKB_EXTENSION.to_owned(),
        ),
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
    // (1c) chiavi canoniche coerenti ma tipo non Binary -> errore di
    // schema (vocabolario Arrow 1.0, sezione 4).
    let schema = schema_v1(vec![canonical_geometry_field(DataType::Utf8)]);
    let result = discover_input_contract_from_schema(schema, resolve_crs);
    assert!(matches!(result, Err(PlenoraError::Schema(_))));
}

#[test]
fn discovery_rejects_canonical_keys_without_the_extension() {
    // Vocabolario Arrow 1.0, sezione 4: «Geometry keys on a field without
    // the geoarrow.wkb extension are invalid». Prima le chiavi canoniche
    // bastavano da sole a dichiarare la colonna.
    let field = canonical_geometry_field(DataType::Binary);
    let mut metadata = field.metadata().clone();
    metadata.remove(GEOARROW_EXTENSION_KEY);
    let field = field.with_metadata(metadata);
    let result = discover_input_contract_from_schema(schema_v1(vec![field]), resolve_crs);
    match result {
        Err(PlenoraError::Schema(message)) => {
            assert!(
                message.contains("senza `ARROW:extension:name`"),
                "{message}"
            );
        }
        other => panic!("attese chiavi senza estensione rifiutate, ottenuto {other:?}"),
    }
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
        Err(PlenoraError::Schema(message)) => {
            assert!(message.contains("divergente"), "{message}");
        }
        other => panic!("attesa divergenza fra contratto e schema, ottenuto {other:?}"),
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
    let field = canonical_field(
        DataType::Binary,
        &[(PLENORA_GEOMETRY_CRS_RESOLUTION_KEY, "missing")],
    );
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
        let field = canonical_field(
            DataType::Binary,
            &[(PLENORA_GEOMETRY_CRS_RESOLUTION_KEY, resolution)],
        );
        let result = discover_input_contract_from_schema(schema_v1(vec![field]), resolve_crs);
        match result {
            Err(PlenoraError::Crs(message)) => {
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
        .expect_err("divergenza fra contratto e schema");
    assert_eq!(error.phase(), ErrorPhase::Validate);
    assert_eq!(error.phase_tag(), None, "nessun tag: fase derivata");
}

// -------------------------------------------------------------------
// Definizione CRS dalle rappresentazioni accettate
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
    assert!(matches!(result, Err(PlenoraError::Crs(_))), "{result:?}");
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
fn contract_crs_from_keys_srid_only_declared_unresolved_is_rejected() {
    // Vocabolario Arrow 1.0, sezione 4: `declared_unresolved` richiede un
    // identificatore o una definizione. Il solo `srid` non basta: prima
    // diventava `DeclaredUnresolved` senza rappresentazioni, e l'uscita
    // dichiarava uno stato che il contratto non ammette. Ora e' la
    // contraddizione «stato senza rappresentazioni», categoria `crs`.
    let keys = CanonicalGeometryKeys {
        srid: Some(4326),
        crs_resolution: Some(CrsResolution::DeclaredUnresolved),
        ..CanonicalGeometryKeys::default()
    };
    let result = contract_crs_from_keys("geometry", None, &keys, resolve_crs);
    assert!(matches!(result, Err(PlenoraError::Crs(_))), "{result:?}");
}

#[test]
fn missing_with_srid_is_accepted_and_stays_missing() {
    // Il vocabolario vieta con `missing` identificatore, definizione,
    // formato e ordine degli assi, non lo `srid`: lo stato resta `missing`
    // (un indizio numerico non diventa un CRS, ARROW-007).
    let field = canonical_field(
        DataType::Binary,
        &[
            (PLENORA_GEOMETRY_CRS_RESOLUTION_KEY, "missing"),
            (PLENORA_GEOMETRY_SRID_KEY, "4326"),
        ],
    );
    let contract = discover_input_contract_from_schema(schema_v1(vec![field]), resolve_crs)
        .expect("missing con srid");
    assert!(matches!(contract.geometries[0].crs, ContractCrs::Missing));
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
        matches!(result, Err(PlenoraError::Crs(_))),
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
        Err(PlenoraError::Schema(message)) => {
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
        Err(PlenoraError::Schema(message)) => {
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
    assert!(matches!(result, Err(PlenoraError::Schema(_))));
}

#[test]
fn errors_never_print_nested_field_metadata() {
    // Arrow 60 stampa i metadati dei campi figli nel `Display` e nel
    // `Debug` di un `DataType`: un messaggio d'errore che descrivesse il
    // tipo cosi' riporterebbe metadati del file («errori senza dati»).
    let figlio = Field::new("figlio_riservato", DataType::Binary, true).with_metadata(
        HashMap::from([("chiave.privata".to_owned(), "SEGRETO".to_owned())]),
    );
    let geometria =
        Field::new_struct("geometry", vec![figlio], true).with_metadata(HashMap::from([(
            GEOARROW_EXTENSION_KEY.to_owned(),
            GEOARROW_WKB_EXTENSION.to_owned(),
        )]));
    let errore =
        discover_input_contract_from_schema(Arc::new(Schema::new(vec![geometria])), resolve_crs)
            .expect_err("geometria non Binary");
    let testo = errore.to_string();
    assert!(matches!(errore, PlenoraError::Schema(_)), "{testo}");
    assert!(testo.contains("Struct<1 campi>"), "{testo}");
    assert!(!testo.contains("SEGRETO"), "{testo}");
    assert!(!testo.contains("figlio_riservato"), "{testo}");
    // La proiezione pubblica (`plenora-error-v1`) porta la stessa
    // categoria e nessun valore.
    let pubblico = errore.public_projection();
    assert_eq!(pubblico.category().as_str(), "schema");
    assert!(
        !pubblico.message().contains("SEGRETO"),
        "{}",
        pubblico.message()
    );
    assert!(
        !pubblico.message().contains("figlio_riservato"),
        "{}",
        pubblico.message()
    );
}

#[test]
fn metadata_errors_project_with_their_category_and_no_values() {
    // I metadati incoerenti escono con categoria `schema` o `crs`
    // (vocabolario Arrow 1.0, sezione 4) anche nella proiezione pubblica, e
    // il valore ricevuto non compare nel messaggio. Codice: nessuno (le
    // varianti `Schema` e `Crs` non ne hanno uno).
    let casi: Vec<(Field, Option<&str>, &str, &str)> = vec![
        // CRS mancante con identificatore: contraddizione del CRS.
        (
            canonical_field(
                DataType::Binary,
                &[
                    (PLENORA_GEOMETRY_CRS_RESOLUTION_KEY, "missing"),
                    (PLENORA_GEOMETRY_CRS_ID_KEY, "EPSG:VALORE_RICEVUTO"),
                    (PLENORA_GEOMETRY_AXIS_ORDER_KEY, "unknown"),
                ],
            ),
            Some("1"),
            "crs",
            "VALORE_RICEVUTO",
        ),
        // Estensione diversa: il nome ricevuto non compare.
        (
            Field::new("geometry", DataType::Binary, true).with_metadata(HashMap::from([(
                GEOARROW_EXTENSION_KEY.to_owned(),
                "geoarrow.VALORE_RICEVUTO".to_owned(),
            )])),
            None,
            "schema",
            "VALORE_RICEVUTO",
        ),
        // `crs_resolution = resolved` senza alcun CRS: il valore non compare.
        (
            canonical_field(
                DataType::Binary,
                &[(PLENORA_GEOMETRY_CRS_RESOLUTION_KEY, "declared_unresolved")],
            ),
            Some("1"),
            "crs",
            "declared_unresolved",
        ),
        // Formato della definizione incoerente con il testo.
        (
            canonical_field(
                DataType::Binary,
                &[
                    (PLENORA_GEOMETRY_CRS_DEFINITION_KEY, "PROJCS[demo]"),
                    (PLENORA_GEOMETRY_CRS_DEFINITION_FORMAT_KEY, "wkt2"),
                    (PLENORA_GEOMETRY_AXIS_ORDER_KEY, "unknown"),
                ],
            ),
            Some("1"),
            "crs",
            "wkt2",
        ),
    ];
    for (campo, versione, categoria, valore) in casi {
        let metadati = versione.map_or_else(HashMap::new, |versione| {
            HashMap::from([(PLENORA_CONTRACT_VERSION_KEY.to_owned(), versione.to_owned())])
        });
        let schema = Arc::new(Schema::new_with_metadata(vec![campo], metadati));
        let errore = discover_input_contract_from_schema(schema, resolve_crs)
            .expect_err("metadati incoerenti");
        let pubblico = errore.public_projection();
        assert_eq!(pubblico.category().as_str(), categoria, "{errore}");
        assert_eq!(pubblico.code(), None, "{errore}");
        assert!(
            !pubblico.message().contains(valore),
            "{}",
            pubblico.message()
        );
    }
}

#[test]
fn nested_plenora_keys_require_the_contract_version() {
    // Una chiave `plenora.*` su un figlio (struct, lista, mappa, a
    // qualunque profondita') rende lo schema uno schema Plenora come quella
    // di un campo di primo livello: senza versione si rifiuta (ARROW-001).
    // Prima si guardavano solo i campi di primo livello.
    let figlio = |chiave: &str| {
        Field::new("figlio", DataType::Float64, true)
            .with_metadata(HashMap::from([(chiave.to_owned(), "x".to_owned())]))
    };
    let annidati = |chiave: &str| {
        vec![
            Field::new_struct("s", vec![figlio(chiave)], true),
            Field::new_list("l", figlio(chiave), true),
            Field::new_list(
                "ll",
                Field::new_struct("elemento", vec![figlio(chiave)], true),
                true,
            ),
            Field::new_map(
                "m",
                "voci",
                Field::new("chiave", DataType::Utf8, false),
                figlio(chiave),
                false,
                true,
            ),
        ]
    };
    for chiave in ["plenora.geometry.precision", "plenora.postgres.tipo"] {
        for campo in annidati(chiave) {
            let nome = campo.name().clone();
            let senza = discover_input_contract_from_schema(
                Arc::new(Schema::new(vec![campo.clone()])),
                resolve_crs,
            );
            assert!(
                matches!(senza, Err(PlenoraError::Schema(_))),
                "{chiave} in `{nome}`: {senza:?}"
            );
            assert!(
                discover_input_contract_from_schema(schema_v1(vec![campo]), resolve_crs).is_ok(),
                "{chiave} in `{nome}` con versione"
            );
        }
    }
    // Una chiave d'altri (non `plenora.`) su un figlio non chiede versione.
    for campo in annidati("pandas.tipo") {
        assert!(discover_input_contract_from_schema(
            Arc::new(Schema::new(vec![campo])),
            resolve_crs
        )
        .is_ok());
    }
}

#[test]
fn discovery_rejects_unknown_contract_versions() {
    // Vocabolario Arrow 1.0, sezione 1: il solo valore ammesso e' `1`, e
    // una versione sconosciuta fallisce chiusa. Una versione successiva e'
    // `Unsupported` (ARROW-002), ogni altra forma e' `Schema`.
    let con_versione = |versione: &str| {
        Arc::new(Schema::new_with_metadata(
            vec![Field::new("id", DataType::Int64, false)],
            HashMap::from([(PLENORA_CONTRACT_VERSION_KEY.to_owned(), versione.to_owned())]),
        ))
    };
    for versione in ["2", "10", "99999999999999999999999"] {
        let result = discover_input_contract_from_schema(con_versione(versione), resolve_crs);
        assert!(
            matches!(result, Err(PlenoraError::Unsupported(_))),
            "{versione}: {result:?}"
        );
    }
    for versione in ["0", "01", "1.0", "+1", " 1", "", "uno"] {
        let result = discover_input_contract_from_schema(con_versione(versione), resolve_crs);
        assert!(
            matches!(result, Err(PlenoraError::Schema(_))),
            "{versione}: {result:?}"
        );
    }
    assert!(discover_input_contract_from_schema(con_versione("1"), resolve_crs).is_ok());
}

#[test]
fn discovery_reads_and_checks_field_identities() {
    // `plenora.field_id`: intero decimale non negativo, unico nello schema,
    // solo sui campi di primo livello.
    let con_id = |nome: &str, id: &str| {
        Field::new(nome, DataType::Int64, true).with_metadata(HashMap::from([(
            "plenora.field_id".to_owned(),
            id.to_owned(),
        )]))
    };
    assert!(discover_input_contract_from_schema(
        schema_v1(vec![con_id("a", "0"), con_id("b", "7")]),
        resolve_crs
    )
    .is_ok());
    let ripetuto = discover_input_contract_from_schema(
        schema_v1(vec![con_id("a", "3"), con_id("b", "3")]),
        resolve_crs,
    );
    assert!(
        matches!(ripetuto, Err(PlenoraError::Schema(_))),
        "{ripetuto:?}"
    );
    for malformato in ["-1", "", "x", "+2", "1.5"] {
        let result = discover_input_contract_from_schema(
            schema_v1(vec![con_id("a", malformato)]),
            resolve_crs,
        );
        assert!(
            matches!(result, Err(PlenoraError::Schema(_))),
            "{malformato}: {result:?}"
        );
    }
    let oltre = discover_input_contract_from_schema(
        schema_v1(vec![con_id("a", "4294967296")]),
        resolve_crs,
    );
    assert!(
        matches!(oltre, Err(PlenoraError::Unsupported(_))),
        "{oltre:?}"
    );
    let annidato = Field::new_struct("s", vec![con_id("figlio", "1")], true);
    let annidato = discover_input_contract_from_schema(schema_v1(vec![annidato]), resolve_crs);
    assert!(
        matches!(annidato, Err(PlenoraError::Unsupported(_))),
        "{annidato:?}"
    );
}

#[test]
fn discovery_rejects_geography_and_non_planar_extension_metadata() {
    // I kernel sono planari: `geography` e `edges` non planari si
    // rifiutano invece di essere letti come `geometry`; un CRS nel metadato
    // d'estensione GeoArrow, che questo componente non interpreta, si
    // rifiuta invece di passare per assente.
    let geography = canonical_field(
        DataType::Binary,
        &[
            ("plenora.geometry.spatial_semantics", "geography"),
            (PLENORA_GEOMETRY_CRS_RESOLUTION_KEY, "missing"),
        ],
    );
    let result = discover_input_contract_from_schema(schema_v1(vec![geography]), resolve_crs);
    assert!(
        matches!(result, Err(PlenoraError::Unsupported(_))),
        "{result:?}"
    );
    for (estensione, atteso_ok) in [
        ("", true),
        ("{}", true),
        (r#"{"edges":"planar"}"#, true),
        (r#"{"edges":"spherical"}"#, false),
        (r#"{"crs":"EPSG:4326"}"#, false),
        (r#"{"crs":null}"#, true),
    ] {
        let mut metadata = geometry_field(None).metadata().clone();
        metadata.insert("ARROW:extension:metadata".to_owned(), estensione.to_owned());
        let field = geometry_field(None).with_metadata(metadata);
        let result =
            discover_input_contract_from_schema(Arc::new(Schema::new(vec![field])), resolve_crs);
        if atteso_ok {
            assert!(result.is_ok(), "{estensione}: {result:?}");
        } else {
            assert!(
                matches!(result, Err(PlenoraError::Unsupported(_))),
                "{estensione}: {result:?}"
            );
        }
    }
    let mut metadata = geometry_field(None).metadata().clone();
    metadata.insert("ARROW:extension:metadata".to_owned(), "[".to_owned());
    let field = geometry_field(None).with_metadata(metadata);
    let result =
        discover_input_contract_from_schema(Arc::new(Schema::new(vec![field])), resolve_crs);
    assert!(matches!(result, Err(PlenoraError::Schema(_))), "{result:?}");
}

#[test]
fn srid_is_a_signed_32_bit_integer() {
    // Vocabolario Arrow 1.0, sezione 3: `srid` e' un intero decimale con
    // segno a 32 bit.
    for (srid, atteso) in [("4326", Some(4326)), ("-1", Some(-1)), ("0", Some(0))] {
        let field = canonical_field(
            DataType::Binary,
            &[
                (PLENORA_GEOMETRY_SRID_KEY, srid),
                (PLENORA_GEOMETRY_CRS_RESOLUTION_KEY, "declared_unresolved"),
            ],
        );
        let keys = read_geometry_contract_keys(&field).expect(srid);
        assert_eq!(keys.srid, atteso, "{srid}");
    }
    for srid in ["2147483648", "-2147483649", "+4326", "4326.0", "", "-"] {
        let field = canonical_field(DataType::Binary, &[(PLENORA_GEOMETRY_SRID_KEY, srid)]);
        let result = read_geometry_contract_keys(&field);
        assert!(
            matches!(result, Err(PlenoraError::Crs(_))),
            "{srid}: {result:?}"
        );
    }
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
