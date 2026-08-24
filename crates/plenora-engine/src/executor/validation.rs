//! Validazione dinamica: cio' che solo i dati possono dire.
//!
//! La validazione statica (decisione D8) legge header e metadati e non puo'
//! guardare dentro le celle. Qui si controlla il resto: la validita' strutturale
//! del WKB, i tetti di righe e byte per arco, e i vincoli di espansione — quanto
//! un'operazione ha il diritto di far crescere il proprio input.
//!
//! # Perche' l'espansione ha un tetto
//!
//! Un'operazione che moltiplica le righe (un join, un explode) puo' trasformare
//! un input innocuo in un output ingestibile. Il vincolo dichiarato nel catalogo
//! dice quanto e' lecito crescere; qui si verifica che sia rispettato, sui dati
//! veri e non sulla stima.

use std::collections::BTreeMap;

use plenora_core::arrow::array::{Array, RecordBatch};
use plenora_core::catalog::{
    find_operation, CancellationBehavior, ExpansionConstraint, JoinExpansion,
};
use plenora_core::contract::{DataContract, GeometryDimensions, GeometryEncoding};
use plenora_core::diagnostics::{
    RowDiagnosticExample, RowDiagnosticScope, RowDiagnostics, RowDiagnosticsCompleteness,
    ROW_DIAGNOSTICS_CONTRACT, ROW_DIAGNOSTICS_INDEX_BASIS,
};
use plenora_core::error::ReplayedError;
use plenora_core::{ErrorPhase, PlenoraError, Result};
use plenora_kernels_geo::arrow_adapter::{
    batch_geometry_cells, canonical_geometry_encoding, canonical_geometry_srid,
};
use plenora_kernels_geo::validate_wkb_transport_for_dimensions_with_depth;

use crate::prepare::PreparedKernel;

use super::check_batch_bytes;
use super::state::ExecState;

// Il contesto di validazione e' un gruppo di parametri coeso (celle, limiti,
// framing, offset): estrarlo in una struct dedicata non aggiungerebbe
// informazione in questo choke point interno.
#[allow(clippy::too_many_arguments)]
pub(super) fn validate_wkb_cells(
    state: &ExecState,
    batch: &RecordBatch,
    geometry_index: usize,
    edge: &str,
    dimensions: GeometryDimensions,
    encoding: GeometryEncoding,
    expected_srid: Option<u32>,
    source_offset: u64,
) -> Result<()> {
    const EXAMPLES_LIMIT: u64 = 10;
    let cells = batch_geometry_cells(batch, geometry_index, "geometry")?;
    let limits = state.plan.limits();
    let max_cell = limits.max_wkb_cell_bytes;
    let max_depth = limits.max_geometry_depth as usize;
    // Diagnostica opt-in (errori arricchiti): il nome della colonna e' contesto
    // strutturale, non un valore — risolto solo nel ramo d'errore (hot path minimale),
    // mai allocato sul percorso felice.
    let column = batch.schema().field(geometry_index).name().clone();
    let mut rejected = Vec::new();
    for row in 0..batch.num_rows() {
        if cells.is_null(row) {
            continue;
        }
        let payload = cells.value(row);
        if payload.len() as u64 > max_cell {
            rejected.push((row, "geometry.cell_too_large"));
            continue;
        }
        if let Err(error) = validate_wkb_transport_for_dimensions_with_depth(
            payload,
            dimensions,
            encoding,
            expected_srid,
            max_depth,
        ) {
            rejected.push((
                row,
                if matches!(error, PlenoraError::Crs(_)) {
                    "geometry.crs_mismatch"
                } else {
                    "geometry.invalid_wkb"
                },
            ));
        }
    }
    if rejected.is_empty() {
        return Ok(());
    }
    let observed_total = u64::try_from(rejected.len())
        .map_err(|_| PlenoraError::Internal("troppe rejection WKB".into()))?;
    let mut counts = BTreeMap::new();
    let mut examples = Vec::new();
    for (row, cause) in rejected {
        let count = counts.entry(cause.to_owned()).or_insert(0_u64);
        *count = count
            .checked_add(1)
            .ok_or_else(|| PlenoraError::Internal("overflow conteggio rejection WKB".into()))?;
        if u64::try_from(examples.len())
            .map_err(|_| PlenoraError::Internal("troppi esempi WKB".into()))?
            < EXAMPLES_LIMIT
        {
            examples.push(RowDiagnosticExample {
                source_index: source_offset
                    .checked_add(u64::try_from(row).map_err(|_| {
                        PlenoraError::Internal("indice WKB non rappresentabile".into())
                    })?)
                    .ok_or_else(|| PlenoraError::Internal("overflow indice WKB".into()))?,
                cause: cause.to_owned(),
                column: Some(column.clone()),
                key: None,
                write_state: None,
            });
        }
    }
    let report = RowDiagnostics {
        contract: ROW_DIAGNOSTICS_CONTRACT.to_owned(),
        scope: RowDiagnosticScope::Read,
        index_basis: ROW_DIAGNOSTICS_INDEX_BASIS.to_owned(),
        completeness: RowDiagnosticsCompleteness::Complete,
        knowledge_limits: None,
        observed_total,
        total: Some(observed_total),
        input_total: None,
        counts,
        examples_limit: EXAMPLES_LIMIT,
        examples_truncated: observed_total > EXAMPLES_LIMIT,
        examples,
        diagnostic_state_counts: None,
        write_outcome: None,
    };
    Err(PlenoraError::DataMapping(format!(
        "geometrie non conformi sull'arco `{edge}`; consultare row_diagnostics"
    ))
    .with_phase(ErrorPhase::Read)
    .with_row_diagnostics(report))
}

pub(super) fn geometry_input_requirements(
    contract: &DataContract,
) -> Result<(
    Option<usize>,
    GeometryDimensions,
    GeometryEncoding,
    Option<u32>,
)> {
    let geometry = contract.active_geometry_column();
    let index = geometry
        .map(|geometry| {
            contract
                .schema
                .column_with_name(&geometry.name)
                .map(|resolved| resolved.0)
                .ok_or_else(|| {
                    PlenoraError::Internal(format!(
                        "colonna geometria `{}` assente dallo schema del contratto",
                        geometry.name
                    ))
                })
        })
        .transpose()?;
    let dimensions = geometry.map_or(GeometryDimensions::Xy, |geometry| geometry.dimensions);
    let declared_encoding = index
        .map(|index| canonical_geometry_encoding(contract.schema.field(index)))
        .transpose()?
        .flatten();
    let contract_encoding = geometry.and_then(|geometry| geometry.encoding);
    if let Some((declared, governed)) = declared_encoding
        .zip(contract_encoding)
        .filter(|(declared, governed)| declared != governed)
    {
        let column = geometry.map_or("<assente>", |geometry| geometry.name.as_str());
        return Err(PlenoraError::Schema(format!(
            "colonna geometria `{column}`: encoding field `{}` incoerente con encoding contratto `{}`",
            declared.as_str(),
            governed.as_str()
        )));
    }
    let encoding = contract_encoding
        .or(declared_encoding)
        .unwrap_or(GeometryEncoding::Wkb);
    let declared_srid = index
        .map(|index| canonical_geometry_srid(contract.schema.field(index)))
        .transpose()?
        .flatten();
    let resolved_srid = geometry
        .and_then(|geometry| geometry.crs.as_resolved())
        .and_then(|crs| {
            crs.authority_srid()
                .or_else(|| plenora_core::crs::authority_code_srid(crs.definition()))
        });
    if resolved_srid
        .zip(declared_srid)
        .is_some_and(|(resolved, declared)| resolved != declared)
    {
        return Err(PlenoraError::Crs(
            "SRID dichiarato incoerente con il CRS risolto".to_owned(),
        ));
    }
    let srid = resolved_srid.or(declared_srid);
    Ok((index, dimensions, encoding, srid))
}

/// Contatori e limiti dell'arco intermedio prodotto da un kernel
/// (`max_rows_per_edge`, `max_batches`, byte per batch).
pub(super) fn check_edge_batch(state: &ExecState, edge: &str, batch: &RecordBatch) -> Result<()> {
    let _ = check_batch_bytes(state, batch, edge)?;
    check_edge_counts(state, edge, batch.num_rows() as u64)
}

/// Conteggi righe/batch di un arco intermedio e limiti corrispondenti
/// (`max_rows_per_edge`, `max_batches`) SENZA il tetto byte del batch:
/// archi interni dei gruppi fusi geo (architettura.md#geometrie D12.8, deroga errori-e-limiti.md#limiti-dichiarati di
/// `docs/errori-e-limiti.md` — il batch non e' materializzato e H-03 e' coperto dal
/// governor, reservation D12.7). Righe e batch restano esatti (1:1).
pub(super) fn check_edge_counts(state: &ExecState, edge: &str, rows: u64) -> Result<()> {
    let mut counts = state.edge_counts.borrow_mut();
    // Chiave clonata solo al primo batch dell'arco (hot path minimale): i batch successivi
    // entrano dal `get_mut` sul nome esistente.
    if let Some(entry) = counts.get_mut(edge) {
        entry.0 = entry
            .0
            .checked_add(rows)
            .ok_or_else(|| PlenoraError::Internal("overflow conteggio righe arco".into()))?;
        entry.1 = entry
            .1
            .checked_add(1)
            .ok_or_else(|| PlenoraError::Internal("overflow conteggio batch arco".into()))?;
    } else {
        counts.insert(edge.to_owned(), (rows, 1));
    }
    let entry = &counts[edge];
    let limits = &state.plan.limits();
    if entry.0 > limits.rows.max_rows_per_edge {
        return Err(PlenoraError::ResourceLimit(format!(
            "max_rows_per_edge superato sull'arco `{edge}`: {} righe > {}",
            entry.0, limits.rows.max_rows_per_edge
        )));
    }
    if entry.1 > limits.max_batches {
        return Err(PlenoraError::ResourceLimit(format!(
            "max_batches superato sull'arco `{edge}`: {} batch > {}",
            entry.1, limits.max_batches
        )));
    }
    Ok(())
}

/// Esenzione da `max_expansion_factor` dichiarata in catalogo (errori-e-limiti.md):
/// le op `WholeToMany` (`geo.generate_grid`, `geo.coverage_validate`,
/// `geo.shared_paths`) sono generative/diagnostiche — l'input funge da
/// trigger o da insieme da analizzare, non da base proporzionale
/// dell'output. Risolta in `prepare` (hot path minimale: nessuno scan del catalogo nel
/// loop per batch).
pub(super) const fn expansion_exempt(kernel: &PreparedKernel) -> bool {
    kernel.expansion_factor_exempt
}

/// Comportamento alla cancellazione del kernel (errori-e-limiti.md#cancellazione): dichiarato in
/// catalogo dal descriptor dell'operazione e risolto in `prepare` (hot path minimale:
/// nessuno scan del catalogo nel loop per batch).
// Non const fn: sotto cfg(test) chiama l'hook `test_behavior_override`
// (non const) — nel build normale il corpo e' la sola lettura di campo.
#[allow(clippy::missing_const_for_fn)]
pub(super) fn cancellation_behavior(kernel: &PreparedKernel) -> CancellationBehavior {
    #[cfg(test)]
    if let Some(behavior) = test_behavior_override(&kernel.node_id) {
        return behavior;
    }
    kernel.cancellation_behavior
}

/// Hook di test (errori-e-limiti.md#cancellazione): override del
/// `CancellationBehavior` di catalogo
/// per id nodo. Serve a verificare il rispetto dei behavior senza i backend
/// opzionali: le sole op `NonInterruptible` del catalogo v1
/// (`geo.make_valid`, `geo.reproject`, `geo.polygonize`, `geo.split`)
/// richiedono le capability `geos`/`proj`. Stesso pattern di
/// [`PANIC_AT_NODES`]: insieme, registrazione/deregistrazione per test.
#[cfg(test)]
pub(super) static CANCEL_BEHAVIOR_OVERRIDES: std::sync::Mutex<Vec<(String, CancellationBehavior)>> =
    std::sync::Mutex::new(Vec::new());

/// Lettura dell'override di behavior di test (scatta solo ai nodi
/// registrati nell'hook).
#[cfg(test)]
pub(super) fn test_behavior_override(node_id: &str) -> Option<CancellationBehavior> {
    CANCEL_BEHAVIOR_OVERRIDES
        .lock()
        .expect("hook behavior non avvelenato")
        .iter()
        .find_map(|(node, behavior)| (node == node_id).then_some(*behavior))
}

/// Fattore di espansione per nodo unario (errori-e-limiti.md: base = righe di input;
/// per i binari si veda [`check_join_expansion`]).
///
/// Per le op esenti (dichiarato in catalogo, [`expansion_exempt`]) il
/// controllo sul fattore output/input non si applica: la produzione resta
/// limitata da `max_rows_per_edge` / `max_output_rows` / `max_batches`.
pub(super) fn check_expansion(
    state: &ExecState,
    kernel: &PreparedKernel,
    base_rows: u64,
) -> Result<()> {
    if expansion_exempt(kernel) {
        return Ok(());
    }
    let mut rows = state.node_rows.borrow_mut();
    // Chiave clonata solo al primo batch del nodo (hot path minimale): i batch successivi
    // entrano dal `get_mut` sull'id esistente.
    if let Some(entry) = rows.get_mut(&kernel.node_id) {
        entry.0 = entry.0.saturating_add(base_rows);
    } else {
        let entry = rows.entry(kernel.node_id.clone()).or_insert((0, 0));
        entry.0 = entry.0.saturating_add(base_rows);
    }
    let entry = &rows[&kernel.node_id];
    let factor = state.plan.limits().rows.max_expansion_factor;
    if plenora_core::limits::expansion_exceeded(entry.1, entry.0, factor) {
        return Err(PlenoraError::ResourceLimit(format!(
            "max_expansion_factor superato al nodo `{}`: {} righe output > {} x {} righe input",
            kernel.node_id, entry.1, factor, entry.0
        )));
    }
    Ok(())
}

/// Fattore di espansione per nodo binario (errori-e-limiti.md): calcola tutte le
/// metriche [`JoinExpansion`] e applica il vincolo vincolante dichiarato in
/// catalogo per l'operazione (default `SumRelative` se l'op non e' in
/// catalogo — non dovrebbe accadere: il piano e' validato sul catalogo).
/// La soglia e' `max_expansion_factor` dei limiti effettivi, tranne per il
/// vincolo `Custom(fattore)`: il fattore dichiarato in catalogo la
/// sovrascrive per la singola operazione (stima a priori,
/// architettura.md#planner-ed-executor, errori-e-limiti.md).
pub(super) fn check_join_expansion(
    state: &ExecState,
    kernel: &PreparedKernel,
    left_rows: u64,
    right_rows: u64,
    output_rows: u64,
) -> Result<()> {
    // Entrambe le proprieta' sono STATICHE dell'operazione e risolte in
    // preparazione. Prima si rileggevano dal catalogo a ogni verifica, con una
    // ricerca lineare su 146 descrittori per una risposta che non cambia mai —
    // e con un `map_or` che, se il descrittore fosse mancato, avrebbe
    // silenziosamente applicato il vincolo di default invece di dirlo.
    if kernel.expansion_factor_exempt {
        return Ok(());
    }
    let constraint = kernel.expansion_constraint;
    let max_expansion_factor = state.plan.limits().rows.max_expansion_factor;
    if !constraint.exceeded(output_rows, left_rows, right_rows, max_expansion_factor) {
        return Ok(());
    }
    // Le metriche si calcolano solo per RACCONTARE l'esito: la decisione e'
    // gia' presa sui conteggi interi, e i rapporti `f64` che seguono sono
    // arrotondati sopra 2^53 righe — cioe' proprio dove il difetto stava.
    let expansion = JoinExpansion::compute(output_rows, left_rows, right_rows);
    let factor = constraint.binding_threshold(max_expansion_factor);
    Err(PlenoraError::ResourceLimit(format!(
        "max_expansion_factor superato al nodo `{}` (vincolo {constraint:?}): \
         soglia {factor}, output={output_rows}, left={left_rows}, right={right_rows}; \
         metriche osservate (approssimate): output/(left+right)={}, \
         output/left={}, output/right={}",
        kernel.node_id,
        expansion.output_over_sum_inputs,
        expansion.output_over_left,
        expansion.output_over_right,
    )))
}

/// Errore di un kernel attribuito al nodo logico (osservabilita' per nodo), preservando la
/// diagnosi senza dati sensibili. L'`execution_id` e' vuoto qui (il dispatch
/// in profondita' non lo ha a disposizione): lo riempie il tag di categoria al
/// confine di uscita (`ExecState::tag_execution`).
pub(super) fn step_error(kernel: &PreparedKernel, error: PlenoraError) -> PlenoraError {
    if let Some(diagnostics) = error.row_diagnostics().cloned() {
        let replayed = PlenoraError::Replayed(Box::new(ReplayedError {
            category: error.category(),
            phase: error.phase(),
            remote_effect: error.remote_effect(),
            retry: error.retry_disposition(),
            message: error.to_string(),
            node: Some(kernel.node_id.clone()),
            operation: Some(kernel.operation.as_str().to_owned()),
            execution_id: None,
            execution_reason: error.execution_reason().map(ToOwned::to_owned),
        }));
        return replayed.with_row_diagnostics(diagnostics);
    }
    // Un limite di RISORSA non diventa `Execution`: la categoria e' cio' su
    // cui il chiamante decide (rilanciare con piu' budget, non correggere il
    // piano), e avvolgerla in `Execution` la faceva sparire — l'errore usciva
    // come `execution`/exit 6 invece di `resource_limit`/exit 4. Si conserva
    // la categoria e si aggiunge il contesto del nodo tramite `Replayed`, che
    // e' il portatore tipizzato di categoria + attribuzione.
    // Il riconoscimento passa da `category()`, non da un `matches!` sulla
    // variante ESTERNA: un `ResourceLimit` puo' arrivare dentro un involucro
    // trasparente — `Tagged` (fase dichiarata da un confine) o un `Replayed`
    // gia' costruito da un livello piu' interno — e in quel caso il match
    // sulla variante non lo vedeva, quindi la categoria si perdeva
    // esattamente nei casi in cui era stata dichiarata con piu' cura.
    // `category()` attraversa gli involucri per costruzione.
    //
    // QUALI categorie si preservano lo decide `error_propagation`, non questa
    // funzione: il gemello legacy fa la stessa scelta, e due elenchi scritti
    // a mano in due file erano gia' divergenti.
    if crate::error_propagation::categoria_preservata(error.category()) {
        return PlenoraError::Replayed(Box::new(ReplayedError {
            category: error.category(),
            phase: error.phase(),
            remote_effect: error.remote_effect(),
            retry: error.retry_disposition(),
            message: error.to_string(),
            node: Some(kernel.node_id.clone()),
            operation: Some(kernel.operation.as_str().to_owned()),
            execution_id: None,
            execution_reason: error.execution_reason().map(ToOwned::to_owned),
        }));
    }
    let reason = match error {
        PlenoraError::Execution { reason, .. } => reason,
        other => other.to_string(),
    };
    PlenoraError::Execution {
        node: kernel.node_id.clone(),
        operation: kernel.operation.as_str().to_owned(),
        execution_id: String::new(),
        reason,
    }
}
