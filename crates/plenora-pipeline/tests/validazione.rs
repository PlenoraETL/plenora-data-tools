//! Validazione del piano: accettazione allineata all'analisi dei contratti,
//! rifiuti prima dell'esecuzione.

#[allow(dead_code)] // Questo target usa solo gli schemi dell'oracolo condiviso.
#[path = "comune/attesi.rs"]
mod attesi;
mod comune;

#[macro_use]
#[path = "../../../tests/comune/esito.rs"]
mod esito;
use plenora_core::{ErrorCategory as Categoria, ErrorPhase as Fase};

use std::collections::BTreeSet;

use plenora_core::arrow::array::RecordBatch;
use plenora_core::arrow::schema::SchemaRef;
use plenora_core::arrow::schema::{DataType, Field, Schema};
use plenora_core::catalog::{Family, ALIASES, CATALOG};
use plenora_core::contract::arrow_schema::contract_from_arrow_schema;
use plenora_core::contract::{
    ContractProperty, DataContract, FieldAllocator, FieldId, NullPlacement, PropertyConfidence,
    PropertyScope, SortDirection, SortOrder,
};
use plenora_core::crs::resolve_crs;
use plenora_core::{PlenoraError, Result};
use plenora_kernels_geo::analyze::analyze_geo_contract;
use plenora_kernels_table::analyze::analyze_table_contract;
use plenora_pipeline::{BaseIndici, Passo, Pipeline, PipelineValidata};
use serde_json::{json, Value};
use std::sync::Arc;

use comune::{nested, nomi_input, tabelle, wide, CASI};

fn passo(out: &str, op: &str, inputs: &[&str], config: Value) -> Passo {
    Passo {
        out: out.to_owned(),
        op: op.to_owned(),
        inputs: inputs.iter().map(|nome| (*nome).to_owned()).collect(),
        config,
    }
}

fn piano(inputs: &[&str], steps: Vec<Passo>, outputs: &[&str]) -> Pipeline {
    Pipeline {
        version: 1,
        inputs: inputs.iter().map(|nome| (*nome).to_owned()).collect(),
        crs: None,
        limits: None,
        steps,
        outputs: outputs.iter().map(|nome| (*nome).to_owned()).collect(),
    }
}

fn schema_wide() -> SchemaRef {
    wide(false).schema()
}

fn verifica_contratto(actual: &DataContract, expected: &DataContract) {
    assert_eq!(format!("{actual:?}"), format!("{expected:?}"));
}

fn valida_wide(pipeline: &Pipeline) -> Result<PipelineValidata> {
    pipeline.validate(&[("t", schema_wide())])
}

/// Contratto scritto per gli scenari wide che conservano colonne e metadati.
fn atteso_wide() -> DataContract {
    let mut atteso = DataContract::tabular(schema_wide());
    for (id, name) in [
        "id", "name", "value", "flag", "date", "date2", "json", "geom",
    ]
    .into_iter()
    .enumerate()
    {
        atteso
            .field_ids
            .insert(name.into(), FieldId(u32::try_from(id).expect("id fixture")));
    }
    atteso
}

/// Identita' degli scenari CASI: ingressi wide 0..8, destra 8..11,
/// nested 0..3. Rinomine e selezioni conservano gli ID; i valori derivati
/// ricevono ID nuovi dopo gli ingressi (contratto `FieldId` e schede Schema).
/// Nessuna geometria dichiarata: geom e' solo Binary, senza metadati geo.
fn contratto_catalogo_atteso(op: &str, ingressi: &[RecordBatch]) -> DataContract {
    let mut atteso = DataContract::tabular(attesi::tabella(op, ingressi).schema());
    let ids: Vec<u32> = match op {
        "table.aggregate" => vec![1, 8],
        "table.align_schema" => vec![0, 8],
        "table.select_columns" => vec![0, 2],
        "table.drop_columns" => (0..7).collect(),
        "table.reorder_columns" => vec![1, 0, 2, 3, 4, 5, 6, 7],
        "table.replace" | "table.text_normalize" => {
            vec![0, 8, 2, 3, 4, 5, 6, 7]
        }
        "table.type_cast" => vec![8, 1, 2, 3, 4, 5, 6, 7],
        "table.melt" | "table.pivot" => vec![0, 8, 9],
        "table.reconcile" => vec![11, 12],
        "table.table_diff" => vec![11, 12, 13, 14],
        "table.explode" => vec![0, 3, 2],
        "table.unnest" => vec![0, 1, 3, 4],
        "table.join" | "table.asof_join" => vec![0, 1, 2, 3, 4, 5, 6, 7, 9, 10],
        // Le altre fixture preservano tutte le colonne e aggiungono in coda
        // solo campi nuovi; cross/fuzzy includono l'intero lato destro.
        _ => (0..u32::try_from(atteso.schema.fields().len()).expect("fixture")).collect(),
    };
    assert_eq!(ids.len(), atteso.schema.fields().len(), "{op}");
    for (field, id) in atteso.schema.fields().iter().zip(ids) {
        atteso.field_ids.insert(field.name().clone(), FieldId(id));
    }
    // Nessun conteggio noto nello schema di ingresso. Solo sort e top_n
    // promettono un ordine in queste config: value discendente, null prima.
    if matches!(op, "table.sort" | "table.top_n") {
        atteso.properties.sorted_by = Some(ContractProperty::new(
            PropertyConfidence::Proven(SortOrder {
                keys: vec![FieldId(2)],
                direction: SortDirection::Descending,
                nulls: NullPlacement::First,
            }),
            PropertyScope::Stream,
        ));
    }
    // Scheda reconcile: cinque metriche anche da ingressi vuoti.
    if op == "table.reconcile" {
        atteso.properties.row_count = Some(ContractProperty::new(
            PropertyConfidence::Proven(5),
            PropertyScope::Dataset,
        ));
    }
    atteso
}

/// Analisi diretta, la stessa sequenza che il runner fa per un passo.
fn analisi_diretta(op: &str, schemi: &[SchemaRef], config: &Value) -> Result<DataContract> {
    let mut campi = FieldAllocator::default();
    let ingressi: Vec<DataContract> = schemi
        .iter()
        .map(|schema| contract_from_arrow_schema(schema.clone(), resolve_crs))
        .collect::<Result<_>>()?;
    analyze_table_contract(
        op,
        &ingressi,
        config,
        &mut campi,
        &plenora_kernels_table::Limits::default(),
    )
}

#[test]
fn ogni_operazione_tabellare_del_catalogo_ha_un_caso() {
    let casi: BTreeSet<&str> = CASI.iter().map(|caso| caso.op).collect();
    let catalogo: BTreeSet<&str> = CATALOG
        .iter()
        .filter(|descrittore| descrittore.family == Family::Table)
        .map(|descrittore| descrittore.id)
        .collect();
    assert_eq!(casi, catalogo);
    assert_eq!(casi.len(), CASI.len(), "un caso per operazione");
}

#[test]
fn la_validazione_accetta_se_e_solo_se_l_analisi_accetta() {
    // hmac_sha256 controlla in validazione che la chiave sia disponibile.
    std::env::set_var(comune::CHIAVE_HMAC, "chiave-di-test-del-runner");
    for caso in CASI {
        let config: Value = serde_json::from_str(caso.config).expect("config del caso");
        let tavole = tabelle(caso.fixture);
        let schemi: Vec<SchemaRef> = tavole.iter().map(RecordBatch::schema).collect();
        let nomi = nomi_input(tavole.len());
        let riferimenti: Vec<&str> = nomi.iter().map(String::as_str).collect();
        let pipeline = piano(
            &riferimenti,
            vec![passo("uscita", caso.op, &riferimenti, config.clone())],
            &["uscita"],
        );
        let forniti: Vec<(&str, SchemaRef)> = riferimenti
            .iter()
            .copied()
            .zip(schemi.iter().cloned())
            .collect();
        let validata = pipeline.validate(&forniti);
        let analisi = analisi_diretta(caso.op, &schemi, &config);
        if caso.op == "table.transpose" {
            assert_rifiuto!(validata, Categoria::Unsupported, Fase::Validate);
            assert_rifiuto!(analisi, Categoria::Unsupported, Fase::Validate);
        } else {
            let validata = validata.expect("fixture con schema statico valida");
            let analisi = analisi.expect("fixture con schema statico analizzabile");
            let contratto = validata.contratto("uscita").expect("contratto dell'uscita");
            let atteso = contratto_catalogo_atteso(caso.op, &tavole);
            assert_eq!(
                format!("{contratto:?}"),
                format!("{atteso:?}"),
                "runner {}",
                caso.op
            );
            assert_eq!(
                format!("{analisi:?}"),
                format!("{atteso:?}"),
                "analisi {}",
                caso.op
            );
        }
    }
}

#[test]
fn un_nome_ridefinito_si_rifiuta() {
    let pipeline = piano(
        &["t"],
        vec![
            passo("a", "table.limit", &["t"], json!({"n": 1})),
            passo("a", "table.limit", &["t"], json!({"n": 2})),
        ],
        &["a"],
    );
    assert!(matches!(
        valida_wide(&pipeline),
        Err(PlenoraError::InvalidPlan(_))
    ));
    let sovrascrive_input = piano(
        &["t"],
        vec![passo("t", "table.limit", &["t"], json!({"n": 1}))],
        &["t"],
    );
    assert!(matches!(
        valida_wide(&sovrascrive_input),
        Err(PlenoraError::InvalidPlan(_))
    ));
}

#[test]
fn un_riferimento_in_avanti_si_rifiuta() {
    let pipeline = piano(
        &["t"],
        vec![
            passo("a", "table.limit", &["b"], json!({"n": 1})),
            passo("b", "table.limit", &["t"], json!({"n": 2})),
        ],
        &["a"],
    );
    assert!(matches!(
        valida_wide(&pipeline),
        Err(PlenoraError::InvalidPlan(_))
    ));
}

#[test]
fn output_inesistenti_ripetuti_o_assenti_si_rifiutano() {
    for outputs in [&["x"][..], &["t", "t"][..], &[][..]] {
        let pipeline = piano(&["t"], vec![], outputs);
        assert!(
            matches!(valida_wide(&pipeline), Err(PlenoraError::InvalidPlan(_))),
            "{outputs:?}"
        );
    }
    // Un input restituito cosi' com'e' e' un piano valido.
    verifica_contratto(
        valida_wide(&piano(&["t"], vec![], &["t"]))
            .expect("identita")
            .contratto("t")
            .expect("t"),
        &DataContract::tabular(schema_wide()),
    );
}

#[test]
fn gli_schemi_devono_corrispondere_agli_input() {
    let pipeline = piano(&["t"], vec![], &["t"]);
    assert!(matches!(
        pipeline.validate(&[]),
        Err(PlenoraError::InvalidPlan(_))
    ));
    assert!(matches!(
        pipeline.validate(&[("t", schema_wide()), ("t", schema_wide())]),
        Err(PlenoraError::InvalidPlan(_))
    ));
    assert!(matches!(
        pipeline.validate(&[("t", schema_wide()), ("u", schema_wide())]),
        Err(PlenoraError::InvalidPlan(_))
    ));
}

#[test]
fn l_arieta_viene_dal_catalogo() {
    let unaria_con_due = piano(
        &["t"],
        vec![passo("a", "table.limit", &["t", "t"], json!({"n": 1}))],
        &["a"],
    );
    assert!(matches!(
        valida_wide(&unaria_con_due),
        Err(PlenoraError::InvalidPlan(_))
    ));
    let binaria_con_una = piano(
        &["t"],
        vec![passo("a", "table.except", &["t"], json!({}))],
        &["a"],
    );
    assert!(matches!(
        valida_wide(&binaria_con_una),
        Err(PlenoraError::InvalidPlan(_))
    ));
    // `table.concat` e' N-aria nel catalogo, ma il runner la esegue a due
    // input: con tre si rifiuta prima di eseguire, come a `190c493`.
    let concat_a_tre = piano(
        &["t"],
        vec![passo("a", "table.concat", &["t", "t", "t"], json!({}))],
        &["a"],
    );
    assert!(matches!(
        valida_wide(&concat_a_tre),
        Err(PlenoraError::Unsupported(_))
    ));
    let concat_a_due = piano(
        &["t"],
        vec![passo("a", "table.concat", &["t", "t"], json!({}))],
        &["a"],
    );
    verifica_contratto(
        valida_wide(&concat_a_due)
            .expect("concat")
            .contratto("a")
            .expect("a"),
        &atteso_wide(),
    );
}

#[test]
fn operazioni_sconosciute_e_alias_si_rifiutano_le_geo_come_l_analisi() {
    let sconosciuta = piano(
        &["t"],
        vec![passo("a", "table.non_esiste", &["t"], json!({}))],
        &["a"],
    );
    assert!(matches!(
        valida_wide(&sconosciuta),
        Err(PlenoraError::Unsupported(_))
    ));
    let prova = piano(
        &["t"],
        vec![passo("a", "table.__custom_test", &["t"], json!({}))],
        &["a"],
    );
    assert!(matches!(
        valida_wide(&prova),
        Err(PlenoraError::Unsupported(_))
    ));

    let (_, alias, canonico) = ALIASES
        .iter()
        .find(|(_, _, canonico)| *canonico == "table.add_row_number")
        .expect("alias legacy di table.add_row_number");
    assert_ne!(alias, canonico);
    let con_alias = piano(&["t"], vec![passo("a", alias, &["t"], json!({}))], &["a"]);
    let Err(PlenoraError::InvalidPlan(messaggio)) = valida_wide(&con_alias) else {
        panic!("un alias legacy si rifiuta come errore di piano");
    };
    assert!(messaggio.contains(canonico), "{messaggio}");

    // Le geo passano dall'analisi dei kernel: su una tabella senza
    // geometria e con la config vuota la validazione rifiuta come l'analisi.
    for descrittore in CATALOG.iter().filter(|d| d.family == Family::Geo) {
        let geo = piano(
            &["t"],
            vec![passo("a", descrittore.id, &["t"], json!({}))],
            &["a"],
        );
        let mut campi = FieldAllocator::default();
        let ingresso = contract_from_arrow_schema(schema_wide(), resolve_crs).expect("contratto");
        // Il corpus ha un solo ingresso privo di geometria: le binarie
        // rifiutano l'arieta', i produttori i campi richiesti mancanti;
        // from_coords usa x/y predefiniti, assenti nello schema.
        let categoria = if descrittore.arity == plenora_core::catalog::Arity::BinaryOrdered
            || matches!(descrittore.id, "geo.from_wkt" | "geo.generate_grid")
        {
            Categoria::InvalidPlan
        } else {
            Categoria::Schema
        };
        assert_rifiuto!(
            analyze_geo_contract(descrittore.id, &[ingresso], &json!({}), None, &mut campi),
            categoria,
            Fase::Validate
        );
        assert_rifiuto!(valida_wide(&geo), categoria, Fase::Validate);
    }
}

#[test]
fn versione_e_limiti_si_controllano() {
    let mut pipeline = piano(&["t"], vec![], &["t"]);
    pipeline.version = 2;
    assert!(matches!(
        valida_wide(&pipeline),
        Err(PlenoraError::InvalidPlan(_))
    ));

    let json_limiti = |limiti: &str| {
        format!(r#"{{"version":1,"inputs":["t"],"limits":{limiti},"steps":[],"outputs":["t"]}}"#)
    };
    let parziale =
        Pipeline::from_json(&json_limiti(r#"{"max_rows_per_edge": 7}"#)).expect("limiti parziali");
    let validata = valida_wide(&parziale).expect("limiti validi");
    assert_eq!(validata.limiti().rows.max_rows_per_edge, 7);
    assert_eq!(validata.limiti().rows.max_input_rows, 10_000_000);

    let a_zero =
        Pipeline::from_json(&json_limiti(r#"{"max_rows_per_edge": 0}"#)).expect("forma valida");
    assert!(matches!(
        valida_wide(&a_zero),
        Err(PlenoraError::InvalidPlan(_))
    ));
    // Un limite che il runner non applica non e' dichiarabile.
    assert!(matches!(
        Pipeline::from_json(&json_limiti(r#"{"max_parallelism": 4}"#)),
        Err(PlenoraError::InvalidPlan(_))
    ));
}

#[test]
fn il_json_rifiuta_campi_sconosciuti_e_chiavi_duplicate() {
    let valido = r#"{"version":1,"inputs":["t"],"steps":[
        {"out":"a","op":"table.limit","in":["t"],"config":{"n":1}}],"outputs":["a"]}"#;
    let pipeline = Pipeline::from_json(valido).expect("piano valido");
    verifica_contratto(
        valida_wide(&pipeline)
            .expect("limit")
            .contratto("a")
            .expect("a"),
        &atteso_wide(),
    );
    assert_eq!(
        Pipeline::from_json(&serde_json::to_string(&pipeline).expect("serializza"))
            .expect("rilettura"),
        pipeline
    );
    for testo in [
        r#"{"version":1,"inputs":["t"],"steps":[],"outputs":["t"],"extra":1}"#,
        r#"{"version":1,"inputs":["t"],"steps":[{"out":"a","op":"table.limit","in":["t"],"cfg":{}}],"outputs":["a"]}"#,
        r#"{"version":1,"version":1,"inputs":["t"],"steps":[],"outputs":["t"]}"#,
        r#"{"version":1,"inputs":["t"],"steps":[{"out":"a","op":"table.limit","in":["t"],"config":{"n":1,"n":2}}],"outputs":["a"]}"#,
    ] {
        assert!(
            matches!(
                Pipeline::from_json(testo),
                Err(PlenoraError::InvalidPlan(_))
            ),
            "{testo}"
        );
    }
}

#[test]
fn il_crs_di_piano_si_risolve_in_validazione() {
    let mut pipeline = piano(&["t"], vec![], &["t"]);
    pipeline.crs = Some("EPSG:4326".to_owned());
    let valida = valida_wide(&pipeline).expect("CRS integrato");
    let crs = valida.crs_piano().expect("CRS del piano");
    assert_eq!(crs.authority_srid(), Some(4326));
    assert_eq!(
        crs.authority_axis_order(),
        Some(plenora_core::contract::AxisOrder::LatLon)
    );
    assert_eq!(
        crs.normalized_gis_axis_order(),
        plenora_core::contract::AxisOrder::LonLat
    );
    pipeline.crs = Some("EPSG:999999".to_owned());
    assert_rifiuto!(valida_wide(&pipeline), Categoria::Crs, Fase::Validate);
}

#[test]
fn la_base_degli_indici_per_riga_si_decide_in_validazione() {
    // assert_not_null riporta indici di riga: dopo un sort non sono piu'
    // quelli della sorgente. La catena si accetta, e gli indici sono
    // dichiarati righe dell'ingresso del passo (tests/diagnostica_righe.rs).
    let pipeline = piano(
        &["t"],
        vec![
            passo("ordinata", "table.sort", &["t"], json!({"columns": ["id"]})),
            passo(
                "controllata",
                "table.assert_not_null",
                &["ordinata"],
                json!({"columns": ["id"]}),
            ),
        ],
        &["controllata"],
    );
    let validata = valida_wide(&pipeline).expect("sort -> assert_not_null accettata");
    assert_eq!(
        validata.base_indici("controllata"),
        Some(BaseIndici::SenzaAttribuzione)
    );
    assert_eq!(validata.base_indici("ordinata"), Some(BaseIndici::Sorgente));
    assert_eq!(validata.base_indici("t"), None);
    let diretta = piano(
        &["t"],
        vec![passo(
            "controllata",
            "table.assert_not_null",
            &["t"],
            json!({"columns": ["id"]}),
        )],
        &["controllata"],
    );
    assert_eq!(
        valida_wide(&diretta)
            .expect("valida")
            .base_indici("controllata"),
        Some(BaseIndici::Sorgente)
    );
}

/// Colonne, identita' e proprieta' della catena, scritte dalle schede.
fn contratti_attesi_catena() -> Vec<(&'static str, DataContract)> {
    let mut attesi = vec![("t", DataContract::tabular(schema_wide()))];
    let mut p1 = atteso_wide();
    let mut fields = schema_wide()
        .fields()
        .iter()
        .map(|f| f.as_ref().clone())
        .collect::<Vec<_>>();
    fields[1] = Field::new("label", DataType::Utf8, false);
    let metadata = plenora_core::arrow::Metadata::from([("origine", "test")]);
    p1.schema = Arc::new(Schema::new_with_metadata(fields.clone(), metadata.clone()));
    p1.field_ids.remove("name");
    p1.field_ids.insert("label".into(), FieldId(1));
    attesi.push(("p1", p1.clone()));
    fields.push(Field::new("doppio", DataType::Float64, true));
    let mut p2 = p1;
    p2.schema = Arc::new(Schema::new_with_metadata(fields, metadata.clone()));
    p2.field_ids.insert("doppio".into(), FieldId(8));
    attesi.push(("p2", p2.clone()));
    let mut p3 = p2;
    p3.properties.sorted_by = Some(ContractProperty::new(
        PropertyConfidence::Proven(SortOrder {
            keys: vec![FieldId(8)],
            direction: SortDirection::Ascending,
            nulls: NullPlacement::Last,
        }),
        PropertyScope::Stream,
    ));
    attesi.push(("p3", p3.clone()));
    let mut p4 = p3;
    // La scheda select_columns elimina l'ordine se scarta colonne.
    p4.properties.sorted_by = None;
    p4.schema = Arc::new(Schema::new_with_metadata(
        vec![
            Field::new("id", DataType::Int64, false),
            Field::new("label", DataType::Utf8, false),
            Field::new("doppio", DataType::Float64, true),
        ],
        metadata.clone(),
    ));
    p4.field_ids
        .retain(|name, _| ["id", "label", "doppio"].contains(&name.as_str()));
    attesi.push(("p4", p4.clone()));
    let mut p5 = p4;
    p5.properties.sorted_by = None;
    attesi.push(("p5", p5));
    let mut p6 = DataContract::tabular(Arc::new(Schema::new_with_metadata(
        vec![
            Field::new("label", DataType::Utf8, false),
            Field::new("doppio", DataType::Float64, true),
        ],
        metadata.clone(),
    )));
    p6.field_ids.insert("label".into(), FieldId(1));
    p6.field_ids.insert("doppio".into(), FieldId(9));
    attesi.push(("p6", p6.clone()));
    let mut p7 = p6;
    p7.schema = Arc::new(Schema::new_with_metadata(
        vec![
            Field::new("label", DataType::Utf8, false),
            Field::new("doppio", DataType::Float64, true),
            Field::new("lunghezza", DataType::Int64, true),
        ],
        metadata,
    ));
    p7.field_ids.insert("lunghezza".into(), FieldId(10));
    attesi.push(("p7", p7));
    attesi
}

#[test]
fn i_contratti_di_una_catena_lunga_corrispondono_allo_scenario() {
    let catena = vec![
        passo(
            "p1",
            "table.rename",
            &["t"],
            json!({"renames": [{"old_name": "name", "new_name": "label"}]}),
        ),
        passo(
            "p2",
            "table.formula",
            &["p1"],
            json!({"new_column": "doppio", "formula": "value * 2"}),
        ),
        passo("p3", "table.sort", &["p2"], json!({"columns": ["doppio"]})),
        passo(
            "p4",
            "table.select_columns",
            &["p3"],
            json!({"columns": ["id", "label", "doppio"]}),
        ),
        passo("p5", "table.concat", &["p4", "p4"], json!({})),
        passo(
            "p6",
            "table.aggregate",
            &["p5"],
            json!({"group_by": ["label"],
                   "aggregations": [{"column": "doppio", "function": "sum"}]}),
        ),
        passo(
            "p7",
            "table.string_length",
            &["p6"],
            json!({"column": "label", "output_column": "lunghezza"}),
        ),
    ];
    let pipeline = piano(&["t"], catena, &["p7"]);
    let validata = valida_wide(&pipeline).expect("catena valida");

    for (nome, atteso) in contratti_attesi_catena() {
        let ottenuto = validata.contratto(nome).expect("contratto del runner");
        verifica_contratto(ottenuto, &atteso);
    }
}

#[test]
fn le_chiavi_ripetute_si_rifiutano_a_ogni_profondita_della_config() {
    let testo = r#"{"version":1,"inputs":["t"],"steps":[{"out":"a","op":"table.sort",
        "in":["t"],"config":{"columns":["id"],"ascending":true,"ascending":false}}],
        "outputs":["a"]}"#;
    assert!(matches!(
        Pipeline::from_json(testo),
        Err(PlenoraError::InvalidPlan(_))
    ));
}

fn schema_largo(colonne: usize) -> SchemaRef {
    std::sync::Arc::new(plenora_core::arrow::schema::Schema::new(
        (0..colonne)
            .map(|indice| {
                plenora_core::arrow::schema::Field::new(
                    format!("c{indice}"),
                    plenora_core::arrow::schema::DataType::Int64,
                    false,
                )
            })
            .collect::<Vec<_>>(),
    ))
}

#[test]
fn le_colonne_oltre_il_limite_si_rifiutano_in_validazione() {
    let massimo = plenora_kernels_table::limiti_interni::MAX_COLUMNS;
    // Input al limite: una rinomina passa, una colonna in piu' no. Prima
    // l'errore arrivava dopo due passi eseguiti.
    let pipeline = piano(
        &["t"],
        vec![
            passo(
                "rinominata",
                "table.rename",
                &["t"],
                json!({"renames": [{"old_name": "c0", "new_name": "zero"}]}),
            ),
            passo(
                "numerata",
                "table.add_row_number",
                &["rinominata"],
                json!({"output_column": "riga"}),
            ),
        ],
        &["numerata"],
    );
    let Err(PlenoraError::ResourceLimit(messaggio)) =
        pipeline.validate(&[("t", schema_largo(massimo))])
    else {
        panic!("oltre max_columns si rifiuta in validazione");
    };
    assert!(messaggio.contains("numerata"), "{messaggio}");

    let solo_input = piano(&["t"], vec![], &["t"]);
    assert!(matches!(
        solo_input.validate(&[("t", schema_largo(massimo + 1))]),
        Err(PlenoraError::ResourceLimit(_))
    ));
    verifica_contratto(
        solo_input
            .validate(&[("t", schema_largo(massimo))])
            .expect("limite incluso")
            .contratto("t")
            .expect("t"),
        &DataContract::tabular(schema_largo(massimo)),
    );
}

#[test]
fn il_limite_di_profondita_del_piano_si_applica() {
    let limiti = plenora_core::limits::PlanLimits::default();
    let catena = |lunghezza: usize| {
        let passi: Vec<Passo> = (0..lunghezza)
            .map(|indice| {
                let sorgente = if indice == 0 {
                    "t".to_owned()
                } else {
                    format!("p{}", indice - 1)
                };
                passo(
                    &format!("p{indice}"),
                    "table.limit",
                    &[&sorgente],
                    json!({"n": 1}),
                )
            })
            .collect();
        let ultimo = format!("p{}", lunghezza - 1);
        piano(&["t"], passi, &[&ultimo])
    };
    {
        let validata = valida_wide(&catena(limiti.max_plan_depth)).expect("limite incluso");
        let atteso = atteso_wide();
        for i in 0..limiti.max_plan_depth {
            verifica_contratto(
                validata.contratto(&format!("p{i}")).expect("passo"),
                &atteso,
            );
        }
    };
    assert!(matches!(
        valida_wide(&catena(limiti.max_plan_depth + 1)),
        Err(PlenoraError::InvalidPlan(_))
    ));
}

#[test]
fn il_limite_di_fan_out_del_piano_si_applica() {
    let limiti = plenora_core::limits::PlanLimits::default();
    let ventaglio = |consumatori: usize| {
        let passi: Vec<Passo> = (0..consumatori)
            .map(|indice| {
                passo(
                    &format!("f{indice}"),
                    "table.limit",
                    &["t"],
                    json!({"n": 1}),
                )
            })
            .collect();
        piano(&["t"], passi, &["f0"])
    };
    {
        let validata = valida_wide(&ventaglio(limiti.max_fan_out)).expect("limite incluso");
        let atteso = atteso_wide();
        for i in 0..limiti.max_fan_out {
            let mut atteso = atteso.clone();
            for (position, field) in schema_wide().fields().iter().enumerate() {
                atteso.field_ids.insert(
                    field.name().clone(),
                    plenora_core::contract::FieldId(
                        u32::try_from(i * schema_wide().fields().len() + position)
                            .expect("id fixture"),
                    ),
                );
            }
            verifica_contratto(
                validata.contratto(&format!("f{i}")).expect("passo"),
                &atteso,
            );
        }
    };
    assert!(matches!(
        valida_wide(&ventaglio(limiti.max_fan_out + 1)),
        Err(PlenoraError::InvalidPlan(_))
    ));
}

#[test]
fn i_limiti_di_dimensione_del_piano_si_applicano() {
    let limiti = plenora_core::limits::PlanLimits::default();
    let nome_lungo = "n".repeat(limiti.max_identifier_bytes + 1);
    let con_nome_lungo = piano(
        &["t"],
        vec![passo(&nome_lungo, "table.limit", &["t"], json!({"n": 1}))],
        &[&nome_lungo],
    );
    assert!(matches!(
        valida_wide(&con_nome_lungo),
        Err(PlenoraError::InvalidPlan(_))
    ));

    let config_grande = piano(
        &["t"],
        vec![passo(
            "a",
            "table.lookup",
            &["t"],
            json!({"column": "name", "mapping": {"a": "x".repeat(limiti.max_config_bytes_per_node)}}),
        )],
        &["a"],
    );
    assert!(matches!(
        valida_wide(&config_grande),
        Err(PlenoraError::InvalidPlan(_))
    ));

    let troppi_input: Vec<String> = (0..=limiti.max_inputs).map(|i| format!("i{i}")).collect();
    let riferimenti: Vec<&str> = troppi_input.iter().map(String::as_str).collect();
    let schemi: Vec<(&str, SchemaRef)> = riferimenti
        .iter()
        .map(|nome| (*nome, schema_wide()))
        .collect();
    assert!(matches!(
        piano(&riferimenti, vec![], &["i0"]).validate(&schemi),
        Err(PlenoraError::InvalidPlan(_))
    ));
}

/// Un caso di config che l'analisi dei kernel rifiuta: il passo che la usa
/// e' il secondo del piano, e senza il rifiuto in validazione il primo
/// girerebbe e l'errore arriverebbe dopo (o non arriverebbe affatto, con un
/// parametro ignorato).
struct SecondoPasso {
    schema: SchemaRef,
    /// Primo passo: `(op, config)`.
    primo: (&'static str, Value),
    op: &'static str,
    config: Value,
    /// Secondo input del passo binario: l'input `t` del piano.
    binario: bool,
    /// Frammento del messaggio che identifica la regola: un rifiuto per un
    /// altro motivo non conta.
    frammento: &'static str,
}

fn caso(
    schema: &SchemaRef,
    op: &'static str,
    config: Value,
    frammento: &'static str,
) -> SecondoPasso {
    SecondoPasso {
        schema: schema.clone(),
        primo: ("table.add_row_number", json!({"output_column": "riga"})),
        op,
        config,
        binario: false,
        frammento,
    }
}

fn caso_binario(
    schema: &SchemaRef,
    op: &'static str,
    config: Value,
    frammento: &'static str,
) -> SecondoPasso {
    SecondoPasso {
        binario: true,
        ..caso(schema, op, config, frammento)
    }
}

/// Tabella con `colonne - 1` colonne Int64 e una colonna Utf8 `json`.
fn schema_largo_con_json(colonne: usize) -> SchemaRef {
    let mut campi: Vec<plenora_core::arrow::schema::Field> = schema_largo(colonne - 1)
        .fields()
        .iter()
        .map(|campo| campo.as_ref().clone())
        .collect();
    campi.push(plenora_core::arrow::schema::Field::new(
        "json",
        plenora_core::arrow::schema::DataType::Utf8,
        false,
    ));
    std::sync::Arc::new(plenora_core::arrow::schema::Schema::new(campi))
}

const TESTO: &str = "non leggibile come scalare testuale";

/// Ogni regola sulle config vive nell'analisi dei kernel
/// (`analyze_table_contract`), e il runner la applica passando da li': le
/// regole che il runner portava da solo (`verifica_config.rs`, rimosso),
/// quelle che prima scattavano solo in esecuzione, i parametri ignorati, e la
/// chiave HMAC, l'unico controllo d'ambiente rimasto nel runner. Ogni caso
/// fallisce al secondo passo, con il messaggio della sua regola.
#[test]
#[allow(clippy::too_many_lines)] // Un caso per regola.
fn cio_che_schemi_e_config_rendono_prevedibile_fallisce_in_validazione() {
    let w = schema_wide();
    let n = nested().schema();
    let una_colonna: SchemaRef =
        std::sync::Arc::new(plenora_core::arrow::schema::Schema::new(vec![
            plenora_core::arrow::schema::Field::new(
                "x",
                plenora_core::arrow::schema::DataType::Int64,
                false,
            ),
        ]));
    let massimo = plenora_kernels_table::limiti_interni::MAX_COLUMNS;
    let funzione = |nome: &str, args: Value| {
        json!({"output_column": "e",
               "expression": {"kind": "function", "name": nome, "args": args}})
    };
    let casi = vec![
        // Chiavi lette come testo.
        caso_binario(
            &n,
            "table.join",
            json!({"left_keys": ["lst"], "right_keys": ["lst"], "how": "inner"}),
            TESTO,
        ),
        caso_binario(
            &n,
            "table.semi_join",
            json!({"left_keys": ["lst"], "right_keys": ["lst"]}),
            TESTO,
        ),
        caso_binario(
            &n,
            "table.anti_join",
            json!({"left_keys": ["lst"], "right_keys": ["lst"]}),
            TESTO,
        ),
        caso_binario(
            &n,
            "table.asof_join",
            json!({"left_on": "id", "right_on": "id", "left_by": ["lst"], "right_by": ["lst"]}),
            TESTO,
        ),
        caso_binario(
            &n,
            "table.table_diff",
            json!({"left_keys": ["lst"], "right_keys": ["lst"]}),
            TESTO,
        ),
        caso_binario(
            &n,
            "table.table_diff",
            json!({"left_keys": ["id"], "right_keys": ["id"], "compare_columns": ["st"]}),
            "colonna `st`",
        ),
        caso_binario(
            &n,
            "table.assert_foreign_key",
            json!({"left_keys": ["lst"], "right_keys": ["lst"]}),
            TESTO,
        ),
        caso_binario(
            &n,
            "table.reconcile",
            json!({"left_keys": ["lst"], "right_keys": ["lst"]}),
            TESTO,
        ),
        // Liste di chiavi di lunghezza diversa.
        caso_binario(
            &w,
            "table.assert_foreign_key",
            json!({"left_keys": ["id", "name"], "right_keys": ["id"]}),
            "cardinalita' diversa",
        ),
        caso_binario(
            &w,
            "table.reconcile",
            json!({"left_keys": ["id"], "right_keys": ["id", "name"]}),
            "cardinalita' diversa",
        ),
        // La riga intera di distinct contiene una List.
        caso(&n, "table.distinct", json!({}), "colonna `lst`"),
        // Operatori testuali su colonne che non sono testo.
        caso(
            &n,
            "table.filter",
            json!({"column": "st", "operator": "contains", "value": "a"}),
            TESTO,
        ),
        caso(
            &n,
            "table.conditional",
            json!({"column": "lst", "conditions": [{"operator": "==", "value": "a", "result": 1}]}),
            TESTO,
        ),
        // Parametri che il target non usa.
        caso(
            &w,
            "table.type_cast",
            json!({"column": "id", "target_type": "str", "date_format": "%Y"}),
            "date_format ammesso solo",
        ),
        caso(
            &w,
            "table.type_cast",
            json!({"column": "id", "target_type": "int", "timezone": "UTC"}),
            "non ammessi per questo target_type",
        ),
        caso(
            &n,
            "table.explode",
            json!({"column": "lst", "output_column": "e", "empty_policy": "drop"}),
            "empty_policy=drop",
        ),
        // Impronta di niente: nessuna colonna ne' in config ne' nello schema.
        SecondoPasso {
            primo: ("table.drop_columns", json!({"columns": ["x"]})),
            ..caso(
                &una_colonna,
                "table.stable_fingerprint",
                json!({}),
                "almeno una colonna",
            )
        },
        caso(
            &w,
            "table.sha256_hash",
            json!({"columns": [], "output_column": "h"}),
            "columns vuoto",
        ),
        // Oltre max_columns.
        SecondoPasso {
            primo: ("table.limit", json!({"n": 1000})),
            ..caso(
                &schema_largo_con_json(massimo),
                "table.flatten_json",
                json!({"column": "json", "output_columns": ["json_a"]}),
                "supera max_columns",
            )
        },
        // Parametri ignorati e asserzioni vacue.
        caso(
            &w,
            "table.dedup_advanced",
            json!({"subset": ["id"], "ascending": false}),
            "ascending senza order_column",
        ),
        caso(
            &w,
            "table.dedup_advanced",
            json!({"subset": ["id"], "ascending": true}),
            "ascending senza order_column",
        ),
        caso(
            &w,
            "table.melt",
            json!({"id_columns": ["id"], "value_columns": ["value"],
                   "var_name": "x", "value_name": "x"}),
            "nomi distinti",
        ),
        caso(
            &w,
            "table.melt",
            json!({"id_columns": ["id"], "value_columns": ["value", "value"]}),
            "colonna ripetuta",
        ),
        caso(
            &w,
            "table.rename",
            json!({"renames": [{"old_name": "name", "new_name": "x"},
                               {"old_name": "name", "new_name": "y"}]}),
            "rename origine: colonna ripetuta",
        ),
        caso(
            &w,
            "table.aggregate",
            json!({"group_by": ["name"],
                   "aggregations": [{"column": "value", "function": "sum", "quantile": 0.5}]}),
            "quantile ammesso solo",
        ),
        caso(
            &w,
            "table.aggregate",
            json!({"group_by": ["name"],
                   "aggregations": [{"column": "value", "function": "sum", "separator": "|"}]}),
            "separator ammesso solo",
        ),
        caso(
            &w,
            "table.aggregate",
            json!({"group_by": ["name"],
                   "aggregations": [{"column": "value", "function": "mean", "ddof": 0}]}),
            "ddof ammesso solo",
        ),
        caso(
            &w,
            "table.mask_data",
            json!({"maskings": [{"column": "name", "mask_type": "email", "chars_start": 1}]}),
            "mask_type=custom",
        ),
        caso(
            &w,
            "table.fill_na",
            json!({"column": "name", "method": "ffill", "value": "x"}),
            "value ammesso solo",
        ),
        caso(
            &w,
            "table.window_function",
            json!({"column": "value", "function": "rank", "offset": 2}),
            "offset ammesso solo",
        ),
        caso(
            &w,
            "table.rolling_window",
            json!({"column": "value", "function": "sum", "window": 2, "ddof": 0,
                   "output_column": "r"}),
            "ddof ammesso solo",
        ),
        caso(
            &w,
            "table.assert_not_null",
            json!({"columns": []}),
            "columns vuoto",
        ),
        caso(
            &w,
            "table.assert_range",
            json!({"column": "value"}),
            "min o max",
        ),
        caso(
            &w,
            "table.assert_range",
            json!({"column": "value", "max": 10, "inclusive_min": false}),
            "senza l'estremo",
        ),
        caso(
            &w,
            "table.assert_cardinality",
            json!({}),
            "exact_rows, min_rows o max_rows",
        ),
        caso(
            &w,
            "table.conditional",
            json!({"column": "value", "conditions": [], "default_value": 0,
                   "output_column": "c"}),
            "almeno una condizione",
        ),
        caso(
            &w,
            "table.date_format",
            json!({"column": "date", "input_format": "%Y-%m-%d", "output_format": "",
                   "output_column": "d"}),
            "output_format non valido",
        ),
        caso(
            &w,
            "table.string_pad",
            json!({"column": "name", "width": 16 * 1024 * 1024 + 1, "fill_char": "0"}),
            "width oltre",
        ),
        caso(
            &w,
            "table.limit",
            json!({"n": u64::MAX}),
            "n oltre max_rows",
        ),
        caso(
            &w,
            "table.validate_rules",
            json!({"rules": [{"name": "r".repeat(1_025), "operator": "notnull",
                              "column": "id"}]}),
            "oltre 1024 byte",
        ),
        // Prima fallivano solo in esecuzione.
        caso(
            &w,
            "table.filter",
            json!({"column": "flag", "operator": ">", "value": 0}),
            "nessun confronto ordinato",
        ),
        caso(
            &n,
            "table.conditional",
            json!({"column": "st", "conditions": [{"operator": "between", "value": "0,1",
                   "result": 1}]}),
            "nessun confronto ordinato",
        ),
        caso(
            &w,
            "table.conditional",
            json!({"column": "value", "conditions": [{"operator": "==", "value": "abc",
                   "result": 1}]}),
            "valore non numerico",
        ),
        caso(
            &w,
            "table.date_add",
            json!({"column": "date", "input_format": "%Y-%m-%d", "amount": i64::MAX,
                   "unit": "days", "output_column": "d"}),
            "amount fuori scala",
        ),
        caso(
            &w,
            "table.expression",
            funzione(
                "lower",
                json!([{"kind": "column", "name": "name"}, {"kind": "column", "name": "name"}]),
            ),
            "numero di argomenti",
        ),
        caso(
            &w,
            "table.expression",
            funzione(
                "regex_replace",
                json!([{"kind": "column", "name": "name"}, {"kind": "literal", "value": "("},
                       {"kind": "literal", "value": "x"}]),
            ),
            "regex non valida",
        ),
        caso(
            &w,
            "table.expression",
            funzione(
                "substring",
                json!([{"kind": "column", "name": "name"}, {"kind": "literal", "value": -1}]),
            ),
            "start negativo",
        ),
        // Parametri che il kernel ignorava in silenzio.
        caso(
            &w,
            "table.add_row_number",
            json!({"output_column": "r", "ascending": false}),
            "ascending senza order_column",
        ),
        caso(
            &w,
            "table.string_extract",
            json!({"column": "name", "pattern": "(?P<l>[ab])", "output_column": "x"}),
            "output_column non ammesso",
        ),
        caso(
            &w,
            "table.string_extract",
            json!({"column": "name", "pattern": "(?P<l>[ab])", "extract_all": true}),
            "extract_all non ammesso",
        ),
        // Ambiente, non config: l'unico controllo rimasto nel runner.
        caso(
            &w,
            "table.hmac_sha256",
            json!({"columns": ["id"], "key_env": "PLENORA_PIPELINE_CHIAVE_ASSENTE"}),
            "chiave HMAC non disponibile",
        ),
    ];
    for SecondoPasso {
        schema,
        primo,
        op,
        config,
        binario,
        frammento,
    } in casi
    {
        let ingressi: &[&str] = if binario { &["primo", "t"] } else { &["primo"] };
        let pipeline = piano(
            &["t"],
            vec![
                passo("primo", primo.0, &["t"], primo.1),
                passo("secondo", op, ingressi, config.clone()),
            ],
            &["secondo"],
        );
        // Solo l'espansione oltre MAX_COLUMNS e' un limite di risorse;
        // gli altri scenari hanno parametri o schemi incompatibili.
        let categoria = if op == "table.flatten_json" && schema.fields().len() == massimo {
            Categoria::ResourceLimit
        } else {
            Categoria::InvalidPlan
        };
        // ResourceLimit non taggato conserva la fase derivata Write:
        // il contratto PlenoraError::phase dichiara questa approssimazione.
        let fase = if categoria == Categoria::ResourceLimit {
            Fase::Write
        } else {
            Fase::Validate
        };
        let errore = assert_rifiuto!(pipeline.validate(&[("t", schema)]), categoria, fase);
        let messaggio = errore.to_string();
        assert!(messaggio.contains("passo `secondo`") && messaggio.contains(frammento));
    }
}

/// Run-end e union, a qualunque profondita', si rifiutano al confine: i
/// kernel non li vedono mai (docs/runner.md, «Limiti dichiarati del runner»).
#[test]
fn run_end_e_union_si_rifiutano_a_ogni_profondita() {
    use std::sync::Arc;

    use plenora_core::arrow::schema::{DataType, Field, Schema, UnionFields, UnionMode};

    let run_end = DataType::RunEndEncoded(
        Arc::new(Field::new("run_ends", DataType::Int32, false)),
        Arc::new(Field::new("values", DataType::Utf8, true)),
    );
    let campi: UnionFields =
        std::iter::once((0_i8, Arc::new(Field::new("i", DataType::Int64, true)))).collect();
    let unione = |modo| DataType::Union(campi.clone(), modo);
    let lista = |figlio: DataType| DataType::List(Arc::new(Field::new("item", figlio, true)));
    let struttura =
        |figlio: DataType| DataType::Struct(vec![Field::new("dentro", figlio, true)].into());
    let dizionario =
        |valori: DataType| DataType::Dictionary(Box::new(DataType::Int32), Box::new(valori));
    for tipo in [
        run_end.clone(),
        unione(UnionMode::Sparse),
        unione(UnionMode::Dense),
        struttura(run_end.clone()),
        lista(unione(UnionMode::Dense)),
        lista(struttura(lista(run_end.clone()))),
        dizionario(run_end),
        dizionario(unione(UnionMode::Sparse)),
    ] {
        let schema: SchemaRef = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int64, false),
            Field::new("x", tipo.clone(), true),
        ]));
        let pipeline = piano(
            &["t"],
            vec![passo("u", "table.limit", &["t"], json!({"n": 1}))],
            &["u"],
        );
        let errore = pipeline
            .validate(&[("t", schema.clone())])
            .expect_err("tipo non supportato");
        assert!(
            matches!(&errore, PlenoraError::Unsupported(messaggio) if messaggio.contains("RunEndEncoded")),
            "{tipo:?}: {errore}"
        );
        // Lo stesso rifiuto per chi legge un contratto dallo schema.
        assert!(matches!(
            contract_from_arrow_schema(schema, resolve_crs),
            Err(PlenoraError::Unsupported(_))
        ));
    }
    // Una dictionary di testo e una lista di struct restano ammesse.
    let ammesso: SchemaRef = Arc::new(Schema::new(vec![
        Field::new("d", dizionario(DataType::Utf8), true),
        Field::new("l", lista(struttura(DataType::Int64)), true),
    ]));
    verifica_contratto(
        &contract_from_arrow_schema(ammesso.clone(), resolve_crs).expect("schema ammesso"),
        &DataContract::tabular(ammesso),
    );
}

/// La chiave HMAC si legge con la stessa funzione nel runner e nel kernel:
/// prima il runner (`var_os`) accettava un valore non UTF-8 che il kernel
/// (`env::var`) trattava come variabile assente, e l'errore arrivava al
/// passo. Ora il valore non UTF-8 si rifiuta in validazione, con la sua causa.
#[test]
fn la_chiave_hmac_non_utf8_si_rifiuta_in_validazione() {
    fn valore_non_utf8() -> std::ffi::OsString {
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStringExt;
            std::ffi::OsString::from_vec(vec![b'k', 0xff])
        }
        #[cfg(windows)]
        {
            use std::os::windows::ffi::OsStringExt;
            std::ffi::OsString::from_wide(&[u16::from(b'k'), 0xD800])
        }
    }
    const VARIABILE: &str = "PLENORA_PIPELINE_CHIAVE_NON_UTF8";
    std::env::set_var(VARIABILE, valore_non_utf8());
    let pipeline = piano(
        &["t"],
        vec![passo(
            "h",
            "table.hmac_sha256",
            &["t"],
            json!({"columns": ["id"], "key_env": VARIABILE}),
        )],
        &["h"],
    );
    let errore = assert_rifiuto!(
        valida_wide(&pipeline),
        Categoria::InvalidPlan,
        Fase::Validate
    );
    assert!(
        matches!(
            errore,
            PlenoraError::InvalidPlan(_) | PlenoraError::Tagged { .. }
        ),
        "{errore:?}"
    );
    let testo = errore.to_string();
    assert!(testo.contains("non UTF-8"), "{testo}");
    assert!(!testo.contains(VARIABILE), "{testo}");
    std::env::remove_var(VARIABILE);
}
