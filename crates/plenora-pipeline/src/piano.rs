//! Il piano: una struttura per l'API Rust, letta dal JSON solo da
//! [`Pipeline::from_json`].
//!
//! `Pipeline` e `Passo` non implementano `Deserialize`: la config è un
//! `serde_json::Value`, e una deserializzazione serde diretta terrebbe in
//! silenzio l'ultima di due chiavi ripetute. L'unica lettura dal testo passa
//! dal controllo dei duplicati, a ogni profondità; il modello serde privato
//! ([`modello`]) esiste solo per lei.
//!
//! Il piano è in forma SSA: ogni nome è definito una volta sola, fra gli
//! `inputs` e le `out` dei passi, e un passo usa solo nomi definiti prima.
//! La forma è controllata da [`Pipeline::validate`](crate::Pipeline::validate),
//! non qui: questo modulo legge e basta.

use plenora_core::limits::{Limits, PlanLimits};
use plenora_core::{PlenoraError, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Unica versione del formato accettata.
pub const VERSIONE_PIANO: u32 = 1;

/// Piano di una pipeline.
///
/// ```json
/// {"version": 1,
///  "inputs": ["ordini"],
///  "steps": [{"out": "ordinati", "op": "table.sort", "in": ["ordini"],
///             "config": {"columns": ["id"]}}],
///  "outputs": ["ordinati"]}
/// ```
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Pipeline {
    /// Versione del formato: solo [`VERSIONE_PIANO`].
    pub version: u32,
    /// Nomi delle tabelle fornite dal chiamante.
    pub inputs: Vec<String>,
    /// CRS di piano, usato solo dai produttori geo (`geo.from_coords`,
    /// `geo.from_wkt`, `geo.generate_grid`). Si risolve in validazione,
    /// fail-closed, anche se nessun passo lo usa.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub crs: Option<String>,
    /// Limiti che sostituiscono quelli di `Limits::default()`, uno per uno.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limits: Option<LimitiParziali>,
    /// Passi, nell'ordine di esecuzione.
    pub steps: Vec<Passo>,
    /// Nomi delle tabelle restituite, nell'ordine dato.
    pub outputs: Vec<String>,
}

/// Un passo: un'operazione del catalogo applicata a tabelle già definite.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Passo {
    /// Nome della tabella prodotta.
    pub out: String,
    /// Id canonico dell'operazione (gli alias legacy sono rifiutati).
    pub op: String,
    /// Tabelle in ingresso, nell'ordine dell'operazione (left, right).
    #[serde(rename = "in")]
    pub inputs: Vec<String>,
    /// Config dell'operazione; nel JSON, assente vale `{}`.
    pub config: Value,
}

/// Modello serde del testo JSON, privato: lo usa solo
/// [`Pipeline::from_json`], dopo il controllo delle chiavi ripetute.
mod modello {
    use serde::Deserialize;
    use serde_json::Value;

    use super::LimitiParziali;

    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    pub(super) struct PipelineJson {
        pub(super) version: u32,
        pub(super) inputs: Vec<String>,
        #[serde(default)]
        pub(super) crs: Option<String>,
        #[serde(default)]
        pub(super) limits: Option<LimitiParziali>,
        pub(super) steps: Vec<PassoJson>,
        pub(super) outputs: Vec<String>,
    }

    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    pub(super) struct PassoJson {
        pub(super) out: String,
        pub(super) op: String,
        #[serde(rename = "in")]
        pub(super) inputs: Vec<String>,
        #[serde(default = "config_vuota")]
        pub(super) config: Value,
    }

    fn config_vuota() -> Value {
        Value::Object(serde_json::Map::new())
    }
}

/// Sostituzione parziale dei limiti: ogni campo presente sostituisce il
/// default, gli assenti restano quelli di `Limits::default()`.
///
/// Ci sono solo i limiti che il runner applica oggi. Gli altri campi di
/// `Limits` (`max_parallelism`, `max_payload_bytes`, `max_batches`,
/// `max_wkb_cell_bytes`, `max_geometry_depth`, `plan`) non sono dichiarabili:
/// accettarli senza applicarli sarebbe una garanzia promessa e non data.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LimitiParziali {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_input_rows: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output_rows: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_rows_per_edge: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_expansion_factor: Option<f64>,
    /// Passato ai kernel che lo applicano; il runner non limita ancora le
    /// tabelle residenti (README, «Runner»).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_governed_memory_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_temp_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spill_partitions: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_string_bytes: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_regex_bytes: Option<usize>,
}

impl LimitiParziali {
    /// Applica la sostituzione ai default e valida il risultato.
    ///
    /// # Errors
    ///
    /// `InvalidPlan` se i limiti risultanti non passano `Limits::validate`.
    pub fn applica(&self) -> Result<Limits> {
        let mut limiti = Limits::default();
        if let Some(valore) = self.max_input_rows {
            limiti.rows.max_input_rows = valore;
        }
        if let Some(valore) = self.max_output_rows {
            limiti.rows.max_output_rows = valore;
        }
        if let Some(valore) = self.max_rows_per_edge {
            limiti.rows.max_rows_per_edge = valore;
        }
        if let Some(valore) = self.max_expansion_factor {
            limiti.rows.max_expansion_factor = valore;
        }
        if let Some(valore) = self.max_governed_memory_bytes {
            limiti.max_governed_memory_bytes = valore;
        }
        if let Some(valore) = self.max_temp_bytes {
            limiti.max_temp_bytes = valore;
        }
        if let Some(valore) = self.spill_partitions {
            limiti.spill_partitions = valore;
        }
        if let Some(valore) = self.max_string_bytes {
            limiti.max_string_bytes = valore;
        }
        if let Some(valore) = self.max_regex_bytes {
            limiti.max_regex_bytes = valore;
        }
        limiti.validate()?;
        Ok(limiti)
    }
}

impl Pipeline {
    /// Legge un piano dal testo JSON.
    ///
    /// Le chiavi ripetute nello stesso oggetto sono rifiutate prima della
    /// deserializzazione: `serde_json` terrebbe l'ultima, e il piano eseguito
    /// non sarebbe quello scritto.
    ///
    /// # Errors
    ///
    /// `InvalidPlan` per testo oltre `max_plan_json_bytes`, chiavi duplicate
    /// a qualunque profondità, campi sconosciuti o forma non valida; la
    /// versione si controlla in validazione.
    ///
    /// ```compile_fail
    /// // Nessuna scorciatoia serde: la lettura passa da `from_json`.
    /// let _: plenora_pipeline::Pipeline = serde_json::from_str("{}").unwrap();
    /// ```
    pub fn from_json(testo: &str) -> Result<Self> {
        let massimo = PlanLimits::default().max_plan_json_bytes;
        if testo.len() > massimo {
            return Err(PlenoraError::InvalidPlan(format!(
                "piano di {} byte oltre max_plan_json_bytes {massimo}",
                testo.len()
            )));
        }
        plenora_core::json::ensure_no_duplicate_keys(testo)?;
        let letto: modello::PipelineJson = serde_json::from_str(testo)
            .map_err(|errore| PlenoraError::InvalidPlan(format!("piano non valido: {errore}")))?;
        Ok(Self {
            version: letto.version,
            inputs: letto.inputs,
            crs: letto.crs,
            limits: letto.limits,
            steps: letto
                .steps
                .into_iter()
                .map(|passo| Passo {
                    out: passo.out,
                    op: passo.op,
                    inputs: passo.inputs,
                    config: passo.config,
                })
                .collect(),
            outputs: letto.outputs,
        })
    }
}
