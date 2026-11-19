#![allow(clippy::float_cmp)]
use super::*;
use geo::{Area, Rect};

fn rect(x: f64, y: f64, xx: f64, yy: f64) -> Geometry<f64> {
    Geometry::Polygon(Rect::new((x, y), (xx, yy)).to_polygon())
}
fn run(a: &Geometry<f64>, b: &Geometry<f64>, op: PolygonBooleanOp) -> Geometry<f64> {
    boolean_with_limits(a, b, op, None, PolygonBooleanLimits::default(), None)
        .expect("geometria attesa")
}
fn area(g: &Geometry<f64>) -> f64 {
    g.unsigned_area()
}

/// O-M1, GOP-014/015: due rettangoli 3x2, overlap 1x2.
#[test]
fn rectangles_and_directional_difference() {
    let a = rect(0., 0., 3., 2.);
    let b = rect(2., 0., 5., 2.);
    for (op, expected) in [
        (PolygonBooleanOp::Union, 10.),
        (PolygonBooleanOp::Intersection, 2.),
        (PolygonBooleanOp::Difference, 4.),
    ] {
        assert_eq!(area(&run(&a, &b, op)), expected);
    }
}
/// SFA 1.2.1 6.1.15, GOP-015: contatti di lato/punto, filtro non boundary.
#[test]
fn edge_and_point_contacts() {
    let a = rect(0., 0., 1., 1.);
    assert!(
        matches!(run(&a,&rect(1.,0.,2.,1.),PolygonBooleanOp::Intersection), Geometry::MultiLineString(ref ls) if ls.0.len()==1)
    );
    assert!(
        matches!(run(&a,&rect(1.,1.,2.,2.),PolygonBooleanOp::Intersection), Geometry::MultiPoint(ref ps) if ps.0.len()==1)
    );
    let value = boolean_with_limits(
        &a,
        &a,
        PolygonBooleanOp::Intersection,
        Some(1),
        PolygonBooleanLimits::default(),
        None,
    )
    .unwrap();
    assert!(matches!(value,Geometry::MultiLineString(ref ls) if ls.0.is_empty()));
    assert_eq!(
        area(&run(&a, &rect(1., 1., 2., 2.), PolygonBooleanOp::Union)),
        2.
    );
}
/// O-M1: quadrato 4x4 meno quadrato 2x2, area 12 e un buco.
#[test]
fn holes_nesting_and_identity() {
    let a = rect(0., 0., 4., 4.);
    let b = rect(1., 1., 3., 3.);
    let hole = run(&a, &b, PolygonBooleanOp::Difference);
    assert_eq!(area(&hole), 12.);
    assert!(
        matches!(&hole, Geometry::MultiPolygon(ps) if ps.0.len()==1 && ps.0[0].interiors().len()==1)
    );
    assert_eq!(area(&run(&hole, &b, PolygonBooleanOp::Union)), 16.);
    assert!(
        matches!(run(&hole,&hole,PolygonBooleanOp::Difference),Geometry::GeometryCollection(ref gs) if gs.0.is_empty())
    );
    assert_eq!(
        area(&run(&hole, &hole, PolygonBooleanOp::Intersection)),
        12.
    );
    assert_eq!(area(&run(&a, &b, PolygonBooleanOp::Intersection)), 4.);
}
/// O-M1 voce 21: n strisce orizzontali e m verticali, nm quadrati unitari.
#[test]
fn stripes_combinatorial_components() {
    for (n, m) in [(2, 3), (3, 5)] {
        let h = Geometry::MultiPolygon(MultiPolygon(
            (0..n)
                .map(|i| {
                    Rect::new(
                        (0., f64::from(2 * i)),
                        (f64::from(2 * m), f64::from(2 * i + 1)),
                    )
                    .to_polygon()
                })
                .collect(),
        ));
        let v = Geometry::MultiPolygon(MultiPolygon(
            (0..m)
                .map(|i| {
                    Rect::new(
                        (f64::from(2 * i), 0.),
                        (f64::from(2 * i + 1), f64::from(2 * n)),
                    )
                    .to_polygon()
                })
                .collect(),
        ));
        let result = run(&h, &v, PolygonBooleanOp::Intersection);
        assert_eq!(area(&result), f64::from(n * m));
        assert!(
            matches!(result,Geometry::MultiPolygon(ref ps) if ps.0.len()==usize::try_from(n*m).unwrap())
        );
    }
}

fn triangle(scale: f64, upper: bool) -> Geometry<f64> {
    let coords = if upper {
        vec![(0., 0.), (scale, 0.), (0., scale), (0., 0.)]
    } else {
        vec![(0., -scale), (scale, scale), (scale, -scale), (0., -scale)]
    };
    Geometry::Polygon(Polygon::new(LineString::from(coords), vec![]))
}
/// GOP-015A: x+y=1 e y=2x-1 danno (2/3,1/3); IEEE nearest-even.
#[test]
fn non_dyadic_nodes_and_collapsed_ring_rejection() {
    use geo::CoordsIter;
    let value = run(
        &triangle(1., true),
        &triangle(1., false),
        PolygonBooleanOp::Intersection,
    );
    assert!(value
        .coords_iter()
        .any(|p| p.x == 2. / 3. && p.y == 1. / 3.));
    assert_eq!(area(&value), 1. / 12.);
    // GOP-015A: a scala minima subnormale, 1/3 s e 0 coincidono;
    // il triangolo esatto non vuoto non puo' diventare un segmento pubblicato.
    let s = f64::from_bits(1);
    assert!(matches!(
        intersection_with_limits(
            &triangle(s, true),
            &triangle(s, false),
            PolygonBooleanLimits::default(),
            None
        ),
        Err(PolygonBooleanError::InvalidRoundedTopology)
    ));
}
/// SFA 6.1.11: un buco puo' toccare il guscio in un solo punto;
/// un'unione puo' avere due gusci a contatto puntuale senza interno comune.
#[test]
fn tangent_hole_and_disconnected_shells() {
    let outer = rect(0., 0., 4., 4.);
    let inner = Geometry::Polygon(Polygon::new(
        LineString::from(vec![(0., 2.), (1., 1.), (2., 2.), (1., 3.), (0., 2.)]),
        vec![],
    ));
    let result = run(&outer, &inner, PolygonBooleanOp::Difference);
    assert_eq!(area(&result), 14.);
    assert!(
        matches!(&result,Geometry::MultiPolygon(ps) if ps.0.len()==1 && ps.0[0].interiors().len()==1)
    );
    let a = rect(0., 0., 1., 1.);
    let b = rect(1., 1., 2., 2.);
    let union = run(&a, &b, PolygonBooleanOp::Union);
    assert_eq!(union, run(&b, &a, PolygonBooleanOp::Union));
}
/// GOP-026/027: soglia esatta ammessa, soglia-1 rifiutata sui contatori reali.
#[test]
fn exact_resource_thresholds() {
    let a = rect(0., 0., 3., 2.);
    let b = rect(2., 0., 5., 2.);
    let limits = PolygonBooleanLimits::default();
    let mut work = Work::new(limits.work, None);
    execute(&a, &b, PolygonBooleanOp::Union, None, limits, &mut work).unwrap();
    let measured = [work.steps, work.nodes, work.pairs, work.peak_bytes()];
    for (field, measured) in measured.iter().enumerate() {
        for delta in [-1_i64, 0, 1] {
            let mut bound = limits;
            let v = measured.checked_add_signed(delta).unwrap();
            match field {
                0 => bound.work.max_steps = v,
                1 => bound.work.max_nodes = v,
                2 => bound.work.max_segment_pairs = v,
                _ => bound.work.max_reserved_bytes = v,
            }
            let result = union_with_limits(&a, &b, bound, None);
            if delta < 0 {
                if field == 3 {
                    assert!(matches!(
                        result,
                        Err(PolygonBooleanError::Predicate(
                            PredicateError::ReservationLimit
                        ))
                    ));
                } else {
                    assert!(matches!(
                        result,
                        Err(PolygonBooleanError::Predicate(PredicateError::WorkLimit))
                    ));
                }
            } else {
                result.unwrap();
            }
        }
    }
    for (edges, faces, rings, ok) in [
        (12, 1, 1, true),
        (11, 1, 1, false),
        (12, 0, 1, false),
        (12, 1, 0, false),
    ] {
        let result = union_with_limits(
            &a,
            &b,
            PolygonBooleanLimits {
                max_edges: edges,
                max_faces: faces,
                max_rings: rings,
                ..limits
            },
            None,
        );
        if ok {
            result.unwrap();
        } else {
            assert!(matches!(
                result,
                Err(PolygonBooleanError::Predicate(PredicateError::WorkLimit))
            ));
        }
    }
}
/// GOP-028: timeout prevale sul token nello stesso checkpoint; regola condivisa da tutte le API con limiti.
#[test]
fn deadline_precedes_cancellation_and_invalid_inputs_are_typed() {
    let flag = AtomicBool::new(true);
    let a = rect(0., 0., 1., 1.);
    let mut limits = PolygonBooleanLimits::default();
    assert!(matches!(
        union_with_limits(&a, &a, limits, Some(&flag)),
        Err(PolygonBooleanError::Predicate(PredicateError::Cancelled))
    ));
    limits.work.deadline = Some(std::time::Instant::now());
    assert!(matches!(
        union_with_limits(&a, &a, limits, Some(&flag)),
        Err(PolygonBooleanError::Predicate(
            PredicateError::DeadlineExceeded
        ))
    ));
    assert!(matches!(
        boolean_with_limits(
            &a,
            &a,
            PolygonBooleanOp::Union,
            Some(3),
            PolygonBooleanLimits::default(),
            None
        ),
        Err(PolygonBooleanError::InvalidOutputDimension)
    ));
    assert!(matches!(
        union_with_limits(
            &Geometry::Point(Point::new(0., 0.)),
            &a,
            PolygonBooleanLimits::default(),
            None
        ),
        Err(PolygonBooleanError::UnsupportedOperands)
    ));
    let invalid = Geometry::Polygon(Polygon::new(
        LineString::from(vec![(0., 0.), (1., 1.), (1., 0.), (0., 1.), (0., 0.)]),
        vec![],
    ));
    assert!(matches!(
        union_with_limits(&invalid, &a, PolygonBooleanLimits::default(), None),
        Err(PolygonBooleanError::Predicate(
            PredicateError::InvalidGeometry { .. }
        ))
    ));
}

// Oracolo autonomo: coordinate razionali con denominatore 4, celle unitarie
// selezionate per appartenenza agli intervalli. Non chiama il noder o Shape.
fn occupied(x: i32, y: i32, bounds: [i32; 4]) -> bool {
    4 * bounds[0] < x && x < 4 * bounds[2] && 4 * bounds[1] < y && y < 4 * bounds[3]
}
fn oracle_select(op: PolygonBooleanOp, a: bool, b: bool) -> bool {
    match op {
        PolygonBooleanOp::Union => a || b,
        PolygonBooleanOp::Intersection => a && b,
        PolygonBooleanOp::Difference => a && !b,
    }
}
fn oracle_position(x: i32, y: i32, membership: impl Fn(i32, i32) -> bool) -> usize {
    let quadrants = [(-1, -1), (-1, 1), (1, -1), (1, 1)].map(|(dx, dy)| membership(x + dx, y + dy));
    if quadrants.iter().all(|v| *v) {
        0
    } else if quadrants.iter().any(|v| *v) {
        1
    } else {
        2
    }
}
fn oracle_matrix(a: [i32; 4], b: [i32; 4], op: PolygonBooleanOp) -> [u8; 9] {
    let mut matrix = [b'F'; 9];
    for x in -4..=12 {
        for y in -4..=12 {
            let rp = oracle_position(2 * x, 2 * y, |x, y| {
                oracle_select(op, occupied(x, y, a), occupied(x, y, b))
            });
            let ap = oracle_position(2 * x, 2 * y, |x, y| occupied(x, y, a));
            let dimension = b'0' + u8::from(x % 2 != 0) + u8::from(y % 2 != 0);
            let cell = &mut matrix[3 * rp + ap];
            if *cell == b'F' || *cell < dimension {
                *cell = dimension;
            }
        }
    }
    matrix
}
/// O-M1 e SFA 6.1.15: oracolo razionale a celle, area e tutte le nove celle
/// DE-9IM risultato/operando; gli attesi non usano alcun kernel geometrico.
#[test]
fn independent_rational_cell_oracle_area_and_de9im() {
    let a = [0, 0, 3, 3];
    for b in [[1, 1, 4, 4], [-1, 1, 2, 2], [1, 1, 2, 2], [0, 0, 3, 3]] {
        let ga = rect(
            f64::from(a[0]),
            f64::from(a[1]),
            f64::from(a[2]),
            f64::from(a[3]),
        );
        let gb = rect(
            f64::from(b[0]),
            f64::from(b[1]),
            f64::from(b[2]),
            f64::from(b[3]),
        );
        for op in [
            PolygonBooleanOp::Union,
            PolygonBooleanOp::Intersection,
            PolygonBooleanOp::Difference,
        ] {
            let result = run(&ga, &gb, op);
            let mut expected_area = 0;
            for x in -2..6 {
                for y in -2..6 {
                    expected_area += i32::from(oracle_select(
                        op,
                        occupied(4 * x + 2, 4 * y + 2, a),
                        occupied(4 * x + 2, 4 * y + 2, b),
                    ));
                }
            }
            assert_eq!(area(&result), f64::from(expected_area));
            let expected = oracle_matrix(a, b, op);
            assert!(geo::Relate::relate(&result, &ga)
                .matches(std::str::from_utf8(&expected).unwrap())
                .unwrap());
            // AGENTS 3: percorso indicizzato contro enumerazione generica.
            let generic = crate::predicates::arrangement_exhaustive::relate(
                &result,
                &ga,
                &mut Work::new(RelateLimits::default(), None),
            )
            .unwrap();
            assert_eq!(generic.cells(), expected);
            let general = crate::general_boolean::boolean_with_limits(
                &ga,
                &gb,
                op,
                None,
                PolygonBooleanLimits::default(),
                None,
            )
            .unwrap();
            assert!(geo::Relate::relate(&general, &ga)
                .matches(std::str::from_utf8(&expected).unwrap())
                .unwrap());
            assert_eq!(
                rational_twice_area(&general).cmp(&Fraction::integer(2 * expected_area)),
                Ordering::Equal
            );
        }
    }
}

// Frazioni indipendenti solo nei test: nessun riuso di Q del kernel.
#[derive(Clone, Debug)]
struct Fraction {
    n: num_bigint::BigInt,
    d: num_bigint::BigInt,
}

fn rational_twice_area(g: &Geometry<f64>) -> Fraction {
    let Geometry::MultiPolygon(ps) = g else {
        assert_eq!(g.coords_count(), 0);
        return Fraction::integer(0);
    };
    let mut result = Fraction::integer(0);
    for p in &ps.0 {
        for ring in std::iter::once(p.exterior()).chain(p.interiors()) {
            for pair in ring.0.windows(2) {
                let cross = Fraction::float(pair[0].x)
                    .mul(&Fraction::float(pair[1].y))
                    .sub(&Fraction::float(pair[1].x).mul(&Fraction::float(pair[0].y)));
                result = result.add(&cross);
            }
        }
    }
    result
}
use geo::CoordsIter;
impl Fraction {
    fn new(n: num_bigint::BigInt, d: num_bigint::BigInt) -> Self {
        assert_ne!(d, num_bigint::BigInt::from(0));
        let (n, d) = if d < num_bigint::BigInt::from(0) {
            (-n, -d)
        } else {
            (n, d)
        };
        Self { n, d }
    }
    fn integer(n: i32) -> Self {
        Self::new(n.into(), 1.into())
    }
    fn float(v: f64) -> Self {
        let bits = v.to_bits();
        let exp = i32::try_from((bits >> 52) & 2047).unwrap();
        let mantissa = (bits & ((1_u64 << 52) - 1)) | if exp == 0 { 0 } else { 1_u64 << 52 };
        let shift = if exp == 0 { -1074 } else { exp - 1075 };
        let n = num_bigint::BigInt::from(mantissa) * if bits >> 63 == 0 { 1 } else { -1 };
        if shift >= 0 {
            Self::new(n << shift.unsigned_abs(), 1.into())
        } else {
            Self::new(n, num_bigint::BigInt::from(1) << shift.unsigned_abs())
        }
    }
    fn add(&self, b: &Self) -> Self {
        Self::new(&self.n * &b.d + &b.n * &self.d, &self.d * &b.d)
    }
    fn sub(&self, b: &Self) -> Self {
        Self::new(&self.n * &b.d - &b.n * &self.d, &self.d * &b.d)
    }
    fn mul(&self, b: &Self) -> Self {
        Self::new(&self.n * &b.n, &self.d * &b.d)
    }
    fn cmp(&self, b: &Self) -> Ordering {
        (&self.n * &b.d).cmp(&(&b.n * &self.d))
    }
}
/// GOP-015A: certificato indipendente nearest-even mediante i punti medi
/// dei binary64 adiacenti; area razionale del triangolo esatto pari a 1/12.
#[test]
fn independent_fraction_certificate_for_constructed_triangle() {
    use geo::CoordsIter;
    let result = run(
        &triangle(1., true),
        &triangle(1., false),
        PolygonBooleanOp::Intersection,
    );
    let exact = [
        Fraction::new(2.into(), 3.into()),
        Fraction::new(1.into(), 3.into()),
    ];
    let c = result.coords_iter().find(|p| p.y > 0.).unwrap();
    for (published, exact) in [c.x, c.y].into_iter().zip(exact) {
        let twice = exact.add(&exact);
        let value = Fraction::float(published);
        let lo = Fraction::float(f64::from_bits(published.to_bits() - 1));
        let hi = Fraction::float(f64::from_bits(published.to_bits() + 1));
        assert!(twice.cmp(&lo.add(&value)).is_gt());
        assert!(twice.cmp(&hi.add(&value)).is_lt());
    }
    let Geometry::MultiPolygon(ps) = result else {
        panic!("tipo atteso")
    };
    let mut twice = Fraction::integer(0);
    for pair in ps.0[0].exterior().0.windows(2) {
        let product = Fraction::float(pair[0].x).mul(&Fraction::float(pair[1].y));
        let other = Fraction::float(pair[1].x).mul(&Fraction::float(pair[0].y));
        twice = twice.add(&product.sub(&other));
    }
    let expected = Fraction::new(1.into(), 6.into());
    // Errore del doppio dell'area = (1/2) errore di y sul vertice (2/3,1/3).
    let error = twice.sub(&expected);
    let bound = Fraction::new(1.into(), num_bigint::BigInt::from(1) << 56_u32);
    assert!(error.cmp(&bound).is_lt());
    assert!(error.cmp(&Fraction::integer(0).sub(&bound)).is_gt());
}

/// GOP-028/O-M3: il contatore dimostra lavoro gia' svolto quando il token
/// viene alzato o scade la deadline; nessuna uscita parziale.
#[test]
fn cancellation_and_deadline_during_work() {
    let polygon = Geometry::MultiPolygon(MultiPolygon(
        (0..100)
            .map(|i| Rect::new((f64::from(i * 2), 0.), (f64::from(i * 2 + 1), 1.)).to_polygon())
            .collect(),
    ));
    let flag = AtomicBool::new(false);
    std::thread::scope(|scope| {
        scope.spawn(|| {
            std::thread::sleep(std::time::Duration::from_millis(20));
            flag.store(true, std::sync::atomic::Ordering::Relaxed);
        });
        let limits = PolygonBooleanLimits::default();
        let mut work = Work::new(limits.work, Some(&flag));
        let result = execute(
            &polygon,
            &polygon,
            PolygonBooleanOp::Union,
            None,
            limits,
            &mut work,
        );
        assert!(matches!(
            result,
            Err(PolygonBooleanError::Predicate(PredicateError::Cancelled))
        ));
        assert!(work.steps > 0);
    });
    let mut limits = PolygonBooleanLimits::default();
    limits.work.deadline = Some(std::time::Instant::now() + std::time::Duration::from_millis(20));
    let mut work = Work::new(limits.work, None);
    assert!(matches!(
        execute(
            &polygon,
            &polygon,
            PolygonBooleanOp::Union,
            None,
            limits,
            &mut work
        ),
        Err(PolygonBooleanError::Predicate(
            PredicateError::DeadlineExceeded
        ))
    ));
    assert!(work.steps > 0);
}
/// Ordine canonico dichiarato: anche gusci con minimo condiviso sono
/// invarianti allo scambio degli operandi e al verso degli anelli d'ingresso.
#[test]
fn canonical_shared_minimum_and_reversed_inputs() {
    let a = Geometry::Polygon(Polygon::new(
        LineString::from(vec![(0., 0.), (2., 1.), (1., 2.), (0., 0.)]),
        vec![],
    ));
    let mut b = Geometry::Polygon(Polygon::new(
        LineString::from(vec![(0., 0.), (1., -2.), (2., -1.), (0., 0.)]),
        vec![],
    ));
    let expected = run(&a, &b, PolygonBooleanOp::Union);
    assert_eq!(expected, run(&b, &a, PolygonBooleanOp::Union));
    if let Geometry::Polygon(p) = &mut b {
        p.exterior_mut(|ring| ring.0.reverse());
    }
    assert_eq!(expected, run(&b, &a, PolygonBooleanOp::Union));
}

/// GOP-028: interruzione deterministica a checkpoint distribuiti nell'intera
/// chiamata, inclusi assemblaggio e ultima verifica prima della pubblicazione.
#[test]
fn deterministic_interruptions_across_the_call() {
    let a = rect(0., 0., 3., 2.);
    let b = rect(2., 0., 5., 2.);
    let limits = PolygonBooleanLimits::default();
    let mut measure = Work::new(limits.work, None);
    execute(&a, &b, PolygonBooleanOp::Union, None, limits, &mut measure).unwrap();
    for at in [
        1,
        measure.steps / 4,
        measure.steps / 2,
        3 * measure.steps / 4,
        measure.steps - 1,
    ] {
        for deadline in [false, true] {
            let flag = AtomicBool::new(false);
            let mut work = Work::new(limits.work, Some(&flag));
            work.interrupt_at = Some((at, deadline));
            let result = execute(&a, &b, PolygonBooleanOp::Union, None, limits, &mut work);
            if deadline {
                assert!(matches!(
                    result,
                    Err(PolygonBooleanError::Predicate(
                        PredicateError::DeadlineExceeded
                    ))
                ));
            } else {
                assert!(matches!(
                    result,
                    Err(PolygonBooleanError::Predicate(PredicateError::Cancelled))
                ));
            }
            assert_eq!(work.steps, at);
        }
    }
}
/// GOP-014/015: dimensioni crescenti e nessuna estrazione dei confini;
/// GOP-015A certifica solo le componenti effettivamente pubblicate.
#[test]
fn mixed_dimensions_empty_and_filter_before_publication() {
    let a = Geometry::MultiPolygon(MultiPolygon(vec![
        Rect::new((0., 0.), (2., 2.)).to_polygon(),
        Rect::new((4., 0.), (5., 1.)).to_polygon(),
        Rect::new((7., 0.), (8., 1.)).to_polygon(),
    ]));
    let b = Geometry::MultiPolygon(MultiPolygon(vec![
        Rect::new((1., 0.), (3., 2.)).to_polygon(),
        Rect::new((5., 0.), (6., 1.)).to_polygon(),
        Rect::new((8., 1.), (9., 2.)).to_polygon(),
    ]));
    let result = run(&a, &b, PolygonBooleanOp::Intersection);
    let Geometry::GeometryCollection(gs) = result else {
        panic!("geometria attesa")
    };
    assert_eq!(gs.0.len(), 3);
    assert!(matches!(&gs.0[0],Geometry::MultiPoint(ps) if ps.0.len()==1));
    assert!(matches!(&gs.0[1],Geometry::MultiLineString(ls) if ls.0.len()==1));
    assert!(matches!(&gs.0[2],Geometry::MultiPolygon(ps) if ps.0.len()==1));
    assert_eq!(area(&gs.0[2]), 2.);
    let s = f64::from_bits(1);
    let value = boolean_with_limits(
        &triangle(s, true),
        &triangle(s, false),
        PolygonBooleanOp::Intersection,
        Some(0),
        PolygonBooleanLimits::default(),
        None,
    )
    .unwrap();
    assert!(matches!(value,Geometry::MultiPoint(ps) if ps.0.is_empty()));
    let empty = Geometry::Polygon(Polygon::empty());
    assert!(
        matches!(run(&empty,&empty,PolygonBooleanOp::Union),Geometry::GeometryCollection(gs) if gs.0.is_empty())
    );
}
/// GOP-015A: un output valido ma traslato non corrisponde al certificato;
/// la sola validita' OGC non autorizza la pubblicazione.
#[test]
fn serialized_surface_is_bound_to_the_certificate() {
    let input = rect(0., 0., 2., 2.);
    let mut work = Work::new(RelateLimits::default(), None);
    let originals = Originals::new(&input, &input, &mut work).unwrap();
    let mut shape = Shape::default();
    shape.geometry(&input, &mut work).unwrap();
    let mut published = CountedVec::new();
    for edge in shape.edges() {
        let coords = [
            originals.coordinate(&edge.a, &mut work).unwrap(),
            originals.coordinate(&edge.b, &mut work).unwrap(),
        ];
        let mut value = PublishedEdge::new(edge, coords, &mut work).unwrap();
        value.surface = true;
        work.push(&mut published, value).unwrap();
    }
    let wrong = Geometry::MultiPolygon(MultiPolygon(vec![
        Rect::new((3., 3.), (5., 5.)).to_polygon()
    ]));
    assert!(matches!(
        line_boolean::verify_publication(&published, &originals, false, Some(&wrong), &mut work),
        Err(line_boolean::LineBooleanError::InvalidRoundedTopology)
    ));
}

fn comb(n: i32, length: i32, transpose: bool) -> Geometry<f64> {
    let mut points = vec![(-2., 0.)];
    for i in 0..n {
        if i > 0 {
            points.push((-1., f64::from(2 * i)));
        }
        points.push((f64::from(length), f64::from(2 * i)));
        points.push((f64::from(length), f64::from(2 * i + 1)));
        points.push((-1., f64::from(2 * i + 1)));
    }
    points.push((-2., f64::from(2 * n - 1)));
    points.push((-2., 0.));
    if transpose {
        for p in &mut points {
            std::mem::swap(&mut p.0, &mut p.1);
        }
    }
    Geometry::Polygon(Polygon::new(LineString::from(points), vec![]))
}
/// O-M1 voce 21: due soli Polygon a pettine con dorsi esterni disgiunti;
/// n denti per m denti producono nm quadrati, area nm, nm facce e anelli.
#[test]
fn polygon_combs_have_independent_combinatorial_counts() {
    for (n, m) in [(2, 3), (3, 5)] {
        let a = comb(n, 2 * m, false);
        let b = comb(m, 2 * n, true);
        let count = u64::try_from(n * m).unwrap();
        let limits = PolygonBooleanLimits {
            max_faces: count,
            max_rings: count,
            ..PolygonBooleanLimits::default()
        };
        let result = intersection_with_limits(&a, &b, limits, None).unwrap();
        assert_eq!(area(&result), f64::from(n * m));
        assert!(
            matches!(result,Geometry::MultiPolygon(ps) if ps.0.len()==usize::try_from(count).unwrap())
        );
        assert!(matches!(
            intersection_with_limits(
                &a,
                &b,
                PolygonBooleanLimits {
                    max_faces: count - 1,
                    ..limits
                },
                None
            ),
            Err(PolygonBooleanError::Predicate(PredicateError::WorkLimit))
        ));
        assert!(matches!(
            intersection_with_limits(
                &a,
                &b,
                PolygonBooleanLimits {
                    max_rings: count - 1,
                    ..limits
                },
                None
            ),
            Err(PolygonBooleanError::Predicate(PredicateError::WorkLimit))
        ));
    }
}

#[test]
#[deny(clippy::disallowed_methods)]
fn multipolygon_point_contact_is_supported() {
    use wkt::TryFromWkt;
    let input =
        Geometry::try_from_wkt_str("MULTIPOLYGON(((0 0,1 0,1 1,0 1,0 0)),((1 1,2 1,2 2,1 2,1 1)))")
            .unwrap();
    let result = run(
        &input,
        &rect(-1., -1., 3., 3.),
        PolygonBooleanOp::Intersection,
    );
    // OGC: due quadrati unitari con interni disgiunti e un vertice comune.
    assert_eq!(area(&result), 2.0);
    assert!(matches!(result, Geometry::MultiPolygon(ref ps) if ps.0.len() == 2));
}

#[test]
fn indexed_successor_matches_exhaustive_rays() {
    let mut work = Work::new(RelateLimits::default(), None);
    let mut edges = Vec::new();
    for (x, y) in [
        (1., 0.),
        (2., 1.),
        (1., 2.),
        (0., 1.),
        (-1., 2.),
        (-2., 1.),
        (-1., 0.),
        (-2., -1.),
        (-1., -2.),
        (0., -1.),
        (1., -2.),
        (2., -1.),
    ] {
        edges.push(Edge {
            a: P::of_in((0., 0.).into(), &work).expect("origin"),
            b: P::of_in((x, y).into(), &work).expect("ray"),
        });
    }
    let mut outgoing: Vec<_> = (0..edges.len()).rev().collect();
    crate::noding::sorted(
        &mut outgoing,
        |a, b| angle_cmp(&edges[*a].b, &edges[*b].b),
        &mut work,
    )
    .expect("sort");
    for i in 0..edges.len() {
        let current = edges.len();
        edges.push(Edge {
            a: edges[i].b.try_clone().expect("ray"),
            b: edges[i].a.try_clone().expect("origin"),
        });
        let incoming = &edges[current].a;
        let mut before: Option<usize> = None;
        let mut last: Option<usize> = None;
        for &id in outgoing.iter().rev() {
            if angle_cmp(&edges[id].b, incoming).expect("angle").is_lt()
                && before
                    .is_none_or(|p| angle_cmp(&edges[p].b, &edges[id].b).expect("angle").is_lt())
            {
                before = Some(id);
            }
            if last.is_none_or(|p| angle_cmp(&edges[p].b, &edges[id].b).expect("angle").is_lt()) {
                last = Some(id);
            }
        }
        assert_eq!(
            successor(current, &edges, &outgoing, &mut work).expect("successor"),
            before.or(last).expect("ray")
        );
        edges.pop();
    }
}

#[test]
fn resource_prefixes_cannot_fill_a_difference_hole() {
    use geo::Relate;
    let a = rect(0.0, 0.0, 4.0, 4.0);
    let b = rect(1.0, 1.0, 3.0, 3.0);
    let expected = Geometry::Polygon(Polygon::new(
        Rect::new((0.0, 0.0), (4.0, 4.0))
            .to_polygon()
            .exterior()
            .clone(),
        vec![Rect::new((1.0, 1.0), (3.0, 3.0))
            .to_polygon()
            .exterior()
            .clone()],
    ));
    // O1 SFA 6.1.15: differenza di quadrati annidati, area 16-4=12,
    // stesso interno/bordo dell'anello quadrato (oracolo geo::Relate).
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
            let mut limits = PolygonBooleanLimits::default();
            match resource {
                0 => limits.work.max_steps = budget,
                1 => limits.work.max_reserved_bytes = budget,
                2 => limits.work.max_nodes = budget,
                _ => limits.work.max_segment_pairs = budget,
            }
            match boolean_with_limits(&a, &b, PolygonBooleanOp::Difference, None, limits, None) {
                Ok(actual) => {
                    assert_eq!(actual.unsigned_area(), 12.0);
                    assert!(actual.relate(&expected).matches("**F**FFF*").unwrap());
                    succeeded = true;
                }
                Err(PolygonBooleanError::Predicate(error)) => {
                    // GOP-026/027: anche l'assemblaggio dei buchi e' atomico.
                    assert!(!succeeded);
                    if memory {
                        assert!(matches!(error, PredicateError::ReservationLimit));
                    } else {
                        assert!(matches!(error, PredicateError::WorkLimit));
                    }
                }
                Err(error) => panic!("categoria inattesa: {error:?}"),
            }
        }
        assert!(succeeded);
    }
}
