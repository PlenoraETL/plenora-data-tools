//! Regressione del round-trip del piano dell'issue #123.

use plenora_pipeline::Pipeline;
use proptest::prelude::*;

#[test]
fn il_piano_dell_issue_123_sopravvive_al_round_trip() {
    let testo = r#"{"version": 1, "inputs": ["sinistra"], "steps": [{"out": "uscita", "op": "table.conditional", "in": ["ssiarint"], "config": {"column": "value", "conditions": [{"operator": ">", "value": 50, "result": 1100000000000000000000000000000000}], "default_value": 0, "output_column": "`conditioned"}}], "outputs": ["uscita"]}"#;
    assert_eq!(testo.len(), 316);
    let piano = Pipeline::from_json(testo).expect("piano originale");
    let serializzato = serde_json::to_string(&piano).expect("serializzazione");
    let riletto = Pipeline::from_json(&serializzato).expect("piano riscritto");
    assert_eq!(riletto, piano);
}

#[test]
fn forme_non_brevi_e_interi_inesatti_si_rifiutano() {
    for numero in [
        "1.10000000000000001e33",
        "0.11e34",
        "1100000000000000000000000000000001",
        "18446744073709551617",
        "100000000000000000001",
    ] {
        for campo in [
            format!(
                r#""steps":[{{"out":"x","op":"table.conditional","in":[],"config":{{"value":{numero}}}}}]"#
            ),
            format!(r#""steps":[],"limits":{{"max_expansion_factor":{numero}}}"#),
        ] {
            let testo = format!(r#"{{"version":1,"inputs":[],"outputs":[],{campo}}}"#);
            let errore = Pipeline::from_json(&testo).expect_err("numero inesatto");
            assert_eq!(errore.category(), plenora_core::ErrorCategory::InvalidPlan);
            assert!(!errore.to_string().contains(numero));
        }
    }
}

proptest! {
    #[test]
    fn ogni_forma_breve_finita_sopravvive_al_round_trip(bits in any::<u64>()) {
        let valore = f64::from_bits(bits);
        if let Some(numero) = serde_json::Number::from_f64(valore) {
            let testo = format!(
                r#"{{"version":1,"inputs":[],"steps":[{{"out":"x","op":"table.conditional","in":[],"config":{{"value":{numero}}}}}],"outputs":[]}}"#
            );
            let piano = Pipeline::from_json(&testo).expect("forma breve finita");
            let serializzato = serde_json::to_string(&piano).expect("serializzazione");
            let riletto = Pipeline::from_json(&serializzato).expect("rilettura");
            prop_assert_eq!(riletto, piano);
        }
    }
}

#[test]
fn forme_esponenziali_brevi_nelle_config_e_nei_limiti() {
    for numero in [
        "1e3",
        "1e+3",
        "1E+3",
        "1.1e33",
        "1.1e+33",
        "1.1E+33",
        "1.1000000000000001e33",
        "1.1000000000000001E+33",
    ] {
        let testo = format!(
            r#"{{"version":1,"inputs":[],"steps":[{{"out":"x","op":"table.conditional","in":[],"config":{{"value":[{numero}]}}}}],"outputs":[],"limits":{{"max_expansion_factor":{numero}}}}}"#
        );
        let piano = Pipeline::from_json(&testo).expect("forma breve");
        let serializzato = serde_json::to_string(&piano).expect("serializzazione");
        let riletto = Pipeline::from_json(&serializzato).expect("rilettura");
        assert_eq!(riletto, piano);
    }
}
