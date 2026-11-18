//! Booleane binarie planari XY, GOP-014/015/015A/027/028.
//!
//! Polygon e `MultiPolygon` validi; le parti di ciascun `MultiPolygon` devono
//! essere disgiunte anche sul confine. Nessuna normalizzazione implicita.
//! Classificazione esatta dei due settori di ciascun lato del noder comune.
//! Gusci antiorari, buchi orari; ogni anello parte dal minimo XY pubblicato.
//! Gusci e buchi ordinati per minimo e vertice successivo; lati collineari nodati conservati.
//! Contatti isolati: segmenti atomici e punti ordinati XY, dimensioni crescenti.
//! Vertici riportati invariati; nodi costruiti nearest-even, <= 1/2 ulp per
//! ordinata nelle unita' delle coordinate, con verifica OGC dopo pubblicazione.

use super::line_boolean::{
    self, coordinate_cmp, edge_cmp, ordered_ids, point_cmp, sort_output, Originals, PublishedEdge,
};
use super::{angle_cmp, ring_contains, Shape};
use crate::noding::{CountedVec, Edge, Work, P};
use crate::predicates::interval_index::IntervalIndex;
use crate::predicates::{PredicateError, RelateLimits};
use geo::{Geometry, LineString, MultiLineString, MultiPoint, MultiPolygon, Point, Polygon};
use std::cmp::Ordering;
use std::sync::atomic::AtomicBool;

/// Operatori insiemistici binari; differenza con chiusura del residuo.
pub use super::line_boolean::LineBooleanOp as PolygonBooleanOp;

/// Limiti cumulativi e contabili; nessuna stima del prodotto degli ingressi.
#[derive(Clone, Copy, Debug)]
pub struct PolygonBooleanLimits {
    /// Lavoro, coppie, nodi e memoria viva, compresa l'aritmetica GOP-027.
    /// Massimo per valore ed esclusioni sono quelli di `RelateLimits`.
    pub work: RelateLimits,
    /// Incidenze di lati atomici prima della deduplicazione.
    pub max_edges: u64,
    /// Cicli di frontiera visitati, inclusi i cicli esterni dei buchi.
    pub max_faces: u64,
    /// Anelli semplici costruiti, inclusi quelli separati a un contatto.
    pub max_rings: u64,
}
impl Default for PolygonBooleanLimits {
    fn default() -> Self {
        Self {
            work: RelateLimits::default(),
            max_edges: 10_000_000,
            max_faces: 1_000_000,
            max_rings: 1_000_000,
        }
    }
}

/// Errori tipizzati senza valori degli operandi; nessun risultato parziale.
#[derive(Debug, thiserror::Error)]
pub enum PolygonBooleanError {
    /// Tipi o parti a contatto che richiedono normalizzazione.
    #[error("operandi non supportati dal profilo booleano poligonale")]
    UnsupportedOperands,
    /// Il filtro ammette solo 0, 1 e 2.
    #[error("dimensione di uscita non valida")]
    InvalidOutputDimension,
    /// Il rounding non conserva il certificato topologico.
    #[error("topologia non conservata dalla pubblicazione binary64")]
    InvalidRoundedTopology,
    /// Il nodo costruito non ha una rappresentazione finita.
    #[error("coordinata costruita fuori dal dominio binary64 finito")]
    CoordinateOutOfRange,
    /// Validazione, risorse, cancellazione e deadline.
    #[error(transparent)]
    Predicate(#[from] PredicateError),
}
impl From<line_boolean::LineBooleanError> for PolygonBooleanError {
    fn from(error: line_boolean::LineBooleanError) -> Self {
        match error {
            line_boolean::LineBooleanError::UnsupportedOperands => Self::UnsupportedOperands,
            line_boolean::LineBooleanError::InvalidOutputDimension => Self::InvalidOutputDimension,
            line_boolean::LineBooleanError::InvalidRoundedTopology => Self::InvalidRoundedTopology,
            line_boolean::LineBooleanError::CoordinateOutOfRange => Self::CoordinateOutOfRange,
            line_boolean::LineBooleanError::Predicate(error) => Self::Predicate(error),
        }
    }
}

const fn selected(op: PolygonBooleanOp, a: bool, b: bool) -> bool {
    match op {
        PolygonBooleanOp::Union => a || b,
        PolygonBooleanOp::Intersection => a && b,
        PolygonBooleanOp::Difference => a && !b,
    }
}
fn charge(count: &mut u64, limit: u64, work: &mut Work<'_>) -> Result<(), PredicateError> {
    work.step()?;
    *count = count
        .checked_add(1)
        .filter(|n| *n <= limit)
        .ok_or(PredicateError::WorkLimit)?;
    Ok(())
}
fn midpoint(edge: &Edge) -> Result<P, PredicateError> {
    Ok(P {
        x: edge.a.x.try_midpoint(&edge.b.x)?,
        y: edge.a.y.try_midpoint(&edge.b.y)?,
    })
}
fn reverse(edge: &Edge) -> Result<Edge, PredicateError> {
    Ok(Edge {
        a: edge.b.try_clone()?,
        b: edge.a.try_clone()?,
    })
}

/// Stesso CRS e ordine degli assi a carico del chiamante, senza riproiezione.
///
/// I vuoti e il filtro dimensionale seguono GOP-014/015; il filtro non estrae
/// confini. Ogni fase condivide il budget e controlla deadline prima del token.
/// Ogni iterazione visita al massimo un lato, nodo o candidato fra checkpoint;
/// le primitive razionali hanno il massimo finito di `RelateLimits::MAX_VALUE_BYTES`.
/// Nessun limite RSS: input preso in prestito, overhead e scratch privati esclusi.
///
/// # Errors
/// Tipi esclusi, input invalido, filtro errato, pubblicazione non certificabile,
/// esaurimento di risorse, allocazione fallita, cancellazione o deadline.
pub fn boolean_with_limits(
    left: &Geometry<f64>,
    right: &Geometry<f64>,
    operation: PolygonBooleanOp,
    output_dimension: Option<u8>,
    limits: PolygonBooleanLimits,
    cancelled: Option<&AtomicBool>,
) -> Result<Geometry<f64>, PolygonBooleanError> {
    execute(
        left,
        right,
        operation,
        output_dimension,
        limits,
        &mut Work::with_deadline_precedence(limits.work, cancelled),
    )
}

fn supported(g: &Geometry<f64>, work: &mut Work<'_>) -> Result<(), PolygonBooleanError> {
    work.step()?;
    match g {
        Geometry::Polygon(_) => Ok(()),
        Geometry::MultiPolygon(polygons) => {
            for (i, a) in polygons.0.iter().enumerate() {
                for b in polygons.0.iter().skip(i + 1) {
                    work.step()?;
                    // Shape consente la validazione senza clonare le geometrie.
                    let mut shape = Shape::default();
                    shape.polygon(a, work)?;
                    shape.polygon(b, work)?;
                    let mut refs = CountedVec::new();
                    for edge in shape.edges() {
                        work.push(&mut refs, edge)?;
                    }
                    let mut split = 0;
                    if let Some(rings) = shape.polygons.first() {
                        for ring in rings {
                            work.step()?;
                            split += ring.len();
                        }
                    }
                    let mut touches = false;
                    crate::noding::visit(&refs, work, |i, j, hits, _| {
                        if (i < split) != (j < split) && hits[0].is_some() {
                            touches = true;
                        }
                        Ok(())
                    })?;
                    if touches {
                        return Err(PolygonBooleanError::UnsupportedOperands);
                    }
                }
            }
            Ok(())
        }
        _ => Err(PolygonBooleanError::UnsupportedOperands),
    }
}

struct Boundary {
    edges: CountedVec<Edge>,
    contacts: CountedVec<Edge>,
    sites: CountedVec<P>,
}
// Un punto isolato non occupa un intervallo aperto, neppure se coincide
// con il campione scelto per classificarlo (chiusura GOP-015).
fn occupies_interval(shape: &Shape, p: &P, work: &mut Work<'_>) -> Result<bool, PredicateError> {
    if shape.area_contains(p, None, work)? {
        return Ok(true);
    }
    for edge in shape.edges() {
        work.step()?;
        if edge.contains(p)? {
            return Ok(true);
        }
    }
    Ok(false)
}
fn boundary(
    a: &Shape,
    b: &Shape,
    op: PolygonBooleanOp,
    limits: &mut PolygonBooleanLimits,
    work: &mut Work<'_>,
) -> Result<Boundary, PredicateError> {
    let mut source = CountedVec::new();
    for edge in a.edges().chain(b.edges()) {
        work.push(&mut source, edge)?;
    }
    let nodes = crate::noding::node(&source, work)?;
    let mut atomic = CountedVec::new();
    let mut count = 0;
    for path in &nodes {
        for pair in path.windows(2) {
            charge(&mut count, limits.max_edges, work)?;
            work.nodes(2)?;
            let (a, b) = if point_cmp(&pair[0], &pair[1])?.is_gt() {
                (&pair[1], &pair[0])
            } else {
                (&pair[0], &pair[1])
            };
            work.push(
                &mut atomic,
                Edge {
                    a: a.try_clone()?,
                    b: b.try_clone()?,
                },
            )?;
        }
    }
    let ids = ordered_ids(&atomic, edge_cmp, work)?;
    let mut edges = CountedVec::new();
    let mut contacts = CountedVec::new();
    for id in ids {
        work.step()?;
        let edge = &atomic[id];
        let p = midpoint(edge)?;
        let delta = edge.b.try_sub(&edge.a)?;
        let normal = P {
            x: work.integer(0)?.try_sub(&delta.y)?,
            y: delta.x,
        };
        let opposite = P {
            x: work.integer(0)?.try_sub(&normal.x)?,
            y: work.integer(0)?.try_sub(&normal.y)?,
        };
        let l = selected(
            op,
            a.area_contains(&p, Some(&normal), work)?,
            b.area_contains(&p, Some(&normal), work)?,
        );
        let r = selected(
            op,
            a.area_contains(&p, Some(&opposite), work)?,
            b.area_contains(&p, Some(&opposite), work)?,
        );
        if l != r {
            work.nodes(2)?;
            let value = if l {
                Edge {
                    a: edge.a.try_clone()?,
                    b: edge.b.try_clone()?,
                }
            } else {
                reverse(edge)?
            };
            work.push(&mut edges, value)?;
        } else if !l
            && selected(
                op,
                occupies_interval(a, &p, work)?,
                occupies_interval(b, &p, work)?,
            )
        {
            work.nodes(2)?;
            work.push(
                &mut contacts,
                Edge {
                    a: edge.a.try_clone()?,
                    b: edge.b.try_clone()?,
                },
            )?;
        }
    }
    let mut sites = CountedVec::new();
    limits.max_edges -= count;
    for path in nodes {
        for p in path {
            work.push(&mut sites, p)?;
        }
    }
    Ok(Boundary {
        edges,
        contacts,
        sites,
    })
}

struct Cycle {
    shell: bool,
    index: IntervalIndex,
    edges: CountedVec<Edge>,
}

// Il successore e' il primo raggio in senso orario dal raggio entrante
// invertito. I lati hanno sempre l'area selezionata alla propria sinistra.
fn successor(current: usize, edges: &[Edge], work: &mut Work<'_>) -> Result<usize, PredicateError> {
    let at = &edges[current].b;
    let incoming = edges[current].a.try_sub(at)?;
    let mut before: Option<(usize, P)> = None;
    let mut last: Option<(usize, P)> = None;
    for (id, edge) in edges.iter().enumerate() {
        work.step()?;
        if &edge.a != at {
            continue;
        }
        let ray = edge.b.try_sub(at)?;
        if angle_cmp(&ray, &incoming)?.is_lt() {
            if let Some((_, previous)) = &before {
                if angle_cmp(previous, &ray)?.is_lt() {
                    before = Some((id, ray.try_clone()?));
                }
            } else {
                before = Some((id, ray.try_clone()?));
            }
        }
        if let Some((_, previous)) = &last {
            if angle_cmp(previous, &ray)?.is_lt() {
                last = Some((id, ray));
            }
        } else {
            last = Some((id, ray));
        }
    }
    before
        .or(last)
        .map(|v| v.0)
        .ok_or(PredicateError::CalcoloNonConcluso(
            "frontiera poligonale aperta",
        ))
}
fn cycle(ids: &[usize], edges: &[Edge], work: &mut Work<'_>) -> Result<Cycle, PredicateError> {
    if ids.len() < 3 {
        return Err(PredicateError::CalcoloNonConcluso("anello degenere"));
    }
    let mut minimum = 0;
    let mut ring = CountedVec::new();
    for (i, &id) in ids.iter().enumerate() {
        work.step()?;
        if point_cmp(&edges[id].a, &edges[ids[minimum]].a)?.is_lt() {
            minimum = i;
        }
        work.nodes(2)?;
        work.push(
            &mut ring,
            Edge {
                a: edges[id].a.try_clone()?,
                b: edges[id].b.try_clone()?,
            },
        )?;
    }
    let p = &edges[ids[minimum]].a;
    let prev = &edges[ids[(minimum + ids.len() - 1) % ids.len()]].a;
    let next = &edges[ids[minimum]].b;
    let sign = p.try_sub(prev)?.try_cross(&next.try_sub(p)?)?.sign();
    if sign.is_eq() {
        return Err(PredicateError::CalcoloNonConcluso("orientamento anello"));
    }
    let index = IntervalIndex::new(ring.iter().map(|e| (&e.a.y, &e.b.y)), work)?;
    Ok(Cycle {
        shell: sign.is_gt(),
        index,
        edges: ring,
    })
}
fn cycles(
    edges: &[Edge],
    limits: &mut PolygonBooleanLimits,
    work: &mut Work<'_>,
) -> Result<CountedVec<Cycle>, PredicateError> {
    let mut points = CountedVec::new();
    for edge in edges {
        work.push(&mut points, &edge.a)?;
    }
    let ids = ordered_ids(&points, |a, b| point_cmp(a, b), work)?;
    let vertex = |p: &P, work: &mut Work<'_>| -> Result<usize, PredicateError> {
        let (mut low, mut high) = (0, ids.len());
        while low < high {
            work.step()?;
            let mid = low + (high - low) / 2;
            match point_cmp(points[ids[mid]], p)? {
                Ordering::Less => low = mid + 1,
                Ordering::Greater => high = mid,
                Ordering::Equal => return Ok(mid),
            }
        }
        Err(PredicateError::CalcoloNonConcluso("vertice di frontiera"))
    };
    let mut arcs = CountedVec::new();
    for (id, edge) in edges.iter().enumerate() {
        let arc = crate::noding::faces::ArcSide {
            from: vertex(&edge.a, work)?,
            to: vertex(&edge.b, work)?,
            next: successor(id, edges, work)?,
            seen: false,
            atom: id,
        };
        work.push(&mut arcs, arc)?;
    }
    let mut result = CountedVec::new();
    let (mut faces, mut rings) = (0, 0);
    crate::noding::faces::walk(
        &mut arcs,
        ids.len(),
        work,
        |work| charge(&mut faces, limits.max_faces, work),
        |path, _, work| {
            charge(&mut rings, limits.max_rings, work)?;
            let value = cycle(path, edges, work)?;
            work.push(&mut result, value)
        },
    )?;
    limits.max_faces -= faces;
    limits.max_rings -= rings;
    Ok(result)
}

fn contains(cycle: &Cycle, p: &P, work: &mut Work<'_>) -> Result<bool, PredicateError> {
    ring_contains(&cycle.edges, &cycle.index, p, None, work)
}
fn publish_ring(
    cycle: &Cycle,
    originals: &Originals,
    work: &mut Work<'_>,
) -> Result<LineString<f64>, PolygonBooleanError> {
    let mut coords = CountedVec::new();
    let mut minimum = 0;
    for edge in &cycle.edges {
        work.nodes(1)?;
        let c = originals.coordinate(&edge.a, work)?;
        if !coords.is_empty() && coordinate_cmp(c, coords[minimum]).is_lt() {
            minimum = coords.len();
        }
        work.push(&mut coords, c)?;
    }
    let mut rotated = CountedVec::new();
    for i in 0..=coords.len() {
        work.nodes(1)?;
        work.push(&mut rotated, coords[(minimum + i) % coords.len()])?;
    }
    Ok(LineString::new(rotated.export(work)?))
}
fn assemble(
    rings: &[Cycle],
    originals: &Originals,
    work: &mut Work<'_>,
) -> Result<Geometry<f64>, PolygonBooleanError> {
    let mut owners = CountedVec::new();
    for hole in rings {
        work.step()?;
        let mut owner: Option<usize> = None;
        if !hole.shell {
            let p = midpoint(&hole.edges[0])?;
            for (id, shell) in rings.iter().enumerate() {
                work.step()?;
                if !shell.shell || !contains(shell, &p, work)? {
                    continue;
                }
                if let Some(previous) = owner {
                    if contains(&rings[previous], &midpoint(&shell.edges[0])?, work)? {
                        owner = Some(id);
                    }
                } else {
                    owner = Some(id);
                }
            }
            if owner.is_none() {
                return Err(PredicateError::CalcoloNonConcluso("buco senza guscio").into());
            }
        }
        work.push(&mut owners, owner)?;
    }
    let mut polygons = CountedVec::new();
    for (id, shell) in rings.iter().enumerate() {
        work.step()?;
        if !shell.shell {
            continue;
        }
        let exterior = publish_ring(shell, originals, work)?;
        let mut holes = CountedVec::new();
        for (hole, owner) in rings.iter().zip(&owners) {
            work.step()?;
            if *owner == Some(id) {
                let ring = publish_ring(hole, originals, work)?;
                work.push(&mut holes, ring)?;
            }
        }
        sort_output(
            &mut holes,
            |a, b| coordinate_cmp(a.0[0], b.0[0]).then_with(|| coordinate_cmp(a.0[1], b.0[1])),
            work,
        )?;
        let polygon = Polygon::new(exterior, holes.export(work)?);
        work.push(&mut polygons, polygon)?;
    }
    sort_output(
        &mut polygons,
        |a, b| {
            coordinate_cmp(a.exterior().0[0], b.exterior().0[0])
                .then_with(|| coordinate_cmp(a.exterior().0[1], b.exterior().0[1]))
        },
        work,
    )?;
    Ok(Geometry::MultiPolygon(MultiPolygon::new(
        polygons.export(work)?,
    )))
}

#[allow(clippy::too_many_lines)]
fn execute(
    left: &Geometry<f64>,
    right: &Geometry<f64>,
    op: PolygonBooleanOp,
    filter: Option<u8>,
    limits: PolygonBooleanLimits,
    work: &mut Work<'_>,
) -> Result<Geometry<f64>, PolygonBooleanError> {
    work.step()?;
    if filter.is_some_and(|d| d > 2) {
        return Err(PolygonBooleanError::InvalidOutputDimension);
    }
    if !matches!(left, Geometry::Polygon(_) | Geometry::MultiPolygon(_))
        || !matches!(right, Geometry::Polygon(_) | Geometry::MultiPolygon(_))
    {
        return Err(PolygonBooleanError::UnsupportedOperands);
    }
    super::relate(left, right, work)?;
    supported(left, work)?;
    supported(right, work)?;
    let mut a = Shape::default();
    let mut b = Shape::default();
    a.geometry(left, work)?;
    b.geometry(right, work)?;
    a.build_indices(work)?;
    b.build_indices(work)?;
    let originals = Originals::new(left, right, work)?;
    assemble_result(&a, &b, op, filter, &mut { limits }, &originals, work)
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
fn assemble_result(
    a: &Shape,
    b: &Shape,
    op: PolygonBooleanOp,
    filter: Option<u8>,
    limits: &mut PolygonBooleanLimits,
    originals: &Originals,
    work: &mut Work<'_>,
) -> Result<Geometry<f64>, PolygonBooleanError> {
    let boundary = boundary(a, b, op, limits, work)?;
    let rings = cycles(&boundary.edges, limits, work)?;
    let polygons = if filter.is_none_or(|d| d == 2) {
        assemble(&rings, originals, work)?
    } else {
        Geometry::MultiPolygon(MultiPolygon::new(Vec::new()))
    };
    let mut publications = CountedVec::new();
    for (id, edge) in boundary.edges.iter().chain(&boundary.contacts).enumerate() {
        work.step()?;
        let surface = id < boundary.edges.len();
        if filter.is_some_and(|d| d != if surface { 2 } else { 1 }) {
            continue;
        }
        let coords = [
            originals.coordinate(&edge.a, work)?,
            originals.coordinate(&edge.b, work)?,
        ];
        let mut value = PublishedEdge::new(edge, coords, work)?;
        value.surface = surface;
        work.push(&mut publications, value)?;
    }
    let mut candidates = CountedVec::new();
    for p in boundary.sites.iter().chain(&a.points).chain(&b.points) {
        work.push(&mut candidates, p)?;
    }
    // Tutti i siti nodati sono candidati, compresi gli incroci fra linee.
    let ids = ordered_ids(&candidates, |a, b| point_cmp(a, b), work)?;
    let mut points = CountedVec::new();
    for id in ids {
        work.step()?;
        let p = candidates[id];
        if !selected(op, a.position(p, work)? != 2, b.position(p, work)? != 2) {
            continue;
        }
        let mut covered = false;
        for edge in boundary.edges.iter().chain(&boundary.contacts) {
            work.step()?;
            if edge.contains(p)? {
                covered = true;
                break;
            }
        }
        if !covered && filter.is_none_or(|d| d == 0) {
            // Fuori dalla frontiera selezionata basta l'appartenenza all'area.
            if selected(
                op,
                a.area_contains(p, None, work)?,
                b.area_contains(p, None, work)?,
            ) {
                continue;
            }
            let c = originals.coordinate(p, work)?;
            let value = PublishedEdge::point(p, c, work)?;
            work.push(&mut publications, value)?;
            work.push(&mut points, Point(c))?;
        }
    }
    line_boolean::verify_publication(&publications, originals, false, Some(&polygons), work)?;
    let mut components = CountedVec::new();
    if filter.is_none_or(|d| d == 0) && !points.is_empty() {
        sort_output(&mut points, |a, b| coordinate_cmp(a.0, b.0), work)?;
        let value = Geometry::MultiPoint(MultiPoint::new(points.export(work)?));
        work.push(&mut components, value)?;
    }
    if filter.is_none_or(|d| d == 1) {
        let mut lines = CountedVec::new();
        for edge in &boundary.contacts {
            let mut coords = CountedVec::new();
            for p in [&edge.a, &edge.b] {
                work.nodes(1)?;
                let c = originals.coordinate(p, work)?;
                work.push(&mut coords, c)?;
            }
            if coordinate_cmp(coords[0], coords[1]).is_gt() {
                coords.swap(0, 1);
            }
            let line = LineString::new(coords.export(work)?);
            work.push(&mut lines, line)?;
        }
        if !lines.is_empty() {
            sort_output(
                &mut lines,
                |a, b| coordinate_cmp(a.0[0], b.0[0]).then_with(|| coordinate_cmp(a.0[1], b.0[1])),
                work,
            )?;
            let value = Geometry::MultiLineString(MultiLineString::new(lines.export(work)?));
            work.push(&mut components, value)?;
        }
    }
    if filter.is_none_or(|d| d == 2) {
        if let Geometry::MultiPolygon(ref ps) = polygons {
            if !ps.0.is_empty() {
                work.push(&mut components, polygons)?;
            }
        }
    }
    Ok(line_boolean::finish(components, filter, work)?)
}

pub(super) fn execute_many(
    left: &[Geometry<f64>],
    right: &[Geometry<f64>],
    op: PolygonBooleanOp,
    filter: Option<u8>,
    limits: &mut PolygonBooleanLimits,
    work: &mut Work<'_>,
) -> Result<Geometry<f64>, PolygonBooleanError> {
    work.step()?;
    if filter.is_some_and(|d| d > 2) {
        return Err(PolygonBooleanError::InvalidOutputDimension);
    }
    let empty = Geometry::MultiPoint(MultiPoint::new(Vec::new()));
    let mut a = Shape::default();
    let mut b = Shape::default();
    for (geometries, shape) in [(left, &mut a), (right, &mut b)] {
        for geometry in geometries {
            work.step()?;
            general_supported(geometry, work)?;
            super::relate(geometry, &empty, work)?;
            shape.geometry(geometry, work)?;
        }
        shape.build_indices(work)?;
    }
    let originals = Originals::many(left.iter().chain(right), work)?;
    assemble_result(&a, &b, op, filter, limits, &originals, work)
}

pub(super) fn general_supported(
    g: &Geometry<f64>,
    work: &mut Work<'_>,
) -> Result<(), PolygonBooleanError> {
    let mut pending = CountedVec::new();
    work.push(&mut pending, g)?;
    while let Some(g) = pending.pop() {
        work.step()?;
        match g {
            Geometry::Point(_)
            | Geometry::MultiPoint(_)
            | Geometry::LineString(_)
            | Geometry::MultiLineString(_)
            | Geometry::Polygon(_)
            | Geometry::MultiPolygon(_) => {}
            Geometry::GeometryCollection(gs) => {
                for g in &gs.0 {
                    work.push(&mut pending, g)?;
                }
            }
            _ => return Err(PolygonBooleanError::UnsupportedOperands),
        }
    }
    Ok(())
}

/// Unione binaria; contratto e limiti di [`boolean_with_limits`].
/// # Errors
/// Gli stessi errori di [`boolean_with_limits`].
pub fn union_with_limits(
    left: &Geometry<f64>,
    right: &Geometry<f64>,
    limits: PolygonBooleanLimits,
    cancelled: Option<&AtomicBool>,
) -> Result<Geometry<f64>, PolygonBooleanError> {
    boolean_with_limits(
        left,
        right,
        PolygonBooleanOp::Union,
        None,
        limits,
        cancelled,
    )
}
/// Intersezione binaria, inclusi contatti di dimensione inferiore.
/// # Errors
/// Gli stessi errori di [`boolean_with_limits`].
pub fn intersection_with_limits(
    left: &Geometry<f64>,
    right: &Geometry<f64>,
    limits: PolygonBooleanLimits,
    cancelled: Option<&AtomicBool>,
) -> Result<Geometry<f64>, PolygonBooleanError> {
    boolean_with_limits(
        left,
        right,
        PolygonBooleanOp::Intersection,
        None,
        limits,
        cancelled,
    )
}
/// Differenza direzionale con chiusura del residuo.
/// # Errors
/// Gli stessi errori di [`boolean_with_limits`].
pub fn difference_with_limits(
    left: &Geometry<f64>,
    right: &Geometry<f64>,
    limits: PolygonBooleanLimits,
    cancelled: Option<&AtomicBool>,
) -> Result<Geometry<f64>, PolygonBooleanError> {
    boolean_with_limits(
        left,
        right,
        PolygonBooleanOp::Difference,
        None,
        limits,
        cancelled,
    )
}

#[cfg(test)]
#[deny(clippy::disallowed_methods)]
mod tests;
