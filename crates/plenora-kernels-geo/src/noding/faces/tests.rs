//! GOP-020: grafo razionale, cicli esaustivi indipendenti e provenienza.
use super::*;
use crate::noding::oracle;
use crate::noding::Q;
use crate::predicates::RelateLimits;

#[derive(Clone, Eq, PartialEq, Ord, PartialOrd)]
struct OracleSite {
    x: oracle::Q,
    y: oracle::Q,
}

fn site(x: i32, y: i32, denominator: i32) -> OracleSite {
    OracleSite {
        x: oracle::Q::int(x).div(&oracle::Q::int(denominator)),
        y: oracle::Q::int(y).div(&oracle::Q::int(denominator)),
    }
}

fn cross(a: &OracleSite, b: &OracleSite) -> oracle::Q {
    a.x.mul(&b.y).sub(&a.y.mul(&b.x))
}

fn midpoint(a: &OracleSite, b: &OracleSite) -> OracleSite {
    OracleSite {
        x: a.x.midpoint(&b.x),
        y: a.y.midpoint(&b.y),
    }
}

fn strictly_inside(p: &OracleSite, cycle: &[usize], sites: &[OracleSite]) -> bool {
    let mut crossings = 0;
    for i in 0..cycle.len() {
        let a = &sites[cycle[i]];
        let b = &sites[cycle[(i + 1) % cycle.len()]];
        let dx = b.x.sub(&a.x);
        let dy = b.y.sub(&a.y);
        let px = p.x.sub(&a.x);
        let py = p.y.sub(&a.y);
        if dx.mul(&py) == dy.mul(&px)
            && p.x >= a.x.clone().min(b.x.clone())
            && p.x <= a.x.clone().max(b.x.clone())
            && p.y >= a.y.clone().min(b.y.clone())
            && p.y <= a.y.clone().max(b.y.clone())
        {
            return false;
        }
        if (a.y > p.y) != (b.y > p.y) && a.x.add(&dx.mul(&py).div(&dy)) > p.x {
            crossings += 1;
        }
    }
    crossings % 2 == 1
}

// Oracolo delle fixture: elimina i lati non ciclici e distingue componenti
// disconnesse; un ciclo minimo non contiene lati ciclici della sua componente.
// Enumera tutti i cammini semplici, senza ordinamento angolare o half-edge.
fn oracle_cycles(sites: &[OracleSite], edges: &[(usize, usize)]) -> Vec<Vec<usize>> {
    let mut found = Vec::new();
    for start in 0..sites.len() {
        let mut pending = vec![vec![start]];
        while let Some(path) = pending.pop() {
            let tail = *path.last().expect("cammino non vuoto");
            for &(a, b) in edges {
                let next = if a == tail {
                    b
                } else if b == tail {
                    a
                } else {
                    continue;
                };
                if next == start && path.len() >= 3 {
                    let mut area = oracle::Q::int(0);
                    for i in 0..path.len() {
                        area =
                            area.add(&cross(&sites[path[i]], &sites[path[(i + 1) % path.len()]]));
                    }
                    if area.sign().is_gt() {
                        found.push(path.clone());
                    }
                } else if next > start && !path.contains(&next) {
                    let mut extended = path.clone();
                    extended.push(next);
                    pending.push(extended);
                }
            }
        }
    }
    found.sort();
    found.dedup();
    let cyclic: Vec<_> = edges
        .iter()
        .copied()
        .filter(|&(a, b)| {
            found.iter().any(|cycle| {
                (0..cycle.len()).any(|i| {
                    let (u, v) = (cycle[i], cycle[(i + 1) % cycle.len()]);
                    (a, b) == (u, v) || (a, b) == (v, u)
                })
            })
        })
        .collect();
    found
        .into_iter()
        .filter(|cycle| {
            let mut component = cycle.clone();
            let mut at = 0;
            while at < component.len() {
                for &(a, b) in &cyclic {
                    if a == component[at] && !component.contains(&b) {
                        component.push(b);
                    }
                    if b == component[at] && !component.contains(&a) {
                        component.push(a);
                    }
                }
                at += 1;
            }
            !cyclic.iter().any(|&(u, v)| {
                component.contains(&u)
                    && strictly_inside(&midpoint(&sites[u], &sites[v]), cycle, sites)
            })
        })
        .collect()
}

fn input(
    lines: &[[(f64, f64); 2]],
    work: &mut Work<'_>,
) -> Result<CountedVec<Edge>, PredicateError> {
    let mut result = CountedVec::new();
    for &[a, b] in lines {
        let edge = Edge {
            a: P::of_in(a.into(), work)?,
            b: P::of_in(b.into(), work)?,
        };
        work.push(&mut result, edge)?;
    }
    Ok(result)
}

fn run(lines: &[[(f64, f64); 2]], work: &mut Work<'_>) -> Result<FaceGraph, PredicateError> {
    let edges = input(lines, work)?;
    let mut refs = CountedVec::new();
    for edge in &edges {
        work.push(&mut refs, edge)?;
    }
    let nodes = crate::noding::node(&refs, work)?;
    enumerate(&nodes, work)
}

fn exact(value: &oracle::Q, work: &Work<'_>) -> Q {
    let n = value.n.to_string().parse::<i32>().expect("fixture piccola");
    let d = value.d.to_string().parse::<i32>().expect("fixture piccola");
    work.integer(n)
        .expect("fixture numeratore")
        .try_div(&work.integer(d).expect("fixture denominatore"))
        .expect("fixture razionale")
}

fn compare_oracle(lines: &[[(f64, f64); 2]], sites: &[OracleSite], edges: &[(usize, usize)]) {
    let mut work = Work::new(RelateLimits::default(), None);
    let actual = run(lines, &mut work).expect("fixture facce");
    let expected_sites: Vec<_> = sites
        .iter()
        .map(|p| P {
            x: exact(&p.x, &work),
            y: exact(&p.y, &work),
        })
        .collect();
    // Fonte: intersezioni razionali della fixture, indipendenti dal noder.
    assert_eq!(&*actual.points, expected_sites);
    let mut actual_edges: Vec<_> = actual.atoms.iter().map(|e| (e.from, e.to)).collect();
    let mut expected_edges = edges.to_vec();
    actual_edges.sort_unstable();
    expected_edges.sort_unstable();
    assert_eq!(actual_edges, expected_edges);
    let mut actual_cycles = Vec::new();
    for ring in &actual.rings {
        // Fonte: ogni lato del cammino collega esattamente i vertici contigui.
        assert_eq!(ring.vertices.len(), ring.atoms.len());
        for i in 0..ring.vertices.len() {
            let a = ring.vertices[i];
            let b = ring.vertices[(i + 1) % ring.vertices.len()];
            let atom = &actual.atoms[ring.atoms[i]];
            assert_eq!((atom.from, atom.to), (a.min(b), a.max(b)));
        }
        if ring.orientation.is_gt() {
            let mut cycle = ring.vertices.to_vec();
            let minimum = cycle
                .iter()
                .enumerate()
                .min_by_key(|(_, v)| *v)
                .expect("fixture ciclo")
                .0;
            cycle.rotate_left(minimum);
            actual_cycles.push(cycle);
        }
    }
    actual_cycles.sort();
    // Fonte GOP-020: enumerazione esaustiva separata dei cicli vuoti.
    assert_eq!(actual_cycles, oracle_cycles(sites, edges));
}

#[test]
fn exhaustive_rational_faces() {
    // Fonte: y=x e y=1-x/2 si incontrano in (2/3,2/3).
    let lines = [
        [(0., 0.), (2., 0.)],
        [(2., 0.), (2., 2.)],
        [(2., 2.), (0., 2.)],
        [(0., 2.), (0., 0.)],
        [(0., 0.), (2., 2.)],
        [(0., 1.), (2., 0.)],
    ];
    let sites = [
        site(0, 0, 1),
        site(0, 1, 1),
        site(0, 2, 1),
        site(2, 2, 3),
        site(2, 0, 1),
        site(2, 2, 1),
    ];
    let edges = [
        (0, 1),
        (0, 3),
        (0, 4),
        (1, 2),
        (1, 3),
        (2, 5),
        (3, 4),
        (3, 5),
        (4, 5),
    ];
    compare_oracle(&lines, &sites, &edges);
    let reversed: Vec<_> = lines.iter().rev().map(|&[a, b]| [b, a]).collect();
    compare_oracle(&reversed, &sites, &edges);
}

#[test]
fn provenance_survives_overlaps_reversal_and_point_contacts() {
    let lines = [
        [(0., 0.), (3., 0.)],
        [(2., 0.), (1., 0.)],
        [(1., 0.), (2., 0.)],
        [(1., 0.), (1., 1.)],
    ];
    let mut work = Work::new(RelateLimits::default(), None);
    let graph = run(&lines, &mut work).expect("fixture provenienza");
    let provenance: Vec<_> = graph
        .atoms
        .iter()
        .map(|e| ((e.from, e.to), e.sources.to_vec()))
        .collect();
    // Fonte GOP-020: overlap positivo conserva le tre sorgenti; il contatto
    // puntuale della verticale non le attribuisce al lato orizzontale.
    assert_eq!(
        provenance,
        [
            ((0, 1), vec![0]),
            ((1, 2), vec![3]),
            ((1, 3), vec![0, 1, 2]),
            ((3, 4), vec![0])
        ]
    );
    assert!(graph.rings.is_empty());
}

#[test]
fn residual_edges_are_not_ring_edges() {
    let lines = [
        [(0., 0.), (2., 0.)],
        [(2., 0.), (2., 2.)],
        [(2., 2.), (0., 2.)],
        [(0., 2.), (0., 0.)],
        [(0., 0.), (1., 1.)],
        [(2., 2.), (3., 3.)],
    ];
    let mut work = Work::new(RelateLimits::default(), None);
    let graph = run(&lines, &mut work).expect("fixture residui");
    // Fonte GOP-020: i due alberi non delimitano facce; entrambi i versi
    // del bordo quadrato conservano solo le quattro sorgenti del quadrato.
    assert_eq!(graph.rings.len(), 2);
    for ring in &graph.rings {
        assert_eq!(ring.atoms.len(), 4);
        for &atom in &ring.atoms {
            assert!(graph.atoms[atom].sources.iter().all(|&s| s < 4));
        }
    }
}

#[test]
fn provenance_growth_has_exact_resource_thresholds() {
    for lines in [
        vec![[(0., 0.), (3., 0.)], [(1., 0.), (2., 0.)]],
        vec![
            [(0., 0.), (2., 0.)],
            [(2., 0.), (0., 2.)],
            [(0., 2.), (0., 0.)],
        ],
    ] {
        let mut measured = Work::new(RelateLimits::default(), None);
        drop(run(&lines, &mut measured).expect("fixture misura"));
        let exact = RelateLimits {
            max_steps: measured.steps,
            max_nodes: measured.nodes,
            max_segment_pairs: measured.pairs,
            max_reserved_bytes: measured.peak_bytes(),
            deadline: None,
        };
        // Fonte GOP-026/027: consumo esatto ammesso, unita' mancante rifiutata.
        for extra in [0, 1] {
            let limits = RelateLimits {
                max_steps: exact.max_steps + extra,
                max_nodes: exact.max_nodes + extra,
                max_segment_pairs: exact.max_segment_pairs + extra,
                max_reserved_bytes: exact.max_reserved_bytes + extra,
                ..exact
            };
            drop(run(&lines, &mut Work::new(limits, None)).expect("soglia ammessa"));
        }
        for limits in [
            RelateLimits {
                max_steps: exact.max_steps - 1,
                ..exact
            },
            RelateLimits {
                max_nodes: exact.max_nodes - 1,
                ..exact
            },
            RelateLimits {
                max_segment_pairs: exact.max_segment_pairs - 1,
                ..exact
            },
        ] {
            assert!(matches!(
                run(&lines, &mut Work::new(limits, None)),
                Err(PredicateError::WorkLimit)
            ));
        }
        assert!(matches!(
            run(
                &lines,
                &mut Work::new(
                    RelateLimits {
                        max_reserved_bytes: exact.max_reserved_bytes - 1,
                        ..exact
                    },
                    None
                )
            ),
            Err(PredicateError::ReservationLimit)
        ));
    }
}

#[test]
fn nested_components_keep_hole_provenance() {
    let lines = [
        [(0., 0.), (6., 0.)],
        [(6., 0.), (6., 6.)],
        [(6., 6.), (0., 6.)],
        [(0., 6.), (0., 0.)],
        [(2., 2.), (4., 2.)],
        [(4., 2.), (4., 4.)],
        [(4., 4.), (2., 4.)],
        [(2., 4.), (2., 2.)],
        [(0., 0.), (2., 2.)],
    ];
    let mut work = Work::new(RelateLimits::default(), None);
    let graph = run(&lines, &mut work).expect("anelli annidati");
    let shells: Vec<_> = graph
        .rings
        .iter()
        .enumerate()
        .filter(|(_, r)| r.orientation.is_gt())
        .collect();
    // Fonte: quadrati annidati di aree 36 e 4; il ponte non divide la corona.
    assert_eq!(shells.len(), 2);
    assert_eq!(graph.holes.iter().map(|h| h.len()).sum::<usize>(), 1);
    let (outer, _) = shells
        .iter()
        .find(|(_, r)| {
            r.vertices
                .iter()
                .any(|&v| graph.points[v].x == work.integer(0).expect("zero"))
        })
        .expect("guscio esterno");
    assert_eq!(graph.holes[*outer].len(), 1);
    let hole = &graph.rings[graph.holes[*outer][0]];
    assert_eq!(hole.orientation, std::cmp::Ordering::Less);
    let mut sources: Vec<_> = hole
        .atoms
        .iter()
        .flat_map(|&i| graph.atoms[i].sources.iter().copied())
        .collect();
    sources.sort_unstable();
    // Fonte GOP-020: adiacenza al buco segue i quattro lati, non il ponte.
    assert_eq!(sources, [4, 5, 6, 7]);
}

#[test]
fn interruption_between_noding_and_face_enumeration() {
    use std::sync::atomic::{AtomicBool, Ordering};
    let flag = AtomicBool::new(false);
    let mut work = Work::new(RelateLimits::default(), Some(&flag));
    let edges = input(&[[(0., 0.), (1., 1.)]], &mut work).expect("fixture ingresso");
    let nodes = crate::noding::node(&[&edges[0]], &mut work).expect("noding completato");
    flag.store(true, Ordering::Relaxed);
    // Fonte GOP-028: interruzione osservata dopo lavoro gia' completato,
    // prima della crescita del grafo; anche l'ingresso vuoto fa checkpoint.
    assert!(matches!(
        enumerate(&nodes, &mut work),
        Err(PredicateError::Cancelled)
    ));
    assert!(matches!(
        enumerate(&[], &mut work),
        Err(PredicateError::Cancelled)
    ));
    let limits = RelateLimits {
        deadline: Some(std::time::Instant::now()),
        ..RelateLimits::default()
    };
    assert!(matches!(
        enumerate(&nodes, &mut Work::new(limits, None)),
        Err(PredicateError::DeadlineExceeded)
    ));
}

fn oracle_ring(cycle: &[usize], sites: &[OracleSite], clockwise: bool) -> geo::LineString<f64> {
    let mut cycle = cycle.to_vec();
    if clockwise {
        cycle.reverse();
    }
    let minimum = cycle
        .iter()
        .enumerate()
        .min_by_key(|(_, v)| &sites[**v])
        .expect("fixture minimo")
        .0;
    cycle.rotate_left(minimum);
    let coords: Vec<_> = cycle
        .iter()
        .chain(cycle.first())
        .map(|&id| {
            let convert = |q: &oracle::Q| {
                q.n.to_string().parse::<f64>().expect("fixture numeratore")
                    / q.d
                        .to_string()
                        .parse::<f64>()
                        .expect("fixture denominatore")
            };
            (convert(&sites[id].x), convert(&sites[id].y))
        })
        .collect();
    geo::LineString::from(coords)
}

#[test]
fn public_completion_against_independent_cycles_with_rational_noding() {
    use geo::{Geometry, LineString, Polygon};
    // Fonte GOP-020: grafo nodato analiticamente, senza chiamare node/enumerate.
    // y=x/3 incontra x=1 in 1/3; le altre due facce sono occupate.
    let sites = [
        site(0, 0, 1),
        site(1, 0, 1),
        site(3, 1, 3),
        site(1, 2, 1),
        site(0, 2, 1),
        site(3, 1, 1),
        site(3, 2, 1),
    ];
    let edges = [
        (0, 1),
        (1, 2),
        (2, 3),
        (3, 4),
        (4, 0),
        (0, 2),
        (2, 5),
        (5, 6),
        (6, 3),
    ];
    let cycles = oracle_cycles(&sites, &edges);
    let expected: Vec<_> = cycles
        .iter()
        .filter(|cycle| cycle.contains(&5))
        .map(|cycle| Polygon::new(oracle_ring(cycle, &sites, false), vec![]))
        .collect();
    let occupied = Polygon::new(
        LineString::from(vec![(0., 0.), (1., 0.), (1., 2.), (0., 2.), (0., 0.)]),
        vec![],
    );
    let lines = Geometry::LineString(LineString::from(vec![
        (0., 0.),
        (3., 1.),
        (3., 2.),
        (1., 2.),
    ]));
    let actual =
        crate::polygon_completion_with_limits(&[occupied], &lines, RelateLimits::default(), None)
            .expect("facce pubbliche");
    assert_eq!(cycles.len(), 3);
    assert_eq!(actual, expected);
}

#[test]
fn public_completion_oracle_nested_component_and_dangle() {
    use geo::{Geometry, LineString, MultiLineString, Polygon};
    // Fonte GOP-020: quadrato occupato sinistro, faccia destra e isola occupata;
    // il lato 1-10 e' un residuo aperto e non appartiene ad alcun ciclo.
    let sites = [
        site(0, 0, 1),
        site(2, 0, 1),
        site(2, 4, 1),
        site(0, 4, 1),
        site(6, 0, 1),
        site(6, 4, 1),
        site(3, 1, 1),
        site(4, 1, 1),
        site(4, 2, 1),
        site(3, 2, 1),
        site(2, -1, 1),
    ];
    let edges = [
        (0, 1),
        (1, 2),
        (2, 3),
        (3, 0),
        (1, 4),
        (4, 5),
        (5, 2),
        (6, 7),
        (7, 8),
        (8, 9),
        (9, 6),
        (1, 10),
    ];
    let cycles = oracle_cycles(&sites, &edges);
    let shell = cycles
        .iter()
        .find(|c| c.contains(&4))
        .expect("faccia destra");
    let holes: Vec<_> = cycles
        .iter()
        .filter(|cycle| strictly_inside(&sites[cycle[0]], shell, &sites))
        .map(|cycle| oracle_ring(cycle, &sites, true))
        .collect();
    let expected = Polygon::new(oracle_ring(shell, &sites, false), holes);
    let left = Polygon::new(
        LineString::from(vec![(0., 0.), (2., 0.), (2., 4.), (0., 4.), (0., 0.)]),
        vec![],
    );
    let island = Polygon::new(oracle_ring(&[6, 7, 8, 9], &sites, false), vec![]);
    let lines = Geometry::MultiLineString(MultiLineString::new(vec![
        LineString::from(vec![(2., 0.), (6., 0.), (6., 4.), (2., 4.)]),
        LineString::from(vec![(2., 0.), (2., -1.)]),
    ]));
    let actual = crate::polygon_completion_with_limits(
        &[left, island],
        &lines,
        RelateLimits::default(),
        None,
    )
    .expect("faccia con buco");
    assert_eq!(actual, vec![expected]);
}

/// Oracolo generico: shoelace e parita' su tutti i lati, inclusi i bordi.
#[test]
fn indexed_ring_locations_and_orientation_match_generic() {
    let lines = [
        [(0., 0.), (6., 0.)],
        [(6., 0.), (6., 6.)],
        [(6., 6.), (3., 2.)],
        [(3., 2.), (0., 6.)],
        [(0., 6.), (0., 0.)],
        [(1., 1.), (2., 1.)],
        [(2., 1.), (2., 2.)],
        [(2., 2.), (1., 1.)],
    ];
    let mut work = Work::new(RelateLimits::default(), None);
    let graph = run(&lines, &mut work).expect("grafo razionale");
    for ring in &graph.rings {
        let mut area = work.integer(0).expect("area iniziale");
        for i in 0..ring.vertices.len() {
            area = area
                .try_add(
                    &graph.points[ring.vertices[i]]
                        .try_cross(&graph.points[ring.vertices[(i + 1) % ring.vertices.len()]])
                        .expect("determinante"),
                )
                .expect("somma generica");
        }
        assert_eq!(ring.orientation, area.sign());
        for x in -2..=14 {
            for y in -2..=14 {
                let p = P::of_in((f64::from(x) / 2., f64::from(y) / 2.).into(), &work)
                    .expect("campione cartesiano");
                let mut odd = false;
                let mut boundary = false;
                for i in 0..ring.vertices.len() {
                    let a = &graph.points[ring.vertices[i]];
                    let b = &graph.points[ring.vertices[(i + 1) % ring.vertices.len()]];
                    let edge = Edge {
                        a: a.try_clone().expect("copia razionale"),
                        b: b.try_clone().expect("copia razionale"),
                    };
                    if edge.contains(&p).expect("incidenza esatta") {
                        boundary = true;
                        break;
                    }
                    if a.y.try_cmp(&p.y).expect("confronto esatto").is_gt()
                        != b.y.try_cmp(&p.y).expect("confronto esatto").is_gt()
                        && b.try_sub(a)
                            .expect("direzione")
                            .try_cross(&p.try_sub(a).expect("raggio"))
                            .expect("segno")
                            .sign()
                            == b.y.try_cmp(&a.y).expect("confronto esatto")
                    {
                        odd = !odd;
                    }
                }
                assert_eq!(
                    inside(&p, ring, &graph.points, &mut work).expect("query indicizzata"),
                    if boundary { None } else { Some(odd) }
                );
            }
        }
    }
}
