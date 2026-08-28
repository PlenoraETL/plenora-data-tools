//! Oracoli di difetti chiusi su engine e core.
//!
//! Ogni test qui sotto isola una proprieta' che un difetto reale ha
//! violato, e cade se quella proprieta' torna a mancare.

use std::collections::HashMap;
use std::sync::Arc;

use plenora_core::arrow::schema::{DataType, Field, Schema};
use plenora_core::contract::{
    ContractCrs, ContractProperties, DataContract, FieldAllocator, FieldId, GeometryColumnContract,
    GeometryDimensions,
};
use plenora_core::diagnostics::{
    RowDiagnosticScope, RowDiagnostics, RowDiagnosticsCompleteness, ROW_DIAGNOSTICS_CONTRACT,
    ROW_DIAGNOSTICS_INDEX_BASIS,
};
use plenora_core::limits::Limits;
use plenora_core::PlenoraError;
use plenora_engine::plan::PlanV5;
use plenora_engine::MemoryGovernor;

fn schema(fields: Vec<Field>) -> Arc<Schema> {
    Arc::new(Schema::new(fields))
}

fn geometry(name: &str) -> GeometryColumnContract {
    GeometryColumnContract {
        field_id: FieldId(0),
        name: name.to_owned(),
        crs: ContractCrs::Missing,
        dimensions: GeometryDimensions::Xy,
        encoding: None,
        nullable: true,
        types: GeometryColumnContract::undeclared_types(),
    }
}

// ---------------------------------------------------------------------------
// Collisione del plan_hash sugli interi oltre 2^53
// ---------------------------------------------------------------------------

fn plan_with(value: &str) -> String {
    format!(
        r#"{{"schema_version": 5, "inputs": ["main"], "output": "a",
            "nodes": [{{"id": "a", "op": "table.filter", "in": ["main"],
                        "config": {{"column": "id", "operator": ">", "value": {value}}}}}]}}"#
    )
}

#[test]
fn gli_interi_oltre_2_53_non_collassano_nel_piano_canonico() {
    let odd = PlanV5::parse_default(&plan_with("9007199254740993"))
        .expect("piano")
        .canonical_json();
    let exact = PlanV5::parse_default(&plan_with("9007199254740992"))
        .expect("piano")
        .canonical_json();
    assert_ne!(odd, exact);
    assert_eq!(
        odd["nodes"][0]["config"]["value"],
        serde_json::json!(9_007_199_254_740_993_u64)
    );
}

// ---------------------------------------------------------------------------
// Chiavi JSON duplicate rifiutate, non risolte «last wins»
// ---------------------------------------------------------------------------

#[test]
fn le_chiavi_duplicate_nel_piano_sono_rifiutate() {
    // Senza il controllo `serde_json` tiene l'ultima, e due testi diversi
    // producono lo stesso piano canonico e lo stesso `plan_hash`.
    let duplicated = r#"{"schema_version": 5, "inputs": ["main"], "output": "a",
        "nodes": [{"id": "a", "op": "table.filter", "in": ["main"],
                   "config": {"column": "id", "operator": ">", "value": 1, "value": 2}}]}"#;
    let error = PlanV5::parse_default(duplicated).expect_err("chiave duplicata");
    assert!(
        error.to_string().contains("duplicata"),
        "atteso il rifiuto della chiave duplicata, ottenuto: {error}"
    );
}

#[test]
fn il_json_malformato_resta_un_errore_di_mappatura() {
    // Il controllo sui duplicati non deve cambiare la categoria degli errori
    // di sintassi, che restano competenza della deserializzazione.
    let malformed = r#"{"schema_version": 5, "inputs": ["main"#;
    let error = PlanV5::parse_default(malformed).expect_err("json malformato");
    assert!(
        matches!(error, PlenoraError::DataMapping(_)),
        "atteso DataMapping, ottenuto: {error:?}"
    );
}

// ---------------------------------------------------------------------------
// Validazione del contratto geometrico
// ---------------------------------------------------------------------------

#[test]
fn una_geometria_non_binary_e_rifiutata_dal_contratto() {
    // Ogni lettore della geometria fa downcast a `BinaryArray`: un contratto
    // che dichiarasse geometrica una colonna di tipo diverso passerebbe il
    // dry-run e fallirebbe a runtime.
    let contract = DataContract {
        schema: schema(vec![Field::new("geom", DataType::Utf8, true)]),
        geometries: vec![geometry("geom")],
        active_geometry: None,
        properties: ContractProperties::default(),
    };
    let error = contract.validate().expect_err("geometria non Binary");
    assert!(error.to_string().contains("atteso Binary"), "{error}");
}

#[test]
fn i_nomi_di_campo_ripetuti_sono_rifiutati() {
    // Con un omonimo `field_with_name` risolve al primo, e il contratto puo'
    // descrivere una colonna diversa da quella che verra' letta.
    let contract = DataContract::tabular(schema(vec![
        Field::new("a", DataType::Int64, false),
        Field::new("a", DataType::Utf8, true),
    ]));
    let error = contract.validate().expect_err("nome ripetuto");
    assert!(error.to_string().contains("ripetuto"), "{error}");
}

// ---------------------------------------------------------------------------
// Limiti fuori dominio rifiutati, non corretti in silenzio
// ---------------------------------------------------------------------------

#[test]
fn spill_partitions_sotto_il_minimo_e_rifiutato() {
    let limits = Limits {
        spill_partitions: 1,
        ..Limits::default()
    };
    let error = limits.validate().expect_err("sotto il minimo");
    assert!(error.to_string().contains("spill_partitions"), "{error}");
    assert!(Limits::default().validate().is_ok());
}

#[test]
fn un_piano_con_spill_partitions_invalido_non_viene_corretto() {
    // Un `.max(2)` nel preparer fa eseguire il piano con limiti diversi da
    // quelli dichiarati, senza che nulla lo segnali.
    let plan = r#"{"schema_version": 5, "inputs": ["main"], "output": "a",
        "limits": {"spill_partitions": 1},
        "nodes": [{"id": "a", "op": "table.filter", "in": ["main"],
                   "config": {"column": "id", "operator": ">", "value": 1}}]}"#;
    let contract = DataContract::tabular(schema(vec![Field::new("id", DataType::Int64, false)]));
    let error = plenora_engine::planner::validate(plan, &[("main".to_owned(), contract)])
        .expect_err("limite fuori dominio");
    assert!(error.to_string().contains("spill_partitions"), "{error}");
}

// ---------------------------------------------------------------------------
// Contabilita' della memoria senza overflow
// ---------------------------------------------------------------------------

#[test]
fn il_governor_non_avvolge_ne_va_in_panico_sui_byte_estremi() {
    // `fetch_add(bytes) + bytes` pubblica l'addendo e poi somma in
    // aritmetica non controllata: con `overflow-checks` attivo e' un panico,
    // e il contatore resta corrotto.
    let governor = MemoryGovernor::new(1_024);
    let error = governor
        .try_reserve(u64::MAX, "test")
        .expect_err("oltre il budget");
    assert!(
        error.to_string().contains("max_governed_memory_bytes"),
        "{error}"
    );
    // Il contatore non e' stato sporcato: una prenotazione legittima passa.
    assert!(governor.try_reserve(512, "test").is_ok());
}

// ---------------------------------------------------------------------------
// Limiti, coerenza del contratto, reticolo della completeness
// ---------------------------------------------------------------------------

#[test]
fn i_limiti_nulli_e_il_fattore_non_finito_sono_rifiutati() {
    // `Limits::validate` deve coprire ogni regola, non il solo
    // `spill_partitions`: le regole che vivessero nel motore tabellare
    // legacy non varrebbero per i piani solo-geo, che non lo attraversano.
    let zero = |mutate: fn(&mut Limits)| {
        let mut limits = Limits::default();
        mutate(&mut limits);
        limits
            .validate()
            .expect_err("limite fuori dominio")
            .to_string()
    };
    assert!(zero(|l| l.max_governed_memory_bytes = 0).contains("max_governed_memory_bytes"));
    assert!(zero(|l| l.rows.max_input_rows = 0).contains("max_input_rows"));
    assert!(zero(|l| l.max_batches = 0).contains("max_batches"));
    assert!(zero(|l| l.max_string_bytes = 0).contains("max_string_bytes"));
    // `spill_partitions` ha ora anche un massimo, come il motore legacy.
    assert!(zero(|l| l.spill_partitions = 100_000).contains("spill_partitions"));
    // `NaN` renderebbe falso ogni confronto di espansione, aprendo il limite
    // invece di chiuderlo: non e' costruibile da JSON, ma lo e' via API Rust.
    assert!(zero(|l| l.rows.max_expansion_factor = f64::NAN).contains("max_expansion_factor"));
    assert!(zero(|l| l.rows.max_expansion_factor = 0.0).contains("max_expansion_factor"));
}

#[test]
fn il_contratto_rifiuta_un_encoding_incoerente_coi_metadati() {
    // Il contratto dichiara `wkb`, la colonna dichiara `ewkb`: senza il
    // controllo di coerenza l'errore arriva a meta' esecuzione, dopo aver
    // aperto gli input.
    let mut metadata = HashMap::new();
    metadata.insert(
        plenora_core::contract::PLENORA_GEOMETRY_ENCODING_KEY.to_owned(),
        "ewkb".to_owned(),
    );
    let contract = DataContract {
        schema: Arc::new(Schema::new(vec![Field::new(
            "geom",
            DataType::Binary,
            true,
        )
        .with_metadata(metadata)])),
        geometries: vec![GeometryColumnContract {
            encoding: Some(plenora_core::contract::GeometryEncoding::Wkb),
            ..geometry("geom")
        }],
        active_geometry: None,
        properties: ContractProperties::default(),
    };
    let error = contract.validate().expect_err("encoding incoerente");
    assert!(
        error.to_string().contains("encoding del contratto"),
        "{error}"
    );
}

#[test]
fn il_contratto_rifiuta_dimensioni_incoerenti_coi_metadati() {
    let mut metadata = HashMap::new();
    metadata.insert(
        plenora_core::contract::PLENORA_GEOMETRY_DIMENSIONS_KEY.to_owned(),
        "xyz".to_owned(),
    );
    let contract = DataContract {
        schema: Arc::new(Schema::new(vec![Field::new(
            "geom",
            DataType::Binary,
            true,
        )
        .with_metadata(metadata)])),
        geometries: vec![geometry("geom")], // dichiara `xy`
        active_geometry: None,
        properties: ContractProperties::default(),
    };
    let error = contract.validate().expect_err("dimensioni incoerenti");
    assert!(error.to_string().contains("dimensions"), "{error}");
}

#[test]
fn la_fusione_dei_report_non_migliora_la_conoscenza() {
    // `Unknown` che ricevesse `Partial` diventando `Partial` MIGLIOREREBBE
    // l'informazione con la fusione, e il risultato dipenderebbe
    // dall'ordine.
    let base = |completeness| RowDiagnostics {
        contract: ROW_DIAGNOSTICS_CONTRACT.to_owned(),
        scope: RowDiagnosticScope::Read,
        index_basis: ROW_DIAGNOSTICS_INDEX_BASIS.to_owned(),
        completeness,
        knowledge_limits: None,
        observed_total: 1,
        total: Some(1),
        input_total: None,
        counts: std::collections::BTreeMap::new(),
        examples_limit: 10,
        examples_truncated: false,
        examples: Vec::new(),
        diagnostic_state_counts: None,
        write_outcome: None,
    };

    let fondi = |primo, secondo| {
        let mut aggregate = Some(base(primo));
        RowDiagnostics::merge_into(&mut aggregate, base(secondo), 0).expect("fusione");
        aggregate.expect("report").completeness
    };
    // Il risultato non dipende dall'ordine, ed e' sempre il peggiore dei due.
    assert_eq!(
        fondi(
            RowDiagnosticsCompleteness::Unknown,
            RowDiagnosticsCompleteness::Partial
        ),
        RowDiagnosticsCompleteness::Unknown
    );
    assert_eq!(
        fondi(
            RowDiagnosticsCompleteness::Partial,
            RowDiagnosticsCompleteness::Unknown
        ),
        RowDiagnosticsCompleteness::Unknown
    );
    assert_eq!(
        fondi(
            RowDiagnosticsCompleteness::Complete,
            RowDiagnosticsCompleteness::Partial
        ),
        RowDiagnosticsCompleteness::Partial
    );
    assert_eq!(
        fondi(
            RowDiagnosticsCompleteness::Complete,
            RowDiagnosticsCompleteness::Complete
        ),
        RowDiagnosticsCompleteness::Complete
    );
    // Anche il declassamento rispetta il reticolo: `Unknown` non torna
    // `Partial`.
    assert_eq!(
        base(RowDiagnosticsCompleteness::Unknown)
            .into_partial("data_tools.test")
            .completeness,
        RowDiagnosticsCompleteness::Unknown
    );
}

// ---------------------------------------------------------------------------
// `into_partial` unisce i limiti, non li sostituisce
// ---------------------------------------------------------------------------

#[test]
fn il_declassamento_a_parziale_non_cancella_i_limiti_gia_noti() {
    let mut report = RowDiagnostics {
        contract: ROW_DIAGNOSTICS_CONTRACT.to_owned(),
        scope: RowDiagnosticScope::Read,
        index_basis: ROW_DIAGNOSTICS_INDEX_BASIS.to_owned(),
        completeness: RowDiagnosticsCompleteness::Complete,
        knowledge_limits: Some(vec!["data_tools.primo".to_owned()]),
        observed_total: 1,
        total: Some(1),
        input_total: None,
        counts: std::collections::BTreeMap::new(),
        examples_limit: 10,
        examples_truncated: false,
        examples: Vec::new(),
        diagnostic_state_counts: None,
        write_outcome: None,
    };
    report = report.into_partial("data_tools.secondo");
    assert_eq!(
        report.knowledge_limits.as_deref(),
        Some(
            [
                "data_tools.primo".to_owned(),
                "data_tools.secondo".to_owned()
            ]
            .as_slice()
        ),
        "il limite preesistente veniva sostituito, perdendo un motivo di incompletezza"
    );
    // Un limite gia' presente non si duplica.
    report = report.into_partial("data_tools.primo");
    assert_eq!(report.knowledge_limits.map(|limits| limits.len()), Some(2));
}

// ---------------------------------------------------------------------------
// `-0.0` non collassa su `0` nel piano canonico
// ---------------------------------------------------------------------------

#[test]
fn lo_zero_negativo_resta_distinto_dallo_zero_nel_piano_canonico() {
    // `-0.0 == 0.0` e `(-0.0).trunc() == -0.0`: la scorciatoia «se e' intero
    // emettilo come intero» trasforma `-0.0` in `0`, e due piani con segni
    // diversi ottengono lo stesso `plan_hash`. Il segno e' osservabile
    // (divisione, atan2, formattazione): non e' un dettaglio da normalizzare.
    let negativo = PlanV5::parse_default(&plan_with("-0.0"))
        .expect("piano")
        .canonical_json();
    let positivo = PlanV5::parse_default(&plan_with("0"))
        .expect("piano")
        .canonical_json();
    assert_ne!(negativo, positivo);
}

// ---------------------------------------------------------------------------
// Una chiave canonica PRESENTE dev'essere valida
// anche quando il contratto non dichiara il lato tipizzato
// ---------------------------------------------------------------------------

fn contract_with_metadata(pairs: &[(&str, &str)]) -> DataContract {
    let metadata: HashMap<String, String> = pairs
        .iter()
        .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
        .collect();
    DataContract {
        schema: schema(vec![
            Field::new("geom", DataType::Binary, true).with_metadata(metadata)
        ]),
        geometries: vec![geometry("geom")],
        active_geometry: None,
        properties: ContractProperties::default(),
    }
}

#[test]
fn le_chiavi_canoniche_malformate_sono_rifiutate_anche_senza_lato_tipizzato() {
    // `geometry("geom")` NON dichiara l'encoding e NON dichiara i tipi: un
    // controllo limitato ai casi in cui entrambe le fonti parlano lascia
    // passare indisturbata una chiave presente ma illeggibile. «Assente» e
    // «presente ma malformata» sono stati diversi.
    let casi: [(&str, &str, &str); 4] = [
        (
            plenora_core::contract::PLENORA_GEOMETRY_ENCODING_KEY,
            "twkb",
            "encoding canonico",
        ),
        (
            plenora_core::contract::PLENORA_GEOMETRY_CRS_RESOLUTION_KEY,
            "forse",
            "crs_resolution canonica",
        ),
        (
            plenora_core::contract::PLENORA_GEOMETRY_TYPES_DECLARATION_KEY,
            "quasi_tutti",
            "types_declaration canonica",
        ),
        (
            plenora_core::contract::PLENORA_GEOMETRY_TYPES_KEY,
            "point",
            "types senza types_declaration",
        ),
    ];
    for (key, value, atteso) in casi {
        let error = contract_with_metadata(&[(key, value)])
            .validate()
            .expect_err("chiave canonica malformata accettata");
        assert!(error.to_string().contains(atteso), "{key}={value}: {error}");
    }
    // L'elenco dei tipi si valida col suo dichiarante: fuori ordine e' invalido
    // quanto un tipo ignoto.
    let error = contract_with_metadata(&[
        (
            plenora_core::contract::PLENORA_GEOMETRY_TYPES_DECLARATION_KEY,
            "exact",
        ),
        (
            plenora_core::contract::PLENORA_GEOMETRY_TYPES_KEY,
            "point,point",
        ),
    ])
    .validate()
    .expect_err("elenco con duplicati");
    assert!(error.to_string().contains("elenco dei tipi"), "{error}");

    // Le stesse chiavi, ben formate, restano accettate: la regola aggiunge un
    // controllo, non rifiuta i contratti validi.
    contract_with_metadata(&[
        (plenora_core::contract::PLENORA_GEOMETRY_ENCODING_KEY, "wkb"),
        (
            plenora_core::contract::PLENORA_GEOMETRY_CRS_RESOLUTION_KEY,
            "missing",
        ),
        (
            plenora_core::contract::PLENORA_GEOMETRY_TYPES_DECLARATION_KEY,
            "exact",
        ),
        (
            plenora_core::contract::PLENORA_GEOMETRY_TYPES_KEY,
            "point,linestring",
        ),
    ])
    .validate()
    .expect("chiavi canoniche valide");
}

// ---------------------------------------------------------------------------
// Allocazione dei FieldId fallibile
// ---------------------------------------------------------------------------

#[test]
fn l_allocatore_dei_field_id_fallisce_invece_di_ripetere_l_ultimo() {
    // Con un incremento saturante, a fondo scala l'allocatore renderebbe
    // ripetutamente lo stesso id: due colonne diverse con la stessa
    // identita'.
    let mut allocator = FieldAllocator::new(u32::MAX - 1);
    assert_eq!(
        allocator.alloc().expect("ultimo id disponibile"),
        FieldId(u32::MAX - 1)
    );
    let error = allocator.alloc().expect_err("spazio esaurito");
    assert!(error.to_string().contains("FieldId"), "{error}");
    // Anche `intern` e `derive` propagano l'esaurimento invece di duplicare.
    assert!(allocator.intern("a").is_err());
    assert!(allocator.derive("b").is_err());
}

// ---------------------------------------------------------------------------
// Profilo stretto degli input (contratto obbligatorio)
// ---------------------------------------------------------------------------

#[test]
// Il test verifica proprio il percorso permissivo deprecato: finche' esiste,
// il suo comportamento e' parte del contratto pubblico.
#[allow(deprecated)]
fn il_profilo_stretto_rifiuta_un_input_senza_contratto() {
    use plenora_core::arrow::array::{Int64Array, RecordBatch};
    use plenora_engine::executor::{Input, Inputs};

    let colonna = Arc::new(Int64Array::from(vec![1_i64, 2, 3]));
    let batch = RecordBatch::try_new(
        schema(vec![Field::new("id", DataType::Int64, false)]),
        vec![colonna],
    )
    .expect("batch");
    let input = || Input::from_batches(vec![batch.clone()]).expect("input");

    // Permissivo: passa, ed e' il comportamento storico che non si rompe.
    assert!(!Inputs::new().is_strict());
    Inputs::new()
        .with("main", input())
        .expect("profilo permissivo");

    // Stretto: l'ingresso senza contratto non e' ammesso, e l'errore lo dice.
    assert!(Inputs::strict().is_strict());
    let mut stretti = Inputs::strict();
    let error = stretti
        .add("main", input())
        .expect_err("input senza contratto nel profilo stretto");
    assert!(error.to_string().contains("senza contratto"), "{error}");

    // Con il contratto passa, e il duplicato resta rifiutato come sempre.
    let contract = DataContract {
        schema: schema(vec![Field::new("id", DataType::Int64, false)]),
        geometries: Vec::new(),
        active_geometry: None,
        properties: ContractProperties::default(),
    };
    stretti
        .add_with_contract("main", input(), contract.clone())
        .expect("input con contratto");
    assert!(
        stretti
            .add_with_contract("main", input(), contract)
            .is_err(),
        "il nome duplicato resta un errore anche nel profilo stretto"
    );
}
