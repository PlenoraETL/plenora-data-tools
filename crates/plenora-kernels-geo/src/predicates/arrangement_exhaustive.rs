//! Decomposizione con eventi del noder comune, senza grafo materializzato.
//!
//! Le ascisse critiche sono vertici e intersezioni. In una striscia aperta
//! l'ordine verticale dei segmenti non cambia. Sulle rette critiche si
//! visitano anche gli intervalli verticali, quindi nessun contatto sparisce.

// Simboli delle formule razionali e parametriche; uguaglianza delle
// coordinate esatta, non una misura approssimata.
#![allow(clippy::many_single_char_names, clippy::float_cmp)]

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
        for (polygon_index, rings) in self.polygons.iter().enumerate() {
            let mut statuses = CountedVec::new();
            for ring in rings {
                let mut boundary = false;
                for edge in ring {
                    work.step()?;
                    if edge.contains(p)? {
                        boundary = true;
                        break;
                    }
                }
                let status = if boundary {
                    1
                } else if ring_contains(ring, p, None, work)? {
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
            work.push(&mut polygon_positions, position)?;
        }
        for (i, group) in self.groups.iter().enumerate() {
            if group.is_none() {
                continue;
            }
            for j in i + 1..self.groups.len() {
                work.step()?;
                if self.groups[j] == *group
                    && ((polygon_positions[i] == 0 && polygon_positions[j] == 0)
                        || (dimension > 0
                            && polygon_positions[i] == 1
                            && polygon_positions[j] == 1))
                {
                    return Err(invalid(side));
                }
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
        for rings in &self.polygons {
            work.step()?;
            if let Some(exterior) = rings.first() {
                if ring_contains(exterior, p, direction, work)? {
                    let mut hole = false;
                    for r in rings.iter().skip(1) {
                        if ring_contains(r, p, direction, work)? {
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
        for e in self.polygons.iter().flatten().flatten() {
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
            for edge in self.polygons.iter().flatten().flatten() {
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
        for e in &self.lines {
            work.step()?;
            if e.contains(p)? {
                let mut odd = false;
                for q in &self.ends {
                    work.step()?;
                    if q == p {
                        odd = !odd;
                    }
                }
                return Ok(usize::from(odd));
            }
        }
        for q in &self.points {
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
    for e in ring {
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
    let mut cells = [b'F'; 9];
    cells[8] = b'2';
    for (x, critical_x) in samples(xs, work)? {
        work.step()?;
        let mut ys = CountedVec::new();
        for e in &edges {
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
        for p in &points {
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
