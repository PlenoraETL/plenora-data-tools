//! C1: attese analitiche indipendenti e snapshot letterale v6.0.0.
use super::*;
use geo::{line_string, MultiLineString, Point};

fn run(source: &LineString<f64>, cutter: &Geometry<f64>) -> Vec<LineString<f64>> {
    split_line_with_limits(source, cutter, RelateLimits::default(), None).expect("split C1")
}

fn transverse(count: u32) -> Geometry<f64> {
    Geometry::MultiLineString(MultiLineString(
        (1..=count)
            .map(|i| {
                let x = f64::from(i);
                line_string![(x:x,y:-1.), (x:x,y:1.)]
            })
            .collect(),
    ))
}

#[test]
fn transverse_rational_endpoints_and_600() {
    for k in 1..=16 {
        let source = line_string![(x:0.,y:0.), (x:32.,y:0.)];
        let cutter = transverse(k);
        let result = run(&source, &cutter);
        // Fonte analitica: k ascisse intere interne distinte partizionano [0,32].
        let expected: Vec<_> = (0..=k)
            .map(|i| line_string![(x:f64::from(i),y:0.),(x:if i == k { 32. } else { f64::from(i+1) },y:0.)])
            .collect();
        assert_eq!(result, expected);
        // Fonte: snapshot v6.0.0, blob bffc2e47508877056318de4dd9990e27746f2b24.
        let old =
            super::super::baseline_600::split_line(&source, &cutter, 0., 1000, 10000, 1000, 10000)
                .expect("6.0.0");
        // Rapporti i/32 esatti anche nella ricostruzione storica.
        assert_eq!(result, old);
    }
    let source = line_string![(x:0.,y:0.),(x:1.,y:0.)];
    let cutter = Geometry::LineString(line_string![(x:0.,y:-1.),(x:1.,y:3.)]);
    // Fonte: y=-1+4t=0 => t=1/4, x=1/4 (razionale binario esatto).
    assert_eq!(
        run(&source, &cutter),
        vec![
            line_string![(x:0.,y:0.),(x:0.25,y:0.)],
            line_string![(x:0.25,y:0.),(x:1.,y:0.)]
        ]
    );
    let third = Geometry::LineString(line_string![(x:0.,y:-1.),(x:1.,y:2.)]);
    // Fonte: y=-1+3t=0 => x=1/3; conversione finale correttamente arrotondata.
    assert_eq!(run(&source, &third)[0].0[1], Coord { x: 1. / 3., y: 0. });
}

#[test]
fn vertices_overlap_reverse_and_points_match_600() {
    let cases = [
        (
            line_string![(x:0.,y:0.),(x:2.,y:0.),(x:4.,y:0.)],
            Geometry::LineString(line_string![(x:2.,y:-1.),(x:2.,y:1.)]),
        ),
        (
            line_string![(x:0.,y:0.),(x:4.,y:0.)],
            Geometry::LineString(line_string![(x:1.,y:0.),(x:3.,y:0.)]),
        ),
        (
            line_string![(x:4.,y:0.),(x:0.,y:0.)],
            Geometry::LineString(line_string![(x:1.,y:0.),(x:3.,y:0.)]),
        ),
        (
            line_string![(x:0.,y:0.),(x:4.,y:0.)],
            Geometry::Point(Point::new(2., 0.)),
        ),
    ];
    for (index, (source, cutter)) in cases.iter().enumerate() {
        let result = run(source, cutter);
        // Fonte analitica: vertice/punto a 2 => 2 parti; overlap [1,3] => 3.
        assert_eq!(result.len(), if index == 1 || index == 2 { 3 } else { 2 });
        // Fonte: snapshot letterale v6.0.0 sopra identificata.
        assert_eq!(
            result,
            super::super::baseline_600::split_line(source, cutter, 0., 100, 100, 100, 100)
                .expect("6.0.0")
        );
    }
}

#[test]
fn comb_and_large_sparse() {
    let n = 64;
    let source = line_string![(x:0.,y:0.),(x:f64::from(n+1),y:0.)];
    let mut teeth = match transverse(n) {
        Geometry::MultiLineString(ls) => ls.0,
        _ => unreachable!(),
    };
    teeth.push(line_string![(x:1.,y:1.),(x:f64::from(n),y:1.)]);
    // Fonte: i n denti incrociano y=0 alle ascisse 1..n; dorso y=1 disgiunto.
    assert_eq!(
        run(&source, &Geometry::MultiLineString(MultiLineString(teeth))).len(),
        65
    );
    let distant = Geometry::MultiLineString(MultiLineString(
        (0..4096)
            .map(|i| {
                let x = f64::from(i) + 100.;
                line_string![(x:x,y:10.),(x:x,y:11.)]
            })
            .collect(),
    ));
    let source = line_string![(x:0.,y:0.),(x:1.,y:0.)];
    // Fonte: inviluppi X disgiunti tra ogni coppia; zero test esatti, 4096
    // primitive contro una sorgente, nessun rifiuto preventivo n*m.
    assert_eq!(
        split_line_with_limits(
            &source,
            &distant,
            RelateLimits {
                max_segment_pairs: 0,
                ..RelateLimits::default()
            },
            None
        )
        .expect("sparso"),
        vec![source]
    );
}

#[test]
fn exact_thresholds_and_interruptions() {
    let source = line_string![(x:0.,y:0.),(x:2.,y:0.)];
    let cutter = transverse(1);
    let mut measured = Work::new(RelateLimits::default(), None);
    let expected = split(&source, &cutter, &mut measured).expect("misura del consumo");
    // Fonte: noding e certificato condividono il consumo misurato.
    assert!(measured.pairs > 1 && measured.nodes > 6);
    // Fonte memoria/passi: picco delle ricevute vive e checkpoint del percorso
    // completo, non stima n*m; soglia misurata rieseguita senza strumentazione.
    let exact = RelateLimits {
        max_segment_pairs: measured.pairs,
        max_nodes: measured.nodes,
        max_steps: measured.steps,
        max_reserved_bytes: measured.peak_bytes(),
        deadline: None,
    };
    assert_eq!(
        split_line_with_limits(&source, &cutter, exact, None).expect("soglia"),
        expected
    );
    for limits in [
        RelateLimits {
            max_segment_pairs: exact.max_segment_pairs - 1,
            ..exact
        },
        RelateLimits {
            max_nodes: exact.max_nodes - 1,
            ..exact
        },
        RelateLimits {
            max_steps: exact.max_steps - 1,
            ..exact
        },
    ] {
        // Fonte: contratto inclusivo RelateLimits, un'unita' sotto il consumo.
        assert!(matches!(
            split_line_with_limits(&source, &cutter, limits, None),
            Err(PredicateError::WorkLimit)
        ));
    }
    // Fonte: stesso contratto, byte di picco meno uno.
    assert!(matches!(
        split_line_with_limits(
            &source,
            &cutter,
            RelateLimits {
                max_reserved_bytes: exact.max_reserved_bytes - 1,
                ..exact
            },
            None
        ),
        Err(PredicateError::ReservationLimit)
    ));
    let flag = std::sync::atomic::AtomicBool::new(true);
    let expired = RelateLimits {
        deadline: Some(std::time::Instant::now()),
        ..RelateLimits::default()
    };
    // Fonte: Work::step, deadline prima del flag, entrambi errori distinti.
    assert!(matches!(
        split_line_with_limits(&source, &cutter, expired, Some(&flag)),
        Err(PredicateError::DeadlineExceeded)
    ));
    assert!(matches!(
        split_line_with_limits(&source, &cutter, expired, None),
        Err(PredicateError::DeadlineExceeded)
    ));
}

#[test]
fn explicit_invalid_coordinates() {
    let source = line_string![(x:0.,y:0.),(x:1.,y:0.)];
    // Fonte: contratto di finitezza, nessun dato nei messaggi.
    assert!(matches!(
        split_line_with_limits(
            &source,
            &Geometry::Point(Point::new(f64::NAN, 0.)),
            RelateLimits::default(),
            None
        ),
        Err(PredicateError::NonFiniteCoordinate { side: "right" })
    ));
}

#[test]
fn repeated_vertices_do_not_create_degenerate_pieces() {
    let cutter = Geometry::Point(Point::new(1., 0.));
    let middle = line_string![(x:0.,y:0.),(x:1.,y:0.),(x:1.,y:0.),(x:2.,y:0.)];
    // Fonte: due intervalli [0,1],[1,2]; ripetere il vertice non aggiunge
    // intervalli di lunghezza positiva. Il duplicato resta nel secondo pezzo.
    assert_eq!(
        run(&middle, &cutter),
        vec![
            line_string![(x:0.,y:0.),(x:1.,y:0.)],
            line_string![(x:1.,y:0.),(x:1.,y:0.),(x:2.,y:0.)]
        ]
    );
    let tail = line_string![(x:0.,y:0.),(x:1.,y:0.),(x:1.,y:0.)];
    // Fonte: taglio all'estremo finale, nessun punto interno => un pezzo.
    assert_eq!(run(&tail, &cutter), vec![tail]);
}

#[test]
fn polygon_boundary_and_invalid_polygon() {
    let source = line_string![(x:0.,y:0.),(x:4.,y:0.)];
    let boundary = Geometry::Polygon(geo::Polygon::new(
        line_string![(x:1.,y:-1.),(x:3.,y:-1.),(x:3.,y:1.),(x:1.,y:1.),(x:1.,y:-1.)],
        vec![],
    ));
    // Fonte: rettangolo [1,3] x [-1,1], bordo incontra y=0 in x=1 e x=3.
    assert_eq!(
        run(&source, &boundary),
        vec![
            line_string![(x:0.,y:0.),(x:1.,y:0.)],
            line_string![(x:1.,y:0.),(x:3.,y:0.)],
            line_string![(x:3.,y:0.),(x:4.,y:0.)]
        ]
    );
    let bow = Geometry::Polygon(geo::Polygon::new(
        line_string![(x:0.,y:0.),(x:2.,y:2.),(x:0.,y:2.),(x:2.,y:0.),(x:0.,y:0.)],
        vec![],
    ));
    // Fonte: SFA 1.2.1 6.1.11.1, anello auto-intersecato non valido.
    assert!(matches!(
        split_line_with_limits(&source, &bow, RelateLimits::default(), None),
        Err(PredicateError::InvalidGeometry { .. })
    ));
}

#[test]
fn gop015a_nondyadic_nodes_have_half_ulp_certificate() {
    // Fonte GOP-015A: a 2^50 la ulp e' 1/4; X+1/10 arrotonda X.
    let x = 2_f64.powi(50);
    let source = line_string![(x:x-1.,y:0.),(x:x+2.,y:0.)];
    let cutter = Geometry::LineString(line_string![(x:x,y:-1.),(x:x+1.,y:9.)]);
    assert_eq!(run(&source, &cutter)[0].0[1].x, x);
    // Fonte: GOP-015A; x=1/d, confrontato con i punti medi binary64 esatti.
    for d in [3, 5, 7] {
        let source = line_string![(x:0.,y:0.),(x:1.,y:0.)];
        let cutter = Geometry::LineString(line_string![(x:0.,y:-1.),(x:1.,y:f64::from(d-1))]);
        let result = run(&source, &cutter);
        let x = result[0].0[1].x;
        let work = Work::new(RelateLimits::default(), None);
        let exact = work
            .integer(1)
            .unwrap()
            .try_div(&work.integer(d).unwrap())
            .unwrap();
        let at = |v| P::of_in(Coord { x: v, y: 0. }, &work).unwrap().x;
        let rounded = at(x);
        assert!(!exact
            .try_cmp(
                &at(f64::from_bits(x.to_bits() - 1))
                    .try_midpoint(&rounded)
                    .unwrap()
            )
            .unwrap()
            .is_lt());
        assert!(!exact
            .try_cmp(
                &rounded
                    .try_midpoint(&at(f64::from_bits(x.to_bits() + 1)))
                    .unwrap()
            )
            .unwrap()
            .is_gt());
    }
}

#[test]
fn gop015a_ties_subnormals_and_collapse() {
    // Fonte: GOP-015A; la parita' e' il bit basso della significand.
    for bits in [1.0_f64.to_bits(), 1.0_f64.to_bits() + 1, 1] {
        for sign in [1.0, -1.0] {
            let a = sign * f64::from_bits(bits);
            let b = sign * f64::from_bits(bits + 1);
            let source = line_string![(x:-2.,y:0.),(x:2.,y:0.)];
            let cutter = Geometry::LineString(line_string![(x:a,y:-1.),(x:b,y:1.)]);
            let expected = sign * f64::from_bits(if bits & 1 == 0 { bits } else { bits + 1 });
            assert_eq!(
                run(&source, &cutter)[0].0[1].x.to_bits(),
                expected.to_bits()
            );
        }
    }
    // Fonte: mezzo subnormale arrotonda a zero, fondendo il nodo con l'estremo.
    let tiny = f64::from_bits(1);
    let source = line_string![(x:0.,y:0.),(x:tiny,y:0.)];
    let cutter = Geometry::LineString(line_string![(x:0.,y:-1.),(x:tiny,y:1.)]);
    assert!(matches!(
        split_line_with_limits(&source, &cutter, RelateLimits::default(), None),
        Err(PredicateError::InvalidRoundedTopology)
    ));
}

#[test]
fn source_crossings_overlaps_and_original_bits_are_preserved() {
    // Fonte: GOP-015A, vertici riportati invariati; noding del certificato solo.
    let empty = Geometry::MultiLineString(MultiLineString(vec![]));
    for source in [
        line_string![(x:0.,y:0.),(x:1.,y:1.),(x:0.,y:1.),(x:1.,y:-1.)],
        line_string![(x:0.,y:-0.),(x:3.,y:0.),(x:1.,y:0.),(x:0.,y:0.)],
    ] {
        let result = run(&source, &empty);
        assert_eq!(result.len(), 1);
        for (a, b) in source.0.iter().zip(&result[0].0) {
            assert_eq!(
                (a.x.to_bits(), a.y.to_bits()),
                (b.x.to_bits(), b.y.to_bits())
            );
        }
    }
}

#[test]
fn splitter_vertex_preserves_signed_zero() {
    // Fonte GOP-015A: vertice riportato della lama, bit dello zero invariati.
    let source = line_string![(x:0.,y:-1.),(x:0.,y:1.)];
    let cutter = Geometry::Point(Point::new(-0., -0.));
    let result = run(&source, &cutter);
    let c = result[0].0[1];
    assert_eq!(
        (c.x.to_bits(), c.y.to_bits()),
        ((-0.0_f64).to_bits(), (-0.0_f64).to_bits())
    );
}

#[test]
#[deny(clippy::disallowed_methods)]
fn excluded_line_cutter_is_typed_unsupported() {
    let source = LineString::from(vec![(0., 0.), (1., 1.)]);
    let cutter = Geometry::Line(geo::Line::new((0., 1.), (1., 0.)));
    assert!(matches!(
        split_line_with_limits(&source, &cutter, RelateLimits::default(), None),
        Err(PredicateError::UnsupportedGeometry)
    ));
}

#[test]
fn resource_prefixes_are_atomic_and_monotone() {
    // Il punto interno 1 divide [0,2] in [0,1] e [1,2].
    let source = line_string![(x:0.,y:0.),(x:2.,y:0.)];
    let cutter = Geometry::Point(Point::new(1.0, 0.0));
    let expected = vec![
        line_string![(x:0.,y:0.),(x:1.,y:0.)],
        line_string![(x:1.,y:0.),(x:2.,y:0.)],
    ];
    // GOP-026/027: esaurimento durante raccolta, noding o pubblicazione
    // non restituisce un prefisso; budget maggiori conservano l'esito.
    for resource in 0..4 {
        let memory = resource == 1;
        let mut succeeded = false;
        for budget in (0..=4096)
            .map(|value| if memory { value * 16 } else { value })
            .chain((17..=26).map(|power| 1_u64 << power))
        {
            if succeeded && budget < (1_u64 << 26) {
                continue;
            }
            let mut limits = RelateLimits::default();
            match resource {
                0 => limits.max_steps = budget,
                1 => limits.max_reserved_bytes = budget,
                2 => limits.max_nodes = budget,
                _ => limits.max_segment_pairs = budget,
            }
            match split_line_with_limits(&source, &cutter, limits, None) {
                Ok(actual) => {
                    assert_eq!(actual, expected);
                    succeeded = true;
                }
                Err(error) => {
                    assert!(!succeeded);
                    if memory {
                        assert!(matches!(error, PredicateError::ReservationLimit));
                    } else {
                        assert!(matches!(error, PredicateError::WorkLimit));
                    }
                }
            }
        }
        assert!(succeeded);
    }
}

#[test]
fn nested_mixed_cutters_have_the_same_cartesian_partition() {
    use geo::{GeometryCollection, MultiPoint, MultiPolygon, Rect};
    let source = line_string![(x:0.,y:0.),(x:12.,y:0.)];
    let cutter = Geometry::GeometryCollection(GeometryCollection(vec![
        Geometry::MultiPoint(MultiPoint(vec![Point::new(3.0, 0.0), Point::new(1.0, 0.0)])),
        Geometry::GeometryCollection(GeometryCollection(vec![
            Geometry::MultiPolygon(MultiPolygon(vec![
                Rect::new((5.0, -1.0), (7.0, 1.0)).to_polygon()
            ])),
            Geometry::MultiLineString(MultiLineString(vec![
                line_string![(x:9.,y:-1.),(x:9.,y:1.)],
            ])),
            Geometry::Point(Point::new(11.0, 0.0)),
        ])),
    ]));
    // GOP-015A e contratto split: nodi ordinati lungo il sorgente,
    // non nell'ordine dei tagli. Il bordo del quadrato taglia in x=5,7.
    let nodes = [0.0, 1.0, 3.0, 5.0, 7.0, 9.0, 11.0, 12.0];
    let expected: Vec<_> = nodes
        .windows(2)
        .map(|p| LineString::from(vec![(p[0], 0.0), (p[1], 0.0)]))
        .collect();
    assert_eq!(run(&source, &cutter), expected);
}

#[cfg(test)]
#[test]
fn degenerate_and_nonfinite_lines_identify_the_operand() {
    let valid = line_string![(x:0.,y:0.),(x:2.,y:0.)];
    // Contratto split: un percorso non vuoto deve contenere due punti
    // distinti; la categoria identifica il lato senza riportare coordinate.
    for invalid in [
        line_string![(x:1.,y:0.)],
        line_string![(x:1.,y:0.),(x:1.,y:0.)],
    ] {
        assert!(matches!(
            split_line_with_limits(
                &invalid,
                &Geometry::LineString(valid.clone()),
                RelateLimits::default(),
                None
            ),
            Err(PredicateError::InvalidGeometry { side: "left", .. })
        ));
        assert!(matches!(
            split_line_with_limits(
                &valid,
                &Geometry::LineString(invalid),
                RelateLimits::default(),
                None
            ),
            Err(PredicateError::InvalidGeometry { side: "right", .. })
        ));
    }
    assert!(matches!(
        split_line_with_limits(
            &line_string![(x:0.,y:f64::NAN),(x:2.,y:0.)],
            &Geometry::Point(Point::new(1.0, 0.0)),
            RelateLimits::default(),
            None
        ),
        Err(PredicateError::NonFiniteCoordinate { side: "left" })
    ));
}
