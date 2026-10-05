//! `data.run` 3 (`plenora_cli::api::esegui_artefatti`) con un risolutore in
//! memoria strumentato: registra letture e pubblicazioni nel loro ordine e
//! inietta i guasti di DT-RUN-001..DT-RUN-008 (profilo data-tools v2 di
//! `plenora-contracts`) nelle diverse fasi. Il risultato si valida con lo
//! schema `data-execution-result-v3` dei contratti, e le tabelle pubblicate
//! si confrontano con quelle di `data.run` 2 sullo stesso piano (DT-RUN-004).

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod comune;

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use comune::{contratto, Registro};
use plenora_cli::api::{
    esegui_artefatti, esegui_in_memoria, Destinazione, PubblicazioneFallita, RifiutoDestinazioni,
    RisolutoreArtefatti,
};
use plenora_core::arrow::array::{Float64Array, Int64Array, RecordBatch, StringArray};
use plenora_core::arrow::schema::{DataType, Field, Schema};
use plenora_core::{ErrorCategory, PlenoraError, RemoteEffect, Result, RetryDisposition};
use plenora_io::{leggi_tabella, scrivi_tabella, Formato, Ingresso, OpzioniScrittura};
use plenora_pipeline::{Interruzione, Pipeline};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

const PIANO: &str = r#"{"version": 1, "inputs": ["t"],
  "steps": [
    {"out": "positivi", "op": "table.filter", "in": ["t"],
     "config": {"column": "valore", "operator": ">", "value": 0}},
    {"out": "primo", "op": "table.limit", "in": ["t"], "config": {"n": 1}}],
  "outputs": ["positivi", "primo"]}"#;

/// Un riferimento che non deve mai comparire in un messaggio d'errore.
const SEGRETO: &str = "artifact://segreto-7f3a91/uscita";
/// Il testo che il risolutore mette nei suoi errori: ciò a cui un
/// riferimento si è risolto (DT-RUN-008), mai nell'errore restituito.
const RISOLTO: &str = "C:/privato-9c1e/clienti.parquet";

fn tabella() -> RecordBatch {
    RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int64, false),
            Field::new("nome", DataType::Utf8, true),
            Field::new("valore", DataType::Float64, true),
        ])),
        vec![
            Arc::new(Int64Array::from(vec![1, 2, 3, 4])),
            Arc::new(StringArray::from(vec![
                Some("a"),
                Some("b"),
                None,
                Some("d"),
            ])),
            Arc::new(Float64Array::from(vec![
                Some(1.5),
                Some(-2.0),
                None,
                Some(4.0),
            ])),
        ],
    )
    .unwrap()
}

fn byte_di(tabella: &RecordBatch, formato: Formato) -> Vec<u8> {
    let cartella = tempfile::tempdir().unwrap();
    let percorso = cartella.path().join("t");
    let opzioni = OpzioniScrittura {
        formato: Some(formato),
        ..OpzioniScrittura::default()
    };
    scrivi_tabella(tabella, &percorso, &opzioni).unwrap();
    std::fs::read(percorso).unwrap()
}

fn sha256(byte: &[u8]) -> String {
    use std::fmt::Write as _;
    Sha256::digest(byte)
        .iter()
        .fold(String::new(), |mut testo, b| {
            let _ = write!(testo, "{b:02x}");
            testo
        })
}

/// Una modifica della richiesta.
type Mutatore = dyn Fn(&mut Value);
/// Una mutazione della richiesta, con il suo nome.
type Mutazione = (&'static str, Box<dyn Fn(&mut Value)>);
/// Una mutazione della richiesta che vede il risolutore.
type MutazioneConRisolutore = Box<dyn Fn(&mut Value, &Strumentato)>;

/// Un guasto iniettato in una pubblicazione, per indice (0 = la prima).
#[derive(Clone, Copy)]
enum Guasto {
    NienteScritto,
    Parziale,
    Ignoto,
}

#[derive(Default)]
struct Strumentato {
    sorgenti: BTreeMap<String, Vec<u8>>,
    pubblicati: RefCell<Vec<(String, String, Vec<u8>)>>,
    eventi: RefCell<Vec<String>>,
    preparazione: Option<RifiutoDestinazioni>,
    guasti: BTreeMap<usize, Guasto>,
    /// Alzato durante la pubblicazione numero `.1`.
    annulla_durante: Option<(Arc<AtomicBool>, usize)>,
}

impl RisolutoreArtefatti for Strumentato {
    fn leggi(&self, riferimento: &str, destinazione: &mut dyn Write) -> std::io::Result<()> {
        self.eventi
            .borrow_mut()
            .push(format!("leggi {riferimento}"));
        let byte = self
            .sorgenti
            .get(riferimento)
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, RISOLTO))?;
        destinazione.write_all(byte)
    }

    fn prepara(
        &self,
        destinazioni: &[Destinazione<'_>],
    ) -> std::result::Result<(), RifiutoDestinazioni> {
        self.eventi
            .borrow_mut()
            .push(format!("prepara {}", destinazioni.len()));
        self.preparazione.map_or(Ok(()), Err)
    }

    fn pubblica(
        &self,
        destinazione: &Destinazione<'_>,
        contenuto: &mut dyn Read,
        byte: u64,
    ) -> std::result::Result<(), PubblicazioneFallita> {
        let indice = self.pubblicati.borrow().len()
            + self
                .eventi
                .borrow()
                .iter()
                .filter(|e| e.starts_with("fallita"))
                .count();
        if let Some((segnale, quando)) = &self.annulla_durante {
            if *quando == indice {
                segnale.store(true, Ordering::Release);
            }
        }
        let mut letti = Vec::new();
        contenuto.read_to_end(&mut letti).unwrap();
        assert_eq!(letti.len() as u64, byte, "byte dichiarati");
        if let Some(guasto) = self.guasti.get(&indice) {
            self.eventi
                .borrow_mut()
                .push(format!("fallita {}", destinazione.nome));
            let causa = std::io::Error::other(RISOLTO);
            return Err(match guasto {
                Guasto::NienteScritto => PubblicazioneFallita::NienteScritto(causa),
                Guasto::Parziale => PubblicazioneFallita::Parziale(causa),
                Guasto::Ignoto => PubblicazioneFallita::Ignoto(causa),
            });
        }
        self.eventi
            .borrow_mut()
            .push(format!("pubblica {}", destinazione.nome));
        self.pubblicati.borrow_mut().push((
            destinazione.riferimento.to_owned(),
            destinazione.tipo.to_owned(),
            letti,
        ));
        Ok(())
    }
}

fn risolutore() -> Strumentato {
    let mut sorgenti = BTreeMap::new();
    sorgenti.insert(
        "artifact://input/t".to_owned(),
        byte_di(&tabella(), Formato::Parquet),
    );
    Strumentato {
        sorgenti,
        ..Strumentato::default()
    }
}

fn richiesta() -> Value {
    json!({
        "schema_version": 1,
        "plan": "PIANO",
        "inputs": {
            "t": {"reference": "artifact://input/t", "content_type": "application/vnd.apache.parquet"}
        },
        "outputs": {
            "positivi": {
                "reference": "artifact://output/positivi",
                "content_type": "application/vnd.apache.arrow.stream",
                "overwrite": false
            },
            "primo": {
                "reference": SEGRETO,
                "content_type": "application/vnd.apache.parquet",
                "overwrite": true
            }
        }
    })
}

/// Il testo della richiesta con il piano scritto com'è (non riserializzato).
fn testo(richiesta: &Value) -> String {
    serde_json::to_string(richiesta)
        .unwrap()
        .replace("\"PIANO\"", PIANO)
}

fn esegui(richiesta: &Value, risolutore: &Strumentato) -> Result<Value> {
    esegui_artefatti(&testo(richiesta), risolutore, &Interruzione::default())
}

fn errore(richiesta: &Value, risolutore: &Strumentato) -> PlenoraError {
    let errore = esegui(richiesta, risolutore).expect_err("doveva fallire");
    senza_posizioni(&errore);
    errore
}

/// DT-RUN-008: né il testo Rust né la proiezione pubblica portano un
/// riferimento o ciò a cui si è risolto.
fn senza_posizioni(errore: &PlenoraError) {
    let testi = [
        errore.to_string(),
        format!("{errore:?}"),
        serde_json::to_string(&errore.public_projection()).unwrap(),
    ];
    for testo in testi {
        assert!(!testo.contains("segreto"), "{testo}");
        assert!(!testo.contains("privato"), "{testo}");
    }
}

fn registro() -> Registro {
    Registro::con(&[
        "data-plan-v1.schema.json",
        "data-execution-input-v3.schema.json",
        "data-execution-result-v3.schema.json",
    ])
}

#[test]
fn il_manifesto_e_quello_del_contratto_e_le_tabelle_quelle_di_data_run_2() {
    let risolutore = risolutore();
    let manifesto = esegui(&richiesta(), &risolutore).unwrap();
    registro()
        .valida("data-execution-result-v3.schema.json", &manifesto)
        .unwrap();
    let ricevuti = risolutore.pubblicati.borrow();
    // Nell'ordine del piano, con riferimento, tipo, byte e digest esatti.
    let attesi = [
        (
            "positivi",
            "artifact://output/positivi",
            "application/vnd.apache.arrow.stream",
        ),
        ("primo", SEGRETO, "application/vnd.apache.parquet"),
    ];
    let manifestate = manifesto["outputs"].as_array().unwrap();
    assert_eq!(manifestate.len(), 2);
    let oracolo = esegui_in_memoria(
        &Pipeline::from_json(PIANO).unwrap(),
        vec![Ingresso::Tabella {
            nome: "t".to_owned(),
            tabella: tabella(),
        }],
        &Interruzione::default(),
    )
    .unwrap();
    for (indice, (nome, riferimento, tipo)) in attesi.iter().enumerate() {
        let uscita = &manifestate[indice];
        let (pubblicato, tipo_pubblicato, byte) = &ricevuti[indice];
        assert_eq!(uscita["name"], *nome);
        assert_eq!(uscita["reference"], *riferimento);
        assert_eq!(pubblicato, riferimento);
        assert_eq!(tipo_pubblicato, tipo);
        assert_eq!(uscita["artifact"]["content_type"], *tipo);
        assert_eq!(uscita["artifact"]["size"], byte.len());
        assert_eq!(uscita["artifact"]["sha256"], sha256(byte));
        // DT-RUN-004: la tabella pubblicata e' quella di data.run 2.
        let cartella = tempfile::tempdir().unwrap();
        let percorso = cartella.path().join("letto");
        std::fs::write(&percorso, byte).unwrap();
        let formato = if *tipo == "application/vnd.apache.parquet" {
            Formato::Parquet
        } else {
            Formato::ArrowIpc
        };
        let riletta = leggi_tabella(&percorso, Some(formato), u64::MAX).unwrap();
        let (nome_oracolo, da_oracolo) = &oracolo.1[indice];
        assert_eq!(nome_oracolo, nome);
        assert_eq!(&riletta, da_oracolo);
        assert_eq!(uscita["rows"], da_oracolo.num_rows());
        assert_eq!(uscita["columns"], da_oracolo.num_columns());
    }
    // I conteggi per passo sono quelli di data.run 2.
    assert_eq!(manifesto["steps"], oracolo.0["steps"]);
}

#[test]
fn si_legge_tutto_e_si_codifica_tutto_prima_di_pubblicare() {
    let risolutore = risolutore();
    esegui(&richiesta(), &risolutore).unwrap();
    assert_eq!(
        *risolutore.eventi.borrow(),
        [
            "prepara 2",
            "leggi artifact://input/t",
            "pubblica positivi",
            "pubblica primo"
        ]
    );
}

#[test]
fn nomi_e_riferimenti_si_rifiutano_prima_di_leggere() {
    let casi: Vec<Mutazione> = vec![
        (
            "sorgente mancante",
            Box::new(|r| {
                r["inputs"].as_object_mut().unwrap().clear();
            }),
        ),
        (
            "destinazione in piu'",
            Box::new(|r| {
                r["outputs"]["altro"] = json!({"reference": "artifact://o/x", "content_type": "application/vnd.apache.arrow.file", "overwrite": false});
            }),
        ),
        (
            "riferimento file:",
            Box::new(|r| r["inputs"]["t"]["reference"] = json!("file:///tmp/t.parquet")),
        ),
        (
            "segmento ..",
            Box::new(|r| r["inputs"]["t"]["reference"] = json!("artifact://input/../t")),
        ),
        (
            "punti codificati",
            Box::new(|r| r["inputs"]["t"]["reference"] = json!("artifact://input/%2e%2e/t")),
        ),
        (
            "spazio",
            Box::new(|r| r["inputs"]["t"]["reference"] = json!("artifact://input/t x")),
        ),
        (
            "percorso",
            Box::new(|r| r["inputs"]["t"]["reference"] = json!("/tmp/t.parquet")),
        ),
        (
            "unita'",
            Box::new(|r| r["inputs"]["t"]["reference"] = json!("C:\\t.parquet")),
        ),
        (
            "tipo",
            Box::new(|r| r["inputs"]["t"]["content_type"] = json!("text/csv")),
        ),
        (
            "overwrite assente",
            Box::new(|r| {
                r["outputs"]["primo"]
                    .as_object_mut()
                    .unwrap()
                    .remove("overwrite");
            }),
        ),
        ("campo sconosciuto", Box::new(|r| r["path"] = json!("/tmp"))),
        (
            "schema_version",
            Box::new(|r| r["schema_version"] = json!(2)),
        ),
        (
            "expected vuoto",
            Box::new(|r| r["inputs"]["t"]["expected"] = json!({})),
        ),
        (
            "digest non valido",
            Box::new(|r| r["inputs"]["t"]["expected"] = json!({"sha256": "ABC"})),
        ),
        (
            "stesso riferimento",
            Box::new(|r| r["outputs"]["primo"]["reference"] = json!("artifact://output/positivi")),
        ),
    ];
    for (nome, muta) in casi {
        let mut r = richiesta();
        muta(&mut r);
        let risolutore = risolutore();
        let errore = errore(&r, &risolutore);
        assert_eq!(
            errore.category(),
            ErrorCategory::InvalidConfiguration,
            "{nome}: {errore}"
        );
        assert_eq!(errore.remote_effect(), RemoteEffect::None, "{nome}");
        assert!(
            risolutore.eventi.borrow().is_empty(),
            "{nome}: {:?}",
            risolutore.eventi.borrow()
        );
    }
}

#[test]
fn il_piano_si_legge_dal_suo_testo() {
    let risolutore = risolutore();
    // Una chiave ripetuta nella richiesta (anche nel piano) e un numero non
    // esatto nel piano (DPLAN-002, DPLAN-003).
    let ripetuta = testo(&richiesta()).replace(
        "\"schema_version\":1",
        "\"schema_version\":1,\"schema_version\":1",
    );
    let errore = esegui_artefatti(&ripetuta, &risolutore, &Interruzione::default()).unwrap_err();
    assert_eq!(errore.category(), ErrorCategory::InvalidConfiguration);
    let inesatto = testo(&richiesta()).replace(
        "\"value\": 0",
        "\"value\": 0.1000000000000000055511151231257827",
    );
    let errore = esegui_artefatti(&inesatto, &risolutore, &Interruzione::default()).unwrap_err();
    assert_eq!(errore.category(), ErrorCategory::InvalidPlan, "{errore}");
    assert!(risolutore.eventi.borrow().is_empty());
}

#[test]
fn una_sorgente_diversa_dall_attesa_non_esegue_e_non_pubblica() {
    let casi: Vec<MutazioneConRisolutore> = vec![
        Box::new(|r, _| r["inputs"]["t"]["expected"] = json!({"size": 1})),
        Box::new(|r, _| r["inputs"]["t"]["expected"] = json!({"sha256": "0".repeat(64)})),
        // Dichiarata file Arrow, e' Parquet.
        Box::new(|r, _| {
            r["inputs"]["t"]["content_type"] = json!("application/vnd.apache.arrow.file");
        }),
    ];
    for muta in casi {
        let risolutore = risolutore();
        let mut r = richiesta();
        muta(&mut r, &risolutore);
        let errore = errore(&r, &risolutore);
        assert_eq!(errore.category(), ErrorCategory::DataMapping, "{errore}");
        assert_eq!(errore.remote_effect(), RemoteEffect::None);
        assert!(risolutore.pubblicati.borrow().is_empty());
    }
    // L'attesa esatta passa.
    let risolutore = risolutore();
    let byte = risolutore.sorgenti["artifact://input/t"].clone();
    let mut r = richiesta();
    r["inputs"]["t"]["expected"] = json!({"size": byte.len(), "sha256": sha256(&byte)});
    esegui(&r, &risolutore).unwrap();
}

#[test]
fn una_sorgente_irrisolta_e_not_found_senza_effetto() {
    let mut risolutore = risolutore();
    risolutore.sorgenti.clear();
    let errore = errore(&richiesta(), &risolutore);
    assert_eq!(errore.category(), ErrorCategory::NotFound);
    assert_eq!(errore.remote_effect(), RemoteEffect::None);
    assert!(risolutore.pubblicati.borrow().is_empty());
}

#[test]
fn il_rifiuto_della_preparazione_ferma_prima_di_leggere() {
    let mut risolutore = risolutore();
    let casi = [
        (
            RifiutoDestinazioni::StessoArtefatto,
            ErrorCategory::InvalidConfiguration,
        ),
        (RifiutoDestinazioni::NonTrovata, ErrorCategory::NotFound),
        (
            RifiutoDestinazioni::NonAutorizzata,
            ErrorCategory::Authorization,
        ),
        (
            RifiutoDestinazioni::SovrascritturaNonAtomica,
            ErrorCategory::Unsupported,
        ),
        (
            RifiutoDestinazioni::Io(std::io::ErrorKind::TimedOut),
            ErrorCategory::Timeout,
        ),
    ];
    for (rifiuto, categoria) in casi {
        risolutore.preparazione = Some(rifiuto);
        risolutore.eventi.borrow_mut().clear();
        let errore = errore(&richiesta(), &risolutore);
        assert_eq!(errore.category(), categoria, "{rifiuto:?}");
        assert_eq!(errore.remote_effect(), RemoteEffect::None);
        assert_eq!(*risolutore.eventi.borrow(), ["prepara 2"]);
    }
}

#[test]
fn un_errore_dell_esecuzione_non_pubblica_nulla() {
    let risolutore = risolutore();
    // La colonna non esiste: il piano fallisce dopo la lettura delle
    // sorgenti, prima di ogni pubblicazione.
    let testo = testo(&richiesta()).replace("\"column\": \"valore\"", "\"column\": \"assente\"");
    let errore = esegui_artefatti(&testo, &risolutore, &Interruzione::default()).unwrap_err();
    assert_eq!(errore.remote_effect(), RemoteEffect::None, "{errore}");
    assert!(risolutore
        .eventi
        .borrow()
        .iter()
        .any(|e| e.starts_with("leggi")));
    assert!(risolutore.pubblicati.borrow().is_empty());
    assert!(!risolutore
        .eventi
        .borrow()
        .iter()
        .any(|e| e.starts_with("pubblica")));
}

/// DT-RUN-006: l'effetto di una pubblicazione fallita.
#[test]
fn gli_esiti_delle_pubblicazioni_fallite() {
    let casi = [
        // (indice del guasto, guasto, effetto atteso, pubblicati prima)
        (0, Guasto::NienteScritto, RemoteEffect::None, 0),
        (1, Guasto::NienteScritto, RemoteEffect::Partial, 1),
        (0, Guasto::Parziale, RemoteEffect::Partial, 0),
        (1, Guasto::Parziale, RemoteEffect::Partial, 1),
        (0, Guasto::Ignoto, RemoteEffect::Unknown, 0),
        (1, Guasto::Ignoto, RemoteEffect::Unknown, 1),
    ];
    for (indice, guasto, effetto, prima) in casi {
        let mut risolutore = risolutore();
        risolutore.guasti.insert(indice, guasto);
        let errore = errore(&richiesta(), &risolutore);
        assert_eq!(errore.remote_effect(), effetto, "{indice}");
        assert_eq!(risolutore.pubblicati.borrow().len(), prima);
        if effetto != RemoteEffect::None {
            // Mai un ritentativo automatico dopo un effetto visibile o ignoto.
            assert!(
                matches!(
                    errore.retry_disposition(),
                    RetryDisposition::Never
                        | RetryDisposition::Quarantine
                        | RetryDisposition::RequiresRecovery
                ),
                "{:?}",
                errore.retry_disposition()
            );
        }
        // Dopo un fallimento non si pubblica altro.
        assert_eq!(
            risolutore
                .eventi
                .borrow()
                .iter()
                .filter(|e| e.starts_with("fallita"))
                .count(),
            1
        );
    }
}

/// DT-RUN-007: annullamento prima della prima pubblicazione (nessun effetto)
/// e fra due pubblicazioni (`partial`).
#[test]
fn l_annullamento_dopo_l_inizio_della_pubblicazione_e_partial() {
    let segnale = Arc::new(AtomicBool::new(false));
    let interruzione = Interruzione {
        scadenza: None,
        annullamento: Some(Arc::clone(&segnale)),
    };
    let mut risolutore = risolutore();
    risolutore.annulla_durante = Some((Arc::clone(&segnale), 0));
    let errore = esegui_artefatti(&testo(&richiesta()), &risolutore, &interruzione).unwrap_err();
    assert_eq!(errore.category(), ErrorCategory::Cancelled);
    assert_eq!(errore.remote_effect(), RemoteEffect::Partial);
    assert_eq!(risolutore.pubblicati.borrow().len(), 1);

    let gia_annullato = Interruzione {
        scadenza: None,
        annullamento: Some(Arc::new(AtomicBool::new(true))),
    };
    let secondo = self::risolutore();
    let errore = esegui_artefatti(&testo(&richiesta()), &secondo, &gia_annullato).unwrap_err();
    assert_eq!(errore.category(), ErrorCategory::Cancelled);
    assert_eq!(errore.remote_effect(), RemoteEffect::None);
    assert!(secondo.pubblicati.borrow().is_empty());
}

/// Il vettore di richiesta dei contratti si legge: forma e regole sono le
/// stesse (il piano del vettore usa una colonna che qui non c'e', quindi si
/// controlla solo la lettura fino alle sorgenti).
#[test]
fn il_vettore_di_richiesta_dei_contratti_si_legge() {
    let vettore = contratto("data-run-request-v3.json");
    let payload = &vettore["payload"];
    registro()
        .valida("data-execution-input-v3.schema.json", payload)
        .unwrap();
    let risolutore = Strumentato::default();
    let errore = esegui_artefatti(
        &serde_json::to_string(payload).unwrap(),
        &risolutore,
        &Interruzione::default(),
    )
    .unwrap_err();
    // Arriva alla lettura della sorgente, che qui non esiste.
    assert_eq!(errore.category(), ErrorCategory::NotFound, "{errore}");
    assert_eq!(
        *risolutore.eventi.borrow(),
        ["prepara 1", "leggi artifact://input/data-run-v3-parcels"]
    );
}

/// DT-RUN-007: un'interruzione arrivata durante l'ultima pubblicazione non
/// è un successo; tutto è pubblicato, l'effetto è `committed`. Anche con un
/// solo output.
#[test]
fn l_annullamento_durante_l_ultima_pubblicazione_non_e_un_successo() {
    let un_output = {
        let mut r = richiesta();
        r["outputs"].as_object_mut().unwrap().remove("primo");
        r
    };
    let piano_un_output = PIANO.replace(
        r#""outputs": ["positivi", "primo"]"#,
        r#""outputs": ["positivi"]"#,
    );
    for (testo, ultimo, attesi) in [
        (testo(&richiesta()), 1, 2),
        (
            serde_json::to_string(&un_output)
                .unwrap()
                .replace("\"PIANO\"", &piano_un_output),
            0,
            1,
        ),
    ] {
        let segnale = Arc::new(AtomicBool::new(false));
        let interruzione = Interruzione {
            scadenza: None,
            annullamento: Some(Arc::clone(&segnale)),
        };
        let mut risolutore = risolutore();
        risolutore.annulla_durante = Some((Arc::clone(&segnale), ultimo));
        let errore = esegui_artefatti(&testo, &risolutore, &interruzione).unwrap_err();
        assert_eq!(errore.category(), ErrorCategory::Cancelled);
        assert_eq!(errore.remote_effect(), RemoteEffect::Committed);
        assert_ne!(errore.retry_disposition(), RetryDisposition::Safe);
        assert_eq!(risolutore.pubblicati.borrow().len(), attesi);
    }
}

/// DT-RUN-008: il testo degli errori del risolutore (lettura, pubblicazione)
/// non arriva all'errore restituito, né al testo Rust né alla proiezione.
#[test]
fn il_testo_degli_errori_del_risolutore_non_passa() {
    let mut senza_sorgente = risolutore();
    senza_sorgente.sorgenti.clear();
    let lettura = errore(&richiesta(), &senza_sorgente);
    assert_eq!(lettura.category(), ErrorCategory::NotFound);
    for guasto in [Guasto::NienteScritto, Guasto::Parziale, Guasto::Ignoto] {
        let mut risolutore = risolutore();
        risolutore.guasti.insert(0, guasto);
        errore(&richiesta(), &risolutore);
    }
}

/// `null` non è l'assenza: un campo facoltativo scritto `null` è una
/// richiesta che lo schema rifiuta.
#[test]
fn un_campo_null_non_e_un_campo_omesso() {
    let casi: Vec<Box<Mutatore>> = vec![
        Box::new(|r| r["$schema"] = Value::Null),
        Box::new(|r| r["inputs"]["t"]["expected"] = Value::Null),
        Box::new(|r| {
            r["inputs"]["t"]["expected"] = json!({"size": null, "sha256": "0".repeat(64)});
        }),
        Box::new(|r| r["inputs"]["t"]["expected"] = json!({"sha256": null, "size": 1})),
    ];
    for muta in casi {
        let mut r = richiesta();
        muta(&mut r);
        let risolutore = risolutore();
        let errore = errore(&r, &risolutore);
        assert_eq!(
            errore.category(),
            ErrorCategory::InvalidConfiguration,
            "{errore}"
        );
        assert!(risolutore.eventi.borrow().is_empty());
    }
    // `$schema` stringa e' ammesso.
    let mut r = richiesta();
    r["$schema"] = json!("../../schemas/data-execution-input-v3.schema.json");
    esegui(&r, &risolutore()).unwrap();
}

/// I confini della regola dei riferimenti dello schema: ammessi e rifiutati
/// come li ammette e rifiuta il `pattern` (semantica ECMAScript, lunghezza in
/// caratteri).
#[test]
fn i_riferimenti_seguono_il_pattern_dello_schema() {
    let lungo = format!("aa:{}", "é".repeat(1024));
    let ammessi = [
        "aa://",
        "aa:.:x",
        "aa:..:x",
        "aa:x\u{85}",
        lungo.as_str(),
        "aa:/x/.../y",
    ];
    let rifiutati = [
        "aa:x\u{feff}",
        "aa:x\u{a0}y",
        "aa:./x",
        "aa:x/..",
        "aa:%2Ex",
        "FILE:x",
        "a:xx",
        "aa:",
        "Aa:xx",
        "aa:x\\y",
    ];
    for riferimento in ammessi {
        let mut r = richiesta();
        r["outputs"]["positivi"]["reference"] = json!(riferimento);
        let risolutore = risolutore();
        esegui(&r, &risolutore)
            .unwrap_or_else(|errore| panic!("{riferimento:?} rifiutato: {errore}"));
    }
    for riferimento in rifiutati {
        let mut r = richiesta();
        r["outputs"]["positivi"]["reference"] = json!(riferimento);
        let risolutore = risolutore();
        let errore = errore(&r, &risolutore);
        assert_eq!(
            errore.category(),
            ErrorCategory::InvalidConfiguration,
            "{riferimento:?}"
        );
        assert!(risolutore.eventi.borrow().is_empty());
    }
}

/// DT-RUN-005: il primo output si codifica, il secondo no (una tabella senza
/// colonne non si scrive in Parquet): nessuna pubblicazione, nessun effetto.
#[test]
fn un_fallimento_della_codifica_non_pubblica_nulla() {
    let piano = PIANO.replace(
        r#"{"out": "primo", "op": "table.limit", "in": ["t"], "config": {"n": 1}}"#,
        r#"{"out": "primo", "op": "table.drop_columns", "in": ["t"], "config": {"columns": ["id", "nome", "valore"]}}"#,
    );
    let testo = serde_json::to_string(&richiesta())
        .unwrap()
        .replace("\"PIANO\"", &piano);
    let risolutore = risolutore();
    let errore = esegui_artefatti(&testo, &risolutore, &Interruzione::default()).unwrap_err();
    assert_eq!(errore.category(), ErrorCategory::Unsupported, "{errore}");
    assert_eq!(errore.remote_effect(), RemoteEffect::None);
    assert!(risolutore.pubblicati.borrow().is_empty());
    assert_eq!(
        *risolutore.eventi.borrow(),
        ["prepara 2", "leggi artifact://input/t"]
    );
}

/// L'ordine di pubblicazione è quello del piano, non quello alfabetico dei
/// nomi.
#[test]
fn si_pubblica_nell_ordine_del_piano() {
    let piano = PIANO.replace(
        r#""outputs": ["positivi", "primo"]"#,
        r#""outputs": ["primo", "positivi"]"#,
    );
    let testo = serde_json::to_string(&richiesta())
        .unwrap()
        .replace("\"PIANO\"", &piano);
    let risolutore = risolutore();
    let manifesto = esegui_artefatti(&testo, &risolutore, &Interruzione::default()).unwrap();
    assert_eq!(
        *risolutore.eventi.borrow(),
        [
            "prepara 2",
            "leggi artifact://input/t",
            "pubblica primo",
            "pubblica positivi"
        ]
    );
    assert_eq!(manifesto["outputs"][0]["name"], "primo");
    assert_eq!(manifesto["outputs"][1]["name"], "positivi");
}

/// Limite dichiarato: un intero scritto con frazione o esponente (`1.0`,
/// `1e0`), che JSON Schema conta come intero, si rifiuta invece di essere
/// convertito.
#[test]
fn un_intero_scritto_con_la_frazione_si_rifiuta() {
    for (da, a) in [
        ("\"schema_version\":1", "\"schema_version\":1.0"),
        ("\"schema_version\":1", "\"schema_version\":1e0"),
    ] {
        let testo = testo(&richiesta()).replace(da, a);
        let risolutore = risolutore();
        let errore = esegui_artefatti(&testo, &risolutore, &Interruzione::default()).unwrap_err();
        assert_eq!(errore.category(), ErrorCategory::InvalidConfiguration);
        assert!(risolutore.eventi.borrow().is_empty());
    }
}
