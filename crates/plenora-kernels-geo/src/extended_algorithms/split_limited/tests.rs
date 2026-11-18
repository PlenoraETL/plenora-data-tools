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
    let expected = split(&source, &cutter, &mut measured).expect("misura");
    // Fonte contabile: 2 estremi sorgente + 2 lama + 2 incidenze; una coppia.
    assert_eq!((measured.pairs, measured.nodes), (1, 6));
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
    // Fonte: Work::step, flag prima della deadline, entrambi errori distinti.
    assert!(matches!(
        split_line_with_limits(&source, &cutter, expired, Some(&flag)),
        Err(PredicateError::Cancelled)
    ));
    assert!(matches!(
        split_line_with_limits(&source, &cutter, expired, None),
        Err(PredicateError::DeadlineExceeded)
    ));
}

#[test]
fn explicit_invalid_and_precision_errors() {
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
    let mut work = Work::new(RelateLimits::default(), None);
    let huge = P::of_in(
        Coord {
            x: 2_f64.powi(50),
            y: 0.,
        },
        &work,
    )
    .expect("razionale");
    let offset = work
        .integer(1)
        .expect("uno")
        .try_div(&work.integer(10).expect("dieci"))
        .expect("decimo");
    let point = P {
        x: huge.x.try_add(&offset).expect("somma"),
        y: huge.y,
    };
    // Fonte: ULP(2^50)=1/4; 2^50+1/10 dista 10 cm dal f64 piu' vicino.
    assert!(matches!(
        coordinate(&point, &mut work),
        Err(PredicateError::CalcoloNonConcluso("precisione del nodo"))
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
