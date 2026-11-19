//! Decomposizione verticale esatta: una rappresentante per ogni cella.
//!
//! Le ascisse critiche sono vertici e intersezioni. In una striscia aperta
//! l'ordine verticale dei segmenti non cambia. Sulle rette critiche si
//! visitano anche gli intervalli verticali, quindi nessun contatto sparisce.

// Simboli delle formule razionali e parametriche; uguaglianza delle
// coordinate esatta, non una misura approssimata.
#![allow(clippy::many_single_char_names, clippy::float_cmp)]

use super::{De9imMatrix, PredicateError, RelateLimits};
use geo::{Coord, CoordsIter, Geometry, LineString, Polygon};
use num_bigint::{BigInt, Sign};
use std::cmp::Ordering;

pub(super) struct Work<'a> {
    limits: RelateLimits,
    cancelled: Option<&'a std::sync::atomic::AtomicBool>,
    steps: u64,
    pairs: u64,
    bytes: u64,
}
impl<'a> Work<'a> {
    pub(super) const fn new(
        limits: RelateLimits,
        cancelled: Option<&'a std::sync::atomic::AtomicBool>,
    ) -> Self {
        Self {
            limits,
            cancelled,
            steps: 0,
            pairs: 0,
            bytes: 0,
        }
    }
    pub(super) fn step(&mut self) -> Result<(), PredicateError> {
        crate::noding::check_interruption(
            self.limits.deadline,
            self.cancelled,
            PredicateError::DeadlineExceeded,
            PredicateError::Cancelled,
        )?;
        self.steps = self
            .steps
            .checked_add(1)
            .filter(|n| *n <= self.limits.max_steps)
            .ok_or(PredicateError::WorkLimit)?;
        Ok(())
    }
    fn pair(&mut self) -> Result<(), PredicateError> {
        self.step()?;
        self.pairs = self
            .pairs
            .checked_add(1)
            .filter(|n| *n <= self.limits.max_segment_pairs)
            .ok_or(PredicateError::WorkLimit)?;
        Ok(())
    }
    fn push<T>(&mut self, vector: &mut Vec<T>, value: T) -> Result<(), PredicateError> {
        self.step()?;
        if vector.len() == vector.capacity() {
            let bytes = u64::try_from(std::mem::size_of::<T>())
                .map_err(|_| PredicateError::ReservationLimit)?;
            self.bytes = self
                .bytes
                .checked_add(bytes)
                .filter(|n| *n <= self.limits.max_reserved_bytes)
                .ok_or(PredicateError::ReservationLimit)?;
            vector
                .try_reserve_exact(1)
                .map_err(|_| PredicateError::AllocationFailed)?;
        }
        vector.push(value);
        Ok(())
    }
}

fn sorted<T>(
    values: &mut [T],
    compare: impl Fn(&T, &T) -> Ordering,
    work: &mut Work<'_>,
) -> Result<(), PredicateError> {
    // Insertion sort: ogni confronto e' interrompibile, nessuna allocazione.
    for i in 1..values.len() {
        let mut j = i;
        while j > 0 {
            work.step()?;
            if compare(&values[j - 1], &values[j]) != Ordering::Greater {
                break;
            }
            values.swap(j - 1, j);
            j -= 1;
        }
    }
    Ok(())
}

fn invalid(side: &'static str) -> PredicateError {
    PredicateError::InvalidGeometry {
        side,
        reason: "struttura geometrica non valida".to_owned(),
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Q {
    n: BigInt,
    d: BigInt,
}

impl Q {
    fn new(mut n: BigInt, mut d: BigInt) -> Self {
        if d.sign() == Sign::Minus {
            n = -n;
            d = -d;
        }
        let mut a = if n.sign() == Sign::Minus {
            -&n
        } else {
            n.clone()
        };
        let mut b = d.clone();
        while b != BigInt::from(0) {
            let r = &a % &b;
            a = b;
            b = r;
        }
        Self {
            n: n / &a,
            d: d / a,
        }
    }
    fn int(n: i32) -> Self {
        Self {
            n: BigInt::from(n),
            d: BigInt::from(1),
        }
    }
    fn float(x: f64) -> Self {
        let bits = x.to_bits();
        let exponent = ((bits >> 52) & 2047) as i32;
        let fraction = bits & ((1_u64 << 52) - 1);
        let mantissa = if exponent == 0 {
            fraction
        } else {
            fraction | (1_u64 << 52)
        };
        let shift = if exponent == 0 {
            -1074
        } else {
            exponent - 1075
        };
        let mut n = BigInt::from(mantissa);
        if bits >> 63 != 0 {
            n = -n;
        }
        if shift >= 0 {
            Self::new(n << shift.unsigned_abs(), BigInt::from(1))
        } else {
            Self::new(n, BigInt::from(1) << shift.unsigned_abs())
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
    fn div(&self, b: &Self) -> Self {
        Self::new(&self.n * &b.d, &self.d * &b.n)
    }
    fn sign(&self) -> Ordering {
        self.n.cmp(&BigInt::from(0))
    }
    fn midpoint(&self, b: &Self) -> Self {
        self.add(b).div(&Self::int(2))
    }
}
impl Ord for Q {
    fn cmp(&self, b: &Self) -> Ordering {
        (&self.n * &b.d).cmp(&(&b.n * &self.d))
    }
}
impl PartialOrd for Q {
    fn partial_cmp(&self, b: &Self) -> Option<Ordering> {
        Some(self.cmp(b))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct P {
    x: Q,
    y: Q,
}
impl P {
    fn of(p: Coord<f64>) -> Self {
        Self {
            x: Q::float(p.x),
            y: Q::float(p.y),
        }
    }
    fn sub(&self, b: &Self) -> Self {
        Self {
            x: self.x.sub(&b.x),
            y: self.y.sub(&b.y),
        }
    }
    fn add(&self, b: &Self) -> Self {
        Self {
            x: self.x.add(&b.x),
            y: self.y.add(&b.y),
        }
    }
    fn cross(&self, b: &Self) -> Q {
        self.x.mul(&b.y).sub(&self.y.mul(&b.x))
    }
}

#[derive(Clone)]
struct Edge {
    a: P,
    b: P,
}
impl Edge {
    fn at_x(&self, x: &Q) -> Q {
        self.a.y.add(
            &self
                .b
                .y
                .sub(&self.a.y)
                .mul(&x.sub(&self.a.x))
                .div(&self.b.x.sub(&self.a.x)),
        )
    }
    fn contains(&self, p: &P) -> bool {
        between(&p.x, &self.a.x, &self.b.x)
            && between(&p.y, &self.a.y, &self.b.y)
            && self.b.sub(&self.a).cross(&p.sub(&self.a)).sign() == Ordering::Equal
    }
}
fn between(x: &Q, a: &Q, b: &Q) -> bool {
    (a <= x && x <= b) || (b <= x && x <= a)
}

#[derive(Default)]
struct Shape {
    points: Vec<P>,
    lines: Vec<Edge>,
    ends: Vec<P>,
    polygons: Vec<Vec<Vec<Edge>>>,
    groups: Vec<Option<usize>>,
    connected: Vec<Vec<usize>>,
}

fn ring(line: &[Coord<f64>], work: &mut Work<'_>) -> Result<Vec<Edge>, PredicateError> {
    let mut edges = Vec::new();
    for w in line.windows(2) {
        work.step()?;
        if w[0] != w[1] {
            work.push(
                &mut edges,
                Edge {
                    a: P::of(w[0]),
                    b: P::of(w[1]),
                },
            )?;
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
            for (i, a) in ring.iter().enumerate() {
                for (j, b) in ring.iter().enumerate().skip(i + 1) {
                    work.pair()?;
                    let adjacent = j == i + 1 || (i == 0 && j + 1 == ring.len());
                    let hit = segment_hit(a, b);
                    if hit == 2 || (hit == 1 && !adjacent) {
                        return Err(invalid(side));
                    }
                }
            }
        }
        Ok(())
    }
    fn validate_cell(
        &self,
        p: &P,
        dimension: u8,
        side: &'static str,
        connected: &mut [Vec<usize>],
        work: &mut Work<'_>,
    ) -> Result<(), PredicateError> {
        let mut polygon_positions = Vec::new();
        for (polygon_index, rings) in self.polygons.iter().enumerate() {
            let mut statuses = Vec::new();
            for ring in rings {
                let mut boundary = false;
                for edge in ring {
                    work.step()?;
                    if edge.contains(p) {
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
        let mut rings = Vec::new();
        for line in std::iter::once(polygon.exterior()).chain(polygon.interiors()) {
            work.step()?;
            let edges = ring(&line.0, work)?;
            work.push(&mut rings, edges)?;
        }
        self.add_polygon(rings, work)
    }
    fn add_polygon(
        &mut self,
        rings: Vec<Vec<Edge>>,
        work: &mut Work<'_>,
    ) -> Result<(), PredicateError> {
        let mut parents = Vec::new();
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
        let mut rings = Vec::new();
        work.push(&mut rings, edges)?;
        self.add_polygon(rings, work)
    }
    fn segment(
        &mut self,
        a: Coord<f64>,
        b: Coord<f64>,
        work: &mut Work<'_>,
    ) -> Result<(), PredicateError> {
        work.push(
            &mut self.lines,
            Edge {
                a: P::of(a),
                b: P::of(b),
            },
        )?;
        work.push(&mut self.ends, P::of(a))?;
        work.push(&mut self.ends, P::of(b))
    }
    fn line(&mut self, line: &LineString<f64>, work: &mut Work<'_>) -> Result<(), PredicateError> {
        work.step()?;
        for e in ring(&line.0, work)? {
            work.push(&mut self.lines, e)?;
        }
        if let (Some(a), Some(b)) = (line.0.first(), line.0.last()) {
            work.push(&mut self.ends, P::of(*a))?;
            work.push(&mut self.ends, P::of(*b))?;
        }
        Ok(())
    }
    fn geometry(
        &mut self,
        geometry: &Geometry<f64>,
        work: &mut Work<'_>,
    ) -> Result<(), PredicateError> {
        let mut pending = Vec::new();
        work.push(&mut pending, geometry)?;
        while let Some(geometry) = pending.pop() {
            work.step()?;
            match geometry {
                Geometry::Point(p) => work.push(&mut self.points, P::of(p.0))?,
                Geometry::Line(l) => self.segment(l.start, l.end, work)?,
                Geometry::LineString(l) => self.line(l, work)?,
                Geometry::Polygon(p) => self.polygon(p, work)?,
                Geometry::MultiPoint(ps) => {
                    for p in &ps.0 {
                        work.push(&mut self.points, P::of(p.0))?;
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
                        work.push(&mut self.points, P::of(r.min()))?;
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
            if e.contains(p) {
                boundary = true;
                break;
            }
        }
        if boundary {
            // Tutte le direzioni incidenti partizionano il giro locale.
            // Gli assi garantiscono settori di ampiezza minore di pi greco:
            // la somma dei raggi adiacenti e' strettamente nel settore.
            let mut rays = Vec::new();
            for (x, y) in [(1, 0), (0, 1), (-1, 0), (0, -1)] {
                work.push(
                    &mut rays,
                    P {
                        x: Q::int(x),
                        y: Q::int(y),
                    },
                )?;
            }
            for edge in self.polygons.iter().flatten().flatten() {
                work.step()?;
                if !edge.contains(p) {
                    continue;
                }
                for endpoint in [&edge.a, &edge.b] {
                    if endpoint != p {
                        work.push(&mut rays, endpoint.sub(p))?;
                    }
                }
            }
            sorted(&mut rays, angle_cmp, work)?;
            rays.dedup_by(|a, b| angle_cmp(a, b) == Ordering::Equal);
            for (a, b) in rays.iter().zip(rays.iter().cycle().skip(1)) {
                if !self.area_contains(p, Some(&a.add(b)), work)? {
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
            if e.contains(p) {
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
fn angle_cmp(a: &P, b: &P) -> Ordering {
    let half = |p: &P| {
        p.y.sign() == Ordering::Less
            || (p.y.sign() == Ordering::Equal && p.x.sign() == Ordering::Less)
    };
    half(a)
        .cmp(&half(b))
        .then_with(|| a.cross(b).sign().reverse())
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
        x: Q::int(0),
        y: Q::int(0),
    };
    let d = direction.unwrap_or(&zero);
    let mut inside = false;
    for e in ring {
        work.step()?;
        let above = |y: &Q| perturbed_sign(&p.y.sub(y), &d.y) == Ordering::Less;
        if above(&e.a.y) != above(&e.b.y) {
            let edge = e.b.sub(&e.a);
            let side = perturbed_sign(&edge.cross(&p.sub(&e.a)), &edge.cross(d));
            if side == e.b.y.cmp(&e.a.y) {
                inside = !inside;
            }
        }
    }
    Ok(inside)
}

fn intersection_x(a: &Edge, b: &Edge) -> Option<Q> {
    let r = a.b.sub(&a.a);
    let s = b.b.sub(&b.a);
    let denominator = r.cross(&s);
    if denominator.sign() == Ordering::Equal {
        return None;
    }
    let offset = b.a.sub(&a.a);
    let t = offset.cross(&s).div(&denominator);
    let u = offset.cross(&r).div(&denominator);
    if between(&t, &Q::int(0), &Q::int(1)) && between(&u, &Q::int(0), &Q::int(1)) {
        Some(a.a.x.add(&r.x.mul(&t)))
    } else {
        None
    }
}

fn segment_hit(a: &Edge, b: &Edge) -> u8 {
    if a.b.sub(&a.a).cross(&b.b.sub(&b.a)).sign() != Ordering::Equal {
        return u8::from(intersection_x(a, b).is_some());
    }
    let mut first = None;
    for p in [&a.a, &a.b, &b.a, &b.b] {
        if a.contains(p) && b.contains(p) {
            if first.is_some_and(|q| q != p) {
                return 2;
            }
            first = Some(p);
        }
    }
    u8::from(first.is_some())
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
    let mut pending = Vec::new();
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
                if P::of(t.v2())
                    .sub(&P::of(t.v1()))
                    .cross(&P::of(t.v3()).sub(&P::of(t.v1())))
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

fn samples(mut values: Vec<Q>, work: &mut Work<'_>) -> Result<Vec<(Q, bool)>, PredicateError> {
    sorted(&mut values, Ord::cmp, work)?;
    values.dedup();
    let mut result = Vec::new();
    if let Some(first) = values.first() {
        work.push(&mut result, (first.sub(&Q::int(1)), false))?;
    }
    for (i, value) in values.iter().enumerate() {
        work.push(&mut result, (value.clone(), true))?;
        if let Some(next) = values.get(i + 1) {
            work.push(&mut result, (value.midpoint(next), false))?;
        }
    }
    if let Some(last) = values.last() {
        work.push(&mut result, (last.add(&Q::int(1)), false))?;
    }
    if values.is_empty() {
        work.push(&mut result, (Q::int(0), false))?;
    }
    Ok(result)
}

pub(super) fn relate(
    left: &Geometry<f64>,
    right: &Geometry<f64>,
    work: &mut Work<'_>,
) -> Result<De9imMatrix, PredicateError> {
    relate_impl(left, right, work, true)
}

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
    if check_input {
        a.validate_rings("left", work)?;
        b.validate_rings("right", work)?;
    }
    let mut a_connected = std::mem::take(&mut a.connected);
    let mut b_connected = std::mem::take(&mut b.connected);
    let mut edges = Vec::new();
    for e in a.edges().chain(b.edges()) {
        work.push(&mut edges, e)?;
    }
    let mut points = Vec::new();
    for p in a.points.iter().chain(&b.points) {
        work.push(&mut points, p)?;
    }
    let mut xs = Vec::new();
    for x in edges
        .iter()
        .flat_map(|e| [&e.a.x, &e.b.x])
        .chain(points.iter().map(|p| &p.x))
    {
        work.push(&mut xs, x.clone())?;
    }
    for (i, edge) in edges.iter().enumerate() {
        for other in edges.iter().skip(i + 1) {
            work.pair()?;
            if let Some(x) = intersection_x(edge, other) {
                work.push(&mut xs, x)?;
            }
        }
    }
    let mut cells = [b'F'; 9];
    cells[8] = b'2';
    for (x, critical_x) in samples(xs, work)? {
        work.step()?;
        let mut ys = Vec::new();
        for e in &edges {
            work.step()?;
            if between(&x, &e.a.x, &e.b.x) {
                if e.a.x == e.b.x {
                    work.push(&mut ys, e.a.y.clone())?;
                    work.push(&mut ys, e.b.y.clone())?;
                } else {
                    work.push(&mut ys, e.at_x(&x))?;
                }
            }
        }
        for p in &points {
            work.step()?;
            if p.x == x {
                work.push(&mut ys, p.y.clone())?;
            }
        }
        for (y, critical_y) in samples(ys, work)? {
            work.step()?;
            let p = P { x: x.clone(), y };
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
    use geo::line_string;
    use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
    use wkt::TryFromWkt as _;

    fn geometry(text: &str) -> Geometry<f64> {
        Geometry::try_from_wkt_str(text).expect("WKT costruito nella prova")
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
            &super::super::relate_matrix(&left, &right).expect("matrice razionale"),
        );
        assert_eq!(
            super::super::relate_matrix(&left, &right)
                .expect("matrice")
                .to_string(),
            "0F1FF0102"
        );
        // La stessa costruzione a esponenti estremi; nessun overflow,
        // sottoflusso o quantizzazione del punto di intersezione.
        for scale in [f64::from_bits(1), f64::MAX] {
            let a = Geometry::Line(geo::Line::new((0.0, 0.0), (scale, scale)));
            let b = Geometry::Line(geo::Line::new((0.0, scale), (scale, -scale)));
            assert_named(
                &a,
                &b,
                &super::super::relate_matrix(&a, &b).expect("matrice estrema"),
            );
            assert_eq!(
                super::super::relate_matrix(&a, &b)
                    .expect("matrice estrema")
                    .to_string(),
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
            max_reserved_bytes: work.bytes,
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
            max_reserved_bytes: work.bytes - 1,
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
                super::super::relate_matrix(&Geometry::Polygon(polygon), &point),
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
                    super::super::relate_matrix(&value, &point),
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
        use super::super::{
            exact_predicate, exact_predicate_validated, relate_matrix, SpatialPredicate as S,
        };
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
                super::super::relate_matrix(&a, &other).expect("prima"),
                super::super::relate_matrix(&b, &other).expect("permutata")
            );
        }
    }
}

/// Fonte GOP-028: anche l'oracolo differenziale osserva deadline prima del token.
#[test]
fn oracle_deadline_precedence_at_entry_and_during_work() {
    use std::sync::atomic::{AtomicBool, Ordering};
    for steps in [0, 5] {
        let flag = AtomicBool::new(false);
        let mut work = Work::new(RelateLimits::default(), Some(&flag));
        for _ in 0..steps {
            work.step().unwrap();
        }
        assert_eq!(work.steps, steps);
        work.limits.deadline = Some(std::time::Instant::now());
        flag.store(true, Ordering::Relaxed);
        assert!(matches!(work.step(), Err(PredicateError::DeadlineExceeded)));
        assert_eq!(work.steps, steps);
    }
}
