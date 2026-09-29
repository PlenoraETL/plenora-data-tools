//! Contratto CRS per i kernel geografici.
//!
//! Il contratto backend-indipendente (`ResolvedCrs`, `CrsKind`, `CrsError`,
//! `required_definition`, `validate_requirement`, dominio lon/lat) vive in
//! `plenora_core::crs` ed e' qui riesportato. Questo modulo aggiunge:
//! - il wrapper tipizzato su `geo::Geometry` per la validazione di dominio.
//!
//! Non c'e' backend PROJ: `plenora_core::crs::resolve_crs` risolve solo gli
//! identificatori della tabella dei CRS integrati (`EPSG:<codice>`, URN OGC,
//! `OGC:CRS84`); ogni altra definizione fallisce chiusa
//! (`CrsError::NotBuiltin` o `CrsError::BackendUnavailable`).

use geo::{CoordsIter, Geometry};
pub use plenora_core::crs::{
    required_definition, validate_requirement, CrsError, CrsKind, ResolvedCrs,
    MAX_CRS_DEFINITION_BYTES,
};

/// Verifica il dominio di validita' del CRS sulle coordinate di una geometria,
/// in ordine GIS normalizzato (x=longitudine o easting, y=latitudine o
/// northing).
///
/// Va chiamata su ogni input dopo la decodifica WKB e prima dei kernel.
/// Wrapper tipizzato: la logica e' in
/// [`plenora_core::crs::validate_geometry_domain`].
///
/// # Errors
///
/// Come [`plenora_core::crs::validate_geometry_domain`]:
/// [`CrsError::CoordinateOutOfDomain`] alla prima coordinata non finita o
/// fuori dal dominio del CRS (mondo lon/lat per i geografici, dominio di
/// validita' per i proiettati della tabella integrata).
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
            plenora_core::crs::resolve_crs(
                r#"PROJCS["WGS 84 / Pseudo-Mercator",GEOGCS["WGS 84"]]"#,
                "crs"
            ),
            Err(CrsError::BackendUnavailable)
        ));
        assert!(matches!(
            plenora_core::crs::resolve_crs("EPSG:99999", "crs"),
            Err(CrsError::NotBuiltin)
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
        // Un proiettato senza dominio (risolto dal chiamante) accetta ogni
        // coordinata finita, mai una non finita.
        validate_geometry_domain(&Geometry::Point(Point::new(-1.0e12, 1.0e12)), &projected())
            .unwrap();
        assert!(matches!(
            validate_geometry_domain(
                &Geometry::Point(Point::new(f64::INFINITY, f64::NEG_INFINITY)),
                &projected(),
            ),
            Err(CrsError::CoordinateOutOfDomain { .. })
        ));
        // Un proiettato della tabella integrata ha il suo dominio.
        let utm = plenora_core::crs::resolve_crs("EPSG:32632", "crs").unwrap();
        validate_geometry_domain(
            &Geometry::LineString(line_string![
                (x: 313_533.063, y: 4_996_791.752),
                (x: 1_312_068.675, y: 4_482_531.752),
            ]),
            &utm,
        )
        .unwrap();
        assert!(matches!(
            validate_geometry_domain(&Geometry::Point(Point::new(5.0e6, 4.0e6)), &utm),
            Err(CrsError::CoordinateOutOfDomain { .. })
        ));
    }
}
