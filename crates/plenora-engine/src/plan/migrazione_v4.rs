//! Migrazione esplicita dei piani `schema_version: 4` al canonico v5
//! (ADR 15).
//!
//! # Perche' un parser separato e non un alias
//!
//! `serde(alias)` avrebbe fatto accettare `max_memory_bytes` **anche** ai
//! piani v5, e un nome che continua a funzionare e' un nome che continua a
//! promettere: chi lo scrive crede ancora in un tetto sull'intero processo
//! che in-process non esiste (ADR 15 §3). Qui la v4 ha una struttura
//! **propria**, che conosce solo il nome vecchio, ed e' usata **solo** da
//! questa migrazione.
//!
//! Ne segue, per costruzione e non per controllo aggiuntivo:
//!
//! - un piano **v5 con il nome vecchio** e' rifiutato — `LimitsOverride` ha
//!   `deny_unknown_fields` e non conosce `max_memory_bytes`;
//! - un piano **v4 con il nome nuovo** e' rifiutato — `LimitsOverrideV4` ha
//!   `deny_unknown_fields` e non conosce `max_governed_memory_bytes`;
//! - un piano con **entrambe le chiavi** e' rifiutato in ogni versione, per
//!   la stessa ragione: qualunque sia la versione dichiarata, una delle due
//!   e' sconosciuta alla struttura che la deserializza.
//!
//! Le chiavi **duplicate** (lo stesso nome due volte) sono rifiutate prima
//! ancora, da `ensure_no_duplicate_keys`: `serde_json` le risolverebbe con
//! «vince l'ultima», e la risoluzione avverrebbe prima della migrazione,
//! producendo due testi diversi con lo stesso piano canonico.
//!
//! # Che cosa la migrazione promette, e che cosa no
//!
//! La migrazione tocca **due** cose sul piano semantico: `schema_version`,
//! che diventa 5, e il blocco `limits`, che attraversa le due strutture
//! tipizzate. Nodi, input, `crs_decisions` e `output` non vengono
//! interpretati.
//!
//! **Non e' però una riscrittura testuale.** Il piano viene deserializzato in
//! un `serde_json::Value` e riserializzato: `serde_json` conserva l'ordine
//! delle chiavi solo con la feature `preserve_order`, e la sintassi dei
//! numeri passa dal suo parser e dal suo formattatore. Spazi, a capo, ordine
//! delle chiavi e forma dei letterali numerici **possono cambiare**.
//!
//! Cio' che e' promesso — e verificato dai test — e' l'equivalenza
//! **semantica** dei valori non toccati e l'equivalenza **canonica** del
//! risultato: un piano v4 e il v5 equivalente producono lo stesso piano
//! canonico e lo stesso `plan_hash`. Che e' poi la sola equivalenza che
//! conti, perche' e' quella su cui poggiano cache e riproducibilita' (ADR 4).
//!
//! # Idempotenza
//!
//! L'ingresso reale e' [`testo_canonico_v5`], ed e' li' che l'idempotenza
//! deve valere: applicato al proprio risultato non cambia nulla e **non
//! cambia esito** — se la prima passata riesce, la seconda riesce. Per questo
//! il tetto `max_plan_json_bytes` viene applicato anche al testo **migrato**:
//! senza quel controllo un piano v4 lungo esattamente quanto il tetto
//! passerebbe la prima volta e fallirebbe la seconda, perche' il nome nuovo
//! e' piu' lungo di nove byte. Un ingresso che cambia risposta a input
//! costante e' peggio di un ingresso severo.
//!
//! La sola funzione di migrazione, invece, **rifiuta** un piano gia' migrato
//! invece di operarci in silenzio: chiamarla due volte e' un errore di chi
//! chiama, e un errore di chi chiama va detto.
//!
//! # Superficie pubblica
//!
//! Pubblico e' solo [`testo_canonico_v5`], che riceve i [`PlanLimits`] e li
//! applica. Le funzioni interne costruiscono un `serde_json::Value` dal
//! testo, cioe' allocano guidate dal contenuto: esporle avrebbe offerto un
//! ingresso che aggira il tetto di ADR 6, ed e' esattamente il genere di
//! porta di servizio che un tetto applicato «il prima possibile» esiste per
//! non avere.

use std::borrow::Cow;

use serde::Deserialize;
use serde_json::{Map, Value};

use plenora_core::limits::PlanLimits;
use plenora_core::{PlenoraError, Result};

use super::{LimitsOverride, PlanLimitsOverride, PLAN_SCHEMA_VERSION_V4, PLAN_SCHEMA_VERSION_V5};

#[cfg(test)]
mod tests;

/// Override dei limiti **come li scriveva la v4**: con `max_memory_bytes`.
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
    /// È una costruzione per campi, non una riscrittura di chiavi, e la
    /// differenza conta: se qualcuno aggiunge un limite a
    /// [`LimitsOverride`], questo letterale smette di compilare. Una
    /// migrazione che si dimentica di un campo nuovo lo lascerebbe cadere in
    /// silenzio, ed e' esattamente il modo in cui un piano migrato finisce
    /// per girare sotto limiti che non ha chiesto.
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
            // L'unico campo che cambia nome. Il valore non si tocca: era
            // gia' un budget di ammissione, solo chiamato male (ADR 15).
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
/// Serve a scegliere il percorso **prima** di impegnarsi su una struttura:
/// deserializzare con quella sbagliata darebbe un errore di campo
/// sconosciuto invece di un errore di versione, e manderebbe chi legge a
/// cercare il problema nel posto sbagliato.
///
/// # Errors
///
/// `PlenoraError::DataMapping` se il testo non e' JSON valido;
/// `PlenoraError::InvalidPlan` se il JSON non e' un oggetto, se
/// `schema_version` manca, non e' un intero non negativo o non sta in `u16`.
fn versione_dichiarata(json_text: &str) -> Result<u16> {
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
/// La riscrittura tocca **due** cose e nient'altro: `schema_version` diventa
/// 5 e il blocco `limits` viene ricostruito da `LimitsOverrideV4` tramite
/// `LimitsOverrideV4::in_v5`, dove `max_memory_bytes` diventa
/// `max_governed_memory_bytes`. Il valore non cambia — cambia il nome,
/// perche' e' il nome a essere stato sbagliato (ADR 15).
///
/// Il passaggio dalla struttura v4 non e' un dettaglio di comodo: rifiuta
/// fail-closed un v4 con chiavi sconosciute o col nome nuovo, che una
/// riscrittura cieca dell'albero lascerebbe passare fino alla validazione v5,
/// dove l'errore parlerebbe della versione sbagliata e manderebbe chi legge a
/// cercare il problema nel posto sbagliato.
///
/// # Errors
///
/// `PlenoraError::DataMapping` se il testo non e' JSON valido, o se il blocco
/// `limits` non e' valido per la v4 — chiavi sconosciute e il nome della v5
/// incluse: e' la stessa categoria che `PlanV5::parse` produce per lo stesso
/// difetto scritto in v5, e da essa dipende l'exit code.
/// `PlenoraError::InvalidPlan` se contiene chiavi duplicate, se non dichiara
/// `schema_version: 4` o se non e' un oggetto.
/// `PlenoraError::Internal` solo per un fallimento di serializzazione, che su
/// una struttura di `Option<numero>` non ha modo di accadere: e' il caso
/// "impossibile" reso esplicito invece che assunto (R6).
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
/// È il **solo** ingresso di versione del crate: `planner::validate` lo
/// attraversa, e chi vuole un piano canonico non deve indovinare quale parser
/// usare. Un piano gia' v5 attraversa **senza copia** (`Cow::Borrowed`): la
/// migrazione non deve costare a chi non la usa.
///
/// Il tetto `max_plan_json_bytes` si applica **due volte**: al testo fornito,
/// prima di costruire qualunque albero JSON (ADR 6: migrare significa
/// allocare, e allocare guidati dal contenuto prima di averlo limitato e'
/// cio' che ADR 6 vieta), e al testo **migrato**, prima di restituirlo.
///
/// Il secondo controllo non e' ridondante: il nome della v5 e' piu' lungo di
/// nove byte, quindi un v4 lungo esattamente quanto il tetto migra in un
/// testo che lo supera. Senza quel controllo la funzione risponderebbe `Ok`
/// alla prima chiamata e `Err` alla seconda sullo stesso input — l'idempotenza
/// varrebbe sul valore ma non sull'esito, che e' il modo peggiore in cui puo'
/// non valere. Ne segue anche che un v4 e il v5 equivalente vengono accettati
/// o rifiutati **insieme**: il v5 equivalente ha la stessa chiave lunga e
/// quindi la stessa dimensione del migrato.
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
