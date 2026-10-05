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
