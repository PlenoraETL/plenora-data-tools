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
//! La v6 non collassa nel canonico v5. Il [`super::PlanV5`] che questo
//! modulo produce porta `schema_version: 6`, la forma canonica lo
//! materializza, e il separatore di dominio del `plan_hash` e' scelto da
//! quella versione. Un v5 e un v6 per il resto identici hanno percio'
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
/// Esiste solo per essere deserializzata da questo modulo. `deny_unknown_fields`
/// e' cio' che rende impossibile, per costruzione e non per controllo:
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
mod tests {
    use super::{PlanV6, PLAN_SCHEMA_VERSION_V6};
    use crate::plan::{PlanV5, PLAN_SCHEMA_VERSION_V5};
    use serde_json::json;

    /// Lo stesso piano nelle due versioni, con i limiti che si vogliono.
    fn piano(versione: u16, limiti: &serde_json::Value) -> String {
        json!({
            "schema_version": versione,
            "limits": limiti,
            "inputs": ["main"],
            "nodes": [{"id": "a", "op": "table.filter", "in": ["main"], "config": {}}],
            "output": "a",
        })
        .to_string()
    }

    #[test]
    fn un_piano_v6_e_accettato() {
        // La prima stesura non poteva accettarne nessuno: la validazione
        // strutturale pretendeva `schema_version == 5`, e la suite restava
        // verde perche' nessun test provava a costruirne uno. Verde senza
        // copertura non e' una verifica.
        let validato = PlanV6::parse_default(&piano(
            PLAN_SCHEMA_VERSION_V6,
            &json!({"max_domain_memory_bytes": 1_073_741_824}),
        ))
        .expect("un piano v6 valido deve essere accettato");
        assert_eq!(validato.max_domain_memory_bytes(), Some(1_073_741_824));
    }

    #[test]
    fn il_tetto_e_facoltativo_e_la_sua_assenza_non_e_un_default() {
        let senza = PlanV6::parse_default(&piano(PLAN_SCHEMA_VERSION_V6, &json!({})))
            .expect("il campo e' facoltativo (PLAN-009)");
        assert_eq!(
            senza.max_domain_memory_bytes(),
            None,
            "assente significa «profilo isolato non selezionabile», mai un tetto implicito"
        );
    }

    #[test]
    fn il_campo_non_esiste_nella_v5() {
        // `PLAN-007`, e per costruzione: `LimitsOverride` ha
        // `deny_unknown_fields` e non conosce quel nome.
        let errore = PlanV5::parse_default(&piano(
            PLAN_SCHEMA_VERSION_V5,
            &json!({"max_domain_memory_bytes": 1_073_741_824}),
        ))
        .expect_err("un v5 che dichiara il campo va rifiutato");
        assert!(
            errore.to_string().contains("max_domain_memory_bytes"),
            "l'errore deve nominare il campo estraneo: {errore}"
        );
    }

    #[test]
    fn lo_zero_non_e_nessun_tetto() {
        // `PLAN-008`. Zero descrive un dominio che non puo' allocare nulla;
        // «nessun tetto» si dichiara omettendo il campo, e i due non devono
        // collassare.
        let errore = PlanV6::parse_default(&piano(
            PLAN_SCHEMA_VERSION_V6,
            &json!({"max_domain_memory_bytes": 0}),
        ))
        .expect_err("zero va rifiutato");
        assert!(errore.to_string().contains("zero"), "{errore}");
    }

    #[test]
    fn il_tetto_sotto_il_budget_governato_effettivo_e_rifiutato() {
        // `PLAN-011` col budget DICHIARATO.
        let errore = PlanV6::parse_default(&piano(
            PLAN_SCHEMA_VERSION_V6,
            &json!({"max_governed_memory_bytes": 268_435_456,
                    "max_domain_memory_bytes": 134_217_728}),
        ))
        .expect_err("un dominio sotto il budget dichiarato va rifiutato");
        assert!(
            errore.to_string().contains("budget governato effettivo"),
            "{errore}"
        );
    }

    #[test]
    fn il_confronto_usa_il_default_quando_il_piano_omette_il_budget() {
        // `PLAN-012`: e' il caso che il contratto nomina esplicitamente, ed
        // e' quello che un confronto con l'override DICHIARATO lascerebbe
        // fuori — li' il budget e' `None` e non ci sarebbe niente da
        // confrontare.
        let default = plenora_core::DEFAULT_MAX_GOVERNED_MEMORY_BYTES;
        let sotto = PlanV6::parse_default(&piano(
            PLAN_SCHEMA_VERSION_V6,
            &json!({"max_domain_memory_bytes": default - 1}),
        ));
        assert!(
            sotto.is_err(),
            "sotto il default, col budget omesso, va rifiutato"
        );
        let pari = PlanV6::parse_default(&piano(
            PLAN_SCHEMA_VERSION_V6,
            &json!({"max_domain_memory_bytes": default}),
        ));
        assert!(pari.is_ok(), "pari al default e' accettato: {pari:?}");
    }

    #[test]
    fn il_canonico_v6_dichiara_la_propria_versione_e_il_tetto() {
        // `PLAN-017`: il campo entra nel canonico SOLO quando e' presente.
        let con = PlanV6::parse_default(&piano(
            PLAN_SCHEMA_VERSION_V6,
            &json!({"max_domain_memory_bytes": 1_073_741_824}),
        ))
        .expect("valido");
        let canonico = con.canonical_json();
        assert_eq!(canonico["schema_version"], json!(PLAN_SCHEMA_VERSION_V6));
        assert_eq!(
            canonico["limits"]["max_domain_memory_bytes"],
            json!(1_073_741_824)
        );

        let senza =
            PlanV6::parse_default(&piano(PLAN_SCHEMA_VERSION_V6, &json!({}))).expect("valido");
        assert!(
            senza.canonical_json()["limits"]
                .get("max_domain_memory_bytes")
                .is_none(),
            "assente non deve comparire"
        );
        assert_ne!(
            canonico,
            senza.canonical_json(),
            "un v6 che lo dichiara e uno che lo omette sono piani diversi"
        );
    }

    #[test]
    fn il_documento_v6_fa_il_giro_completo() {
        // Il tipo e' pubblico e parallelo alla v5: si legge e si scrive. Se
        // il giro perdesse un campo, chi costruisce un v6 a mano e lo emette
        // otterrebbe un piano diverso da quello che ha scritto — e, passando
        // dal parser, un `plan_hash` diverso.
        let testo = piano(
            PLAN_SCHEMA_VERSION_V6,
            &json!({"max_domain_memory_bytes": 1_073_741_824,
                    "max_governed_memory_bytes": 536_870_912}),
        );
        let letto: PlanV6 = serde_json::from_str(&testo).expect("v6 leggibile");
        let riscritto = serde_json::to_string(&letto).expect("v6 scrivibile");
        let riletto: PlanV6 = serde_json::from_str(&riscritto).expect("v6 rileggibile");
        assert_eq!(
            serde_json::to_value(&letto).expect("valore"),
            serde_json::to_value(&riletto).expect("valore"),
            "il giro non deve perdere nulla"
        );
        assert_eq!(
            riletto.limits.max_domain_memory_bytes,
            Some(1_073_741_824),
            "il campo della v6 sopravvive al giro"
        );
        // Un limite assente non compare, come nella v5: altrimenti il
        // documento riscritto dichiarerebbe `null` dove l'originale taceva.
        let scarno: PlanV6 =
            serde_json::from_str(&piano(PLAN_SCHEMA_VERSION_V6, &json!({}))).expect("v6");
        let emesso = serde_json::to_value(&scarno).expect("valore");
        assert!(
            emesso["limits"].get("max_domain_memory_bytes").is_none(),
            "un limite assente non deve comparire: {emesso}"
        );
    }

    #[test]
    fn lo_stesso_piano_in_v5_e_in_v6_ha_identita_diverse() {
        // `PLAN-018`. Le due strutture sono identiche — stessi nodi, stessi
        // archi, stessi limiti — e i due documenti no: il canonico dichiara
        // la propria versione, quindi i byte che si hashano differiscono, e
        // il separatore di dominio li tiene distinti anche se non
        // differissero.
        //
        // Da NON generalizzare: la v4 condivide l'identita' con la v5
        // (`PLAN-019`), e le regressioni della migrazione lo verificano.
        let come_v5 = PlanV5::parse_default(&piano(PLAN_SCHEMA_VERSION_V5, &json!({})))
            .expect("un v5 valido");
        let come_v6 = PlanV6::parse_default(&piano(PLAN_SCHEMA_VERSION_V6, &json!({})))
            .expect("un v6 valido");
        let canonico_v5 = come_v5.canonical_json();
        let canonico_v6 = come_v6.canonical_json();
        assert_eq!(canonico_v5["schema_version"], json!(PLAN_SCHEMA_VERSION_V5));
        assert_eq!(canonico_v6["schema_version"], json!(PLAN_SCHEMA_VERSION_V6));
        assert_ne!(
            canonico_v5, canonico_v6,
            "un v5 e un v6 per il resto identici sono piani diversi"
        );
        // E la sola differenza e' la versione: la struttura non e' cambiata.
        let mut senza_versione_v6 = canonico_v6;
        senza_versione_v6["schema_version"] = json!(PLAN_SCHEMA_VERSION_V5);
        assert_eq!(
            canonico_v5, senza_versione_v6,
            "la struttura condivisa deve restare identica"
        );
    }
}
