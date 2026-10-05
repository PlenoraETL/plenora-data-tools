//! Errori senza dati: il testo di una dipendenza che cita un valore scritto
//! nel piano non attraversa il messaggio pubblico (docs/errori.md).
//!
//! Per ogni caso il piano si rifiuta in `validate` con `InvalidPlan`, e la
//! chiamata diretta del kernel sulle stesse tabelle fallisce: nessuno dei
//! due messaggi contiene il valore scritto, ed entrambi dicono il motivo.
//! Le config rifiutate da serde portano una chiave o un valore sentinella
//! (il testo di serde li citerebbe), le regex un pattern sentinella (il
//! testo del crate `regex` lo riporterebbe).

mod comune;

use plenora_core::arrow::array::RecordBatch;
use plenora_core::arrow::schema::SchemaRef;
use plenora_core::PlenoraError;
use plenora_pipeline::{Passo, Pipeline};
use serde_json::{json, Value};

use comune::{chiamata_diretta, nomi_input, tabelle, Fixture};

/// Il testo che nessun messaggio deve contenere.
const SENTINELLA: &str = "SEGRETO";

fn piano(op: &str, ingressi: &[&str], config: Value) -> Pipeline {
    Pipeline {
        version: 1,
        inputs: ingressi.iter().map(|nome| (*nome).to_owned()).collect(),
        crs: None,
        limits: None,
        steps: vec![Passo {
            out: "uscita".to_owned(),
            op: op.to_owned(),
            inputs: ingressi.iter().map(|nome| (*nome).to_owned()).collect(),
            config,
        }],
        outputs: vec!["uscita".to_owned()],
    }
}

fn e_piano(errore: &PlenoraError) -> bool {
    match errore {
        PlenoraError::InvalidPlan(_) => true,
        PlenoraError::Tagged { source, .. } => e_piano(source),
        _ => false,
    }
}

/// L'errore di validazione del piano di un passo.
fn valida(op: &str, fixture: Fixture, config: &Value) -> PlenoraError {
    let tavole = tabelle(fixture);
    let ingressi = nomi_input(tavole.len());
    let riferimenti: Vec<&str> = ingressi.iter().map(String::as_str).collect();
    let schemi: Vec<(&str, SchemaRef)> = riferimenti
        .iter()
        .copied()
        .zip(tavole.iter().map(RecordBatch::schema))
        .collect();
    match piano(op, &riferimenti, config.clone()).validate(&schemi) {
        Ok(_) => panic!("{op} {config}: validato"),
        Err(errore) => errore,
    }
}

/// Un caso: l'operazione, la config rifiutata, il frammento atteso in
/// entrambi i messaggi.
const fn caso(
    op: &'static str,
    config: Value,
    frammento: &'static str,
) -> (&'static str, Value, &'static str) {
    (op, config, frammento)
}

/// **Una chiave ripetuta non entra nel messaggio.** Le chiavi di
/// `mapping` di `table.lookup` sono valori dei dati: il rifiuto e' fisso,
/// con la sola posizione nel documento.
#[test]
fn la_chiave_ripetuta_non_entra_nel_messaggio() {
    for testo in [
        r#"{"version": 1, "inputs": ["t"], "steps": [], "outputs": [], "SEGRETO": 1,
            "SEGRETO": 2}"#,
        r#"{"version": 1, "inputs": ["t"], "outputs": ["u"], "steps": [{"out": "u",
            "op": "table.lookup", "in": ["t"], "config": {"column": "name",
            "mapping": {"SEGRETO": "a", "SEGRETO": "b"}}}]}"#,
    ] {
        let errore = Pipeline::from_json(testo).expect_err("chiave ripetuta");
        let messaggio = errore.to_string();
        assert!(e_piano(&errore), "{messaggio}");
        assert!(messaggio.contains("chiave JSON duplicata"), "{messaggio}");
        assert!(messaggio.contains("(riga "), "{messaggio}");
        assert!(!messaggio.contains(SENTINELLA), "{messaggio}");
    }
}

#[test]
fn nessun_messaggio_cita_il_valore_scritto() {
    let casi = [
        // Config che serde rifiuta: chiave sconosciuta, tipo sbagliato,
        // variante sconosciuta.
        caso(
            "table.filter",
            json!({"column": "id", "operator": "==", "value": 1, "SEGRETO": true}),
            "campo sconosciuto; campi ammessi: `column`, `operator`, `value`",
        ),
        caso(
            "table.filter",
            json!({"column": 7_123_456, "operator": "==", "value": "SEGRETO"}),
            "tipo, valore o forma di un campo non validi",
        ),
        caso(
            "table.filter",
            json!({"column": "id", "operator": "SEGRETO", "value": 1}),
            "valore sconosciuto; valori ammessi:",
        ),
        // Regex della config: ogni operazione che ne compila una.
        caso(
            "table.replace",
            json!({"column": "name", "old_value": "(SEGRETO", "new_value": "z", "regex": true}),
            "regex non valida: sintassi",
        ),
        caso(
            "table.assert_regex",
            json!({"column": "name", "pattern": "(SEGRETO"}),
            "regex non valida: sintassi",
        ),
        caso(
            "table.string_extract",
            json!({"column": "name", "pattern": "(SEGRETO"}),
            "regex non valida: sintassi",
        ),
        caso(
            "table.validate_rules",
            json!({"rules": [{"name": "r", "operator": "regex", "column": "name",
                              "value": "(SEGRETO"}]}),
            "regex non valida: sintassi",
        ),
        caso(
            "table.expression",
            json!({"output_column": "e", "expression": {"kind": "function",
                   "name": "regex_replace",
                   "args": [{"kind": "column", "name": "name"},
                            {"kind": "literal", "value": "(SEGRETO"},
                            {"kind": "literal", "value": "x"}]}}),
            "regex non valida: sintassi",
        ),
        // Letterali della config fuori elenco: il messaggio dice gli
        // ammessi, non quello scritto.
        caso(
            "table.expression",
            json!({"output_column": "e", "expression": {"kind": "function",
                   "name": "date_trunc",
                   "args": [{"kind": "literal", "value": "SEGRETO"},
                            {"kind": "column", "name": "date"}]}}),
            "unita' non valida; ammesse: year, month, day, hour, minute, second",
        ),
        caso(
            "table.assert_schema",
            json!({"fields": [{"name": "id", "data_type": "SEGRETO"}], "allow_extra": true}),
            "data_type non supportato; ammessi: utf8",
        ),
        caso(
            "table.timezone_convert",
            json!({"column": "date", "input_format": "%Y-%m-%d", "source_timezone": "UTC",
                   "target_timezone": "SEGRETO", "output_column": "d"}),
            "timezone non valida",
        ),
    ];
    let mut difetti = Vec::new();
    for (op, config, frammento) in casi {
        let validazione = valida(op, Fixture::Wide, &config);
        let tavole = tabelle(Fixture::Wide);
        let kernel = chiamata_diretta(op, &config, &tavole);
        for (dove, esito) in [
            ("validazione", Err(validazione)),
            ("kernel", kernel.map(|_| ())),
        ] {
            match esito {
                Err(errore)
                    if e_piano(&errore)
                        && errore.to_string().contains(frammento)
                        && !errore.to_string().contains(SENTINELLA)
                        && !errore.to_string().contains("7123456") => {}
                altro => difetti.push(format!(
                    "{op} {config}: {dove}, atteso InvalidPlan con «{frammento}» e senza il \
                     valore scritto, avuto {altro:?}"
                )),
            }
        }
    }
    assert!(difetti.is_empty(), "{}", difetti.join("\n"));
}

/// **Un testo della config riconosciuto o rifiutato non torna nel
/// messaggio.** Per ogni caso: l'operazione, la config, il frammento atteso
/// e il testo scritto che non deve comparire, in validazione e nel kernel.
#[test]
fn i_testi_della_config_non_tornano_nel_messaggio() {
    let casi = [
        // Tipo riconosciuto ma diverso: il nome canonico, non lo scritto.
        (
            "table.assert_schema",
            json!({"fields": [{"name": "id", "data_type": "  StRiNg  "}], "allow_extra": true}),
            "atteso utf8, trovato",
            "StRiNg",
        ),
        // Carattere non ammesso: la posizione, non il carattere.
        (
            "table.formula",
            json!({"new_column": "f", "formula": "value * #2"}),
            "carattere formula non ammesso alla posizione 8",
            "#",
        ),
        // Prefisso configurato: resta il nome della colonna d'uscita.
        (
            "table.flatten_json",
            json!({"column": "json", "prefix": "SEGRETO_", "output_columns": ["json_a"]}),
            "non inizia con",
            SENTINELLA,
        ),
    ];
    let mut difetti = Vec::new();
    for (op, config, frammento, scritto) in casi {
        let validazione = valida(op, Fixture::Wide, &config);
        let kernel = chiamata_diretta(op, &config, &tabelle(Fixture::Wide));
        for (dove, esito) in [
            ("validazione", Err(validazione)),
            ("kernel", kernel.map(|_| ())),
        ] {
            match esito {
                Err(errore)
                    if errore.to_string().contains(frammento)
                        && !errore.to_string().contains(scritto) => {}
                altro => difetti.push(format!(
                    "{op}: {dove}, atteso «{frammento}» senza «{scritto}», avuto {altro:?}"
                )),
            }
        }
    }
    assert!(difetti.is_empty(), "{}", difetti.join("\n"));
}

/// **La versione del piano scritta non entra nel messaggio.**
#[test]
fn la_versione_scritta_non_entra_nel_messaggio() {
    let mut pipeline = piano("table.limit", &["ingresso_0"], json!({"n": 1}));
    pipeline.version = 4242;
    let tavole = tabelle(Fixture::Wide);
    let errore = pipeline
        .validate(&[("ingresso_0", tavole[0].schema())])
        .expect_err("versione non supportata");
    let testo = errore.to_string();
    assert!(e_piano(&errore), "{testo}");
    assert!(
        testo.contains("versione del piano non supportata: attesa 1"),
        "{testo}"
    );
    assert!(!testo.contains("4242"), "{testo}");
}

/// **Il fuso di `type_cast` non entra nel messaggio.** La validazione lo
/// rifiuta con il messaggio fisso; il kernel chiamato da solo non lo
/// verifica prima e rifiuta le righe (diagnostica per riga, senza valori).
#[test]
fn il_fuso_di_type_cast_non_entra_nel_messaggio() {
    let config = json!({"column": "date", "target_type": "timestamp_millis",
                        "timezone": "SEGRETO"});
    let validazione = valida("table.type_cast", Fixture::Wide, &config);
    let testo = validazione.to_string();
    assert!(e_piano(&validazione), "{testo}");
    assert!(testo.contains("timezone non valida"), "{testo}");
    assert!(!testo.contains(SENTINELLA), "{testo}");
    let kernel = chiamata_diretta("table.type_cast", &config, &tabelle(Fixture::Wide))
        .expect_err("il fuso non e' valido");
    assert!(!format!("{kernel:?}").contains(SENTINELLA), "{kernel:?}");
}
