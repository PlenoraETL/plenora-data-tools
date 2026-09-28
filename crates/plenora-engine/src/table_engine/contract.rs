//! Contratto del piano tabellare: il formato lineare a compatibilita'
//! congelata (`schema_version <= 3`).

use serde::{Deserialize, Serialize};
use serde_json::Value;

use plenora_core::catalog::{
    find_operation, Arity, ExecutionClass, Family, Maturity, OperationDescriptor,
};
use plenora_core::{PlenoraError, Result};

use super::Limits;

pub const SCHEMA_VERSION: u16 = 1;
const HARD_MAX_STEPS: usize = 128;
const HARD_MAX_CONFIG_BYTES: usize = 256 * 1024;

/// Risolve l'id operazione di un piano (nudo legacy, es. `filter`, o
/// canonico `table.filter`) nel descrittore del catalogo unificato,
/// limitatamente alla famiglia tabellare: gli id geo restano sconosciuti
/// come nel catalogo storico di nogeo.
fn resolve_table_operation(name: &str) -> Option<&'static OperationDescriptor> {
    find_operation(name).filter(|descriptor| descriptor.family == Family::Table)
}

/// Nome nudo usato dal dispatch: agli id canonici `table.*` viene rimosso il
/// prefisso di namespace, gli id legacy passano invariati.
#[must_use]
pub fn dispatch_name(operation: &str) -> &str {
    operation.strip_prefix("table.").unwrap_or(operation)
}

/// Validazione dei valori dei limiti. Vive qui, e non accanto a [`Limits`],
/// perche' quella struct sta in `plenora-kernels-table`.
fn validate_limits(limits: &Limits) -> Result<()> {
    if limits.max_rows == 0
        || limits.max_columns == 0
        || limits.max_string_bytes == 0
        || limits.max_regex_bytes == 0
        || limits.max_split_columns == 0
        || limits.max_governed_memory_bytes == 0
        || limits.max_temp_bytes == 0
        || limits.spill_partitions < 2
        || limits.spill_partitions > 4_096
    {
        return Err(PlenoraError::InvalidPlan(
            "limiti nulli o spill_partitions fuori 2..=4096".into(),
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Step {
    pub operation: String,
    #[serde(default)]
    pub config: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Plan {
    pub schema_version: u16,
    /// I limiti **come li scrive il formato v1**, tradotti sul tipo Rust
    /// corrente da [`limiti_v1`]. Vedi il modulo per il perche'.
    #[serde(default, with = "limiti_v1")]
    pub limits: Limits,
    pub steps: Vec<Step>,
}

/// Il blocco `limits` del formato lineare **v1**, sul filo.
///
/// Il tipo Rust [`Limits`] usa `max_governed_memory_bytes`
/// (errori-e-limiti.md#memoria-governata); il formato v1 e' pubblicato e un
/// piano gia' scritto non cambia nome. Questa struttura privata e' una
/// traduzione, non un alias: vale solo dentro `schema_version: 1`, e
/// `deny_unknown_fields` rifiuta qui il nome della v5 come la v5 rifiuta quello
/// della v1.
///
/// La conversione e' per campi: un limite nuovo in [`Limits`] fa smettere di
/// compilare questi letterali, invece di sparire in silenzio dal formato v1.
mod limiti_v1 {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    use super::Limits;

    #[derive(Serialize, Deserialize)]
    #[serde(default, deny_unknown_fields)]
    struct LimitsV1 {
        max_rows: usize,
        max_columns: usize,
        max_string_bytes: usize,
        max_regex_bytes: usize,
        max_split_columns: usize,
        /// Il nome della v1. E' l'unica differenza rispetto al tipo Rust.
        max_memory_bytes: usize,
        max_temp_bytes: u64,
        spill_partitions: usize,
    }

    impl Default for LimitsV1 {
        fn default() -> Self {
            // Un solo posto in cui vivono i default, e non e' questo.
            Self::from(Limits::default())
        }
    }

    impl From<Limits> for LimitsV1 {
        fn from(limits: Limits) -> Self {
            let Limits {
                max_rows,
                max_columns,
                max_string_bytes,
                max_regex_bytes,
                max_split_columns,
                max_governed_memory_bytes,
                max_temp_bytes,
                spill_partitions,
            } = limits;
            Self {
                max_rows,
                max_columns,
                max_string_bytes,
                max_regex_bytes,
                max_split_columns,
                max_memory_bytes: max_governed_memory_bytes,
                max_temp_bytes,
                spill_partitions,
            }
        }
    }

    impl From<LimitsV1> for Limits {
        fn from(wire: LimitsV1) -> Self {
            let LimitsV1 {
                max_rows,
                max_columns,
                max_string_bytes,
                max_regex_bytes,
                max_split_columns,
                max_memory_bytes,
                max_temp_bytes,
                spill_partitions,
            } = wire;
            Self {
                max_rows,
                max_columns,
                max_string_bytes,
                max_regex_bytes,
                max_split_columns,
                max_governed_memory_bytes: max_memory_bytes,
                max_temp_bytes,
                spill_partitions,
            }
        }
    }

    /// Un piano v1 riserializzato torna a nominare i limiti come la v1: chi
    /// legge e riscrive un piano non deve ritrovarselo cambiato di formato.
    pub fn serialize<S>(limits: &Limits, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        LimitsV1::from(limits.clone()).serialize(serializer)
    }

    /// # Errors
    ///
    /// L'errore del deserializzatore: chiave sconosciuta (compreso il nome
    /// della v5), tipo sbagliato, JSON malformato.
    pub fn deserialize<'de, D>(deserializer: D) -> Result<Limits, D::Error>
    where
        D: Deserializer<'de>,
    {
        LimitsV1::deserialize(deserializer).map(Limits::from)
    }
}

#[derive(Debug, Clone)]
pub struct ValidatedPlan {
    limits: Limits,
    steps: Vec<Step>,
    /// Config tipizzate dei passi, allineate per indice a `steps`:
    /// deserializzate una volta in `Plan::validate`, mai nel percorso per
    /// batch. `Arc` per clonare il piano senza ri-allocare le config.
    prepared: std::sync::Arc<[super::executor::PreparedStep]>,
}

impl Plan {
    /// Valida l'intero piano prima che venga aperto qualunque input.
    ///
    /// # Errors
    ///
    /// Restituisce un errore se versione, limiti, operazioni o configurazioni
    /// non rispettano il contratto fail-closed.
    pub fn validate(self) -> Result<ValidatedPlan> {
        if self.schema_version != SCHEMA_VERSION {
            return Err(PlenoraError::InvalidPlan(format!(
                "schema_version {} non supportata; attesa {SCHEMA_VERSION}",
                self.schema_version
            )));
        }
        validate_limits(&self.limits)?;
        if self.steps.is_empty() {
            return Err(PlenoraError::InvalidPlan(
                "la catena non contiene passi".into(),
            ));
        }
        if self.steps.len() > HARD_MAX_STEPS {
            return Err(PlenoraError::InvalidPlan(format!(
                "troppi passi: {} > {HARD_MAX_STEPS}",
                self.steps.len()
            )));
        }

        let binary_steps = self
            .steps
            .iter()
            .filter(|step| {
                resolve_table_operation(&step.operation)
                    .is_some_and(|operation| operation.arity != Arity::Unary)
            })
            .count();
        if binary_steps > 0 && (binary_steps != 1 || self.steps.len() != 1) {
            return Err(PlenoraError::InvalidPlan(
                "il protocollo a due input accetta esattamente un passo binario".into(),
            ));
        }

        for (index, step) in self.steps.iter().enumerate() {
            if !step.config.is_object() {
                return Err(PlenoraError::InvalidPlan(format!(
                    "config del passo {index} deve essere un oggetto"
                )));
            }
            let config_size = serde_json::to_vec(&step.config)?.len();
            if config_size > HARD_MAX_CONFIG_BYTES {
                return Err(PlenoraError::InvalidPlan(format!(
                    "config del passo {index} troppo grande: {config_size} byte"
                )));
            }
            let operation = resolve_table_operation(&step.operation).ok_or_else(|| {
                PlenoraError::InvalidPlan(format!(
                    "operazione sconosciuta al passo {index}: {}",
                    step.operation
                ))
            })?;
            if operation.maturity == Maturity::Planned {
                return Err(PlenoraError::Unsupported(format!(
                    "{} e' catalogata ma non ancora validata",
                    operation.id
                )));
            }
            super::executor::validate_step_contract(step, &self.limits).map_err(|error| {
                PlenoraError::InvalidPlan(format!(
                    "config non valida al passo {index} ({}): {error}",
                    step.operation
                ))
            })?;
        }

        // La config di ogni passo e' deserializzata nella sua forma tipizzata
        // una volta qui; l'esecuzione per batch usa solo queste (il fallimento
        // e' irraggiungibile: stessa validazione di cui sopra).
        let prepared = self
            .steps
            .iter()
            .map(super::executor::prepare_step)
            .collect::<Result<Vec<_>>>()?;

        Ok(ValidatedPlan {
            limits: self.limits,
            steps: self.steps,
            prepared: prepared.into(),
        })
    }
}

impl ValidatedPlan {
    #[must_use]
    pub const fn limits(&self) -> &Limits {
        &self.limits
    }

    /// Copia del piano con `max_governed_memory_bytes` **ridotto** al valore indicato.
    ///
    /// Nel percorso legacy della CLI il budget e' un tetto globale del piano:
    /// la memoria trattenuta da un input caricato non e' piu' disponibile, e i
    /// kernel devono vedere cio' che resta. Il budget puo' solo scendere.
    ///
    /// # Errors
    ///
    /// [`PlenoraError::ResourceLimit`] se il budget residuo e' zero: «non
    /// resta memoria» e' un limite di risorsa, non un piano sbagliato.
    pub fn with_memory_budget(&self, bytes: usize) -> Result<Self> {
        if bytes == 0 {
            return Err(PlenoraError::ResourceLimit(
                "budget di memoria esaurito: nessuna memoria residua per l'esecuzione".into(),
            ));
        }
        let mut ridotto = self.clone();
        ridotto.limits.max_governed_memory_bytes = bytes.min(self.limits.max_governed_memory_bytes);
        Ok(ridotto)
    }

    #[must_use]
    pub fn steps(&self) -> &[Step] {
        &self.steps
    }

    /// Config tipizzate dei passi, allineate per indice a [`Self::steps`]
    /// (configurazioni preparate, hot path minimale: percorso per batch senza parsing JSON).
    #[must_use]
    pub(crate) fn prepared_steps(&self) -> &[super::executor::PreparedStep] {
        &self.prepared
    }

    #[must_use]
    pub fn requires_secondary(&self) -> bool {
        self.steps.iter().any(|step| {
            resolve_table_operation(&step.operation)
                .is_some_and(|operation| operation.arity != Arity::Unary)
        })
    }

    #[must_use]
    pub fn requires_blocking(&self) -> bool {
        self.steps.iter().any(|step| {
            resolve_table_operation(&step.operation)
                .is_some_and(|operation| operation.execution_class != ExecutionClass::Streaming)
        })
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn plan(operation: &str, config: Value) -> Plan {
        Plan {
            schema_version: SCHEMA_VERSION,
            limits: Limits::default(),
            steps: vec![Step {
                operation: operation.into(),
                config,
            }],
        }
    }

    #[test]
    fn validates_known_executable_operation() {
        assert!(plan("drop_columns", json!({"columns": ["secret"]}))
            .validate()
            .is_ok());
    }

    #[test]
    fn rejects_unknown_and_malformed_operations() {
        assert!(plan("geo_buffer", json!({})).validate().is_err());
        assert!(plan("join", json!({})).validate().is_err());
    }

    #[test]
    fn rejects_non_object_config_and_empty_chain() {
        assert!(plan("drop_columns", json!([])).validate().is_err());
        assert!(Plan {
            schema_version: SCHEMA_VERSION,
            limits: Limits::default(),
            steps: vec![],
        }
        .validate()
        .is_err());
    }

    #[test]
    fn unknown_json_fields_are_rejected() {
        let value = json!({"schema_version": 1, "steps": [], "surprise": true});
        assert!(serde_json::from_value::<Plan>(value).is_err());
    }

    /// Il rifiuto atteso: `InvalidPlan`, con il frammento che identifica la
    /// guardia che l'ha deciso.
    fn rifiutato(plan: Plan, guardia: &str) {
        match plan.validate() {
            Err(PlenoraError::InvalidPlan(motivo)) => {
                assert!(
                    motivo.contains(guardia),
                    "guardia attesa `{guardia}`: {motivo}"
                );
            }
            Err(altro) => panic!("atteso InvalidPlan per `{guardia}`, ottenuto {altro:?}"),
            Ok(_) => panic!("piano accettato, atteso il rifiuto di `{guardia}`"),
        }
    }

    // La guardia di maturita' (`Maturity::Planned` -> `Unsupported`) non ha
    // un caso qui: nessuna operazione tabellare del catalogo e' `Planned`, e
    // il catalogo e' statico, quindi il ramo non e' raggiungibile da un
    // piano. Il nome del test non la promette piu'.
    #[test]
    fn rejects_invalid_versions_limits_sizes_and_configs() {
        let mut wrong_version = plan("drop_columns", json!({"columns": []}));
        wrong_version.schema_version = 99;
        rifiutato(wrong_version, "schema_version 99 non supportata");

        let mut zero_limit = plan("drop_columns", json!({"columns": []}));
        zero_limit.limits.max_rows = 0;
        rifiutato(zero_limit, "limiti nulli");

        let repeated = Step {
            operation: "drop_columns".into(),
            config: json!({"columns": []}),
        };
        rifiutato(
            Plan {
                schema_version: SCHEMA_VERSION,
                limits: Limits::default(),
                steps: vec![repeated; HARD_MAX_STEPS + 1],
            },
            "troppi passi",
        );

        rifiutato(
            plan(
                "drop_columns",
                json!({"columns": ["x".repeat(HARD_MAX_CONFIG_BYTES)]}),
            ),
            "config del passo 0 troppo grande",
        );
        let binary = plan("concat", json!({}))
            .validate()
            .expect("valid binary plan");
        assert!(binary.requires_secondary());
        rifiutato(
            Plan {
                schema_version: SCHEMA_VERSION,
                limits: Limits::default(),
                steps: vec![
                    Step {
                        operation: "concat".into(),
                        config: json!({}),
                    },
                    Step {
                        operation: "drop_columns".into(),
                        config: json!({"columns": []}),
                    },
                ],
            },
            "esattamente un passo binario",
        );
        rifiutato(
            plan("aggregate", json!({})),
            "config non valida al passo 0 (aggregate)",
        );
        rifiutato(
            plan(
                "string_pad",
                json!({"column": "x", "width": 2, "side": "left", "fill_char": "xx", "output_column": null}),
            ),
            "config non valida al passo 0 (string_pad)",
        );
    }

    #[test]
    fn canonical_table_ids_resolve_like_legacy_bare_ids() {
        assert!(plan("table.drop_columns", json!({"columns": ["secret"]}))
            .validate()
            .is_ok());
        let binary = plan("table.concat", json!({}))
            .validate()
            .expect("valid canonical binary plan");
        assert!(binary.requires_secondary());
    }
}

#[cfg(test)]
mod tests_limiti_v1 {
    //! Il formato v1 sul filo (errori-e-limiti.md#memoria-governata):
    //! conserva il proprio nome, e non accetta quello della v5.

    use serde_json::json;

    use super::{Limits, Plan, SCHEMA_VERSION};

    fn testo(limiti: &serde_json::Value) -> String {
        json!({
            "schema_version": SCHEMA_VERSION,
            "limits": limiti,
            "steps": [{"operation": "drop_columns", "config": {"columns": []}}]
        })
        .to_string()
    }

    #[test]
    fn il_nome_della_v1_viene_tradotto_sul_campo_corrente() {
        let plan: Plan = serde_json::from_str(&testo(&json!({"max_memory_bytes": 4096})))
            .expect("un piano v1 scrive il nome della v1");
        assert_eq!(plan.limits.max_governed_memory_bytes, 4096);
    }

    #[test]
    fn il_nome_della_v5_non_e_accettato_nella_v1() {
        // Se passasse, avremmo scritto un alias e chiamato traduzione.
        let errore = serde_json::from_str::<Plan>(&testo(&json!({
            "max_governed_memory_bytes": 4096
        })))
        .expect_err("il nome della v5 non appartiene alla v1")
        .to_string();
        assert!(errore.contains("max_governed_memory_bytes"), "{errore}");
    }

    #[test]
    fn le_due_chiavi_insieme_sono_rifiutate() {
        let errore = serde_json::from_str::<Plan>(&testo(&json!({
            "max_memory_bytes": 4096,
            "max_governed_memory_bytes": 4096
        })))
        .expect_err("una delle due e' sempre sconosciuta")
        .to_string();
        assert!(errore.contains("max_governed_memory_bytes"), "{errore}");
    }

    #[test]
    fn gli_altri_limiti_v1_attraversano_intatti() {
        let plan: Plan = serde_json::from_str(&testo(&json!({
            "max_rows": 7,
            "max_columns": 8,
            "max_string_bytes": 9,
            "max_regex_bytes": 10,
            "max_split_columns": 11,
            "max_memory_bytes": 12,
            "max_temp_bytes": 13,
            "spill_partitions": 14
        })))
        .expect("piano v1");
        let Limits {
            max_rows,
            max_columns,
            max_string_bytes,
            max_regex_bytes,
            max_split_columns,
            max_governed_memory_bytes,
            max_temp_bytes,
            spill_partitions,
        } = plan.limits;
        assert_eq!(
            (
                max_rows,
                max_columns,
                max_string_bytes,
                max_regex_bytes,
                max_split_columns,
                max_governed_memory_bytes,
                max_temp_bytes,
                spill_partitions
            ),
            (7, 8, 9, 10, 11, 12, 13, 14)
        );
    }

    #[test]
    fn i_limiti_omessi_restano_quelli_di_default() {
        let plan: Plan =
            serde_json::from_str(&testo(&json!({"max_memory_bytes": 4096}))).expect("piano v1");
        assert_eq!(plan.limits.max_rows, Limits::default().max_rows);
        assert_eq!(
            plan.limits.spill_partitions,
            Limits::default().spill_partitions
        );
    }

    #[test]
    fn riserializzare_un_piano_v1_riscrive_il_nome_della_v1() {
        // Chi legge un piano e lo riscrive non deve ritrovarselo cambiato di
        // formato: sarebbe una migrazione silenziosa verso un formato che il
        // documento non dichiara.
        let plan: Plan =
            serde_json::from_str(&testo(&json!({"max_memory_bytes": 4096}))).expect("piano v1");
        let riscritto = serde_json::to_value(&plan).expect("serializzazione");
        assert_eq!(riscritto["limits"]["max_memory_bytes"], json!(4096));
        assert!(riscritto["limits"]
            .get("max_governed_memory_bytes")
            .is_none());

        let ririletto: Plan = serde_json::from_value(riscritto).expect("round-trip");
        assert_eq!(ririletto.limits.max_governed_memory_bytes, 4096);
    }
}
