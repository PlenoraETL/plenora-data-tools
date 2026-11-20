//! GOP-015A/020, raccolta indipendente: attese generate con Python Fraction.
#![deny(clippy::disallowed_methods)]
use geo::{Geometry, GeometryCollection, LineString, MultiLineString, MultiPolygon, Polygon};
use plenora_kernels_geo::crs::CrsContext;
use plenora_kernels_geo::general_boolean as gb;
use plenora_kernels_geo::line_boolean as lb;
use plenora_kernels_geo::polygon_boolean as pb;
use plenora_kernels_geo::predicates::RelateLimits;
use plenora_kernels_geo::with_crs::polygon_completion_with_limits;

fn polygon(coords: &[(f64, f64)]) -> Polygon<f64> {
    Polygon::new(LineString::from(coords.to_vec()), vec![])
}
fn triangle() -> Polygon<f64> {
    polygon(&[(0., 0.), (3., 0.), (0., 7.), (0., 0.)])
}

#[test]
fn p3b_fraction_two_hundred_unions_and_eleven_completion_faces() {
    let tri = Geometry::Polygon(triangle());
    let mut faces = 0;
    let mut rejected = 0;
    for row in include_str!("oracles/publication_fraction.txt").lines() {
        let v: Vec<f64> = row.split_whitespace().map(|x| x.parse().unwrap()).collect();
        let line = Geometry::LineString(LineString::from(vec![(v[0], v[1]), (v[2], v[3])]));
        let expected = Geometry::GeometryCollection(GeometryCollection(vec![
            Geometry::MultiLineString(MultiLineString(vec![LineString::from(vec![
                (v[4], v[5]),
                (v[2], v[3]),
            ])])),
            Geometry::MultiPolygon(MultiPolygon(vec![polygon(&[
                (0., 0.),
                (3., 0.),
                (v[4], v[5]),
                (0., 7.),
                (0., 0.),
            ])])),
        ]));
        assert_eq!(
            lb::with_crs::union_with_limits(
                CrsContext::new(&[crs_fixture_metrico(); 2]),
                &tri,
                &line,
                None,
                RelateLimits::default(),
                None
            )
            .map_err(kernel_error_fixture)
            .unwrap(),
            expected
        );
        assert_eq!(
            gb::with_crs::boolean_with_limits(
                CrsContext::new(&[crs_fixture_metrico(); 2]),
                &tri,
                &line,
                gb::GeneralBooleanOp::Union,
                None,
                gb::GeneralBooleanLimits::default(),
                None
            )
            .map_err(kernel_error_fixture)
            .unwrap(),
            expected
        );
        let pocket = Geometry::LineString(LineString::from(vec![(0., 6.5), (2., 8.), (v[6], 2.)]));
        let result = polygon_completion_with_limits(
            CrsContext::new(&[crs_fixture_metrico(); 2]),
            &[triangle()],
            &pocket,
            RelateLimits::default(),
            None,
        )
        .map_err(kernel_error_fixture);
        if v[12].to_bits() == 1_f64.to_bits() {
            // Fraction: il nodo arrotondato entra nell'area occupata immutabile.
            assert!(matches!(
                result,
                Err(plenora_kernels_geo::polygon_completion::PolygonCompletionError::InvalidRoundedTopology)
            ));
            rejected += 1;
            continue;
        }
        let actual = result.unwrap();
        if v[7].to_bits() == 1_f64.to_bits() {
            faces += 1;
            assert_eq!(
                actual,
                vec![polygon(&[
                    (v[8], v[9]),
                    (v[10], v[11]),
                    (2., 8.),
                    (v[8], v[9])
                ])]
            );
        } else {
            assert!(actual.is_empty());
        }
    }
    assert_eq!(faces, 6);
    assert_eq!(rejected, 5);
}

#[test]
fn carried_surfaces_and_lines_have_no_constructed_kinks() {
    let tri = Geometry::Polygon(triangle());
    let line = Geometry::LineString(LineString::from(vec![(1., 1.), (3., 2.)]));
    let expected = Geometry::MultiPolygon(MultiPolygon(vec![triangle()]));
    for (op, filter) in [
        (gb::GeneralBooleanOp::Difference, None),
        (gb::GeneralBooleanOp::Union, Some(2)),
    ] {
        assert_eq!(
            gb::with_crs::boolean_with_limits(
                CrsContext::new(&[crs_fixture_metrico(); 2]),
                &tri,
                &line,
                op,
                filter,
                gb::GeneralBooleanLimits::default(),
                None
            )
            .map_err(kernel_error_fixture)
            .unwrap(),
            expected
        );
        assert_eq!(
            lb::with_crs::boolean_with_limits(
                CrsContext::new(&[crs_fixture_metrico(); 2]),
                &tri,
                &line,
                op,
                filter,
                RelateLimits::default(),
                None
            )
            .map_err(kernel_error_fixture)
            .unwrap(),
            expected
        );
    }
    let a = Geometry::LineString(LineString::from(vec![(0., 0.), (1., 1.)]));
    let b = Geometry::LineString(LineString::from(vec![(0., 1.), (1., -1.)]));
    // GOP-015: chiusura della differenza, nessun vertice (1/3,1/3) necessario.
    let expected = Geometry::MultiLineString(MultiLineString(vec![match &a {
        Geometry::LineString(l) => l.clone(),
        _ => unreachable!(),
    }]));
    assert_eq!(
        gb::with_crs::boolean_with_limits(
            CrsContext::new(&[crs_fixture_metrico(); 2]),
            &a,
            &b,
            gb::GeneralBooleanOp::Difference,
            None,
            gb::GeneralBooleanLimits::default(),
            None
        )
        .map_err(kernel_error_fixture)
        .unwrap(),
        expected
    );
    assert_eq!(
        lb::with_crs::boolean_with_limits(
            CrsContext::new(&[crs_fixture_metrico(); 2]),
            &a,
            &b,
            lb::LineBooleanOp::Difference,
            None,
            RelateLimits::default(),
            None
        )
        .map_err(kernel_error_fixture)
        .unwrap(),
        expected
    );
    // Un triangolo esterno tangente al lato obliquo in un nodo costruito
    // non cambia la superficie della differenza ne' le ordinate originali.
    let touching = Geometry::Polygon(polygon(&[(0., 7.), (3., 7.), (3., 0.), (0., 7.)]));
    let actual = pb::with_crs::boolean_with_limits(
        CrsContext::new(&[crs_fixture_metrico(); 2]),
        &tri,
        &touching,
        pb::PolygonBooleanOp::Difference,
        None,
        pb::PolygonBooleanLimits::default(),
        None,
    )
    .map_err(kernel_error_fixture)
    .unwrap();
    assert_eq!(
        actual,
        Geometry::MultiPolygon(MultiPolygon(vec![triangle()]))
    );
}

#[test]
fn duplicate_sources_do_not_make_a_node_necessary() {
    let a = Geometry::MultiLineString(MultiLineString(vec![
        LineString::from(vec![(0., 0.), (1., 1.)]),
        LineString::from(vec![(0., 0.), (1., 1.)]),
    ]));
    let b = Geometry::LineString(LineString::from(vec![(0., 1.), (1., -1.)]));
    let expected = Geometry::MultiLineString(MultiLineString(vec![LineString::from(vec![
        (0., 0.),
        (1., 1.),
    ])]));
    assert_eq!(
        lb::with_crs::boolean_with_limits(
            CrsContext::new(&[crs_fixture_metrico(); 2]),
            &a,
            &b,
            lb::LineBooleanOp::Difference,
            None,
            RelateLimits::default(),
            None
        )
        .map_err(kernel_error_fixture)
        .unwrap(),
        expected
    );
    assert_eq!(
        gb::with_crs::boolean_with_limits(
            CrsContext::new(&[crs_fixture_metrico(); 2]),
            &a,
            &b,
            gb::GeneralBooleanOp::Difference,
            None,
            gb::GeneralBooleanLimits::default(),
            None
        )
        .map_err(kernel_error_fixture)
        .unwrap(),
        expected
    );
}

#[test]
fn completion_dangling_crossing_does_not_bend_carried_boundary() {
    // GOP-020/015A: il ramo aperto non cambia l'unica faccia esterna.
    let lines = Geometry::MultiLineString(MultiLineString(vec![
        LineString::from(vec![(3., 0.), (4., 0.), (4., 8.), (0., 8.), (0., 7.)]),
        LineString::from(vec![(1., 1.), (3., 2.)]),
    ]));
    let expected = vec![polygon(&[
        (0., 7.),
        (3., 0.),
        (4., 0.),
        (4., 8.),
        (0., 8.),
        (0., 7.),
    ])];
    assert_eq!(
        polygon_completion_with_limits(
            CrsContext::new(&[crs_fixture_metrico(); 2]),
            &[triangle()],
            &lines,
            RelateLimits::default(),
            None
        )
        .map_err(kernel_error_fixture)
        .unwrap(),
        expected
    );
}

// Un rifiuto CRS inatteso deve far fallire la fixture, senza mascherare l'oracolo del kernel.
fn kernel_error_fixture<E>(error: plenora_kernels_geo::crs::DirectError<E>) -> E {
    match error {
        plenora_kernels_geo::crs::DirectError::Kernel(error) => error,
        plenora_kernels_geo::crs::DirectError::Crs(_) => panic!("CRS della fixture rifiutato"),
    }
}

// SR esplicito e stabile delle coordinate di questa fixture.
fn crs_fixture_metrico() -> &'static plenora_core::crs::ResolvedCrs {
    static CRS: std::sync::OnceLock<plenora_core::crs::ResolvedCrs> = std::sync::OnceLock::new();
    CRS.get_or_init(|| plenora_core::crs::resolve_crs("EPSG:3857", "crs").expect("CRS fixture"))
}
