//! Costruzione con il contesto comune; Spade resta un confine esterno.
use super::{chiave, Sito, Triangolazione};
use crate::extended_algorithms::delaunay_limited::DelaunayBuildError;
use crate::noding::{sorted, CountedVec, Work};
use geo::{Coord, GeoNum, Kernel, Orientation};
use spade::{DelaunayTriangulation, Triangulation as _};

fn distinct(
    coordinates: &[Coord<f64>],
    work: &mut Work<'_>,
) -> Result<CountedVec<Coord<f64>>, DelaunayBuildError> {
    let mut order = CountedVec::new();
    for index in 0..coordinates.len() {
        work.push(&mut order, index)?;
    }
    sorted(
        &mut order,
        |&a, &b| Ok((chiave(coordinates[a]), a).cmp(&(chiave(coordinates[b]), b))),
        work,
    )?;
    let mut groups = CountedVec::new();
    for _ in coordinates {
        work.push(&mut groups, 0_usize)?;
    }
    let mut previous = None;
    for &index in &order {
        work.step()?;
        let key = chiave(coordinates[index]);
        let first = match previous {
            Some((same, first)) if same == key => first,
            _ => index,
        };
        previous = Some((key, first));
        groups[index] = first;
    }
    drop(order);
    let mut sites = CountedVec::new();
    let mut collinear = true;
    for (index, &coordinate) in coordinates.iter().enumerate() {
        work.step()?;
        let first = groups[index];
        if first == index {
            if collinear && sites.len() >= 2 {
                collinear = <f64 as GeoNum>::Ker::orient2d(sites[0], sites[1], coordinate)
                    == Orientation::Collinear;
            }
            groups[index] = sites.len();
            work.nodes(1)?;
            work.push(&mut sites, coordinate)?;
        } else if sites.len() == 1 || !collinear {
            sites[groups[first]] = coordinate;
        }
    }
    Ok(sites)
}

pub fn build(
    coordinates: &[Coord<f64>],
    work: &mut Work<'_>,
) -> Result<Triangolazione, DelaunayBuildError> {
    work.step()?;
    for &coordinate in coordinates {
        work.step()?;
        spade::validate_vertex(&coordinate).map_err(|error| {
            DelaunayBuildError::Triangulation(super::ErroreTriangolazione::Inserimento(error))
        })?;
    }
    let sites = distinct(coordinates, work)?;
    let mut vertices = CountedVec::new();
    for (rango, &coordinata) in sites.iter().enumerate() {
        work.push(&mut vertices, Sito { coordinata, rango })?;
    }
    let vertices = vertices.export(work)?;
    work.step()?;
    // Opzione A: nessuna promessa di prenotazioni o checkpoint DENTRO Spade.
    // L'ordine dei vertici e la chiamata bulk_load sono quelli della baseline.
    let result = crate::calcolo_protetto(|| DelaunayTriangulation::<Sito>::bulk_load(vertices));
    work.step()?;
    let triangulation = result
        .map_err(DelaunayBuildError::Calculation)?
        .map_err(|error| {
            DelaunayBuildError::Triangulation(super::ErroreTriangolazione::Inserimento(error))
        })?;
    let mut present = CountedVec::new();
    for _ in &sites {
        work.push(&mut present, false)?;
    }
    let mut complete = triangulation.num_vertices() == sites.len();
    for vertex in triangulation.vertices() {
        work.step()?;
        match present.get_mut(vertex.data().rango) {
            Some(mark) if !*mark => *mark = true,
            _ => complete = false,
        }
    }
    if !complete {
        return Err(DelaunayBuildError::Triangulation(
            super::ErroreTriangolazione::VerticiInattesi,
        ));
    }
    Ok(Triangolazione {
        triangolazione: triangulation,
        siti: sites.export(work)?,
    })
}

/// Nucleo razionale del clipping; non pubblica ancora celle sulla griglia storica.
#[cfg(test)]
#[deny(clippy::disallowed_methods)]
mod clipping {
    use crate::noding::{CountedVec, Work, P, Q};
    use crate::predicates::PredicateError;
    use geo::{Coord, Rect};
    use std::cmp::Ordering;

    /// Errori senza coordinate, compreso l'ingresso fuori dal profilo convesso.
    #[derive(Debug, thiserror::Error)]
    pub enum ClipError {
        /// Serve un anello convesso, semplice e non degenere.
        #[error("anello fuori dal profilo convesso del clipping")]
        UnsupportedRing,
        /// Serve un inviluppo finito con area positiva.
        #[error("inviluppo del clipping non valido")]
        InvalidEnvelope,
        /// Risorse e interruzioni del contesto chiamante.
        #[error(transparent)]
        Resource(#[from] PredicateError),
    }

    fn ordinate(point: &P, axis: usize) -> &Q {
        if axis == 0 {
            &point.x
        } else {
            &point.y
        }
    }

    fn append(
        output: &mut CountedVec<P>,
        point: P,
        work: &mut Work<'_>,
    ) -> Result<(), PredicateError> {
        work.step()?;
        if output.last() != Some(&point) {
            work.nodes(1)?;
            work.push(output, point)?;
        }
        Ok(())
    }

    fn open(ring: &mut CountedVec<P>, work: &mut Work<'_>) -> Result<(), PredicateError> {
        work.step()?;
        if ring.len() > 1 && ring.first() == ring.last() {
            ring.pop();
        }
        Ok(())
    }

    // Tutti i vertici devono stare nello stesso semipiano di ogni lato.
    // I soli segni delle svolte non bastano: accetterebbero stelle avvolte.
    fn convex(ring: &[P], work: &mut Work<'_>) -> Result<(), ClipError> {
        if ring.len() < 3 {
            return Err(ClipError::UnsupportedRing);
        }
        let mut direction = Ordering::Equal;
        for (index, a) in ring.iter().enumerate() {
            work.step()?;
            let edge = ring[(index + 1) % ring.len()].try_sub(a)?;
            for (other, point) in ring.iter().enumerate() {
                work.pair()?;
                if point == a && other != index {
                    return Err(ClipError::UnsupportedRing);
                }
                let sign = edge.try_cross(&point.try_sub(a)?)?.sign();
                if sign != Ordering::Equal {
                    if direction != Ordering::Equal && direction != sign {
                        return Err(ClipError::UnsupportedRing);
                    }
                    direction = sign;
                }
            }
        }
        if direction == Ordering::Equal {
            return Err(ClipError::UnsupportedRing);
        }
        Ok(())
    }

    fn half_plane(
        input: &[P],
        axis: usize,
        boundary: &Q,
        lower: bool,
        work: &mut Work<'_>,
    ) -> Result<CountedVec<P>, PredicateError> {
        let mut output = CountedVec::new();
        let Some(mut previous) = input.last() else {
            work.step()?;
            return Ok(output);
        };
        let inside = |point: &P| -> Result<bool, PredicateError> {
            let order = ordinate(point, axis).try_cmp(boundary)?;
            Ok(if lower {
                !order.is_lt()
            } else {
                !order.is_gt()
            })
        };
        let mut previous_inside = inside(previous)?;
        for current in input {
            work.step()?;
            let current_inside = inside(current)?;
            if previous_inside != current_inside {
                let fraction = boundary
                    .try_sub(ordinate(previous, axis))?
                    .try_div(&ordinate(current, axis).try_sub(ordinate(previous, axis))?)?;
                let delta = current.try_sub(previous)?;
                let point = P {
                    x: previous.x.try_add(&delta.x.try_mul(&fraction)?)?,
                    y: previous.y.try_add(&delta.y.try_mul(&fraction)?)?,
                };
                append(&mut output, point, work)?;
            }
            if current_inside {
                append(&mut output, current.try_clone()?, work)?;
            }
            previous = current;
            previous_inside = current_inside;
        }
        open(&mut output, work)?;
        Ok(output)
    }

    /// Interseca un anello convesso con quattro semipiani esatti.
    ///
    /// Anello aperto o chiuso, in entrambi i versi; duplicati consecutivi
    /// rimossi. Uscita aperta, stesso verso, eventualmente di dimensione
    /// inferiore, anche con vertici collineari. Nessun arrotondamento o snap.
    /// Il controllo di convessita' e' quadratico e conta ogni coppia;
    /// clipping e copie sono lineari. Buffer e interi razionali condividono
    /// il budget chiamante, compresa la coesistenza fra due passaggi.
    /// Non crea un secondo Work e non chiama backend esterni.
    ///
    /// # Errors
    /// [`ClipError`]: profilo, inviluppo, risorse o interruzione. Non
    /// sostituisce il clipping storico: manca l'adapter di pubblicazione
    /// bit per bit, inclusa la pulizia su griglia.
    pub fn convex_ring(
        coordinates: &[Coord<f64>],
        envelope: Rect<f64>,
        work: &mut Work<'_>,
    ) -> Result<CountedVec<P>, ClipError> {
        work.step()?;
        let min = envelope.min();
        let max = envelope.max();
        if ![min.x, min.y, max.x, max.y].iter().all(|v| v.is_finite())
            || min.x >= max.x
            || min.y >= max.y
        {
            return Err(ClipError::InvalidEnvelope);
        }
        let min = P::of_in(min, work)?;
        let max = P::of_in(max, work)?;
        let mut ring = CountedVec::new();
        for &coordinate in coordinates {
            work.step()?;
            if !coordinate.x.is_finite() || !coordinate.y.is_finite() {
                return Err(ClipError::UnsupportedRing);
            }
            append(&mut ring, P::of_in(coordinate, work)?, work)?;
        }
        open(&mut ring, work)?;
        convex(&ring, work)?;
        for (axis, boundary, lower) in [
            (0, &min.x, true),
            (0, &max.x, false),
            (1, &min.y, true),
            (1, &max.y, false),
        ] {
            work.step()?;
            ring = half_plane(&ring, axis, boundary, lower, work)?;
        }
        work.step()?;
        Ok(ring)
    }
}

#[cfg(test)]
#[deny(clippy::disallowed_methods)]
mod clipping_tests {
    use super::clipping::{convex_ring, ClipError};
    use crate::noding::{Work, P, Q};
    use crate::predicates::{PredicateError, RelateLimits};
    use geo::{Coord, Rect};
    use std::sync::atomic::AtomicBool;

    fn triangle() -> [Coord<f64>; 3] {
        [(0., 0.), (7., 2.), (1., 9.)].map(|(x, y)| Coord { x, y })
    }

    fn area2(ring: &[P], work: &Work<'_>) -> Q {
        let mut sum = work.integer(0).unwrap();
        for (i, a) in ring.iter().enumerate() {
            sum = sum
                .try_add(&a.try_cross(&ring[(i + 1) % ring.len()]).unwrap())
                .unwrap();
        }
        sum
    }

    /// Oracolo analitico: intersezioni (4,8/7), (4,4), (4/9,4).
    #[test]
    fn clipping_exact_fractional_vertices() {
        let mut work = Work::new(RelateLimits::default(), None);
        let ring = convex_ring(&triangle(), Rect::new((0., 0.), (4., 4.)), &mut work).unwrap();
        let integer = |n| work.integer(n).unwrap();
        let expected = [
            P {
                x: integer(0),
                y: integer(0),
            },
            P {
                x: integer(4),
                y: integer(8).try_div(&integer(7)).unwrap(),
            },
            P {
                x: integer(4),
                y: integer(4),
            },
            P {
                x: integer(4).try_div(&integer(9)).unwrap(),
                y: integer(4),
            },
        ];
        assert_eq!(ring.len(), expected.len());
        assert!(expected.iter().all(|p| ring.contains(p)));
        assert!(area2(&ring, &work).sign().is_gt());
    }

    /// Partizione in sedici rettangoli: somma delle aree razionali uguale
    /// al determinante intero del triangolo, anche invertendo il verso.
    #[test]
    fn clipping_exact_area_partition_and_winding() {
        for n in 1..=12 {
            let input = [(0., 0.), (8., f64::from(n % 7)), (f64::from(n % 5), 8.)]
                .map(|(x, y)| Coord { x, y });
            for reverse in [false, true] {
                let mut input = input;
                if reverse {
                    input.reverse();
                }
                let mut work = Work::new(RelateLimits::default(), None);
                let points: Vec<_> = input.iter().map(|&p| P::of_in(p, &work).unwrap()).collect();
                let expected = area2(&points, &work);
                let mut sum = work.integer(0).unwrap();
                for x in 0..4 {
                    for y in 0..4 {
                        let rect = Rect::new(
                            (f64::from(2 * x), f64::from(2 * y)),
                            (f64::from(2 * x + 2), f64::from(2 * y + 2)),
                        );
                        let ring = convex_ring(&input, rect, &mut work).unwrap();
                        let lo = P::of_in(rect.min(), &work).unwrap();
                        let hi = P::of_in(rect.max(), &work).unwrap();
                        // Le celle stanno in rettangoli a interni disgiunti;
                        // l'area totale non puo' compensare sovrapposizioni.
                        for point in &ring {
                            assert!(!point.x.try_cmp(&lo.x).unwrap().is_lt());
                            assert!(!point.x.try_cmp(&hi.x).unwrap().is_gt());
                            assert!(!point.y.try_cmp(&lo.y).unwrap().is_lt());
                            assert!(!point.y.try_cmp(&hi.y).unwrap().is_gt());
                            for i in 0..3 {
                                let sign = points[(i + 1) % 3]
                                    .try_sub(&points[i])
                                    .unwrap()
                                    .try_cross(&point.try_sub(&points[i]).unwrap())
                                    .unwrap()
                                    .sign();
                                assert!(sign.is_eq() || sign == expected.sign());
                            }
                        }
                        sum = sum.try_add(&area2(&ring, &work)).unwrap();
                    }
                }
                assert_eq!(sum, expected);
            }
        }
    }

    #[test]
    fn clipping_rejects_nonconvex_repeated_and_nonfinite_rings() {
        let inputs = [
            vec![(0., 0.), (4., 0.), (1., 1.), (4., 4.), (0., 4.)],
            vec![(0., 0.), (4., 0.), (4., 4.), (0., 0.), (0., 4.)],
            vec![(0., 0.), (1., 1.), (2., 2.)],
            vec![(0., 0.), (f64::NAN, 1.), (2., 2.)],
            vec![(0., 3.), (2., -3.), (-3., 1.), (3., 1.), (-2., -3.)],
        ];
        for input in inputs {
            let coordinates: Vec<_> = input.into_iter().map(|(x, y)| Coord { x, y }).collect();
            assert!(matches!(
                convex_ring(
                    &coordinates,
                    Rect::new((0., 0.), (4., 4.)),
                    &mut Work::new(RelateLimits::default(), None)
                ),
                Err(ClipError::UnsupportedRing)
            ));
        }
    }

    #[test]
    fn clipping_closed_duplicate_and_lower_dimensional_results() {
        let mut work = Work::new(RelateLimits::default(), None);
        let square = [(0., 0.), (0., 0.), (2., 0.), (2., 2.), (0., 2.), (0., 0.)]
            .map(|(x, y)| Coord { x, y });
        for (rect, count) in [
            (Rect::new((3., 3.), (4., 4.)), 0),
            (Rect::new((2., 2.), (4., 4.)), 1),
            (Rect::new((2., 0.), (4., 2.)), 2),
            (Rect::new((0., 0.), (2., 2.)), 4),
        ] {
            assert_eq!(convex_ring(&square, rect, &mut work).unwrap().len(), count);
        }
    }

    fn measure(limits: RelateLimits) -> (Result<(), ClipError>, u64, u64, u64, u64) {
        let mut work = Work::new(limits, None);
        let result = convex_ring(&triangle(), Rect::new((0., 0.), (4., 4.)), &mut work).map(drop);
        (
            result,
            work.steps,
            work.peak_bytes(),
            work.nodes,
            work.pairs,
        )
    }

    #[test]
    fn clipping_exact_resource_thresholds() {
        let (result, steps, bytes, nodes, pairs) = measure(RelateLimits::default());
        result.unwrap();
        for axis in 0..4 {
            let exact = [steps, bytes, nodes, pairs][axis];
            for limit in [exact - 1, exact, exact + 1] {
                let mut limits = RelateLimits::default();
                match axis {
                    0 => limits.max_steps = limit,
                    1 => limits.max_reserved_bytes = limit,
                    2 => limits.max_nodes = limit,
                    _ => limits.max_segment_pairs = limit,
                }
                let result = measure(limits).0;
                if limit < exact {
                    if axis == 1 {
                        assert!(matches!(
                            result,
                            Err(ClipError::Resource(PredicateError::ReservationLimit))
                        ));
                    } else {
                        assert!(matches!(
                            result,
                            Err(ClipError::Resource(PredicateError::WorkLimit))
                        ));
                    }
                } else {
                    result.unwrap();
                }
            }
        }
    }

    #[test]
    fn clipping_interrupts_inside_and_at_delivery() {
        let steps = measure(RelateLimits::default()).1;
        for at in [0, 1, steps / 2, steps - 1] {
            for deadline in [false, true] {
                let flag = AtomicBool::new(false);
                let mut work = Work::new(RelateLimits::default(), Some(&flag));
                work.interrupt_at = Some((at, deadline));
                let result = convex_ring(&triangle(), Rect::new((0., 0.), (4., 4.)), &mut work);
                if deadline {
                    assert!(matches!(
                        result,
                        Err(ClipError::Resource(PredicateError::DeadlineExceeded))
                    ));
                } else {
                    assert!(matches!(
                        result,
                        Err(ClipError::Resource(PredicateError::Cancelled))
                    ));
                }
            }
        }
        let flag = AtomicBool::new(true);
        let limits = RelateLimits {
            deadline: Some(std::time::Instant::now()),
            ..RelateLimits::default()
        };
        assert!(matches!(
            convex_ring(
                &[],
                Rect::new((0., 0.), (0., 0.)),
                &mut Work::new(limits, Some(&flag))
            ),
            Err(ClipError::Resource(PredicateError::DeadlineExceeded))
        ));
    }
}
