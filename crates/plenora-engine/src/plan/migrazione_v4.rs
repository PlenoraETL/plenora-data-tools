//! Migrazione esplicita dei piani `schema_version: 4` al canonico v5
//! (errori-e-limiti.md#memoria-governata).
//!
//! Un parser separato e non `serde(alias)`: il nome della v4 promette un tetto
//! sull'intero processo che in-process non esiste
//! (errori-e-limiti.md#che-cosa-la-memoria-governata-non-garantisce), e non
//! deve funzionare nella v5. Con `deny_unknown_fields` su entrambe le
//! strutture, un v5 col nome vecchio, un v4 col nome nuovo e un piano con
//! entrambe le chiavi si rifiutano per costruzione; le chiavi duplicate le
//! rifiuta prima `ensure_no_duplicate_keys`.
//!
//! La migrazione tocca solo `schema_version` e il blocco `limits`, ma passa
//! da `serde_json::Value`: spazi, ordine delle chiavi e forma dei numeri
//! possono cambiare. Si promettono l'equivalenza semantica dei valori non
//! toccati e quella canonica del risultato: stesso `plan_hash`
//! (piano-v5.md#identita-e-fingerprint).
//!
//! [`testo_canonico_v5`] e' l'unico ingresso pubblico ed e' idempotente anche
//! sull'esito; la funzione di migrazione interna rifiuta un piano gia'
//! migrato. Le funzioni interne allocano guidate dal contenuto, quindi
//! esporle aggirerebbe il tetto di errori-e-limiti.md.

use std::borrow::Cow;

use serde::Deserialize;
use serde_json::{Map, Value};

use plenora_core::limits::PlanLimits;
use plenora_core::{PlenoraError, Result};

use super::{LimitsOverride, PlanLimitsOverride, PLAN_SCHEMA_VERSION_V4, PLAN_SCHEMA_VERSION_V5};

#[cfg(test)]
mod tests;

/// Override dei limiti nella forma della v4: con `max_memory_bytes`.
///
/// Esiste solo per essere deserializzata dalla migrazione, e non e' esposta:
/// nessun percorso di esecuzione la vede. `deny_unknown_fields` e' cio' che
/// rende impossibile il nome nuovo in un piano v4.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct LimitsOverrideV4 {
    #[serde(default)]
    pub max_input_rows: Option<u64>,
    #[serde(default)]
    pub max_output_rows: Option<u64>,
    #[serde(default)]
    pub max_rows_per_edge: Option<u64>,
    #[serde(default)]
    pub max_expansion_factor: Option<f64>,
    #[serde(default)]
    pub plan: PlanLimitsOverride,
    /// Il nome vecchio. È l'unica differenza rispetto alla v5, ed è la
    /// ragione di questo modulo.
    #[serde(default)]
    pub max_memory_bytes: Option<u64>,
    #[serde(default)]
    pub max_temp_bytes: Option<u64>,
    #[serde(default)]
    pub spill_partitions: Option<u32>,
    #[serde(default)]
    pub max_parallelism: Option<u32>,
    #[serde(default)]
    pub max_wkb_cell_bytes: Option<u64>,
    #[serde(default)]
    pub max_payload_bytes: Option<u64>,
    #[serde(default)]
    pub max_batches: Option<u64>,
    #[serde(default)]
    pub max_geometry_depth: Option<u32>,
    #[serde(default)]
    pub max_string_bytes: Option<usize>,
    #[serde(default)]
    pub max_regex_bytes: Option<usize>,
}

impl LimitsOverrideV4 {
    /// Traduce gli override v4 nella forma v5.
    ///
    /// Costruzione per campi, non riscrittura di chiavi: un limite nuovo in
    /// [`LimitsOverride`] fa smettere di compilare questo letterale invece di
    /// cadere in silenzio dal piano migrato.
    const fn in_v5(self) -> LimitsOverride {
        let Self {
            max_input_rows,
            max_output_rows,
            max_rows_per_edge,
            max_expansion_factor,
            plan,
            max_memory_bytes,
            max_temp_bytes,
            spill_partitions,
            max_parallelism,
            max_wkb_cell_bytes,
            max_payload_bytes,
            max_batches,
            max_geometry_depth,
            max_string_bytes,
            max_regex_bytes,
        } = self;
        LimitsOverride {
            max_input_rows,
            max_output_rows,
            max_rows_per_edge,
            max_expansion_factor,
            plan,
            // L'unico campo che cambia nome. Il valore non si tocca: e' lo
            // stesso budget di ammissione, con un nome che lo dice
            // (errori-e-limiti.md#memoria-governata).
            max_governed_memory_bytes: max_memory_bytes,
            max_temp_bytes,
            spill_partitions,
            max_parallelism,
            max_wkb_cell_bytes,
            max_payload_bytes,
            max_batches,
            max_geometry_depth,
            max_string_bytes,
            max_regex_bytes,
        }
    }
}

/// Errore di migrazione: e' sempre un problema del piano fornito, mai
/// un'invariante interna.
const fn errore(messaggio: String) -> PlenoraError {
    PlenoraError::InvalidPlan(messaggio)
}

/// Legge `schema_version` da un testo JSON senza deserializzare il resto.
///
/// Sceglie il percorso prima di impegnarsi su una struttura, cosi' l'errore
/// e' di versione e non di campo sconosciuto.
///
/// # Errors
///
/// `PlenoraError::DataMapping` se il testo non e' JSON valido;
/// `PlenoraError::InvalidPlan` se il JSON non e' un oggetto, se
/// `schema_version` manca, non e' un intero non negativo o non sta in `u16`.
pub(super) fn versione_dichiarata(json_text: &str) -> Result<u16> {
    let valore: Value = serde_json::from_str(json_text)?;
    let oggetto = valore
        .as_object()
        .ok_or_else(|| errore("il piano deve essere un oggetto JSON".to_owned()))?;
    let versione = oggetto
        .get("schema_version")
        .ok_or_else(|| errore("il piano non dichiara `schema_version`".to_owned()))?;
    let numero = versione
        .as_u64()
        .ok_or_else(|| errore("`schema_version` deve essere un intero non negativo".to_owned()))?;
    u16::try_from(numero)
        .map_err(|_| errore(format!("`schema_version` fuori intervallo: {numero}")))
}

/// Migra il testo di un piano v4 nel testo di un piano v5.
///
/// `schema_version` diventa 5 e il blocco `limits` si ricostruisce con
/// `LimitsOverrideV4::in_v5`: `max_memory_bytes` diventa
/// `max_governed_memory_bytes`, con lo stesso valore
/// (errori-e-limiti.md#memoria-governata). Il passaggio dalla struttura v4
/// rifiuta qui un v4 con chiavi sconosciute, invece di lasciarlo arrivare
/// alla validazione v5 con un errore di versione sbagliata.
///
/// # Errors
///
/// `PlenoraError::DataMapping` se il testo non e' JSON valido, o se il blocco
/// `limits` non e' valido per la v4 (la stessa categoria che `PlanV5::parse`
/// produce per lo stesso difetto, da cui dipende l'exit code).
/// `PlenoraError::InvalidPlan` se contiene chiavi duplicate, se non dichiara
/// `schema_version: 4` o se non e' un oggetto.
/// `PlenoraError::Internal` solo per un fallimento di serializzazione,
/// impossibile su `Option<numero>` ma reso esplicito (R6).
fn migra_v4_a_v5(json_text: &str) -> Result<String> {
    plenora_core::json::ensure_no_duplicate_keys(json_text)?;
    let versione = versione_dichiarata(json_text)?;
    if versione != PLAN_SCHEMA_VERSION_V4 {
        return Err(errore(format!(
            "la migrazione v4->v5 accetta solo `schema_version: {PLAN_SCHEMA_VERSION_V4}`, \
             ricevuta {versione}"
        )));
    }

    let mut valore: Value = serde_json::from_str(json_text)?;
    let oggetto: &mut Map<String, Value> = valore
        .as_object_mut()
        .ok_or_else(|| errore("il piano deve essere un oggetto JSON".to_owned()))?;

    // Il blocco `limits` passa dalla struttura v4: e' qui che un nome nuovo o
    // una chiave sconosciuta vengono rifiutati, con l'errore che parla della
    // v4 e non di una versione che il piano non dichiara.
    if let Some(limiti) = oggetto.get("limits") {
        let v4: LimitsOverrideV4 =
            serde_json::from_value(limiti.clone()).map_err(|errore_serde| {
                // `DataMapping`, non `InvalidPlan`: e' la categoria che
                // `PlanV5::parse` produce per lo stesso identico difetto
                // scritto in v5, e da essa dipende l'exit code della CLI. Un
                // piano rifiutato deve dare la stessa risposta a chi lo
                // scrive nelle due versioni, altrimenti la migrazione cambia
                // il contratto d'errore invece di tradurre un nome.
                PlenoraError::DataMapping(format!(
                    "json error: limiti non validi per un piano v4: {errore_serde}. \
                     Un piano v4 dichiara `max_memory_bytes`; \
                     `max_governed_memory_bytes` appartiene alla v5 e non ha alias"
                ))
            })?;
        let v5 = serde_json::to_value(v4.in_v5()).map_err(|errore_serde| {
            PlenoraError::Internal(format!(
                "serializzazione dei limiti migrati fallita: {errore_serde}"
            ))
        })?;
        oggetto.insert("limits".to_owned(), v5);
    }

    oggetto.insert(
        "schema_version".to_owned(),
        Value::from(PLAN_SCHEMA_VERSION_V5),
    );

    serde_json::to_string(&valore).map_err(|errore_serde| {
        PlenoraError::Internal(format!(
            "serializzazione del piano migrato fallita: {errore_serde}"
        ))
    })
}

/// Porta un testo di piano alla versione canonica v5, migrandolo se dichiara
/// la v4.
///
/// E' il **solo** ingresso di versione del crate; un piano gia' v5 attraversa
/// senza copia (`Cow::Borrowed`). Il tetto `max_plan_json_bytes` si applica
/// al testo fornito, prima di costruire un albero JSON (errori-e-limiti.md),
/// e al testo migrato: il nome della v5 e' piu' lungo, e senza il secondo
/// controllo lo stesso input darebbe `Ok` e poi `Err`. Cosi' un v4 e il v5
/// equivalente si accettano o si rifiutano insieme.
///
/// # Errors
///
/// `PlenoraError::DataMapping` se il testo non e' JSON valido o se il blocco
/// `limits` di un piano v4 non e' valido;
/// `PlenoraError::InvalidPlan` se il testo **o il suo migrato** superano
/// `max_plan_json_bytes`, se contiene chiavi duplicate, se non dichiara una
/// `schema_version` leggibile, se dichiara una versione `<= 3` (quei piani
/// sono la forma lineare legacy: passano da [`super::PlanV5::from_legacy`],
/// che li porta al canonico v5 senza toccare la v4) o una versione futura.
pub fn testo_canonico_v5<'a>(json_text: &'a str, plan_limits: &PlanLimits) -> Result<Cow<'a, str>> {
    if json_text.len() > plan_limits.max_plan_json_bytes {
        return Err(errore(format!(
            "max_plan_json_bytes superato: {} byte > {}",
            json_text.len(),
            plan_limits.max_plan_json_bytes
        )));
    }
    // Le chiavi duplicate si rifiutano prima ancora di LEGGERE la versione:
    // `serde_json` risolverebbe `schema_version` con «vince l'ultima», e la
    // scelta del percorso finirebbe per dipendere da quale duplicato ha vinto.
    plenora_core::json::ensure_no_duplicate_keys(json_text)?;
    match versione_dichiarata(json_text)? {
        PLAN_SCHEMA_VERSION_V5 => Ok(Cow::Borrowed(json_text)),
        PLAN_SCHEMA_VERSION_V4 => {
            let migrato = migra_v4_a_v5(json_text)?;
            if migrato.len() > plan_limits.max_plan_json_bytes {
                return Err(errore(format!(
                    "max_plan_json_bytes superato dal piano migrato: {} byte > {} \
                     (il nome del budget di memoria nella v5 e' piu' lungo di nove byte; \
                     il v5 equivalente avrebbe la stessa dimensione e sarebbe rifiutato \
                     allo stesso modo)",
                    migrato.len(),
                    plan_limits.max_plan_json_bytes
                )));
            }
            Ok(Cow::Owned(migrato))
        }
        altra if altra <= 3 => Err(errore(format!(
            "`schema_version: {altra}` e' un piano lineare legacy: non ha un percorso DAG \
             diretto, va convertito dal chiamante legacy"
        ))),
        altra => Err(errore(format!(
            "`schema_version: {altra}` non e' supportata: la versione canonica e' \
             {PLAN_SCHEMA_VERSION_V5}, la {PLAN_SCHEMA_VERSION_V4} passa dalla migrazione"
        ))),
    }
}
