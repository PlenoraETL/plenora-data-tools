//! `null` non è l'assenza: un campo facoltativo del piano scritto `null`
//! (`crs`, `limits`, un limite) è un piano malformato, come per lo schema
//! `data-plan-v1` dei contratti, e non un campo omesso.

use plenora_core::ErrorCategory;
use plenora_pipeline::Pipeline;

const PIANO: &str = r#"{"version": 1, "inputs": ["t"], CAMPO
  "steps": [{"out": "u", "op": "table.limit", "in": ["t"], "config": {"n": 1}}],
  "outputs": ["u"]}"#;

#[test]
fn un_campo_facoltativo_null_e_un_piano_malformato() {
    for campo in [
        r#""crs": null,"#,
        r#""limits": null,"#,
        r#""limits": {"max_input_rows": null},"#,
        r#""limits": {"max_expansion_factor": null},"#,
        r#""limits": {"max_regex_bytes": null},"#,
    ] {
        let errore = Pipeline::from_json(&PIANO.replace("CAMPO", campo))
            .expect_err("un null non e' un campo omesso");
        assert_eq!(errore.category(), ErrorCategory::InvalidPlan, "{campo}");
    }
    // Omessi, o presenti con un valore, si leggono.
    for campo in [
        "",
        r#""crs": "EPSG:32632","#,
        r#""limits": {"max_input_rows": 10},"#,
    ] {
        assert!(
            Pipeline::from_json(&PIANO.replace("CAMPO", campo)).is_ok(),
            "{campo}"
        );
    }
}
