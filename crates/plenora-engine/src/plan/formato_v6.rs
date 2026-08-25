//! Formato del piano `schema_version: 6` — `max_domain_memory_bytes`.
//!
//! Ratificato in `plenora-contracts` come `Plan Budget 1.0`
//! (`plenora-plan-budget-v1`), requisiti `PLAN-001` … `PLAN-021`.
//!
//! # Perche' una versione nuova e non un campo in piu'
//!
//! `LimitsOverride` ha `deny_unknown_fields`. Un lettore v5 messo davanti a
//! un piano che dichiara il campo nuovo non lo ignora: **rifiuta il
//! documento**. L'aggiunta dentro la v5 sarebbe percio' compatibile
//! all'indietro — i piani vecchi restano validi — e **non** in avanti, e
//! l'incompatibilita' in avanti e' quella che rompe i dispiegamenti, perche'
//! e' li' che i lettori vecchi esistono gia'.
//!
//! E' lo stesso ragionamento, e lo stesso precedente, della migrazione v4→v5
//! ([`super::migrazione_v4`]): `deny_unknown_fields` e' una scelta
//! deliberata di questo formato, e convivere con una sua conseguenza
//! fingendo che sia additiva la contraddirebbe.
//!
//! # Che cosa NON fa questo modulo
//!
//! **Non migra.** Non esiste un `PlanV5 -> PlanV6`, e non e' una svista:
//! `PLAN-021` vieta di presentare i due documenti come identita' equivalenti,
//! e una migrazione che cambia identita' va progettata con chi consuma gli
//! hash — come comunicare il nuovo, come invalidare cache e grafi
//! persistiti, quale tipo rappresenti la vecchia e la nuova. Nessuno di
//! quei consumatori esiste oggi. La v5 e la v6 sono **percorsi paralleli**:
//! un piano v6 si scrive o si deserializza esplicitamente.
//!
//! # Come si separano le identita'
//!
//! La v6 non collassa nel canonico v5, e non lo fa **cambiando la versione a
//! un piano v5**: quella era una stesura precedente, e produceva un `PlanV5`
//! che dichiarava `6` senza portare il tetto — un valore che nessun parser
//! puo' generare.
//!
//! Qui il documento validato e' un [`super::ValidatedPlanV6`], che conserva
//! un [`PlanV6`] intero. La sua forma canonica dichiara `schema_version: 6`,
//! vi materializza il tetto quando c'e', e il separatore di dominio del
//! `plan_hash` e' scelto dalla **variante** del piano validato, non da un
//! campo mutabile. Un v5 e un v6 per il resto identici hanno percio'
//! identita' **diverse** (`PLAN-018`).
//!
//! Attenzione a non generalizzare: la v4 **conserva** l'equivalenza con la
//! v5, perche' e' migrata nel canonico v5 e ne condivide il `plan_hash`. Il
//! confine d'identita' e' fra v5 e v6, e li' soltanto (`PLAN-019`).

use serde::{Deserialize, Serialize};

use plenora_core::limits::PlanLimits;
use plenora_core::Result;

use super::{
    contract_error, LimitsOverride, NodeV5, PlanLimitsOverride, PlanV5, ValidatedPlanV6,
    PLAN_SCHEMA_VERSION_V6,
};
use std::collections::BTreeMap;

/// Override dei limiti **come li scrive la v6**: con
/// `max_domain_memory_bytes`.
///
/// E' pubblica e va nei due versi — vedi la nota sulla simmetria piu' sotto —
/// perche' e' il blocco `limits` di un documento pubblico.
/// `deny_unknown_fields` e' cio' che rende impossibile, per costruzione e non
/// per controllo:
///
/// - un piano **v5 che dichiara `max_domain_memory_bytes`** — lo
///   deserializza [`LimitsOverride`], che non conosce quel nome (`PLAN-007`);
/// - un piano **v6 con un nome che la v6 non ha** — lo deserializza questa,
///   che conosce solo i nomi della v6.
/// # Simmetrica alla v5, anche in scrittura
///
/// Deriva `Serialize` con gli stessi `skip_serializing_if` di
/// [`LimitsOverride`]: un tipo pubblico di piano che si puo' leggere e non
/// scrivere sarebbe asimmetrico senza motivo, e chi costruisce un v6 a mano
/// deve poterlo emettere. Un limite assente non compare, come nella v5, cosi'
/// il giro `serializza -> deserializza` rende lo stesso documento.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LimitsOverrideV6 {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_input_rows: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output_rows: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_rows_per_edge: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_expansion_factor: Option<f64>,
    #[serde(default)]
    pub plan: PlanLimitsOverride,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_governed_memory_bytes: Option<u64>,
    /// Il tetto di memoria che il piano **richiede** per il proprio dominio
    /// di isolamento. E' l'unica differenza rispetto alla v5, ed e' la
    /// ragione di questo modulo.
    ///
    /// Facoltativo (`PLAN-009`). La sua assenza significa una cosa sola: il
    /// profilo isolato **non e' selezionabile** per quel piano — mai un
    /// tetto implicito (`PLAN-010`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_domain_memory_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_temp_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spill_partitions: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_parallelism: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_wkb_cell_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_payload_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_batches: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_geometry_depth: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_string_bytes: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_regex_bytes: Option<usize>,
}

impl LimitsOverrideV6 {
    /// Separa gli override della v6 nei due pezzi che il resto della fase 1
    /// tratta diversamente: quelli **condivisi** con la v5, e il tetto del
    /// dominio, che la v5 non ha.
    ///
    /// E' una costruzione per campi, non una riscrittura di chiavi: se
    /// domani la v5 guadagnasse un limite, questa funzione non
    /// compilerebbe finche' qualcuno non decide che cosa la v6 ne fa.
    pub(crate) fn condivisi(&self) -> LimitsOverride {
        self.clone().separa().0
    }

    /// Ricompone gli override v6 dalla forma condivisa piu' il tetto.
    ///
    /// L'inverso esatto di [`Self::separa`], e per campi come quella: le due
    /// insieme non possono perdere un limite senza smettere di compilare.
    pub(crate) const fn da_condivisi(condivisi: LimitsOverride, tetto: Option<u64>) -> Self {
        let LimitsOverride {
            max_input_rows,
            max_output_rows,
            max_rows_per_edge,
            max_expansion_factor,
            plan,
            max_governed_memory_bytes,
            max_temp_bytes,
            spill_partitions,
            max_parallelism,
            max_wkb_cell_bytes,
            max_payload_bytes,
            max_batches,
            max_geometry_depth,
            max_string_bytes,
            max_regex_bytes,
        } = condivisi;
        Self {
            max_input_rows,
            max_output_rows,
            max_rows_per_edge,
            max_expansion_factor,
            plan,
            max_governed_memory_bytes,
            max_domain_memory_bytes: tetto,
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

    const fn separa(self) -> (LimitsOverride, Option<u64>) {
        let Self {
            max_input_rows,
            max_output_rows,
            max_rows_per_edge,
            max_expansion_factor,
            plan,
            max_governed_memory_bytes,
            max_domain_memory_bytes,
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
        let condivisi = LimitsOverride {
            max_input_rows,
            max_output_rows,
            max_rows_per_edge,
            max_expansion_factor,
            plan,
            max_governed_memory_bytes,
            max_temp_bytes,
            spill_partitions,
            max_parallelism,
            max_wkb_cell_bytes,
            max_payload_bytes,
            max_batches,
            max_geometry_depth,
            max_string_bytes,
            max_regex_bytes,
        };
        (condivisi, max_domain_memory_bytes)
    }
}

/// Piano `schema_version: 6`.
///
/// Ha gli stessi nodi, archi e contratti della v5: l'unica differenza e' il
/// blocco `limits`, che qui puo' portare `max_domain_memory_bytes`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanV6 {
    pub schema_version: u16,
    #[serde(default)]
    pub limits: LimitsOverrideV6,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub crs: Option<String>,
    #[serde(default)]
    pub inputs: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub crs_decisions: BTreeMap<String, String>,
    pub nodes: Vec<NodeV5>,
    pub output: String,
}

impl PlanV6 {
    /// La **vista v5** della struttura: gli stessi nodi, archi e limiti
    /// condivisi, senza il tetto del dominio.
    ///
    /// Serve a due cose, entrambe interne: validare la struttura col nucleo
    /// condiviso, e costruire la forma canonica — che poi dichiara la
    /// versione 6 e vi aggiunge il tetto. Non e' pubblica perche' un `PlanV5`
    /// ricavato da un v6 non e' un piano v5: e' una proiezione, e trattarla
    /// come documento produrrebbe un `plan_hash` del dominio sbagliato.
    pub(crate) fn come_struttura_condivisa(&self) -> PlanV5 {
        PlanV5 {
            schema_version: super::PLAN_SCHEMA_VERSION_V5,
            limits: self.limits.condivisi(),
            crs: self.crs.clone(),
            inputs: self.inputs.clone(),
            crs_decisions: self.crs_decisions.clone(),
            nodes: self.nodes.clone(),
            output: self.output.clone(),
        }
    }

    /// Parse completo di un piano v6.
    ///
    /// Passa dallo **stesso** nucleo strutturale della v5
    /// ([`PlanV5::valida_struttura_condivisa`]) dopo aver separato il campo
    /// che la v5 non conosce, e vi aggiunge i controlli del contratto sul
    /// tetto del dominio.
    ///
    /// # Errors
    ///
    /// `PlenoraError::InvalidPlan` per ogni violazione di limiti, struttura o
    /// dei requisiti `PLAN-008` e `PLAN-011`; `PlenoraError::DataMapping` per
    /// JSON malformato.
    pub fn parse(json_text: &str, plan_limits: &PlanLimits) -> Result<ValidatedPlanV6> {
        if json_text.len() > plan_limits.max_plan_json_bytes {
            return Err(contract_error(format!(
                "max_plan_json_bytes superato: {} byte > {}",
                json_text.len(),
                plan_limits.max_plan_json_bytes
            )));
        }
        // Stessa ragione della v5: `serde_json` risolverebbe le chiavi
        // duplicate con «vince l'ultima», e la risoluzione avverrebbe prima
        // della validazione e prima del `plan_hash`.
        plenora_core::json::ensure_no_duplicate_keys(json_text)?;
        let piano: Self = serde_json::from_str(json_text)?;
        if piano.schema_version != PLAN_SCHEMA_VERSION_V6 {
            return Err(contract_error(format!(
                "schema_version {} non e' un piano v6",
                piano.schema_version
            )));
        }
        let Self {
            schema_version,
            limits,
            crs,
            inputs,
            crs_decisions,
            nodes,
            output,
        } = piano;
        let (condivisi, tetto_del_dominio) = limits.separa();
        // `PLAN-008`: intero positivo. Lo zero non e' «nessun tetto» — quello
        // si dice omettendo il campo — ed e' un dominio che non puo' allocare
        // nulla, cioe' una configurazione che non puo' riuscire.
        if tetto_del_dominio == Some(0) {
            return Err(contract_error(
                "max_domain_memory_bytes a zero: un tetto nullo non e' un dominio, e «nessun tetto» si dichiara omettendo il campo"
                    .to_owned(),
            ));
        }
        // La struttura si valida attraverso la forma condivisa, che e' un
        // `PlanV5` **legittimo** — dichiara 5 e non porta il tetto. Il
        // documento v6 si ricompone dopo, con i nodi che la validazione ha
        // normalizzato: gli alias sono risolti li'.
        let struttura = PlanV5 {
            schema_version: super::PLAN_SCHEMA_VERSION_V5,
            limits: condivisi,
            crs,
            inputs,
            crs_decisions,
            nodes,
            output,
        };
        let (validata, nucleo) =
            PlanV5::valida_struttura_condivisa(struttura, plan_limits)?.in_parti();
        // `PLAN-011`: il confronto e' col budget governato **effettivo** —
        // quello dichiarato dal piano, oppure il default pubblicato quando il
        // piano lo omette. Si fa DOPO la validazione perche' e' li' che gli
        // effettivi sono risolti, e confrontarlo con l'override dichiarato
        // lascerebbe fuori proprio il caso che il contratto nomina.
        if let Some(tetto) = tetto_del_dominio {
            let governato = validata.limits.effective().max_governed_memory_bytes;
            if tetto < governato {
                return Err(contract_error(format!(
                    "max_domain_memory_bytes {tetto} sotto il budget governato effettivo {governato}: il dominio non potrebbe contenere l'esecuzione che il piano dichiara"
                )));
            }
        }
        let documento = Self {
            schema_version,
            limits: LimitsOverrideV6::da_condivisi(validata.limits, tetto_del_dominio),
            crs: validata.crs,
            inputs: validata.inputs,
            crs_decisions: validata.crs_decisions,
            nodes: validata.nodes,
            output: validata.output,
        };
        Ok(ValidatedPlanV6::nuovo(documento, nucleo))
    }

    /// Parse con i `PlanLimits` di default (fail-closed).
    ///
    /// # Errors
    /// Come [`PlanV6::parse`].
    pub fn parse_default(json_text: &str) -> Result<ValidatedPlanV6> {
        Self::parse(json_text, &PlanLimits::default())
    }
}

#[cfg(test)]
mod tests;
