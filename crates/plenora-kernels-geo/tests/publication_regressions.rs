//! GOP-015A/020, raccolta indipendente: attese generate con Python Fraction.
#![deny(clippy::disallowed_methods)]
use geo::{Geometry, GeometryCollection, LineString, MultiLineString, MultiPolygon, Polygon};
use plenora_kernels_geo::{
    general_boolean as gb, line_boolean as lb, polygon_boolean as pb,
    polygon_completion_with_limits, predicates::RelateLimits,
};

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
            lb::union_with_limits(&tri, &line, None, RelateLimits::default(), None).unwrap(),
            expected
        );
        assert_eq!(
            gb::boolean_with_limits(
                &tri,
                &line,
                gb::GeneralBooleanOp::Union,
                None,
                gb::GeneralBooleanLimits::default(),
                None
            )
            .unwrap(),
            expected
        );
        let pocket = Geometry::LineString(LineString::from(vec![(0., 6.5), (2., 8.), (v[6], 2.)]));
        let result =
            polygon_completion_with_limits(&[triangle()], &pocket, RelateLimits::default(), None);
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
            gb::boolean_with_limits(
                &tri,
                &line,
                op,
                filter,
                gb::GeneralBooleanLimits::default(),
                None
            )
            .unwrap(),
            expected
        );
        assert_eq!(
            lb::boolean_with_limits(&tri, &line, op, filter, RelateLimits::default(), None)
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
        gb::boolean_with_limits(
            &a,
            &b,
            gb::GeneralBooleanOp::Difference,
            None,
            gb::GeneralBooleanLimits::default(),
            None
        )
        .unwrap(),
        expected
    );
    assert_eq!(
        lb::boolean_with_limits(
            &a,
            &b,
            lb::LineBooleanOp::Difference,
            None,
            RelateLimits::default(),
            None
        )
        .unwrap(),
        expected
    );
    // Un triangolo esterno tangente al lato obliquo in un nodo costruito
    // non cambia la superficie della differenza ne' le ordinate originali.
    let touching = Geometry::Polygon(polygon(&[(0., 7.), (3., 7.), (3., 0.), (0., 7.)]));
    let actual = pb::boolean_with_limits(
        &tri,
        &touching,
        pb::PolygonBooleanOp::Difference,
        None,
        pb::PolygonBooleanLimits::default(),
        None,
    )
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
        lb::boolean_with_limits(
            &a,
            &b,
            lb::LineBooleanOp::Difference,
            None,
            RelateLimits::default(),
            None
        )
        .unwrap(),
        expected
    );
    assert_eq!(
        gb::boolean_with_limits(
            &a,
            &b,
            gb::GeneralBooleanOp::Difference,
            None,
            gb::GeneralBooleanLimits::default(),
            None
        )
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
        polygon_completion_with_limits(&[triangle()], &lines, RelateLimits::default(), None)
            .unwrap(),
        expected
    );
}
