//! La superficie dell'SDK Python vista dalla tabella delle operazioni: il
//! documento delle capacità che `plenora_data.capabilities()` rende, la
//! sezione di `bindings/python-sdk-v1.json` che l'aggiornamento dei
//! contratti deve scrivere, e l'equivalenza fra le forme in memoria
//! dell'API (quelle che l'SDK chiama) e le forme da file (quelle della CLI).

mod comune;

use std::collections::BTreeSet;
use std::sync::Arc;

use comune::{contratto, Registro};
use plenora_cli::api;
use plenora_cli::capacita::{documento, documento_della, mappa_python, Superficie};
use plenora_core::arrow::array::{Int64Array, RecordBatch, StringArray};
use plenora_core::arrow::schema::{DataType, Field, Schema};
use plenora_io::{
    leggi_tabella, scrivi_tabella, FileIngresso, FileUscita, Ingresso, OpzioniScrittura,
};
use plenora_pipeline::{Interruzione, Pipeline};
use serde_json::{json, Value};

#[test]
fn le_capacita_dell_sdk_sono_quelle_della_cli_sulla_superficie_python() {
    let python = documento_della(Superficie::Python);
    Registro::dei_contratti()
        .valida("capabilities-v2.schema.json", &python)
        .unwrap_or_else(|motivo| panic!("capabilities-v2: {motivo}"));
    assert_eq!(
        python["interfaces"],
        json!([{
            "kind": "python_sdk",
            "contract": "plenora-python-sdk-v1",
            "version": 1,
            "artifact": "plenora-data",
        }])
    );
    // CAP-005 e CAP-007.
    let mut identita = BTreeSet::new();
    for operazione in python["operations"].as_array().expect("operations") {
        assert!(identita.insert((
            operazione["id"].as_str().expect("id"),
            operazione["version"].as_u64().expect("version"),
        )));
        assert_eq!(operazione["surfaces"], json!(["python_sdk"]));
    }
    // Tutto il resto è uguale al documento della CLI: stessi contratti,
    // tipi di contenuto, effetti, controlli e attributi (Python SDK 1.0,
    // sezione 12).
    let cli = documento();
    let senza_superficie = |documento: &Value| -> Vec<Value> {
        documento["operations"]
            .as_array()
            .expect("operations")
            .iter()
            .map(|operazione| {
                let mut operazione = operazione.clone();
                operazione
                    .as_object_mut()
                    .expect("operazione")
                    .remove("surfaces");
                operazione
            })
            .collect()
    };
    assert_eq!(senza_superficie(&python), senza_superficie(&cli));
    assert_eq!(python["component_version"], cli["component_version"]);
}

#[test]
fn la_mappa_python_e_una_sezione_valida_dei_binding_dei_contratti() {
    let mappa = mappa_python();
    // Al commit fissato i contratti non hanno ancora un artefatto Python per
    // questo componente: la mappa è ciò che il loro aggiornamento scriverà.
    let mut documento_python = contratto("python-sdk-v1.json");
    let componenti = documento_python["components"]
        .as_array_mut()
        .expect("components");
    let nostra = componenti
        .iter_mut()
        .find(|sezione| sezione["component"] == "plenora-data-tools")
        .expect("sezione di plenora-data-tools");
    assert_eq!(
        *nostra,
        json!({"component": "plenora-data-tools", "artifact": null,
               "discovery": [], "bindings": []}),
        "i contratti fissati hanno gia' la sezione Python: allineare la mappa"
    );
    *nostra = mappa.clone();
    Registro::dei_contratti()
        .valida("surface-bindings-v1.schema.json", &documento_python)
        .unwrap_or_else(|motivo| panic!("surface-bindings-v1: {motivo}"));

    assert_eq!(mappa["artifact"], "plenora-data / plenora_data");
    assert_eq!(
        mappa["discovery"],
        json!(["plenora_data.version", "plenora_data.capabilities"])
    );
    // Ogni operazione del catalogo pubblico, con la sua versione e il suo
    // requisito; ogni simbolo una volta sola (validate_specs.py dei
    // contratti: niente entrypoint ripetuti in un componente).
    let catalogo = contratto("data-tools-v1.json");
    let pubbliche = catalogo["operations"].as_array().expect("operations");
    let binding = mappa["bindings"].as_array().expect("bindings");
    assert_eq!(binding.len(), pubbliche.len());
    let mut simboli = BTreeSet::new();
    for pubblica in pubbliche {
        let voce = binding
            .iter()
            .find(|voce| voce["operation"] == pubblica["id"])
            .unwrap_or_else(|| panic!("{} senza binding", pubblica["id"]));
        assert_eq!(voce["version"], pubblica["version"]);
        assert_eq!(voce["requirement"], pubblica["requirement"]);
        let simboli_operazione = voce["entrypoints"].as_array().expect("entrypoints");
        // Una forma sincrona e una asincrona per ogni operazione.
        assert_eq!(simboli_operazione.len(), 2, "{}", pubblica["id"]);
        for simbolo in simboli_operazione {
            assert!(simboli.insert(simbolo.as_str().expect("simbolo").to_owned()));
        }
    }
}

fn tabella() -> RecordBatch {
    RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int64, false),
            Field::new("nome", DataType::Utf8, true),
        ])),
        vec![
            Arc::new(Int64Array::from(vec![3, 1, 2, 5])),
            Arc::new(StringArray::from(vec![
                Some("c"),
                None,
                Some("b"),
                Some("e"),
            ])),
        ],
    )
    .expect("tabella")
}

fn piano() -> Pipeline {
    Pipeline::from_json(
        r#"{"version": 1, "inputs": ["t"],
            "steps": [
              {"out": "alti", "op": "table.filter", "in": ["t"],
               "config": {"column": "id", "operator": ">", "value": 1}},
              {"out": "ordinati", "op": "table.sort", "in": ["alti"],
               "config": {"columns": ["id"]}}],
            "outputs": ["alti", "ordinati"]}"#,
    )
    .expect("piano")
}

#[test]
fn le_forme_in_memoria_dicono_cio_che_dicono_le_forme_da_file() {
    let cartella = tempfile::tempdir().expect("cartella temporanea");
    let sorgente = cartella.path().join("t.arrow");
    scrivi_tabella(&tabella(), &sorgente, &OpzioniScrittura::default()).expect("sorgente");
    let interruzione = Interruzione::default();
    let da_file = FileIngresso {
        nome: "t".to_owned(),
        percorso: sorgente.clone(),
        formato: None,
    };
    let in_memoria = || Ingresso::Tabella {
        nome: "t".to_owned(),
        tabella: tabella(),
    };

    assert_eq!(
        api::descrivi_tabella(&tabella(), &interruzione).expect("in memoria"),
        api::descrivi(&sorgente, &interruzione).expect("da file")
    );
    assert_eq!(
        api::valida_ingressi(&piano(), vec![in_memoria()], &interruzione).expect("in memoria"),
        api::valida(&piano(), std::slice::from_ref(&da_file), &interruzione).expect("da file")
    );

    let uscite: Vec<FileUscita> = ["alti", "ordinati"]
        .iter()
        .map(|nome| FileUscita {
            nome: (*nome).to_owned(),
            percorso: cartella.path().join(format!("{nome}.arrows")),
            formato: None,
        })
        .collect();
    let scritto = api::esegui(
        &piano(),
        std::slice::from_ref(&da_file),
        &uscite,
        &OpzioniScrittura::default(),
        &interruzione,
    )
    .expect("da file");
    let (documento, tabelle) =
        api::esegui_in_memoria(&piano(), vec![in_memoria()], &interruzione).expect("in memoria");
    // Stesso documento: gli output `.arrows` sono stream, come le tabelle
    // rese in memoria.
    assert_eq!(documento, scritto);
    assert_eq!(
        documento["outputs"][0]["content_type"],
        "application/vnd.apache.arrow.stream"
    );
    let nomi: Vec<&str> = tabelle.iter().map(|(nome, _)| nome.as_str()).collect();
    assert_eq!(nomi, ["alti", "ordinati"]);
    // Stesse tabelle, schema e metadati compresi.
    for ((_, resa), uscita) in tabelle.iter().zip(&uscite) {
        let riletta = leggi_tabella(&uscita.percorso, None, u64::MAX).expect("rilettura");
        assert_eq!(*resa, riletta, "{}", uscita.nome);
    }
    // Una sola tabella in memoria, mescolata a un input da file, segue le
    // stesse regole dei nomi.
    let doppio = api::esegui_in_memoria(
        &piano(),
        vec![in_memoria(), Ingresso::File(da_file)],
        &interruzione,
    )
    .expect_err("input dato due volte");
    assert_eq!(doppio.category(), plenora_core::ErrorCategory::InvalidPlan);
}

#[test]
fn una_tabella_in_memoria_conta_nel_budget_del_piano() {
    let piano = Pipeline::from_json(
        r#"{"version": 1, "inputs": ["t"], "limits": {"max_governed_memory_bytes": 16},
            "steps": [{"out": "u", "op": "table.filter", "in": ["t"],
                       "config": {"column": "id", "operator": ">", "value": 1}}],
            "outputs": ["u"]}"#,
    )
    .expect("piano");
    for errore in [
        api::valida_ingressi(
            &piano,
            vec![Ingresso::Tabella {
                nome: "t".to_owned(),
                tabella: tabella(),
            }],
            &Interruzione::default(),
        )
        .expect_err("oltre il budget"),
        api::esegui_in_memoria(
            &piano,
            vec![Ingresso::Tabella {
                nome: "t".to_owned(),
                tabella: tabella(),
            }],
            &Interruzione::default(),
        )
        .expect_err("oltre il budget"),
    ] {
        assert_eq!(
            errore.category(),
            plenora_core::ErrorCategory::ResourceLimit
        );
        assert_eq!(errore.phase(), plenora_core::ErrorPhase::Read);
        assert!(errore.to_string().contains("input `t`"), "{errore}");
    }
}

/// Una tabella in memoria è residente prima di qualunque lettura: un file
/// letto prima di lei ha come residuo il budget meno la tabella, e si
/// ferma lui (non la tabella dopo averlo caricato tutto).
#[test]
fn le_tabelle_in_memoria_si_riservano_prima_di_leggere_i_file() {
    let cartella = tempfile::tempdir().expect("cartella temporanea");
    let sorgente = cartella.path().join("a.arrow");
    scrivi_tabella(&tabella(), &sorgente, &OpzioniScrittura::default()).expect("sorgente");
    let vivi = plenora_core::memoria::byte_vivi(std::iter::once(&tabella())).expect("byte");
    // Basta per una tabella, non per due.
    let budget = vivi + vivi / 2;
    let piano = Pipeline::from_json(&format!(
        r#"{{"version": 1, "inputs": ["a", "b"], "limits": {{"max_governed_memory_bytes": {budget}}},
            "steps": [{{"out": "u", "op": "table.concat", "in": ["a", "b"], "config": {{}}}}],
            "outputs": ["u"]}}"#
    ))
    .expect("piano");
    let errore = api::esegui_in_memoria(
        &piano,
        vec![
            Ingresso::File(FileIngresso {
                nome: "a".to_owned(),
                percorso: sorgente,
                formato: None,
            }),
            Ingresso::Tabella {
                nome: "b".to_owned(),
                tabella: tabella(),
            },
        ],
        &Interruzione::default(),
    )
    .expect_err("oltre il budget");
    assert_eq!(
        errore.category(),
        plenora_core::ErrorCategory::ResourceLimit
    );
    assert_eq!(errore.phase(), plenora_core::ErrorPhase::Read);
    assert!(errore.to_string().contains("input `a`"), "{errore}");
}

#[test]
fn i_nomi_dati_dal_chiamante_si_verificano_senza_ripeterli() {
    let dichiarati = vec!["a".to_owned(), "b".to_owned()];
    assert!(api::verifica_nomi(&dichiarati, ["b", "a"].into_iter(), "inputs").is_ok());
    for dati in [vec!["a"], vec!["a", "a", "b"], vec!["a", "b", "SEGRETO"]] {
        let errore = api::verifica_nomi(&dichiarati, dati.into_iter(), "inputs")
            .expect_err("nomi sbagliati");
        assert_eq!(
            errore.category(),
            plenora_core::ErrorCategory::InvalidConfiguration
        );
        assert!(!errore.to_string().contains("SEGRETO"), "{errore}");
    }
}
