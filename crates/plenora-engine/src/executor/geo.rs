//! Dispatch delle estensioni geo v1.1-v1.3 sugli adapter Arrow dei kernel.
//!
//! Le operazioni geometriche non passano dal dispatch tabellare: hanno bisogno
//! di decodificare le celle WKB, di conoscere quale colonna e' la geometria
//! attiva e di riattaccare l'output con l'encoding e il CRS giusti.
//!
//! # L'indice della colonna e' risolto in `prepare`
//!
//! `kernel_geometry_cells` non cerca la colonna: la trova per indice, deciso
//! una volta sola durante la preparazione. E' un hot path — per batch, non per
//! piano — e cercare un nome a ogni batch sarebbe lavoro ripetuto per una
//! risposta che non cambia.

use crate::geo_transport::pair::{decode_geometry_batches, preflight_decoded_bytes, PairOperation};
use crate::geo_transport::transport::TransformArrowSchema;
use crate::governor::GovernedBatch;
use crate::prepare::{
    AccessorKind, ExecutionPlan, GeoBinaryPlan, PreparedConfig, PreparedGeoKernel, PreparedKernel,
    PreparedTableKernel,
};
use crate::table_engine;
use crate::temp_store::TempStore;
use plenora_core::arrow::array::{
    Array, ArrayRef, BinaryArray, BooleanArray, Float64Array, RecordBatch, StringArray, UInt64Array,
};
use plenora_core::arrow::select::concat::concat_batches;
use plenora_core::arrow::select::take::take;
use plenora_core::contract::DataContract;
use plenora_core::{ErrorPhase, PlenoraError, Result};
use plenora_kernels_geo::analysis::{
    count_points_in_polygons_validated, nearest_matches_validated, within_indexes_validated,
};
use plenora_kernels_geo::arrow_adapter::{batch_geometry_cells, decode_geometry_cell};
use plenora_kernels_geo::spatial_join::spatial_join_nullable_validated;
/// Celle WKB della colonna geometria attiva del batch (indice risolto in
/// `prepare`, hot path minimale), con errore attribuito al nodo logico.
use std::collections::BTreeMap;
use std::rc::Rc;
use std::time::Instant;

#[cfg(test)]
use super::inject_test_panic;
use super::metrics::{accumulate, accumulate_time, sum_rows};
use super::state::ExecState;
use super::validation::{check_edge_batch, check_expansion, check_join_expansion};
use super::{
    blocking_output_sequence, check_batch_bytes, geo_binary_step_error, panic_step_error,
    record_kernel_metrics, run_kernel, step_error, GeoBinarySide, GeoBinaryStepError,
};

pub(super) fn kernel_geometry_cells<'a>(
    kernel: &PreparedKernel,
    batch: &'a RecordBatch,
) -> Result<&'a BinaryArray> {
    let geometry_index = kernel.geometry_column_index.ok_or_else(|| {
        step_error(
            kernel,
            PlenoraError::Schema("op geo senza colonna geometria".into()),
        )
    })?;
    let geometry_name = kernel.input_contracts[0]
        .active_geometry_column()
        .map_or("geometry", |geometry| geometry.name.as_str());
    batch_geometry_cells(batch, geometry_index, geometry_name)
        .map_err(|error| step_error(kernel, error))
}

/// Indice risolto della colonna geometria attiva (hot path minimale), con errore di nodo.
pub(super) fn kernel_geometry_index(kernel: &PreparedKernel) -> Result<usize> {
    kernel.geometry_column_index.ok_or_else(|| {
        step_error(
            kernel,
            PlenoraError::Schema("op geo senza colonna geometria".into()),
        )
    })
}

/// Batch con una colonna aggiunta in coda: lo schema e' quello del contratto
/// di output inferito dal planner (fonte unica di verita', configurazioni preparate).
pub(super) fn append_output_column(
    kernel: &PreparedKernel,
    batch: &RecordBatch,
    column: ArrayRef,
) -> Result<RecordBatch> {
    let mut columns = batch.columns().to_vec();
    columns.push(column);
    let righe = batch.num_rows();
    plenora_core::batch_with_rows(kernel.output_contract.schema.clone(), columns, righe)
        .map_err(|error| step_error(kernel, error))
}

/// `geo.from_wkt` (streaming 1:1): colonna WKT → nuova colonna geometria.
pub(super) fn geo_from_wkt_batch(
    kernel: &PreparedKernel,
    batch: &RecordBatch,
    wkt_column_index: usize,
    on_error: plenora_kernels_geo::extensions::OnWktError,
) -> Result<RecordBatch> {
    let values = batch
        .column(wkt_column_index)
        .as_any()
        .downcast_ref::<StringArray>()
        .ok_or_else(|| step_error(kernel, PlenoraError::Schema("colonna WKT non Utf8".into())))?;
    let column_name = batch.schema().field(wkt_column_index).name().clone();
    let cells = plenora_kernels_geo::extensions::from_wkt_column_named(
        values,
        on_error,
        Some(&column_name),
    )
    .map_err(|error| step_error(kernel, error))?;
    let geometry = BinaryArray::from(cells.iter().map(|cell| cell.as_deref()).collect::<Vec<_>>());
    append_output_column(kernel, batch, std::sync::Arc::new(geometry))
}

/// `geo.geometry_accessors` (streaming 1:1): colonne accessorie per riga;
/// geometria null → tutti gli accessori null.
pub(super) fn geo_accessors_batch(
    kernel: &PreparedKernel,
    batch: &RecordBatch,
    columns: &[(String, AccessorKind)],
) -> Result<RecordBatch> {
    let cells = kernel_geometry_cells(kernel, batch)?;
    let mut accessors: Vec<Option<plenora_kernels_geo::extensions::GeometryAccessors>> =
        Vec::with_capacity(batch.num_rows());
    for row in 0..batch.num_rows() {
        if cells.is_null(row) {
            accessors.push(None);
            continue;
        }
        let geometry =
            decode_geometry_cell(cells.value(row)).map_err(|error| step_error(kernel, error))?;
        let values = plenora_kernels_geo::extensions::geometry_accessors(&geometry)
            .map_err(|error| step_error(kernel, PlenoraError::InvalidPlan(error.to_string())))?;
        accessors.push(Some(values));
    }
    let mut produced: Vec<ArrayRef> = Vec::with_capacity(columns.len());
    for (_, kind) in columns {
        let column: ArrayRef = match kind {
            AccessorKind::GeometryType => std::sync::Arc::new(StringArray::from(
                accessors
                    .iter()
                    .map(|access| access.as_ref().map(|access| access.geometry_type))
                    .collect::<Vec<_>>(),
            )),
            AccessorKind::NumGeometries => std::sync::Arc::new(UInt64Array::from(
                accessors
                    .iter()
                    .map(|access| access.as_ref().map(|access| access.num_geometries))
                    .collect::<Vec<_>>(),
            )),
            AccessorKind::NumInteriorRings => std::sync::Arc::new(UInt64Array::from(
                accessors
                    .iter()
                    .map(|access| access.as_ref().map(|access| access.num_interior_rings))
                    .collect::<Vec<_>>(),
            )),
            AccessorKind::StartPoint => std::sync::Arc::new(StringArray::from(
                accessors
                    .iter()
                    .map(|access| {
                        access
                            .as_ref()
                            .and_then(|access| access.start_point.as_deref())
                    })
                    .collect::<Vec<_>>(),
            )),
            AccessorKind::EndPoint => std::sync::Arc::new(StringArray::from(
                accessors
                    .iter()
                    .map(|access| {
                        access
                            .as_ref()
                            .and_then(|access| access.end_point.as_deref())
                    })
                    .collect::<Vec<_>>(),
            )),
            AccessorKind::IsClosed => std::sync::Arc::new(BooleanArray::from(
                accessors
                    .iter()
                    .map(|access| access.as_ref().map(|access| access.is_closed))
                    .collect::<Vec<_>>(),
            )),
        };
        produced.push(column);
    }
    let mut all_columns = batch.columns().to_vec();
    all_columns.extend(produced);
    let righe = batch.num_rows();
    plenora_core::batch_with_rows(kernel.output_contract.schema.clone(), all_columns, righe)
        .map_err(|error| step_error(kernel, error))
}

/// `geo.line_locate_point` (streaming 1:1 "add column"): frazione [0,1] del
/// punto di riferimento sulla linea; null per geometrie null o non-linee.
pub(super) fn geo_line_locate_point_batch(
    kernel: &PreparedKernel,
    batch: &RecordBatch,
    point: &geo::Point<f64>,
    output_column: &str,
) -> Result<RecordBatch> {
    let _ = output_column; // Il nome e' gia' nel contratto di output usato per lo schema.
    let cells = kernel_geometry_cells(kernel, batch)?;
    let mut values: Vec<Option<f64>> = Vec::with_capacity(batch.num_rows());
    for row in 0..batch.num_rows() {
        if cells.is_null(row) {
            values.push(None);
            continue;
        }
        let geometry =
            decode_geometry_cell(cells.value(row)).map_err(|error| step_error(kernel, error))?;
        let fraction = plenora_kernels_geo::extensions::line_locate_point(&geometry, point)
            .map_err(|error| step_error(kernel, PlenoraError::InvalidPlan(error.to_string())))?;
        values.push(fraction);
    }
    append_output_column(
        kernel,
        batch,
        std::sync::Arc::new(Float64Array::from(values)),
    )
}

/// `geo.subdivide` (streaming OneToMany): espansione 1:N per batch con
/// `__parent_index` di lineage (come `explode`); riga con geometria null
/// produce una riga con geometria null.
pub(super) fn geo_subdivide_batch(
    kernel: &PreparedKernel,
    batch: &RecordBatch,
    max_vertices: usize,
) -> Result<RecordBatch> {
    let geometry_index = kernel_geometry_index(kernel)?;
    let cells = kernel_geometry_cells(kernel, batch)?;
    let mut parent_index: Vec<u64> = Vec::new();
    let mut parts: Vec<Option<Vec<u8>>> = Vec::new();
    for row in 0..batch.num_rows() {
        let row_index = row as u64;
        if cells.is_null(row) {
            parent_index.push(row_index);
            parts.push(None);
            continue;
        }
        let pieces =
            plenora_kernels_geo::extensions2::subdivide_wkb(cells.value(row), max_vertices)
                .map_err(|error| step_error(kernel, error))?;
        for piece in pieces {
            parent_index.push(row_index);
            parts.push(Some(piece));
        }
    }
    let indices = UInt64Array::from(parent_index);
    let mut columns: Vec<ArrayRef> = Vec::with_capacity(batch.num_columns() + 1);
    for (index, column) in batch.columns().iter().enumerate() {
        if index == geometry_index {
            columns.push(std::sync::Arc::new(BinaryArray::from(
                parts.iter().map(|part| part.as_deref()).collect::<Vec<_>>(),
            )));
        } else {
            columns.push(
                take(column.as_ref(), &indices, None)
                    .map_err(|error| step_error(kernel, PlenoraError::from(error)))?,
            );
        }
    }
    let righe = columns
        .first()
        .map_or(0, plenora_core::arrow::array::Array::len);
    columns.push(std::sync::Arc::new(indices));
    plenora_core::batch_with_rows(kernel.output_contract.schema.clone(), columns, righe)
        .map_err(|error| step_error(kernel, error))
}

/// `geo.snap` (streaming 1:1 in place): vertici agganciati al riferimento
/// entro tolleranza; schema invariato.
pub(super) fn geo_snap_batch(
    kernel: &PreparedKernel,
    batch: &RecordBatch,
    reference: &geo::Geometry<f64>,
    tolerance: f64,
) -> Result<RecordBatch> {
    let geometry_index = kernel_geometry_index(kernel)?;
    let cells = kernel_geometry_cells(kernel, batch)?;
    let snapped = plenora_kernels_geo::extensions2::snap_column(cells, reference, tolerance)
        .map_err(|error| step_error(kernel, error))?;
    let mut columns = batch.columns().to_vec();
    columns[geometry_index] = std::sync::Arc::new(BinaryArray::from(
        snapped
            .iter()
            .map(|cell| cell.as_deref())
            .collect::<Vec<_>>(),
    ));
    let righe = columns
        .first()
        .map_or(0, plenora_core::arrow::array::Array::len);
    plenora_core::batch_with_rows(kernel.output_contract.schema.clone(), columns, righe)
        .map_err(|error| step_error(kernel, error))
}

/// `geo.collect` (blocking, ManyToOne): raggruppamento canonico per chiavi
/// (ordine lessicografico della chiave testuale, come `table.aggregate`) e
/// collezione delle geometrie del gruppo in ordine di input.
pub(super) fn geo_collect_batch(
    kernel: &PreparedKernel,
    batch: &RecordBatch,
    group_by_indices: &[usize],
) -> Result<RecordBatch> {
    let cells = kernel_geometry_cells(kernel, batch)?;
    let geometries = plenora_kernels_geo::arrow_adapter::map_nullable(cells, |payload| {
        decode_geometry_cell(payload).map(Some)
    })
    .map_err(|error| step_error(kernel, error))?;
    let mut groups: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for row in 0..batch.num_rows() {
        let mut key = String::new();
        for &index in group_by_indices {
            let column = batch.column(index);
            key.push_str(&column.data_type().to_string());
            key.push('\u{1e}');
            match plenora_kernels_table::scalar_as_string(column.as_ref(), row)
                .map_err(|error| step_error(kernel, error))?
            {
                Some(value) => {
                    key.push('1');
                    key.push_str(&value.len().to_string());
                    key.push(':');
                    key.push_str(&value);
                }
                None => key.push('0'),
            }
            key.push('\u{1f}');
        }
        groups.entry(key).or_default().push(row);
    }
    let mut collected: Vec<Option<Vec<u8>>> = Vec::with_capacity(groups.len());
    let mut representatives: Vec<u64> = Vec::with_capacity(groups.len());
    for rows in groups.values() {
        let group: Vec<Option<geo::Geometry<f64>>> =
            rows.iter().map(|&row| geometries[row].clone()).collect();
        let geometry = plenora_kernels_geo::extensions::collect_geometries(&group)
            .map_err(|error| step_error(kernel, PlenoraError::InvalidPlan(error.to_string())))?;
        collected.push(match &geometry {
            Some(geometry) => Some(
                plenora_kernels_geo::arrow_adapter::encode_geometry(geometry)
                    .map_err(|error| step_error(kernel, error))?,
            ),
            None => None,
        });
        representatives.push(rows[0] as u64);
    }
    let indices = UInt64Array::from(representatives);
    let mut columns: Vec<ArrayRef> = Vec::with_capacity(group_by_indices.len() + 1);
    columns.push(std::sync::Arc::new(BinaryArray::from(
        collected
            .iter()
            .map(|cell| cell.as_deref())
            .collect::<Vec<_>>(),
    )));
    for &index in group_by_indices {
        columns.push(
            take(batch.column(index).as_ref(), &indices, None)
                .map_err(|error| step_error(kernel, PlenoraError::from(error)))?,
        );
    }
    let righe = columns
        .first()
        .map_or(0, plenora_core::arrow::array::Array::len);
    plenora_core::batch_with_rows(kernel.output_contract.schema.clone(), columns, righe)
        .map_err(|error| step_error(kernel, error))
}

/// `geo.generate_grid` (blocking, generativa): l'input funge da trigger; lo
/// schema di output (con o senza centroidi) decide le colonne prodotte.
pub(super) fn geo_generate_grid_batch(
    kernel: &PreparedKernel,
    extent: &plenora_kernels_geo::extensions2::GridExtent,
    cell_size: f64,
    shape: plenora_kernels_geo::extensions2::GridShape,
) -> Result<RecordBatch> {
    let rows = plenora_kernels_geo::extensions2::generate_grid_rows(extent, cell_size, shape)
        .map_err(|error| step_error(kernel, error))?;
    let include_centroid = kernel
        .output_contract
        .schema
        .field_with_name("centroid_x")
        .is_ok();
    let mut columns: Vec<ArrayRef> = Vec::with_capacity(5);
    columns.push(std::sync::Arc::new(BinaryArray::from(
        rows.iter()
            .map(|row| row.wkb.as_slice())
            .collect::<Vec<_>>(),
    )));
    columns.push(std::sync::Arc::new(UInt64Array::from_iter_values(
        rows.iter().map(|row| row.cell_i),
    )));
    columns.push(std::sync::Arc::new(UInt64Array::from_iter_values(
        rows.iter().map(|row| row.cell_j),
    )));
    if include_centroid {
        columns.push(std::sync::Arc::new(Float64Array::from_iter_values(
            rows.iter().map(|row| row.centroid_x),
        )));
        columns.push(std::sync::Arc::new(Float64Array::from_iter_values(
            rows.iter().map(|row| row.centroid_y),
        )));
    }
    let righe = columns
        .first()
        .map_or(0, plenora_core::arrow::array::Array::len);
    plenora_core::batch_with_rows(kernel.output_contract.schema.clone(), columns, righe)
        .map_err(|error| step_error(kernel, error))
}

/// `geo.coverage_validate` (blocking, WholeToMany): una riga per overlap.
pub(super) fn geo_coverage_validate_batch(
    kernel: &PreparedKernel,
    batch: &RecordBatch,
    tolerance: f64,
    max_issues: usize,
) -> Result<RecordBatch> {
    let cells = kernel_geometry_cells(kernel, batch)?;
    let rows =
        plenora_kernels_geo::extensions3::coverage_validate_rows(cells, tolerance, max_issues)
            .map_err(|error| step_error(kernel, error))?;
    let columns: Vec<ArrayRef> = vec![
        std::sync::Arc::new(StringArray::from(
            rows.iter().map(|row| row.issue_type).collect::<Vec<_>>(),
        )),
        std::sync::Arc::new(UInt64Array::from_iter_values(
            rows.iter().map(|row| row.index_a),
        )),
        std::sync::Arc::new(UInt64Array::from_iter_values(
            rows.iter().map(|row| row.index_b),
        )),
        std::sync::Arc::new(Float64Array::from_iter_values(
            rows.iter().map(|row| row.area),
        )),
        std::sync::Arc::new(BinaryArray::from(
            rows.iter()
                .map(|row| row.wkb.as_slice())
                .collect::<Vec<_>>(),
        )),
    ];
    let righe = columns
        .first()
        .map_or(0, plenora_core::arrow::array::Array::len);
    plenora_core::batch_with_rows(kernel.output_contract.schema.clone(), columns, righe)
        .map_err(|error| step_error(kernel, error))
}

/// `geo.shared_paths` (blocking, WholeToMany): una riga per coppia con
/// confine condiviso.
pub(super) fn geo_shared_paths_batch(
    kernel: &PreparedKernel,
    batch: &RecordBatch,
    tolerance: f64,
    min_length: f64,
) -> Result<RecordBatch> {
    let cells = kernel_geometry_cells(kernel, batch)?;
    let rows = plenora_kernels_geo::extensions3::shared_paths_rows(cells, tolerance, min_length)
        .map_err(|error| step_error(kernel, error))?;
    let columns: Vec<ArrayRef> = vec![
        std::sync::Arc::new(UInt64Array::from_iter_values(
            rows.iter().map(|row| row.index_a),
        )),
        std::sync::Arc::new(UInt64Array::from_iter_values(
            rows.iter().map(|row| row.index_b),
        )),
        std::sync::Arc::new(Float64Array::from_iter_values(
            rows.iter().map(|row| row.shared_length),
        )),
        std::sync::Arc::new(BinaryArray::from(
            rows.iter()
                .map(|row| row.wkb.as_slice())
                .collect::<Vec<_>>(),
        )),
    ];
    let righe = columns
        .first()
        .map_or(0, plenora_core::arrow::array::Array::len);
    plenora_core::batch_with_rows(kernel.output_contract.schema.clone(), columns, righe)
        .map_err(|error| step_error(kernel, error))
}

/// `geo.cluster_dbscan` (blocking, output allineato alle righe): etichetta
/// `UInt64` nullable per riga (noise → null).
pub(super) fn geo_cluster_dbscan_batch(
    kernel: &PreparedKernel,
    batch: &RecordBatch,
    eps: f64,
    min_points: usize,
    output_column: &str,
) -> Result<RecordBatch> {
    let _ = output_column; // Il nome e' gia' nel contratto di output usato per lo schema.
    let cells = kernel_geometry_cells(kernel, batch)?;
    let labels = plenora_kernels_geo::cluster::dbscan_column(cells, eps, min_points)
        .map_err(|error| step_error(kernel, error))?;
    append_output_column(
        kernel,
        batch,
        std::sync::Arc::new(UInt64Array::from(labels)),
    )
}

/// Il kernel di un segmento blocking unario e' spill-capable (architettura.md#memoria,
/// Fase 2B, spill generalizzato): `table.sort`/`distinct`/`aggregate` hanno la variante
/// `*_spilled` in kernels-table (cfr. `table_engine::unary_spill_capable`).
pub(super) fn spill_capable_unary(kernel: &PreparedKernel) -> bool {
    matches!(
        &kernel.config,
        PreparedConfig::Table(PreparedTableKernel::Unary(table_plan)) if table_engine::unary_spill_capable(table_plan)
    )
}

/// Segmento blocking unario: input materializzato (previsto dal piano, materializzazione minima),
/// concatenato ed eseguito una sola volta.
///
/// Confine architettura.md#memoria: i lease degli input sono trattenuti durante la
/// concatenazione (i buffer sorgente sono vivi), poi la materializzazione
/// concatenata riceve il suo lease — reservation completa prima di iniziare
/// (categoria "memoria stimabile"), acquisita PRIMA di rilasciare gli input
/// (mai sotto-conteggio al confine) — e l'output riceve a sua volta un
/// lease nuovo. Sequenza riassegnata con la regola documentata in
/// [`blocking_output_sequence`].
pub(super) fn run_blocking(
    plan: &Rc<ExecutionPlan>,
    segment_index: usize,
    state: &ExecState,
    batches: Vec<GovernedBatch>,
) -> Result<GovernedBatch> {
    // Come nel percorso streaming: heartbeat persistentemente fallito =
    // errore esplicito, mai silenzio (errori-e-limiti.md).
    state.verifica_heartbeat()?;
    let segment = &plan.segments()[segment_index];
    let kernel = segment.kernels.first().ok_or_else(|| {
        PlenoraError::Internal(
            "segmento blocking senza kernel: invariante del planner violata".into(),
        )
    })?;
    let rows_in = batches.iter().map(|g| g.batch.num_rows()).sum::<usize>() as u64;
    let bytes_in = batches
        .iter()
        .map(GovernedBatch::accounted_bytes)
        .sum::<u64>();
    let schema = kernel.input_contracts[0].schema.clone();
    let full = if batches.is_empty() {
        RecordBatch::new_empty(schema)
    } else {
        let unwrapped: Vec<RecordBatch> = batches.iter().map(|g| g.batch.clone()).collect();
        concat_batches(&schema, &unwrapped)
            .map_err(|error| step_error(kernel, PlenoraError::from(error)))?
    };
    // Il batch concatenato non ha un produttore a monte che ne abbia
    // verificato i byte: il tetto duro in byte per batch si applica anche qui (fail-closed).
    // I byte restituiti alimentano la reservation (hot path minimale: un solo conteggio).
    let full_bytes = check_batch_bytes(state, &full, &kernel.node_id)?;
    // architettura.md#memoria (Fase 2B, spill generalizzato): se il kernel spillera' — stessa soglia
    // deterministica valutata al dispatch tabellare (`should_spill_unary`
    // sui byte stimati dell'input), stessi limiti — l'intermedio
    // concatenato NON consuma quota governor: la memoria di lavoro
    // dell'operatore e' auto-limitata dallo spill su disco e la
    // reservation fallirebbe per costruzione (la soglia ha la stessa
    // grandezza del budget). Altrimenti reservation completa
    // dell'intermedio prima di rilasciare i lease degli input (architettura.md#memoria:
    // mai attesa con reservation parziale).
    let spill_path = match &kernel.config {
        PreparedConfig::Table(PreparedTableKernel::Unary(table_plan))
            if table_engine::unary_spill_capable(table_plan) =>
        {
            plenora_kernels_table::spill::should_spill_unary(&full, table_plan.limits())
        }
        _ => false,
    };
    let full_lease = if spill_path {
        None
    } else {
        Some(state.governor.reserve(full_bytes, &kernel.node_id)?)
    };
    drop(batches);
    // errori-e-limiti.md#cancellazione: a fine drenaggio, prima del kernel monolitico
    // (`BoundaryOnly`: check tra kernel/a fine kernel; `NonInterruptible`:
    // mai).
    state.check_cancellation(kernel)?;
    let start = Instant::now();
    let output = run_kernel(kernel, full, state)?;
    let elapsed = start.elapsed();
    // Lease dell'output acquisito prima di rilasciare l'intermedio.
    let output_lease = state
        .governor
        .reserve(output.get_array_memory_size() as u64, &kernel.node_id)?;
    drop(full_lease);
    let rows_out = output.num_rows() as u64;
    state.add_node_rows_out(&kernel.node_id, rows_out);
    check_expansion(state, kernel, rows_in)?;
    if segment.output_edge != plan.output_edge() {
        check_edge_batch(state, &kernel.node_id, &output)?;
    }
    let bytes_out = output_lease.bytes();
    record_kernel_metrics(
        state, segment, kernel, rows_in, rows_out, bytes_in, bytes_out, elapsed, true, true,
    );
    Ok(GovernedBatch::new(
        output,
        Some(output_lease),
        Some(blocking_output_sequence(kernel)),
    ))
}

/// Segmento blocking binario: left e right materializzati, concatenati ed
/// eseguiti una sola volta via `execute_binary`.
///
/// Confine architettura.md#memoria come [`run_blocking`], con reservation multiple in
/// ORDINE GLOBALE FISSO — left prima di right — completa prima di iniziare
/// (mai attesa con reservation parziale; in v1 fail-fast non c'e' attesa,
/// ma l'ordine e' gia' quello richiesto al runtime parallelo M3 per evitare
/// deadlock). Sequenza riassegnata con la regola documentata in
/// [`blocking_output_sequence`] (scansione seriale left-then-right).
// La lunghezza e' data dal guscio architettura.md#memoria completo (concat, reservation,
// metriche) piu' lo smistamento D14.2: sequenza lineare, non complessita'
// logica (stesso criterio di `pair_arrow`).
#[allow(clippy::too_many_lines)]
pub(super) fn run_binary_blocking(
    plan: &Rc<ExecutionPlan>,
    segment_index: usize,
    state: &ExecState,
    left_batches: Vec<GovernedBatch>,
    right_batches: Vec<GovernedBatch>,
) -> Result<GovernedBatch> {
    // Terzo percorso di esecuzione, e va guardato come gli altri due: un
    // segmento binario emette un solo batch, quindi non esiste
    // necessariamente un confine successivo dove un heartbeat fermo da
    // troppo tempo diventerebbe un errore. Senza questo controllo l'output
    // poteva essere pubblicato con il lock ormai stantio.
    state.verifica_heartbeat()?;
    let segment = &plan.segments()[segment_index];
    let kernel = segment.kernels.first().ok_or_else(|| {
        PlenoraError::Internal(
            "segmento binario senza kernel: invariante del planner violata".into(),
        )
    })?;
    // Smistamento architettura.md#geometrie D14.2 sul `PreparedConfig`: il ramo geo ha il
    // percorso dedicato [`run_geo_binary_blocking`] (stesso guscio, cuore
    // decode → kernel validated → output v4); il ramo tabellare prosegue
    // qui sotto, invariato.
    if let PreparedConfig::Geo(PreparedGeoKernel::Binary(geo_plan)) = &kernel.config {
        return run_geo_binary_blocking(
            plan,
            segment_index,
            state,
            geo_plan,
            left_batches,
            right_batches,
        );
    }
    let PreparedConfig::Table(PreparedTableKernel::Binary(binary_plan)) = &kernel.config else {
        return Err(PlenoraError::Internal(format!(
            "nodo `{}`: config non binaria in un segmento BinaryBlocking",
            kernel.node_id
        )));
    };
    let left_rows = left_batches
        .iter()
        .map(|g| g.batch.num_rows())
        .sum::<usize>() as u64;
    let right_rows = right_batches
        .iter()
        .map(|g| g.batch.num_rows())
        .sum::<usize>() as u64;
    let bytes_in = left_batches
        .iter()
        .chain(right_batches.iter())
        .map(GovernedBatch::accounted_bytes)
        .sum::<u64>();
    let batches_in = (left_batches.len() + right_batches.len()) as u64;
    let left_schema = kernel.input_contracts[0].schema.clone();
    let right_schema = kernel.input_contracts[1].schema.clone();
    let left = if left_batches.is_empty() {
        RecordBatch::new_empty(left_schema)
    } else {
        let unwrapped: Vec<RecordBatch> = left_batches.iter().map(|g| g.batch.clone()).collect();
        concat_batches(&left_schema, &unwrapped)
            .map_err(|error| step_error(kernel, PlenoraError::from(error)))?
    };
    let right = if right_batches.is_empty() {
        RecordBatch::new_empty(right_schema)
    } else {
        let unwrapped: Vec<RecordBatch> = right_batches.iter().map(|g| g.batch.clone()).collect();
        concat_batches(&right_schema, &unwrapped)
            .map_err(|error| step_error(kernel, PlenoraError::from(error)))?
    };
    // Come per il blocking unario: tetto duro in byte per batch sui batch concatenati; i
    // byte restituiti alimentano le reservation (hot path minimale: un solo conteggio).
    let left_bytes = check_batch_bytes(state, &left, &kernel.node_id)?;
    let right_bytes = check_batch_bytes(state, &right, &kernel.node_id)?;
    // Reservation complete degli intermedi in ordine globale fisso (left,
    // poi right), poi rilascio dei lease degli input.
    let left_lease = state.governor.reserve(left_bytes, &kernel.node_id)?;
    let right_lease = state.governor.reserve(right_bytes, &kernel.node_id)?;
    drop(left_batches);
    drop(right_batches);
    // errori-e-limiti.md#cancellazione: a fine drenaggio, prima del kernel binario monolitico
    // (come `run_blocking`).
    state.check_cancellation(kernel)?;
    let start = Instant::now();
    // Confine di panic policy (errori-e-limiti.md#panic-policy) come
    // `run_kernel`: panic del kernel binario convertito
    // in errore `Execution` attribuito al nodo, mai publish dopo panic.
    let output = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        #[cfg(test)]
        inject_test_panic(&kernel.node_id);
        table_engine::execute_binary(&left, &right, binary_plan)
    }))
    .unwrap_or_else(|payload| Err(panic_step_error(kernel, &*payload)))
    .map_err(|error| step_error(kernel, error))?;
    let elapsed = start.elapsed();
    let output_lease = state
        .governor
        .reserve(output.get_array_memory_size() as u64, &kernel.node_id)?;
    drop(left_lease);
    drop(right_lease);
    let rows_out = output.num_rows() as u64;
    state.add_node_rows_out(&kernel.node_id, rows_out);
    // errori-e-limiti.md: per le operazioni binarie il runtime calcola tutte le metriche
    // di espansione e applica il vincolo vincolante dichiarato in catalogo.
    check_join_expansion(state, kernel, left_rows, right_rows, rows_out)?;
    if segment.output_edge != plan.output_edge() {
        check_edge_batch(state, &kernel.node_id, &output)?;
    }
    // Metriche per nodo: righe in = left + right, batch in = quelli reali
    // drenati dai due rami.
    // errori-e-limiti.md: heartbeat del TempStore al punto centrale (come
    // `record_kernel_metrics`, non riusata qui per i conteggi doppi input).
    state.heartbeat();
    let config = state.plan.metrics_config();
    let mut borrowed = state.metrics.borrow_mut();
    let metrics = &mut *borrowed;
    let saturated = &mut metrics.counters_saturated;
    // Metrica obbligatoria errori-e-limiti.md (come in `record_kernel_metrics`). Le righe
    // in ingresso di un nodo binario sono la somma dei due lati: con limiti
    // configurabili fino a `u64::MAX` la somma non e' rappresentabile per
    // costruzione, e non e' una metrica che possa abortire l'esecuzione.
    let rows_in = sum_rows(left_rows, right_rows, saturated);
    accumulate(&mut metrics.total_rows_processed, rows_in, saturated);
    if config.per_node {
        if let Some(node) = metrics.nodes.get_mut(&kernel.node_id) {
            accumulate(&mut node.rows_in, rows_in, saturated);
            accumulate(&mut node.rows_out, rows_out, saturated);
            accumulate(&mut node.batches_in, batches_in, saturated);
            accumulate(&mut node.batches_out, 1, saturated);
            accumulate(&mut node.bytes_in, bytes_in, saturated);
            accumulate(&mut node.bytes_out, output_lease.bytes(), saturated);
            accumulate_time(&mut node.wall_time, elapsed, saturated);
        }
    }
    if config.per_segment {
        if let Some(seg) = metrics.segments.get_mut(&segment.id) {
            accumulate(&mut seg.rows_in, rows_in, saturated);
            accumulate(&mut seg.rows_out, rows_out, saturated);
            accumulate(&mut seg.batches_in, batches_in, saturated);
            accumulate(&mut seg.batches_out, 1, saturated);
            accumulate_time(&mut seg.wall_time, elapsed, saturated);
        }
    }
    Ok(GovernedBatch::new(
        output,
        Some(output_lease),
        Some(blocking_output_sequence(kernel)),
    ))
}

/// Ramo geo di [`run_binary_blocking`] (architettura.md#geometrie, D14.2): stesso
/// guscio del ramo tabellare — drenaggio dei due rami a monte,
/// `concat_batches`, tetti in byte per batch, reservation in ordine globale fisso
/// left→right, cancellazione post-drenaggio, `catch_unwind`,
/// `check_join_expansion`, `blocking_output_sequence`, metriche — con il
/// cuore sostituito da decode totale D14.3 → kernel `*_validated` → output
/// secondo il contratto v4 ([`execute_geo_binary`]).
///
/// Cancellazione `BoundaryOnly` (catalogo, D14.5.5): i confini sono quelli
/// esistenti (batch in drenaggio + post-drenaggio pre-kernel), nessun check
/// dentro il kernel — comportamento voluto, non lacuna.
///
/// Contabilita' D14.4: per ciascun lato, nell'ordine globale fisso
/// left→right — reservation dei byte Arrow, preflight della forma
/// decodificata ([`preflight_decoded_bytes`]), reservation della forma
/// decodificata, decode. Rifiuto fail-closed PRIMA dell'allocazione: una
/// reservation rifiutata ferma il nodo senza partial state (i lease sono
/// RAII). Il lease Arrow right e' rilasciato prima del kernel (il left
/// resta per take/passthrough); i lease decodificati dopo il lease
/// dell'output.
///
/// Errori D14.5: decode → fase `Read` con side/riga strutturati,
/// kernel e costruzione output → fase `Write` (carrier
/// [`GeoBinaryStepError`], conversione [`geo_binary_step_error`]); il primo
/// errore e' in ordine (side, riga) per costruzione della sequenza.
// La lunghezza e' data dal guscio architettura.md#memoria completo (concat, reservation,
// metriche) piu' la sequenza D14.4 per lato: sequenza lineare, non
// complessita' logica (stesso criterio di `pair_arrow`).
#[allow(clippy::too_many_lines)]
pub(super) fn run_geo_binary_blocking(
    plan: &Rc<ExecutionPlan>,
    segment_index: usize,
    state: &ExecState,
    geo_plan: &GeoBinaryPlan,
    left_batches: Vec<GovernedBatch>,
    right_batches: Vec<GovernedBatch>,
) -> Result<GovernedBatch> {
    let segment = &plan.segments()[segment_index];
    let kernel = segment.kernels.first().ok_or_else(|| {
        PlenoraError::Internal(
            "segmento binario senza kernel: invariante del planner violata".into(),
        )
    })?;
    let left_rows = left_batches
        .iter()
        .map(|g| g.batch.num_rows())
        .sum::<usize>() as u64;
    let right_rows = right_batches
        .iter()
        .map(|g| g.batch.num_rows())
        .sum::<usize>() as u64;
    let bytes_in = left_batches
        .iter()
        .chain(right_batches.iter())
        .map(GovernedBatch::accounted_bytes)
        .sum::<u64>();
    let batches_in = (left_batches.len() + right_batches.len()) as u64;
    let left_schema = kernel.input_contracts[0].schema.clone();
    let right_schema = kernel.input_contracts[1].schema.clone();
    let left = if left_batches.is_empty() {
        RecordBatch::new_empty(left_schema)
    } else {
        let unwrapped: Vec<RecordBatch> = left_batches.iter().map(|g| g.batch.clone()).collect();
        concat_batches(&left_schema, &unwrapped)
            .map_err(|error| step_error(kernel, PlenoraError::from(error)))?
    };
    let right = if right_batches.is_empty() {
        RecordBatch::new_empty(right_schema)
    } else {
        let unwrapped: Vec<RecordBatch> = right_batches.iter().map(|g| g.batch.clone()).collect();
        concat_batches(&right_schema, &unwrapped)
            .map_err(|error| step_error(kernel, PlenoraError::from(error)))?
    };
    // Come nel ramo tabellare: tetto duro in byte per batch sui batch concatenati; i byte
    // restituiti alimentano le reservation (hot path minimale: un solo conteggio).
    let left_bytes = check_batch_bytes(state, &left, &kernel.node_id)?;
    let right_bytes = check_batch_bytes(state, &right, &kernel.node_id)?;
    // I batch d'ingresso non servono piu' (il concat possiede i nuovi
    // buffer): i loro lease tornano al governor prima delle reservation.
    drop(left_batches);
    drop(right_batches);
    // D14.4: per ciascun lato, in ordine globale fisso left→right —
    // reservation Arrow, preflight della forma decodificata, reservation
    // decodificata, decode. Il decode left completo precede qualunque
    // contabilita' del lato right: il primo errore e' in ordine (side,
    // riga) per costruzione (D14.5.3).
    let left_lease = state.governor.reserve(left_bytes, &kernel.node_id)?;
    let left_decoded_bytes = preflight_decoded_bytes(
        &left.schema(),
        std::slice::from_ref(&left),
        geo_plan.left_geometry_index,
    );
    let left_decoded_lease = state
        .governor
        .reserve(left_decoded_bytes, &kernel.node_id)?;
    let left_geometries = decode_geometry_batches(
        &left.schema(),
        std::slice::from_ref(&left),
        geo_plan.left_geometry_index,
    )
    .map_err(|error| {
        geo_binary_step_error(
            state,
            kernel,
            GeoBinaryStepError {
                phase: ErrorPhase::Read,
                side: Some(GeoBinarySide::Left),
                row_index: error.row_index,
                source: PlenoraError::InvalidPlan(error.source.to_string()),
            },
        )
    })?;
    let right_lease = state.governor.reserve(right_bytes, &kernel.node_id)?;
    let right_decoded_bytes = preflight_decoded_bytes(
        &right.schema(),
        std::slice::from_ref(&right),
        geo_plan.right_geometry_index,
    );
    let right_decoded_lease = state
        .governor
        .reserve(right_decoded_bytes, &kernel.node_id)?;
    let right_geometries = decode_geometry_batches(
        &right.schema(),
        std::slice::from_ref(&right),
        geo_plan.right_geometry_index,
    )
    .map_err(|error| {
        geo_binary_step_error(
            state,
            kernel,
            GeoBinaryStepError {
                phase: ErrorPhase::Read,
                side: Some(GeoBinarySide::Right),
                row_index: error.row_index,
                source: PlenoraError::InvalidPlan(error.source.to_string()),
            },
        )
    })?;
    // D14.4: lease Arrow RIGHT rilasciato prima del kernel (il left resta
    // per take/passthrough); il batch right non serve piu'.
    drop(right_lease);
    drop(right);
    // errori-e-limiti.md / D14.5.5: a fine drenaggio, prima del kernel binario
    // monolitico (come `run_blocking` e il ramo tabellare).
    state.check_cancellation(kernel)?;
    let start = Instant::now();
    // Confine di panic policy (errori-e-limiti.md#panic-policy, D14.5.6)
    // come il ramo tabellare: panic del kernel
    // convertito in errore `Execution` attribuito al nodo, mai publish
    // dopo panic; hook `PANIC_AT_NODES` esteso al ramo.
    let output = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        #[cfg(test)]
        inject_test_panic(&kernel.node_id);
        execute_geo_binary(kernel, geo_plan, &left, &left_geometries, &right_geometries)
    }))
    .unwrap_or_else(|payload| Err(panic_step_error(kernel, &*payload)))
    .map_err(|source| {
        geo_binary_step_error(
            state,
            kernel,
            GeoBinaryStepError {
                phase: ErrorPhase::Write,
                side: None,
                row_index: None,
                source,
            },
        )
    })?;
    let elapsed = start.elapsed();
    let output_lease = state
        .governor
        .reserve(output.get_array_memory_size() as u64, &kernel.node_id)?;
    // D14.4: lease decodificati rilasciati dopo il lease dell'output; anche
    // il lease Arrow left (take/passthrough completati nel kernel).
    drop(left_decoded_lease);
    drop(right_decoded_lease);
    drop(left_lease);
    let rows_out = output.num_rows() as u64;
    state.add_node_rows_out(&kernel.node_id, rows_out);
    // errori-e-limiti.md: il vincolo relativo vincolante di catalogo si applica come nel
    // ramo tabellare; il tetto assoluto D14.6 e' gia' stato fatto rispettare
    // dal kernel (`max_pairs`/`max_results` risolti in prepare dai limiti
    // effettivi del piano, prima della materializzazione completa).
    check_join_expansion(state, kernel, left_rows, right_rows, rows_out)?;
    if segment.output_edge != plan.output_edge() {
        check_edge_batch(state, &kernel.node_id, &output)?;
    }
    // Metriche per nodo: righe in = left + right, batch in = quelli reali
    // drenati dai due rami (stesso blocco del ramo tabellare).
    // errori-e-limiti.md: heartbeat del TempStore al punto centrale.
    state.heartbeat();
    let config = state.plan.metrics_config();
    let mut borrowed = state.metrics.borrow_mut();
    let metrics = &mut *borrowed;
    let saturated = &mut metrics.counters_saturated;
    // Metrica obbligatoria errori-e-limiti.md (come in `record_kernel_metrics`). Le righe
    // in ingresso di un nodo binario sono la somma dei due lati: con limiti
    // configurabili fino a `u64::MAX` la somma non e' rappresentabile per
    // costruzione, e non e' una metrica che possa abortire l'esecuzione.
    let rows_in = sum_rows(left_rows, right_rows, saturated);
    accumulate(&mut metrics.total_rows_processed, rows_in, saturated);
    if config.per_node {
        if let Some(node) = metrics.nodes.get_mut(&kernel.node_id) {
            accumulate(&mut node.rows_in, rows_in, saturated);
            accumulate(&mut node.rows_out, rows_out, saturated);
            accumulate(&mut node.batches_in, batches_in, saturated);
            accumulate(&mut node.batches_out, 1, saturated);
            accumulate(&mut node.bytes_in, bytes_in, saturated);
            accumulate(&mut node.bytes_out, output_lease.bytes(), saturated);
            accumulate_time(&mut node.wall_time, elapsed, saturated);
        }
    }
    if config.per_segment {
        if let Some(seg) = metrics.segments.get_mut(&segment.id) {
            accumulate(&mut seg.rows_in, rows_in, saturated);
            accumulate(&mut seg.rows_out, rows_out, saturated);
            accumulate(&mut seg.batches_in, batches_in, saturated);
            accumulate(&mut seg.batches_out, 1, saturated);
            accumulate_time(&mut seg.wall_time, elapsed, saturated);
        }
    }
    Ok(GovernedBatch::new(
        output,
        Some(output_lease),
        Some(blocking_output_sequence(kernel)),
    ))
}

/// Cuore del ramo geo (D14.2): kernel `*_validated` (architettura.md#geometrie:
/// precondizione soddisfatta per costruzione dal decode totale D14.3,
/// eseguito dal chiamante) e costruzione dell'output secondo il contratto
/// v4:
///
/// - `sjoin`: `take` delle colonne left sugli indici di coppia + colonna
///   `right_index` (non-null per inner join);
/// - `nearest`: come sjoin + colonna `distance` (una riga per match, gia'
///   filtrato su `max_distance` dal kernel);
/// - `within`/`count_points_in_polygons`: left passthrough + colonna
///   flag/conteggio allineata alle righe left (null dove la geometria left
///   e' null).
///
/// Lo schema di output e' quello del contratto inferito dal planner
/// (fonte unica di verita', configurazioni preparate) — identico per costruzione allo schema del
/// contratto analyze v4, verificato dai test di identita'.
///
/// Gli errori propagano come sorgente grezza: fase (`Write`), side e riga
/// sono aggiunti dal chiamante nel carrier [`GeoBinaryStepError`] (D14.5.2).
// La lunghezza e' data dalla sequenza lineare dei quattro casi del
// perimetro della fusione (kernel e costruzione output per op), non da
// complessita'
// logica (stesso criterio di `pair_arrow`).
#[allow(clippy::too_many_lines)]
pub(super) fn execute_geo_binary(
    kernel: &PreparedKernel,
    geo_plan: &GeoBinaryPlan,
    left: &RecordBatch,
    left_geometries: &[Option<geo::Geometry<f64>>],
    right_geometries: &[Option<geo::Geometry<f64>>],
) -> Result<RecordBatch> {
    match geo_plan.operation {
        PairOperation::SJoin => {
            let pairs = spatial_join_nullable_validated(
                left_geometries,
                right_geometries,
                geo_plan.predicate.ok_or_else(|| {
                    PlenoraError::Internal(
                        "sjoin senza predicato: invariante di prepare violata".into(),
                    )
                })?,
                geo_plan.max_pairs,
            )
            .map_err(|error| PlenoraError::InvalidPlan(error.to_string()))?;
            let left_indices = UInt64Array::from_iter_values(pairs.iter().map(|pair| pair.left));
            let mut columns: Vec<ArrayRef> = Vec::with_capacity(left.num_columns() + 1);
            for column in left.columns() {
                columns
                    .push(take(column.as_ref(), &left_indices, None).map_err(PlenoraError::from)?);
            }
            columns.push(std::sync::Arc::new(UInt64Array::from_iter_values(
                pairs.iter().map(|pair| pair.right),
            )));
            let righe = columns
                .first()
                .map_or(0, plenora_core::arrow::array::Array::len);
            plenora_core::batch_with_rows(kernel.output_contract.schema.clone(), columns, righe)
        }
        PairOperation::Nearest => {
            let matches = nearest_matches_validated(
                left_geometries,
                right_geometries,
                geo_plan.max_distance,
                geo_plan.max_comparisons,
                geo_plan.max_results,
            )
            .map_err(|error| PlenoraError::InvalidPlan(error.to_string()))?;
            let left_indices = UInt64Array::from_iter_values(matches.iter().map(|m| m.left));
            let mut columns: Vec<ArrayRef> = Vec::with_capacity(left.num_columns() + 2);
            for column in left.columns() {
                columns
                    .push(take(column.as_ref(), &left_indices, None).map_err(PlenoraError::from)?);
            }
            columns.push(std::sync::Arc::new(UInt64Array::from_iter_values(
                matches.iter().map(|m| m.right),
            )));
            columns.push(std::sync::Arc::new(Float64Array::from_iter_values(
                matches.iter().map(|m| m.distance),
            )));
            let righe = columns
                .first()
                .map_or(0, plenora_core::arrow::array::Array::len);
            plenora_core::batch_with_rows(kernel.output_contract.schema.clone(), columns, righe)
        }
        PairOperation::Within => {
            let indexes =
                within_indexes_validated(left_geometries, right_geometries, geo_plan.max_pairs)
                    .map_err(|error| PlenoraError::InvalidPlan(error.to_string()))?;
            let matched: std::collections::HashSet<u64> = indexes.into_iter().collect();
            let flags: Vec<Option<bool>> = left_geometries
                .iter()
                .enumerate()
                .map(|(index, geometry)| {
                    geometry.as_ref().map(|_| matched.contains(&(index as u64)))
                })
                .collect();
            let mut columns = left.columns().to_vec();
            columns.push(std::sync::Arc::new(BooleanArray::from(flags)));
            let righe = columns
                .first()
                .map_or(0, plenora_core::arrow::array::Array::len);
            plenora_core::batch_with_rows(kernel.output_contract.schema.clone(), columns, righe)
        }
        PairOperation::CountPointsInPolygons => {
            // Contratto: left = poligoni (output allineato), right = punti.
            let counts = count_points_in_polygons_validated(
                left_geometries,
                right_geometries,
                geo_plan.max_pairs,
            )
            .map_err(|error| PlenoraError::InvalidPlan(error.to_string()))?;
            let values: Vec<Option<u64>> = counts
                .iter()
                .enumerate()
                .map(|(index, count)| left_geometries[index].as_ref().map(|_| *count))
                .collect();
            let mut columns = left.columns().to_vec();
            columns.push(std::sync::Arc::new(UInt64Array::from(values)));
            let righe = columns
                .first()
                .map_or(0, plenora_core::arrow::array::Array::len);
            plenora_core::batch_with_rows(kernel.output_contract.schema.clone(), columns, righe)
        }
        _ => Err(PlenoraError::Internal(
            "op binaria geo fuori perimetro M1: invariante di prepare violata".into(),
        )),
    }
}
