//! Validazione del piano: accettazione allineata all'analisi dei contratti,
//! rifiuti prima dell'esecuzione.

mod comune;

use std::collections::BTreeSet;

use plenora_core::arrow::array::RecordBatch;
use plenora_core::arrow::schema::SchemaRef;
use plenora_core::catalog::{Family, ALIASES, CATALOG};
use plenora_core::contract::arrow_schema::contract_from_arrow_schema;
use plenora_core::contract::{DataContract, FieldAllocator};
use plenora_core::crs::resolve_crs;
use plenora_core::{PlenoraError, Result};
use plenora_kernels_geo::analyze::analyze_geo_contract;
use plenora_kernels_table::analyze::analyze_table_contract;
use plenora_pipeline::{Passo, Pipeline, PipelineValidata};
use serde_json::{json, Value};

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

fn valida_wide(pipeline: &Pipeline) -> Result<PipelineValidata> {
    pipeline.validate(&[("t", schema_wide())])
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
    let mut rifiutate = Vec::new();
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
        assert_eq!(
            validata.is_ok(),
            analisi.is_ok(),
            "{}: validazione {:?}, analisi {:?}",
            caso.op,
            validata.as_ref().err(),
            analisi.as_ref().err()
        );
        if let (Ok(validata), Ok(analisi)) = (&validata, &analisi) {
            let contratto = validata.contratto("uscita").expect("contratto dell'uscita");
            assert_eq!(
                format!("{contratto:?}"),
                format!("{analisi:?}"),
                "{}: contratto diverso dall'analisi diretta",
                caso.op
            );
        }
        if validata.is_err() {
            rifiutate.push(caso.op);
        }
    }
    // La sola operazione con schema d'uscita che dipende dai dati: `pivot`
    // del caso ha un `mapping`, che fissa le colonne.
    assert_eq!(rifiutate, ["table.transpose"]);
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
    assert!(valida_wide(&piano(&["t"], vec![], &["t"])).is_ok());
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
    assert!(valida_wide(&concat_a_due).is_ok());
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
        let atteso =
            analyze_geo_contract(descrittore.id, &[ingresso], &json!({}), None, &mut campi)
                .expect_err("l'analisi rifiuta");
        let ottenuto = valida_wide(&geo).expect_err("la validazione rifiuta");
        assert_eq!(ottenuto.category(), atteso.category(), "{}", descrittore.id);
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
    assert!(valida_wide(&pipeline).is_ok());
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
    assert!(valida_wide(&pipeline)
        .expect("CRS integrato")
        .crs_piano()
        .is_some());
    pipeline.crs = Some("EPSG:999999".to_owned());
    assert!(matches!(valida_wide(&pipeline), Err(PlenoraError::Crs(_))));
}

#[test]
fn la_provenance_per_riga_si_controlla_in_validazione() {
    // assert_not_null riporta indici di riga: dopo un sort non sono piu'
    // quelli della sorgente.
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
    assert!(matches!(
        valida_wide(&pipeline),
        Err(PlenoraError::InvalidPlan(_))
    ));
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
    assert!(valida_wide(&diretta).is_ok());
}

#[test]
fn i_contratti_di_una_catena_lunga_sono_quelli_dell_analisi_passo_per_passo() {
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
    let pipeline = piano(&["t"], catena.clone(), &["p7"]);
    let validata = valida_wide(&pipeline).expect("catena valida");

    let mut campi = FieldAllocator::default();
    let mut attesi = std::collections::BTreeMap::new();
    attesi.insert(
        "t".to_owned(),
        contract_from_arrow_schema(schema_wide(), resolve_crs).expect("contratto di input"),
    );
    for passo in &catena {
        let ingressi: Vec<DataContract> = passo
            .inputs
            .iter()
            .map(|nome| attesi[nome].clone())
            .collect();
        let uscita = analyze_table_contract(
            &passo.op,
            &ingressi,
            &passo.config,
            &mut campi,
            &plenora_kernels_table::Limits::default(),
        )
        .expect("analisi del passo");
        attesi.insert(passo.out.clone(), uscita);
    }
    for (nome, atteso) in &attesi {
        let ottenuto = validata.contratto(nome).expect("contratto del runner");
        assert_eq!(format!("{ottenuto:?}"), format!("{atteso:?}"), "{nome}");
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
    assert!(solo_input.validate(&[("t", schema_largo(massimo))]).is_ok());
}

#[test]
fn i_limiti_di_complessita_del_piano_si_applicano() {
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
    assert!(valida_wide(&catena(limiti.max_plan_depth)).is_ok());
    assert!(matches!(
        valida_wide(&catena(limiti.max_plan_depth + 1)),
        Err(PlenoraError::InvalidPlan(_))
    ));

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
    assert!(valida_wide(&ventaglio(limiti.max_fan_out)).is_ok());
    assert!(matches!(
        valida_wide(&ventaglio(limiti.max_fan_out + 1)),
        Err(PlenoraError::InvalidPlan(_))
    ));

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
            primo: ("table.drop_columns", json!({"columns": []})),
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
        let esito = pipeline.validate(&[("t", schema)]);
        assert!(
            matches!(
                &esito,
                Err(PlenoraError::InvalidPlan(messaggio)
                    | PlenoraError::ResourceLimit(messaggio))
                    if messaggio.contains("passo `secondo`") && messaggio.contains(frammento)
            ),
            "{op} {config}: atteso `{frammento}`, avuto {esito:?}"
        );
    }
}
