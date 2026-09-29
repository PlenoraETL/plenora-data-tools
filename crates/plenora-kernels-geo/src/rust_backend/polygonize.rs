//! Polygonize di linework 2D in Rust puro: grafo planare, noding iterativo
//! double-double, estrazione delle facce per mezzi archi ordinati.
//!
//! Portato da `plenora-memory-lab/operations/geo_rust/polygonize/src/lib.rs`
//! (SHA-256 `3f333e4b6992ba697c5e3c095d9db9485fefcba9b0d6c708b3851a41bb68fab0`,
//! lo stesso registrato in `results/geo-rust/fuzz-provenance.json`). Le
//! modifiche sono solo quelle elencate nel modulo padre.
#![forbid(unsafe_code)]
#![deny(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
// Codice portato dal laboratorio (vedi il modulo padre): la numerica e'
// quella qualificata contro GEOS, e i lint qui sotto la toccherebbero.
// `mul_add` cambierebbe l'arrotondamento del double-double di Dekker, che si
// regge proprio su prodotti e somme separati; i confronti esatti fra aree
// sono voluti (spareggi deterministici, nessuna tolleranza); `split_axis`
// resta un `if` perche' `usize::from` invertirebbe il ramo su un NaN;
// spezzare `extract_faces` o passare le facce per riferimento allontanerebbe
// il codice dalla sorgente qualificata senza cambiarne il comportamento.
#![allow(
    clippy::suboptimal_flops,
    clippy::float_cmp,
    clippy::similar_names,
    clippy::too_many_lines,
    clippy::bool_to_int_with_if,
    clippy::needless_pass_by_value
)]

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, VecDeque};

use super::exact::{self, AreaPoligono};

use geo::algorithm::line_intersection::{line_intersection, LineIntersection};
use geo::algorithm::validation::{InvalidPolygon, Validation};
use geo::kernels::{Kernel, Orientation, RobustKernel};
use geo::{Contains, Coord, CoordsIter, Geometry, InteriorPoint, Line, LineString, Point, Polygon};
use thiserror::Error;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PolygonizeLimits {
    pub max_input_coordinates: u64,
    pub max_noding_work: u64,
    pub max_output_geometries: u64,
    pub max_output_coordinates: u64,
}

impl PolygonizeLimits {
    #[must_use]
    pub const fn unlimited() -> Self {
        Self {
            max_input_coordinates: u64::MAX,
            max_noding_work: u64::MAX,
            max_output_geometries: u64::MAX,
            max_output_coordinates: u64::MAX,
        }
    }

    /// Indica che ogni asse del budget ha un tetto esplicito.
    #[must_use]
    pub const fn is_fully_bounded(self) -> bool {
        self.max_input_coordinates != u64::MAX
            && self.max_noding_work != u64::MAX
            && self.max_output_geometries != u64::MAX
            && self.max_output_coordinates != u64::MAX
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PolygonizeOptions {
    pub node_input: bool,
    pub require_complete: bool,
    pub limits: PolygonizeLimits,
    /// La precisione dichiarata nelle unita' delle coordinate (1 cm a terra
    /// con un CRS, `super::precision`): lo spostamento massimo che il noding
    /// puo' introdurre. Nessun valore predefinito.
    pub precision: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PolygonizeResult {
    pub polygons: Vec<Polygon<f64>>,
    pub cut_edges: Vec<LineString<f64>>,
    pub dangles: Vec<LineString<f64>>,
    pub invalid_ring_lines: Vec<LineString<f64>>,
}

impl PolygonizeResult {
    /// Restituisce il numero totale di residui senza consentire wrap di
    /// `usize` su piattaforme o input estremi.
    ///
    /// # Errors
    ///
    /// Restituisce [`PolygonizeError::IndexOverflow`] se la somma non e'
    /// rappresentabile.
    pub fn residual_count(&self) -> Result<usize, PolygonizeError> {
        self.cut_edges
            .len()
            .checked_add(self.dangles.len())
            .and_then(|value| value.checked_add(self.invalid_ring_lines.len()))
            .ok_or(PolygonizeError::IndexOverflow)
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum PolygonizeError {
    #[error("tipo geometria non supportato: {0}")]
    UnsupportedGeometry(&'static str),
    #[error("geometria di input non valida: {0}")]
    InvalidInput(String),
    #[error("profilo limiti incompleto: input, noding e output devono avere tetti espliciti")]
    UnboundedLimitConfiguration,
    #[error("coordinate oltre il limite di {limit}: {actual}")]
    CoordinateLimit { actual: u64, limit: u64 },
    #[error("lavoro di noding oltre il limite di {limit}: {actual}")]
    WorkLimit { actual: u64, limit: u64 },
    #[error(
        "noding non convergente dopo {iterations} iterazioni: intersezioni precedenti={previous}, correnti={current}"
    )]
    NodingDidNotConverge {
        iterations: usize,
        previous: usize,
        current: usize,
    },
    #[error("output oltre il limite di {limit}: {actual}")]
    OutputLimit { actual: u64, limit: u64 },
    #[error("polygonize incompleto: residui={residuals}")]
    Incomplete { residuals: usize },
    #[error("output poligonale non valido: {0}")]
    InvalidOutput(String),
    #[error("indice non rappresentabile")]
    IndexOverflow,
    /// Un segno o un confronto d'area che il filtro in `f64` non decide e
    /// che le coordinate non permettono di calcolare in modo esatto (vedi
    /// [`super::exact`]). Deviazione dal laboratorio, che decideva comunque.
    #[error("coordinate fuori dal dominio dell'aritmetica esatta delle aree")]
    NumericRange,
    /// Un punto di noding arrotondato in `f64` dista da uno dei segmenti che
    /// divide piu' della quota di precisione dichiarata di un giro (vedi
    /// [`NODING_PRECISION_SHARE`]): il grafo si sposterebbe oltre la
    /// precisione, e non si costruisce.
    #[error("noding oltre la precisione dichiarata delle coordinate")]
    PrecisionInsufficient,
    /// La precisione dichiarata passata non e' un numero finito positivo.
    #[error("precisione dichiarata non valida: deve essere finita e positiva")]
    InvalidPrecision,
    #[error("invariante interna violata: {0}")]
    InternalInvariant(&'static str),
    #[error("prenotazione di memoria fallita per {0}")]
    AllocationFailed(&'static str),
}

#[derive(Clone, Copy, Debug)]
struct OutputBudget {
    geometries: u64,
    coordinates: u64,
    limits: PolygonizeLimits,
}

#[derive(Clone, Copy, Debug)]
struct NodingBudget {
    work: u64,
    limit: u64,
}

impl NodingBudget {
    const fn new(limit: u64) -> Self {
        Self { work: 0, limit }
    }

    fn charge_pair(&mut self) -> Result<(), PolygonizeError> {
        let actual = self.work.checked_add(1).ok_or(PolygonizeError::WorkLimit {
            actual: u64::MAX,
            limit: self.limit,
        })?;
        if actual > self.limit {
            return Err(PolygonizeError::WorkLimit {
                actual,
                limit: self.limit,
            });
        }
        self.work = actual;
        Ok(())
    }
}

impl OutputBudget {
    const fn new(limits: PolygonizeLimits) -> Self {
        Self {
            geometries: 0,
            coordinates: 0,
            limits,
        }
    }

    fn charge(&mut self, coordinates: usize) -> Result<(), PolygonizeError> {
        let next_geometries = self
            .geometries
            .checked_add(1)
            .ok_or(PolygonizeError::IndexOverflow)?;
        if next_geometries > self.limits.max_output_geometries {
            return Err(PolygonizeError::OutputLimit {
                actual: next_geometries,
                limit: self.limits.max_output_geometries,
            });
        }
        let coordinates = u64::try_from(coordinates).map_err(|_| PolygonizeError::IndexOverflow)?;
        let next_coordinates = self
            .coordinates
            .checked_add(coordinates)
            .ok_or(PolygonizeError::IndexOverflow)?;
        if next_coordinates > self.limits.max_output_coordinates {
            return Err(PolygonizeError::OutputLimit {
                actual: next_coordinates,
                limit: self.limits.max_output_coordinates,
            });
        }
        self.geometries = next_geometries;
        self.coordinates = next_coordinates;
        Ok(())
    }

    fn charge_line(&mut self, line: &LineString<f64>) -> Result<(), PolygonizeError> {
        self.charge(line.coords_count())
    }

    fn charge_polygon(&mut self, polygon: &Polygon<f64>) -> Result<(), PolygonizeError> {
        self.charge(polygon.coords_count())
    }
}

#[derive(Clone, Copy, Debug)]
struct Segment {
    start: Coord<f64>,
    end: Coord<f64>,
}

#[derive(Clone, Copy, Debug)]
struct IndexedEnvelope {
    index: usize,
    min_x: f64,
    max_x: f64,
    min_y: f64,
    max_y: f64,
}

type SplitPoints = Vec<Vec<Coord<f64>>>;
type NodedEdges = BTreeMap<EdgeKey, (Coord<f64>, Coord<f64>)>;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct CoordKey {
    x: u64,
    y: u64,
}

impl CoordKey {
    fn new(coord: Coord<f64>) -> Self {
        let x = if coord.x == 0.0 { 0.0 } else { coord.x };
        let y = if coord.y == 0.0 { 0.0 } else { coord.y };
        Self {
            x: x.to_bits(),
            y: y.to_bits(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct EdgeKey(CoordKey, CoordKey);

impl EdgeKey {
    fn new(left: CoordKey, right: CoordKey) -> Option<Self> {
        match left.cmp(&right) {
            std::cmp::Ordering::Equal => None,
            std::cmp::Ordering::Less => Some(Self(left, right)),
            std::cmp::Ordering::Greater => Some(Self(right, left)),
        }
    }

    fn opposite(self, vertex: CoordKey) -> CoordKey {
        if self.0 == vertex {
            self.1
        } else {
            self.0
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

fn collect_lines<'a>(
    geometry: &'a Geometry<f64>,
    lines: &mut Vec<&'a LineString<f64>>,
) -> Result<(), PolygonizeError> {
    match geometry {
        Geometry::LineString(line) => {
            lines
                .try_reserve(1)
                .map_err(|_| PolygonizeError::AllocationFailed("riferimenti alle linee"))?;
            lines.push(line);
            Ok(())
        }
        Geometry::MultiLineString(multi) => {
            for line in &multi.0 {
                lines.push(line);
            }
            Ok(())
        }
        Geometry::GeometryCollection(collection) => {
            for child in &collection.0 {
                collect_lines(child, lines)?;
            }
            Ok(())
        }
        other => Err(PolygonizeError::UnsupportedGeometry(geometry_type(other))),
    }
}

fn segment_count(lines: &[&LineString<f64>]) -> Result<usize, PolygonizeError> {
    lines.iter().try_fold(0_usize, |total, line| {
        total
            .checked_add(line.0.len().saturating_sub(1))
            .ok_or(PolygonizeError::IndexOverflow)
    })
}

fn collect_segments(
    lines: &[&LineString<f64>],
    count: usize,
) -> Result<Vec<Segment>, PolygonizeError> {
    let mut segments = Vec::new();
    segments
        .try_reserve_exact(count)
        .map_err(|_| PolygonizeError::AllocationFailed("segmenti di input"))?;
    for line in lines {
        for segment in line.lines() {
            segments.push(Segment {
                start: segment.start,
                end: segment.end,
            });
        }
    }
    if segments.len() != count {
        return Err(PolygonizeError::InternalInvariant(
            "conteggio dei segmenti cambiato durante la raccolta",
        ));
    }
    Ok(segments)
}

fn deduplicate_segments(segments: Vec<Segment>) -> Vec<Segment> {
    let mut unique = BTreeMap::new();
    for segment in segments {
        let Some(key) = EdgeKey::new(CoordKey::new(segment.start), CoordKey::new(segment.end))
        else {
            continue;
        };
        unique.entry(key).or_insert(segment);
    }
    unique.into_values().collect()
}

fn canonical_line_key(line: &LineString<f64>) -> Vec<CoordKey> {
    let forward = line
        .0
        .iter()
        .copied()
        .map(CoordKey::new)
        .collect::<Vec<_>>();
    let reverse = forward.iter().rev().copied().collect::<Vec<_>>();
    if forward <= reverse {
        forward
    } else {
        reverse
    }
}

fn duplicate_lines_without_noding(
    lines: &[&LineString<f64>],
    node_input: bool,
    output_budget: &mut OutputBudget,
) -> Result<(BTreeSet<EdgeKey>, Vec<LineString<f64>>), PolygonizeError> {
    if node_input {
        return Ok((BTreeSet::new(), Vec::new()));
    }
    let mut groups = BTreeMap::<Vec<CoordKey>, Vec<usize>>::new();
    for (index, line) in lines.iter().enumerate() {
        groups
            .entry(canonical_line_key(line))
            .or_default()
            .push(index);
    }
    let mut excluded_edges = BTreeSet::new();
    let mut cut_lines = Vec::new();
    for indices in groups.values().filter(|indices| indices.len() > 1) {
        for index in indices {
            let line = lines.get(*index).ok_or(PolygonizeError::InternalInvariant(
                "indice della linea duplicata assente",
            ))?;
            output_budget.charge_line(line)?;
            cut_lines
                .try_reserve(1)
                .map_err(|_| PolygonizeError::AllocationFailed("linee duplicate residue"))?;
            cut_lines.push((*line).clone());
            for segment in line.lines() {
                if let Some(edge) =
                    EdgeKey::new(CoordKey::new(segment.start), CoordKey::new(segment.end))
                {
                    excluded_edges.insert(edge);
                }
            }
        }
    }
    Ok((excluded_edges, cut_lines))
}

fn checked_preflight(
    geometry: &Geometry<f64>,
    options: PolygonizeOptions,
) -> Result<(), PolygonizeError> {
    let coordinates =
        u64::try_from(geometry.coords_count()).map_err(|_| PolygonizeError::CoordinateLimit {
            actual: u64::MAX,
            limit: options.limits.max_input_coordinates,
        })?;
    if coordinates > options.limits.max_input_coordinates {
        return Err(PolygonizeError::CoordinateLimit {
            actual: coordinates,
            limit: options.limits.max_input_coordinates,
        });
    }
    Ok(())
}

fn compare_along_segment(
    segment: Segment,
    left: Coord<f64>,
    right: Coord<f64>,
) -> std::cmp::Ordering {
    let dx = segment.end.x - segment.start.x;
    let dy = segment.end.y - segment.start.y;
    let compare_axis = |left_value: f64, right_value: f64, increasing: bool| {
        if increasing {
            left_value.total_cmp(&right_value)
        } else {
            right_value.total_cmp(&left_value)
        }
    };
    if dx.abs() >= dy.abs() {
        let primary = compare_axis(left.x, right.x, dx >= 0.0);
        if primary == std::cmp::Ordering::Equal {
            compare_axis(left.y, right.y, dy >= 0.0)
        } else {
            primary
        }
    } else {
        let primary = compare_axis(left.y, right.y, dy >= 0.0);
        if primary == std::cmp::Ordering::Equal {
            compare_axis(left.x, right.x, dx >= 0.0)
        } else {
            primary
        }
    }
}

fn add_split(
    split_points: &mut [Vec<Coord<f64>>],
    index: usize,
    point: Coord<f64>,
) -> Result<(), PolygonizeError> {
    let points = split_points
        .get_mut(index)
        .ok_or(PolygonizeError::InternalInvariant(
            "segmento assente durante il noding",
        ))?;
    points
        .try_reserve(1)
        .map_err(|_| PolygonizeError::AllocationFailed("punti di noding"))?;
    points.push(point);
    Ok(())
}

fn coordinate_in_segment_envelope(point: Coord<f64>, segment: Segment) -> bool {
    point.x >= segment.start.x.min(segment.end.x)
        && point.x <= segment.start.x.max(segment.end.x)
        && point.y >= segment.start.y.min(segment.end.y)
        && point.y <= segment.start.y.max(segment.end.y)
}

/// Il noding si ripete al piu' [`MAX_NODING_ITERATIONS`] volte, e ogni giro
/// divide i segmenti del giro precedente: gli spostamenti si sommano. Ogni
/// giro ha quindi a disposizione [`NODING_PRECISION_SHARE`] della precisione
/// dichiarata, e il grafo finale dista dal linework d'ingresso al piu' la
/// precisione.
const MAX_NODING_ITERATIONS: usize = 5;

/// Quota della precisione dichiarata concessa a ogni giro di noding.
const NODING_PRECISION_SHARE: f64 = 0.2;

/// Il punto di noding `point` dista dal segmento che divide al piu'
/// `budget`?
///
/// Il punto d'intersezione esatto sta su entrambi i segmenti; quello
/// arrotondato in `f64` puo' uscirne (a `2^52` l'unita' in ultima posizione
/// e' 1, e `(B + 1.5, B + 1.5)` diventa `(B + 2, B + 2)`, a 0.707 unita' da
/// uno dei due segmenti). Dividere il segmento in quel punto sposta il
/// linework di quanto il punto dista dal segmento: e' questa la distanza che
/// si misura, per entrambi i segmenti, e che la precisione dichiarata
/// limita, con il margine d'arrotondamento di
/// [`super::precision::punto_entro_segmento`].
fn noding_point_within(point: Coord<f64>, segment: Segment, budget: f64) -> bool {
    super::precision::punto_entro_segmento(point, segment.start, segment.end, budget)
}

/// Double-double minimo per il noding. La sequenza delle operazioni segue
/// l'aritmetica di Dekker usata dalla famiglia JTS/GEOS, inclusa la sua
/// politica di arrotondamento osservabile.
#[derive(Clone, Copy, Debug)]
struct NodingDouble {
    high: f64,
    low: f64,
}

impl NodingDouble {
    const SPLIT: f64 = 134_217_729.0;

    const fn from_f64(value: f64) -> Self {
        Self {
            high: value,
            low: 0.0,
        }
    }

    fn add(self, other: Self) -> Self {
        let high_sum = self.high + other.high;
        let low_sum = self.low + other.low;
        let high_virtual = high_sum - self.high;
        let low_virtual = low_sum - self.low;
        let high_error = (other.high - high_virtual) + (self.high - (high_sum - high_virtual));
        let low_error = (other.low - low_virtual) + (self.low - (low_sum - low_virtual));
        let carry = high_error + low_sum;
        let normalized_high = high_sum + carry;
        let normalized_low = carry + (high_sum - normalized_high);
        let tail = low_error + normalized_low;
        let high = normalized_high + tail;
        let low = tail + (normalized_high - high);
        Self { high, low }
    }

    fn subtract(self, other: Self) -> Self {
        self.add(Self {
            high: -other.high,
            low: -other.low,
        })
    }

    fn split(value: f64) -> (f64, f64) {
        let scaled = Self::SPLIT * value;
        let high = scaled - (scaled - value);
        (high, value - high)
    }

    fn multiply(self, other: Self) -> Self {
        let (left_high, left_low) = Self::split(self.high);
        let (right_high, right_low) = Self::split(other.high);
        let product = self.high * other.high;
        let product_error = ((((left_high * right_high - product) + left_high * right_low)
            + left_low * right_high)
            + left_low * right_low)
            + (self.high * other.low + self.low * other.high);
        let high = product + product_error;
        let low = product_error + (product - high);
        Self { high, low }
    }

    fn quotient(self, denominator: Self) -> Option<Self> {
        if denominator.high == 0.0 || !denominator.high.is_finite() {
            return None;
        }
        let estimate = self.high / denominator.high;
        let (estimate_high, estimate_low) = Self::split(estimate);
        let (denominator_high, denominator_low) = Self::split(denominator.high);
        let product = estimate * denominator.high;
        let product_error = (((estimate_high * denominator_high - product)
            + estimate_high * denominator_low)
            + estimate_low * denominator_high)
            + estimate_low * denominator_low;
        let correction = ((((self.high - product) - product_error) + self.low)
            - estimate * denominator.low)
            / denominator.high;
        let high = estimate + correction;
        let low = (estimate - high) + correction;
        let result = Self { high, low };
        result.is_finite().then_some(result)
    }

    fn is_zero(self) -> bool {
        self.high == 0.0 && self.low == 0.0
    }

    const fn is_finite(self) -> bool {
        self.high.is_finite() && self.low.is_finite()
    }

    fn to_f64(self) -> f64 {
        self.high + self.low
    }
}

fn extended_precision_intersection(left: Segment, right: Segment) -> Option<Coord<f64>> {
    let left_start_x = NodingDouble::from_f64(left.start.x);
    let left_start_y = NodingDouble::from_f64(left.start.y);
    let left_end_x = NodingDouble::from_f64(left.end.x);
    let left_end_y = NodingDouble::from_f64(left.end.y);
    let right_start_x = NodingDouble::from_f64(right.start.x);
    let right_start_y = NodingDouble::from_f64(right.start.y);
    let right_end_x = NodingDouble::from_f64(right.end.x);
    let right_end_y = NodingDouble::from_f64(right.end.y);

    let left_x = left_start_y.subtract(left_end_y);
    let left_y = left_end_x.subtract(left_start_x);
    let left_w = left_start_x
        .multiply(left_end_y)
        .subtract(left_end_x.multiply(left_start_y));
    let right_x = right_start_y.subtract(right_end_y);
    let right_y = right_end_x.subtract(right_start_x);
    let right_w = right_start_x
        .multiply(right_end_y)
        .subtract(right_end_x.multiply(right_start_y));

    let x_weighted = left_y.multiply(right_w).subtract(right_y.multiply(left_w));
    let y_weighted = right_x.multiply(left_w).subtract(left_x.multiply(right_w));
    let weight = left_x.multiply(right_y).subtract(right_x.multiply(left_y));
    if !weight.is_finite() || weight.is_zero() {
        return None;
    }
    let point = Coord {
        x: x_weighted.quotient(weight)?.to_f64(),
        y: y_weighted.quotient(weight)?.to_f64(),
    };
    if point.x.is_finite()
        && point.y.is_finite()
        && coordinate_in_segment_envelope(point, left)
        && coordinate_in_segment_envelope(point, right)
    {
        Some(point)
    } else {
        None
    }
}

fn robust_line_intersection(left: Segment, right: Segment) -> Option<LineIntersection<f64>> {
    let intersection = line_intersection(
        Line::new(left.start, left.end),
        Line::new(right.start, right.end),
    )?;
    match intersection {
        LineIntersection::SinglePoint {
            intersection: fallback,
            is_proper: true,
        } => Some(LineIntersection::SinglePoint {
            intersection: extended_precision_intersection(left, right).unwrap_or(fallback),
            is_proper: true,
        }),
        other => Some(other),
    }
}

fn indexed_envelope(index: usize, segment: Segment) -> Option<IndexedEnvelope> {
    let coordinates = [
        segment.start.x,
        segment.start.y,
        segment.end.x,
        segment.end.y,
    ];
    if coordinates.iter().any(|value| !value.is_finite()) {
        return None;
    }
    Some(IndexedEnvelope {
        index,
        min_x: segment.start.x.min(segment.end.x),
        max_x: segment.start.x.max(segment.end.x),
        min_y: segment.start.y.min(segment.end.y),
        max_y: segment.start.y.max(segment.end.y),
    })
}

/// Visita soltanto coppie i cui inviluppi possono intersecarsi. Ogni coppia
/// che supera il filtro sull'asse X consuma budget, compresi i successivi
/// scarti sull'asse Y: il limite continua quindi a coprire tutto il lavoro
/// potenzialmente quadratico del ciclo interno. Coordinate non finite fanno
/// ricadere nel confronto esaustivo, senza consentire al filtro di nascondere
/// una coppia al kernel robusto.
fn visit_candidate_pairs(
    segments: &[Segment],
    budget: &mut NodingBudget,
    mut visit: impl FnMut(usize, usize) -> Result<bool, PolygonizeError>,
) -> Result<bool, PolygonizeError> {
    let mut envelopes = Vec::new();
    envelopes
        .try_reserve_exact(segments.len())
        .map_err(|_| PolygonizeError::AllocationFailed("indice spaziale del noding"))?;
    for (index, segment) in segments.iter().copied().enumerate() {
        let Some(envelope) = indexed_envelope(index, segment) else {
            for left_index in 0..segments.len() {
                for right_index in left_index.saturating_add(1)..segments.len() {
                    budget.charge_pair()?;
                    if !visit(left_index, right_index)? {
                        return Ok(false);
                    }
                }
            }
            return Ok(true);
        };
        envelopes.push(envelope);
    }
    envelopes.sort_by(|left, right| {
        left.min_x
            .total_cmp(&right.min_x)
            .then_with(|| left.max_x.total_cmp(&right.max_x))
            .then_with(|| left.index.cmp(&right.index))
    });
    for left_position in 0..envelopes.len() {
        let left = envelopes[left_position];
        for right in envelopes.iter().skip(left_position.saturating_add(1)) {
            if right.min_x > left.max_x {
                break;
            }
            budget.charge_pair()?;
            if right.max_y < left.min_y || right.min_y > left.max_y {
                continue;
            }
            if !visit(left.index, right.index)? {
                return Ok(false);
            }
        }
    }
    Ok(true)
}

fn node_segments(
    segments: &[Segment],
    budget: &mut NodingBudget,
    displacement: f64,
    rounded: &mut BTreeSet<CoordKey>,
) -> Result<(SplitPoints, usize), PolygonizeError> {
    let mut split_points = Vec::new();
    split_points
        .try_reserve_exact(segments.len())
        .map_err(|_| PolygonizeError::AllocationFailed("segmenti da nodare"))?;
    for segment in segments {
        let mut points = Vec::new();
        points
            .try_reserve_exact(2)
            .map_err(|_| PolygonizeError::AllocationFailed("estremi dei segmenti"))?;
        points.push(segment.start);
        points.push(segment.end);
        split_points.push(points);
    }
    let mut interior_intersections = 0_usize;
    let completed = visit_candidate_pairs(segments, budget, |left_index, right_index| {
        let left = segments[left_index];
        let right = segments[right_index];
        match robust_line_intersection(left, right) {
            Some(LineIntersection::SinglePoint { intersection, .. }) => {
                if !noding_point_within(intersection, left, displacement)
                    || !noding_point_within(intersection, right, displacement)
                {
                    return Err(PolygonizeError::PrecisionInsufficient);
                }
                let intersection_key = CoordKey::new(intersection);
                // Un punto d'incrocio che non sta esattamente su entrambi i
                // segmenti e' arrotondato: lo si ricorda per chi deve sapere
                // dove il grafo non e' esatto (`make_valid` `LINEWORK`).
                if RobustKernel::orient2d(left.start, left.end, intersection)
                    != Orientation::Collinear
                    || RobustKernel::orient2d(right.start, right.end, intersection)
                        != Orientation::Collinear
                {
                    rounded.insert(intersection_key);
                }
                if intersection_key != CoordKey::new(left.start)
                    && intersection_key != CoordKey::new(left.end)
                {
                    interior_intersections = interior_intersections
                        .checked_add(1)
                        .ok_or(PolygonizeError::IndexOverflow)?;
                }
                if intersection_key != CoordKey::new(right.start)
                    && intersection_key != CoordKey::new(right.end)
                {
                    interior_intersections = interior_intersections
                        .checked_add(1)
                        .ok_or(PolygonizeError::IndexOverflow)?;
                }
                add_split(&mut split_points, left_index, intersection)?;
                add_split(&mut split_points, right_index, intersection)?;
            }
            Some(LineIntersection::Collinear { intersection }) => {
                for point in [intersection.start, intersection.end] {
                    if !noding_point_within(point, left, displacement)
                        || !noding_point_within(point, right, displacement)
                    {
                        return Err(PolygonizeError::PrecisionInsufficient);
                    }
                    let point_key = CoordKey::new(point);
                    if point_key != CoordKey::new(left.start)
                        && point_key != CoordKey::new(left.end)
                    {
                        interior_intersections = interior_intersections
                            .checked_add(1)
                            .ok_or(PolygonizeError::IndexOverflow)?;
                    }
                    if point_key != CoordKey::new(right.start)
                        && point_key != CoordKey::new(right.end)
                    {
                        interior_intersections = interior_intersections
                            .checked_add(1)
                            .ok_or(PolygonizeError::IndexOverflow)?;
                    }
                    add_split(&mut split_points, left_index, point)?;
                    add_split(&mut split_points, right_index, point)?;
                }
            }
            None => {}
        }
        Ok(true)
    })?;
    if !completed {
        return Err(PolygonizeError::InternalInvariant(
            "visita del noding interrotta senza richiesta",
        ));
    }
    Ok((split_points, interior_intersections))
}

fn node_edges_iteratively(
    segments: &[Segment],
    work_limit: u64,
    precision: f64,
    rounded: &mut BTreeSet<CoordKey>,
) -> Result<NodedEdges, PolygonizeError> {
    const MAX_ITERATIONS: usize = MAX_NODING_ITERATIONS;

    let displacement = precision * NODING_PRECISION_SHARE;
    let mut budget = NodingBudget::new(work_limit);
    let mut current = Vec::new();
    current
        .try_reserve_exact(segments.len())
        .map_err(|_| PolygonizeError::AllocationFailed("segmenti del noding iterativo"))?;
    current.extend_from_slice(segments);
    let mut last_intersections = None;
    let mut iteration = 0_usize;
    loop {
        iteration = iteration
            .checked_add(1)
            .ok_or(PolygonizeError::IndexOverflow)?;
        let (split_points, interior_intersections) =
            node_segments(&current, &mut budget, displacement, rounded)?;
        let edges = build_edges(&current, split_points)?;
        if edges_are_fully_noded(&edges, &mut budget)? {
            return Ok(edges);
        }
        if interior_intersections == 0 {
            let previous = last_intersections.unwrap_or(interior_intersections);
            return Err(PolygonizeError::NodingDidNotConverge {
                iterations: iteration,
                previous,
                current: interior_intersections,
            });
        }
        if iteration >= MAX_ITERATIONS {
            let previous = last_intersections.unwrap_or(interior_intersections);
            return Err(PolygonizeError::NodingDidNotConverge {
                iterations: iteration,
                previous,
                current: interior_intersections,
            });
        }
        let mut next = Vec::new();
        next.try_reserve_exact(edges.len())
            .map_err(|_| PolygonizeError::AllocationFailed("archi del noding iterativo"))?;
        for (start, end) in edges.values().copied() {
            next.push(Segment { start, end });
        }
        current = next;
        last_intersections = Some(interior_intersections);
    }
}

fn edges_are_fully_noded(
    edges: &BTreeMap<EdgeKey, (Coord<f64>, Coord<f64>)>,
    budget: &mut NodingBudget,
) -> Result<bool, PolygonizeError> {
    let mut segments = Vec::new();
    segments
        .try_reserve_exact(edges.len())
        .map_err(|_| PolygonizeError::AllocationFailed("archi da verificare"))?;
    for (start, end) in edges.values().copied() {
        segments.push(Segment { start, end });
    }
    visit_candidate_pairs(&segments, budget, |left_index, right_index| {
        let left = segments[left_index];
        let right = segments[right_index];
        match robust_line_intersection(left, right) {
            Some(LineIntersection::SinglePoint { intersection, .. }) => {
                let key = CoordKey::new(intersection);
                let left_endpoint =
                    key == CoordKey::new(left.start) || key == CoordKey::new(left.end);
                let right_endpoint =
                    key == CoordKey::new(right.start) || key == CoordKey::new(right.end);
                if !left_endpoint || !right_endpoint {
                    return Ok(false);
                }
            }
            Some(LineIntersection::Collinear { .. }) => return Ok(false),
            None => {}
        }
        Ok(true)
    })
}

fn build_edges(
    segments: &[Segment],
    mut split_points: SplitPoints,
) -> Result<NodedEdges, PolygonizeError> {
    let mut edges = BTreeMap::new();
    for (index, points) in split_points.iter_mut().enumerate() {
        let segment = segments
            .get(index)
            .copied()
            .ok_or(PolygonizeError::InternalInvariant(
                "segmento assente durante la costruzione degli archi",
            ))?;
        points.sort_by(|left, right| compare_along_segment(segment, *left, *right));
        points.dedup_by(|left, right| CoordKey::new(*left) == CoordKey::new(*right));
        for pair in points.windows(2) {
            let [start, end] = pair else {
                return Err(PolygonizeError::InternalInvariant(
                    "finestra di noding senza due coordinate",
                ));
            };
            let start = *start;
            let end = *end;
            let start_key = CoordKey::new(start);
            let end_key = CoordKey::new(end);
            if let Some(key) = EdgeKey::new(start_key, end_key) {
                let ordered_coordinates = if key.0 == start_key {
                    (start, end)
                } else {
                    (end, start)
                };
                edges.entry(key).or_insert(ordered_coordinates);
            }
        }
    }
    Ok(edges)
}

fn build_unnoded_edges(segments: &[Segment]) -> NodedEdges {
    let mut edges = BTreeMap::new();
    for segment in segments {
        let start_key = CoordKey::new(segment.start);
        let end_key = CoordKey::new(segment.end);
        if let Some(key) = EdgeKey::new(start_key, end_key) {
            let ordered_coordinates = if key.0 == start_key {
                (segment.start, segment.end)
            } else {
                (segment.end, segment.start)
            };
            edges.entry(key).or_insert(ordered_coordinates);
        }
    }
    edges
}

fn adjacency(edges: &BTreeSet<EdgeKey>) -> BTreeMap<CoordKey, Vec<EdgeKey>> {
    let mut graph = BTreeMap::<CoordKey, Vec<EdgeKey>>::new();
    for edge in edges {
        graph.entry(edge.0).or_default().push(*edge);
        graph.entry(edge.1).or_default().push(*edge);
    }
    graph
}

fn remove_dangles(active: &mut BTreeSet<EdgeKey>) -> BTreeSet<EdgeKey> {
    let mut removed = BTreeSet::new();
    loop {
        let graph = adjacency(active);
        let leaves = graph
            .iter()
            .filter(|(_, edges)| edges.len() <= 1)
            .map(|(vertex, _)| *vertex)
            .collect::<VecDeque<_>>();
        if leaves.is_empty() {
            break;
        }
        let mut changed = false;
        for vertex in leaves {
            let Some(edge) = graph.get(&vertex).and_then(|edges| edges.first()).copied() else {
                continue;
            };
            if active.remove(&edge) {
                removed.insert(edge);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    removed
}

fn outgoing_neighbors(
    active: &BTreeSet<EdgeKey>,
    coordinates: &BTreeMap<CoordKey, Coord<f64>>,
) -> Result<BTreeMap<CoordKey, Vec<CoordKey>>, PolygonizeError> {
    let mut output = BTreeMap::<CoordKey, Vec<CoordKey>>::new();
    for edge in active {
        output.entry(edge.0).or_default().push(edge.1);
        output.entry(edge.1).or_default().push(edge.0);
    }
    for (vertex, neighbors) in &mut output {
        let origin = coordinates
            .get(vertex)
            .copied()
            .ok_or(PolygonizeError::InternalInvariant(
                "coordinata del vertice del grafo assente",
            ))?;
        let mut ordered = Vec::new();
        ordered
            .try_reserve_exact(neighbors.len())
            .map_err(|_| PolygonizeError::AllocationFailed("vicini ordinati del grafo"))?;
        for neighbor in neighbors.iter().copied() {
            let coordinate =
                coordinates
                    .get(&neighbor)
                    .copied()
                    .ok_or(PolygonizeError::InternalInvariant(
                        "coordinata del vicino del grafo assente",
                    ))?;
            ordered.push((neighbor, coordinate));
        }
        ordered.sort_by(|left, right| {
            let left_dx = left.1.x - origin.x;
            let left_dy = left.1.y - origin.y;
            let right_dx = right.1.x - origin.x;
            let right_dy = right.1.y - origin.y;
            let left_half = left_dy < 0.0 || (left_dy == 0.0 && left_dx < 0.0);
            let right_half = right_dy < 0.0 || (right_dy == 0.0 && right_dx < 0.0);
            left_half.cmp(&right_half).then_with(|| {
                match RobustKernel::orient2d(origin, left.1, right.1) {
                    Orientation::CounterClockwise => std::cmp::Ordering::Less,
                    Orientation::Clockwise => std::cmp::Ordering::Greater,
                    Orientation::Collinear => left_dx
                        .hypot(left_dy)
                        .total_cmp(&right_dx.hypot(right_dy))
                        .then_with(|| left.0.cmp(&right.0)),
                }
            })
        });
        neighbors.clear();
        neighbors.extend(ordered.into_iter().map(|(neighbor, _)| neighbor));
    }
    Ok(output)
}

fn next_half_edge(
    from: CoordKey,
    at: CoordKey,
    neighbors: &BTreeMap<CoordKey, Vec<CoordKey>>,
) -> Result<(CoordKey, CoordKey), PolygonizeError> {
    let outgoing = neighbors
        .get(&at)
        .ok_or(PolygonizeError::InternalInvariant(
            "vertice di arrivo assente dal grafo",
        ))?;
    let incoming_index = outgoing
        .iter()
        .position(|candidate| *candidate == from)
        .ok_or(PolygonizeError::InternalInvariant(
            "mezzo arco entrante assente dai vicini",
        ))?;
    let next_index = if incoming_index == 0 {
        outgoing
            .len()
            .checked_sub(1)
            .ok_or(PolygonizeError::InternalInvariant(
                "vertice senza archi uscenti",
            ))?
    } else {
        incoming_index - 1
    };
    outgoing
        .get(next_index)
        .copied()
        .map(|next| (at, next))
        .ok_or(PolygonizeError::InternalInvariant(
            "indice del mezzo arco successivo assente",
        ))
}

/// Orientamento esatto dell'anello chiuso (`Greater` antiorario, `Equal`
/// solo se degenere). Il laboratorio usava il segno di una somma di Gauss in
/// `f64`, che su coordinate grandi si annulla per cancellazione: deviazione
/// dichiarata in `super`.
fn ring_orientation(ring: &[Coord<f64>]) -> Result<Ordering, PolygonizeError> {
    exact::orientamento(ring).map_err(|_| PolygonizeError::NumericRange)
}

/// Aree (senza segno) delle facce, confrontate in modo esatto.
struct FaceAreas<'a> {
    polygons: &'a [Polygon<f64>],
    approximations: Vec<AreaPoligono>,
}

impl<'a> FaceAreas<'a> {
    fn new(polygons: &'a [Polygon<f64>]) -> Self {
        Self {
            polygons,
            approximations: polygons.iter().map(AreaPoligono::di).collect(),
        }
    }

    fn cmp(&self, left: usize, right: usize) -> Result<Ordering, PolygonizeError> {
        let (Some(left_polygon), Some(left_area), Some(right_polygon), Some(right_area)) = (
            self.polygons.get(left),
            self.approximations.get(left),
            self.polygons.get(right),
            self.approximations.get(right),
        ) else {
            return Err(PolygonizeError::InternalInvariant(
                "faccia assente nel confronto delle aree",
            ));
        };
        exact::confronta_aree(left_polygon, *left_area, right_polygon, *right_area)
            .map_err(|_| PolygonizeError::NumericRange)
    }

    /// Almeno due aree diverse: senza, nessuna faccia puo' contenerne
    /// un'altra.
    fn distinct(&self) -> Result<bool, PolygonizeError> {
        for index in 1..self.polygons.len() {
            if self.cmp(0, index)? != Ordering::Equal {
                return Ok(true);
            }
        }
        Ok(false)
    }
}

fn decompose_closed_walk(mut keys: Vec<CoordKey>) -> Vec<Vec<CoordKey>> {
    let mut cycles = Vec::new();
    loop {
        let mut positions = BTreeMap::new();
        let mut repeated = None;
        for (index, key) in keys.iter().take(keys.len().saturating_sub(1)).enumerate() {
            if let Some(previous) = positions.insert(*key, index) {
                repeated = Some((previous, index));
                break;
            }
        }
        let Some((start, end)) = repeated else {
            break;
        };
        cycles.push(keys[start..=end].to_vec());
        let mut remainder = Vec::with_capacity(keys.len().saturating_sub(end - start));
        remainder.extend_from_slice(&keys[..=start]);
        remainder.extend_from_slice(&keys[end + 1..]);
        keys = remainder;
    }
    if keys.len() >= 4 {
        cycles.push(keys);
    }
    cycles
}

struct Faces {
    polygons: Vec<Polygon<f64>>,
    polygon_edges: BTreeSet<EdgeKey>,
    invalid_rings: Vec<LineString<f64>>,
}

fn validate_polygonized_face(face: &Polygon<f64>, context: &str) -> Result<(), PolygonizeError> {
    let errors = face.validation_errors();
    if errors.is_empty()
        || errors.iter().all(|error| {
            matches!(
                error,
                InvalidPolygon::InteriorRingNotContainedInExteriorRing(_)
            )
        })
    {
        return Ok(());
    }
    Err(PolygonizeError::InvalidOutput(format!(
        "{context}: {}",
        errors
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("; ")
    )))
}

fn extract_faces(
    active: &BTreeSet<EdgeKey>,
    coordinates: &BTreeMap<CoordKey, Coord<f64>>,
    limits: PolygonizeLimits,
    output_budget: &mut OutputBudget,
) -> Result<Faces, PolygonizeError> {
    let neighbors = outgoing_neighbors(active, coordinates)?;
    let mut visited = BTreeSet::<(CoordKey, CoordKey)>::new();
    let mut polygons = Vec::new();
    let mut intermediate_budget = OutputBudget::new(limits);
    let mut polygon_edges = BTreeSet::new();
    let mut invalid_rings = Vec::new();
    for edge in active {
        for start in [(edge.0, edge.1), (edge.1, edge.0)] {
            if visited.contains(&start) {
                continue;
            }
            let mut half_edge = start;
            let mut keys = vec![start.0];
            let max_steps = active
                .len()
                .checked_mul(2)
                .and_then(|value| value.checked_add(1))
                .ok_or(PolygonizeError::IndexOverflow)?;
            let mut closed = false;
            for _ in 0..max_steps {
                if !visited.insert(half_edge) && half_edge != start {
                    break;
                }
                keys.push(half_edge.1);
                let next = next_half_edge(half_edge.0, half_edge.1, &neighbors)?;
                if next == start {
                    closed = true;
                    break;
                }
                half_edge = next;
            }
            if !closed {
                return Err(PolygonizeError::InternalInvariant(
                    "cammino di mezzi archi non chiuso",
                ));
            }
            for cycle in decompose_closed_walk(keys) {
                let ring_coordinates =
                    u64::try_from(cycle.len()).map_err(|_| PolygonizeError::IndexOverflow)?;
                if ring_coordinates > limits.max_output_coordinates {
                    return Err(PolygonizeError::OutputLimit {
                        actual: ring_coordinates,
                        limit: limits.max_output_coordinates,
                    });
                }
                let mut ring = Vec::new();
                ring.try_reserve_exact(cycle.len())
                    .map_err(|_| PolygonizeError::AllocationFailed("coordinate della faccia"))?;
                for key in &cycle {
                    ring.push(coordinates.get(key).copied().ok_or(
                        PolygonizeError::InternalInvariant(
                            "coordinata dell'anello estratto assente",
                        ),
                    )?);
                }
                let edge_count = cycle.len().saturating_sub(1);
                let mut cycle_edges = Vec::new();
                cycle_edges.try_reserve_exact(edge_count).map_err(|_| {
                    PolygonizeError::AllocationFailed("archi della faccia estratta")
                })?;
                for pair in cycle.windows(2) {
                    cycle_edges.push(EdgeKey::new(pair[0], pair[1]).ok_or(
                        PolygonizeError::InternalInvariant("arco degenere nell'anello estratto"),
                    )?);
                }
                if ring.len() < 4 {
                    continue;
                }
                let orientation = ring_orientation(&ring)?;
                if orientation == Ordering::Greater {
                    let polygon = Polygon::new(LineString::new(ring), Vec::new());
                    if polygon.check_validation().is_ok() {
                        intermediate_budget.charge_polygon(&polygon)?;
                        polygons.try_reserve(1).map_err(|_| {
                            PolygonizeError::AllocationFailed("facce poligonali intermedie")
                        })?;
                        polygon_edges.extend(cycle_edges);
                        polygons.push(polygon);
                    } else if cycle_edges.iter().any(|edge| !polygon_edges.contains(edge)) {
                        output_budget.charge_line(polygon.exterior())?;
                        invalid_rings.try_reserve(1).map_err(|_| {
                            PolygonizeError::AllocationFailed("anelli invalidi residui")
                        })?;
                        polygon_edges.extend(&cycle_edges);
                        invalid_rings.push(polygon.exterior().clone());
                    }
                } else if orientation == Ordering::Equal
                    && cycle_edges.iter().any(|edge| !polygon_edges.contains(edge))
                {
                    let invalid = LineString::new(ring);
                    output_budget.charge_line(&invalid)?;
                    invalid_rings.try_reserve(1).map_err(|_| {
                        PolygonizeError::AllocationFailed("anelli invalidi residui")
                    })?;
                    polygon_edges.extend(&cycle_edges);
                    invalid_rings.push(invalid);
                }
            }
        }
    }
    drop(neighbors);
    drop(visited);
    let polygons = assemble_face_holes(polygons, limits, output_budget)?;
    Ok(Faces {
        polygons,
        polygon_edges,
        invalid_rings,
    })
}

fn union_boundary_rings(
    children: &[&Polygon<f64>],
    limits: PolygonizeLimits,
) -> Result<Vec<LineString<f64>>, PolygonizeError> {
    let mut counts = BTreeMap::<EdgeKey, u64>::new();
    let mut coordinates = BTreeMap::<CoordKey, Coord<f64>>::new();
    // I lati di ciascun figlio: un lato condiviso con un altro figlio non si
    // scarta mai per il punto medio (vedi sotto).
    let child_edges = children
        .iter()
        .map(|child| {
            child
                .exterior()
                .lines()
                .filter_map(|segment| {
                    EdgeKey::new(CoordKey::new(segment.start), CoordKey::new(segment.end))
                })
                .collect::<BTreeSet<_>>()
        })
        .collect::<Vec<_>>();
    for (child_index, child) in children.iter().enumerate() {
        for segment in child.exterior().lines() {
            let start = CoordKey::new(segment.start);
            let end = CoordKey::new(segment.end);
            let Some(edge) = EdgeKey::new(start, end) else {
                continue;
            };
            let midpoint = Point::new(
                segment.start.x + (segment.end.x - segment.start.x) * 0.5,
                segment.start.y + (segment.end.y - segment.start.y) * 0.5,
            );
            // Deviazione dal laboratorio: il punto medio in `f64` di un lato
            // obliquo condiviso da due figli cade dentro uno dei due, e il
            // lato, contato una volta sola, restava come bordo pendente (buco
            // aperto). Un lato che e' anche lato dell'altro figlio e' bordo
            // comune, non interno.
            if children.iter().enumerate().any(|(other_index, other)| {
                other_index != child_index
                    && !child_edges[other_index].contains(&edge)
                    && other.contains(&midpoint)
            }) {
                continue;
            }
            coordinates.entry(start).or_insert(segment.start);
            coordinates.entry(end).or_insert(segment.end);
            let count = counts.entry(edge).or_insert(0);
            *count = count.checked_add(1).ok_or(PolygonizeError::IndexOverflow)?;
        }
    }
    let edges = counts
        .into_iter()
        .filter(|(_, count)| *count == 1)
        .map(|(edge, _)| edge)
        .collect::<BTreeSet<_>>();
    let mut budget = OutputBudget::new(limits);
    let mut rings = Vec::new();
    let mut open = Vec::new();
    for chain in edge_chains(&edges, &coordinates, &mut budget)? {
        if chain.0.first() == chain.0.last() {
            if chain.0.len() >= 4 {
                rings.push(chain);
            }
        } else {
            open.push(
                chain
                    .0
                    .iter()
                    .copied()
                    .map(CoordKey::new)
                    .collect::<Vec<_>>(),
            );
        }
    }
    // Un anello di bordo che tocca il resto del bordo in due o piu' vertici
    // esce da `edge_chains` spezzato in catene aperte. Il laboratorio le
    // scartava, e la faccia perdeva il buco in silenzio (e copriva i figli).
    // Deviazione: le catene si ricuciono in cammini chiusi per estremi comuni
    // e si scompongono in anelli semplici ai vertici ripetuti; una catena che
    // non si chiude e' un errore. Una scomposizione che desse buchi
    // sovrapposti e' rifiutata dalla validazione della faccia.
    while let Some(first) = open.pop() {
        let mut walk = first;
        loop {
            let (Some(start), Some(end)) = (walk.first().copied(), walk.last().copied()) else {
                return Err(PolygonizeError::InternalInvariant("catena di bordo vuota"));
            };
            if start == end {
                break;
            }
            let Some(index) = open
                .iter()
                .position(|chain| chain.first() == Some(&end) || chain.last() == Some(&end))
            else {
                return Err(PolygonizeError::InvalidOutput(
                    "bordo dei buchi non chiuso".to_owned(),
                ));
            };
            let mut next = open.swap_remove(index);
            if next.first() != Some(&end) {
                next.reverse();
            }
            walk.extend(next.into_iter().skip(1));
        }
        for cycle in decompose_closed_walk(walk) {
            if cycle.len() < 4 {
                continue;
            }
            let mut ring = Vec::new();
            ring.try_reserve_exact(cycle.len())
                .map_err(|_| PolygonizeError::AllocationFailed("anelli di bordo ricuciti"))?;
            for key in &cycle {
                ring.push(coordinates.get(key).copied().ok_or(
                    PolygonizeError::InternalInvariant("coordinata del bordo ricucito assente"),
                )?);
            }
            rings.push(LineString::new(ring));
        }
    }
    Ok(rings)
}

const FACE_INDEX_LEAF_SIZE: usize = 16;
const NO_FACE: usize = usize::MAX;

#[derive(Clone, Copy, Debug)]
struct FaceBounds {
    min_x: f64,
    min_y: f64,
    max_x: f64,
    max_y: f64,
}

impl FaceBounds {
    fn from_polygon(polygon: &Polygon<f64>) -> Result<Self, PolygonizeError> {
        let mut coordinates = polygon.exterior().0.iter().copied();
        let first = coordinates
            .next()
            .ok_or(PolygonizeError::InternalInvariant(
                "faccia senza coordinate per l'indice spaziale",
            ))?;
        let mut bounds = Self {
            min_x: first.x,
            min_y: first.y,
            max_x: first.x,
            max_y: first.y,
        };
        for coordinate in coordinates {
            bounds.min_x = bounds.min_x.min(coordinate.x);
            bounds.min_y = bounds.min_y.min(coordinate.y);
            bounds.max_x = bounds.max_x.max(coordinate.x);
            bounds.max_y = bounds.max_y.max(coordinate.y);
        }
        Ok(bounds)
    }

    const fn union(self, other: Self) -> Self {
        Self {
            min_x: self.min_x.min(other.min_x),
            min_y: self.min_y.min(other.min_y),
            max_x: self.max_x.max(other.max_x),
            max_y: self.max_y.max(other.max_y),
        }
    }

    fn contains_point(self, point: Point<f64>) -> bool {
        point.x() >= self.min_x
            && point.x() <= self.max_x
            && point.y() >= self.min_y
            && point.y() <= self.max_y
    }

    fn contains_bounds(self, other: Self) -> bool {
        other.min_x >= self.min_x
            && other.max_x <= self.max_x
            && other.min_y >= self.min_y
            && other.max_y <= self.max_y
    }

    fn intersects(self, other: Self) -> bool {
        self.min_x <= other.max_x
            && self.max_x >= other.min_x
            && self.min_y <= other.max_y
            && self.max_y >= other.min_y
    }

    fn center(self, axis: usize) -> f64 {
        if axis == 0 {
            self.min_x + (self.max_x - self.min_x) * 0.5
        } else {
            self.min_y + (self.max_y - self.min_y) * 0.5
        }
    }

    fn split_axis(self) -> usize {
        if self.max_x - self.min_x >= self.max_y - self.min_y {
            0
        } else {
            1
        }
    }
}

#[derive(Debug)]
enum FaceIndexNode {
    Leaf {
        bounds: FaceBounds,
        start: usize,
        end: usize,
    },
    Branch {
        bounds: FaceBounds,
        left: usize,
        right: usize,
    },
}

impl FaceIndexNode {
    const fn bounds(&self) -> FaceBounds {
        match self {
            Self::Leaf { bounds, .. } | Self::Branch { bounds, .. } => *bounds,
        }
    }
}

#[derive(Debug)]
struct FaceSpatialIndex {
    face_bounds: Vec<FaceBounds>,
    order: Vec<usize>,
    nodes: Vec<FaceIndexNode>,
    root: Option<usize>,
}

impl FaceSpatialIndex {
    fn build(polygons: &[Polygon<f64>]) -> Result<Self, PolygonizeError> {
        let mut face_bounds = Vec::new();
        face_bounds
            .try_reserve_exact(polygons.len())
            .map_err(|_| PolygonizeError::AllocationFailed("envelope delle facce"))?;
        for polygon in polygons {
            face_bounds.push(FaceBounds::from_polygon(polygon)?);
        }
        let mut order = Vec::new();
        order
            .try_reserve_exact(polygons.len())
            .map_err(|_| PolygonizeError::AllocationFailed("ordine dell'indice delle facce"))?;
        order.extend(0..polygons.len());
        let leaves = polygons
            .len()
            .checked_add(FACE_INDEX_LEAF_SIZE.saturating_sub(1))
            .ok_or(PolygonizeError::IndexOverflow)?
            / FACE_INDEX_LEAF_SIZE;
        let node_capacity = leaves
            .checked_mul(2)
            .and_then(|value| value.checked_add(1))
            .ok_or(PolygonizeError::IndexOverflow)?;
        let mut nodes = Vec::new();
        nodes
            .try_reserve_exact(node_capacity)
            .map_err(|_| PolygonizeError::AllocationFailed("nodi dell'indice delle facce"))?;
        let root = if order.is_empty() {
            None
        } else {
            Some(Self::build_node(&mut order, 0, &face_bounds, &mut nodes)?)
        };
        Ok(Self {
            face_bounds,
            order,
            nodes,
            root,
        })
    }

    fn build_node(
        order: &mut [usize],
        offset: usize,
        face_bounds: &[FaceBounds],
        nodes: &mut Vec<FaceIndexNode>,
    ) -> Result<usize, PolygonizeError> {
        let first_index = *order.first().ok_or(PolygonizeError::InternalInvariant(
            "partizione vuota nell'indice delle facce",
        ))?;
        let mut bounds = face_bounds[first_index];
        for index in order.iter().copied().skip(1) {
            bounds = bounds.union(face_bounds[index]);
        }
        if order.len() <= FACE_INDEX_LEAF_SIZE {
            let end = offset
                .checked_add(order.len())
                .ok_or(PolygonizeError::IndexOverflow)?;
            let index = nodes.len();
            nodes.push(FaceIndexNode::Leaf {
                bounds,
                start: offset,
                end,
            });
            return Ok(index);
        }
        let axis = bounds.split_axis();
        let middle = order.len() / 2;
        order.select_nth_unstable_by(middle, |left, right| {
            face_bounds[*left]
                .center(axis)
                .total_cmp(&face_bounds[*right].center(axis))
                .then_with(|| left.cmp(right))
        });
        let (left_order, right_order) = order.split_at_mut(middle);
        let left = Self::build_node(left_order, offset, face_bounds, nodes)?;
        let right_offset = offset
            .checked_add(middle)
            .ok_or(PolygonizeError::IndexOverflow)?;
        let right = Self::build_node(right_order, right_offset, face_bounds, nodes)?;
        let index = nodes.len();
        nodes.push(FaceIndexNode::Branch {
            bounds,
            left,
            right,
        });
        Ok(index)
    }

    fn for_each_containing_point(&self, point: Point<f64>, mut visit: impl FnMut(usize)) {
        if let Some(root) = self.root {
            self.visit_containing_point(root, point, &mut visit);
        }
    }

    fn visit_containing_point(
        &self,
        node_index: usize,
        point: Point<f64>,
        visit: &mut impl FnMut(usize),
    ) {
        let node = &self.nodes[node_index];
        if !node.bounds().contains_point(point) {
            return;
        }
        match node {
            FaceIndexNode::Leaf { start, end, .. } => {
                for face_index in self.order[*start..*end].iter().copied() {
                    if self.face_bounds[face_index].contains_point(point) {
                        visit(face_index);
                    }
                }
            }
            FaceIndexNode::Branch { left, right, .. } => {
                self.visit_containing_point(*left, point, visit);
                self.visit_containing_point(*right, point, visit);
            }
        }
    }

    fn for_each_within(&self, query: FaceBounds, mut visit: impl FnMut(usize)) {
        if let Some(root) = self.root {
            self.visit_within(root, query, &mut visit);
        }
    }

    fn visit_within(&self, node_index: usize, query: FaceBounds, visit: &mut impl FnMut(usize)) {
        let node = &self.nodes[node_index];
        if !node.bounds().intersects(query) {
            return;
        }
        match node {
            FaceIndexNode::Leaf { start, end, .. } => {
                for face_index in self.order[*start..*end].iter().copied() {
                    if query.contains_bounds(self.face_bounds[face_index]) {
                        visit(face_index);
                    }
                }
            }
            FaceIndexNode::Branch { left, right, .. } => {
                self.visit_within(*left, query, visit);
                self.visit_within(*right, query, visit);
            }
        }
    }
}

#[derive(Debug)]
struct ChildLinks {
    first_child: Vec<usize>,
    next_sibling: Vec<usize>,
}

fn child_links(parents: &[usize]) -> Result<ChildLinks, PolygonizeError> {
    let mut first_child = Vec::new();
    first_child
        .try_reserve_exact(parents.len())
        .map_err(|_| PolygonizeError::AllocationFailed("primo figlio delle facce"))?;
    first_child.resize(parents.len(), NO_FACE);
    let mut next_sibling = Vec::new();
    next_sibling
        .try_reserve_exact(parents.len())
        .map_err(|_| PolygonizeError::AllocationFailed("fratello delle facce"))?;
    next_sibling.resize(parents.len(), NO_FACE);
    for child_index in (0..parents.len()).rev() {
        let parent_index = parents[child_index];
        if parent_index != NO_FACE {
            next_sibling[child_index] = first_child[parent_index];
            first_child[parent_index] = child_index;
        }
    }
    Ok(ChildLinks {
        first_child,
        next_sibling,
    })
}

fn assemble_face_holes(
    polygons: Vec<Polygon<f64>>,
    limits: PolygonizeLimits,
    output_budget: &mut OutputBudget,
) -> Result<Vec<Polygon<f64>>, PolygonizeError> {
    let areas = FaceAreas::new(&polygons);
    let distinct_areas = areas.distinct()?;
    let links = if distinct_areas {
        let mut parents = vec![NO_FACE; polygons.len()];
        let index = FaceSpatialIndex::build(&polygons)?;
        for (child_index, child) in polygons.iter().enumerate() {
            let sample = child
                .interior_point()
                .ok_or(PolygonizeError::InternalInvariant(
                    "punto interno della faccia assente",
                ))?;
            let mut visited = Vec::new();
            index.for_each_containing_point(sample, |candidate_index| {
                visited.push(candidate_index);
            });
            let mut best: Option<usize> = None;
            for candidate_index in visited {
                if candidate_index == child_index
                    || areas.cmp(candidate_index, child_index)? != Ordering::Greater
                    || !polygons[candidate_index].contains(&sample)
                {
                    continue;
                }
                let keep_previous = match best {
                    Some(previous) => {
                        let order = areas.cmp(previous, candidate_index)?;
                        order == Ordering::Less
                            || (order == Ordering::Equal && previous < candidate_index)
                    }
                    None => false,
                };
                if !keep_previous {
                    best = Some(candidate_index);
                }
            }
            parents[child_index] = best.unwrap_or(NO_FACE);
        }
        Some(child_links(&parents)?)
    } else {
        None
    };
    let mut output = Vec::new();
    output
        .try_reserve_exact(polygons.len())
        .map_err(|_| PolygonizeError::AllocationFailed("facce con fori"))?;
    let mut intermediate_budget = OutputBudget::new(limits);
    for (index, polygon) in polygons.iter().enumerate() {
        let mut children = Vec::new();
        let mut child = links
            .as_ref()
            .map_or(NO_FACE, |value| value.first_child[index]);
        while child != NO_FACE {
            children
                .try_reserve(1)
                .map_err(|_| PolygonizeError::AllocationFailed("figli delle facce"))?;
            children.push(&polygons[child]);
            child = links
                .as_ref()
                .map_or(NO_FACE, |value| value.next_sibling[child]);
        }
        let holes = union_boundary_rings(&children, limits)?;
        let face = Polygon::new(polygon.exterior().clone(), holes);
        validate_polygonized_face(&face, &format!("assemblaggio della faccia {index} fallito"))?;
        intermediate_budget.charge_polygon(&face)?;
        output.push(face);
    }
    drop(areas);
    drop(links);
    atomize_contained_faces(output, limits, output_budget)
}

fn atomize_contained_faces(
    polygons: Vec<Polygon<f64>>,
    limits: PolygonizeLimits,
    output_budget: &mut OutputBudget,
) -> Result<Vec<Polygon<f64>>, PolygonizeError> {
    let areas = FaceAreas::new(&polygons);
    let interior_points = polygons
        .iter()
        .map(InteriorPoint::interior_point)
        .collect::<Vec<_>>();
    let index = if areas.distinct()? {
        Some(FaceSpatialIndex::build(&polygons)?)
    } else {
        None
    };
    let mut output = Vec::new();
    output
        .try_reserve_exact(polygons.len())
        .map_err(|_| PolygonizeError::AllocationFailed("facce atomizzate"))?;
    for (parent_index, parent) in polygons.iter().enumerate() {
        let mut candidates = Vec::new();
        if let Some(index) = &index {
            let parent_bounds = index.face_bounds[parent_index];
            let mut visited = Vec::new();
            index.for_each_within(parent_bounds, |child_index| visited.push(child_index));
            for child_index in visited {
                if child_index != parent_index
                    && areas.cmp(child_index, parent_index)? == Ordering::Less
                    && polygons[child_index]
                        .exterior()
                        .0
                        .iter()
                        .take(polygons[child_index].exterior().0.len().saturating_sub(1))
                        .all(|coordinate| parent.contains(&Point::from(*coordinate)))
                {
                    candidates.push(child_index);
                }
            }
        }
        let mut direct_children = Vec::new();
        for &child_index in &candidates {
            let mut has_between = false;
            for &between_index in &candidates {
                if between_index != child_index
                    && areas.cmp(between_index, child_index)? == Ordering::Greater
                    && interior_points[child_index]
                        .is_some_and(|point| polygons[between_index].contains(&point))
                {
                    has_between = true;
                    break;
                }
            }
            if !has_between {
                direct_children.push(&polygons[child_index]);
            }
        }
        let mut holes = parent.interiors().to_vec();
        holes.extend(union_boundary_rings(&direct_children, limits)?);
        let face = Polygon::new(parent.exterior().clone(), holes);
        validate_polygonized_face(
            &face,
            &format!("atomizzazione della faccia {parent_index} fallita"),
        )?;
        output_budget.charge_polygon(&face)?;
        output.push(face);
    }
    Ok(output)
}

fn walk_edge_chain(
    start: CoordKey,
    first_edge: EdgeKey,
    graph: &BTreeMap<CoordKey, Vec<EdgeKey>>,
    remaining: &mut BTreeSet<EdgeKey>,
    coordinates: &BTreeMap<CoordKey, Coord<f64>>,
) -> Result<Option<LineString<f64>>, PolygonizeError> {
    let first_coordinate =
        coordinates
            .get(&start)
            .copied()
            .ok_or(PolygonizeError::InternalInvariant(
                "coordinata iniziale della catena assente",
            ))?;
    let mut output = vec![first_coordinate];
    let mut current = start;
    let mut edge = first_edge;
    loop {
        if !remaining.remove(&edge) {
            break;
        }
        let next = edge.opposite(current);
        output.push(
            coordinates
                .get(&next)
                .copied()
                .ok_or(PolygonizeError::InternalInvariant(
                    "coordinata della catena assente",
                ))?,
        );
        if next == start {
            break;
        }
        let incident = graph.get(&next).ok_or(PolygonizeError::InternalInvariant(
            "vertice della catena assente dal grafo",
        ))?;
        if incident.len() != 2 {
            break;
        }
        let Some(next_edge) = incident
            .iter()
            .find(|candidate| remaining.contains(candidate))
            .copied()
        else {
            break;
        };
        current = next;
        edge = next_edge;
    }
    Ok((output.len() >= 2).then(|| LineString::new(output)))
}

fn edge_chains(
    edges: &BTreeSet<EdgeKey>,
    coordinates: &BTreeMap<CoordKey, Coord<f64>>,
    output_budget: &mut OutputBudget,
) -> Result<Vec<LineString<f64>>, PolygonizeError> {
    let graph = adjacency(edges);
    let mut remaining = edges.clone();
    let mut output = Vec::new();
    let starts = graph
        .iter()
        .filter(|(_, incident)| incident.len() != 2)
        .map(|(vertex, _)| *vertex)
        .collect::<Vec<_>>();
    for start in starts {
        while let Some(edge) = graph
            .get(&start)
            .and_then(|incident| {
                incident
                    .iter()
                    .find(|candidate| remaining.contains(candidate))
            })
            .copied()
        {
            if let Some(chain) = walk_edge_chain(start, edge, &graph, &mut remaining, coordinates)?
            {
                output_budget.charge_line(&chain)?;
                output
                    .try_reserve(1)
                    .map_err(|_| PolygonizeError::AllocationFailed("catene di archi"))?;
                output.push(chain);
            }
        }
    }
    while let Some(edge) = remaining.iter().next().copied() {
        if let Some(chain) = walk_edge_chain(edge.0, edge, &graph, &mut remaining, coordinates)? {
            output_budget.charge_line(&chain)?;
            output
                .try_reserve(1)
                .map_err(|_| PolygonizeError::AllocationFailed("catene di archi"))?;
            output.push(chain);
        } else {
            return Err(PolygonizeError::InternalInvariant(
                "catena residua senza coordinate",
            ));
        }
    }
    Ok(output)
}

fn checked_output(
    result: &PolygonizeResult,
    options: PolygonizeOptions,
) -> Result<(), PolygonizeError> {
    let geometries = result
        .polygons
        .len()
        .checked_add(result.residual_count()?)
        .ok_or(PolygonizeError::IndexOverflow)?;
    let geometries = u64::try_from(geometries).map_err(|_| PolygonizeError::IndexOverflow)?;
    if geometries > options.limits.max_output_geometries {
        return Err(PolygonizeError::OutputLimit {
            actual: geometries,
            limit: options.limits.max_output_geometries,
        });
    }
    let coordinates = result
        .polygons
        .iter()
        .map(CoordsIter::coords_count)
        .chain(result.cut_edges.iter().map(CoordsIter::coords_count))
        .chain(result.dangles.iter().map(CoordsIter::coords_count))
        .chain(
            result
                .invalid_ring_lines
                .iter()
                .map(CoordsIter::coords_count),
        )
        .try_fold(0_u64, |total, count| {
            let count = u64::try_from(count).map_err(|_| PolygonizeError::IndexOverflow)?;
            total
                .checked_add(count)
                .ok_or(PolygonizeError::IndexOverflow)
        })?;
    if coordinates > options.limits.max_output_coordinates {
        return Err(PolygonizeError::OutputLimit {
            actual: coordinates,
            limit: options.limits.max_output_coordinates,
        });
    }
    Ok(())
}

/// Polygonizza linework 2D senza GEOS.
///
/// Il candidato costruisce un grafo planare, elimina iterativamente i dangle
/// ed estrae le facce tramite mezzi archi ordinati per angolo. Con
/// `node_input=true`, ogni intersezione viene inserita in entrambi i segmenti
/// prima della costruzione del grafo.
///
/// # Errors
///
/// Restituisce un errore per tipi non lineari, input non validi, limiti
/// superati, output invalido o residui quando `require_complete` e' attivo.
pub fn polygonize_linework_rust(
    linework: &Geometry<f64>,
    options: PolygonizeOptions,
) -> Result<PolygonizeResult, PolygonizeError> {
    polygonize_linework_rust_with_rounded(linework, options).map(|(result, _)| result)
}

/// [`polygonize_linework_rust`] che restituisce anche i vertici del grafo
/// nodato che sono punti d'incrocio arrotondati (non esattamente su entrambi
/// i segmenti che dividono).
///
/// # Errors
///
/// Come [`polygonize_linework_rust`].
pub fn polygonize_linework_rust_with_rounded(
    linework: &Geometry<f64>,
    options: PolygonizeOptions,
) -> Result<(PolygonizeResult, Vec<Coord<f64>>), PolygonizeError> {
    if !(options.precision.is_finite() && options.precision > 0.0) {
        return Err(PolygonizeError::InvalidPrecision);
    }
    checked_preflight(linework, options)?;
    let mut lines = Vec::new();
    collect_lines(linework, &mut lines)?;
    linework
        .check_validation()
        .map_err(|error| PolygonizeError::InvalidInput(error.to_string()))?;
    // Coordinate troppo rade per la precisione: nessun punto calcolato
    // (noding) potrebbe restarvi entro.
    if !super::precision::coordinate_abbastanza_fitte(
        super::precision::modulo_massimo(linework.coords_iter()),
        options.precision,
    ) {
        return Err(PolygonizeError::PrecisionInsufficient);
    }
    let count = segment_count(&lines)?;
    let mut segments = collect_segments(&lines, count)?;
    if options.node_input {
        segments = deduplicate_segments(segments);
    }
    let mut output_budget = OutputBudget::new(options.limits);
    let (excluded_edges, mut duplicate_cut_lines) =
        duplicate_lines_without_noding(&lines, options.node_input, &mut output_budget)?;
    let mut rounded = BTreeSet::new();
    let mut edges = if options.node_input {
        node_edges_iteratively(
            &segments,
            options.limits.max_noding_work,
            options.precision,
            &mut rounded,
        )?
    } else {
        build_unnoded_edges(&segments)
    };
    edges.retain(|edge, _| !excluded_edges.contains(edge));
    drop(excluded_edges);
    drop(segments);
    drop(lines);
    let mut coordinates = BTreeMap::new();
    for (edge, (start, end)) in &edges {
        coordinates.entry(edge.0).or_insert(*start);
        coordinates.entry(edge.1).or_insert(*end);
    }
    let rounded = rounded
        .into_iter()
        .filter_map(|key| coordinates.get(&key).copied())
        .collect::<Vec<_>>();
    let mut active = edges.keys().copied().collect::<BTreeSet<_>>();
    drop(edges);
    let dangle_edges = remove_dangles(&mut active);
    let Faces {
        polygons,
        polygon_edges,
        invalid_rings,
    } = extract_faces(&active, &coordinates, options.limits, &mut output_budget)?;
    let cut_edges = active
        .difference(&polygon_edges)
        .copied()
        .collect::<BTreeSet<_>>();
    drop(active);
    drop(polygon_edges);
    let cut_lines = edge_chains(&cut_edges, &coordinates, &mut output_budget)?;
    drop(cut_edges);
    duplicate_cut_lines.extend(cut_lines);
    let dangles = edge_chains(&dangle_edges, &coordinates, &mut output_budget)?;
    drop(dangle_edges);
    drop(coordinates);
    let result = PolygonizeResult {
        polygons,
        cut_edges: duplicate_cut_lines,
        dangles,
        invalid_ring_lines: invalid_rings,
    };
    checked_output(&result, options)?;
    let residuals = result.residual_count()?;
    if options.require_complete && residuals != 0 {
        return Err(PolygonizeError::Incomplete { residuals });
    }
    Ok((result, rounded))
}

/// Variante per l'integrazione controllata che richiede tutti i budget.
///
/// # Errors
///
/// Restituisce [`PolygonizeError::UnboundedLimitConfiguration`] se almeno un
/// limite e' lasciato a [`u64::MAX`]; altrimenti propaga gli errori di
/// [`polygonize_linework_rust`].
pub fn polygonize_linework_rust_bounded(
    linework: &Geometry<f64>,
    options: PolygonizeOptions,
) -> Result<PolygonizeResult, PolygonizeError> {
    if !options.limits.is_fully_bounded() {
        return Err(PolygonizeError::UnboundedLimitConfiguration);
    }
    polygonize_linework_rust(linework, options)
}

#[cfg(test)]
mod tests {
    use super::*;
    use geo::{line_string, polygon, Area, MultiLineString};

    /// Precisione dei test del laboratorio: coordinate astratte, un
    /// milionesimo di unita'.
    const PRECISION: f64 = 1e-6;

    fn options(node_input: bool, require_complete: bool) -> PolygonizeOptions {
        PolygonizeOptions {
            node_input,
            require_complete,
            precision: PRECISION,
            limits: PolygonizeLimits::unlimited(),
        }
    }

    #[test]
    fn face_index_preserves_nested_and_disjoint_candidates() -> Result<(), PolygonizeError> {
        let polygons = vec![
            polygon![(x: 0.0, y: 0.0), (x: 10.0, y: 0.0), (x: 10.0, y: 10.0), (x: 0.0, y: 10.0), (x: 0.0, y: 0.0)],
            polygon![(x: 2.0, y: 2.0), (x: 3.0, y: 2.0), (x: 3.0, y: 3.0), (x: 2.0, y: 3.0), (x: 2.0, y: 2.0)],
            polygon![(x: 20.0, y: 0.0), (x: 22.0, y: 0.0), (x: 22.0, y: 2.0), (x: 20.0, y: 2.0), (x: 20.0, y: 0.0)],
        ];
        let index = FaceSpatialIndex::build(&polygons)?;
        let mut containing = Vec::new();
        index.for_each_containing_point(Point::new(2.5, 2.5), |value| containing.push(value));
        containing.sort_unstable();
        if containing != vec![0, 1] {
            return Err(PolygonizeError::InvalidOutput(format!(
                "candidati contenenti inattesi: {containing:?}"
            )));
        }
        let mut within = Vec::new();
        index.for_each_within(index.face_bounds[0], |value| within.push(value));
        within.sort_unstable();
        if within != vec![0, 1] {
            return Err(PolygonizeError::InvalidOutput(format!(
                "candidati interni inattesi: {within:?}"
            )));
        }
        Ok(())
    }

    #[test]
    fn polygonizes_many_disjoint_faces_with_distinct_areas() -> Result<(), PolygonizeError> {
        let mut rings = Vec::new();
        for index in 0_u32..128 {
            let x = f64::from(index) * 3.0;
            let width = if index % 2 == 0 { 1.0 } else { 1.5 };
            rings.push(LineString::new(vec![
                Coord { x, y: 0.0 },
                Coord {
                    x: x + width,
                    y: 0.0,
                },
                Coord {
                    x: x + width,
                    y: width,
                },
                Coord { x, y: width },
                Coord { x, y: 0.0 },
            ]));
        }
        let input = Geometry::MultiLineString(MultiLineString(rings));
        let result = polygonize_linework_rust(&input, options(false, true))?;
        if result.polygons.len() != 128 || result.residual_count()? != 0 {
            return Err(PolygonizeError::InvalidOutput(format!(
                "output distinto inatteso: poligoni={}, residui={}",
                result.polygons.len(),
                result.residual_count()?
            )));
        }
        Ok(())
    }

    #[test]
    fn polygonizes_square_and_classifies_tail_as_dangle() -> Result<(), PolygonizeError> {
        let input = Geometry::MultiLineString(MultiLineString(vec![
            line_string![(x: 0.0, y: 0.0), (x: 2.0, y: 0.0)],
            line_string![(x: 2.0, y: 0.0), (x: 2.0, y: 2.0)],
            line_string![(x: 2.0, y: 2.0), (x: 0.0, y: 2.0)],
            line_string![(x: 0.0, y: 2.0), (x: 0.0, y: 0.0)],
            line_string![(x: 2.0, y: 2.0), (x: 3.0, y: 2.0)],
        ]));
        let result = polygonize_linework_rust(&input, options(true, false))?;
        if result.polygons.len() != 1 || result.dangles.len() != 1 {
            return Err(PolygonizeError::InvalidOutput(format!(
                "poligoni={}, dangles={}",
                result.polygons.len(),
                result.dangles.len()
            )));
        }
        Ok(())
    }

    #[test]
    fn nodes_crossing_lines_into_four_faces() -> Result<(), PolygonizeError> {
        let input = Geometry::MultiLineString(MultiLineString(vec![
            line_string![(x: 0.0, y: 0.0), (x: 2.0, y: 0.0), (x: 2.0, y: 2.0), (x: 0.0, y: 2.0), (x: 0.0, y: 0.0)],
            line_string![(x: 1.0, y: 0.0), (x: 1.0, y: 2.0)],
            line_string![(x: 0.0, y: 1.0), (x: 2.0, y: 1.0)],
        ]));
        let result = polygonize_linework_rust(&input, options(true, true))?;
        let residuals = result.residual_count()?;
        if result.polygons.len() != 4 || residuals != 0 {
            return Err(PolygonizeError::InvalidOutput(format!(
                "poligoni={}, residui={}",
                result.polygons.len(),
                residuals
            )));
        }
        Ok(())
    }

    #[test]
    fn enforces_noding_work_limit() {
        let input = Geometry::LineString(line_string![
            (x: 0.0, y: 0.0), (x: 1.0, y: 0.0), (x: 1.0, y: 1.0)
        ]);
        let result = polygonize_linework_rust(
            &input,
            PolygonizeOptions {
                node_input: true,
                require_complete: false,
                precision: PRECISION,
                limits: PolygonizeLimits {
                    max_noding_work: 1,
                    ..PolygonizeLimits::unlimited()
                },
            },
        );
        assert!(matches!(result, Err(PolygonizeError::WorkLimit { .. })));
    }

    #[test]
    fn noding_index_prunes_only_disjoint_envelopes_and_preserves_budget(
    ) -> Result<(), PolygonizeError> {
        let segments = vec![
            Segment {
                start: Coord { x: 0.0, y: 0.0 },
                end: Coord { x: 2.0, y: 0.0 },
            },
            Segment {
                start: Coord { x: 1.0, y: -1.0 },
                end: Coord { x: 1.0, y: 1.0 },
            },
            Segment {
                start: Coord { x: 3.0, y: 0.0 },
                end: Coord { x: 4.0, y: 0.0 },
            },
            Segment {
                start: Coord { x: 0.0, y: 3.0 },
                end: Coord { x: 2.0, y: 3.0 },
            },
        ];
        let mut visited = BTreeSet::new();
        let mut budget = NodingBudget::new(u64::MAX);
        let completed = visit_candidate_pairs(&segments, &mut budget, |left, right| {
            visited.insert((left, right));
            Ok(true)
        })?;
        if !completed || visited != BTreeSet::from([(0, 1)]) {
            return Err(PolygonizeError::InternalInvariant(
                "il filtro spaziale del noding ha visitato coppie inattese",
            ));
        }

        let same_x_disjoint_y = [segments[0], segments[3]];
        let limited =
            visit_candidate_pairs(&same_x_disjoint_y, &mut NodingBudget::new(0), |_, _| {
                Ok(true)
            });
        if !matches!(limited, Err(PolygonizeError::WorkLimit { .. })) {
            return Err(PolygonizeError::InternalInvariant(
                "il filtro Y non ha consumato il budget del ciclo interno",
            ));
        }

        let x_disjoint = [segments[0], segments[2]];
        visit_candidate_pairs(&x_disjoint, &mut NodingBudget::new(0), |_, _| Ok(true))?;
        Ok(())
    }

    #[test]
    fn enforces_all_limits_and_is_deterministic() -> Result<(), PolygonizeError> {
        let input = Geometry::MultiLineString(MultiLineString(vec![
            line_string![(x: 0.0, y: 0.0), (x: 2.0, y: 0.0)],
            line_string![(x: 2.0, y: 0.0), (x: 2.0, y: 2.0)],
            line_string![(x: 2.0, y: 2.0), (x: 0.0, y: 2.0)],
            line_string![(x: 0.0, y: 2.0), (x: 0.0, y: 0.0)],
            line_string![(x: 2.0, y: 2.0), (x: 3.0, y: 2.0)],
        ]));
        let first = polygonize_linework_rust(&input, options(true, false))?;
        let second = polygonize_linework_rust(&input, options(true, false))?;
        if first != second {
            return Err(PolygonizeError::InvalidOutput(
                "esecuzioni identiche non deterministiche".to_owned(),
            ));
        }
        let coordinate_limit = polygonize_linework_rust(
            &input,
            PolygonizeOptions {
                limits: PolygonizeLimits {
                    max_input_coordinates: 9,
                    ..PolygonizeLimits::unlimited()
                },
                ..options(true, false)
            },
        );
        if !matches!(
            coordinate_limit,
            Err(PolygonizeError::CoordinateLimit { .. })
        ) {
            return Err(PolygonizeError::InvalidOutput(
                "limite coordinate input non applicato".to_owned(),
            ));
        }
        let geometry_limit = polygonize_linework_rust(
            &input,
            PolygonizeOptions {
                limits: PolygonizeLimits {
                    max_output_geometries: 1,
                    ..PolygonizeLimits::unlimited()
                },
                ..options(true, false)
            },
        );
        if !matches!(geometry_limit, Err(PolygonizeError::OutputLimit { .. })) {
            return Err(PolygonizeError::InvalidOutput(
                "limite geometrie output non applicato".to_owned(),
            ));
        }
        let output_coordinate_limit = polygonize_linework_rust(
            &input,
            PolygonizeOptions {
                limits: PolygonizeLimits {
                    max_output_coordinates: 5,
                    ..PolygonizeLimits::unlimited()
                },
                ..options(true, false)
            },
        );
        if !matches!(
            output_coordinate_limit,
            Err(PolygonizeError::OutputLimit { .. })
        ) {
            return Err(PolygonizeError::InvalidOutput(
                "limite coordinate output non applicato".to_owned(),
            ));
        }
        let incomplete = polygonize_linework_rust(&input, options(true, true));
        if !matches!(incomplete, Err(PolygonizeError::Incomplete { .. })) {
            return Err(PolygonizeError::InvalidOutput(
                "require_complete non applicato".to_owned(),
            ));
        }
        let unsupported = polygonize_linework_rust(
            &Geometry::Point(Point::new(0.0, 0.0)),
            options(false, false),
        );
        if !matches!(unsupported, Err(PolygonizeError::UnsupportedGeometry(_))) {
            return Err(PolygonizeError::InvalidOutput(
                "tipo non lineare accettato".to_owned(),
            ));
        }
        Ok(())
    }

    #[test]
    fn nodes_an_anisotropic_bow_tie() -> Result<(), PolygonizeError> {
        let input = Geometry::LineString(line_string![
            (x: -2_000_000.0, y: 0.000_003),
            (x: 2_000_000.0, y: 0.000_007),
            (x: -2_000_000.0, y: 0.000_007),
            (x: 2_000_000.0, y: 0.000_003),
            (x: -2_000_000.0, y: 0.000_003)
        ]);
        let result = polygonize_linework_rust(&input, options(true, false))?;
        let residuals = result.residual_count()?;
        if result.polygons.len() != 2 || residuals != 0 {
            return Err(PolygonizeError::InvalidOutput(format!(
                "poligoni={}, residui={}",
                result.polygons.len(),
                residuals
            )));
        }
        Ok(())
    }

    #[test]
    fn extended_intersection_matches_geos_rounding() -> Result<(), PolygonizeError> {
        let point = extended_precision_intersection(
            Segment {
                start: Coord { x: 3.3, y: 11.3 },
                end: Coord { x: -2.7, y: -12.7 },
            },
            Segment {
                start: Coord { x: 3.3, y: -2.7 },
                end: Coord { x: -2.7, y: -2.7 },
            },
        );
        let expected = Some(Coord {
            x: -0.200_000_000_000_000_4,
            y: -2.7,
        });
        if point != expected {
            return Err(PolygonizeError::InvalidOutput(format!(
                "intersezione={point:?}, attesa={expected:?}"
            )));
        }
        Ok(())
    }

    #[test]
    fn iterated_noding_matches_geos_on_near_coincident_crossings() -> Result<(), PolygonizeError> {
        let input = Geometry::MultiLineString(MultiLineString(vec![
            line_string![(x: 2.55, y: 23.3), (x: 2.55, y: -24.7)],
            line_string![(x: 3.3, y: 11.3), (x: -2.7, y: -12.7)],
            line_string![(x: 3.3, y: -2.7), (x: -2.7, y: -2.7)],
            line_string![(x: 0.3, y: 9.3), (x: 0.3, y: 17.3), (x: 1.05, y: 17.3), (x: 1.05, y: 9.3), (x: 0.3, y: 9.3)],
            line_string![(x: 1.3, y: -10.7), (x: 1.3, y: -2.7), (x: 1.55, y: -2.7), (x: 1.55, y: -10.7), (x: 1.3, y: -10.7)],
            line_string![(x: -1.2, y: -6.7), (x: -1.2, y: 5.3), (x: -0.2, y: 5.3), (x: -0.2, y: -6.7), (x: -1.2, y: -6.7)],
            line_string![(x: 0.3, y: 9.3), (x: 0.3, y: 13.3), (x: 1.8, y: 13.3), (x: 1.8, y: 9.3), (x: 0.3, y: 9.3)],
        ]));
        let result = polygonize_linework_rust(&input, options(true, false))?;
        // GEOS: 8 poligoni e 5 residui (4 dangle, 1 anello invalido). Con il
        // segno esatto l'anello invalido di GEOS e' un triangolo vero di area
        // circa 3,45e-31 e diventa il nono poligono: GEOS lo scarta con la
        // propria precisione double-double. Divergenza dichiarata nel README
        // («Differenze da GEOS»); area totale e dangle restano quelli di GEOS.
        let polygon_area = result.polygons.iter().map(Area::unsigned_area).sum::<f64>();
        let residuals = result.residual_count()?;
        let slivers = result
            .polygons
            .iter()
            .filter(|polygon| polygon.unsigned_area() < 1e-30)
            .count();
        if result.polygons.len() != 9
            || slivers != 1
            || !result.cut_edges.is_empty()
            || result.dangles.len() != 4
            || (polygon_area - 38.125).abs() > 1e-12
            || residuals != 4
        {
            return Err(PolygonizeError::InvalidOutput(format!(
                "poligoni={}, area={}, cut={}, dangles={}, invalid={}",
                result.polygons.len(),
                polygon_area,
                result.cut_edges.len(),
                result.dangles.len(),
                result.invalid_ring_lines.len()
            )));
        }
        Ok(())
    }

    #[test]
    fn preserves_near_endpoint_face_on_anisotropic_linework() -> Result<(), PolygonizeError> {
        let input = Geometry::MultiLineString(MultiLineString(vec![
            line_string![(x: 6_000_000.0, y: 0.000_015), (x: 6_000_000.0, y: -0.000_009)],
            line_string![(x: 10_000_000.0, y: 0.000_007), (x: -14_000_000.0, y: -0.000_000_999_999_999_999_999_7)],
            line_string![(x: -3_000_000.0, y: 0.0), (x: -3_000_000.0, y: 0.000_002_000_000_000_000_000_3), (x: 0.0, y: 0.000_002_000_000_000_000_000_3), (x: 0.0, y: 0.0), (x: -3_000_000.0, y: 0.0)],
            line_string![(x: 1_000_000.0, y: -0.000_001_999_999_999_999_999_5), (x: 1_000_000.0, y: 0.000_004), (x: 6_000_000.0, y: 0.000_004), (x: 6_000_000.0, y: -0.000_001_999_999_999_999_999_5), (x: 1_000_000.0, y: -0.000_001_999_999_999_999_999_5)],
            line_string![(x: 4_000_000.0, y: 0.000_004_999_999_999_999_999_6), (x: 4_000_000.0, y: 0.000_007), (x: 10_000_000.0, y: 0.000_007), (x: 10_000_000.0, y: 0.000_004_999_999_999_999_999_6), (x: 4_000_000.0, y: 0.000_004_999_999_999_999_999_6)],
            line_string![(x: 4_000_000.0, y: -0.000_000_999_999_999_999_999_7), (x: 4_000_000.0, y: 0.0), (x: 5_000_000.0, y: 0.0), (x: 5_000_000.0, y: -0.000_000_999_999_999_999_999_7), (x: 4_000_000.0, y: -0.000_000_999_999_999_999_999_7)],
        ]));
        let result = polygonize_linework_rust(&input, options(true, false))?;
        if result.polygons.len() != 8 || !result.cut_edges.is_empty() || result.dangles.len() != 3 {
            return Err(PolygonizeError::InvalidOutput(format!(
                "poligoni={}, cut={}, dangles={}",
                result.polygons.len(),
                result.cut_edges.len(),
                result.dangles.len()
            )));
        }
        Ok(())
    }
}
