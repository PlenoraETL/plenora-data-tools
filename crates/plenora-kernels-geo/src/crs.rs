//! Contratto CRS per i kernel geografici.
//!
//! Il contratto backend-indipendente (`ResolvedCrs`, `CrsKind`, `CrsError`,
//! `required_definition`, `validate_requirement`, dominio lon/lat) vive in
//! `plenora_core::crs` ed e' qui riesportato. Questo modulo aggiunge:
//! - il wrapper tipizzato su `geo::Geometry` per la validazione di dominio.
//!
//! Non c'e' backend PROJ: una definizione CRS che non coincide con quella del
//! piano non si risolve e fallisce chiusa (`CrsError::BackendUnavailable`).

use geo::{CoordsIter, Geometry};
pub use plenora_core::crs::{
    required_definition, validate_requirement, CrsError, CrsKind, ResolvedCrs,
    MAX_CRS_DEFINITION_BYTES,
};

/// Validates normalized GIS axis order (x=longitude, y=latitude).
///
/// Call this for every geographic input after WKB decoding and before any
/// kernel work. Wrapper tipizzato: la logica e' in
/// [`plenora_core::crs::validate_geometry_domain`].
///
/// # Errors
///
/// Come [`plenora_core::crs::validate_geometry_domain`]:
/// [`CrsError::CoordinateOutOfDomain`] alla prima coordinata non finita o
/// fuori dal dominio longitude/latitude di un CRS geografico; per un CRS
/// proiettato non fallisce mai.
pub fn validate_geometry_domain(
    geometry: &Geometry<f64>,
    crs: &ResolvedCrs,
) -> Result<(), CrsError> {
    plenora_core::crs::validate_geometry_domain(
        geometry
            .coords_iter()
            .map(|coordinate| (coordinate.x, coordinate.y)),
        crs,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn projected() -> ResolvedCrs {
        ResolvedCrs::from_resolved_parts(
            "EPSG:3857".to_owned(),
            serde_json::json!({"type": "ProjectedCRS", "name": "WGS 84 / Pseudo-Mercator"}),
            CrsKind::Projected,
            Some(1.0),
        )
    }

    #[test]
    fn missing_backend_never_trusts_an_unverified_declaration() {
        assert!(matches!(
            plenora_core::crs::resolve_crs("EPSG:3857", "crs"),
            Err(CrsError::BackendUnavailable)
        ));
    }

    #[test]
    fn geometry_domain_wrapper_matches_coordinate_level_contract() {
        use geo::{line_string, Point};

        let geographic = ResolvedCrs::from_resolved_parts(
            "OGC:CRS84".to_owned(),
            serde_json::json!({"type": "GeographicCRS", "name": "WGS 84 (CRS84)"}),
            CrsKind::Geographic,
            None,
        );
        validate_geometry_domain(
            &Geometry::LineString(line_string![(x: -180.0, y: -90.0), (x: 180.0, y: 90.0)]),
            &geographic,
        )
        .unwrap();
        assert!(matches!(
            validate_geometry_domain(&Geometry::Point(Point::new(181.0, 0.0)), &geographic),
            Err(CrsError::CoordinateOutOfDomain { .. })
        ));
        assert!(matches!(
            validate_geometry_domain(&Geometry::Point(Point::new(0.0, 91.0)), &geographic),
            Err(CrsError::CoordinateOutOfDomain { .. })
        ));
        // Il dominio non si applica ai CRS proiettati.
        validate_geometry_domain(
            &Geometry::Point(Point::new(f64::INFINITY, f64::NEG_INFINITY)),
            &projected(),
        )
        .unwrap();
    }
}
