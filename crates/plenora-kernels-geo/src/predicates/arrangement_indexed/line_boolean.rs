//! Booleane binarie XY per punti/linee e linea/poligono, GOP-014/015.
//!
//! Noding e classificazione razionali esatti, senza snap. I nodi pubblicati
//! seguono GOP-015A: binary64 piu' vicino, meta' al pari, errore <= 1/2 ulp
//! per ordinata. Verifica topologica dopo arrotondamento, senza tolleranze.
//! Le parti sono segmenti atomici, orientati e ordinati lessicograficamente
//! XY; i nodi costruiti collineari senza incidenze sono eliminati.
//! Vertici originali invariati.
//! Per siti coincidenti con zeri di segno diverso prevale il primo in ingresso.
//! Profilo binario: Point, `MultiPoint`, `LineString`, `MultiLineString`, Polygon;
//! almeno una linea oppure entrambi puntuali. Nessuna booleana area/area.

use super::{validate_structure, Shape};
use crate::noding::{CountedVec, Edge, Work, P, Q};
use crate::predicates::interval_index::{sort_ids, IntervalIndex};
use crate::predicates::{PredicateError, RelateLimits};
use geo::{
    Coord, CoordsIter, Geometry, GeometryCollection, LineString, MultiLineString, MultiPoint,
    MultiPolygon, Point,
};
use std::cmp::Ordering;
use std::sync::atomic::AtomicBool;

/// Operatore insiemistico con chiusura dei residui della differenza.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LineBooleanOp {
    /// Unione con eliminazione delle componenti coperte.
    Union,
    /// Intersezione, compresi contatti puntuali isolati.
    Intersection,
    /// Differenza direzionale left meno right.
    Difference,
}

/// Errori senza valori degli operandi; nessuna uscita parziale.
#[derive(Debug, thiserror::Error)]
pub enum LineBooleanError {
    /// Operandi esterni al profilo binario dichiarato.
    #[error("operandi non supportati dal profilo booleano lineare")]
    UnsupportedOperands,
    /// Sono ammesse soltanto le dimensioni 0, 1, 2.
    #[error("dimensione di uscita non valida")]
    InvalidOutputDimension,
    /// La pubblicazione altererebbe incidenze, orientamento o validita' del grafo.
    #[error("topologia non conservata dalla pubblicazione binary64")]
    InvalidRoundedTopology,
    /// Coordinata costruita fuori dal dominio finito pubblicabile.
    #[error("coordinata costruita fuori dal dominio binary64 finito")]
    CoordinateOutOfRange,
    /// Validazione, risorse, cancellazione e deadline.
    #[error(transparent)]
    Predicate(#[from] PredicateError),
}

const fn dimension(g: &Geometry<f64>) -> Result<u8, LineBooleanError> {
    match g {
        Geometry::Point(_) | Geometry::MultiPoint(_) => Ok(0),
        Geometry::LineString(_) | Geometry::MultiLineString(_) => Ok(1),
        Geometry::Polygon(_) => Ok(2),
        _ => Err(LineBooleanError::UnsupportedOperands),
    }
}

/// Booleana binaria con budget cumulativo di validazione, noding e uscita.
///
/// Stesso CRS e assi a carico del chiamante. Il filtro conserva soltanto
/// componenti della dimensione richiesta, senza estrarre confini. Contenitori
/// multi omogenei; collezioni miste in dimensione crescente; vuoto generale
/// `GeometryCollection` e vuoto filtrato multi tipizzato. La differenza conserva
/// la chiusura: poligono meno linea conserva il poligono. Le coordinate del
/// poligono seguono il pubblicatore comune delle booleane generali. I tratti
/// lineari non hanno duplicati o nodi costruiti superflui. Ogni ciclo ha
/// checkpoint contati, con deadline prima della cancellazione.
/// Limiti contabili e scratch aritmetici esclusi come in `RelateLimits`.
///
/// # Errors
/// Tipi o filtro non supportati, geometria invalida/non finita, nodo non
/// pubblicabile senza alterare la topologia, limiti, allocazione, cancellazione o deadline.
pub(crate) fn boolean_with_limits(
    left: &Geometry<f64>,
    right: &Geometry<f64>,
    operation: LineBooleanOp,
    output_dimension: Option<u8>,
    limits: RelateLimits,
    cancelled: Option<&AtomicBool>,
) -> Result<Geometry<f64>, LineBooleanError> {
    execute(
        left,
        right,
        operation,
        output_dimension,
        limits,
        &mut Work::new(limits, cancelled),
    )
}

const fn selected(op: LineBooleanOp, left: bool, right: bool) -> bool {
    match op {
        LineBooleanOp::Union => left || right,
        LineBooleanOp::Intersection => left && right,
        LineBooleanOp::Difference => left && !right,
    }
}

pub(super) fn point_cmp(a: &P, b: &P) -> Result<Ordering, PredicateError> {
    let x = a.x.try_cmp(&b.x)?;
    if x.is_eq() {
        a.y.try_cmp(&b.y)
    } else {
        Ok(x)
    }
}

pub(super) fn edge_cmp(a: &Edge, b: &Edge) -> Result<Ordering, PredicateError> {
    let first = point_cmp(&a.a, &b.a)?;
    if first.is_eq() {
        point_cmp(&a.b, &b.b)
    } else {
        Ok(first)
    }
}

pub(super) fn ordered_ids<T>(
    values: &[T],
    cmp: impl Fn(&T, &T) -> Result<Ordering, PredicateError>,
    work: &mut Work<'_>,
) -> Result<CountedVec<usize>, PredicateError> {
    let mut ids = CountedVec::new();
    for id in 0..values.len() {
        work.push(&mut ids, id)?;
    }
    sort_ids(&mut ids, |a, b| cmp(&values[a], &values[b]), work)?;
    ids.dedup_by(|a, b| Ok(cmp(&values[*a], &values[*b])?.is_eq()), work)?;
    Ok(ids)
}

fn select_edges(
    nodes: &[CountedVec<P>],
    a: &Shape,
    b: &Shape,
    op: LineBooleanOp,
    work: &mut Work<'_>,
) -> Result<CountedVec<Edge>, PredicateError> {
    let mut edges = CountedVec::new();
    for path in nodes {
        for pair in path.windows(2) {
            work.nodes(1)?;
            let mid = P {
                x: pair[0].x.try_midpoint(&pair[1].x)?,
                y: pair[0].y.try_midpoint(&pair[1].y)?,
            };
            if !selected(
                op,
                (!a.lines.is_empty() || !a.polygons.is_empty()) && a.position(&mid, work)? != 2,
                (!b.lines.is_empty() || !b.polygons.is_empty()) && b.position(&mid, work)? != 2,
            ) {
                continue;
            }
            let (start, end) = if point_cmp(&pair[0], &pair[1])?.is_gt() {
                (&pair[1], &pair[0])
            } else {
                (&pair[0], &pair[1])
            };
            work.nodes(2)?;
            work.push(
                &mut edges,
                Edge {
                    a: start.try_clone()?,
                    b: end.try_clone()?,
                },
            )?;
        }
    }
    let ids = ordered_ids(&edges, edge_cmp, work)?;
    let mut unique = CountedVec::new();
    for id in ids {
        work.nodes(2)?;
        work.push(
            &mut unique,
            Edge {
                a: edges[id].a.try_clone()?,
                b: edges[id].b.try_clone()?,
            },
        )?;
    }
    Ok(unique)
}

#[allow(clippy::too_many_lines)]
fn execute(
    left: &Geometry<f64>,
    right: &Geometry<f64>,
    op: LineBooleanOp,
    filter: Option<u8>,
    limits: RelateLimits,
    work: &mut Work<'_>,
) -> Result<Geometry<f64>, LineBooleanError> {
    work.step()?;
    if filter.is_some_and(|d| d > 2) {
        return Err(LineBooleanError::InvalidOutputDimension);
    }
    let (ld, rd) = (dimension(left)?, dimension(right)?);
    if ld.max(rd) == 2 && ld.min(rd) != 1 {
        return Err(LineBooleanError::UnsupportedOperands);
    }
    validate_structure(left, "left", work)?;
    validate_structure(right, "right", work)?;
    if ld == 2 || rd == 2 {
        let empty = Geometry::MultiPoint(MultiPoint::new(Vec::new()));
        if ld == 2 {
            super::relate(left, &empty, work)?;
        } else {
            super::relate(&empty, right, work)?;
        }
    }
    if ld == 2 || rd == 2 {
        use super::polygon_boolean::{execute_many, PolygonBooleanError, PolygonBooleanLimits};
        return execute_many(
            std::slice::from_ref(left),
            std::slice::from_ref(right),
            op,
            filter,
            &PolygonBooleanLimits {
                work: limits,
                // Ogni elemento addebita almeno uno step: nessun tetto aggiunto.
                max_edges: limits.max_steps,
                max_faces: limits.max_steps,
                max_rings: limits.max_steps,
            },
            work,
        )
        .map_err(|error| match error {
            PolygonBooleanError::UnsupportedOperands => LineBooleanError::UnsupportedOperands,
            PolygonBooleanError::InvalidOutputDimension => LineBooleanError::InvalidOutputDimension,
            PolygonBooleanError::InvalidRoundedTopology => LineBooleanError::InvalidRoundedTopology,
            PolygonBooleanError::CoordinateOutOfRange => LineBooleanError::CoordinateOutOfRange,
            PolygonBooleanError::Predicate(error) => LineBooleanError::Predicate(error),
        });
    }
    let mut a = Shape::default();
    let mut b = Shape::default();
    a.geometry(left, work)?;
    b.geometry(right, work)?;
    a.build_indices(work)?;
    b.build_indices(work)?;
    let mut source = CountedVec::new();
    for edge in a.edges().chain(b.edges()) {
        work.push(&mut source, edge)?;
    }
    let originals = Originals::new(left, right, work)?;
    let nodes = crate::noding::node(&source, work)?;
    let edges = select_edges(&nodes, &a, &b, op, work)?;
    let edges = super::polygon_boolean::carried_edges(&edges, &[], &originals, work)?;
    let edge_ids = ordered_ids(&edges, edge_cmp, work)?;
    let index = IntervalIndex::new(edges.iter().map(|e| (&e.a.x, &e.b.x)), work)?;
    let mut candidates = CountedVec::new();
    if op == LineBooleanOp::Intersection {
        for p in nodes.iter().flatten() {
            work.push(&mut candidates, p)?;
        }
    }
    for p in a.points.iter().chain(&b.points) {
        work.push(&mut candidates, p)?;
    }
    let point_ids = ordered_ids(&candidates, |a, b| point_cmp(a, b), work)?;
    let mut points = CountedVec::new();
    let mut published = CountedVec::new();
    for id in point_ids {
        work.step()?;
        let p = candidates[id];
        if !selected(op, a.position(p, work)? != 2, b.position(p, work)? != 2) {
            continue;
        }
        let mut covered = false;
        for id in index.candidates(&p.x, work)? {
            work.step()?;
            if edges[id].contains(p)? {
                covered = true;
                break;
            }
        }
        if !covered && filter.is_none_or(|d| d == 0) {
            let c = originals.coordinate(p, work)?;
            let publication = PublishedEdge::point(p, c, work)?;
            work.push(&mut published, publication)?;
            work.push(&mut points, Point(c))?;
        }
    }
    let mut components = CountedVec::new();
    if !points.is_empty() {
        sort_output(&mut points, |a, b| coordinate_cmp(a.0, b.0), work)?;
        let points = points.export(work)?;
        work.push(
            &mut components,
            Geometry::MultiPoint(MultiPoint::new(points)),
        )?;
    }
    if filter.is_none_or(|d| d == 1) {
        let mut lines = CountedVec::new();
        for id in edge_ids {
            let edge = &edges[id];
            let mut coords = CountedVec::new();
            for p in [&edge.a, &edge.b] {
                let c = originals.coordinate(p, work)?;
                work.push(&mut coords, c)?;
            }
            let publication = PublishedEdge::new(edge, [coords[0], coords[1]], work)?;
            work.push(&mut published, publication)?;
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
            let lines = lines.export(work)?;
            work.push(
                &mut components,
                Geometry::MultiLineString(MultiLineString::new(lines)),
            )?;
        }
    }
    verify_publication(&published, &originals, false, None, work)?;
    finish(components, filter, work)
}

pub(super) fn finish(
    mut components: CountedVec<Geometry<f64>>,
    filter: Option<u8>,
    work: &mut Work<'_>,
) -> Result<Geometry<f64>, LineBooleanError> {
    work.step()?;
    if components.len() == 1 {
        if let Some(g) = components.pop() {
            return Ok(g);
        }
    }
    if components.is_empty() {
        return Ok(match filter {
            Some(0) => Geometry::MultiPoint(MultiPoint::new(Vec::new())),
            Some(1) => Geometry::MultiLineString(MultiLineString::new(Vec::new())),
            Some(2) => Geometry::MultiPolygon(MultiPolygon::new(Vec::new())),
            _ => Geometry::GeometryCollection(GeometryCollection::empty()),
        });
    }
    Ok(Geometry::GeometryCollection(GeometryCollection(
        components.export(work)?,
    )))
}

// Ricerca nell'ordine monotono dei bit positivi: al massimo 63 confronti
// razionali per ordinata, anche per subnormali ed esponenti estremi.
pub(crate) fn ordinate(value: &Q, work: &mut Work<'_>) -> Result<f64, LineBooleanError> {
    let negative = value.sign().is_lt();
    let mut low = 0_u64;
    let mut high = f64::MAX.to_bits();
    while low <= high {
        work.step()?;
        let middle = low + (high - low) / 2;
        let candidate = f64::from_bits(middle);
        let signed = if negative { -candidate } else { candidate };
        let rational = P::of_in(Coord { x: signed, y: 0.0 }, work)?;
        let cmp = rational.x.try_cmp(value)?;
        let cmp = if negative { cmp.reverse() } else { cmp };
        match cmp {
            Ordering::Equal => return Ok(signed),
            Ordering::Less => low = middle + 1,
            Ordering::Greater => {
                if middle == 0 {
                    break;
                }
                high = middle - 1;
            }
        }
    }
    if low > f64::MAX.to_bits() {
        let maximum = P::of_in(
            Coord {
                x: f64::MAX,
                y: 0.0,
            },
            work,
        )?
        .x;
        let previous = P::of_in(
            Coord {
                x: f64::MAX.next_down(),
                y: 0.0,
            },
            work,
        )?
        .x;
        let half_ulp = maximum.try_sub(&previous)?.try_div(&work.integer(2)?)?;
        let threshold = maximum.try_add(&half_ulp)?;
        let magnitude = if negative {
            work.integer(0)?.try_sub(value)?
        } else {
            value.try_clone()?
        };
        if magnitude.try_cmp(&threshold)?.is_lt() {
            return Ok(if negative { -f64::MAX } else { f64::MAX });
        }
        return Err(LineBooleanError::CoordinateOutOfRange);
    }
    let signed = |bits| {
        let value = f64::from_bits(bits);
        if negative {
            -value
        } else {
            value
        }
    };
    let lower = P::of_in(
        Coord {
            x: signed(high),
            y: 0.0,
        },
        work,
    )?;
    let upper = P::of_in(
        Coord {
            x: signed(low),
            y: 0.0,
        },
        work,
    )?;
    let midpoint = lower.x.try_midpoint(&upper.x)?;
    let cmp = value.try_cmp(&midpoint)?;
    let cmp = if negative { cmp.reverse() } else { cmp };
    Ok(signed(match cmp {
        Ordering::Less => high,
        Ordering::Greater => low,
        Ordering::Equal => {
            if high & 1 == 0 {
                high
            } else {
                low
            }
        }
    }))
}

fn coordinate(p: &P, work: &mut Work<'_>) -> Result<Coord<f64>, LineBooleanError> {
    work.nodes(1)?;
    Ok(Coord {
        x: ordinate(&p.x, work)?,
        y: ordinate(&p.y, work)?,
    })
}

// Ordine numerico finito; i due zeri rappresentano lo stesso sito.
pub(super) fn coordinate_cmp(a: Coord<f64>, b: Coord<f64>) -> Ordering {
    let canonical = |v: f64| if v.to_bits() << 1 == 0 { 0.0 } else { v };
    canonical(a.x)
        .total_cmp(&canonical(b.x))
        .then_with(|| canonical(a.y).total_cmp(&canonical(b.y)))
}

pub(super) fn sort_output<T>(
    values: &mut [T],
    cmp: impl Fn(&T, &T) -> Ordering,
    work: &mut Work<'_>,
) -> Result<(), PredicateError> {
    let mut ids = CountedVec::new();
    let mut destinations = CountedVec::new();
    for i in 0..values.len() {
        work.push(&mut ids, i)?;
        work.push(&mut destinations, 0)?;
    }
    sort_ids(
        &mut ids,
        |a, b| Ok(cmp(&values[a], &values[b]).then(a.cmp(&b))),
        work,
    )?;
    for (destination, &source) in ids.iter().enumerate() {
        work.step()?;
        destinations[source] = destination;
    }
    for i in 0..values.len() {
        work.step()?;
        while destinations[i] != i {
            work.step()?;
            let other = destinations[i];
            values.swap(i, other);
            destinations.swap(i, other);
        }
    }
    Ok(())
}

// Provenienza indicizzata: nessuna scansione dei vertici per ciascun nodo.
pub(crate) struct Originals {
    values: CountedVec<(P, Coord<f64>)>,
    ids: CountedVec<usize>,
}
impl Originals {
    pub(crate) fn new(
        a: &Geometry<f64>,
        b: &Geometry<f64>,
        work: &mut Work<'_>,
    ) -> Result<Self, PredicateError> {
        Self::many([a, b], work)
    }
    pub(super) fn many<'g>(
        geometries: impl IntoIterator<Item = &'g Geometry<f64>>,
        work: &mut Work<'_>,
    ) -> Result<Self, PredicateError> {
        let mut values = CountedVec::new();
        Self::collect_geometries(geometries, &mut values, work)?;
        Self::from_values(values, work)
    }
    /// Provenienza degli editor: prefisso sorgente, poi visita iterativa.
    pub(crate) fn for_editing(
        prefix: impl Iterator<Item = Coord<f64>>,
        geometry: &Geometry<f64>,
        work: &mut Work<'_>,
    ) -> Result<Self, PredicateError> {
        let mut values = CountedVec::new();
        Self::collect(prefix, &mut values, work)?;
        Self::collect_geometries([geometry], &mut values, work)?;
        Self::from_values(values, work)
    }
    fn collect_geometries<'g>(
        geometries: impl IntoIterator<Item = &'g Geometry<f64>>,
        values: &mut CountedVec<(P, Coord<f64>)>,
        work: &mut Work<'_>,
    ) -> Result<(), PredicateError> {
        let mut pending = CountedVec::new();
        for geometry in geometries {
            work.push(&mut pending, geometry)?;
            while let Some(geometry) = pending.pop() {
                work.step()?;
                if let Geometry::GeometryCollection(gs) = geometry {
                    for child in gs.0.iter().rev() {
                        work.push(&mut pending, child)?;
                    }
                } else {
                    match geometry {
                        Geometry::MultiLineString(ls) => {
                            for line in &ls.0 {
                                Self::collect(line.coords_iter(), values, work)?;
                            }
                        }
                        Geometry::Polygon(p) => Self::collect_polygon(p, values, work)?,
                        Geometry::MultiPolygon(ps) => {
                            for polygon in &ps.0 {
                                Self::collect_polygon(polygon, values, work)?;
                            }
                        }
                        _ => Self::collect(geometry.coords_iter(), values, work)?,
                    }
                }
            }
        }
        Ok(())
    }
    pub(crate) fn from_coords(
        coords: impl Iterator<Item = Coord<f64>>,
        work: &mut Work<'_>,
    ) -> Result<Self, PredicateError> {
        let mut values = CountedVec::new();
        Self::collect(coords, &mut values, work)?;
        Self::from_values(values, work)
    }
    fn from_values(
        values: CountedVec<(P, Coord<f64>)>,
        work: &mut Work<'_>,
    ) -> Result<Self, PredicateError> {
        let mut ids = CountedVec::new();
        for id in 0..values.len() {
            work.push(&mut ids, id)?;
        }
        sort_ids(
            &mut ids,
            |a, b| Ok(point_cmp(&values[a].0, &values[b].0)?.then(a.cmp(&b))),
            work,
        )?;
        ids.dedup_by(|a, b| Ok(values[*a].0 == values[*b].0), work)?;
        Ok(Self { values, ids })
    }
    fn collect(
        coords: impl Iterator<Item = Coord<f64>>,
        values: &mut CountedVec<(P, Coord<f64>)>,
        work: &mut Work<'_>,
    ) -> Result<(), PredicateError> {
        work.step()?;
        for c in coords {
            work.nodes(1)?;
            work.push(values, (P::of_in(c, work)?, c))?;
        }
        Ok(())
    }
    fn collect_polygon(
        p: &geo::Polygon<f64>,
        values: &mut CountedVec<(P, Coord<f64>)>,
        work: &mut Work<'_>,
    ) -> Result<(), PredicateError> {
        work.step()?;
        for ring in std::iter::once(p.exterior()).chain(p.interiors()) {
            Self::collect(ring.coords_iter(), values, work)?;
        }
        Ok(())
    }
    pub(super) fn contains(&self, p: &P, work: &mut Work<'_>) -> Result<bool, PredicateError> {
        let (mut low, mut high) = (0, self.ids.len());
        while low < high {
            work.step()?;
            let mid = low + (high - low) / 2;
            match point_cmp(&self.values[self.ids[mid]].0, p)? {
                Ordering::Equal => return Ok(true),
                Ordering::Less => low = mid + 1,
                Ordering::Greater => high = mid,
            }
        }
        Ok(false)
    }
    pub(crate) fn coordinate(
        &self,
        p: &P,
        work: &mut Work<'_>,
    ) -> Result<Coord<f64>, LineBooleanError> {
        let (mut low, mut high) = (0, self.ids.len());
        while low < high {
            work.step()?;
            let mid = low + (high - low) / 2;
            let (original, coord) = &self.values[self.ids[mid]];
            match point_cmp(original, p)? {
                Ordering::Equal => return Ok(*coord),
                Ordering::Less => low = mid + 1,
                Ordering::Greater => high = mid,
            }
        }
        coordinate(p, work)
    }
}

/// Certificato di un lato; un sito isolato ha estremi uguali.
/// La modalita' atomica richiede lati gia' nodati, quella percorso li certifica
/// anche in presenza di incroci interni non pubblicati come vertici.
pub(crate) struct PublishedEdge<'a> {
    exact: [&'a P; 2],
    rounded: Edge,
    coords: [Coord<f64>; 2],
    noded: bool,
    // Supporto di provenienza esatta; i nodi pubblicati seguono GOP-015A.
    pub(crate) support: Option<&'a Edge>,
    pub(crate) surface: bool,
    pub(crate) component: bool,
}
impl<'a> PublishedEdge<'a> {
    /// Corrispondenza ordinata fra nodi esatti e noding dell'output binary64.
    /// I nodi interni sono siti del certificato, non coordinate pubblicate.
    /// Richiede noding completo prima/dopo, intervalli e verso corrispondenti,
    /// estremi pubblicati ottenuti da Originals o copiati dall'ingresso.
    /// Il noding dopo la pubblicazione deve ricevere solo estremi binary64.
    pub(crate) fn noded(
        exact: [&'a P; 2],
        rounded: [&P; 2],
        work: &mut Work<'_>,
    ) -> Result<Self, PredicateError> {
        work.nodes(2)?;
        Ok(Self {
            exact,
            rounded: Edge {
                a: rounded[0].try_clone()?,
                b: rounded[1].try_clone()?,
            },
            coords: [Coord { x: 0.0, y: 0.0 }; 2],
            noded: true,
            surface: false,
            component: true,
            support: None,
        })
    }

    pub(crate) fn new(
        edge: &'a Edge,
        coords: [Coord<f64>; 2],
        work: &mut Work<'_>,
    ) -> Result<Self, PredicateError> {
        work.nodes(2)?;
        Ok(Self {
            exact: [&edge.a, &edge.b],
            support: None,
            surface: false,
            component: true,
            noded: false,
            coords,
            rounded: Edge {
                a: P::of_in(coords[0], work)?,
                b: P::of_in(coords[1], work)?,
            },
        })
    }
    pub(crate) fn point(
        p: &'a P,
        coord: Coord<f64>,
        work: &mut Work<'_>,
    ) -> Result<Self, PredicateError> {
        work.nodes(2)?;
        Ok(Self {
            exact: [p, p],
            support: None,
            surface: false,
            component: true,
            noded: false,
            coords: [coord, coord],
            rounded: Edge {
                a: P::of_in(coord, work)?,
                b: P::of_in(coord, work)?,
            },
        })
    }
}

// Le facce del completamento possono condividere lati: restano una collezione
// di Polygon validi con interni disgiunti, non un MultiPolygon OGC.
fn surface_polygons<'a>(
    geometry: &'a Geometry<f64>,
    work: &mut Work<'_>,
) -> Result<CountedVec<&'a geo::Polygon<f64>>, LineBooleanError> {
    let mut polygons = CountedVec::new();
    match geometry {
        Geometry::MultiPolygon(ps) => {
            for polygon in &ps.0 {
                work.push(&mut polygons, polygon)?;
            }
        }
        Geometry::GeometryCollection(gs) => {
            for geometry in &gs.0 {
                work.step()?;
                let Geometry::Polygon(polygon) = geometry else {
                    return Err(LineBooleanError::InvalidRoundedTopology);
                };
                work.push(&mut polygons, polygon)?;
            }
        }
        _ => return Err(LineBooleanError::InvalidRoundedTopology),
    }
    Ok(polygons)
}

// Ogni lato serializzato deve consumare esattamente un lato del certificato
// superficiale, nello stesso verso e con gli stessi bit delle coordinate.
fn verify_surface_edges(
    polygons: &[&geo::Polygon<f64>],
    edges: &[PublishedEdge<'_>],
    work: &mut Work<'_>,
) -> Result<(), LineBooleanError> {
    let key = |coords: [Coord<f64>; 2]| {
        [
            coords[0].x.to_bits(),
            coords[0].y.to_bits(),
            coords[1].x.to_bits(),
            coords[1].y.to_bits(),
        ]
    };
    let mut expected = CountedVec::new();
    let mut actual = CountedVec::new();
    for edge in edges {
        if edge.surface {
            work.push(&mut expected, key(edge.coords))?;
        }
    }
    for polygon in polygons {
        for ring in std::iter::once(polygon.exterior()).chain(polygon.interiors()) {
            for pair in ring.0.windows(2) {
                work.push(&mut actual, key([pair[0], pair[1]]))?;
            }
        }
    }
    crate::noding::sorted(&mut expected, |a, b| Ok(a.cmp(b)), work)?;
    crate::noding::sorted(&mut actual, |a, b| Ok(a.cmp(b)), work)?;
    if expected.len() != actual.len() {
        return Err(LineBooleanError::InvalidRoundedTopology);
    }
    for (a, b) in expected.iter().zip(&actual) {
        work.step()?;
        if a != b {
            return Err(LineBooleanError::InvalidRoundedTopology);
        }
    }
    Ok(())
}

fn verify_rings(
    polygons: &[&geo::Polygon<f64>],
    work: &mut Work<'_>,
) -> Result<(), LineBooleanError> {
    for polygon in polygons {
        for (ring_id, ring) in std::iter::once(polygon.exterior())
            .chain(polygon.interiors())
            .enumerate()
        {
            work.step()?;
            if ring.0.len() < 4 || ring.0.first() != ring.0.last() {
                return Err(LineBooleanError::InvalidRoundedTopology);
            }
            let mut minimum = 0;
            for (i, c) in ring.0[..ring.0.len() - 1].iter().enumerate() {
                work.step()?;
                if coordinate_cmp(*c, ring.0[minimum]).is_lt() {
                    minimum = i;
                }
            }
            let n = ring.0.len() - 1;
            work.nodes(3)?;
            let p = P::of_in(ring.0[minimum], work)?;
            let prev = P::of_in(ring.0[(minimum + n - 1) % n], work)?;
            let next = P::of_in(ring.0[(minimum + 1) % n], work)?;
            let sign = p.try_sub(&prev)?.try_cross(&next.try_sub(&p)?)?.sign();
            if sign
                != if ring_id == 0 {
                    Ordering::Greater
                } else {
                    Ordering::Less
                }
            {
                return Err(LineBooleanError::InvalidRoundedTopology);
            }
        }
    }
    Ok(())
}

/// Verifica unica della pubblicazione di un grafo atomico: conversione corretta,
/// iniettivita' dei siti, assenza di nuovi contatti/overlap e ordine dei raggi.
/// Gli anelli semplici del grafo non possono collassare o invertirsi: nessuna
/// fusione, nessun nuovo incrocio e nessun cambio di verso dei settori non nulli.
/// Gli anelli costruiti consumano i lati superficiali orientati del certificato,
/// conservano il verso e superano la validazione OGC prima della pubblicazione.
/// I certificati gia' nodati preservano biiezione e settori dei siti virtuali;
/// non si mescolano a certificati ordinari o superficiali.
/// In modalita' atomica non rinoda razionali costruiti. Con `directed_path`
/// conserva anche gli incroci interni: noda testimone e uscita, confronta
/// ordine lungo i lati e identita' dei siti, senza pubblicare i nodi ausiliari.
#[allow(clippy::too_many_lines)]
pub(crate) fn verify_publication(
    edges: &[PublishedEdge<'_>],
    originals: &Originals,
    directed_path: bool,
    polygons: Option<&Geometry<f64>>,
    work: &mut Work<'_>,
) -> Result<(), LineBooleanError> {
    let noded = edges.first().is_some_and(|edge| edge.noded);
    let mut vertices = CountedVec::new();
    let mut refs = CountedVec::new();
    for edge in edges {
        work.step()?;
        if edge.noded != noded || (edge.noded && edge.surface) {
            return Err(LineBooleanError::InvalidRoundedTopology);
        }
        for (position, (exact, rounded)) in edge
            .exact
            .into_iter()
            .zip([&edge.rounded.a, &edge.rounded.b])
            .enumerate()
        {
            if !edge.noded {
                let expected = originals.coordinate(exact, work)?;
                let actual = edge.coords[position];
                if expected.x.to_bits() != actual.x.to_bits()
                    || expected.y.to_bits() != actual.y.to_bits()
                {
                    return Err(LineBooleanError::InvalidRoundedTopology);
                }
            }
            if let Some(support) = edge.support {
                work.pair()?;
                if !support.contains(exact)? {
                    return Err(LineBooleanError::InvalidRoundedTopology);
                }
            }
            work.push(&mut vertices, (exact, rounded))?;
        }
        work.push(&mut refs, &edge.rounded)?;
    }
    let mut ids = CountedVec::new();
    for id in 0..vertices.len() {
        work.push(&mut ids, id)?;
    }
    sort_ids(
        &mut ids,
        |a, b| point_cmp(vertices[a].1, vertices[b].1),
        work,
    )?;
    for pair in ids.windows(2) {
        work.step()?;
        let (a, b) = (vertices[pair[0]], vertices[pair[1]]);
        if a.1 == b.1 && a.0 != b.0 {
            return Err(LineBooleanError::InvalidRoundedTopology);
        }
    }
    // Una corrispondenza deve essere anche univoca dal sito esatto al convertito.
    sort_ids(
        &mut ids,
        |a, b| point_cmp(vertices[a].0, vertices[b].0),
        work,
    )?;
    for pair in ids.windows(2) {
        work.step()?;
        let (a, b) = (vertices[pair[0]], vertices[pair[1]]);
        if a.0 == b.0 && a.1 != b.1 {
            return Err(LineBooleanError::InvalidRoundedTopology);
        }
    }
    if noded {
        if directed_path || polygons.is_some() {
            return Err(LineBooleanError::InvalidRoundedTopology);
        }
        noded_sectors(edges, &vertices, &ids, work)?;
    } else if directed_path {
        verify_path_pairs(edges, work)?;
    } else {
        let mut invalid = false;
        let scan = crate::noding::visit(&refs, work, |i, j, hits, work| {
            let (a, b) = (&edges[i], &edges[j]);
            for hit in hits.into_iter().flatten() {
                work.step()?;
                let mut shared = false;
                for (ai, ap) in [&a.rounded.a, &a.rounded.b].into_iter().enumerate() {
                    for (bi, bp) in [&b.rounded.a, &b.rounded.b].into_iter().enumerate() {
                        work.step()?;
                        if ap == &hit && bp == &hit && a.exact[ai] == b.exact[bi] {
                            shared = true;
                            let before = a.exact[1 - ai]
                                .try_sub(a.exact[ai])?
                                .try_cross(&b.exact[1 - bi].try_sub(b.exact[bi])?)?
                                .sign();
                            let after = [&a.rounded.a, &a.rounded.b][1 - ai]
                                .try_sub(ap)?
                                .try_cross(&[&b.rounded.a, &b.rounded.b][1 - bi].try_sub(bp)?)?
                                .sign();
                            if !before.is_eq() && before != after {
                                invalid = true;
                            }
                        }
                    }
                }
                if !shared {
                    invalid = true;
                }
            }
            Ok(())
        });
        if invalid {
            return Err(LineBooleanError::InvalidRoundedTopology);
        }
        scan?;
    }
    work.step()?;
    if let Some(polygons) = polygons {
        let components = surface_polygons(polygons, work)?;
        verify_surface_edges(&components, edges, work)?;
        verify_rings(&components, work)?;
        let mut surface = super::Shape::default();
        surface.geometry(polygons, work)?;
        surface.build_indices(work)?;
        for edge in edges.iter().filter(|edge| !edge.surface && edge.component) {
            work.nodes(1)?;
            let middle = P {
                x: edge.rounded.a.x.try_midpoint(&edge.rounded.b.x)?,
                y: edge.rounded.a.y.try_midpoint(&edge.rounded.b.y)?,
            };
            if surface.position(&middle, work)? != 2 {
                return Err(LineBooleanError::InvalidRoundedTopology);
            }
        }
        let empty = Geometry::MultiPoint(MultiPoint::new(Vec::new()));
        match super::relate(polygons, &empty, work) {
            Err(PredicateError::InvalidGeometry { .. }) => {
                return Err(LineBooleanError::InvalidRoundedTopology);
            }
            result => {
                result?;
            }
        }
        if let Geometry::GeometryCollection(gs) = polygons {
            use crate::predicates::interval_index::{geometry_bounds, BoxIndex};
            let mut bounds = CountedVec::new();
            for g in &gs.0 {
                let b = geometry_bounds(g, work)?;
                work.push(&mut bounds, b)?;
            }
            let index = BoxIndex::new(bounds, work)?;
            for (i, a) in gs.0.iter().enumerate() {
                for j in index.candidates(index.at(i), work)? {
                    if j >= i {
                        continue;
                    }
                    if super::relate(a, &gs.0[j], work)?.cells()[0] == b'2' {
                        return Err(LineBooleanError::InvalidRoundedTopology);
                    }
                }
            }
        }
    }
    Ok(())
}

// Il percorso conserva identita' e ordine delle incidenze lungo ogni lato.
// La collinearita' di lati disgiunti non e' una proprieta' promessa.
fn verify_path_pairs(
    edges: &[PublishedEdge<'_>],
    work: &mut Work<'_>,
) -> Result<(), LineBooleanError> {
    let mut exact_edges = CountedVec::new();
    let mut rounded_refs = CountedVec::new();
    for edge in edges {
        work.nodes(2)?;
        work.push(
            &mut exact_edges,
            Edge {
                a: edge.exact[0].try_clone()?,
                b: edge.exact[1].try_clone()?,
            },
        )?;
        work.push(&mut rounded_refs, &edge.rounded)?;
    }
    let mut exact_refs = CountedVec::new();
    for edge in &exact_edges {
        work.push(&mut exact_refs, edge)?;
    }
    let exact_nodes = crate::noding::node(&exact_refs, work)?;
    let rounded_nodes = crate::noding::node(&rounded_refs, work)?;
    let mut certificates = CountedVec::new();
    for (before, after) in exact_nodes.iter().zip(&rounded_nodes) {
        work.step()?;
        if before.len() != after.len() {
            return Err(LineBooleanError::InvalidRoundedTopology);
        }
        for (before, after) in before.windows(2).zip(after.windows(2)) {
            let certificate =
                PublishedEdge::noded([&before[0], &before[1]], [&after[0], &after[1]], work)?;
            work.push(&mut certificates, certificate)?;
        }
    }
    let empty = Geometry::GeometryCollection(GeometryCollection::empty());
    let originals = Originals::new(&empty, &empty, work)?;
    verify_publication(&certificates, &originals, false, None, work)?;
    work.step()?;
    Ok(())
}

fn noded_sectors(
    edges: &[PublishedEdge<'_>],
    vertices: &[(&P, &P)],
    ids: &[usize],
    work: &mut Work<'_>,
) -> Result<(), LineBooleanError> {
    let mut start = 0;
    while start < ids.len() {
        work.step()?;
        let mut end = start + 1;
        while end < ids.len() && vertices[ids[start]].0 == vertices[ids[end]].0 {
            work.step()?;
            end += 1;
        }
        for i in start..end {
            for j in i + 1..end {
                work.pair()?;
                let (ai, bi) = (ids[i], ids[j]);
                let (a, b) = (&edges[ai / 2], &edges[bi / 2]);
                if !a.noded || !b.noded {
                    return Err(LineBooleanError::InvalidRoundedTopology);
                }
                let (ap, bp) = (ai % 2, bi % 2);
                let before = a.exact[1 - ap]
                    .try_sub(a.exact[ap])?
                    .try_cross(&b.exact[1 - bp].try_sub(b.exact[bp])?)?
                    .sign();
                let ar = [&a.rounded.a, &a.rounded.b];
                let br = [&b.rounded.a, &b.rounded.b];
                let after = ar[1 - ap]
                    .try_sub(ar[ap])?
                    .try_cross(&br[1 - bp].try_sub(br[bp])?)?
                    .sign();
                if !before.is_eq() && before != after {
                    return Err(LineBooleanError::InvalidRoundedTopology);
                }
            }
        }
        start = end;
    }
    work.step()?;
    Ok(())
}

#[cfg(test)]
#[deny(clippy::disallowed_methods)]
mod tests;

/// Semantica, ordine, precisione e limiti di [`boolean_with_limits`].
///
/// # Errors
/// Gli stessi errori di [`boolean_with_limits`].
pub(crate) fn union_with_limits(
    left: &Geometry<f64>,
    right: &Geometry<f64>,
    output_dimension: Option<u8>,
    limits: RelateLimits,
    cancelled: Option<&AtomicBool>,
) -> Result<Geometry<f64>, LineBooleanError> {
    boolean_with_limits(
        left,
        right,
        LineBooleanOp::Union,
        output_dimension,
        limits,
        cancelled,
    )
}

/// Semantica, ordine, precisione e limiti di [`boolean_with_limits`].
///
/// # Errors
/// Gli stessi errori di [`boolean_with_limits`].
pub(crate) fn intersection_with_limits(
    left: &Geometry<f64>,
    right: &Geometry<f64>,
    output_dimension: Option<u8>,
    limits: RelateLimits,
    cancelled: Option<&AtomicBool>,
) -> Result<Geometry<f64>, LineBooleanError> {
    boolean_with_limits(
        left,
        right,
        LineBooleanOp::Intersection,
        output_dimension,
        limits,
        cancelled,
    )
}

/// Semantica, ordine, precisione e limiti di [`boolean_with_limits`].
///
/// # Errors
/// Gli stessi errori di [`boolean_with_limits`].
pub(crate) fn difference_with_limits(
    left: &Geometry<f64>,
    right: &Geometry<f64>,
    output_dimension: Option<u8>,
    limits: RelateLimits,
    cancelled: Option<&AtomicBool>,
) -> Result<Geometry<f64>, LineBooleanError> {
    boolean_with_limits(
        left,
        right,
        LineBooleanOp::Difference,
        output_dimension,
        limits,
        cancelled,
    )
}

/// Ingressi con CRS verificato e consenso planare esplicito.
pub mod with_crs {
    use super::{AtomicBool, Geometry, LineBooleanError, LineBooleanOp, RelateLimits};

    crate::crs::checked_kernel!(boolean_with_limits, "geo.union", 2,
        (left: &Geometry<f64>, right: &Geometry<f64>, operation: LineBooleanOp, output_dimension: Option<u8>, limits: RelateLimits, cancelled: Option<&AtomicBool>) -> (Geometry<f64>, LineBooleanError),
        |context| {  super::boolean_with_limits(left, right, operation, output_dimension, limits, cancelled) });
    crate::crs::checked_kernel!(union_with_limits, "geo.union", 2,
        (left: &Geometry<f64>, right: &Geometry<f64>, output_dimension: Option<u8>, limits: RelateLimits, cancelled: Option<&AtomicBool>) -> (Geometry<f64>, LineBooleanError),
        |context| {  super::union_with_limits(left, right, output_dimension, limits, cancelled) });
    crate::crs::checked_kernel!(intersection_with_limits, "geo.intersection", 2,
        (left: &Geometry<f64>, right: &Geometry<f64>, output_dimension: Option<u8>, limits: RelateLimits, cancelled: Option<&AtomicBool>) -> (Geometry<f64>, LineBooleanError),
        |context| {  super::intersection_with_limits(left, right, output_dimension, limits, cancelled) });
    crate::crs::checked_kernel!(difference_with_limits, "geo.difference", 2,
        (left: &Geometry<f64>, right: &Geometry<f64>, output_dimension: Option<u8>, limits: RelateLimits, cancelled: Option<&AtomicBool>) -> (Geometry<f64>, LineBooleanError),
        |context| {  super::difference_with_limits(left, right, output_dimension, limits, cancelled) });
}
