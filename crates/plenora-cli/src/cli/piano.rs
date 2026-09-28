//! Il piano come documento di controllo: lettura limitata, sonde di
//! versione e di input, parser unico.

use std::borrow::Cow;
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;

use plenora_core::limits::PlanLimits;
use plenora_core::{ErrorPhase, PlenoraError};
use plenora_engine::plan::{migrazione_v4, PLAN_SCHEMA_VERSION_V4, PLAN_SCHEMA_VERSION_V6};
use serde::Deserialize;

use crate::contract;

// ---------------------------------------------------------------------------
// DAG: scoperta dei contratti, validate e run
// ---------------------------------------------------------------------------
//
// - scoperta dei contratti dal solo header IPC (`cli::contract_discovery`),
//   con i metadati incoerenti rifiutati e il `FieldId` provvisorio;
// - decisioni CRS del piano (`crs_decisions`, R4.6.3) applicate prima della
//   validazione;
// - accoppiamento degli input di `--input`/`--inputs` a quelli dichiarati;
// - `validate`: `planner::validate` (D8) ed `explain`, cosi' un piano fuori
//   dal dispatch fallisce qui; riepilogo JSON su stdout;
// - `run`: `execute` con input lazy e publish atomico no-clobber; metriche
//   per nodo e per segmento in JSON su stdout.

/// Sonda del solo `schema_version`: decide il percorso (DAG vs legacy).
#[derive(Debug, Deserialize)]
struct PlanVersionProbe {
    schema_version: u32,
}

/// Sonda dei nomi di input dichiarati dal piano DAG (accoppiamento posizionale
/// con i percorsi CLI; la validazione vera resta al planner) e delle
/// decisioni CRS esplicite (R4.6.3, applicate da [`apply_crs_decisions`]).
#[derive(Debug, Deserialize)]
pub struct PlanInputsProbe {
    #[serde(default)]
    pub inputs: Vec<String>,
    #[serde(default)]
    pub crs_decisions: std::collections::BTreeMap<String, String>,
    /// Il blocco `limits` del piano, **grezzo**: la sonda ne legge il solo
    /// `plan.max_inputs`, e un blocco malformato a qualunque livello lo giudica
    /// il planner, col proprio messaggio. Una struttura tipizzata qui
    /// fallirebbe prima, con un altro.
    #[serde(default)]
    pub limits: Option<serde_json::Value>,
}

impl PlanInputsProbe {
    /// Il tetto sugli input che `planner::validate` applichera': il default,
    /// oppure l'override del piano se e' un intero che lo abbassa. Un override
    /// piu' alto il planner lo rifiuta, e qui vale il default.
    pub fn tetto_ingressi(&self) -> usize {
        let predefinito = plenora_core::limits::Limits::default().plan.max_inputs;
        self.limits
            .as_ref()
            .and_then(|limiti| limiti.get("plan"))
            .and_then(|piano| piano.get("max_inputs"))
            .and_then(serde_json::Value::as_u64)
            .and_then(|valore| usize::try_from(valore).ok())
            .filter(|valore| *valore <= predefinito)
            .unwrap_or(predefinito)
    }
}

/// `schema_version` del piano, senza validazione strutturale.
fn plan_schema_version(plan_text: &str) -> Result<u32, PlenoraError> {
    Ok(da_testo_di_controllo::<PlanVersionProbe>(plan_text)?.schema_version)
}

/// Deserializza un documento di controllo (piano, schema di comando) dal suo
/// testo.
///
/// Un documento che non ha la forma attesa e' un rifiuto in validazione:
/// senza il tag, `DataMapping` deriverebbe la fase `write` anche in un
/// comando che non scrive nulla. E' l'unico parser dei documenti di
/// controllo, perche' la fase non dipenda da quale comando li legge.
pub fn da_testo_di_controllo<T: serde::de::DeserializeOwned>(
    text: &str,
) -> Result<T, PlenoraError> {
    serde_json::from_str(text)
        .map_err(|error| PlenoraError::from(error).with_phase(ErrorPhase::Validate))
}

/// Fissa **un solo testo** per un piano DAG, e lo rende; `None` se il piano
/// dichiara la forma lineare legacy (`schema_version <= 3`), che prosegue sul
/// percorso invariato.
///
/// La v4 migra al canonico v5; v5 e v6 si prestano invariate. La CLI sonda il
/// piano piu' volte prima del planner, e tutte le sonde devono leggere lo
/// stesso testo.
pub fn testo_piano_dag(plan_text: &str) -> Result<Option<Cow<'_, str>>, PlenoraError> {
    let versione = plan_schema_version(plan_text)?;
    if versione < u32::from(PLAN_SCHEMA_VERSION_V4) {
        return Ok(None);
    }
    // La v6 non si migra e non si tocca: e' gia' il testo che il suo parser
    // legge, e riscriverlo qui — anche solo per rinormalizzarlo — cambierebbe
    // un documento che porta la propria identita'. Passarlo da
    // `testo_canonico_v5` lo farebbe **rifiutare**, perche' quella funzione
    // conosce solo la v4 e la v5: nessun piano v6 arriverebbe al planner.
    if versione == u32::from(PLAN_SCHEMA_VERSION_V6) {
        return Ok(Some(Cow::Borrowed(plan_text)));
    }
    migrazione_v4::testo_canonico_v5(plan_text, &PlanLimits::default()).map(Some)
}

/// Tetto sui byte di un documento JSON di controllo letto da file.
///
/// Coincide con `PlanLimits::max_plan_json_bytes` di default: i piani legacy
/// e gli schemi di comando sono documenti di controllo della stessa classe, e
/// non c'e' ragione perche' abbiano un tetto diverso — o nessun tetto.
const MAX_CONTROL_JSON_BYTES: u64 = 16 * 1024 * 1024;

/// Legge un documento JSON di CONTROLLO da file: limitato nei byte e
/// rifiutato se contiene chiavi duplicate.
///
/// E' l'unico lettore dei documenti di controllo: `serde_json::from_reader`
/// non ha tetto e fa vincere l'ultima chiave duplicata. Gli errori nascono
/// leggendo la sorgente (BLOCK-03).
pub fn read_control_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, PlenoraError> {
    let text = read_control_json_text(path)?;
    plenora_core::json::ensure_no_duplicate_keys(&text)?;
    da_testo_di_controllo(&text)
}

/// Testo di un documento JSON di controllo, entro [`MAX_CONTROL_JSON_BYTES`].
fn read_control_json_text(path: &Path) -> Result<String, PlenoraError> {
    let read = (|| -> Result<String, PlenoraError> {
        let file = File::open(path)?;
        let declared = file.metadata()?.len();
        if declared > MAX_CONTROL_JSON_BYTES {
            return Err(contract(format!(
                "documento di controllo da {declared} byte oltre il limite {MAX_CONTROL_JSON_BYTES}"
            )));
        }
        // Il tetto si applica anche alla lettura, non solo alla dimensione
        // dichiarata: fra `metadata()` e la lettura il file puo' crescere.
        let mut text = String::new();
        BufReader::with_capacity(64 * 1024, file)
            .take(MAX_CONTROL_JSON_BYTES.saturating_add(1))
            .read_to_string(&mut text)?;
        if text.len() as u64 > MAX_CONTROL_JSON_BYTES {
            return Err(contract(format!(
                "documento di controllo oltre il limite {MAX_CONTROL_JSON_BYTES} byte"
            )));
        }
        Ok(text)
    })();
    read.map_err(|error| error.with_phase(ErrorPhase::Read))
}

/// Contratto assente per un input gia' accoppiato: invariante nostra, non un
/// errore del chiamante.
pub fn contract_error_missing(name: &str) -> PlenoraError {
    PlenoraError::Internal(format!(
        "contratto di discovery assente per l'input `{name}`"
    ))
}

/// Testo del piano, letto una volta e gia' verificato contro le chiavi
/// duplicate.
///
/// Il controllo sul testo vale per tutte le sonde successive; i parser di
/// formato e il dispatch lo ripetono, ed e' idempotente.
pub fn read_control_plan_text(path: &Path) -> Result<String, PlenoraError> {
    let text = read_control_json_text(path)?;
    plenora_core::json::ensure_no_duplicate_keys(&text)?;
    Ok(text)
}

#[cfg(test)]
mod tests {
    /// Il tetto dei documenti di controllo e' scritto qui, ma il commento lo
    /// dichiara uguale a quello del piano in core: questo test e' cio' che
    /// rende vera la frase se uno dei due cambia.
    #[test]
    fn il_tetto_dei_documenti_di_controllo_e_quello_del_piano() {
        let del_piano = plenora_core::limits::PlanLimits::default().max_plan_json_bytes;
        assert_eq!(
            usize::try_from(super::MAX_CONTROL_JSON_BYTES).ok(),
            Some(del_piano)
        );
    }
}
