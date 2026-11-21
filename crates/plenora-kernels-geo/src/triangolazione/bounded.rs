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

    /// Trasformazione della griglia conservativa i64 storica. Non esegue
    /// noding, agganci o pulizia degli anelli: questi restano da qualificare.
    pub struct Grid {
        bounds: Rect<f64>,
        center: Coord<f64>,
        scale: f64,
        step: f64,
        integer_min: [i64; 2],
        integer_max: [i64; 2],
    }

    impl Grid {
        pub fn new(
            coordinates: &[Coord<f64>],
            envelope: Rect<f64>,
            work: &mut Work<'_>,
        ) -> Result<Self, ClipError> {
            work.step()?;
            let mut min = envelope.min();
            let mut max = envelope.max();
            let valid = |p: Coord<f64>| {
                [p.x, p.y]
                    .iter()
                    .all(|v| v.is_finite() && v.abs() <= 2_f64.powi(500))
            };
            if !valid(min) || !valid(max) || min.x >= max.x || min.y >= max.y {
                return Err(ClipError::InvalidEnvelope);
            }
            for &point in coordinates {
                work.step()?;
                if !valid(point) {
                    return Err(ClipError::UnsupportedRing);
                }
                min.x = min.x.min(point.x);
                min.y = min.y.min(point.y);
                max.x = max.x.max(point.x);
                max.y = max.y.max(point.y);
            }
            let bounds = Rect::new(min, max);
            // Stesse operazioni del backend: midpoint e mul_add cambiano i bit.
            #[allow(clippy::manual_midpoint)]
            let center = Coord {
                x: (min.x + max.x) * 0.5,
                y: (min.y + max.y) * 0.5,
            };
            let step = crate::rust_backend::griglia::passo_griglia(bounds)
                .filter(|step| *step > 0.0 && step.is_finite())
                .ok_or(ClipError::InvalidEnvelope)?;
            let scale = 1.0 / step;
            let integer_min = [
                Self::integer(((min.x - center.x) * scale).floor())?,
                Self::integer(((min.y - center.y) * scale).floor())?,
            ];
            let integer_max = [
                Self::integer(((max.x - center.x) * scale).ceil())?,
                Self::integer(((max.y - center.y) * scale).ceil())?,
            ];
            Ok(Self {
                bounds,
                center,
                scale,
                step,
                integer_min,
                integer_max,
            })
        }

        // Valore gia arrotondato a intero; conversione non saturante.
        #[allow(clippy::cast_possible_truncation)]
        fn integer(value: f64) -> Result<i64, ClipError> {
            if !value.is_finite() || value.abs() > 2_f64.powi(61) {
                return Err(ClipError::UnsupportedRing);
            }
            Ok(value as i64)
        }

        fn quantize(&self, point: Coord<f64>) -> Result<[i64; 2], ClipError> {
            Ok([
                Self::integer(((point.x - self.center.x) * self.scale).round())?,
                Self::integer(((point.y - self.center.y) * self.scale).round())?,
            ])
        }

        /// Solo per punti gia prodotti sulla griglia. La somma floating del
        /// centro puo pubblicarli fuori dalle bounds floating dell'ingresso.
        pub fn requantize(
            &self,
            point: [i64; 2],
            work: &mut Work<'_>,
        ) -> Result<[i64; 2], ClipError> {
            let published = self.decode(point, work)?;
            work.step()?;
            self.quantize(published)
        }

        pub fn encode(
            &self,
            point: Coord<f64>,
            work: &mut Work<'_>,
        ) -> Result<[i64; 2], ClipError> {
            work.step()?;
            let min = self.bounds.min();
            let max = self.bounds.max();
            if !point.x.is_finite()
                || !point.y.is_finite()
                || point.x < min.x
                || point.x > max.x
                || point.y < min.y
                || point.y > max.y
            {
                return Err(ClipError::UnsupportedRing);
            }
            self.quantize(point)
        }

        #[allow(clippy::cast_precision_loss, clippy::suboptimal_flops)]
        pub fn decode(
            &self,
            point: [i64; 2],
            work: &mut Work<'_>,
        ) -> Result<Coord<f64>, ClipError> {
            work.step()?;
            // Conversione nearest-even di Rust seguita da prodotto e somma
            // separati, come int_to_float; non si fondono le operazioni.
            if (0..2).any(|axis| {
                point[axis] < self.integer_min[axis] || point[axis] > self.integer_max[axis]
            }) {
                return Err(ClipError::UnsupportedRing);
            }
            Ok(Coord {
                x: point[0] as f64 * self.step + self.center.x,
                y: point[1] as f64 * self.step + self.center.y,
            })
        }
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

    /// Qualifica soltanto questo corpus; non assume convessita universale.
    #[test]
    fn raw_cell_convex_profile_deterministic_corpus() {
        use crate::advanced::celle_grezze_da_spade;
        use crate::rust_backend::precision::Precision;
        use geo::{CoordsIter, Geometry, Point};
        let fingerprint = |cells: Vec<Geometry<f64>>| -> Vec<Vec<(u64, u64)>> {
            cells
                .iter()
                .map(|cell| {
                    cell.coords_iter()
                        .map(|p| (p.x.to_bits(), p.y.to_bits()))
                        .collect()
                })
                .collect()
        };
        let mut state = 620_u64;
        let mut rejected = 0;
        let mut accepted = 0;
        for _ in 0..64 {
            let mut points = Vec::new();
            for _ in 0..12 {
                let mut next = || {
                    state = state
                        .wrapping_mul(6_364_136_223_846_793_005)
                        .wrapping_add(1);
                    f64::from(u32::try_from((state >> 32) % 100).unwrap())
                };
                points.push(Coord {
                    x: next(),
                    y: next(),
                });
            }
            let (cells, envelope) =
                celle_grezze_da_spade(&points, Precision::new(0.01).unwrap()).unwrap();
            let input: Vec<_> = points
                .iter()
                .map(|&point| Geometry::Point(Point(point)))
                .collect();
            let current = crate::advanced::voronoi_cells(&input, 12, Precision::new(0.01).unwrap())
                .map(&fingerprint)
                .map_err(|error| error.to_string());
            let baseline = crate::triangolazione::voronoi_610::voronoi_cells(
                &input,
                12,
                Precision::new(0.01).unwrap(),
            )
            .map(&fingerprint)
            .map_err(|error| error.to_string());
            assert_eq!(current, baseline);
            for cell in cells {
                let mut work = Work::new(RelateLimits::default(), None);
                match convex_ring(&cell.exterior().0, envelope, &mut work) {
                    Ok(_) => accepted += 1,
                    Err(ClipError::UnsupportedRing) => rejected += 1,
                    Err(error) => panic!("unexpected profile failure: {error}"),
                }
            }
        }
        assert_eq!((accepted, rejected), (768, 0));
    }

    /// Il backend e' l'oracolo della sola trasformazione: triangoli strettamente
    /// interni all'inviluppo non richiedono intersezioni o agganci.
    /// Nessuna pretesa su ordine degli anelli o pulizia dei contorni.
    #[test]
    fn clipping_grid_matches_backend_unintersected_vertices() {
        use super::clipping::Grid;
        use geo::{BooleanOps, LineString, Polygon};
        for exponent in [-500, -60, -1, 0, 20, 60, 490] {
            let scale = 2_f64.powi(exponent);
            for offset in [-17.3, -0.1, 0.0, 13.7] {
                let point = |x: f64, y: f64| Coord {
                    x: (x + offset) * scale,
                    y: (y - offset) * scale,
                };
                let coordinates = [point(0.1, 0.3), point(2.7, 0.4), point(1.3, 3.1)];
                let envelope = Rect::new(point(-1.1, -1.2), point(4.3, 4.7));
                let polygon = Polygon::new(LineString::new(coordinates.to_vec()), vec![]);
                let historic = polygon.intersection(&envelope.to_polygon());
                assert_eq!(historic.0.len(), 1);
                let mut work = Work::new(RelateLimits::default(), None);
                let grid = Grid::new(&coordinates, envelope, &mut work).unwrap();
                let mut expected = Vec::new();
                for coordinate in coordinates {
                    let integer = grid.encode(coordinate, &mut work).unwrap();
                    let first = grid.decode(integer, &mut work).unwrap();
                    let integer = grid.requantize(integer, &mut work).unwrap();
                    let second = grid.decode(integer, &mut work).unwrap();
                    assert_eq!(
                        (first.x.to_bits(), first.y.to_bits()),
                        (second.x.to_bits(), second.y.to_bits())
                    );
                    expected.push((first.x.to_bits(), first.y.to_bits()));
                }
                let ring = &historic.0[0].exterior().0;
                assert_eq!(ring.len(), 4);
                let mut actual: Vec<_> = ring[..3]
                    .iter()
                    .map(|p| (p.x.to_bits(), p.y.to_bits()))
                    .collect();
                actual.sort_unstable();
                expected.sort_unstable();
                assert_eq!(actual, expected);
            }
        }
    }

    #[test]
    fn clipping_grid_roundtrip_outside_float_bounds_and_scale_cap() {
        use super::clipping::Grid;
        let mut work = Work::new(RelateLimits::default(), None);
        let envelope = Rect::new((1.0e-20, 0.), (1., 1.));
        let grid = Grid::new(&[], envelope, &mut work).unwrap();
        let encoded = grid.encode(envelope.min(), &mut work).unwrap();
        let decoded = grid.decode(encoded, &mut work).unwrap();
        assert!(decoded.x < envelope.min().x);
        assert!(matches!(
            grid.encode(decoded, &mut work),
            Err(ClipError::UnsupportedRing)
        ));
        assert_eq!(grid.requantize(encoded, &mut work).unwrap(), encoded);
        for exponent in [-1074, -1023, -1000, -963] {
            let tiny = if exponent == -1074 {
                f64::from_bits(1)
            } else {
                2_f64.powi(exponent)
            };
            let envelope = Rect::new((-tiny, -tiny), (tiny, tiny));
            let grid = Grid::new(&[], envelope, &mut work).unwrap();
            let encoded = grid.encode(envelope.max(), &mut work).unwrap();
            let decoded = grid.decode(encoded, &mut work).unwrap();
            // Quando la scala e' limitata a 2^1023, il passo e' 2^-1023.
            let expected = (tiny * 2_f64.powi(1023)).round() * 2_f64.powi(-1023);
            assert_eq!(decoded.x.to_bits(), expected.to_bits());
            assert_eq!(decoded.y.to_bits(), expected.to_bits());
            for zero in [0., -0.] {
                let encoded = grid.encode(Coord { x: zero, y: zero }, &mut work).unwrap();
                let decoded = grid.decode(encoded, &mut work).unwrap();
                assert_eq!((decoded.x.to_bits(), decoded.y.to_bits()), (0, 0));
            }
        }
        let grid = Grid::new(&[], Rect::new((-1., -0.01), (1., 0.01)), &mut work).unwrap();
        assert!(matches!(
            grid.decode([0, 1_i64 << 61], &mut work),
            Err(ClipError::UnsupportedRing)
        ));
    }

    /// La trasformazione usa solo scalari sullo stack; il lavoro cresce con
    /// le coordinate e tutte le primitive osservano il checkpoint comune.
    #[test]
    fn clipping_grid_thresholds_and_interruptions() {
        use super::clipping::Grid;
        let envelope = Rect::new((0., 0.), (4., 4.));
        let run = |work: &mut Work<'_>| -> Result<(), ClipError> {
            let grid = Grid::new(&triangle(), envelope, work)?;
            let point = grid.encode(Coord { x: 1., y: 2. }, work)?;
            grid.decode(point, work)?;
            Ok(())
        };
        let mut work = Work::new(RelateLimits::default(), None);
        run(&mut work).unwrap();
        let steps = work.steps;
        assert_eq!(steps, 6);
        assert_eq!((work.nodes, work.pairs, work.peak_bytes()), (0, 0, 0));
        for limit in [steps - 1, steps, steps + 1] {
            let limits = RelateLimits {
                max_steps: limit,
                ..RelateLimits::default()
            };
            let result = run(&mut Work::new(limits, None));
            if limit < steps {
                assert!(matches!(
                    result,
                    Err(ClipError::Resource(PredicateError::WorkLimit))
                ));
            } else {
                result.unwrap();
            }
        }
        for at in 0..steps {
            for deadline in [false, true] {
                let flag = AtomicBool::new(false);
                let mut work = Work::new(RelateLimits::default(), Some(&flag));
                work.interrupt_at = Some((at, deadline));
                let result = run(&mut work);
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
    }

    #[test]
    fn clipping_grid_rejects_outside_domain_before_casts() {
        use super::clipping::Grid;
        let mut work = Work::new(RelateLimits::default(), None);
        let envelope = Rect::new((-1., -1.), (1., 1.));
        let grid = Grid::new(&[], envelope, &mut work).unwrap();
        for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, 2., -2.] {
            assert!(matches!(
                grid.encode(Coord { x: value, y: 0. }, &mut work),
                Err(ClipError::UnsupportedRing)
            ));
        }
        for value in [i64::MIN, i64::MAX, (1_i64 << 61) + 1, -(1_i64 << 61) - 1] {
            assert!(matches!(
                grid.decode([value, 0], &mut work),
                Err(ClipError::UnsupportedRing)
            ));
        }
        assert!(matches!(
            Grid::new(
                &[Coord {
                    x: 2_f64.powi(501),
                    y: 0.
                }],
                envelope,
                &mut work
            ),
            Err(ClipError::UnsupportedRing)
        ));
        assert!(matches!(
            Grid::new(&[], Rect::new((0., 0.), (0., 1.)), &mut work),
            Err(ClipError::InvalidEnvelope)
        ));
        let flag = AtomicBool::new(true);
        let limits = RelateLimits {
            deadline: Some(std::time::Instant::now()),
            ..RelateLimits::default()
        };
        assert!(matches!(
            Grid::new(&[], envelope, &mut Work::new(limits, Some(&flag))),
            Err(ClipError::Resource(PredicateError::DeadlineExceeded))
        ));
    }

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
