//! Contratto CRS per i kernel geografici.
//!
//! Il contratto backend-indipendente (`ResolvedCrs`, `CrsKind`, `CrsError`,
//! `required_definition`, `validate_requirement`, dominio lon/lat) vive in
//! `plenora_core::crs` ed e' qui riesportato. Questo modulo aggiunge:
//! - il wrapper tipizzato su `geo::Geometry` per la validazione di dominio;
//! - dietro la feature `proj-backend`, la risoluzione PROJ (`resolve_crs`)
//!   contro il database bundled.

use geo::{CoordsIter, Geometry};
pub use plenora_core::crs::{
    required_definition, validate_requirement, CrsError, CrsKind, ResolvedCrs,
    MAX_CRS_DEFINITION_BYTES,
};
#[cfg(feature = "proj-backend")]
use serde_json::Value;

/// Validates normalized GIS axis order (x=longitude, y=latitude).
///
/// Call this for every geographic input after WKB decoding and before any
/// kernel work. Wrapper tipizzato: la logica e' in
/// [`plenora_core::crs::validate_geometry_domain`], senza differenze di
/// comportamento rispetto al sorgente.
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

/// Risolve una definizione CRS contro il database PROJ bundled.
///
/// Produce il [`ResolvedCrs`] canonico: PROJJSON, classificazione
/// geographic/projected e unita' lineare orizzontale in metri. Per un
/// `BoundCRS`, tipo e proprieta' d'identita' sono dedotti dal `source_crs`;
/// canonical completo e definizione originale conservano l'operazione.
///
/// # Errors
///
/// - come [`required_definition`]: [`CrsError::Required`] se la definizione
///   e' vuota, [`CrsError::InvalidDefinition`] se supera il limite di byte
///   o contiene NUL;
/// - [`CrsError::InvalidDefinition`]: PROJ rifiuta la definizione, non
///   produce PROJJSON o il PROJJSON prodotto non e' JSON valido;
/// - [`CrsError::UnsupportedType`]: il tipo PROJJSON manca o non e' un
///   `GeographicCRS` bidimensionale ne' un `ProjectedCRS`;
/// - [`CrsError::MissingLinearUnit`]: un `ProjectedCRS` non dichiara
///   un'unita' lineare orizzontale valida, finita, positiva e uguale sui
///   due assi.
#[cfg(feature = "proj-backend")]
pub fn resolve_crs(definition: &str, name: &'static str) -> Result<ResolvedCrs, CrsError> {
    use proj::Proj;

    plenora_core::crs::required_definition(Some(definition), name)?;
    let projection = Proj::new(definition).map_err(|error| CrsError::InvalidDefinition {
        name,
        reason: error.to_string(),
    })?;
    let projjson = projection
        .to_projjson(Some(false), None, None)
        .map_err(|error| CrsError::InvalidDefinition {
            name,
            reason: error.to_string(),
        })?;
    let canonical: Value =
        serde_json::from_str(&projjson).map_err(|error| CrsError::InvalidDefinition {
            name,
            reason: format!("PROJJSON non valido prodotto da PROJ: {error}"),
        })?;
    let root_type = canonical
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| CrsError::UnsupportedType("mancante".to_owned()))?;
    let canonical_crs = if root_type == "BoundCRS" {
        canonical
            .get("source_crs")
            .ok_or_else(|| CrsError::UnsupportedType("BoundCRS senza source_crs".to_owned()))?
    } else {
        &canonical
    };
    let crs_type = canonical_crs
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| CrsError::UnsupportedType(format!("{root_type} senza tipo sorgente")))?;
    let kind = match crs_type {
        "GeographicCRS" if has_two_horizontal_axes(canonical_crs) => CrsKind::Geographic,
        "GeographicCRS" => {
            return Err(CrsError::UnsupportedType(
                "GeographicCRS non bidimensionale".to_owned(),
            ))
        }
        "ProjectedCRS" => CrsKind::Projected,
        other => return Err(CrsError::UnsupportedType(other.to_owned())),
    };
    let horizontal_unit_to_metre = match kind {
        CrsKind::Geographic => None,
        CrsKind::Projected => Some(projected_linear_unit(canonical_crs)?),
    };
    Ok(ResolvedCrs::from_resolved_parts(
        definition.to_owned(),
        canonical,
        kind,
        horizontal_unit_to_metre,
    ))
}

#[cfg(feature = "proj-backend")]
fn has_two_horizontal_axes(canonical: &Value) -> bool {
    canonical
        .get("coordinate_system")
        .and_then(|value| value.get("axis"))
        .and_then(Value::as_array)
        .is_some_and(|axes| axes.len() == 2)
}

#[cfg(feature = "proj-backend")]
fn projected_linear_unit(canonical: &Value) -> Result<f64, CrsError> {
    let axes = canonical
        .get("coordinate_system")
        .and_then(|value| value.get("axis"))
        .and_then(Value::as_array)
        .ok_or(CrsError::MissingLinearUnit)?;
    if axes.len() < 2 {
        return Err(CrsError::MissingLinearUnit);
    }
    let first = axis_linear_unit(&axes[0]).ok_or(CrsError::MissingLinearUnit)?;
    let second = axis_linear_unit(&axes[1]).ok_or(CrsError::MissingLinearUnit)?;
    let tolerance = first.abs().max(second.abs()).max(1.0) * 1e-12;
    if !first.is_finite()
        || !second.is_finite()
        || first <= 0.0
        || second <= 0.0
        || (first - second).abs() > tolerance
    {
        return Err(CrsError::MissingLinearUnit);
    }
    Ok(first)
}

#[cfg(feature = "proj-backend")]
fn axis_linear_unit(axis: &Value) -> Option<f64> {
    let unit = axis.get("unit")?;
    match unit {
        Value::String(name) if matches!(name.to_ascii_lowercase().as_str(), "metre" | "meter") => {
            Some(1.0)
        }
        Value::Object(object) => {
            let unit_type = object.get("type")?.as_str()?;
            if unit_type != "LinearUnit" {
                return None;
            }
            object.get("conversion_factor")?.as_f64()
        }
        _ => None,
    }
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

    #[cfg(not(feature = "proj-backend"))]
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

    #[cfg(feature = "proj-backend")]
    #[test]
    fn proj_classifies_geographic_projected_and_linear_units() {
        let geographic = resolve_crs("EPSG:4326", "crs").unwrap();
        assert_eq!(geographic.kind(), CrsKind::Geographic);
        assert_eq!(geographic.horizontal_unit_to_metre(), None);

        let metres = resolve_crs("EPSG:3857", "crs").unwrap();
        assert_eq!(metres.kind(), CrsKind::Projected);
        assert!((metres.horizontal_unit_to_metre().unwrap() - 1.0).abs() < 1e-15);

        let feet = resolve_crs("EPSG:2230", "crs").unwrap();
        assert_eq!(feet.kind(), CrsKind::Projected);
        assert!((feet.horizontal_unit_to_metre().unwrap() - 0.304_800_609_601_219).abs() < 1e-12);

        assert!(matches!(
            resolve_crs("EPSG:4978", "crs"),
            Err(CrsError::UnsupportedType(_))
        ));
        assert!(matches!(
            resolve_crs("EPSG:4979", "crs"),
            Err(CrsError::UnsupportedType(_))
        ));
    }

    #[cfg(feature = "proj-backend")]
    #[test]
    fn bound_crs_uses_its_source_crs_without_losing_the_bound_definition() {
        use plenora_core::contract::AxisOrder;

        const MONTE_MARIO_WKT_TOWGS84: &str = concat!(
            r#"PROJCS["Monte Mario / Italy zone 1",GEOGCS["Monte Mario","#,
            r#"DATUM["Monte_Mario",SPHEROID["International 1924",6378388,297],"#,
            r#"TOWGS84[-104.1,-49.1,-9.9,0.971,-2.917,0.714,-11.68]],"#,
            r#"PRIMEM["Greenwich",0],UNIT["degree",0.0174532925199433]],"#,
            r#"PROJECTION["Transverse_Mercator"],PARAMETER["latitude_of_origin",0],"#,
            r#"PARAMETER["central_meridian",9],PARAMETER["scale_factor",0.9996],"#,
            r#"PARAMETER["false_easting",1500000],PARAMETER["false_northing",0],"#,
            r#"UNIT["metre",1],AXIS["Easting",EAST],AXIS["Northing",NORTH],"#,
            r#"AUTHORITY["EPSG","3003"]]"#,
        );

        let bound = resolve_crs(MONTE_MARIO_WKT_TOWGS84, "crs").unwrap();
        assert_eq!(bound.definition(), MONTE_MARIO_WKT_TOWGS84);
        assert_eq!(bound.kind(), CrsKind::Projected);
        assert!((bound.horizontal_unit_to_metre().unwrap() - 1.0).abs() < 1e-15);
        assert_eq!(bound.authority_identifier(), Some(("EPSG", 3003)));
        assert_eq!(
            bound.authority_axis_order(),
            Some(AxisOrder::EastingNorthing)
        );

        let Geometry::Point(reprojected) = crate::proj_backend::reproject_geometry(
            &Geometry::Point(geo::Point::new(1_500_000.0, 5_030_000.0)),
            MONTE_MARIO_WKT_TOWGS84,
            "EPSG:4326",
            1,
        )
        .unwrap() else {
            panic!("point expected")
        };
        // Oracolo PROJ bundled 9.6.2, ordine GIS normalizzato lon/lat. La
        // tolleranza di circa un centimetro resta molto sotto lo shift datum.
        assert!((reprojected.x() - 8.999_646_005_468).abs() < 1e-7);
        assert!((reprojected.y() - 45.423_341_342_810).abs() < 1e-7);

        let alternative_definition = MONTE_MARIO_WKT_TOWGS84.replace("-104.1,-49.1,-9.9", "0,0,0");
        let alternative = resolve_crs(&alternative_definition, "crs").unwrap();
        assert!(!bound.semantically_equals(&alternative));
    }

    #[cfg(feature = "proj-backend")]
    #[test]
    fn requirements_and_semantic_crs_equality_are_fail_closed() {
        use geo::Point;
        use plenora_core::catalog::CrsRequirement;

        let geographic = resolve_crs("EPSG:4326", "left_crs").unwrap();
        let projected = resolve_crs("EPSG:3857", "left_crs").unwrap();
        let projected_alias = resolve_crs("epsg:3857", "right_crs").unwrap();
        let different = resolve_crs("EPSG:32632", "right_crs").unwrap();

        assert!(matches!(
            validate_requirement(CrsRequirement::Projected, &[&geographic]),
            Err(CrsError::ProjectedRequired { .. })
        ));
        validate_requirement(CrsRequirement::Geographic, &[&geographic]).unwrap();
        validate_requirement(
            CrsRequirement::SameProjected,
            &[&projected, &projected_alias],
        )
        .unwrap();
        assert!(matches!(
            validate_requirement(CrsRequirement::SameProjected, &[&projected, &different]),
            Err(CrsError::Mismatch)
        ));
        assert!(validate_requirement(CrsRequirement::Reprojection, &[&geographic]).is_err());

        assert_eq!(projected.definition(), "EPSG:3857");
        validate_requirement(CrsRequirement::Known, &[&geographic]).unwrap();
        validate_requirement(CrsRequirement::Projected, &[&projected]).unwrap();
        validate_requirement(CrsRequirement::Reprojection, &[&geographic, &projected]).unwrap();
        assert!(matches!(
            validate_requirement(CrsRequirement::Known, &[]),
            Err(CrsError::InvalidContract(_))
        ));
        assert!(matches!(
            validate_requirement(CrsRequirement::Geographic, &[&projected]),
            Err(CrsError::GeographicRequired { .. })
        ));
        validate_geometry_domain(
            &Geometry::Point(Point::new(f64::INFINITY, f64::NEG_INFINITY)),
            &projected,
        )
        .unwrap();
    }

    #[cfg(feature = "proj-backend")]
    #[test]
    fn geographic_domain_checks_normalized_longitude_latitude() {
        use geo::{line_string, Point};

        let geographic = resolve_crs("OGC:CRS84", "crs").unwrap();
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
    }
}
