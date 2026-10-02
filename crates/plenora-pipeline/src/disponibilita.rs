//! Quali operazioni del catalogo il runner esegue.
//!
//! Il catalogo (`plenora_core::catalog::CATALOG`) elenca ogni operazione con
//! un kernel; il runner ne esegue tutte tranne quelle elencate qui, che la
//! validazione rifiuta con **ogni** config. Le operazioni che il runner
//! rifiuta solo con certe config (`table.pivot` senza `mapping`,
//! `table.flatten_json` senza `output_columns`) restano eseguibili: è la
//! validazione del piano a dire di no, con un errore esplicito.
//!
//! Chi pubblica il catalogo (la CLI `plenora-data`, `data.catalog`) usa
//! questo elenco per non dichiarare disponibile un'operazione che nessun
//! piano può eseguire (Capability Discovery 2.0, CAP-009). Le due direzioni
//! sono provate: le voci elencate si rifiutano davvero con una config
//! valida (test sotto), e ogni altra operazione esegue l'esempio della sua
//! scheda dal runner (`crates/plenora-io/tests/operazioni_doc.rs`).

/// Operazioni del catalogo che il runner rifiuta con ogni config, con il
/// motivo pubblico (senza dati, al più 512 caratteri).
pub const NON_ESEGUIBILI: &[(&str, &str)] = &[(
    "table.transpose",
    "the output schema depends on the data (one column per input row): the runner \
     rejects the operation at plan validation",
)];

/// Il motivo per cui il runner non esegue `op`, o `None` se la esegue.
#[must_use]
pub fn motivo_non_eseguibile(op: &str) -> Option<&'static str> {
    NON_ESEGUIBILI
        .iter()
        .find(|(id, _)| *id == op)
        .map(|(_, motivo)| *motivo)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use plenora_core::arrow::schema::{DataType, Field, Schema};
    use plenora_core::catalog::find_operation;
    use plenora_core::ErrorCategory;

    use super::{motivo_non_eseguibile, NON_ESEGUIBILI};
    use crate::Pipeline;

    /// Ogni voce è un'operazione del catalogo con un motivo pubblicabile, e
    /// la validazione la rifiuta con `Unsupported` anche con una config che
    /// l'analisi accetta in ogni sua regola.
    #[test]
    fn le_operazioni_elencate_si_rifiutano_con_una_config_valida() {
        assert!(motivo_non_eseguibile("table.transpose").is_some());
        assert!(motivo_non_eseguibile("table.pivot").is_none());
        for (op, motivo) in NON_ESEGUIBILI {
            assert!(find_operation(op).is_some_and(|d| d.id == *op), "{op}");
            assert!(!motivo.is_empty() && motivo.chars().count() <= 512, "{op}");
        }
        let schema = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Utf8, false),
            Field::new("a", DataType::Int64, true),
        ]));
        let piano = Pipeline::from_json(
            r#"{"version": 1, "inputs": ["t"], "steps": [
                {"out": "x", "op": "table.transpose", "in": ["t"],
                 "config": {"id_column": "id", "output_columns": ["r0"]}}],
                "outputs": ["x"]}"#,
        )
        .expect("piano");
        let errore = piano
            .validate(&[("t", schema)])
            .expect_err("il runner non esegue table.transpose");
        assert_eq!(errore.category(), ErrorCategory::Unsupported, "{errore}");
    }
}
