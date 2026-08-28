use std::collections::{BTreeMap, HashSet};
use std::fmt;

use serde::{Deserialize, Serialize};

pub const ROW_DIAGNOSTICS_CONTRACT: &str = "plenora-row-diagnostics-v1";
pub const ROW_DIAGNOSTICS_INDEX_BASIS: &str = "source_row_zero_based";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RowDiagnosticScope {
    Read,
    Write,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RowDiagnosticsCompleteness {
    Complete,
    Partial,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RowDiagnosticKeyState {
    Value,
    Redacted,
    Unavailable,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RowDiagnosticKeyValue {
    String(String),
    Integer(i64),
    Boolean(bool),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RowDiagnosticKey {
    pub field: String,
    pub state: RowDiagnosticKeyState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<RowDiagnosticKeyValue>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RowDiagnosticWriteState {
    CertainlyRejected,
    CertainlyNotAttempted,
    CertainlyRolledBack,
    EffectUnknown,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RowDiagnosticExample {
    pub source_index: u64,
    pub cause: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub column: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<RowDiagnosticKey>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub write_state: Option<RowDiagnosticWriteState>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum KnownOrUnknownCount {
    Known { value: u64 },
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WriteDiagnosticStateCounts {
    pub certainly_rejected: u64,
    pub certainly_not_attempted: u64,
    pub certainly_rolled_back: u64,
    pub effect_unknown: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RowDiagnosticWriteOutcome {
    pub certainly_rejected: KnownOrUnknownCount,
    pub certainly_not_attempted: KnownOrUnknownCount,
    pub certainly_rolled_back: KnownOrUnknownCount,
    pub effect_unknown: KnownOrUnknownCount,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RowDiagnostics {
    pub contract: String,
    pub scope: RowDiagnosticScope,
    pub index_basis: String,
    pub completeness: RowDiagnosticsCompleteness,
    pub knowledge_limits: Option<Vec<String>>,
    pub observed_total: u64,
    pub total: Option<u64>,
    pub input_total: Option<u64>,
    pub counts: BTreeMap<String, u64>,
    pub examples_limit: u64,
    pub examples_truncated: bool,
    pub examples: Vec<RowDiagnosticExample>,
    pub diagnostic_state_counts: Option<WriteDiagnosticStateCounts>,
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

/// Invariante violata fondendo due report row-scoped.
///
/// E' un enum e non una stringa perche' i due chiamanti — l'executor e il
/// trasporto geo — hanno tipi di errore diversi e devono poterlo tradurre nel
/// proprio senza inventare messaggi: [`RowDiagnosticsMergeError::message`] e'
/// la sola forma testuale, `&'static str`, senza dati.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowDiagnosticsMergeError {
    /// I due report descrivono contratti, scope, basi d'indice o limiti
    /// diversi: non sono lo stesso report e non vanno sommati.
    IncompatibleReports,
    /// L'offset sorgente porta un indice fuori intervallo.
    SourceIndexOverflow,
    /// Somma di `observed_total` fuori intervallo.
    ObservedTotalOverflow,
    /// Somma di `total` fuori intervallo.
    TotalOverflow,
    /// Somma di `input_total` fuori intervallo.
    InputTotalOverflow,
    /// Somma dei conteggi per causa fuori intervallo.
    CauseCountOverflow,
    /// Numero di esempi non rappresentabile.
    ExampleCountOverflow,
    /// Report di scope `Write` con payload di scrittura: non esiste una
    /// semantica di fusione dichiarata per `diagnostic_state_counts` e
    /// `write_outcome`, e fonderne solo una parte perderebbe in silenzio
    /// proprio i campi che distinguono quel report.
    WriteScopeNotMergeable,
}

impl RowDiagnosticsMergeError {
    /// Messaggio stabile dell'invariante violata (nessun valore dei dati).
    #[must_use]
    pub const fn message(self) -> &'static str {
        match self {
            Self::IncompatibleReports => "report row-scoped incompatibili nello stesso stream",
            Self::SourceIndexOverflow => "indice sorgente fuori intervallo",
            Self::ObservedTotalOverflow => "conteggio row-scoped fuori intervallo",
            Self::TotalOverflow => "totale row-scoped fuori intervallo",
            Self::InputTotalOverflow => "input_total diagnostico overflow",
            Self::CauseCountOverflow => "conteggio causa fuori intervallo",
            Self::ExampleCountOverflow => "numero esempi fuori intervallo",
            Self::WriteScopeNotMergeable => {
                "report row-scoped di scrittura non fondibile: payload write senza semantica di fusione"
            }
        }
    }
}

impl fmt::Display for RowDiagnosticsMergeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.message())
    }
}

/// Peggiore fra due livelli di completeness: `Unknown` > `Partial` >
/// `Complete`.
///
/// Fondere due report non puo' mai aumentare cio' che si sa: il risultato
/// eredita il livello di conoscenza piu' debole dei due contributi.
const fn worst_completeness(
    left: RowDiagnosticsCompleteness,
    right: RowDiagnosticsCompleteness,
) -> RowDiagnosticsCompleteness {
    match (left, right) {
        (RowDiagnosticsCompleteness::Unknown, _) | (_, RowDiagnosticsCompleteness::Unknown) => {
            RowDiagnosticsCompleteness::Unknown
        }
        (RowDiagnosticsCompleteness::Partial, _) | (_, RowDiagnosticsCompleteness::Partial) => {
            RowDiagnosticsCompleteness::Partial
        }
        (RowDiagnosticsCompleteness::Complete, RowDiagnosticsCompleteness::Complete) => {
            RowDiagnosticsCompleteness::Complete
        }
    }
}

impl RowDiagnostics {
    /// Fonde `incoming` nel report accumulato applicando l'offset sorgente
    /// assoluto: conteggi sommati in aritmetica controllata, esempi limitati
    /// a `examples_limit`, completeness degradata se un contributo qualunque
    /// non e' completo.
    ///
    /// Vive qui, accanto al tipo, perche' la stessa procedura serve due
    /// percorsi — l'executor del DAG e il runner fuso del trasporto geo — ed
    /// e' logica di contratto (R9.9: mai un indice inventato, mai un
    /// overflow silenzioso). Due copie parallele, fossero anche identiche
    /// riga per riga, sarebbero due occasioni di divergere su un report che
    /// finisce in output.
    ///
    /// # Errors
    ///
    /// [`RowDiagnosticsMergeError`] su report incompatibili o su qualunque
    /// somma fuori intervallo.
    pub fn merge_into(
        aggregate: &mut Option<Self>,
        mut incoming: Self,
        source_offset: u64,
    ) -> std::result::Result<(), RowDiagnosticsMergeError> {
        for example in &mut incoming.examples {
            example.source_index = source_offset
                .checked_add(example.source_index)
                .ok_or(RowDiagnosticsMergeError::SourceIndexOverflow)?;
        }
        let Some(existing) = aggregate.as_ref() else {
            *aggregate = Some(incoming);
            return Ok(());
        };
        let mut merged = existing.clone();
        if merged.contract != incoming.contract
            || merged.scope != incoming.scope
            || merged.index_basis != incoming.index_basis
            || merged.examples_limit != incoming.examples_limit
        {
            return Err(RowDiagnosticsMergeError::IncompatibleReports);
        }
        merged.observed_total = merged
            .observed_total
            .checked_add(incoming.observed_total)
            .ok_or(RowDiagnosticsMergeError::ObservedTotalOverflow)?;
        merged.total = match (merged.total, incoming.total) {
            (Some(left), Some(right)) => Some(
                left.checked_add(right)
                    .ok_or(RowDiagnosticsMergeError::TotalOverflow)?,
            ),
            _ => None,
        };
        merged.input_total = match (merged.input_total, incoming.input_total) {
            (Some(left), Some(right)) => Some(
                left.checked_add(right)
                    .ok_or(RowDiagnosticsMergeError::InputTotalOverflow)?,
            ),
            _ => None,
        };
        for (cause, count) in incoming.counts {
            let entry = merged.counts.entry(cause).or_default();
            *entry = entry
                .checked_add(count)
                .ok_or(RowDiagnosticsMergeError::CauseCountOverflow)?;
        }
        let incoming_write_payload =
            incoming.diagnostic_state_counts.is_some() || incoming.write_outcome.is_some();
        let incoming_example_count = incoming.examples.len();
        let before = merged.examples.len();
        for example in incoming.examples {
            if u64::try_from(merged.examples.len())
                .map_err(|_| RowDiagnosticsMergeError::ExampleCountOverflow)?
                >= merged.examples_limit
            {
                break;
            }
            merged.examples.push(example);
        }
        merged.examples_truncated = merged.examples_truncated
            || incoming.examples_truncated
            || merged.examples.len().saturating_sub(before) < incoming_example_count;
        // Reticolo esplicito della completeness: `Unknown` > `Partial` >
        // `Complete`, e la fusione tiene il PEGGIORE dei due.
        //
        // L'assegnazione diretta dipenderebbe dall'ordine: `Unknown` che
        // riceve `Partial` diventerebbe `Partial`, cioe' migliorerebbe
        // illegittimamente l'informazione — «non so quanto ho visto»
        // tornerebbe «ho visto una parte» — mentre nell'ordine opposto
        // resterebbe `Unknown`. Due stream con gli stessi contributi in
        // ordine diverso darebbero due report diversi.
        merged.completeness = worst_completeness(merged.completeness, incoming.completeness);
        if incoming.completeness != RowDiagnosticsCompleteness::Complete {
            let mut knowledge_limits = merged.knowledge_limits.take().unwrap_or_default();
            for limit in incoming.knowledge_limits.unwrap_or_default() {
                if !knowledge_limits.contains(&limit) {
                    knowledge_limits.push(limit);
                }
            }
            merged.knowledge_limits = (!knowledge_limits.is_empty()).then_some(knowledge_limits);
        }
        // Scope `Write`: i contatori di stato e l'esito di scrittura sono
        // parte del report e vanno fusi anch'essi. Finche' non esiste una
        // semantica di fusione dichiarata per questi due campi, un report
        // `Write` non e' fondibile e va rifiutato invece di essere fuso a
        // meta' — perdendo in silenzio proprio i campi che lo distinguono.
        if merged.scope == RowDiagnosticScope::Write
            && (merged.diagnostic_state_counts.is_some()
                || merged.write_outcome.is_some()
                || incoming_write_payload)
        {
            return Err(RowDiagnosticsMergeError::WriteScopeNotMergeable);
        }
        *aggregate = Some(merged);
        Ok(())
    }

    /// Declassa il report a `Partial` con `total` sconosciuto e il knowledge
    /// limit dichiarato: la scansione completa non e' piu' dimostrabile.
    #[must_use]
    pub fn into_partial(mut self, knowledge_limit: &str) -> Self {
        // `Unknown` non torna `Partial`: declassare non puo' aumentare cio'
        // che si sa. Il reticolo vale anche qui.
        self.completeness =
            worst_completeness(self.completeness, RowDiagnosticsCompleteness::Partial);
        self.total = None;
        // I limiti gia' registrati si UNISCONO, non si sostituiscono:
        // rimpiazzarli con un singoletto cancellerebbe conoscenza gia'
        // acquisita — il report direbbe un solo motivo di incompletezza dove
        // ce n'e' piu' d'uno.
        let mut limits = self.knowledge_limits.take().unwrap_or_default();
        if !limits.iter().any(|limit| limit == knowledge_limit) {
            limits.push(knowledge_limit.to_owned());
        }
        self.knowledge_limits = Some(limits);
        self
    }
}

impl RowDiagnostics {
    /// Valida schema e invarianti aritmetiche rc17 prima dell'emissione.
    ///
    /// # Errors
    ///
    /// Restituisce un errore bounded, senza dati di riga, se il report non è
    /// serializzabile secondo `plenora-row-diagnostics-v1`.
    // La sequenza resta intenzionalmente monolitica per essere confrontabile,
    // nell'ordine, con il validator normativo rc17.
    #[allow(clippy::too_many_lines)]
    pub fn validate_for_emission(&self) -> Result<(), &'static str> {
        if self.contract != ROW_DIAGNOSTICS_CONTRACT
            || self.index_basis != ROW_DIAGNOSTICS_INDEX_BASIS
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
mod tests {
    use super::*;

    fn report(observed_total: u64, examples: Vec<RowDiagnosticExample>) -> RowDiagnostics {
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

    fn example(source_index: u64) -> RowDiagnosticExample {
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
        let invalid = report(1, vec![example(0), example(1)]);
        assert!(invalid.validate_for_emission().is_err());
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
