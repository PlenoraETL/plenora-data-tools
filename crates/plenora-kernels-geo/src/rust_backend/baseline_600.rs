// Snapshot letterale v6.0.0; blob 123653e6f869852498eb79a3bff86a27210bc6fd.
// Delega alla snapshot del kernel, mantenendo gli helper del modulo padre.
use super::polygonize::baseline_600 as polygonize;
use super::*;

#[allow(clippy::too_many_arguments)]
pub fn polygonize_linework(
    linework: &Geometry<f64>,
    node_input: bool,
    require_complete: bool,
    max_input_coordinates: u64,
    max_noding_work: u64,
    max_output_geometries: u64,
    max_output_coordinates: u64,
    precision: Precision,
) -> Result<PolygonizeResult, RustBackendError> {
    let limits = PolygonizeLimits {
        max_input_coordinates,
        max_noding_work,
        max_output_geometries,
        max_output_coordinates,
    };
    if !limits.is_fully_bounded() {
        return Err(RustBackendError::UnboundedLimits);
    }
    ensure_linework(linework, "polygonize")?;
    checked_input(linework, max_input_coordinates)?;
    let result = protetto(
        || {
            polygonize::polygonize_linework_rust_bounded(
                linework,
                PolygonizeOptions {
                    node_input,
                    require_complete: false,
                    limits,
                    precision: precision.value(),
                },
            )
        },
        |error| RustBackendError::from_polygonize(&error, "polygonize"),
    )?;
    let result = PolygonizeResult::from(result);
    if require_complete && result.residual_count() != 0 {
        return Err(RustBackendError::IncompletePolygonize {
            cuts: result.cut_edges.len(),
            dangles: result.dangles.len(),
            invalid_rings: result.invalid_ring_lines.len(),
        });
    }
    Ok(result)
}
