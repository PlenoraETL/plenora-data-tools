//! Semplificazioni con distanze e aree razionali, prima di arrotondare.
//! Mantiene lo spareggio all'ultimo vertice e il minimo strutturale 2/4.

use super::OperationError;
use crate::noding::{CountedVec, Work};
use crate::segment_index::DistanceKey;
use geo::{Geometry, LineString, MultiLineString, MultiPolygon, Polygon};

pub(super) fn simplify(
    geometry: &Geometry<f64>,
    tolerance: f64,
) -> Result<Geometry<f64>, OperationError> {
    let mut work = Work::new(crate::predicates::RelateLimits::default(), None);
    let result = match geometry {
        Geometry::LineString(line) => {
            Geometry::LineString(simplify_line(line, tolerance, 2, &mut work)?)
        }
        Geometry::MultiLineString(lines) => Geometry::MultiLineString(MultiLineString(
            lines
                .0
                .iter()
                .map(|line| simplify_line(line, tolerance, 2, &mut work))
                .collect::<Result<_, _>>()?,
        )),
        Geometry::Polygon(p) => Geometry::Polygon(simplify_polygon(p, tolerance, &mut work)?),
        Geometry::MultiPolygon(ps) => Geometry::MultiPolygon(MultiPolygon(
            ps.0.iter()
                .map(|p| simplify_polygon(p, tolerance, &mut work))
                .collect::<Result<_, _>>()?,
        )),
        other => other.clone(),
    };
    Ok(result)
}

fn simplify_polygon(
    p: &Polygon<f64>,
    tolerance: f64,
    work: &mut Work<'_>,
) -> Result<Polygon<f64>, OperationError> {
    Ok(Polygon::new(
        simplify_line(p.exterior(), tolerance, 4, work)?,
        p.interiors()
            .iter()
            .map(|r| simplify_line(r, tolerance, 4, work))
            .collect::<Result<_, _>>()?,
    ))
}

fn simplify_line(
    line: &LineString<f64>,
    tolerance: f64,
    minimum: usize,
    work: &mut Work<'_>,
) -> Result<LineString<f64>, OperationError> {
    if line.0.len() < 3 || tolerance <= 0.0 {
        return Ok(line.clone());
    }
    let mut keep = CountedVec::new();
    for _ in &line.0 {
        work.push(&mut keep, true)?;
    }
    let mut pending = CountedVec::new();
    work.push(&mut pending, (0, line.0.len() - 1))?;
    let mut retained = line.0.len();
    while let Some((first, last)) = pending.pop() {
        work.step()?;
        if last - first < 2 {
            continue;
        }
        let mut farthest = first + 1;
        let mut maximum = DistanceKey::zero(work)?;
        for i in first + 1..last {
            let key = DistanceKey::coordinates(line.0[i], line.0[first], line.0[last], work)?;
            if key.compare(&maximum)?.is_ge() {
                farthest = i;
                maximum = key;
            }
        }
        if !maximum.within(tolerance, work)? {
            work.push(&mut pending, (farthest, last))?;
            work.push(&mut pending, (first, farthest))?;
        } else if retained - (last - first - 1) >= minimum {
            for flag in &mut keep[first + 1..last] {
                work.step()?;
                *flag = false;
            }
            retained -= last - first - 1;
        }
    }
    Ok(LineString(
        line.0
            .iter()
            .zip(keep.iter())
            .filter_map(|(p, keep)| keep.then_some(*p))
            .collect(),
    ))
}

// VW mantiene i vicini con indici stabili. Le aree sono razionali; la
// coda fallibile usa lo stesso bilancio delle primitive geometriche.
struct AreaEntry {
    left: usize,
    current: usize,
    right: usize,
    area: DistanceKey,
    forced: bool,
}

fn area_less(a: &AreaEntry, b: &AreaEntry) -> Result<bool, OperationError> {
    Ok(if a.forced == b.forced {
        !a.forced && a.area.compare(&b.area)?.is_lt()
    } else {
        a.forced
    })
}

fn area_push(
    heap: &mut CountedVec<AreaEntry>,
    entry: AreaEntry,
    work: &mut Work<'_>,
) -> Result<(), OperationError> {
    work.push(heap, entry)?;
    let mut child = heap.len() - 1;
    while child > 0 {
        work.step()?;
        let parent = (child - 1) / 2;
        if !area_less(&heap[child], &heap[parent])? {
            break;
        }
        heap.swap(child, parent);
        child = parent;
    }
    Ok(())
}

fn area_pop(
    heap: &mut CountedVec<AreaEntry>,
    work: &mut Work<'_>,
) -> Result<Option<AreaEntry>, OperationError> {
    let Some(last) = heap.pop() else {
        return Ok(None);
    };
    if heap.is_empty() {
        return Ok(Some(last));
    }
    let result = std::mem::replace(&mut heap[0], last);
    let mut parent = 0;
    while parent < heap.len() / 2 {
        work.step()?;
        let mut child = parent * 2 + 1;
        if child + 1 < heap.len() && !area_less(&heap[child], &heap[child + 1])? {
            child += 1;
        }
        if !area_less(&heap[child], &heap[parent])? {
            break;
        }
        heap.swap(parent, child);
        parent = child;
    }
    Ok(Some(result))
}

fn triangle_area(
    points: &[crate::noding::P],
    left: usize,
    current: usize,
    right: usize,
    work: &mut Work<'_>,
) -> Result<DistanceKey, OperationError> {
    work.step()?;
    let a = points[current].try_sub(&points[left])?;
    let b = points[right].try_sub(&points[left])?;
    let signed = a.x.try_mul(&b.y)?.try_sub(&a.y.try_mul(&b.x)?)?;
    let area = if signed.sign().is_lt() {
        work.integer(0)?.try_sub(&signed)?
    } else {
        signed
    };
    Ok(DistanceKey::squared(area.try_div(&work.integer(2)?)?)?)
}

// Stato dei vicini e aggiornamento della coda restano nello stesso ciclo.
#[allow(clippy::too_many_lines)]
fn vw_line(
    line: &LineString<f64>,
    tolerance: f64,
    minimum: usize,
    intersection_minimum: usize,
    segments: &mut CountedVec<(geo::Coord<f64>, geo::Coord<f64>)>,
    work: &mut Work<'_>,
) -> Result<LineString<f64>, OperationError> {
    if line.0.len() < 3 || tolerance == 0.0 {
        return Ok(line.clone());
    }
    let mut points = CountedVec::new();
    let mut adjacent = CountedVec::new();
    for (i, p) in line.0.iter().enumerate() {
        work.nodes(1)?;
        let point = crate::noding::P::of_in(*p, work)?;
        work.push(&mut points, point)?;
        work.push(
            &mut adjacent,
            (
                i.checked_sub(1),
                (i + 1 < line.0.len()).then_some(i + 1),
                true,
            ),
        )?;
    }
    let threshold = DistanceKey::squared(crate::noding::P::of_in((tolerance, 0.).into(), work)?.x)?;
    let mut heap = CountedVec::new();
    for current in 1..line.0.len() - 1 {
        let area = triangle_area(&points, current - 1, current, current + 1, work)?;
        area_push(
            &mut heap,
            AreaEntry {
                left: current - 1,
                current,
                right: current + 1,
                area,
                forced: false,
            },
            work,
        )?;
    }
    let mut retained = line.0.len();
    while let Some(entry) = area_pop(&mut heap, work)? {
        if retained <= minimum || (!entry.forced && entry.area.compare(&threshold)?.is_gt()) {
            break;
        }
        if adjacent[entry.current] != (Some(entry.left), Some(entry.right), true) {
            continue;
        }
        let (a, b) = (line.0[entry.left], line.0[entry.right]);
        let edge = crate::noding::Edge {
            a: points[entry.left].try_clone()?,
            b: points[entry.right].try_clone()?,
        };
        let mut intersects = false;
        for &(c, d) in segments.iter() {
            work.step()?;
            if c == a || c == b || d == a || d == b {
                continue;
            }
            if c.x.min(d.x) > a.x.max(b.x)
                || c.x.max(d.x) < a.x.min(b.x)
                || c.y.min(d.y) > a.y.max(b.y)
                || c.y.max(d.y) < a.y.min(b.y)
            {
                continue;
            }
            work.nodes(2)?;
            let other = crate::noding::Edge {
                a: crate::noding::P::of_in(c, work)?,
                b: crate::noding::P::of_in(d, work)?,
            };
            if DistanceKey::segments(&edge, &other, work)?.within(0., work)? {
                intersects = true;
                break;
            }
        }
        if intersects && retained <= intersection_minimum {
            break;
        }
        let middle = line.0[entry.current];
        for old in [(a, middle), (middle, b)] {
            let mut found = None;
            for (i, segment) in segments.iter().enumerate() {
                work.step()?;
                if *segment == old {
                    found = Some(i);
                    break;
                }
            }
            let i = found.ok_or(OperationError::Exact(
                crate::predicates::PredicateError::AccuracyUnproven,
            ))?;
            let last = segments.len() - 1;
            segments.swap(i, last);
            segments.pop();
        }
        work.push(segments, (a, b))?;
        let ll = adjacent[entry.left].0;
        let rr = adjacent[entry.right].1;
        adjacent[entry.left].1 = Some(entry.right);
        adjacent[entry.right].0 = Some(entry.left);
        adjacent[entry.current].2 = false;
        retained -= 1;
        for (left, current, right) in [
            (ll, entry.left, Some(entry.right)),
            (Some(entry.left), entry.right, rr),
        ] {
            if let (Some(left), Some(right)) = (left, right) {
                let area = triangle_area(&points, left, current, right, work)?;
                area_push(
                    &mut heap,
                    AreaEntry {
                        left,
                        current,
                        right,
                        area,
                        forced: intersects && current < entry.current,
                    },
                    work,
                )?;
            }
        }
    }
    Ok(LineString(
        line.0
            .iter()
            .zip(adjacent.iter())
            .filter_map(|(p, state)| state.2.then_some(*p))
            .collect(),
    ))
}

fn vw_rings(
    lines: &[&LineString<f64>],
    tolerance: f64,
    minimum: usize,
    intersection_minimum: usize,
    work: &mut Work<'_>,
) -> Result<Vec<LineString<f64>>, OperationError> {
    let mut segments = CountedVec::new();
    for line in lines {
        for edge in line.lines() {
            work.push(&mut segments, (edge.start, edge.end))?;
        }
    }
    lines
        .iter()
        .map(|line| {
            vw_line(
                line,
                tolerance,
                minimum,
                intersection_minimum,
                &mut segments,
                work,
            )
        })
        .collect()
}

pub(super) fn preserve_topology(
    geometry: &Geometry<f64>,
    tolerance: f64,
) -> Result<Geometry<f64>, OperationError> {
    fn one(
        line: &LineString<f64>,
        tolerance: f64,
        work: &mut Work<'_>,
    ) -> Result<LineString<f64>, OperationError> {
        vw_rings(&[line], tolerance, 2, 4, work)?
            .pop()
            .ok_or(OperationError::Exact(
                crate::predicates::PredicateError::AccuracyUnproven,
            ))
    }
    fn polygon(
        p: &Polygon<f64>,
        tolerance: f64,
        work: &mut Work<'_>,
    ) -> Result<Polygon<f64>, OperationError> {
        let lines: Vec<_> = std::iter::once(p.exterior()).chain(p.interiors()).collect();
        let mut rings = vw_rings(&lines, tolerance, 4, 5, work)?.into_iter();
        let exterior = rings.next().ok_or(OperationError::Exact(
            crate::predicates::PredicateError::AccuracyUnproven,
        ))?;
        Ok(Polygon::new(exterior, rings.collect()))
    }
    if tolerance == 0.0 {
        return Ok(geometry.clone());
    }
    let mut work = Work::new(crate::predicates::RelateLimits::default(), None);
    Ok(match geometry {
        Geometry::LineString(line) => Geometry::LineString(one(line, tolerance, &mut work)?),
        Geometry::MultiLineString(lines) => Geometry::MultiLineString(MultiLineString(
            lines
                .0
                .iter()
                .map(|l| one(l, tolerance, &mut work))
                .collect::<Result<_, _>>()?,
        )),
        Geometry::Polygon(p) => Geometry::Polygon(polygon(p, tolerance, &mut work)?),
        Geometry::MultiPolygon(ps) => Geometry::MultiPolygon(MultiPolygon(
            ps.0.iter()
                .map(|p| polygon(p, tolerance, &mut work))
                .collect::<Result<_, _>>()?,
        )),
        other => other.clone(),
    })
}

#[cfg(test)]
#[deny(clippy::disallowed_methods)]
mod tests {
    use super::*;
    #[test]
    fn vw_mixed_scales_do_not_flatten_vertices() {
        let source = Geometry::LineString(LineString::from(vec![
            (0., 0.),
            (1e300, 1e-300),
            (2e300, 0.),
        ]));
        assert_eq!(preserve_topology(&source, 0.5).unwrap(), source);
        assert_eq!(
            preserve_topology(&source, 2.).unwrap(),
            Geometry::LineString(LineString::from(vec![(0., 0.), (2e300, 0.)]))
        );
        let tiny = Geometry::LineString(LineString::from(vec![
            (0., 0.),
            (1e-300, 1e-300),
            (2e-300, 0.),
        ]));
        assert_eq!(
            preserve_topology(&tiny, f64::from_bits(1)).unwrap(),
            Geometry::LineString(LineString::from(vec![(0., 0.), (2e-300, 0.)]))
        );
    }

    #[test]
    fn area_heap_matches_sorted_integer_oracle() {
        let mut work = Work::new(crate::predicates::RelateLimits::default(), None);
        let mut heap = CountedVec::new();
        let mut expected = Vec::new();
        for i in 0..128 {
            let value = (i * 37) % 113;
            let forced = i % 11 == 0;
            expected.push(if forced { -1 } else { value });
            let area = DistanceKey::squared(work.integer(value).unwrap()).unwrap();
            area_push(
                &mut heap,
                AreaEntry {
                    left: 0,
                    current: 0,
                    right: 0,
                    area,
                    forced,
                },
                &mut work,
            )
            .unwrap();
        }
        expected.sort_unstable();
        for value in expected {
            let entry = area_pop(&mut heap, &mut work).unwrap().unwrap();
            if value == -1 {
                assert!(entry.forced);
            } else {
                assert!(!entry.forced);
                assert!(entry
                    .area
                    .compare(&DistanceKey::squared(work.integer(value).unwrap()).unwrap())
                    .unwrap()
                    .is_eq());
            }
        }
        assert!(area_pop(&mut heap, &mut work).unwrap().is_none());
    }

    #[test]
    fn exact_thresholds_at_extreme_scales_keep_original_vertices() {
        for scale in [f64::from_bits(1), 1e-300, 1.0, 1e300] {
            let source = Geometry::LineString(LineString::from(vec![
                (0., 0.),
                (scale, scale),
                (2. * scale, 0.),
            ]));
            let simplified = simplify(&source, scale).unwrap();
            assert_eq!(
                simplified,
                Geometry::LineString(LineString::from(vec![(0., 0.), (2. * scale, 0.)]))
            );
            assert_eq!(simplify(&source, scale.next_down()).unwrap(), source);
        }
    }
}
