//! Scoperta: `--help`, `--version`, `capabilities`, `catalog`, dal binario
//! come sottoprocesso (CLI 2.0, sezioni 3, 4 e 10; Capability Discovery
//! 2.0; Public Catalogs 1.0, sezione 6; Surface Bindings 1.0, sezione 3).

mod comune;

use std::collections::BTreeSet;

use comune::{contratto, invoca, processo, provenienza_completa, Registro};
use plenora_core::catalog::CATALOG;
use serde_json::Value;

#[test]
fn i_file_dei_contratti_sono_quelli_del_commit_fissato() {
    provenienza_completa();
}

#[test]
fn version_usa_l_inviluppo_di_successo() {
    for argomenti in [
        &["--version", "--format", "json"][..],
        &["--format", "json", "--version"][..],
        &["--version"][..],
    ] {
        let esito = invoca(argomenti);
        assert_eq!(esito.codice, 0);
        let documento = &esito.documento;
        assert_eq!(documento["status"], "ok");
        assert_eq!(documento["protocol_version"], 2);
        assert_eq!(documento["component"], "plenora-data-tools");
        assert_eq!(documento["component_version"], env!("CARGO_PKG_VERSION"));
        assert_eq!(documento["command"], "version");
        assert_eq!(documento["result"]["protocol_version"], 2);
        assert_eq!(
            documento["result"]["component_version"],
            env!("CARGO_PKG_VERSION")
        );
    }
}

#[test]
fn help_testo_e_json_elencano_solo_i_comandi_del_binario() {
    let uscita = processo(&["--help"]);
    assert!(uscita.status.success());
    assert!(uscita.stderr.is_empty());
    let testo = String::from_utf8(uscita.stdout).expect("UTF-8");
    let esito = invoca(&["--help", "--format", "json"]);
    assert_eq!(esito.codice, 0);
    assert_eq!(esito.documento["result"]["usage"], testo.as_str());
    let elencati: BTreeSet<String> = esito.documento["result"]["commands"]
        .as_array()
        .expect("commands")
        .iter()
        .map(|voce| voce["command"].as_str().expect("command").to_owned())
        .collect();
    // I comandi canonici del binding CLI dei contratti per plenora-data, più
    // la scoperta: né uno di più né uno di meno.
    let binding = contratto("cli-v1.json");
    let componente = binding["components"]
        .as_array()
        .expect("components")
        .iter()
        .find(|voce| voce["component"] == "plenora-data-tools")
        .expect("plenora-data-tools nel binding");
    assert_eq!(componente["artifact"], "plenora-data");
    let mut attesi: BTreeSet<String> = ["--help", "--version", "capabilities"]
        .iter()
        .map(|comando| (*comando).to_owned())
        .collect();
    for voce in componente["bindings"].as_array().expect("bindings") {
        for ingresso in voce["entrypoints"].as_array().expect("entrypoints") {
            let comando = ingresso
                .as_str()
                .and_then(|testo| testo.split_whitespace().next())
                .expect("comando");
            attesi.insert(comando.to_owned());
            assert!(
                testo.contains(&format!("plenora-data {comando}")),
                "{comando}"
            );
        }
    }
    assert_eq!(elencati, attesi);
}

#[test]
fn capabilities_descrive_il_binario_che_risponde() {
    let esito = invoca(&["capabilities", "--format", "json"]);
    assert_eq!(esito.codice, 0);
    assert_eq!(esito.documento["contract"], "plenora-capabilities-v2");
    let risultato = &esito.documento["result"];
    let registro = Registro::dei_contratti();
    registro
        .valida("capabilities-v2.schema.json", risultato)
        .unwrap_or_else(|motivo| panic!("capabilities-v2: {motivo}"));
    assert_eq!(risultato["component_version"], env!("CARGO_PKG_VERSION"));
    // CAP-005 e CAP-007 (tools/conformance_checks.py dei contratti).
    let interfacce: BTreeSet<&str> = risultato["interfaces"]
        .as_array()
        .expect("interfaces")
        .iter()
        .map(|interfaccia| interfaccia["kind"].as_str().expect("kind"))
        .collect();
    assert_eq!(interfacce, BTreeSet::from(["cli"]));
    let operazioni = risultato["operations"].as_array().expect("operations");
    let mut identita = BTreeSet::new();
    for operazione in operazioni {
        assert!(
            identita.insert((
                operazione["id"].as_str().expect("id"),
                operazione["version"].as_u64().expect("version")
            )),
            "CAP-005: identita' ripetuta"
        );
        for superficie in operazione["surfaces"].as_array().expect("surfaces") {
            assert!(
                interfacce.contains(superficie.as_str().expect("surface")),
                "CAP-007"
            );
        }
    }
    // Il catalogo pubblico v2: stesse operazioni, versioni, contratti, tipi
    // di contenuto, controlli ed effetti, senza eccezioni.
    let catalogo = contratto("data-tools-v2.json");
    let pubbliche = catalogo["operations"].as_array().expect("operations");
    assert_eq!(pubbliche.len(), operazioni.len());
    for pubblica in pubbliche {
        let nostra = operazioni
            .iter()
            .find(|operazione| operazione["id"] == pubblica["id"])
            .unwrap_or_else(|| panic!("{} assente", pubblica["id"]));
        assert_eq!(nostra["version"], pubblica["version"]);
        for lato in ["input", "output"] {
            assert_eq!(nostra[lato]["contract"], pubblica[lato]["contract"]);
            assert_eq!(
                nostra[lato]["content_types"],
                pubblica[lato]["content_types"]
            );
        }
        assert_eq!(nostra["controls"], pubblica["controls"]);
        assert_eq!(nostra["side_effect"], pubblica["side_effect"], "{}", pubblica["id"]);
        // Gli attributi del catalogo pubblico, con i loro valori; i nostri
        // aggiungono solo il contratto degli attributi.
        let mut attributi = nostra["attributes"].clone();
        if let Some(campi) = attributi.as_object_mut() {
            assert_eq!(
                campi.remove("contract"),
                Some(Value::from("plenora-data-capability-attributes-v1"))
            );
        }
        let mut attesi = pubblica["attributes"].clone();
        if let Some(campi) = attesi.as_object_mut() {
            // `registry` è il percorso del file nei contratti, non un attributo
            // dell'artefatto.
            campi.remove("registry");
        }
        assert_eq!(attributi, attesi, "{}", pubblica["id"]);
        assert_eq!(nostra["status"], "available");
        assert_eq!(nostra["surfaces"], serde_json::json!(["cli"]));
    }
}

#[test]
fn catalog_pubblica_il_registro_dei_kernel_del_runner() {
    let esito = invoca(&["catalog", "--format", "json"]);
    assert_eq!(esito.codice, 0);
    assert_eq!(
        esito.documento["contract"],
        "plenora-data-catalog-result-v2"
    );
    assert_eq!(esito.documento["result"]["plan_format"], "plenora-data-plan-v1");
    let risultato = &esito.documento["result"];
    let registro = Registro::dei_contratti();
    registro
        .valida("operation-registry-v1.schema.json", &risultato["registry"])
        .unwrap_or_else(|motivo| panic!("operation-registry-v1: {motivo}"));
    assert_eq!(
        risultato["registry"]["registry"],
        "plenora-data-kernel-catalog-v2"
    );
    assert_eq!(risultato["registry"]["component"], "plenora-data-tools");

    // DT-001 (profilo v2): ogni kernel del registro comune `data-kernels-v2`
    // ha un descrittore con id, versione e famiglia del registro; quelli
    // disponibili sono, alla lettera, il `registry` del risultato; quelli
    // non disponibili hanno un motivo e mancano dal `registry`.
    let identita = |voce: &Value| {
        (
            voce["id"].as_str().expect("id").to_owned(),
            voce["version"].as_u64().expect("version"),
            voce["family"].as_str().expect("family").to_owned(),
        )
    };
    let registro_comune = contratto("data-kernels-v2.json");
    let comuni: BTreeSet<_> = registro_comune["operations"]
        .as_array()
        .expect("operations")
        .iter()
        .map(identita)
        .collect();
    let kernel = risultato["kernels"].as_array().expect("kernels");
    let nostri: BTreeSet<_> = kernel.iter().map(identita).collect();
    assert_eq!(nostri, comuni, "id, versioni e famiglie del registro comune");
    let disponibili: BTreeSet<_> = kernel
        .iter()
        .filter(|voce| voce["status"] == "available")
        .map(identita)
        .collect();
    let nel_registro: BTreeSet<_> = risultato["registry"]["operations"]
        .as_array()
        .expect("operations")
        .iter()
        .map(identita)
        .collect();
    assert_eq!(nel_registro, disponibili, "registry = kernel disponibili");
    for voce in kernel.iter().filter(|voce| voce["status"] != "available") {
        assert_eq!(voce["status"], "unavailable");
        assert!(voce["reason"].as_str().is_some_and(|motivo| !motivo.is_empty()));
    }
    assert_eq!(kernel.len(), CATALOG.len());

    // Il registro elenca solo i kernel eseguibili; i non eseguibili hanno
    // stato e motivo, e non sono nel registro.
    let nel_registro: BTreeSet<&str> = risultato["registry"]["operations"]
        .as_array()
        .expect("operations")
        .iter()
        .map(|voce| voce["id"].as_str().expect("id"))
        .collect();
    for voce in kernel {
        let id = voce["id"].as_str().expect("id");
        let descrittore = CATALOG.iter().find(|d| d.id == id).expect("nel catalogo");
        assert_eq!(voce["version"], descrittore.semantic_version);
        assert_eq!(voce["versions"]["kernel"], descrittore.kernel_version);
        match voce["status"].as_str() {
            Some("available") => {
                assert!(nel_registro.contains(id), "{id}");
                assert!(voce.get("reason").is_none(), "{id}");
            }
            Some("unavailable") => {
                assert!(!nel_registro.contains(id), "{id}");
                assert!(voce["reason"].as_str().is_some_and(|m| !m.is_empty()));
            }
            altro => panic!("{id}: stato {altro:?}"),
        }
    }
    assert!(!nel_registro.contains("table.transpose"));
    assert_eq!(nel_registro.len() + 1, kernel.len());
}

type Alterazione = Box<dyn Fn(&mut Value)>;

/// Il validatore dei test non è vacuo: inviluppi alterati si rifiutano.
#[test]
fn il_validatore_rifiuta_inviluppi_alterati() {
    let buono = invoca(&["describe"]).documento;
    assert!(comune::valida_inviluppo(&buono).is_ok());
    let alterazioni: Vec<Alterazione> = vec![
        Box::new(|d| {
            d.as_object_mut().expect("oggetto").remove("contract");
        }),
        Box::new(|d| d["extra"] = Value::from(1)),
        Box::new(|d| d["protocol_version"] = Value::from(1)),
        Box::new(|d| d["status"] = Value::from("ok")),
        Box::new(|d| d["error"]["category"] = Value::from("boom")),
        Box::new(|d| {
            d["error"]["remote_effect"] = Value::from("unknown");
            d["error"]["retry"] = serde_json::json!({"kind": "safe"});
        }),
        Box::new(|d| d["error"]["retry"] = serde_json::json!({"kind": "never", "delay_ms": 1})),
        Box::new(|d| d["error"]["code"] = Value::from("minuscolo")),
        Box::new(|d| d["error"]["message"] = Value::from("")),
    ];
    for (indice, altera) in alterazioni.iter().enumerate() {
        let mut documento = buono.clone();
        altera(&mut documento);
        assert!(
            comune::valida_inviluppo(&documento).is_err(),
            "alterazione {indice} accettata"
        );
    }
    let mut capacita = invoca(&["capabilities"]).documento["result"].clone();
    capacita["operations"][0]["status"] = Value::from("unavailable");
    assert!(Registro::dei_contratti()
        .valida("capabilities-v2.schema.json", &capacita)
        .is_err());
}

#[test]
fn la_scoperta_e_deterministica() {
    for argomenti in [
        &["capabilities", "--format", "json"][..],
        &["catalog", "--format", "json"][..],
        &["--help", "--format", "json"][..],
    ] {
        assert_eq!(invoca(argomenti).stdout, invoca(argomenti).stdout);
    }
}
