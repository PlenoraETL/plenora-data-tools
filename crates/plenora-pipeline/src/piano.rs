//! Il piano: una sola struttura serde, API Rust e formato JSON insieme.
//!
//! Il piano è in forma SSA: ogni nome è definito una volta sola, fra gli
//! `inputs` e le `out` dei passi, e un passo usa solo nomi definiti prima.
//! La forma è controllata da [`Pipeline::validate`](crate::Pipeline::validate),
//! non qui: questo modulo legge e basta.

use plenora_core::limits::Limits;
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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Passo {
    /// Nome della tabella prodotta.
    pub out: String,
    /// Id canonico dell'operazione (gli alias legacy sono rifiutati).
    pub op: String,
    /// Tabelle in ingresso, nell'ordine dell'operazione (left, right).
    #[serde(rename = "in")]
    pub inputs: Vec<String>,
    /// Config dell'operazione; assente vale `{}`.
    #[serde(default = "config_vuota")]
    pub config: Value,
}

fn config_vuota() -> Value {
    Value::Object(serde_json::Map::new())
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
    /// `InvalidPlan` per chiavi duplicate, campi sconosciuti o forma non
    /// valida; la versione si controlla in validazione.
    pub fn from_json(testo: &str) -> Result<Self> {
        plenora_core::json::ensure_no_duplicate_keys(testo)?;
        serde_json::from_str(testo)
            .map_err(|errore| PlenoraError::InvalidPlan(format!("piano non valido: {errore}")))
    }
}
