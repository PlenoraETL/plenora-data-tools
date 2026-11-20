//! Completamento GOP-020, profilo planare XY e pubblicazione GOP-015A.
use super::line_boolean::{
    coordinate_cmp as coord_cmp, verify_publication, LineBooleanError, Originals, PublishedEdge,
};
use super::{point_boolean::copy_line, Shape};
use crate::noding::faces::{FaceGraph, FaceRing};
use crate::noding::{CountedVec, Edge, Work, P};
use crate::predicates::{PredicateError, RelateLimits};
use geo::{Geometry, LineString, MultiPoint, Polygon};
use std::cmp::Ordering;
use std::sync::atomic::AtomicBool;

/// Errori senza coordinate o valori degli operandi.
#[derive(Debug, thiserror::Error)]
pub enum PolygonCompletionError {
    /// Il linework deve essere `LineString` o `MultiLineString` semplice.
    #[error("linework non supportato dal completamento")]
    UnsupportedLinework,
    /// Due poligoni hanno interni sovrapposti.
    #[error("interni poligonali sovrapposti")]
    OverlappingInteriors,
    /// Un lato atomico appartiene a piu' sorgenti.
    #[error("provenienza del lato ambigua")]
    AmbiguousProvenance,
    /// Pubblicazione nearest-even non compatibile con la topologia promessa.
    #[error("topologia non conservata dalla pubblicazione binary64")]
    InvalidRoundedTopology,
    /// Ordinata costruita non finita.
    #[error("coordinata costruita fuori dal dominio binary64 finito")]
    CoordinateOutOfRange,
    /// Geometria invalida, risorse, cancellazione o deadline.
    #[error(transparent)]
    Predicate(#[from] PredicateError),
}
impl From<LineBooleanError> for PolygonCompletionError {
    fn from(error: LineBooleanError) -> Self {
        match error {
            LineBooleanError::Predicate(error) => Self::Predicate(error),
            LineBooleanError::CoordinateOutOfRange => Self::CoordinateOutOfRange,
            _ => Self::InvalidRoundedTopology,
        }
    }
}

/// Restituisce ogni faccia limitata non occupata con almeno un lato nuovo
/// e uno esistente di lunghezza positiva, compresi i bordi dei buchi.
///
/// Lista vuota quando nessuna faccia qualifica; residui aperti esclusi.
///
/// Poligoni OGC validi con interni disgiunti; linework `LineString` oppure
/// `MultiLineString` semplice, anche vuoto. Duplicati e sovrapposizioni di lati
/// sono ambigui e rifiutati, anche fra poligoni esistenti. Nessuna riparazione.
/// XY finiti planari, CRS e ordine degli assi comuni a carico del chiamante;
/// i tipi Rust non rappresentano Z, M o curve. Nessuna unita' metrica implicita.
/// Gusci antiorari e buchi orari dal minimo XY; facce e buchi ordinati per
/// minimo XY, area esatta e sequenza canonica. Vertici originali invariati,
/// nodi costruiti nearest-even con errore <= mezza ulp per ordinata.
/// Il certificato comune verifica incidenze e settori dopo il rounding;
/// i lati esistenti restano sul supporto originale, gli output sono validati.
///
/// Un solo `RelateLimits` cumulativo copre validazione, noding, grafo, output
/// e verifiche: passi e byte limitano anche lati e facce prima della crescita.
/// Ogni copia di coordinata consuma nodi. Input prestato, overhead allocatore
/// e scratch privati `BigInt` sono esclusi come in `RelateLimits`. Le primitive
/// intere rifiutano prima di superare 145173 bit. I confronti d'area usano
/// somme a punto fisso con resto certificato, senza denominatori cumulativi;
/// il massimo sforamento contabile resta `RelateLimits::MAX_VALUE_BYTES`.
/// Deadline precede cancellazione, come in tutte le API con limiti.
/// Checkpoint nei cicli, nei confronti di ordinamento e alla pubblicazione.
/// Ogni intero e' limitato a 145173 bit. La prova tabellare GOP-028 verifica
/// tutte le API con limiti all'ingresso e durante il calcolo.
/// `Polygon::new` visita i buchi senza callback: nessuna garanzia di latenza temporale.
///
/// # Errors
/// Errori tipizzati per dominio escluso, geometria invalida/non finita,
/// pubblicazione non certificabile, risorse, cancellazione e deadline.
pub fn polygon_completion_with_limits(
    polygons: &[Polygon<f64>],
    linework: &Geometry<f64>,
    limits: RelateLimits,
    cancelled: Option<&AtomicBool>,
) -> Result<Vec<Polygon<f64>>, PolygonCompletionError> {
    execute(polygons, linework, &mut Work::new(limits, cancelled))
}

fn occupied_index(
    copies: &[Geometry<f64>],
    work: &mut Work<'_>,
) -> Result<crate::predicates::interval_index::BoxIndex, PredicateError> {
    let mut bounds = CountedVec::new();
    for g in copies {
        let b = crate::predicates::interval_index::geometry_bounds(g, work)?;
        work.push(&mut bounds, b)?;
    }
    crate::predicates::interval_index::BoxIndex::new(bounds, work)
}

fn validate_polygons(
    polygons: &[Polygon<f64>],
    work: &mut Work<'_>,
) -> Result<CountedVec<Geometry<f64>>, PolygonCompletionError> {
    let empty = Geometry::MultiPoint(MultiPoint::new(Vec::new()));
    let mut copies = CountedVec::new();
    for polygon in polygons {
        work.step()?;
        // Polygon::new chiude gli anelli: verificare prima per non riparare input.
        for ring in std::iter::once(polygon.exterior()).chain(polygon.interiors()) {
            work.step()?;
            if !(polygon.exterior().0.is_empty() && polygon.interiors().is_empty())
                && (ring.0.len() < 4 || ring.0.first() != ring.0.last())
            {
                return Err(super::invalid("completion polygons").into());
            }
            for coord in &ring.0 {
                work.step()?;
                if !coord.x.is_finite() || !coord.y.is_finite() {
                    return Err(PredicateError::NonFiniteCoordinate {
                        side: "completion polygons",
                    }
                    .into());
                }
            }
        }
        let shell = copy_line(polygon.exterior(), work)?;
        let mut holes = CountedVec::new();
        for hole in polygon.interiors() {
            let hole = copy_line(hole, work)?;
            work.push(&mut holes, hole)?;
        }
        let geometry = Geometry::Polygon(Polygon::new(shell, holes.export(work)?));
        super::relate(&geometry, &empty, work)?;
        work.push(&mut copies, geometry)?;
    }
    let index = occupied_index(&copies, work)?;
    for (i, geometry) in copies.iter().enumerate() {
        for j in index.candidates(index.at(i), work)? {
            if j < i && super::relate(&copies[j], geometry, work)?.cells()[0] == b'2' {
                return Err(PolygonCompletionError::OverlappingInteriors);
            }
        }
    }
    Ok(copies)
}

fn validate_lines(
    linework: &Geometry<f64>,
    work: &mut Work<'_>,
) -> Result<Shape, PolygonCompletionError> {
    let mut parts = CountedVec::new();
    match linework {
        Geometry::LineString(line) => work.push(&mut parts, line)?,
        Geometry::MultiLineString(lines) => {
            for line in &lines.0 {
                work.push(&mut parts, line)?;
            }
        }
        _ => return Err(PolygonCompletionError::UnsupportedLinework),
    }
    super::validate_structure(linework, "completion linework", work)?;
    let mut shape = Shape::default();
    let mut owners = CountedVec::new();
    let mut lengths = CountedVec::new();
    for (part, line) in parts.iter().enumerate() {
        let mut segment = 0_usize;
        for pair in line.0.windows(2) {
            work.step()?;
            if pair[0] == pair[1] {
                continue;
            }
            work.push(&mut owners, (part, segment))?;
            segment += 1;
        }
        work.push(&mut lengths, segment)?;
        shape.line(line, work)?;
    }
    let mut refs = CountedVec::new();
    for edge in &shape.lines {
        work.push(&mut refs, edge)?;
    }
    let mut unsupported = false;
    crate::noding::visit(&refs, work, |i, j, hits, _| {
        if hits[1].is_some() {
            unsupported = true;
        }
        if let Some(hit) = &hits[0] {
            let (pi, si) = owners[i];
            let (pj, sj) = owners[j];
            let adjacent = pi == pj
                && (si.abs_diff(sj) == 1
                    || (parts[pi].0.first() == parts[pi].0.last()
                        && si.min(sj) == 0
                        && si.max(sj) + 1 == lengths[pi]));
            let terminal = |part: usize, segment: usize, edge: &Edge| {
                parts[part].0.first() != parts[part].0.last()
                    && ((segment == 0 && hit == &edge.a)
                        || (segment + 1 == lengths[part] && hit == &edge.b))
            };
            if !(adjacent || pi != pj && terminal(pi, si, refs[i]) && terminal(pj, sj, refs[j])) {
                unsupported = true;
            }
        }
        Ok(())
    })?;
    if unsupported {
        return Err(PolygonCompletionError::UnsupportedLinework);
    }
    Ok(shape)
}

// Un punto medio del primo lato con perturbazione a sinistra appartiene
// alla faccia: il grafo e' gia' nodato, senza intersezioni interne al lato.
fn occupied(
    ring: &FaceRing,
    graph: &FaceGraph,
    shape: &Shape,
    work: &mut Work<'_>,
) -> Result<bool, PredicateError> {
    work.nodes(1)?;
    let a = &graph.points[ring.vertices[0]];
    let b = &graph.points[ring.vertices[1]];
    let mid = P {
        x: a.x.try_midpoint(&b.x)?,
        y: a.y.try_midpoint(&b.y)?,
    };
    let left = P {
        x: a.y.try_sub(&b.y)?,
        y: b.x.try_sub(&a.x)?,
    };
    shape.area_contains(&mid, Some(&left), work)
}

struct OutputFace {
    shell: usize,
    holes: CountedVec<usize>,
}

fn roles(
    ring: &FaceRing,
    graph: &FaceGraph,
    roles: &[Option<usize>],
    work: &mut Work<'_>,
) -> Result<(bool, bool), PredicateError> {
    let (mut new, mut old) = (false, false);
    for &id in &ring.atoms {
        work.step()?;
        if roles[graph.atoms[id].sources[0]].is_some() {
            old = true;
        } else {
            new = true;
        }
    }
    Ok((new, old))
}

fn select(
    graph: &FaceGraph,
    roles_by_source: &[Option<usize>],
    shape: &Shape,
    work: &mut Work<'_>,
) -> Result<CountedVec<OutputFace>, PolygonCompletionError> {
    for atom in &graph.atoms {
        work.step()?;
        if atom.sources.iter().any(|&id| roles_by_source[id].is_none())
            && atom.sources.iter().any(|&id| roles_by_source[id].is_some())
        {
            return Err(PolygonCompletionError::AmbiguousProvenance);
        }
    }
    let mut result = CountedVec::new();
    for (id, ring) in graph.rings.iter().enumerate() {
        work.step()?;
        if !ring.orientation.is_gt() || occupied(ring, graph, shape, work)? {
            continue;
        }
        let (mut new, mut old) = roles(ring, graph, roles_by_source, work)?;
        let mut holes = CountedVec::new();
        for &h in &graph.holes[id] {
            let (hn, ho) = roles(&graph.rings[h], graph, roles_by_source, work)?;
            new |= hn;
            old |= ho;
            work.push(&mut holes, h)?;
        }
        if new && old {
            work.push(&mut result, OutputFace { shell: id, holes })?;
        }
    }
    Ok(result)
}

fn compare_areas(
    a: &[usize],
    b: &[usize],
    graph: &FaceGraph,
    work: &mut Work<'_>,
) -> Result<Ordering, PredicateError> {
    let mut terms = CountedVec::new();
    for (rings, subtract) in [(a, false), (b, true)] {
        for &id in rings {
            let vertices = &graph.rings[id].vertices;
            for i in 0..vertices.len() {
                work.step()?;
                let term = graph.points[vertices[i]]
                    .try_cross(&graph.points[vertices[(i + 1) % vertices.len()]])?;
                work.push(&mut terms, (term, subtract))?;
            }
        }
    }
    crate::noding::sum_sign(&terms, work)
}

fn ring_cmp(
    a: usize,
    b: usize,
    graph: &FaceGraph,
    coords: &[geo::Coord<f64>],
    areas: (&[usize], &[usize]),
    work: &mut Work<'_>,
) -> Result<Ordering, PredicateError> {
    let (a, b) = (&graph.rings[a], &graph.rings[b]);
    let order = coord_cmp(coords[a.vertices[0]], coords[b.vertices[0]]);
    if !order.is_eq() {
        return Ok(order);
    }
    let order = compare_areas(areas.0, areas.1, graph, work)?;
    if !order.is_eq() {
        return Ok(order);
    }
    for (&av, &bv) in a.vertices.iter().zip(&b.vertices) {
        work.step()?;
        let order = coord_cmp(coords[av], coords[bv]);
        if !order.is_eq() {
            return Ok(order);
        }
    }
    Ok(a.vertices.len().cmp(&b.vertices.len()))
}

fn canonicalize(
    graph: &mut FaceGraph,
    faces: &mut [OutputFace],
    coords: &[geo::Coord<f64>],
    work: &mut Work<'_>,
) -> Result<(), PredicateError> {
    for ring in graph.rings.iter_mut() {
        let mut min = 0;
        for i in 1..ring.vertices.len() {
            work.step()?;
            if coord_cmp(coords[ring.vertices[i]], coords[ring.vertices[min]]).is_lt() {
                min = i;
            }
        }
        // Rotazione tramite scambi contati, senza una primitiva lineare opaca.
        for range in [0..min, min..ring.vertices.len(), 0..ring.vertices.len()] {
            let mut lo = range.start;
            let mut hi = range.end;
            while lo < hi {
                work.step()?;
                hi -= 1;
                ring.vertices.swap(lo, hi);
                ring.atoms.swap(lo, hi);
                lo += 1;
            }
        }
    }
    for face in faces.iter_mut() {
        crate::noding::sorted_with_work(
            &mut face.holes,
            |a, b, work| ring_cmp(*a, *b, graph, coords, (&[*b], &[*a]), work),
            work,
        )?;
    }
    crate::noding::sorted_with_work(
        faces,
        |a, b, work| {
            let order = coord_cmp(
                coords[graph.rings[a.shell].vertices[0]],
                coords[graph.rings[b.shell].vertices[0]],
            );
            if !order.is_eq() {
                return Ok(order);
            }
            let mut aa = CountedVec::new();
            let mut bb = CountedVec::new();
            for &id in std::iter::once(&a.shell).chain(&a.holes) {
                work.push(&mut aa, id)?;
            }
            for &id in std::iter::once(&b.shell).chain(&b.holes) {
                work.push(&mut bb, id)?;
            }
            ring_cmp(a.shell, b.shell, graph, coords, (&aa, &bb), work)
        },
        work,
    )?;
    Ok(())
}

fn output_ring(
    ring: &FaceRing,
    coords: &[geo::Coord<f64>],
    work: &mut Work<'_>,
) -> Result<LineString<f64>, PredicateError> {
    let mut values = CountedVec::new();
    for &id in ring.vertices.iter().chain(ring.vertices.first()) {
        work.nodes(1)?;
        work.push(&mut values, coords[id])?;
    }
    Ok(LineString::new(values.export(work)?))
}

// Le facce conservano solo le incidenze pubblicate, con la stessa regola
// delle booleane. Area e provenienza restano quelle del grafo esatto.
fn simplify_selected_rings(
    graph: &mut FaceGraph,
    faces: &[OutputFace],
    originals: &Originals,
    work: &mut Work<'_>,
) -> Result<(), PredicateError> {
    let mut selected = CountedVec::new();
    for face in faces {
        for &id in std::iter::once(&face.shell).chain(&face.holes) {
            work.push(&mut selected, id)?;
        }
    }
    let mut degree = CountedVec::new();
    for _ in &graph.points {
        work.push(&mut degree, 0_u8)?;
    }
    for &id in &selected {
        for &v in &graph.rings[id].vertices {
            work.step()?;
            degree[v] = (degree[v] + 2).min(3);
        }
    }
    for &id in &selected {
        let ring = &graph.rings[id];
        let mut vertices = CountedVec::new();
        let mut atoms = CountedVec::new();
        for (i, &v) in ring.vertices.iter().enumerate() {
            work.step()?;
            let p = &graph.points[v];
            let prev =
                &graph.points[ring.vertices[(i + ring.vertices.len() - 1) % ring.vertices.len()]];
            let next = &graph.points[ring.vertices[(i + 1) % ring.vertices.len()]];
            if degree[v] != 2
                || originals.contains(p, work)?
                || !p
                    .try_sub(prev)?
                    .try_cross(&next.try_sub(p)?)?
                    .sign()
                    .is_eq()
            {
                work.push(&mut vertices, v)?;
                work.push(&mut atoms, ring.atoms[i])?;
            }
        }
        graph.rings[id].vertices = vertices;
        graph.rings[id].atoms = atoms;
    }
    Ok(())
}

#[allow(clippy::too_many_lines)]
fn execute(
    polygons: &[Polygon<f64>],
    linework: &Geometry<f64>,
    work: &mut Work<'_>,
) -> Result<Vec<Polygon<f64>>, PolygonCompletionError> {
    work.step()?;
    let copies = validate_polygons(polygons, work)?;
    let lines = validate_lines(linework, work)?;
    let mut shape = Shape::default();
    for polygon in polygons {
        shape.polygon(polygon, work)?;
    }
    shape.build_indices(work)?;
    let mut sources = CountedVec::new();
    let mut roles = CountedVec::new();
    for (id, rings) in shape.polygons.iter().enumerate() {
        for edge in rings.iter().flatten() {
            work.push(&mut sources, edge)?;
            work.push(&mut roles, Some(id))?;
        }
    }
    for edge in &lines.lines {
        work.push(&mut sources, edge)?;
        work.push(&mut roles, None)?;
    }
    let nodes = crate::noding::node(&sources, work)?;
    let mut graph = crate::noding::faces::enumerate(&nodes, work)?;
    let mut faces = select(&graph, &roles, &shape, work)?;
    if faces.is_empty() {
        work.step()?;
        return Ok(Vec::new());
    }
    let mut input_coords = CountedVec::new();
    for polygon in polygons {
        work.step()?;
        for ring in std::iter::once(polygon.exterior()).chain(polygon.interiors()) {
            work.step()?;
            for &coord in &ring.0 {
                work.nodes(1)?;
                work.push(&mut input_coords, coord)?;
            }
        }
    }
    match linework {
        Geometry::LineString(line) => {
            for &coord in &line.0 {
                work.nodes(1)?;
                work.push(&mut input_coords, coord)?;
            }
        }
        Geometry::MultiLineString(lines) => {
            for line in &lines.0 {
                work.step()?;
                for &coord in &line.0 {
                    work.nodes(1)?;
                    work.push(&mut input_coords, coord)?;
                }
            }
        }
        _ => return Err(PolygonCompletionError::UnsupportedLinework),
    }
    let originals = Originals::from_coords(input_coords.iter().copied(), work)?;
    simplify_selected_rings(&mut graph, &faces, &originals, work)?;
    let mut coords = CountedVec::new();
    for p in &graph.points {
        work.nodes(1)?;
        let c = originals.coordinate(p, work)?;
        work.push(&mut coords, c)?;
    }
    let mut exact = CountedVec::new();
    let mut supports = CountedVec::new();
    let mut published_coords = CountedVec::new();
    let mut selected_points = CountedVec::new();
    for _ in &graph.points {
        work.push(&mut selected_points, false)?;
    }
    // Una ricevuta orientata per ogni lato serializzato, anche se due facce
    // condividono lo stesso atomo in versi opposti.
    for face in &faces {
        for &ring_id in std::iter::once(&face.shell).chain(&face.holes) {
            let ring = &graph.rings[ring_id];
            for (i, &atom_id) in ring.atoms.iter().enumerate() {
                work.step()?;
                let atom = &graph.atoms[atom_id];
                let from = ring.vertices[i];
                let to = ring.vertices[(i + 1) % ring.vertices.len()];
                selected_points[from] = true;
                selected_points[to] = true;
                work.nodes(2)?;
                work.push(
                    &mut exact,
                    Edge {
                        a: graph.points[from].try_clone()?,
                        b: graph.points[to].try_clone()?,
                    },
                )?;
                work.nodes(2)?;
                work.push(&mut published_coords, [coords[from], coords[to]])?;
                work.push(
                    &mut supports,
                    roles[atom.sources[0]].map(|_| atom.sources[0]),
                )?;
            }
        }
    }
    let surface_count = exact.len();
    // I confini occupati sono suddivisi soltanto nei siti dell'output.
    // Un nodo escluso non impone un arrotondamento estraneo al risultato.
    for (source, path) in nodes.iter().enumerate() {
        work.step()?;
        if roles[source].is_none() {
            continue;
        }
        let mut previous = &path[0];
        for (position, p) in path.iter().enumerate().skip(1) {
            work.step()?;
            let mut selected = position + 1 == path.len();
            if !selected {
                let (mut low, mut high) = (0, graph.points.len());
                while low < high {
                    work.step()?;
                    let mid = low + (high - low) / 2;
                    match super::line_boolean::point_cmp(&graph.points[mid], p)? {
                        Ordering::Less => low = mid + 1,
                        Ordering::Greater => high = mid,
                        Ordering::Equal => {
                            selected = selected_points[mid];
                            break;
                        }
                    }
                }
            }
            if selected {
                work.nodes(2)?;
                work.push(
                    &mut exact,
                    Edge {
                        a: previous.try_clone()?,
                        b: p.try_clone()?,
                    },
                )?;
                let a = originals.coordinate(previous, work)?;
                let b = originals.coordinate(p, work)?;
                work.nodes(2)?;
                work.push(&mut published_coords, [a, b])?;
                work.push(&mut supports, Some(source))?;
                previous = p;
            }
        }
    }
    let mut certificate = CountedVec::new();
    for (id, ((edge, &coords), &support)) in exact
        .iter()
        .zip(&published_coords)
        .zip(&supports)
        .enumerate()
    {
        let mut published = PublishedEdge::new(edge, coords, work)?;
        published.support = support.map(|id| sources[id]);
        published.surface = id < surface_count;
        published.component = id < surface_count;
        work.push(&mut certificate, published)?;
    }
    canonicalize(&mut graph, &mut faces, &coords, work)?;
    let mut output = CountedVec::new();
    for face in faces {
        let shell = output_ring(&graph.rings[face.shell], &coords, work)?;
        let mut holes = CountedVec::new();
        for h in face.holes {
            let ring = output_ring(&graph.rings[h], &coords, work)?;
            work.push(&mut holes, ring)?;
        }
        let geometry = Geometry::Polygon(Polygon::new(shell, holes.export(work)?));
        work.push(&mut output, geometry)?;
    }
    let publication = Geometry::GeometryCollection(geo::GeometryCollection(output.export(work)?));
    verify_publication(&certificate, &originals, false, Some(&publication), work)?;
    let Geometry::GeometryCollection(output) = publication else {
        return Err(PolygonCompletionError::InvalidRoundedTopology);
    };
    let mut polygons = CountedVec::new();
    let occupied_index = occupied_index(&copies, work)?;
    for geometry in output.0 {
        // L'area occupata non e' un output modificabile del completamento.
        // GOP-020 richiede disgiunzione anche dopo il rounding dei nodi nuovi.
        let bounds = crate::predicates::interval_index::geometry_bounds(&geometry, work)?;
        for id in occupied_index.candidates(&bounds, work)? {
            if super::relate(&geometry, &copies[id], work)?.cells()[0] == b'2' {
                return Err(PolygonCompletionError::InvalidRoundedTopology);
            }
        }
        let Geometry::Polygon(polygon) = geometry else {
            return Err(PolygonCompletionError::InvalidRoundedTopology);
        };
        work.push(&mut polygons, polygon)?;
    }
    work.step()?;
    Ok(polygons.export(work)?)
}

#[cfg(test)]
#[deny(clippy::disallowed_methods)]
mod tests;

/// Ingressi con CRS verificato e consenso planare esplicito.
pub mod with_crs {
    use super::{AtomicBool, Geometry, Polygon, PolygonCompletionError, RelateLimits};

    crate::crs::checked_kernel!(polygon_completion_with_limits, "geo.split", 2,
        (polygons: &[Polygon<f64>], linework: &Geometry<f64>, limits: RelateLimits, cancelled: Option<&AtomicBool>) -> (Vec<Polygon<f64>>, PolygonCompletionError),
        |context| {  super::polygon_completion_with_limits(polygons, linework, limits, cancelled) });
}
