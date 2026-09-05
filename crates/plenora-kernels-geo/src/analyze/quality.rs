//! Qualita' e copertura (v1.3): inferenza per `coverage_validate`,
//! `shared_paths` e `cluster_dbscan` e la forma di risultato condivisa
//! `WholeToMany` delle op di copertura.

use std::collections::HashMap;
use std::sync::Arc;

use plenora_core::arrow::{DataType, Field, Schema};
use plenora_core::contract::{
    ContractProperties, DataContract, FieldAllocator, GeometryColumnContract, GeometryDimensions,
};
use plenora_core::{PlenoraError, Result};
use serde_json::Value;

use crate::arrow_adapter::DEFAULT_GEOMETRY_COLUMN;

use super::config::{ClusterDbscanConfig, CoverageValidateConfig, SharedPathsConfig};
use super::helpers::{
    ensure_non_negative, ensure_positive, invalid_param, new_geometry_field, output_name,
    parse_config,
};
use super::producers::analyze_add_column;
use super::{
    CLUSTER_ID_COLUMN, INDEX_A_COLUMN, INDEX_B_COLUMN, ISSUE_AREA_COLUMN, ISSUE_TYPE_COLUMN,
    SHARED_LENGTH_COLUMN,
};

/// Costruisce il contratto `WholeToMany` delle op di copertura v1.3: schema
/// nuovo con le colonne diagnostiche non-null elencate piu' la geometria WKB
/// non-null (nuovo `FieldId`, CRS dell'input); proprieta' azzerate.
///
/// I metadati dello SCHEMA di input sono conservati (R2.4): riguardano il
/// dataset, non le colonne attributo soppresse.
pub(in crate::analyze) fn analyze_coverage_rows(
    geometry: &GeometryColumnContract,
    schema_metadata: &HashMap<String, String>,
    columns: &[(&str, DataType)],
    fields_allocator: &mut FieldAllocator,
) -> Result<DataContract> {
    // Invariante: le op di copertura dichiarano un `CrsRequirement` e il gate
    // R4.6.3 di `dispatch` le ferma prima di arrivare qui se il CRS e'
    // `Missing` — un `None` sarebbe una violazione della catena di analyze,
    // mai uno stato da interpretare.
    let crs = geometry.crs.as_resolved().ok_or_else(|| {
        PlenoraError::Internal(
            "op di copertura senza CRS risolto: gate del requisito bypassato".to_owned(),
        )
    })?;
    let mut fields: Vec<Field> = columns
        .iter()
        .map(|(name, data_type)| Field::new(*name, data_type.clone(), false))
        .collect();
    fields.push(new_geometry_field(
        DEFAULT_GEOMETRY_COLUMN,
        crs,
        GeometryDimensions::Xy,
        // Kernel elaborante: l'output e' ricodificato WKB ISO XY — nessun
        // encoding dichiarato (chiave omessa, mai ereditata dall'input).
        None,
        false,
    )?);
    let field_id = fields_allocator.alloc()?;
    let output_geometry = GeometryColumnContract {
        field_id,
        name: DEFAULT_GEOMETRY_COLUMN.to_owned(),
        crs: geometry.crs.clone(),
        // Kernel elaborante: l'input e' gated a `Xy` in analisi
        // (`require_xy_dimensions`) e l'output e' ricodificato XY — dichiara
        // Xy, mai la dimensionalita' dell'input copiata silenziosamente.
        dimensions: GeometryDimensions::Xy,
        encoding: None,
        nullable: false,
        types: GeometryColumnContract::undeclared_types(),
    };
    DataContract::new(
        Arc::new(Schema::new_with_metadata(fields, schema_metadata.clone())),
        vec![output_geometry],
        Some(field_id),
        ContractProperties::default(),
    )
}

/// `coverage_validate` (v1.3): `tolerance` finita non negativa, `max_issues`
/// maggiore di zero; una riga per overlap.
pub(in crate::analyze) fn analyze_coverage_validate(
    op: &str,
    input: &DataContract,
    geometry: &GeometryColumnContract,
    config: &Value,
    fields_allocator: &mut FieldAllocator,
) -> Result<DataContract> {
    let parsed: CoverageValidateConfig = parse_config(op, config)?;
    if let Some(tolerance) = parsed.tolerance {
        ensure_non_negative(op, "tolerance", tolerance)?;
    }
    if let Some(max_issues) = parsed.max_issues {
        if max_issues == 0 {
            return Err(invalid_param(
                op,
                "max_issues",
                "deve essere maggiore di zero",
            ));
        }
    }
    analyze_coverage_rows(
        geometry,
        input.schema.metadata(),
        &[
            (ISSUE_TYPE_COLUMN, DataType::Utf8),
            (INDEX_A_COLUMN, DataType::UInt64),
            (INDEX_B_COLUMN, DataType::UInt64),
            (ISSUE_AREA_COLUMN, DataType::Float64),
        ],
        fields_allocator,
    )
}

/// `shared_paths` (v1.3): `tolerance` e `min_length` finite non negative; una
/// riga per coppia con confine condiviso.
pub(in crate::analyze) fn analyze_shared_paths(
    op: &str,
    input: &DataContract,
    geometry: &GeometryColumnContract,
    config: &Value,
    fields_allocator: &mut FieldAllocator,
) -> Result<DataContract> {
    let parsed: SharedPathsConfig = parse_config(op, config)?;
    if let Some(tolerance) = parsed.tolerance {
        ensure_non_negative(op, "tolerance", tolerance)?;
    }
    if let Some(min_length) = parsed.min_length {
        ensure_non_negative(op, "min_length", min_length)?;
    }
    analyze_coverage_rows(
        geometry,
        input.schema.metadata(),
        &[
            (INDEX_A_COLUMN, DataType::UInt64),
            (INDEX_B_COLUMN, DataType::UInt64),
            (SHARED_LENGTH_COLUMN, DataType::Float64),
        ],
        fields_allocator,
    )
}

/// `cluster_dbscan` (v1.3): `eps` finito e maggiore di zero, `min_points >=
/// 1`; aggiunge la colonna etichetta `UInt64` nullable (noise → null). Output
/// allineato alle righe: le proprieta' dell'input sono preservate.
pub(in crate::analyze) fn analyze_cluster_dbscan(
    op: &str,
    input: &DataContract,
    config: &Value,
) -> Result<DataContract> {
    let parsed: ClusterDbscanConfig = parse_config(op, config)?;
    ensure_positive(op, "eps", parsed.eps)?;
    if parsed.min_points < 1 {
        return Err(invalid_param(op, "min_points", "deve essere almeno 1"));
    }
    let name = output_name(op, parsed.output_column.as_deref(), CLUSTER_ID_COLUMN)?;
    analyze_add_column(op, input, name, DataType::UInt64)
}
