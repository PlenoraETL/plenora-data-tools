//! Split di Polygon/MultiPolygon mediante linework in Rust puro, sopra il
//! polygonize di [`super::polygonize`].
//!
//! Portato da `plenora-memory-lab/operations/geo_rust/split/src/lib.rs`
//! (SHA-256 `7f9931c0ad4d7fb791c5ce207e03d86656ed4725d15e9fd48482f17cd2bf487a`,
//! lo stesso registrato in `results/geo-rust/fuzz-provenance.json`). Le
//! modifiche sono solo quelle elencate nel modulo padre.
#![forbid(unsafe_code)]
#![deny(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
// Codice portato dal laboratorio (vedi il modulo padre): la numerica e'
// quella qualificata contro GEOS, e i lint qui sotto la toccherebbero.
// `mul_add` e `f64::midpoint` cambierebbero l'arrotondamento del campione
// interno e della tolleranza sul bordo; i nomi dei binding sono quelli della
// sorgente qualificata.
#![allow(
    clippy::suboptimal_flops,
    clippy::manual_midpoint,
    clippy::similar_names
)]

use std::collections::{BTreeMap, BTreeSet};

use super::polygonize::{
    polygonize_linework_rust, PolygonizeError, PolygonizeLimits, PolygonizeOptions,
};
use geo::algorithm::validation::Validation;
use geo::kernels::{Kernel, Orientation, RobustKernel};
use geo::{
    Area, Contains, Coord, CoordsIter, Geometry, InteriorPoint, LineString, MultiLineString, Point,
    Polygon,
};
use thiserror::Error;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SplitLimits {
    pub max_input_coordinates: u64,
    pub max_noding_work: u64,
    pub max_output_parts: u64,
    pub max_output_coordinates: u64,
}

impl SplitLimits {
    #[must_use]
    pub const fn unlimited() -> Self {
        Self {
            max_input_coordinates: u64::MAX,
            max_noding_work: u64::MAX,
            max_output_parts: u64::MAX,
            max_output_coordinates: u64::MAX,
        }
    }

    /// Indica che input, noding e output hanno tutti un tetto esplicito.
    #[must_use]
    pub const fn is_fully_bounded(self) -> bool {
        self.max_input_coordinates != u64::MAX
            && self.max_noding_work != u64::MAX
            && self.max_output_parts != u64::MAX
            && self.max_output_coordinates != u64::MAX
    }
}

#[derive(Debug, Error)]
pub enum SplitError {
    #[error("tipo sorgente non supportato: {0}")]
    UnsupportedSource(&'static str),
    #[error("tipo splitter non supportato: {0}")]
    UnsupportedSplitter(&'static str),
    #[error("input non valido: {0}")]
    InvalidInput(String),
    #[error("profilo limiti incompleto: input, noding e output devono avere tetti espliciti")]
    UnboundedLimitConfiguration,
    #[error("coordinate oltre il limite di {limit}: {actual}")]
    CoordinateLimit { actual: u64, limit: u64 },
    #[error("lavoro di noding oltre il limite di {limit}: {actual}")]
    WorkLimit { actual: u64, limit: u64 },
    #[error("output oltre il limite di {limit}: {actual}")]
    OutputLimit { actual: u64, limit: u64 },
    #[error("polygonize Rust fallita: {0}")]
    Polygonize(PolygonizeError),
    /// Le aree confrontate non entrano nell'errore: sono grandezze derivate
    /// dalla geometria di input, cioe' dato di cella (regola «errori senza
    /// dati»). Deviazione dalla sorgente del laboratorio, che le
    /// portava nel messaggio; il controllo e' invariato.
    #[error("lo split poligonale non conserva l'area dell'input")]
    AreaMismatch,
    #[error("lo split poligonale non ricopre esattamente l'input")]
    CoverageMismatch,
    /// La precisione dichiarata passata non e' un numero finito positivo.
    #[error("precisione dichiarata non valida: deve essere finita e positiva")]
    InvalidPrecision,
    #[error("indice non rappresentabile")]
    IndexOverflow,
    #[error("invariante interna violata: {0}")]
    InternalInvariant(&'static str),
    #[error("prenotazione di memoria fallita per {0}")]
    AllocationFailed(&'static str),
}

impl From<PolygonizeError> for SplitError {
    fn from(error: PolygonizeError) -> Self {
        match error {
            PolygonizeError::CoordinateLimit { actual, limit } => {
                Self::CoordinateLimit { actual, limit }
            }
            PolygonizeError::WorkLimit { actual, limit } => Self::WorkLimit { actual, limit },
            PolygonizeError::OutputLimit { actual, limit } => Self::OutputLimit { actual, limit },
            PolygonizeError::IndexOverflow => Self::IndexOverflow,
            other => Self::Polygonize(other),
        }
    }
}

const fn geometry_type(geometry: &Geometry<f64>) -> &'static str {
    match geometry {
        Geometry::Point(_) => "Point",
        Geometry::Line(_) => "Line",
        Geometry::LineString(_) => "LineString",
        Geometry::Polygon(_) => "Polygon",
        Geometry::MultiPoint(_) => "MultiPoint",
        Geometry::MultiLineString(_) => "MultiLineString",
        Geometry::MultiPolygon(_) => "MultiPolygon",
        Geometry::GeometryCollection(_) => "GeometryCollection",
        Geometry::Rect(_) => "Rect",
        Geometry::Triangle(_) => "Triangle",
    }
}

fn collect_splitter(
    geometry: &Geometry<f64>,
    output: &mut Vec<LineString<f64>>,
) -> Result<(), SplitError> {
    match geometry {
        Geometry::LineString(line) => {
            output
                .try_reserve(1)
                .map_err(|_| SplitError::AllocationFailed("linee dello splitter"))?;
            output.push(line.clone());
        }
        Geometry::MultiLineString(lines) => {
            output
                .try_reserve(lines.0.len())
                .map_err(|_| SplitError::AllocationFailed("linee dello splitter"))?;
            output.extend(lines.0.iter().cloned());
        }
        Geometry::GeometryCollection(collection) => {
            for child in &collection.0 {
                collect_splitter(child, output)?;
            }
        }
        other => return Err(SplitError::UnsupportedSplitter(geometry_type(other))),
    }
    Ok(())
}

fn collect_boundaries(
    source: &Geometry<f64>,
    output: &mut Vec<LineString<f64>>,
) -> Result<(), SplitError> {
    let mut add_polygon = |polygon: &Polygon<f64>| -> Result<(), SplitError> {
        let additional = polygon
            .interiors()
            .len()
            .checked_add(1)
            .ok_or(SplitError::IndexOverflow)?;
        output
            .try_reserve(additional)
            .map_err(|_| SplitError::AllocationFailed("anelli del boundary sorgente"))?;
        output.push(polygon.exterior().clone());
        output.extend(polygon.interiors().iter().cloned());
        Ok(())
    };
    match source {
        Geometry::Polygon(polygon) => add_polygon(polygon)?,
        Geometry::MultiPolygon(polygons) => {
            for polygon in &polygons.0 {
                add_polygon(polygon)?;
            }
        }
        other => return Err(SplitError::UnsupportedSource(geometry_type(other))),
    }
    Ok(())
}

fn checked_combined_input_coordinates(
    source: &Geometry<f64>,
    splitter: &Geometry<f64>,
    limit: u64,
) -> Result<(), SplitError> {
    let source_count =
        u64::try_from(source.coords_count()).map_err(|_| SplitError::IndexOverflow)?;
    let splitter_count =
        u64::try_from(splitter.coords_count()).map_err(|_| SplitError::IndexOverflow)?;
    let actual = source_count
        .checked_add(splitter_count)
        .ok_or(SplitError::IndexOverflow)?;
    if actual > limit {
        return Err(SplitError::CoordinateLimit { actual, limit });
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct CoordKey(u64, u64);

impl CoordKey {
    fn new(coordinate: Coord<f64>) -> Self {
        let x = if coordinate.x == 0.0 {
            0.0
        } else {
            coordinate.x
        };
        let y = if coordinate.y == 0.0 {
            0.0
        } else {
            coordinate.y
        };
        Self(x.to_bits(), y.to_bits())
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct SegmentKey(CoordKey, CoordKey);

impl SegmentKey {
    fn new(start: Coord<f64>, end: Coord<f64>) -> Option<Self> {
        let start = CoordKey::new(start);
        let end = CoordKey::new(end);
        match start.cmp(&end) {
            std::cmp::Ordering::Equal => None,
            std::cmp::Ordering::Less => Some(Self(start, end)),
            std::cmp::Ordering::Greater => Some(Self(end, start)),
        }
    }
}

/// Distanza euclidea del punto dal segmento, in `f64` (l'errore del
/// calcolo e' molto sotto la precisione dichiarata).
fn segment_distance(point: Coord<f64>, start: Coord<f64>, end: Coord<f64>) -> f64 {
    let dx = end.x - start.x;
    let dy = end.y - start.y;
    let length_squared = dx * dx + dy * dy;
    let t = if length_squared > 0.0 {
        (((point.x - start.x) * dx + (point.y - start.y) * dy) / length_squared).clamp(0.0, 1.0)
    } else {
        0.0
    };
    (point.x - (start.x + t * dx)).hypot(point.y - (start.y + t * dy))
}

/// Test pari-dispari del laboratorio con il lato del punto deciso da
/// `orient2d` esatto invece che dall'ascissa d'incrocio in `f64`, che su
/// facce sottili o lontane dall'origine sbagliava lato (deviazione
/// dichiarata in `super`). `point.x < x_incrocio` equivale a «il punto sta a
/// sinistra del lato orientato verso l'alto».
fn ring_contains_point(ring: &LineString<f64>, point: Point<f64>) -> bool {
    let mut inside = false;
    for pair in ring.0.windows(2) {
        let start = pair[0];
        let end = pair[1];
        let crosses = (start.y > point.y()) != (end.y > point.y());
        if crosses {
            let orientation = RobustKernel::orient2d(start, end, point.0);
            let left_of_crossing = if end.y > start.y {
                orientation == Orientation::CounterClockwise
            } else {
                orientation == Orientation::Clockwise
            };
            if left_of_crossing {
                inside = !inside;
            }
        }
    }
    inside
}

fn face_sample(face: &Polygon<f64>) -> Option<Point<f64>> {
    let coordinates = &face.exterior().0;
    if coordinates.len() < 4 {
        return None;
    }
    let mut minimum_x = f64::INFINITY;
    let mut maximum_x = f64::NEG_INFINITY;
    let mut minimum_y = f64::INFINITY;
    let mut maximum_y = f64::NEG_INFINITY;
    for coordinate in coordinates {
        minimum_x = minimum_x.min(coordinate.x);
        maximum_x = maximum_x.max(coordinate.x);
        minimum_y = minimum_y.min(coordinate.y);
        maximum_y = maximum_y.max(coordinate.y);
    }
    let span_x = (maximum_x - minimum_x).abs();
    let span_y = (maximum_y - minimum_y).abs();
    if span_x == 0.0 || span_y == 0.0 {
        return None;
    }
    // Verso esatto dell'anello (deviazione dichiarata in `super`). Se non e'
    // decidibile non si sceglie un verso: si passa direttamente a
    // `interior_point`, come quando nessun campione direzionale e' interno.
    let orientation = match super::exact::orientamento(coordinates) {
        Ok(std::cmp::Ordering::Less) => -1.0,
        Ok(_) => 1.0,
        Err(_) => return face.interior_point(),
    };
    for pair in coordinates.windows(2) {
        let start = pair[0];
        let end = pair[1];
        let normalized_dx = (end.x - start.x) / span_x;
        let normalized_dy = (end.y - start.y) / span_y;
        let normal_length = normalized_dx.hypot(normalized_dy);
        if normal_length == 0.0 {
            continue;
        }
        let direction_x = -normalized_dy * orientation / normal_length;
        let direction_y = normalized_dx * orientation / normal_length;
        let midpoint_x = (start.x + end.x) * 0.5;
        let midpoint_y = (start.y + end.y) * 0.5;
        for factor in [1e-3, 1e-5, 1e-7, 1e-9, 1e-11] {
            let point = Point::new(
                midpoint_x + direction_x * span_x * factor,
                midpoint_y + direction_y * span_y * factor,
            );
            if face.contains(&point) {
                return Some(point);
            }
        }
    }
    face.interior_point()
}

fn polygon_contains_point(polygon: &Polygon<f64>, point: Point<f64>) -> bool {
    ring_contains_point(polygon.exterior(), point)
        && !polygon
            .interiors()
            .iter()
            .any(|ring| ring_contains_point(ring, point))
}

fn source_contains_point(source: &Geometry<f64>, point: Point<f64>) -> bool {
    match source {
        Geometry::Polygon(polygon) => polygon_contains_point(polygon, point),
        Geometry::MultiPolygon(polygons) => polygons
            .iter()
            .any(|polygon| polygon_contains_point(polygon, point)),
        _ => false,
    }
}

/// Lato delle parti (chiave canonica) -> estremi e numero di parti che lo
/// usano.
type OutputSegments = BTreeMap<SegmentKey, (Coord<f64>, Coord<f64>, u64)>;

/// I lati delle parti con il numero di parti che li usano: un lato interno
/// (taglio) compare due volte, un lato di bordo una.
fn output_segment_counts(output: &[Polygon<f64>]) -> Result<OutputSegments, SplitError> {
    let mut segments = OutputSegments::new();
    for polygon in output {
        for ring in std::iter::once(polygon.exterior()).chain(polygon.interiors()) {
            for segment in ring.lines() {
                let Some(key) = SegmentKey::new(segment.start, segment.end) else {
                    continue;
                };
                let entry = segments
                    .entry(key)
                    .or_insert((segment.start, segment.end, 0));
                entry.2 = entry.2.checked_add(1).ok_or(SplitError::IndexOverflow)?;
            }
        }
    }
    Ok(segments)
}

/// Il parametro della proiezione di `point` sul segmento, in `[0, 1]`.
fn projection_parameter(point: Coord<f64>, start: Coord<f64>, end: Coord<f64>) -> f64 {
    let dx = end.x - start.x;
    let dy = end.y - start.y;
    let length_squared = dx * dx + dy * dy;
    if length_squared > 0.0 {
        (((point.x - start.x) * dx + (point.y - start.y) * dy) / length_squared).clamp(0.0, 1.0)
    } else {
        0.0
    }
}

/// La lunghezza, come frazione di `[0, 1]`, dell'unione degli intervalli.
fn union_fraction(intervals: &mut [(f64, f64)]) -> f64 {
    intervals.sort_by(|left, right| left.0.total_cmp(&right.0).then(left.1.total_cmp(&right.1)));
    let mut covered = 0.0;
    let mut current: Option<(f64, f64)> = None;
    for &(start, end) in intervals.iter() {
        current = match current {
            Some((open, close)) if start <= close => Some((open, close.max(end))),
            Some((open, close)) => {
                covered += close - open;
                Some((start, end))
            }
            None => Some((start, end)),
        };
    }
    if let Some((open, close)) = current {
        covered += close - open;
    }
    covered
}

/// Copertura del bordo della sorgente, entro la precisione dichiarata `p`
/// (unita' delle coordinate).
///
/// Ogni lato delle parti usato una volta sola deve stare entro `p` da un
/// lato della sorgente (entrambi gli estremi, quindi tutto il lato), e copre
/// di quel lato la sua proiezione; conta per **ogni** lato sorgente entro
/// `p`, cosi' un pezzo vicino a un vertice copre entrambi i lati. Per ogni
/// anello della sorgente la lunghezza **scoperta**, sommata su tutti i suoi
/// lati, deve restare entro `p`: le parti sono fatte dei pezzi nodati dei
/// lati, che condividono i vertici, e un bordo coperto non ha buchi.
///
/// Il laboratorio confrontava lato per lato la somma delle lunghezze dei
/// pezzi con la lunghezza del lato, entro una tolleranza per pezzo: un buco
/// di 1 m^2 a lati di 1 cm, omesso dall'output, lasciava scoperto ogni lato
/// per meno della tolleranza e passava. La somma per anello non si divide.
fn checked_boundary_coverage(
    source: &Geometry<f64>,
    boundary_segments: &[(Coord<f64>, Coord<f64>)],
    precision: f64,
) -> Result<(), SplitError> {
    let mut source_rings = Vec::new();
    collect_boundaries(source, &mut source_rings)?;
    let mut rings = Vec::new();
    for ring in &source_rings {
        let segments = ring.lines().collect::<Vec<_>>();
        let intervals = vec![Vec::<(f64, f64)>::new(); segments.len()];
        rings.push((segments, intervals));
    }
    for &(start, end) in boundary_segments {
        let mut matched = false;
        for (segments, intervals) in &mut rings {
            for (segment, covered) in segments.iter().zip(intervals.iter_mut()) {
                let distance = segment_distance(start, segment.start, segment.end)
                    .max(segment_distance(end, segment.start, segment.end));
                if distance <= precision {
                    matched = true;
                    let first = projection_parameter(start, segment.start, segment.end);
                    let second = projection_parameter(end, segment.start, segment.end);
                    covered
                        .try_reserve(1)
                        .map_err(|_| SplitError::AllocationFailed("copertura del bordo"))?;
                    covered.push((first.min(second), first.max(second)));
                }
            }
        }
        if !matched {
            return Err(SplitError::CoverageMismatch);
        }
    }
    for (segments, mut intervals) in rings {
        let mut uncovered = 0.0;
        for (segment, covered) in segments.iter().zip(intervals.iter_mut()) {
            let length = (segment.end.x - segment.start.x).hypot(segment.end.y - segment.start.y);
            uncovered += length * (1.0 - union_fraction(covered)).max(0.0);
        }
        // Anche un valore non confrontabile (NaN) e' un errore.
        if !matches!(
            uncovered.partial_cmp(&precision),
            Some(std::cmp::Ordering::Less | std::cmp::Ordering::Equal)
        ) {
            return Err(SplitError::CoverageMismatch);
        }
    }
    Ok(())
}

/// Maggiorazione dell'errore d'arrotondamento dell'area di un anello
/// (formula di Gauss sulle differenze dal primo vertice, come in `geo`):
/// `8 * n * EPSILON` per il quadrato dell'estensione.
fn ring_area_rounding(ring: &LineString<f64>) -> f64 {
    let mut minimum_x = f64::INFINITY;
    let mut maximum_x = f64::NEG_INFINITY;
    let mut minimum_y = f64::INFINITY;
    let mut maximum_y = f64::NEG_INFINITY;
    for coordinate in &ring.0 {
        minimum_x = minimum_x.min(coordinate.x);
        maximum_x = maximum_x.max(coordinate.x);
        minimum_y = minimum_y.min(coordinate.y);
        maximum_y = maximum_y.max(coordinate.y);
    }
    if ring.0.is_empty() {
        return 0.0;
    }
    let extent = (maximum_x - minimum_x) + (maximum_y - minimum_y);
    #[allow(
        clippy::cast_precision_loss,
        reason = "maggiorazione: il numero di vertici entra come ordine di grandezza"
    )]
    let vertices = ring.0.len() as f64;
    8.0 * vertices * f64::EPSILON * extent * extent
}

fn polygons_area_rounding<'a>(polygons: impl Iterator<Item = &'a Polygon<f64>>) -> f64 {
    polygons
        .flat_map(|polygon| std::iter::once(polygon.exterior()).chain(polygon.interiors()))
        .map(ring_area_rounding)
        .sum()
}

fn checked_output(
    source: &Geometry<f64>,
    output: &[Polygon<f64>],
    limits: SplitLimits,
    precision: f64,
    input_vertices: &BTreeSet<CoordKey>,
) -> Result<(), SplitError> {
    let parts = u64::try_from(output.len()).map_err(|_| SplitError::IndexOverflow)?;
    if parts > limits.max_output_parts {
        return Err(SplitError::OutputLimit {
            actual: parts,
            limit: limits.max_output_parts,
        });
    }
    let coordinates = output.iter().try_fold(0_u64, |total, polygon| {
        let count = u64::try_from(polygon.coords_count()).map_err(|_| SplitError::IndexOverflow)?;
        total.checked_add(count).ok_or(SplitError::IndexOverflow)
    })?;
    if coordinates > limits.max_output_coordinates {
        return Err(SplitError::OutputLimit {
            actual: coordinates,
            limit: limits.max_output_coordinates,
        });
    }
    let mut boundary_segments = Vec::new();
    for (start, end, count) in output_segment_counts(output)?.into_values() {
        match count {
            1 => {
                boundary_segments
                    .try_reserve(1)
                    .map_err(|_| SplitError::AllocationFailed("lati di bordo delle parti"))?;
                boundary_segments.push((start, end));
            }
            2 => {}
            _ => return Err(SplitError::CoverageMismatch),
        }
    }
    // Area entro la precisione dichiarata `p` (README, «Limiti dichiarati»),
    // con un margine **locale**: i tagli interni si compensano, e i vertici
    // d'ingresso restano esatti, quindi l'area puo' cambiare solo dove un
    // lato di bordo ha un estremo calcolato dal noding (spostato al piu' di
    // `p`): al piu' `p` per la lunghezza di quei lati, piu' l'arrotondamento
    // delle aree. Il laboratorio usava `1e-9 * max(area, 1)`, poi il
    // perimetro di tutte le parti per `p`: un buco di 1 m^2 omesso stava
    // sotto `p` per 400 m di perimetro.
    let input_area = source.unsigned_area();
    let output_area = output.iter().map(Area::unsigned_area).sum::<f64>();
    let displaced_length = boundary_segments
        .iter()
        .filter(|(start, end)| {
            !input_vertices.contains(&CoordKey::new(*start))
                || !input_vertices.contains(&CoordKey::new(*end))
        })
        .map(|(start, end)| (end.x - start.x).hypot(end.y - start.y))
        .sum::<f64>();
    let source_rounding = match source {
        Geometry::Polygon(polygon) => polygons_area_rounding(std::iter::once(polygon)),
        Geometry::MultiPolygon(polygons) => polygons_area_rounding(polygons.0.iter()),
        _ => 0.0,
    };
    let allowed_error =
        precision * displaced_length + source_rounding + polygons_area_rounding(output.iter());
    if !matches!(
        (output_area - input_area).abs().partial_cmp(&allowed_error),
        Some(std::cmp::Ordering::Less | std::cmp::Ordering::Equal)
    ) {
        return Err(SplitError::AreaMismatch);
    }
    let Some(_) = output.first() else {
        return if input_area == 0.0 {
            Ok(())
        } else {
            Err(SplitError::CoverageMismatch)
        };
    };
    checked_boundary_coverage(source, &boundary_segments, precision)
}

/// Divide Polygon/MultiPolygon mediante linework usando soltanto Rust.
///
/// Boundary e splitter vengono completamente nodati dal polygonizer Rust; le
/// facce il cui punto interno appartiene alla sorgente diventano le parti. Il
/// risultato viene verificato indipendentemente per area e copertura.
///
/// # Errors
///
/// Restituisce un errore per tipi o geometrie non validi, limiti superati,
/// fallimento della polygonizzazione o mancata conservazione di area/copertura.
pub fn split_polygon_by_linework_rust(
    source: &Geometry<f64>,
    splitter: &Geometry<f64>,
    limits: SplitLimits,
    precision: f64,
) -> Result<Vec<Polygon<f64>>, SplitError> {
    if !(precision.is_finite() && precision > 0.0) {
        return Err(SplitError::InvalidPrecision);
    }
    if !matches!(source, Geometry::Polygon(_) | Geometry::MultiPolygon(_)) {
        return Err(SplitError::UnsupportedSource(geometry_type(source)));
    }
    source
        .check_validation()
        .map_err(|error| SplitError::InvalidInput(error.to_string()))?;
    splitter
        .check_validation()
        .map_err(|error| SplitError::InvalidInput(error.to_string()))?;
    checked_combined_input_coordinates(source, splitter, limits.max_input_coordinates)?;
    let mut linework = Vec::new();
    collect_boundaries(source, &mut linework)?;
    collect_splitter(splitter, &mut linework)?;
    let input_vertices = linework
        .iter()
        .flat_map(|line| line.0.iter().copied())
        .map(CoordKey::new)
        .collect::<BTreeSet<_>>();
    let faces = polygonize_linework_rust(
        &Geometry::MultiLineString(MultiLineString::new(linework)),
        PolygonizeOptions {
            node_input: true,
            require_complete: false,
            limits: PolygonizeLimits {
                max_input_coordinates: u64::MAX,
                max_noding_work: limits.max_noding_work,
                max_output_geometries: limits.max_output_parts,
                max_output_coordinates: limits.max_output_coordinates,
            },
            precision,
        },
    )?;
    let mut output = Vec::new();
    let mut output_parts = 0_u64;
    let mut output_coordinates = 0_u64;
    for polygon in faces.polygons {
        let point = face_sample(&polygon).ok_or(SplitError::InternalInvariant(
            "punto interno della faccia polygonize assente",
        ))?;
        if source_contains_point(source, point) {
            let next_parts = output_parts
                .checked_add(1)
                .ok_or(SplitError::IndexOverflow)?;
            if next_parts > limits.max_output_parts {
                return Err(SplitError::OutputLimit {
                    actual: next_parts,
                    limit: limits.max_output_parts,
                });
            }
            let coordinates =
                u64::try_from(polygon.coords_count()).map_err(|_| SplitError::IndexOverflow)?;
            let next_coordinates = output_coordinates
                .checked_add(coordinates)
                .ok_or(SplitError::IndexOverflow)?;
            if next_coordinates > limits.max_output_coordinates {
                return Err(SplitError::OutputLimit {
                    actual: next_coordinates,
                    limit: limits.max_output_coordinates,
                });
            }
            output
                .try_reserve(1)
                .map_err(|_| SplitError::AllocationFailed("parti di output"))?;
            output.push(polygon);
            output_parts = next_parts;
            output_coordinates = next_coordinates;
        }
    }
    checked_output(source, &output, limits, precision, &input_vertices)?;
    Ok(output)
}

/// Variante per l'integrazione controllata che richiede tutti i budget.
///
/// # Errors
///
/// Restituisce [`SplitError::UnboundedLimitConfiguration`] se almeno un
/// limite e' lasciato a [`u64::MAX`]; altrimenti propaga gli errori di
/// [`split_polygon_by_linework_rust`].
pub fn split_polygon_by_linework_rust_bounded(
    source: &Geometry<f64>,
    splitter: &Geometry<f64>,
    limits: SplitLimits,
    precision: f64,
) -> Result<Vec<Polygon<f64>>, SplitError> {
    if !limits.is_fully_bounded() {
        return Err(SplitError::UnboundedLimitConfiguration);
    }
    split_polygon_by_linework_rust(source, splitter, limits, precision)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Precisione dichiarata dei test del laboratorio: coordinate astratte,
    /// un miliardesimo come le tolleranze originali.
    const PRECISION: f64 = 1e-9;
    use geo::{line_string, polygon};

    /// I vertici d'ingresso di una sorgente senza splitter.
    fn source_vertices(source: &Geometry<f64>) -> BTreeSet<CoordKey> {
        source.coords_iter().map(CoordKey::new).collect()
    }

    /// Anello quadrato di lato `side` con angolo in `(min_x, min_y)`, ogni
    /// lato diviso in `pieces` segmenti.
    fn subdivided_square(min_x: f64, min_y: f64, side: f64, pieces: u32) -> LineString<f64> {
        let corners = [
            (min_x, min_y),
            (min_x + side, min_y),
            (min_x + side, min_y + side),
            (min_x, min_y + side),
        ];
        let mut points = Vec::new();
        for (index, &(start_x, start_y)) in corners.iter().enumerate() {
            let (end_x, end_y) = corners[(index + 1) % corners.len()];
            for step in 0..pieces {
                let t = f64::from(step) / f64::from(pieces);
                points.push(Coord {
                    x: start_x + (end_x - start_x) * t,
                    y: start_y + (end_y - start_y) * t,
                });
            }
        }
        points.push(points[0]);
        LineString::new(points)
    }

    /// Controesempio della revisione: un buco di 1 m^2 con lati da 1 cm,
    /// omesso dall'output. Ogni lato scoperto stava sotto la tolleranza per
    /// pezzo, e 1 m^2 sotto il perimetro (400 m) per 1 cm: passava. Ora la
    /// copertura si somma sull'anello e l'area ha un margine locale.
    #[test]
    fn an_omitted_hole_with_centimetre_edges_is_rejected() {
        let shell = subdivided_square(0.0, 0.0, 100.0, 1);
        let hole = subdivided_square(50.0, 50.0, 1.0, 100);
        let source = Geometry::Polygon(Polygon::new(shell.clone(), vec![hole]));
        let without_hole = vec![Polygon::new(shell, vec![])];
        let result = checked_output(
            &source,
            &without_hole,
            SplitLimits::unlimited(),
            0.01,
            &source_vertices(&source),
        );
        assert!(matches!(
            result,
            Err(SplitError::AreaMismatch | SplitError::CoverageMismatch)
        ));
        // Solo copertura: l'anello del buco resta scoperto per 4 m.
        let boundary = without_hole[0]
            .exterior()
            .lines()
            .map(|line| (line.start, line.end))
            .collect::<Vec<_>>();
        assert!(matches!(
            checked_boundary_coverage(&source, &boundary, 0.01),
            Err(SplitError::CoverageMismatch)
        ));
    }

    /// Il punto sta a sinistra del lato `s -> e` (segno esatto), ma
    /// l'ascissa d'incrocio in `f64` del laboratorio lo metteva a destra:
    /// il test pari-dispari lo dava fuori da un triangolo che lo contiene.
    #[test]
    fn even_odd_side_is_decided_exactly() {
        let hex = |bits: u64| f64::from_bits(bits);
        let start = Coord {
            x: hex(0x41D0_0000_0046_3657),
            y: hex(0x41D0_0000_000B_22C3),
        };
        let end = Coord {
            x: hex(0x41D0_0000_02E1_6D7B),
            y: hex(0x41D0_0000_FA07_32FD),
        };
        let point = Point::new(hex(0x41D0_0000_0167_8C1E), hex(0x41D0_0000_6C72_C0A0));
        let apex = Coord {
            x: start.x - 100.0,
            y: end.y,
        };
        let ring = LineString::new(vec![start, end, apex, start]);
        let crossing_x = (end.x - start.x) * (point.y() - start.y) / (end.y - start.y) + start.x;
        assert!(point.x() >= crossing_x, "la formula in f64 sbaglia lato");
        assert_eq!(
            RobustKernel::orient2d(start, end, point.0),
            Orientation::CounterClockwise
        );
        assert!(ring_contains_point(&ring, point));
        assert!(Polygon::new(ring, Vec::new()).contains(&point));
    }

    /// Campagna differenziale traslata di `2^30`, seme 1 caso 5: GEOS lo
    /// risolve esattamente, le tolleranze `1e-9` del laboratorio lo
    /// rifiutavano ("non ricopre esattamente l'input"). In metri con 1 cm
    /// escono le due parti.
    #[test]
    fn translated_split_within_the_precision_is_accepted() -> Result<(), SplitError> {
        let base = 1_073_741_824.0; // 2^30
        let source = Geometry::Polygon(Polygon::new(
            line_string![
                (x: 1_071_741_824.0, y: base + 0.000_003),
                (x: 1_078_741_824.0, y: base + 0.000_003),
                (x: 1_078_741_824.0, y: base + 0.000_017),
                (x: 1_071_741_824.0, y: base + 0.000_017),
                (x: 1_071_741_824.0, y: base + 0.000_003)
            ],
            vec![],
        ));
        let splitter = Geometry::LineString(line_string![
            (x: 1_075_741_824.0, y: base), (x: 1_075_741_824.0, y: base + 0.000_02)
        ]);
        let output =
            split_polygon_by_linework_rust(&source, &splitter, SplitLimits::unlimited(), 0.01)?;
        if output.len() != 2 {
            return Err(SplitError::CoverageMismatch);
        }
        Ok(())
    }

    /// Una parte mancante piu' larga della precisione resta un errore: i
    /// controlli a posteriori, con la tolleranza di 1 cm, non la coprono.
    #[test]
    fn a_missing_piece_above_the_precision_is_rejected() {
        let source = Geometry::Polygon(polygon![
            (x: 0.0, y: 0.0), (x: 10.0, y: 0.0),
            (x: 10.0, y: 10.0), (x: 0.0, y: 10.0),
            (x: 0.0, y: 0.0)
        ]);
        let left = polygon![
            (x: 0.0, y: 0.0), (x: 5.0, y: 0.0),
            (x: 5.0, y: 10.0), (x: 0.0, y: 10.0),
            (x: 0.0, y: 0.0)
        ];
        let result = checked_output(
            &source,
            &[left],
            SplitLimits::unlimited(),
            0.01,
            &source_vertices(&source),
        );
        assert!(matches!(
            result,
            Err(SplitError::AreaMismatch | SplitError::CoverageMismatch)
        ));
    }

    #[test]
    fn splits_rectangle_with_vertical_line() -> Result<(), SplitError> {
        let source = Geometry::Polygon(polygon![
            (x: 0.0, y: 0.0), (x: 10.0, y: 0.0),
            (x: 10.0, y: 10.0), (x: 0.0, y: 10.0),
            (x: 0.0, y: 0.0)
        ]);
        let splitter = Geometry::LineString(line_string![
            (x: 5.0, y: -1.0), (x: 5.0, y: 11.0)
        ]);
        let output = split_polygon_by_linework_rust(
            &source,
            &splitter,
            SplitLimits::unlimited(),
            PRECISION,
        )?;
        if output.len() != 2 {
            return Err(SplitError::CoverageMismatch);
        }
        Ok(())
    }

    #[test]
    fn enforces_contract_and_is_deterministic() -> Result<(), SplitError> {
        let source = Geometry::Polygon(polygon![
            (x: 0.0, y: 0.0), (x: 10.0, y: 0.0),
            (x: 10.0, y: 10.0), (x: 0.0, y: 10.0),
            (x: 0.0, y: 0.0)
        ]);
        let splitter = Geometry::LineString(line_string![
            (x: 5.0, y: -1.0), (x: 5.0, y: 11.0)
        ]);
        let first = split_polygon_by_linework_rust(
            &source,
            &splitter,
            SplitLimits::unlimited(),
            PRECISION,
        )?;
        let second = split_polygon_by_linework_rust(
            &source,
            &splitter,
            SplitLimits::unlimited(),
            PRECISION,
        )?;
        if first != second {
            return Err(SplitError::CoverageMismatch);
        }
        let coordinate_limit = split_polygon_by_linework_rust(
            &source,
            &splitter,
            SplitLimits {
                max_input_coordinates: 4,
                ..SplitLimits::unlimited()
            },
            PRECISION,
        );
        if !matches!(coordinate_limit, Err(SplitError::CoordinateLimit { .. })) {
            return Err(SplitError::CoverageMismatch);
        }
        let work_limit = split_polygon_by_linework_rust(
            &source,
            &splitter,
            SplitLimits {
                max_noding_work: 24,
                ..SplitLimits::unlimited()
            },
            PRECISION,
        );
        if !matches!(work_limit, Err(SplitError::WorkLimit { .. })) {
            return Err(SplitError::CoverageMismatch);
        }
        let part_limit = split_polygon_by_linework_rust(
            &source,
            &splitter,
            SplitLimits {
                max_output_parts: 1,
                ..SplitLimits::unlimited()
            },
            PRECISION,
        );
        if !matches!(part_limit, Err(SplitError::OutputLimit { .. })) {
            return Err(SplitError::CoverageMismatch);
        }
        let output_coordinate_limit = split_polygon_by_linework_rust(
            &source,
            &splitter,
            SplitLimits {
                max_output_coordinates: 9,
                ..SplitLimits::unlimited()
            },
            PRECISION,
        );
        if !matches!(output_coordinate_limit, Err(SplitError::OutputLimit { .. })) {
            return Err(SplitError::CoverageMismatch);
        }
        let unsupported_source = split_polygon_by_linework_rust(
            &Geometry::Point(geo::Point::new(0.0, 0.0)),
            &splitter,
            SplitLimits::unlimited(),
            PRECISION,
        );
        if !matches!(unsupported_source, Err(SplitError::UnsupportedSource(_))) {
            return Err(SplitError::CoverageMismatch);
        }
        let unsupported_splitter =
            split_polygon_by_linework_rust(&source, &source, SplitLimits::unlimited(), PRECISION);
        if !matches!(
            unsupported_splitter,
            Err(SplitError::UnsupportedSplitter(_))
        ) {
            return Err(SplitError::CoverageMismatch);
        }
        Ok(())
    }

    #[test]
    fn splits_narrow_polygon_when_cutter_touches_hole_vertex() -> Result<(), SplitError> {
        let source = Geometry::Polygon(Polygon::new(
            line_string![
                (x: 0.3, y: -0.7), (x: 1.8, y: -0.7),
                (x: 1.8, y: 17.3), (x: 0.3, y: 17.3),
                (x: 0.3, y: -0.7)
            ],
            vec![line_string![
                (x: 0.8, y: 3.3), (x: 1.3, y: 3.3),
                (x: 1.3, y: 13.3), (x: 0.8, y: 13.3),
                (x: 0.8, y: 3.3)
            ]],
        ));
        let splitter = Geometry::MultiLineString(MultiLineString::new(vec![
            line_string![(x: -0.45, y: 3.3), (x: 4.8, y: -2.7)],
            line_string![(x: -0.45, y: 17.3), (x: 4.8, y: 5.3)],
        ]));
        let output = split_polygon_by_linework_rust(
            &source,
            &splitter,
            SplitLimits::unlimited(),
            PRECISION,
        )?;
        if output.len() != 3 {
            return Err(SplitError::CoverageMismatch);
        }
        Ok(())
    }

    #[test]
    fn splits_when_diagonal_touches_hole_vertex() -> Result<(), SplitError> {
        let source = Geometry::Polygon(Polygon::new(
            line_string![
                (x: 0.3, y: -0.7), (x: 3.55, y: -0.7),
                (x: 3.55, y: 15.3), (x: 0.3, y: 15.3),
                (x: 0.3, y: -0.7)
            ],
            vec![line_string![
                (x: 0.8, y: 3.3), (x: 3.05, y: 3.3),
                (x: 3.05, y: 11.3), (x: 0.8, y: 11.3),
                (x: 0.8, y: 3.3)
            ]],
        ));
        let splitter = Geometry::MultiLineString(MultiLineString::new(vec![
            line_string![(x: 8.3, y: 9.3), (x: -0.45, y: -0.7)],
            line_string![(x: 8.3, y: 17.3), (x: -0.45, y: 11.3)],
            line_string![(x: -0.2, y: 21.3), (x: -0.2, y: -6.7)],
        ]));
        let output = split_polygon_by_linework_rust(
            &source,
            &splitter,
            SplitLimits::unlimited(),
            PRECISION,
        )?;
        if output.len() != 3 {
            return Err(SplitError::CoverageMismatch);
        }
        Ok(())
    }
}
