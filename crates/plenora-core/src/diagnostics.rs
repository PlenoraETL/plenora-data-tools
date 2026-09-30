//! Diagnostica per riga: il payload `plenora-row-diagnostics-v1` che un
//! kernel allega a un errore quando rifiuta righe
//! ([`crate::PlenoraError::with_row_diagnostics`]).
//!
//! Il payload dice quante righe sono state rifiutate e perché (un codice
//! per causa), con al più `examples_limit` esempi: indice di riga nella
//! sorgente, codice, nome della colonna. Mai il valore della cella.
//! [`RowDiagnostics::validate_for_emission`] ne verifica la coerenza prima di
//! ogni serializzazione e dopo ogni lettura.
//!
//! I campi di scrittura (`scope` `write`, stati e esiti di scrittura) e la
//! chiave di riga degli esempi vengono dal contratto di `plenora-data-tools`,
//! dove li riempiva la pubblicazione verso un database; qui nessun codice li
//! produce.

use std::collections::{BTreeMap, HashSet};

use serde::{Deserialize, Serialize};

/// Nome e versione del contratto, valore obbligato di
/// [`RowDiagnostics::contract`].
pub const ROW_DIAGNOSTICS_CONTRACT: &str = "plenora-row-diagnostics-v1";
/// Base degli indici di riga di default di [`RowDiagnostics::index_basis`]:
/// indice nella sorgente, da zero. È quella che ogni kernel scrive.
pub const ROW_DIAGNOSTICS_INDEX_BASIS: &str = "source_row_zero_based";
/// Base degli indici di riga quando la sorgente non è raggiungibile: indice,
/// da zero, nel primo ingresso del passo che ha rifiutato le righe.
///
/// La scrive solo il runner (`plenora-pipeline`), riscrivendo il payload di
/// un kernel il cui ingresso discende da un passo che cambia numero o ordine
/// delle righe; il passo e il suo ingresso sono nominati nel testo
/// dell'errore. Un lettore che conosce solo [`ROW_DIAGNOSTICS_INDEX_BASIS`]
/// rifiuta il payload invece di leggere gli indici come righe della sorgente.
pub const ROW_DIAGNOSTICS_INDEX_BASIS_STEP_INPUT: &str = "step_input_row_zero_based";

/// Dove sono state rifiutate le righe.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RowDiagnosticScope {
    /// Per il valore d'ingresso della riga: una conversione, un calcolo o
    /// un'asserzione di un kernel lo rifiuta. È lo scope di tutti i rifiuti
    /// dei kernel, anche a metà di un piano, dove nessun file si legge: dice
    /// che cosa è stato rifiutato (una riga d'ingresso), non la fase
    /// dell'errore, che per i kernel è `write` ([`crate::ErrorPhase`]).
    Read,
    /// Scrivendo verso una destinazione; richiede `input_total`,
    /// `diagnostic_state_counts` e `write_outcome`. Qui nessun codice lo
    /// produce.
    Write,
}

/// Quanto il payload conosce delle righe rifiutate.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RowDiagnosticsCompleteness {
    /// Tutte le righe rifiutate sono contate: `total` è `observed_total`,
    /// niente `knowledge_limits`.
    Complete,
    /// Una parte: `knowledge_limits` dice perché, `total` se noto.
    Partial,
    /// Il totale non è noto: `total` assente, `knowledge_limits` presente.
    Unknown,
}

/// Stato della chiave di riga di un esempio.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RowDiagnosticKeyState {
    /// Il valore della chiave è riportato in `value`.
    Value,
    /// Il valore esiste ma è oscurato.
    Redacted,
    /// Il valore non è disponibile.
    Unavailable,
}

/// Valore di una chiave di riga: testo (al più 1024 caratteri), intero
/// entro ±(2^53 - 1) o booleano.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RowDiagnosticKeyValue {
    /// Testo.
    String(String),
    /// Intero rappresentabile esattamente in un numero JSON.
    Integer(i64),
    /// Booleano.
    Boolean(bool),
}

/// Chiave di riga di un esempio: il campo che la identifica e il suo stato.
/// Qui nessun kernel la riempie.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RowDiagnosticKey {
    /// Nome del campo chiave (1-256 caratteri).
    pub field: String,
    /// Se il valore è riportato, oscurato o non disponibile.
    pub state: RowDiagnosticKeyState,
    /// Il valore, presente se e solo se `state` è `Value`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<RowDiagnosticKeyValue>,
}

/// Esito di scrittura di una riga rifiutata (solo `scope` `write`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RowDiagnosticWriteState {
    /// La destinazione l'ha rifiutata.
    CertainlyRejected,
    /// Non si è tentato di scriverla.
    CertainlyNotAttempted,
    /// Scritta e poi annullata.
    CertainlyRolledBack,
    /// Non si sa se sia stata scritta.
    EffectUnknown,
}

/// Una riga rifiutata, d'esempio.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RowDiagnosticExample {
    /// Indice della riga, da zero, nella base di
    /// [`RowDiagnostics::index_basis`] (la sorgente, o il primo ingresso del
    /// passo); unico fra gli esempi.
    pub source_index: u64,
    /// Codice della causa (minuscole, cifre, `.`, `_`, `-`; al più 128
    /// byte), una delle chiavi di [`RowDiagnostics::counts`].
    pub cause: String,
    /// Nome della colonna (1-256 caratteri), se la causa ne ha una.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub column: Option<String>,
    /// Chiave di riga (vedi [`RowDiagnosticKey`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<RowDiagnosticKey>,
    /// Esito di scrittura: obbligatorio con `scope` `write`, vietato con
    /// `read`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub write_state: Option<RowDiagnosticWriteState>,
}

/// Un conteggio noto o dichiaratamente ignoto.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum KnownOrUnknownCount {
    /// Conteggio noto, in `value`.
    Known { value: u64 },
    /// Conteggio non noto.
    Unknown,
}

/// Righe rifiutate per esito di scrittura (solo `scope` `write`): la somma è
/// `observed_total`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WriteDiagnosticStateCounts {
    /// Rifiutate dalla destinazione.
    pub certainly_rejected: u64,
    /// Non tentate.
    pub certainly_not_attempted: u64,
    /// Annullate.
    pub certainly_rolled_back: u64,
    /// Con effetto ignoto.
    pub effect_unknown: u64,
}

/// Esito di tutte le righe d'ingresso di una scrittura (solo `scope`
/// `write`), per stato; i conteggi noti non superano `input_total`, e se
/// sono tutti noti lo sommano.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RowDiagnosticWriteOutcome {
    /// Righe rifiutate.
    pub certainly_rejected: KnownOrUnknownCount,
    /// Righe non tentate.
    pub certainly_not_attempted: KnownOrUnknownCount,
    /// Righe annullate.
    pub certainly_rolled_back: KnownOrUnknownCount,
    /// Righe con effetto ignoto.
    pub effect_unknown: KnownOrUnknownCount,
}

/// Il payload `plenora-row-diagnostics-v1`.
///
/// Si serializza e si deserializza solo se
/// [`RowDiagnostics::validate_for_emission`] lo accetta.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RowDiagnostics {
    /// Sempre [`ROW_DIAGNOSTICS_CONTRACT`].
    pub contract: String,
    /// Lettura o scrittura.
    pub scope: RowDiagnosticScope,
    /// [`ROW_DIAGNOSTICS_INDEX_BASIS`] (righe della sorgente) o
    /// [`ROW_DIAGNOSTICS_INDEX_BASIS_STEP_INPUT`] (righe del primo ingresso
    /// del passo): dice a che cosa si riferisce
    /// [`RowDiagnosticExample::source_index`].
    pub index_basis: String,
    /// Quanto il payload conosce delle righe rifiutate.
    pub completeness: RowDiagnosticsCompleteness,
    /// Codici distinti che dicono perché il conteggio non è completo;
    /// assente con `Complete`, non vuoto con `Partial` e `Unknown`.
    pub knowledge_limits: Option<Vec<String>>,
    /// Righe rifiutate osservate: la somma di `counts`.
    pub observed_total: u64,
    /// Righe rifiutate in tutto, se noto: positivo e non minore di
    /// `observed_total`.
    pub total: Option<u64>,
    /// Righe d'ingresso della scrittura (solo `scope` `write`).
    pub input_total: Option<u64>,
    /// Righe rifiutate per codice di causa, ognuna almeno 1.
    pub counts: BTreeMap<String, u64>,
    /// Numero massimo di esempi, almeno 1.
    pub examples_limit: u64,
    /// Vero se e solo se gli esempi sono al limite e le righe osservate
    /// sono di più.
    pub examples_truncated: bool,
    /// Esempi, al più `examples_limit` e al più `observed_total`; con
    /// `Complete` esattamente `min(observed_total, examples_limit)`.
    pub examples: Vec<RowDiagnosticExample>,
    /// Righe rifiutate per esito di scrittura (solo `scope` `write`).
    pub diagnostic_state_counts: Option<WriteDiagnosticStateCounts>,
    /// Esito di tutte le righe della scrittura (solo `scope` `write`).
    pub write_outcome: Option<RowDiagnosticWriteOutcome>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RowDiagnosticsOwnedWire {
    contract: String,
    scope: RowDiagnosticScope,
    index_basis: String,
    completeness: RowDiagnosticsCompleteness,
    #[serde(default)]
    knowledge_limits: Option<Vec<String>>,
    observed_total: u64,
    #[serde(default)]
    total: Option<u64>,
    #[serde(default)]
    input_total: Option<u64>,
    counts: BTreeMap<String, u64>,
    examples_limit: u64,
    examples_truncated: bool,
    examples: Vec<RowDiagnosticExample>,
    #[serde(default)]
    diagnostic_state_counts: Option<WriteDiagnosticStateCounts>,
    #[serde(default)]
    write_outcome: Option<RowDiagnosticWriteOutcome>,
}

impl From<RowDiagnosticsOwnedWire> for RowDiagnostics {
    fn from(wire: RowDiagnosticsOwnedWire) -> Self {
        Self {
            contract: wire.contract,
            scope: wire.scope,
            index_basis: wire.index_basis,
            completeness: wire.completeness,
            knowledge_limits: wire.knowledge_limits,
            observed_total: wire.observed_total,
            total: wire.total,
            input_total: wire.input_total,
            counts: wire.counts,
            examples_limit: wire.examples_limit,
            examples_truncated: wire.examples_truncated,
            examples: wire.examples,
            diagnostic_state_counts: wire.diagnostic_state_counts,
            write_outcome: wire.write_outcome,
        }
    }
}

impl<'de> Deserialize<'de> for RowDiagnostics {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let report = Self::from(RowDiagnosticsOwnedWire::deserialize(deserializer)?);
        report
            .validate_for_emission()
            .map_err(serde::de::Error::custom)?;
        Ok(report)
    }
}

impl RowDiagnostics {
    /// Valida schema e invarianti aritmetiche del contratto prima di
    /// emettere o dopo aver letto il payload.
    ///
    /// # Errors
    ///
    /// Restituisce un motivo breve, senza dati di riga, se il payload non è
    /// valido secondo `plenora-row-diagnostics-v1`.
    // La sequenza resta intenzionalmente monolitica per essere confrontabile,
    // nell'ordine, con il validatore del contratto del progetto d'origine.
    #[allow(clippy::too_many_lines)]
    pub fn validate_for_emission(&self) -> Result<(), &'static str> {
        if self.contract != ROW_DIAGNOSTICS_CONTRACT
            || !matches!(
                self.index_basis.as_str(),
                ROW_DIAGNOSTICS_INDEX_BASIS | ROW_DIAGNOSTICS_INDEX_BASIS_STEP_INPUT
            )
            || self.examples_limit == 0
        {
            return Err("campi radice non validi");
        }
        let mut limits = HashSet::new();
        if self
            .knowledge_limits
            .iter()
            .flatten()
            .any(|value| !valid_code(value) || !limits.insert(value.as_str()))
        {
            return Err("limiti di conoscenza non validi");
        }
        let example_count = u64::try_from(self.examples.len()).map_err(|_| "troppi esempi")?;
        if example_count > self.examples_limit || example_count > self.observed_total {
            return Err("limite esempi superato");
        }
        let counted = self.counts.values().try_fold(0_u64, |sum, count| {
            if *count == 0 {
                return Err("conteggio causa nullo");
            }
            sum.checked_add(*count).ok_or("overflow conteggi")
        })?;
        if counted != self.observed_total
            || self.counts.keys().any(|cause| !valid_code(cause))
            || self.examples_truncated
                != (self.observed_total > example_count && example_count == self.examples_limit)
            || self
                .total
                .is_some_and(|total| total == 0 || total < self.observed_total)
        {
            return Err("conteggi incoerenti");
        }
        match self.completeness {
            RowDiagnosticsCompleteness::Complete => {
                if self.total != Some(self.observed_total)
                    || self.knowledge_limits.is_some()
                    || example_count != self.observed_total.min(self.examples_limit)
                {
                    return Err("complete incoerente");
                }
            }
            RowDiagnosticsCompleteness::Partial => {
                if self.knowledge_limits.as_ref().is_none_or(Vec::is_empty) {
                    return Err("partial incoerente");
                }
            }
            RowDiagnosticsCompleteness::Unknown => {
                if self.total.is_some() || self.knowledge_limits.as_ref().is_none_or(Vec::is_empty)
                {
                    return Err("unknown incoerente");
                }
            }
        }
        let mut source_indices = HashSet::new();
        let mut example_cause_counts = BTreeMap::new();
        let mut example_state_counts = [0_u64; 4];
        for example in &self.examples {
            if !source_indices.insert(example.source_index)
                || !valid_code(&example.cause)
                || !self.counts.contains_key(&example.cause)
                || example
                    .column
                    .as_ref()
                    .is_some_and(|column| column.is_empty() || column.chars().count() > 256)
            {
                return Err("esempio non valido");
            }
            let cause_count = example_cause_counts
                .entry(example.cause.as_str())
                .or_insert(0_u64);
            *cause_count = cause_count.checked_add(1).ok_or("overflow esempi causa")?;
            if let Some(key) = &example.key {
                if key.field.is_empty() || key.field.chars().count() > 256 {
                    return Err("chiave non valida");
                }
                match (&key.state, &key.value) {
                    (RowDiagnosticKeyState::Value, Some(RowDiagnosticKeyValue::String(value)))
                        if value.chars().count() <= 1024 => {}
                    (RowDiagnosticKeyState::Value, Some(RowDiagnosticKeyValue::Integer(value)))
                        if (-9_007_199_254_740_991..=9_007_199_254_740_991).contains(value) => {}
                    (RowDiagnosticKeyState::Value, Some(RowDiagnosticKeyValue::Boolean(_)))
                    | (
                        RowDiagnosticKeyState::Redacted | RowDiagnosticKeyState::Unavailable,
                        None,
                    ) => {}
                    _ => return Err("valore chiave non valido"),
                }
            }
            if let Some(state) = example.write_state {
                let state_count = &mut example_state_counts[write_state_index(state)];
                *state_count = state_count.checked_add(1).ok_or("overflow esempi write")?;
            }
        }
        if example_cause_counts.iter().any(|(cause, count)| {
            self.counts
                .get(*cause)
                .is_none_or(|declared| count > declared)
        }) {
            return Err("esempi eccedono il conteggio causa");
        }
        match self.scope {
            RowDiagnosticScope::Read => {
                if self.input_total.is_some()
                    || self
                        .examples
                        .iter()
                        .any(|example| example.write_state.is_some())
                    || self.diagnostic_state_counts.is_some()
                    || self.write_outcome.is_some()
                {
                    return Err("campi write in report read");
                }
            }
            RowDiagnosticScope::Write => {
                let input_total = self
                    .input_total
                    .filter(|total| *total > 0)
                    .ok_or("input_total write mancante")?;
                let states = self
                    .diagnostic_state_counts
                    .as_ref()
                    .ok_or("diagnostic_state_counts mancante")?;
                let outcome = self
                    .write_outcome
                    .as_ref()
                    .ok_or("write_outcome mancante")?;
                if self.observed_total > input_total
                    || self
                        .examples
                        .iter()
                        .any(|example| example.write_state.is_none())
                {
                    return Err("report write incoerente");
                }
                let state_counts = [
                    states.certainly_rejected,
                    states.certainly_not_attempted,
                    states.certainly_rolled_back,
                    states.effect_unknown,
                ];
                let state_sum = state_counts.iter().try_fold(0_u64, |sum, count| {
                    sum.checked_add(*count).ok_or("overflow stati diagnostici")
                })?;
                if state_sum != self.observed_total
                    || example_state_counts
                        .iter()
                        .zip(state_counts)
                        .any(|(examples, diagnostics)| *examples > diagnostics)
                {
                    return Err("stati diagnostici incoerenti");
                }
                let buckets = [
                    outcome.certainly_rejected,
                    outcome.certainly_not_attempted,
                    outcome.certainly_rolled_back,
                    outcome.effect_unknown,
                ];
                let mut known_sum = 0_u64;
                let mut diagnosed_unknown = 0_u64;
                let mut all_known = true;
                for (index, bucket) in buckets.into_iter().enumerate() {
                    match bucket {
                        KnownOrUnknownCount::Known { value } => {
                            if state_counts[index] > value {
                                return Err("diagnostica eccede outcome noto");
                            }
                            known_sum = known_sum.checked_add(value).ok_or("overflow outcome")?;
                        }
                        KnownOrUnknownCount::Unknown => {
                            all_known = false;
                            diagnosed_unknown = diagnosed_unknown
                                .checked_add(state_counts[index])
                                .ok_or("overflow outcome ignoto")?;
                        }
                    }
                }
                if known_sum > input_total
                    || known_sum
                        .checked_add(diagnosed_unknown)
                        .is_none_or(|sum| sum > input_total)
                    || (all_known && known_sum != input_total)
                {
                    return Err("partizione write incoerente");
                }
            }
        }
        Ok(())
    }
}

const fn write_state_index(state: RowDiagnosticWriteState) -> usize {
    match state {
        RowDiagnosticWriteState::CertainlyRejected => 0,
        RowDiagnosticWriteState::CertainlyNotAttempted => 1,
        RowDiagnosticWriteState::CertainlyRolledBack => 2,
        RowDiagnosticWriteState::EffectUnknown => 3,
    }
}

#[derive(Serialize)]
struct RowDiagnosticsWire<'a> {
    contract: &'a str,
    scope: RowDiagnosticScope,
    index_basis: &'a str,
    completeness: RowDiagnosticsCompleteness,
    #[serde(skip_serializing_if = "Option::is_none")]
    knowledge_limits: Option<&'a Vec<String>>,
    observed_total: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    total: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    input_total: Option<u64>,
    counts: &'a BTreeMap<String, u64>,
    examples_limit: u64,
    examples_truncated: bool,
    examples: &'a [RowDiagnosticExample],
    #[serde(skip_serializing_if = "Option::is_none")]
    diagnostic_state_counts: Option<&'a WriteDiagnosticStateCounts>,
    #[serde(skip_serializing_if = "Option::is_none")]
    write_outcome: Option<&'a RowDiagnosticWriteOutcome>,
}

impl Serialize for RowDiagnostics {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.validate_for_emission()
            .map_err(serde::ser::Error::custom)?;
        RowDiagnosticsWire {
            contract: &self.contract,
            scope: self.scope,
            index_basis: &self.index_basis,
            completeness: self.completeness,
            knowledge_limits: self.knowledge_limits.as_ref(),
            observed_total: self.observed_total,
            total: self.total,
            input_total: self.input_total,
            counts: &self.counts,
            examples_limit: self.examples_limit,
            examples_truncated: self.examples_truncated,
            examples: &self.examples,
            diagnostic_state_counts: self.diagnostic_state_counts.as_ref(),
            write_outcome: self.write_outcome.as_ref(),
        }
        .serialize(serializer)
    }
}

fn valid_code(value: &str) -> bool {
    if value.is_empty() || value.len() > 128 {
        return false;
    }
    let mut previous_separator = false;
    for (index, byte) in value.bytes().enumerate() {
        let separator = matches!(byte, b'.' | b'_' | b'-');
        if (index == 0 && !byte.is_ascii_lowercase())
            || (!byte.is_ascii_lowercase() && !byte.is_ascii_digit() && !separator)
            || (separator && previous_separator)
        {
            return false;
        }
        previous_separator = separator;
    }
    !previous_separator
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Diagnostica valida di scope `Read`; la usano anche i test di `error`.
    pub fn report(observed_total: u64, examples: Vec<RowDiagnosticExample>) -> RowDiagnostics {
        let mut counts = BTreeMap::new();
        if observed_total > 0 {
            counts.insert("conversion.invalid_date".to_owned(), observed_total);
        }
        RowDiagnostics {
            contract: ROW_DIAGNOSTICS_CONTRACT.to_owned(),
            scope: RowDiagnosticScope::Read,
            index_basis: ROW_DIAGNOSTICS_INDEX_BASIS.to_owned(),
            completeness: RowDiagnosticsCompleteness::Complete,
            knowledge_limits: None,
            observed_total,
            total: Some(observed_total),
            input_total: None,
            counts,
            examples_limit: 2,
            examples_truncated: false,
            examples,
            diagnostic_state_counts: None,
            write_outcome: None,
        }
    }

    pub fn example(source_index: u64) -> RowDiagnosticExample {
        RowDiagnosticExample {
            source_index,
            cause: "conversion.invalid_date".to_owned(),
            column: Some("value".to_owned()),
            key: None,
            write_state: None,
        }
    }

    fn confirmed_write_report() -> RowDiagnostics {
        RowDiagnostics {
            contract: ROW_DIAGNOSTICS_CONTRACT.to_owned(),
            scope: RowDiagnosticScope::Write,
            index_basis: ROW_DIAGNOSTICS_INDEX_BASIS.to_owned(),
            completeness: RowDiagnosticsCompleteness::Complete,
            knowledge_limits: None,
            observed_total: 1,
            total: Some(1),
            input_total: Some(5_200),
            counts: BTreeMap::from([("database.constraint_violation".to_owned(), 1)]),
            examples_limit: 10,
            examples_truncated: false,
            examples: vec![RowDiagnosticExample {
                source_index: 4_999,
                cause: "database.constraint_violation".to_owned(),
                column: Some("area_m2".to_owned()),
                key: Some(RowDiagnosticKey {
                    field: "parcel_id".to_owned(),
                    state: RowDiagnosticKeyState::Redacted,
                    value: None,
                }),
                write_state: Some(RowDiagnosticWriteState::CertainlyRejected),
            }],
            diagnostic_state_counts: Some(WriteDiagnosticStateCounts {
                certainly_rejected: 1,
                certainly_not_attempted: 0,
                certainly_rolled_back: 0,
                effect_unknown: 0,
            }),
            write_outcome: Some(RowDiagnosticWriteOutcome {
                certainly_rejected: KnownOrUnknownCount::Known { value: 1 },
                certainly_not_attempted: KnownOrUnknownCount::Known { value: 200 },
                certainly_rolled_back: KnownOrUnknownCount::Known { value: 4_999 },
                effect_unknown: KnownOrUnknownCount::Known { value: 0 },
            }),
        }
    }

    #[test]
    fn rc17_rejects_examples_exceeding_observed_or_cause_count() {
        // Piu' esempi che righe osservate: si ferma al limite complessivo.
        let oltre_osservate = report(1, vec![example(0), example(1)]);
        assert_eq!(
            oltre_osservate.validate_for_emission(),
            Err("limite esempi superato")
        );

        // Totale coerente, ma una causa ha piu' esempi del proprio conteggio:
        // due cause da uno, entrambi gli esempi sulla prima.
        let mut oltre_causa = report(2, vec![example(0), example(1)]);
        oltre_causa.counts = BTreeMap::from([
            ("conversion.invalid_date".to_owned(), 1),
            ("conversion.invalid_number".to_owned(), 1),
        ]);
        assert_eq!(
            oltre_causa.validate_for_emission(),
            Err("esempi eccedono il conteggio causa")
        );

        // Controllo: gli stessi conteggi con un esempio per causa passano.
        let mut per_causa = oltre_causa;
        per_causa.examples[1].cause = "conversion.invalid_number".to_owned();
        assert_eq!(per_causa.validate_for_emission(), Ok(()));
    }

    #[test]
    fn rc17_rejects_false_truncation_and_read_input_total() {
        let mut false_truncation = report(1, vec![example(0)]);
        false_truncation.examples_truncated = true;
        assert!(false_truncation.validate_for_emission().is_err());

        let mut read_input_total = report(1, vec![example(0)]);
        read_input_total.input_total = Some(1);
        assert!(read_input_total.validate_for_emission().is_err());
    }

    #[test]
    fn le_basi_degli_indici_sono_due_e_chiuse() {
        let mut passo = report(1, vec![example(3)]);
        passo.index_basis = ROW_DIAGNOSTICS_INDEX_BASIS_STEP_INPUT.to_owned();
        assert_eq!(passo.validate_for_emission(), Ok(()));
        let testo = serde_json::to_string(&passo).expect("serializzabile");
        assert!(testo.contains("\"index_basis\":\"step_input_row_zero_based\""));
        let riletto: RowDiagnostics = serde_json::from_str(&testo).expect("rileggibile");
        assert_eq!(riletto, passo);
        for altra in ["", "source_row_one_based", "step_input_row"] {
            let mut ignota = report(1, vec![example(3)]);
            ignota.index_basis = altra.to_owned();
            assert_eq!(
                ignota.validate_for_emission(),
                Err("campi radice non validi"),
                "{altra}"
            );
        }
    }

    #[test]
    fn rc17_accepts_partial_zero_and_unordered_unique_examples() {
        let mut partial = report(0, Vec::new());
        partial.completeness = RowDiagnosticsCompleteness::Partial;
        partial.total = None;
        partial.knowledge_limits = Some(vec!["scan.interrupted".to_owned()]);
        assert_eq!(partial.validate_for_emission(), Ok(()));

        let unordered = report(2, vec![example(1), example(0)]);
        assert_eq!(unordered.validate_for_emission(), Ok(()));
    }

    #[test]
    fn rc17_validates_complete_and_unknown_write_partitions() {
        let complete = confirmed_write_report();
        assert_eq!(complete.validate_for_emission(), Ok(()));
        let complete_wire =
            serde_json::to_value(&complete).expect("fixture completa serializzabile");
        assert_eq!(
            complete_wire,
            serde_json::json!({
                "contract": "plenora-row-diagnostics-v1",
                "scope": "write",
                "index_basis": "source_row_zero_based",
                "completeness": "complete",
                "observed_total": 1,
                "total": 1,
                "input_total": 5200,
                "counts": {"database.constraint_violation": 1},
                "examples_limit": 10,
                "examples_truncated": false,
                "examples": [{
                    "source_index": 4999,
                    "cause": "database.constraint_violation",
                    "column": "area_m2",
                    "key": {"field": "parcel_id", "state": "redacted"},
                    "write_state": "certainly_rejected"
                }],
                "diagnostic_state_counts": {
                    "certainly_rejected": 1,
                    "certainly_not_attempted": 0,
                    "certainly_rolled_back": 0,
                    "effect_unknown": 0
                },
                "write_outcome": {
                    "certainly_rejected": {"state": "known", "value": 1},
                    "certainly_not_attempted": {"state": "known", "value": 200},
                    "certainly_rolled_back": {"state": "known", "value": 4999},
                    "effect_unknown": {"state": "known", "value": 0}
                }
            })
        );

        let mut unknown = complete;
        unknown.write_outcome = Some(RowDiagnosticWriteOutcome {
            certainly_rejected: KnownOrUnknownCount::Known { value: 1 },
            certainly_not_attempted: KnownOrUnknownCount::Known { value: 200 },
            certainly_rolled_back: KnownOrUnknownCount::Unknown,
            effect_unknown: KnownOrUnknownCount::Unknown,
        });
        assert_eq!(unknown.validate_for_emission(), Ok(()));
        let mut expected_unknown = complete_wire;
        expected_unknown["write_outcome"]["certainly_rolled_back"] =
            serde_json::json!({"state": "unknown"});
        expected_unknown["write_outcome"]["effect_unknown"] =
            serde_json::json!({"state": "unknown"});
        assert_eq!(
            serde_json::to_value(&unknown).expect("fixture outcome ignoto serializzabile"),
            expected_unknown
        );
    }

    #[test]
    fn rc17_rejects_write_partition_mismatch_and_checked_overflow() {
        let mut mismatch = confirmed_write_report();
        mismatch.write_outcome = Some(RowDiagnosticWriteOutcome {
            certainly_rejected: KnownOrUnknownCount::Known { value: 1 },
            certainly_not_attempted: KnownOrUnknownCount::Known { value: 200 },
            certainly_rolled_back: KnownOrUnknownCount::Known { value: 4_998 },
            effect_unknown: KnownOrUnknownCount::Known { value: 0 },
        });
        assert!(mismatch.validate_for_emission().is_err());

        let mut overflow = report(u64::MAX, Vec::new());
        overflow.completeness = RowDiagnosticsCompleteness::Partial;
        overflow.total = None;
        overflow.knowledge_limits = Some(vec!["counter.overflow".to_owned()]);
        overflow.counts = BTreeMap::from([("a".to_owned(), u64::MAX), ("b".to_owned(), 1)]);
        assert!(overflow.validate_for_emission().is_err());
    }

    #[test]
    fn rc17_enforces_key_policy_and_unicode_character_limits() {
        let mut valid = confirmed_write_report();
        let key = valid.examples[0].key.as_mut().expect("chiave fixture");
        key.state = RowDiagnosticKeyState::Value;
        key.field = "é".repeat(256);
        key.value = Some(RowDiagnosticKeyValue::String("界".repeat(1_024)));
        assert_eq!(valid.validate_for_emission(), Ok(()));

        let mut invalid_length = valid.clone();
        invalid_length.examples[0]
            .key
            .as_mut()
            .expect("chiave fixture")
            .field = "é".repeat(257);
        assert!(invalid_length.validate_for_emission().is_err());

        let mut invalid_redaction = valid;
        let key = invalid_redaction.examples[0]
            .key
            .as_mut()
            .expect("chiave fixture");
        key.state = RowDiagnosticKeyState::Redacted;
        assert!(invalid_redaction.validate_for_emission().is_err());
    }

    #[test]
    fn rc17_rejects_duplicate_knowledge_limits_and_missing_write_state() {
        let mut unknown_zero = report(0, Vec::new());
        unknown_zero.completeness = RowDiagnosticsCompleteness::Unknown;
        unknown_zero.total = None;
        unknown_zero.knowledge_limits = Some(vec!["scan.interrupted".to_owned()]);
        assert_eq!(unknown_zero.validate_for_emission(), Ok(()));

        unknown_zero.knowledge_limits = Some(vec![
            "scan.interrupted".to_owned(),
            "scan.interrupted".to_owned(),
        ]);
        assert!(unknown_zero.validate_for_emission().is_err());

        let mut missing_state = confirmed_write_report();
        missing_state.examples[0].write_state = None;
        assert!(missing_state.validate_for_emission().is_err());
    }

    #[test]
    fn serde_refuses_a_directly_constructed_invalid_report() {
        let invalid = report(1, vec![example(0), example(1)]);
        assert!(serde_json::to_value(invalid).is_err());
    }

    #[test]
    fn serde_refuses_rc17_invalid_json_before_constructing_public_type() {
        let valid = serde_json::to_value(report(1, vec![example(0)]))
            .expect("fixture valida serializzabile");
        let invalid_documents = [
            {
                let mut value = valid.clone();
                value["examples_limit"] = serde_json::json!(0);
                value
            },
            {
                let mut value = valid.clone();
                value["contract"] = serde_json::json!("wrong-contract");
                value
            },
            {
                let mut value = valid.clone();
                value["observed_total"] = serde_json::json!(2);
                value
            },
            {
                let mut value = valid.clone();
                value["total"] = serde_json::json!(2);
                value
            },
            {
                let mut value = valid;
                value["input_total"] = serde_json::json!(1);
                value
            },
        ];
        for document in invalid_documents {
            assert!(serde_json::from_value::<RowDiagnostics>(document).is_err());
        }
    }
}
