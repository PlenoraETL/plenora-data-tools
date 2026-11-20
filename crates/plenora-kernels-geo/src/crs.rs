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
    required_definition, validate_requirement, CrsError, CrsKind, CrsMode, ResolvedCrs,
    MAX_CRS_DEFINITION_BYTES,
};

/// CRS degli operandi di una chiamata diretta.
///
/// Un elemento per operando
/// (una collezione omogenea e' un operando); nessuna inferenza dalle coordinate.
/// Le API `with_crs` verificano questo contratto prima di chiamare il kernel.
/// Assi XY normalizzati e dominio delle coordinate restano precondizioni del
/// chiamante, come per le API storiche; questo tipo non visita geometrie.
#[derive(Clone, Copy, Debug)]
pub struct CrsContext<'a> {
    inputs: &'a [&'a ResolvedCrs],
    mode: CrsMode,
}

impl<'a> CrsContext<'a> {
    /// Requisito del catalogo, senza consenso al piano angolare.
    #[must_use]
    pub const fn new(inputs: &'a [&'a ResolvedCrs]) -> Self {
        Self {
            inputs,
            mode: CrsMode::Required,
        }
    }

    /// Consenso al piano XY nelle unita' dello SR. Nessuna geodetica,
    /// conversione di unita', normalizzazione del seam o riproiezione.
    #[must_use]
    pub const fn planar_coordinates(inputs: &'a [&'a ResolvedCrs]) -> Self {
        Self {
            inputs,
            mode: CrsMode::PlanarCoordinates,
        }
    }

    pub(crate) fn check(self, operation: &str, arity: usize) -> Result<(), CrsError> {
        if self.inputs.len() != arity {
            return Err(CrsError::InvalidContract(
                "numero di CRS degli operandi errato",
            ));
        }
        let requirement = plenora_core::catalog::find_operation(operation)
            .and_then(|operation| operation.crs_requirement)
            .ok_or(CrsError::InvalidContract("requisito CRS non dichiarato"))?;
        plenora_core::crs::validate_requirement_with_mode(requirement, self.inputs, self.mode)
    }

    pub(crate) fn precision(self) -> Result<crate::rust_backend::precision::Precision, CrsError> {
        let crs = self
            .inputs
            .first()
            .ok_or(CrsError::InvalidContract("CRS assente"))?;
        crate::rust_backend::precision::Precision::from_crs(crs)
            .map_err(|_| CrsError::InvalidContract("precisione del CRS non disponibile"))
    }

    pub(crate) fn ellipsoid(self) -> Result<crate::geodetica::EllissoideGeodetico, CrsError> {
        let crs = self
            .inputs
            .first()
            .ok_or(CrsError::InvalidContract("CRS assente"))?;
        if self
            .inputs
            .iter()
            .any(|other| !crs.semantically_equals(other))
        {
            return Err(CrsError::Mismatch);
        }
        crate::geodetica::EllissoideGeodetico::da_crs(crs)
    }
}

/// Errore di una chiamata diretta verificata.
///
/// Il fallimento del kernel resta
/// tipizzato, inclusi deadline, cancellazione e limiti: nessuna riclassificazione.
#[derive(Debug, thiserror::Error)]
pub enum DirectError<E> {
    /// Il requisito CRS o la precisione non sono soddisfatti.
    #[error(transparent)]
    Crs(#[from] CrsError),
    /// Errore originale del kernel, senza dati aggiunti.
    #[error(transparent)]
    Kernel(E),
}

// Il solo punto d'ingresso dei wrapper: requisito dal catalogo, poi kernel.
macro_rules! checked_kernel {
    ($name:ident, $op:literal, $arity:literal,
     ($($arg:ident: $ty:ty),* $(,)?) -> ($out:ty, $error:ty),
     |$context:ident| $body:block) => {
        #[doc = concat!("Chiamata con CRS verificato di [`super::", stringify!($name), "`].")]
        ///
        /// Unita', dominio, limiti e interruzioni sono quelli del kernel.
        /// La precisione e l'ellissoide, quando necessari, derivano dal CRS.
        ///
        /// # Errors
        /// `DirectError::Crs` prima del calcolo; `DirectError::Kernel` conserva
        /// esattamente l'errore del kernel, senza risultati parziali.
        #[allow(clippy::too_many_arguments, reason = "Firma del kernel piu' contesto CRS")]
        pub fn $name(
            $context: $crate::crs::CrsContext<'_>,
            $($arg: $ty),*
        ) -> Result<$out, $crate::crs::DirectError<$error>> {
            $context.check($op, $arity)?;
            $body.map_err($crate::crs::DirectError::Kernel)
        }
    };
}
pub(crate) use checked_kernel;

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

#[deny(clippy::disallowed_methods)]
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
            Err(CrsError::NotBuiltin)
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

#[cfg(test)]
#[deny(clippy::disallowed_methods)]
mod direct_tests {
    use super::*;
    use plenora_core::catalog::CrsRequirement;
    use plenora_core::crs::{resolve_crs, validate_requirement_with_mode};
    use std::collections::BTreeSet;

    // Censimento delle famiglie: ogni funzione pubblica di questi sorgenti
    // deve avere il wrapper generato dalla macro che applica il gate comune.
    const SOURCES: &[(&str, &str)] = &[
        ("advanced", include_str!("advanced.rs")),
        ("analysis", include_str!("analysis.rs")),
        ("cluster", include_str!("cluster.rs")),
        ("construction", include_str!("construction.rs")),
        ("extended", include_str!("extended.rs")),
        (
            "extended_algorithms",
            include_str!("extended_algorithms.rs"),
        ),
        (
            "extended_algorithms/polygonize_limited",
            include_str!("extended_algorithms/polygonize_limited.rs"),
        ),
        (
            "extended_algorithms/split_limited",
            include_str!("extended_algorithms/split_limited.rs"),
        ),
        ("extensions", include_str!("extensions.rs")),
        ("extensions2", include_str!("extensions2.rs")),
        ("extensions3", include_str!("extensions3.rs")),
        ("offset", include_str!("offset.rs")),
        ("operations", include_str!("operations.rs")),
        ("predicates", include_str!("predicates.rs")),
        (
            "predicates/arrangement_indexed/general_boolean",
            include_str!("predicates/arrangement_indexed/general_boolean.rs"),
        ),
        (
            "predicates/arrangement_indexed/line_boolean",
            include_str!("predicates/arrangement_indexed/line_boolean.rs"),
        ),
        (
            "predicates/arrangement_indexed/point_boolean",
            include_str!("predicates/arrangement_indexed/point_boolean.rs"),
        ),
        (
            "predicates/arrangement_indexed/polygon_boolean",
            include_str!("predicates/arrangement_indexed/polygon_boolean.rs"),
        ),
        (
            "predicates/arrangement_indexed/polygon_completion",
            include_str!("predicates/arrangement_indexed/polygon_completion.rs"),
        ),
        ("reshape", include_str!("reshape.rs")),
        (
            "rust_backend/make_valid",
            include_str!("rust_backend/make_valid.rs"),
        ),
        ("rust_backend/mod", include_str!("rust_backend/mod.rs")),
        (
            "rust_backend/polygonize",
            include_str!("rust_backend/polygonize.rs"),
        ),
        ("rust_backend/split", include_str!("rust_backend/split.rs")),
        ("spatial_join", include_str!("spatial_join.rs")),
        ("topology", include_str!("topology.rs")),
        ("trim_extend", include_str!("trim_extend.rs")),
    ];

    #[test]
    fn every_direct_api_has_the_catalogue_gate() {
        let projected = resolve_crs("EPSG:3857", "crs").unwrap();
        let geographic = resolve_crs("EPSG:4326", "crs").unwrap();
        for (module, source) in SOURCES {
            let public: BTreeSet<_> = source
                .lines()
                .filter_map(|line| {
                    line.strip_prefix("pub fn ")
                        .and_then(|rest| rest.split('(').next())
                })
                .filter(|name| *name != "residual_classes")
                .collect();
            let mut covered = BTreeSet::new();
            for entry in source.split("crate::crs::checked_kernel!(").skip(1) {
                let mut fields = entry.split(',');
                let name = fields.next().unwrap().trim();
                let operation = fields.next().unwrap().trim().trim_matches('"');
                let arity: usize = fields.next().unwrap().trim().parse().unwrap();
                assert!(covered.insert(name), "{module}::{name}");
                let requirement = plenora_core::catalog::find_operation(operation)
                    .unwrap()
                    .crs_requirement
                    .unwrap();
                let projected_inputs = vec![&projected; arity];
                let geographic_inputs = vec![&geographic; arity];
                assert!(matches!(
                    CrsContext::new(&[]).check(operation, arity),
                    Err(CrsError::InvalidContract(_))
                ));
                match requirement {
                    CrsRequirement::Projected | CrsRequirement::SameProjected => {
                        CrsContext::new(&projected_inputs)
                            .check(operation, arity)
                            .unwrap();
                        assert!(matches!(
                            CrsContext::new(&geographic_inputs).check(operation, arity),
                            Err(CrsError::ProjectedRequired { .. })
                        ));
                        CrsContext::planar_coordinates(&geographic_inputs)
                            .check(operation, arity)
                            .unwrap();
                    }
                    CrsRequirement::Geographic => {
                        CrsContext::new(&geographic_inputs)
                            .check(operation, arity)
                            .unwrap();
                        assert!(matches!(
                            CrsContext::new(&projected_inputs).check(operation, arity),
                            Err(CrsError::GeographicRequired { .. })
                        ));
                        assert!(matches!(
                            CrsContext::planar_coordinates(&geographic_inputs)
                                .check(operation, arity),
                            Err(CrsError::InvalidContract(_))
                        ));
                    }
                    CrsRequirement::Known => {
                        CrsContext::new(&projected_inputs)
                            .check(operation, arity)
                            .unwrap();
                        CrsContext::new(&geographic_inputs)
                            .check(operation, arity)
                            .unwrap();
                    }
                    CrsRequirement::Reprojection => {
                        assert_eq!(arity, 2);
                    }
                }
            }
            assert_eq!(public, covered, "{module}: API senza controllo CRS");
        }
    }

    #[test]
    fn catalogue_and_root_transformations_are_completely_enumerated() {
        let mut covered = BTreeSet::from([
            "geo.centroid",
            "geo.convex_hull",
            "geo.envelope",
            "geo.reproject",
            "geo.symmetric_difference",
            "geo.predicate_disjoint",
            "geo.predicate_contains",
            "geo.predicate_within",
            "geo.predicate_equals_topo",
            "geo.predicate_covers",
            "geo.predicate_covered_by",
            "geo.predicate_contains_properly",
            "geo.predicate_touches",
            "geo.predicate_crosses",
            "geo.predicate_overlaps",
        ]);
        for (_, source) in SOURCES {
            for entry in source.split("crate::crs::checked_kernel!(").skip(1) {
                covered.insert(entry.split(',').nth(1).unwrap().trim().trim_matches('"'));
            }
        }
        let expected: BTreeSet<_> = plenora_core::catalog::CATALOG
            .iter()
            .filter(|descriptor| descriptor.crs_requirement.is_some())
            .map(|descriptor| descriptor.id)
            .collect();
        assert_eq!(covered, expected);
        let geographic = resolve_crs("EPSG:4326", "crs").unwrap();
        let square = crate::test_support::rect(-1., -1., 1., 1.);
        let wkb = crate::test_support::to_wkb(&square);
        for operation in crate::Operation::ALL {
            assert!(matches!(
                crate::with_crs::transform_geometry(
                    CrsContext::new(&[&geographic]),
                    operation,
                    &square
                ),
                Err(DirectError::Crs(CrsError::ProjectedRequired { .. }))
            ));
            assert!(matches!(
                crate::with_crs::transform_wkb(CrsContext::new(&[&geographic]), operation, &wkb),
                Err(DirectError::Crs(CrsError::ProjectedRequired { .. }))
            ));
            crate::with_crs::transform_geometry(
                CrsContext::planar_coordinates(&[&geographic]),
                operation,
                &square,
            )
            .unwrap();
        }
    }

    #[test]
    fn precision_and_ellipsoid_come_from_the_context() {
        let geographic = resolve_crs("EPSG:4326", "crs").unwrap();
        let other = resolve_crs("EPSG:4269", "crs").unwrap();
        let inputs = [&geographic];
        let context = CrsContext::planar_coordinates(&inputs);
        assert_eq!(
            context.precision().unwrap().value().to_bits(),
            geographic.precisione_coordinate().unwrap().to_bits()
        );
        assert!(matches!(
            CrsContext::new(&[&geographic, &other]).ellipsoid(),
            Err(CrsError::Mismatch)
        ));
        let point = geo::Point::new(0., 0.);
        assert!(matches!(
            crate::extended::with_crs::geodesic_distance_m(context, point, point),
            Err(DirectError::Crs(CrsError::InvalidContract(_)))
        ));
        let inputs = [&geographic, &geographic];
        let context = CrsContext::new(&inputs);
        assert_eq!(
            crate::extended::with_crs::geodesic_distance_m(context, point, point)
                .unwrap()
                .to_bits(),
            0_f64.to_bits()
        );
    }

    #[test]
    fn square_units_seam_and_poles_are_explicitly_planar() {
        use crate::operations::with_crs::{area, length};
        let geographic = resolve_crs("EPSG:4326", "crs").unwrap();
        let projected = resolve_crs("EPSG:3857", "crs").unwrap();
        let square = crate::test_support::rect(-1., -1., 1., 1.);
        assert_eq!(
            area(CrsContext::new(&[&projected]), &square)
                .unwrap()
                .to_bits(),
            4_f64.to_bits()
        );
        assert!(matches!(
            area(CrsContext::new(&[&geographic]), &square),
            Err(DirectError::Crs(CrsError::ProjectedRequired { .. }))
        ));
        assert!(matches!(
            area(CrsContext::new(&[]), &square),
            Err(DirectError::Crs(CrsError::InvalidContract(_)))
        ));
        assert_eq!(
            area(CrsContext::planar_coordinates(&[&geographic]), &square)
                .unwrap()
                .to_bits(),
            4_f64.to_bits()
        );
        // Oracolo analitico XY: 179 - (-179) = 358 gradi, nessun wrap.
        let seam = Geometry::LineString(geo::LineString::from(vec![(-179., 0.), (179., 0.)]));
        assert_eq!(
            length(CrsContext::planar_coordinates(&[&geographic]), &seam)
                .unwrap()
                .to_bits(),
            358_f64.to_bits()
        );
        let polar = Geometry::LineString(geo::LineString::from(vec![(-1., 90.), (1., 90.)]));
        assert_eq!(
            length(CrsContext::planar_coordinates(&[&geographic]), &polar)
                .unwrap()
                .to_bits(),
            2_f64.to_bits()
        );
    }

    #[test]
    fn mismatch_and_invalid_linear_units_do_not_get_a_planar_exception() {
        let geographic = resolve_crs("EPSG:4326", "crs").unwrap();
        let other = resolve_crs("EPSG:4269", "crs").unwrap();
        assert!(matches!(
            validate_requirement_with_mode(
                CrsRequirement::SameProjected,
                &[&geographic, &other],
                CrsMode::PlanarCoordinates
            ),
            Err(CrsError::Mismatch)
        ));
        let invalid = ResolvedCrs::from_resolved_parts(
            "local".to_owned(),
            serde_json::json!({}),
            CrsKind::Projected,
            None,
        );
        assert!(matches!(
            validate_requirement_with_mode(
                CrsRequirement::Projected,
                &[&invalid],
                CrsMode::PlanarCoordinates
            ),
            Err(CrsError::MissingLinearUnit)
        ));
        assert!(matches!(
            validate_requirement_with_mode(
                CrsRequirement::Reprojection,
                &[&geographic, &other],
                CrsMode::PlanarCoordinates
            ),
            Err(CrsError::InvalidContract(_))
        ));
    }

    #[test]
    fn shared_entrypoints_have_identical_catalogue_requirements() {
        for family in [
            &["geo.centroid", "geo.convex_hull", "geo.envelope"][..],
            &[
                "geo.union",
                "geo.intersection",
                "geo.difference",
                "geo.symmetric_difference",
            ][..],
            &[
                "geo.predicate_intersects",
                "geo.predicate_disjoint",
                "geo.predicate_contains",
                "geo.predicate_within",
                "geo.predicate_equals_topo",
                "geo.predicate_covers",
                "geo.predicate_covered_by",
                "geo.predicate_contains_properly",
                "geo.predicate_touches",
                "geo.predicate_crosses",
                "geo.predicate_overlaps",
            ][..],
        ] {
            let expected = plenora_core::catalog::find_operation(family[0])
                .unwrap()
                .crs_requirement;
            for name in family {
                assert_eq!(
                    plenora_core::catalog::find_operation(name)
                        .unwrap()
                        .crs_requirement,
                    expected
                );
            }
        }
    }
}
