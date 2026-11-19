//! Decomposizione con eventi del noder comune, senza grafo materializzato.
//!
//! Le ascisse critiche sono vertici e intersezioni. In una striscia aperta
//! l'ordine verticale dei segmenti non cambia. Sulle rette critiche si
//! visitano anche gli intervalli verticali, quindi nessun contatto sparisce.

// Simboli delle formule razionali e parametriche; uguaglianza delle
// coordinate esatta, non una misura approssimata.
#![allow(clippy::many_single_char_names, clippy::float_cmp)]

use super::interval_index::{BoxIndex, IntervalIndex};
use super::{De9imMatrix, PredicateError};
pub(super) use crate::noding::Work;
use crate::noding::{between, sorted, CountedVec, Edge, P, Q};
use geo::{Coord, CoordsIter, Geometry, LineString, Polygon};
use std::cmp::Ordering;

fn invalid(side: &'static str) -> PredicateError {
    PredicateError::InvalidGeometry {
        side,
        reason: "struttura geometrica non valida".to_owned(),
    }
}

#[derive(Default)]
struct Shape {
    polygon_index: BoxIndex,
    line_index: IntervalIndex,
    end_index: IntervalIndex,
    point_index: IntervalIndex,
    ring_indices: CountedVec<CountedVec<IntervalIndex>>,
    points: CountedVec<P>,
    lines: CountedVec<Edge>,
    ends: CountedVec<P>,
    polygons: CountedVec<CountedVec<CountedVec<Edge>>>,
    groups: CountedVec<Option<usize>>,
    connected: CountedVec<CountedVec<usize>>,
}

fn point(coordinate: Coord<f64>, work: &mut Work<'_>) -> Result<P, PredicateError> {
    work.nodes(1)?;
    P::of_in(coordinate, work)
}

fn push_point(
    points: &mut CountedVec<P>,
    coordinate: Coord<f64>,
    work: &mut Work<'_>,
) -> Result<(), PredicateError> {
    let p = point(coordinate, work)?;
    work.push(points, p)
}

fn ring(line: &[Coord<f64>], work: &mut Work<'_>) -> Result<CountedVec<Edge>, PredicateError> {
    let mut edges = CountedVec::new();
    for w in line.windows(2) {
        work.step()?;
        if w[0] != w[1] {
            let edge = Edge {
                a: point(w[0], work)?,
                b: point(w[1], work)?,
            };
            work.push(&mut edges, edge)?;
        }
    }
    Ok(edges)
}
impl Shape {
    fn build_indices(&mut self, work: &mut Work<'_>) -> Result<(), PredicateError> {
        self.line_index = IntervalIndex::new(self.lines.iter().map(|e| (&e.a.x, &e.b.x)), work)?;
        self.end_index = IntervalIndex::new(self.ends.iter().map(|p| (&p.x, &p.x)), work)?;
        self.point_index = IntervalIndex::new(self.points.iter().map(|p| (&p.x, &p.x)), work)?;
        let mut bounds = CountedVec::new();
        for rings in &self.polygons {
            let mut bb = [
                work.integer(0)?,
                work.integer(0)?,
                work.integer(0)?,
                work.integer(0)?,
            ];
            for (i, p) in rings
                .iter()
                .flatten()
                .flat_map(|e| [&e.a, &e.b])
                .enumerate()
            {
                work.step()?;
                for (j, q) in [&p.x, &p.x, &p.y, &p.y].into_iter().enumerate() {
                    if i == 0
                        || (j % 2 == 0 && q.try_cmp(&bb[j])?.is_lt())
                        || (j % 2 == 1 && q.try_cmp(&bb[j])?.is_gt())
                    {
                        bb[j] = q.try_clone()?;
                    }
                }
            }
            work.push(&mut bounds, bb)?;
        }
        self.polygon_index = BoxIndex::new(bounds, work)?;
        for rings in &self.polygons {
            let mut indices = CountedVec::new();
            for ring in rings {
                let index = IntervalIndex::new(ring.iter().map(|e| (&e.a.y, &e.b.y)), work)?;
                work.push(&mut indices, index)?;
            }
            work.push(&mut self.ring_indices, indices)?;
        }
        Ok(())
    }

    fn boundary_candidates(
        &self,
        p: &P,
        work: &mut Work<'_>,
    ) -> Result<CountedVec<&Edge>, PredicateError> {
        let mut edges = CountedVec::new();
        for id in self.polygon_index.point_candidates(p, work)? {
            let (rings, indices) = (&self.polygons[id], &self.ring_indices[id]);
            for (ring, index) in rings.iter().zip(indices) {
                work.step()?;
                for id in index.candidates(&p.y, work)? {
                    work.push(&mut edges, &ring[id])?;
                }
            }
        }
        Ok(edges)
    }

    fn validate_rings(
        &self,
        side: &'static str,
        work: &mut Work<'_>,
    ) -> Result<(), PredicateError> {
        for ring in self.polygons.iter().flatten() {
            work.step()?;
            if ring.is_empty() {
                continue;
            }
            if ring.len() < 3 {
                return Err(invalid(side));
            }
            let mut edges = CountedVec::new();
            for edge in ring {
                work.push(&mut edges, edge)?;
            }
            crate::noding::visit(&edges, work, |i, j, hits, _work| {
                let (i, j) = (i.min(j), i.max(j));
                let adjacent = j == i + 1 || (i == 0 && j + 1 == ring.len());
                if hits[1].is_some() || (hits[0].is_some() && !adjacent) {
                    return Err(invalid(side));
                }
                Ok(())
            })?;
        }
        Ok(())
    }
    fn validate_cell(
        &self,
        p: &P,
        dimension: u8,
        side: &'static str,
        connected: &mut [CountedVec<usize>],
        work: &mut Work<'_>,
    ) -> Result<(), PredicateError> {
        let mut polygon_positions = CountedVec::new();
        for polygon_index in self.polygon_index.point_candidates(p, work)? {
            let rings = &self.polygons[polygon_index];
            let mut statuses = CountedVec::new();
            for (ring, index) in rings.iter().zip(&self.ring_indices[polygon_index]) {
                let mut boundary = false;
                for id in index.candidates(&p.y, work)? {
                    work.step()?;
                    if ring[id].contains(p)? {
                        boundary = true;
                        break;
                    }
                }
                let status = if boundary {
                    1
                } else if ring_contains(ring, index, p, None, work)? {
                    0
                } else {
                    2
                };
                work.push(&mut statuses, status)?;
            }
            let exterior = statuses.first().copied().unwrap_or(2);
            let mut holes = 0;
            let mut boundaries = usize::from(exterior == 1);
            for status in statuses.iter().skip(1) {
                work.step()?;
                if *status == 0 {
                    holes += 1;
                    if exterior != 0 {
                        return Err(invalid(side));
                    }
                }
                if *status == 1 {
                    boundaries += 1;
                }
            }
            if holes > 1 || (dimension > 0 && boundaries > 1) {
                return Err(invalid(side));
            }
            // Il grafo bipartito anelli/contatti deve essere aciclico:
            // un ciclo separa l'interno del Polygon (SFA 6.1.11.1).
            if dimension == 0 && boundaries > 1 {
                let parents = &mut connected[polygon_index];
                let mut first_root = None;
                for (ring_index, status) in statuses.iter().enumerate() {
                    if *status != 1 {
                        continue;
                    }
                    let mut root = ring_index;
                    while parents[root] != root {
                        work.step()?;
                        root = parents[root];
                    }
                    if let Some(first) = first_root {
                        if root == first {
                            return Err(invalid(side));
                        }
                        parents[root] = first;
                    } else {
                        first_root = Some(root);
                    }
                }
            }
            let position = if boundaries > 0 {
                1
            } else if exterior == 0 && holes == 0 {
                0
            } else {
                2
            };
            if let Some(group) = self.groups[polygon_index] {
                if position == 0 || (dimension > 0 && position == 1) {
                    work.push(&mut polygon_positions, (group, position))?;
                }
            }
        }
        sorted(&mut polygon_positions, |a, b| Ok(a.cmp(b)), work)?;
        for pair in polygon_positions.windows(2) {
            work.step()?;
            if pair[0] == pair[1] {
                return Err(invalid(side));
            }
        }
        Ok(())
    }
    fn polygon(
        &mut self,
        polygon: &Polygon<f64>,
        work: &mut Work<'_>,
    ) -> Result<(), PredicateError> {
        work.step()?;
        if polygon.exterior().0.is_empty() {
            return Ok(());
        }
        let mut rings = CountedVec::new();
        for line in std::iter::once(polygon.exterior()).chain(polygon.interiors()) {
            work.step()?;
            let edges = ring(&line.0, work)?;
            work.push(&mut rings, edges)?;
        }
        self.add_polygon(rings, work)
    }
    fn add_polygon(
        &mut self,
        rings: CountedVec<CountedVec<Edge>>,
        work: &mut Work<'_>,
    ) -> Result<(), PredicateError> {
        let mut parents = CountedVec::new();
        for index in 0..rings.len() {
            work.push(&mut parents, index)?;
        }
        work.push(&mut self.connected, parents)?;
        work.push(&mut self.polygons, rings)?;
        work.push(&mut self.groups, None)
    }
    fn polygon_coords(
        &mut self,
        coords: &[Coord<f64>],
        work: &mut Work<'_>,
    ) -> Result<(), PredicateError> {
        let edges = ring(coords, work)?;
        let mut rings = CountedVec::new();
        work.push(&mut rings, edges)?;
        self.add_polygon(rings, work)
    }
    fn segment(
        &mut self,
        a: Coord<f64>,
        b: Coord<f64>,
        work: &mut Work<'_>,
    ) -> Result<(), PredicateError> {
        let edge = Edge {
            a: point(a, work)?,
            b: point(b, work)?,
        };
        work.push(&mut self.lines, edge)?;
        push_point(&mut self.ends, a, work)?;
        push_point(&mut self.ends, b, work)
    }
    fn line(&mut self, line: &LineString<f64>, work: &mut Work<'_>) -> Result<(), PredicateError> {
        work.step()?;
        for e in ring(&line.0, work)? {
            work.push(&mut self.lines, e)?;
        }
        if let (Some(a), Some(b)) = (line.0.first(), line.0.last()) {
            push_point(&mut self.ends, *a, work)?;
            push_point(&mut self.ends, *b, work)?;
        }
        Ok(())
    }
    fn geometry(
        &mut self,
        geometry: &Geometry<f64>,
        work: &mut Work<'_>,
    ) -> Result<(), PredicateError> {
        let mut pending = CountedVec::new();
        work.push(&mut pending, geometry)?;
        while let Some(geometry) = pending.pop() {
            work.step()?;
            match geometry {
                Geometry::Point(p) => push_point(&mut self.points, p.0, work)?,
                Geometry::Line(l) => self.segment(l.start, l.end, work)?,
                Geometry::LineString(l) => self.line(l, work)?,
                Geometry::Polygon(p) => self.polygon(p, work)?,
                Geometry::MultiPoint(ps) => {
                    for p in &ps.0 {
                        push_point(&mut self.points, p.0, work)?;
                    }
                }
                Geometry::MultiLineString(ls) => {
                    for l in &ls.0 {
                        self.line(l, work)?;
                    }
                }
                Geometry::MultiPolygon(ps) => {
                    let group = self.polygons.len();
                    for p in &ps.0 {
                        let before = self.polygons.len();
                        self.polygon(p, work)?;
                        if self.polygons.len() != before {
                            self.groups[before] = Some(group);
                        }
                    }
                }
                Geometry::GeometryCollection(gs) => {
                    for g in gs.0.iter().rev() {
                        work.push(&mut pending, g)?;
                    }
                }
                Geometry::Rect(r) => {
                    if r.min() == r.max() {
                        push_point(&mut self.points, r.min(), work)?;
                    } else if r.min().x == r.max().x || r.min().y == r.max().y {
                        self.segment(r.min(), r.max(), work)?;
                    } else {
                        let min = r.min();
                        let max = r.max();
                        self.polygon_coords(
                            &[
                                min,
                                Coord { x: max.x, y: min.y },
                                max,
                                Coord { x: min.x, y: max.y },
                                min,
                            ],
                            work,
                        )?;
                    }
                }
                Geometry::Triangle(t) => {
                    self.polygon_coords(&[t.v1(), t.v2(), t.v3(), t.v1()], work)?;
                }
            }
        }
        Ok(())
    }
    fn edges(&self) -> impl Iterator<Item = &Edge> {
        self.lines
            .iter()
            .chain(self.polygons.iter().flatten().flatten())
    }
    fn area_contains(
        &self,
        p: &P,
        direction: Option<&P>,
        work: &mut Work<'_>,
    ) -> Result<bool, PredicateError> {
        for id in self.polygon_index.point_candidates(p, work)? {
            let (rings, indices) = (&self.polygons[id], &self.ring_indices[id]);
            work.step()?;
            if let Some(exterior) = rings.first() {
                if ring_contains(exterior, &indices[0], p, direction, work)? {
                    let mut hole = false;
                    for (r, index) in rings.iter().zip(indices).skip(1) {
                        if ring_contains(r, index, p, direction, work)? {
                            hole = true;
                            break;
                        }
                    }
                    if !hole {
                        return Ok(true);
                    }
                }
            }
        }
        Ok(false)
    }
    fn position(&self, p: &P, work: &mut Work<'_>) -> Result<usize, PredicateError> {
        let mut boundary = false;
        let candidates = self.boundary_candidates(p, work)?;
        for e in &candidates {
            work.step()?;
            if e.contains(p)? {
                boundary = true;
                break;
            }
        }
        if boundary {
            // Tutte le direzioni incidenti partizionano il giro locale.
            // Gli assi garantiscono settori di ampiezza minore di pi greco:
            // la somma dei raggi adiacenti e' strettamente nel settore.
            let mut rays = CountedVec::new();
            for (x, y) in [(1, 0), (0, 1), (-1, 0), (0, -1)] {
                work.push(
                    &mut rays,
                    P {
                        x: work.integer(x)?,
                        y: work.integer(y)?,
                    },
                )?;
            }
            for edge in &candidates {
                work.step()?;
                if !edge.contains(p)? {
                    continue;
                }
                for endpoint in [&edge.a, &edge.b] {
                    if endpoint != p {
                        work.push(&mut rays, endpoint.try_sub(p)?)?;
                    }
                }
            }
            sorted(&mut rays, angle_cmp, work)?;
            rays.dedup_by(|a, b| Ok(angle_cmp(a, b)? == Ordering::Equal), work)?;
            for (a, b) in rays.iter().zip(rays.iter().cycle().skip(1)) {
                if !self.area_contains(p, Some(&a.try_add(b)?), work)? {
                    return Ok(1);
                }
            }
            return Ok(0);
        }
        if self.area_contains(p, None, work)? {
            return Ok(0);
        }
        for id in self.line_index.candidates(&p.x, work)? {
            let e = &self.lines[id];
            work.step()?;
            if e.contains(p)? {
                let mut odd = false;
                for id in self.end_index.candidates(&p.x, work)? {
                    let q = &self.ends[id];
                    work.step()?;
                    if q == p {
                        odd = !odd;
                    }
                }
                return Ok(usize::from(odd));
            }
        }
        for id in self.point_index.candidates(&p.x, work)? {
            let q = &self.points[id];
            work.step()?;
            if q == p {
                return Ok(0);
            }
        }
        Ok(2)
    }
}
fn angle_cmp(a: &P, b: &P) -> Result<Ordering, PredicateError> {
    let half = |p: &P| {
        p.y.sign() == Ordering::Less
            || (p.y.sign() == Ordering::Equal && p.x.sign() == Ordering::Less)
    };
    let halves = half(a).cmp(&half(b));
    if halves.is_eq() {
        Ok(a.try_cross(b)?.sign().reverse())
    } else {
        Ok(halves)
    }
}
fn perturbed_sign(value: &Q, delta: &Q) -> Ordering {
    let sign = value.sign();
    if sign == Ordering::Equal {
        delta.sign()
    } else {
        sign
    }
}
fn ring_contains(
    ring: &[Edge],
    index: &IntervalIndex,
    p: &P,
    direction: Option<&P>,
    work: &mut Work<'_>,
) -> Result<bool, PredicateError> {
    work.step()?;
    let zero = P {
        x: work.integer(0)?,
        y: work.integer(0)?,
    };
    let d = direction.unwrap_or(&zero);
    let mut inside = false;
    for id in index.candidates(&p.y, work)? {
        let e = &ring[id];
        work.step()?;
        let above = |y: &Q| -> Result<bool, PredicateError> {
            Ok(perturbed_sign(&p.y.try_sub(y)?, &d.y) == Ordering::Less)
        };
        if above(&e.a.y)? != above(&e.b.y)? {
            let edge = e.b.try_sub(&e.a)?;
            let side = perturbed_sign(&edge.try_cross(&p.try_sub(&e.a)?)?, &edge.try_cross(d)?);
            if side == e.b.y.try_cmp(&e.a.y)? {
                inside = !inside;
            }
        }
    }
    Ok(inside)
}

fn finite_geometry(
    geometry: &Geometry<f64>,
    side: &'static str,
    work: &mut Work<'_>,
) -> Result<(), PredicateError> {
    fn finite(
        coords: impl Iterator<Item = Coord<f64>>,
        side: &'static str,
        work: &mut Work<'_>,
    ) -> Result<(), PredicateError> {
        work.step()?;
        for c in coords {
            work.step()?;
            if !c.x.is_finite() || !c.y.is_finite() {
                return Err(PredicateError::NonFiniteCoordinate { side });
            }
        }
        Ok(())
    }
    fn polygon(
        p: &Polygon<f64>,
        side: &'static str,
        work: &mut Work<'_>,
    ) -> Result<(), PredicateError> {
        work.step()?;
        for ring in std::iter::once(p.exterior()).chain(p.interiors()) {
            finite(ring.coords_iter(), side, work)?;
        }
        Ok(())
    }
    match geometry {
        Geometry::Polygon(p) => polygon(p, side, work)?,
        Geometry::MultiPolygon(ps) => {
            for p in &ps.0 {
                polygon(p, side, work)?;
            }
        }
        Geometry::MultiLineString(ls) => {
            for l in &ls.0 {
                finite(l.coords_iter(), side, work)?;
            }
        }
        _ => finite(geometry.coords_iter(), side, work)?,
    }
    Ok(())
}

fn validate_structure(
    geometry: &Geometry<f64>,
    side: &'static str,
    work: &mut Work<'_>,
) -> Result<(), PredicateError> {
    let mut pending = CountedVec::new();
    work.push(&mut pending, geometry)?;
    while let Some(geometry) = pending.pop() {
        if let Geometry::GeometryCollection(gs) = geometry {
            for g in gs.0.iter().rev() {
                work.push(&mut pending, g)?;
            }
            continue;
        }
        finite_geometry(geometry, side, work)?;
        let line_valid =
            |line: &LineString<f64>, work: &mut Work<'_>| -> Result<(), PredicateError> {
                work.step()?;
                if let Some(first) = line.0.first() {
                    for p in &line.0 {
                        work.step()?;
                        if p != first {
                            return Ok(());
                        }
                    }
                    return Err(invalid(side));
                }
                Ok(())
            };
        let polygon_valid = |p: &Polygon<f64>, work: &mut Work<'_>| -> Result<(), PredicateError> {
            work.step()?;
            if p.exterior().0.is_empty() && p.interiors().is_empty() {
                return Ok(());
            }
            for r in std::iter::once(p.exterior()).chain(p.interiors()) {
                work.step()?;
                if r.0.len() < 4 || r.0.first() != r.0.last() {
                    return Err(invalid(side));
                }
                let mut edges = 0;
                for pair in r.0.windows(2) {
                    work.step()?;
                    if pair[0] != pair[1] {
                        edges += 1;
                    }
                }
                if edges < 3 {
                    return Err(invalid(side));
                }
            }
            Ok(())
        };
        match geometry {
            Geometry::Line(l) if l.start == l.end => return Err(invalid(side)),
            Geometry::LineString(l) => line_valid(l, work)?,
            Geometry::MultiLineString(ls) => {
                for l in &ls.0 {
                    line_valid(l, work)?;
                }
            }
            Geometry::Polygon(p) => polygon_valid(p, work)?,
            Geometry::MultiPolygon(ps) => {
                for p in &ps.0 {
                    polygon_valid(p, work)?;
                }
            }
            Geometry::Triangle(t)
                if point(t.v2(), work)?
                    .try_sub(&point(t.v1(), work)?)?
                    .try_cross(&point(t.v3(), work)?.try_sub(&point(t.v1(), work)?)?)?
                    .sign()
                    == Ordering::Equal =>
            {
                return Err(invalid(side));
            }
            _ => {}
        }
    }
    Ok(())
}

fn samples(
    mut values: CountedVec<Q>,
    work: &mut Work<'_>,
) -> Result<CountedVec<(Q, bool)>, PredicateError> {
    sorted(&mut values, Q::try_cmp, work)?;
    values.dedup_by(|a, b| Ok(a == b), work)?;
    let mut result = CountedVec::new();
    if let Some(first) = values.first() {
        work.push(&mut result, (first.try_sub(&work.integer(1)?)?, false))?;
    }
    for (i, value) in values.iter().enumerate() {
        work.push(&mut result, (value.try_clone()?, true))?;
        if let Some(next) = values.get(i + 1) {
            work.push(&mut result, (value.try_midpoint(next)?, false))?;
        }
    }
    if let Some(last) = values.last() {
        work.push(&mut result, (last.try_add(&work.integer(1)?)?, false))?;
    }
    if values.is_empty() {
        work.push(&mut result, (work.integer(0)?, false))?;
    }
    Ok(result)
}

pub fn relate(
    left: &Geometry<f64>,
    right: &Geometry<f64>,
    work: &mut Work<'_>,
) -> Result<De9imMatrix, PredicateError> {
    relate_impl(left, right, work, true)
}

#[cfg(test)]
#[deny(clippy::disallowed_methods)]
pub(super) fn relate_validated(
    left: &Geometry<f64>,
    right: &Geometry<f64>,
    work: &mut Work<'_>,
) -> Result<De9imMatrix, PredicateError> {
    relate_impl(left, right, work, false)
}

fn relate_impl(
    left: &Geometry<f64>,
    right: &Geometry<f64>,
    work: &mut Work<'_>,
    check_input: bool,
) -> Result<De9imMatrix, PredicateError> {
    let mut a = Shape::default();
    let mut b = Shape::default();
    if check_input {
        validate_structure(left, "left", work)?;
        validate_structure(right, "right", work)?;
    }
    a.geometry(left, work)?;
    b.geometry(right, work)?;
    a.build_indices(work)?;
    b.build_indices(work)?;
    if check_input {
        a.validate_rings("left", work)?;
        b.validate_rings("right", work)?;
    }
    let mut a_connected = std::mem::take(&mut a.connected);
    let mut b_connected = std::mem::take(&mut b.connected);
    let mut edges = CountedVec::new();
    for e in a.edges().chain(b.edges()) {
        work.push(&mut edges, e)?;
    }
    let mut points = CountedVec::new();
    for p in a.points.iter().chain(&b.points) {
        work.push(&mut points, p)?;
    }
    let mut xs = CountedVec::new();
    for x in edges
        .iter()
        .flat_map(|e| [&e.a.x, &e.b.x])
        .chain(points.iter().map(|p| &p.x))
    {
        work.push(&mut xs, x.try_clone()?)?;
    }
    crate::noding::visit(&edges, work, |_, _, hits, work| {
        for point in hits.into_iter().flatten() {
            work.push(&mut xs, point.x)?;
        }
        Ok(())
    })?;
    let edge_index = IntervalIndex::new(edges.iter().map(|e| (&e.a.x, &e.b.x)), work)?;
    let point_index = IntervalIndex::new(points.iter().map(|p| (&p.x, &p.x)), work)?;
    let mut cells = [b'F'; 9];
    cells[8] = b'2';
    for (x, critical_x) in samples(xs, work)? {
        work.step()?;
        let mut ys = CountedVec::new();
        for id in edge_index.candidates(&x, work)? {
            let e = edges[id];
            work.step()?;
            if between(&x, &e.a.x, &e.b.x)? {
                if e.a.x == e.b.x {
                    work.push(&mut ys, e.a.y.try_clone()?)?;
                    work.push(&mut ys, e.b.y.try_clone()?)?;
                } else {
                    work.push(&mut ys, e.at_x(&x)?)?;
                }
            }
        }
        for id in point_index.candidates(&x, work)? {
            let p = points[id];
            work.step()?;
            if p.x == x {
                work.push(&mut ys, p.y.try_clone()?)?;
            }
        }
        for (y, critical_y) in samples(ys, work)? {
            work.step()?;
            let p = P {
                x: x.try_clone()?,
                y,
            };
            let dimension = b'0' + u8::from(!critical_x) + u8::from(!critical_y);
            if check_input {
                a.validate_cell(&p, dimension - b'0', "left", &mut a_connected, work)?;
                b.validate_cell(&p, dimension - b'0', "right", &mut b_connected, work)?;
            }
            let index = a.position(&p, work)? * 3 + b.position(&p, work)?;
            if cells[index] == b'F' || cells[index] < dimension {
                cells[index] = dimension;
            }
        }
    }
    work.step()?;
    let text = std::str::from_utf8(&cells)
        .map_err(|_| PredicateError::CalcoloNonConcluso("celle DE-9IM"))?;
    text.parse()
        .map(De9imMatrix)
        .map_err(|_| PredicateError::CalcoloNonConcluso("celle DE-9IM"))
}

#[deny(clippy::disallowed_methods)]
#[cfg(test)]
mod tests {
    use super::*;
    use crate::predicates::RelateLimits;
    use geo::line_string;
    use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
    use std::vec::Vec;
    use wkt::TryFromWkt as _;

    fn sparse(count: i32) -> Geometry<f64> {
        Geometry::MultiLineString(geo::MultiLineString(
            (0..count)
                .map(|i| {
                    let x = f64::from(i).mul_add(4.0, 16.0);
                    line_string![(x:x, y:0.0), (x:x+1.0, y:1.0)]
                })
                .collect(),
        ))
    }

    #[test]
    fn sparse_work_and_exact_public_threshold() {
        let left = geometry("LINESTRING(0 0,1 0)");
        for count in [16, 64, 256, 4096] {
            let right = sparse(count);
            let mut work = Work::new(RelateLimits::default(), None);
            work.step().expect("passo API");
            let matrix = relate(&left, &right, &mut work).expect("limiti predefiniti");
            assert_eq!(matrix.to_string(), "FF1FF0102");
            let threshold = work.steps;
            let public = super::super::relate_matrix_with_limits;
            assert_eq!(
                public(
                    &left,
                    &right,
                    RelateLimits {
                        max_steps: threshold,
                        ..RelateLimits::default()
                    },
                    None
                )
                .expect("soglia"),
                matrix
            );
            assert!(matches!(
                public(
                    &left,
                    &right,
                    RelateLimits {
                        max_steps: threshold - 1,
                        ..RelateLimits::default()
                    },
                    None
                ),
                Err(PredicateError::WorkLimit)
            ));
            if count <= 256 {
                let mut before = Work::new(RelateLimits::default(), None);
                before.step().expect("passo API");
                let baseline =
                    super::super::arrangement_exhaustive::relate(&left, &right, &mut before)
                        .expect("esaustivo");
                assert_eq!(matrix, baseline);
                println!("sparso{count}: prima={}, dopo={threshold}", before.steps);
                assert!(threshold < before.steps);
            } else {
                println!(
                    "sparso{count}: dopo={threshold}, limite={}",
                    RelateLimits::default().max_steps
                );
            }
        }
    }

    fn relate_matrix(a: &Geometry<f64>, b: &Geometry<f64>) -> Result<De9imMatrix, PredicateError> {
        let result = relate(a, b, &mut Work::new(RelateLimits::default(), None));
        let oracle = super::super::arrangement_exhaustive::relate(
            a,
            b,
            &mut Work::new(RelateLimits::default(), None),
        );
        assert_eq!(
            result.as_ref().map_err(ToString::to_string),
            oracle.as_ref().map_err(ToString::to_string),
            "classificazione esaustiva"
        );
        result
    }

    fn geometry(text: &str) -> Geometry<f64> {
        Geometry::try_from_wkt_str(text).expect("WKT costruito nella prova")
    }

    /// D-a: la 1a non e' rilasciata. Il conteggio puo' cambiare; la matrice
    /// analitica dei due intervalli disgiunti resta FF1FF0102.
    #[test]
    fn unreleased_limits_may_change_without_changing_the_matrix() {
        let a = geometry("LINESTRING(0 0,1 0)");
        let b = geometry("LINESTRING(3 0,4 0)");
        let baseline = |bytes| {
            let limits = RelateLimits {
                max_reserved_bytes: bytes,
                ..RelateLimits::default()
            };
            super::super::arrangement::relate(
                &a,
                &b,
                &mut super::super::arrangement::Work::new(limits, None),
            )
        };
        let mut low = 0;
        let mut high = RelateLimits::default().max_reserved_bytes;
        assert_eq!(baseline(high).expect("baseline").to_string(), "FF1FF0102");
        while low < high {
            let mid = low + (high - low) / 2;
            match baseline(mid) {
                Ok(matrix) => {
                    assert_eq!(matrix.to_string(), "FF1FF0102");
                    high = mid;
                }
                error => {
                    assert!(matches!(error, Err(PredicateError::ReservationLimit)));
                    low = mid + 1;
                }
            }
        }
        assert_eq!(
            baseline(low).expect("soglia baseline").to_string(),
            "FF1FF0102"
        );
        // La soglia della 1a non e' un vincolo della nuova API.
        assert_eq!(
            relate_matrix(&a, &b)
                .expect("default del noder")
                .to_string(),
            "FF1FF0102"
        );
    }

    fn assert_named(a: &Geometry<f64>, b: &Geometry<f64>, matrix: &De9imMatrix) {
        use super::super::{exact_predicate, exact_predicate_validated, SpatialPredicate as S};
        for predicate in [
            S::Intersects,
            S::Disjoint,
            S::Contains,
            S::Within,
            S::EqualsTopo,
            S::Covers,
            S::CoveredBy,
            S::ContainsProperly,
            S::Touches,
            S::Crosses,
            S::Overlaps,
        ] {
            assert_eq!(
                matrix.evaluate(predicate),
                exact_predicate(a, b, predicate).expect("predicato del corpus")
            );
            assert_eq!(
                matrix.evaluate(predicate),
                exact_predicate_validated(a, b, predicate).expect("predicato validato del corpus")
            );
        }
    }

    /// SFA 6.1.15.2: due intervalli si incrociano in un punto interno.
    /// Coordinate razionali, incrocio (1/3,1/3) non rappresentabile f64.
    #[test]
    fn rational_intersection_and_full_finite_exponent_range() {
        let left = geometry("LINESTRING(0 0,1 1)");
        let right = geometry("LINESTRING(0 1,1 -1)");
        assert_named(
            &left,
            &right,
            &relate_matrix(&left, &right).expect("matrice razionale"),
        );
        assert_eq!(
            relate_matrix(&left, &right).expect("matrice").to_string(),
            "0F1FF0102"
        );
        // La stessa costruzione a esponenti estremi; nessun overflow,
        // sottoflusso o quantizzazione del punto di intersezione.
        for scale in [f64::from_bits(1), f64::MAX] {
            let a = Geometry::Line(geo::Line::new((0.0, 0.0), (scale, scale)));
            let b = Geometry::Line(geo::Line::new((0.0, scale), (scale, -scale)));
            assert_named(&a, &b, &relate_matrix(&a, &b).expect("matrice estrema"));
            assert_eq!(
                relate_matrix(&a, &b).expect("matrice estrema").to_string(),
                "0F1FF0102"
            );
            assert!(
                super::super::exact_predicate(&a, &b, super::super::SpatialPredicate::Crosses)
                    .expect("predicato estremo")
            );
        }
    }

    /// Contratto delle risorse: nessun rifiuto da maggioranti. Con due
    /// segmenti il noding valuta esattamente una coppia, non quattro.
    #[test]
    fn actual_work_and_reservation_thresholds() {
        let a = geometry("LINESTRING(0 0,1 1)");
        let b = geometry("LINESTRING(0 1,1 0)");
        let mut work = Work::new(RelateLimits::default(), None);
        let expected = relate(&a, &b, &mut work).expect("misura");
        assert_eq!(work.pairs, 1);
        let exact = RelateLimits {
            max_segment_pairs: work.pairs,
            max_steps: work.steps,
            max_reserved_bytes: work.peak_bytes(),
            ..RelateLimits::default()
        };
        assert_eq!(
            relate(&a, &b, &mut Work::new(exact, None)).expect("limite esatto"),
            expected
        );
        for limits in [
            RelateLimits {
                max_segment_pairs: 0,
                ..exact
            },
            RelateLimits {
                max_steps: work.steps - 1,
                ..exact
            },
        ] {
            assert!(matches!(
                relate(&a, &b, &mut Work::new(limits, None)),
                Err(PredicateError::WorkLimit)
            ));
        }
        let limits = RelateLimits {
            max_reserved_bytes: work.peak_bytes() - 1,
            ..exact
        };
        assert!(matches!(
            relate(&a, &b, &mut Work::new(limits, None)),
            Err(PredicateError::ReservationLimit)
        ));
        let point = geometry("POINT(0 0)");
        let limits = RelateLimits {
            max_segment_pairs: 0,
            ..RelateLimits::default()
        };
        assert_eq!(
            relate(&point, &point, &mut Work::new(limits, None))
                .expect("nessuna coppia")
                .to_string(),
            "0FFFFFFF2"
        );
    }

    /// Contratto di annullamento: il flag puo' cambiare dopo passi riusciti;
    /// il prossimo passo rifiuta con la variante tipizzata, senza matrice.
    #[test]
    fn cancellation_after_progress_and_at_public_entry() {
        let flag = AtomicBool::new(false);
        let mut work = Work::new(RelateLimits::default(), Some(&flag));
        work.step().expect("passo prima della cancellazione");
        flag.store(true, AtomicOrdering::Relaxed);
        assert!(matches!(work.pair(), Err(PredicateError::Cancelled)));
        let point = geometry("POINT(0 0)");
        assert!(matches!(
            super::super::relate_matrix_with_limits(
                &point,
                &point,
                RelateLimits::default(),
                Some(&flag)
            ),
            Err(PredicateError::Cancelled)
        ));
    }

    /// D-b/GOP-028: rifiuto pubblico su zero nodi e deadline gia' raggiunta;
    /// nessuna geometria o coordinata viene riportata nell'errore.
    #[test]
    fn node_limit_and_deadline_at_public_entry() {
        let p = geometry("POINT(0 0)");
        let limits = RelateLimits {
            max_nodes: 0,
            ..RelateLimits::default()
        };
        assert!(matches!(
            super::super::relate_matrix_with_limits(&p, &p, limits, None),
            Err(PredicateError::WorkLimit)
        ));
        let limits = RelateLimits {
            deadline: Some(std::time::Instant::now()),
            ..RelateLimits::default()
        };
        assert!(matches!(
            super::super::relate_matrix_with_limits(&p, &p, limits, None),
            Err(PredicateError::DeadlineExceeded)
        ));
    }

    /// Contratto di costo: anche membri e anelli vuoti sono visite reali.
    #[test]
    fn empty_members_cannot_bypass_the_work_limit() {
        let empty = LineString::new(Vec::new());
        let point = geometry("POINT(0 0)");
        let values = [
            Geometry::MultiLineString(geo::MultiLineString(vec![empty.clone(); 100])),
            Geometry::MultiPolygon(geo::MultiPolygon(vec![Polygon::empty(); 100])),
            Geometry::Polygon(Polygon::new(
                geo::line_string![(x:0.0,y:0.0),(x:2.0,y:0.0),(x:0.0,y:2.0),(x:0.0,y:0.0)],
                vec![empty; 100],
            )),
        ];
        for value in values {
            let limits = RelateLimits {
                max_steps: 10,
                ..RelateLimits::default()
            };
            assert!(matches!(
                super::super::relate_matrix_with_limits(&value, &point, limits, None),
                Err(PredicateError::WorkLimit)
            ));
        }
    }

    /// SFA 6.1.11.1 e 6.1.14: anelli semplici, buchi contenuti,
    /// interno connesso, membri `MultiPolygon` senza sovrapposizioni.
    #[test]
    fn invalid_polygon_topology_is_not_silently_classified() {
        let point = geometry("POINT(0 0)");
        // SFA 6.1.11.1 e geometry_contract::check_polygon: il vuoto
        // ha zero anelli; ogni anello presente deve essere chiuso e valido.
        let triangle = line_string![(x:0.0,y:0.0),(x:2.0,y:0.0),(x:0.0,y:2.0),(x:0.0,y:0.0)];
        for polygon in [
            Polygon::new(LineString::new(vec![]), vec![triangle.clone()]),
            Polygon::new(LineString::new(vec![]), vec![LineString::new(vec![])]),
            Polygon::new(triangle, vec![LineString::new(vec![])]),
        ] {
            assert!(matches!(
                relate_matrix(&Geometry::Polygon(polygon), &point),
                Err(PredicateError::InvalidGeometry { side: "left", .. })
            ));
        }
        for text in [
            "POLYGON((0 0,0 0,0 0,0 0))",
            "POLYGON((0 0,4 0,4 4,0 4,0 0),(0 2,2 0,4 2,2 4,0 2))",
            "POLYGON((0 0,4 0,4 4,0 4,0 0),(5 5,6 5,6 6,5 6,5 5))",
            "POLYGON((0 0,4 0,4 4,0 4,0 0),(1 1,3 1,3 3,1 3,1 1),(2 2,3 2,3 3,2 3,2 2))",
            "MULTIPOLYGON(((0 0,2 0,2 2,0 2,0 0)),((1 0,3 0,3 2,1 2,1 0)))",
        ] {
            let value = geometry(text);
            assert!(
                matches!(
                    relate_matrix(&value, &point),
                    Err(PredicateError::InvalidGeometry { side: "left", .. })
                ),
                "{text}"
            );
        }
    }

    /// SFA 6.1.15.2-3: coerenza di ogni predicato su ogni coppia del
    /// corpus, non usata per produrre le attese analitiche C1 separate.
    #[test]
    fn every_named_predicate_on_the_cartesian_corpus() {
        use super::super::{exact_predicate, exact_predicate_validated, SpatialPredicate as S};
        let mut corpus: Vec<_> = [
            "POINT(0 0)",
            "POINT(1 1)",
            "MULTIPOINT((0 0),(1 1))",
            "LINESTRING(0 0,2 0)",
            "LINESTRING(1 -1,1 1)",
            "LINESTRING(0 0,2 0,1 2,0 0)",
            "MULTILINESTRING((0 0,1 0),(1 0,2 0))",
            "POLYGON((0 0,2 0,2 2,0 2,0 0))",
            "POLYGON((0 0,4 0,4 4,0 4,0 0),(1 1,1 3,3 3,3 1,1 1))",
            "GEOMETRYCOLLECTION EMPTY",
            "LINESTRING EMPTY",
            "POLYGON EMPTY",
            "GEOMETRYCOLLECTION(POINT(3 0),LINESTRING(0 0,2 0))",
            "MULTIPOLYGON(((0 0,1 0,1 1,0 1,0 0)),((2 2,3 2,3 3,2 3,2 2)))",
        ]
        .into_iter()
        .map(geometry)
        .collect();
        corpus.push(Geometry::Rect(geo::Rect::new((0.0, 0.0), (2.0, 0.0))));
        corpus.push(Geometry::Rect(geo::Rect::new((1.0, 1.0), (1.0, 1.0))));
        corpus.push(Geometry::Triangle(geo::Triangle::new(
            (0.0, 0.0).into(),
            (2.0, 0.0).into(),
            (0.0, 2.0).into(),
        )));
        corpus.push(Geometry::Line(geo::Line::new((0.0, 0.0), (2.0, 0.0))));
        for a in &corpus {
            for b in &corpus {
                let matrix = relate_matrix(a, b).expect("matrice del corpus");
                assert_eq!(
                    matrix,
                    relate_validated(a, b, &mut Work::new(RelateLimits::default(), None))
                        .expect("indice validato")
                );
                let mut baseline =
                    super::super::arrangement::Work::new(RelateLimits::default(), None);
                assert_eq!(
                    matrix,
                    super::super::arrangement::relate(a, b, &mut baseline).expect("baseline 1a")
                );
                let mut baseline =
                    super::super::arrangement::Work::new(RelateLimits::default(), None);
                assert_eq!(
                    matrix,
                    super::super::arrangement::relate_validated(a, b, &mut baseline)
                        .expect("baseline validata 1a")
                );
                for predicate in [
                    S::Intersects,
                    S::Disjoint,
                    S::Contains,
                    S::Within,
                    S::EqualsTopo,
                    S::Covers,
                    S::CoveredBy,
                    S::ContainsProperly,
                    S::Touches,
                    S::Crosses,
                    S::Overlaps,
                ] {
                    assert_eq!(
                        matrix.evaluate(predicate),
                        exact_predicate(a, b, predicate).expect("predicato del corpus"),
                        "{a:?} {b:?} {predicate:?}"
                    );
                    assert_eq!(
                        matrix.evaluate(predicate),
                        exact_predicate_validated(a, b, predicate)
                            .expect("predicato validato del corpus")
                    );
                }
            }
        }
    }

    /// SFA 6.1.3.2: permutazioni e annidamento non cambiano l'insieme.
    #[test]
    fn nested_members_and_permutations_preserve_cells() {
        let a = geometry("GEOMETRYCOLLECTION(POLYGON((0 0,2 0,2 2,0 2,0 0)),POLYGON((1 0,3 0,3 2,1 2,1 0)),POINT(4 4))");
        let b = geometry("GEOMETRYCOLLECTION(POINT(4 4),GEOMETRYCOLLECTION(POLYGON((1 0,3 0,3 2,1 2,1 0)),POLYGON((0 0,2 0,2 2,0 2,0 0))))");
        for other in [
            "POINT(1 1)",
            "LINESTRING(1 0,1 2)",
            "POLYGON((0 0,3 0,3 2,0 2,0 0))",
        ] {
            let other = geometry(other);
            assert_eq!(
                relate_matrix(&a, &other).expect("prima"),
                relate_matrix(&b, &other).expect("permutata")
            );
        }
    }
}

/// Booleane planari con almeno un operando puntuale.
pub mod point_boolean;

pub mod general_boolean;
pub mod line_boolean;

pub mod polygon_boolean;
/// Completamento planare delle facce con provenienza dei lati.
pub mod polygon_completion;
