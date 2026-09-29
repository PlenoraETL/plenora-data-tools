//! Riparazione geometrica (`LINEWORK` e `STRUCTURE`) in Rust puro, sopra il
//! polygonize di [`super::polygonize`] e l'overlay di `geo`.
//!
//! Portato da `plenora-memory-lab/operations/geo_rust/make_valid/src/lib.rs`
//! (SHA-256 `c32ce24489a53aaee7b814e873dbf166f6611df196899363c45619568e99630e`,
//! lo stesso registrato in `results/geo-rust/fuzz-provenance.json`). Le
//! modifiche sono solo quelle elencate nel modulo padre.
#![forbid(unsafe_code)]
#![deny(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
// Codice portato dal laboratorio (vedi il modulo padre): la numerica e'
// quella qualificata contro GEOS, e i lint qui sotto la toccherebbero.
// `mul_add` cambierebbe l'arrotondamento della normalizzazione dell'overlay
// e del test punto-su-segmento; `linework` resta una funzione sola come
// nella sorgente qualificata, e i nomi dei binding sono quelli.
#![allow(
    clippy::suboptimal_flops,
    clippy::similar_names,
    clippy::too_many_lines
)]

use std::collections::{BTreeMap, BTreeSet};

use super::polygonize::{
    polygonize_linework_rust, PolygonizeError, PolygonizeLimits, PolygonizeOptions,
};
use geo::algorithm::validation::Validation;
use geo::kernels::{Kernel, Orientation, RobustKernel};
use geo::line_intersection::{line_intersection, LineIntersection};
use geo::{
    BooleanOps, Coord, CoordsIter, Geometry, GeometryCollection, Intersects, Line, LineString,
    MultiLineString, MultiPoint, MultiPolygon, Point, Polygon,
};
use rstar::{RTree, RTreeObject, AABB};
use thiserror::Error;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RepairMethod {
    Linework,
    Structure,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MakeValidLimits {
    pub max_input_coordinates: u64,
    pub max_noding_work: u64,
    pub max_output_geometries: u64,
    pub max_output_coordinates: u64,
}

impl MakeValidLimits {
    #[must_use]
    pub const fn unlimited() -> Self {
        Self {
            max_input_coordinates: u64::MAX,
            max_noding_work: u64::MAX,
            max_output_geometries: u64::MAX,
            max_output_coordinates: u64::MAX,
        }
    }

    /// Indica che input, noding e output hanno tutti un tetto esplicito.
    #[must_use]
    pub const fn is_fully_bounded(self) -> bool {
        self.max_input_coordinates != u64::MAX
            && self.max_noding_work != u64::MAX
            && self.max_output_geometries != u64::MAX
            && self.max_output_coordinates != u64::MAX
    }
}

#[derive(Debug, Error)]
pub enum MakeValidError {
    #[error("coordinate non finite")]
    NonFiniteCoordinate,
    #[error("geometria strutturalmente non valida: {0}")]
    InvalidStructure(&'static str),
    #[error("tipo non supportato dal candidato: {0}")]
    UnsupportedGeometry(&'static str),
    #[error("profilo limiti incompleto: input, noding e output devono avere tetti espliciti")]
    UnboundedLimitConfiguration,
    #[error("coordinate oltre il limite di {limit}: {actual}")]
    CoordinateLimit { actual: u64, limit: u64 },
    #[error("lavoro di noding oltre il limite di {limit}: {actual}")]
    WorkLimit { actual: u64, limit: u64 },
    #[error("output oltre il limite di {limit}: {actual}")]
    OutputLimit { actual: u64, limit: u64 },
    #[error("indice non rappresentabile")]
    IndexOverflow,
    #[error("polygonize fallita: {0}")]
    Polygonize(PolygonizeError),
    #[error("output Rust ancora non valido: {0}")]
    InvalidOutput(String),
    #[error("invariante interna violata: {0}")]
    InternalInvariant(&'static str),
    #[error("prenotazione di memoria fallita per {0}")]
    AllocationFailed(&'static str),
    /// Un segno d'area non decidibile in `f64` su coordinate fuori dal
    /// dominio dell'aritmetica esatta (vedi `super::exact`). Deviazione dal
    /// laboratorio, che decideva comunque.
    #[error("coordinate fuori dal dominio dell'aritmetica esatta delle aree")]
    NumericRange,
    /// La griglia intera di `i_overlay` su questa geometria e' piu' grossa
    /// della precisione dichiarata: l'overlay non verrebbe eseguito entro la
    /// precisione, e non viene eseguito. Il solo rifiuto legato alla
    /// precisione (README, «Limiti dichiarati»).
    #[error("geometria troppo estesa per la precisione dichiarata")]
    PrecisionInsufficient,
    /// La precisione dichiarata passata non e' un numero finito positivo.
    #[error("precisione dichiarata non valida: deve essere finita e positiva")]
    InvalidPrecision,
}

impl From<PolygonizeError> for MakeValidError {
    fn from(error: PolygonizeError) -> Self {
        match error {
            PolygonizeError::CoordinateLimit { actual, limit } => {
                Self::CoordinateLimit { actual, limit }
            }
            PolygonizeError::WorkLimit { actual, limit } => Self::WorkLimit { actual, limit },
            PolygonizeError::OutputLimit { actual, limit } => Self::OutputLimit { actual, limit },
            PolygonizeError::IndexOverflow => Self::IndexOverflow,
            // Un solo rifiuto legato alla precisione, qualunque passo lo dia.
            PolygonizeError::PrecisionInsufficient => Self::PrecisionInsufficient,
            other => Self::Polygonize(other),
        }
    }
}

const fn polygonize_limits(limits: MakeValidLimits) -> PolygonizeLimits {
    PolygonizeLimits {
        max_input_coordinates: limits.max_input_coordinates,
        max_noding_work: limits.max_noding_work,
        max_output_geometries: limits.max_output_geometries,
        max_output_coordinates: limits.max_output_coordinates,
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct CoordKey {
    x: u64,
    y: u64,
}

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
        Self {
            x: x.to_bits(),
            y: y.to_bits(),
        }
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

fn segment_count(geometry: &Geometry<f64>) -> Result<u64, MakeValidError> {
    let line_segments = |line: &LineString<f64>| {
        u64::try_from(line.0.len().saturating_sub(1)).map_err(|_| MakeValidError::IndexOverflow)
    };
    let add = |left: u64, right: u64| left.checked_add(right).ok_or(MakeValidError::IndexOverflow);
    match geometry {
        Geometry::Point(_) | Geometry::MultiPoint(_) => Ok(0),
        Geometry::Line(_) => Ok(1),
        Geometry::LineString(line) => line_segments(line),
        Geometry::Polygon(polygon) => {
            let mut total = line_segments(polygon.exterior())?;
            for ring in polygon.interiors() {
                total = add(total, line_segments(ring)?)?;
            }
            Ok(total)
        }
        Geometry::MultiLineString(lines) => lines
            .0
            .iter()
            .try_fold(0_u64, |total, line| add(total, line_segments(line)?)),
        Geometry::MultiPolygon(polygons) => polygons.0.iter().try_fold(0_u64, |total, polygon| {
            add(total, segment_count(&Geometry::Polygon(polygon.clone()))?)
        }),
        Geometry::GeometryCollection(collection) => collection
            .0
            .iter()
            .try_fold(0_u64, |total, child| add(total, segment_count(child)?)),
        Geometry::Rect(_) => Ok(4),
        Geometry::Triangle(_) => Ok(3),
    }
}

fn checked_preflight(
    geometry: &Geometry<f64>,
    limits: MakeValidLimits,
) -> Result<(), MakeValidError> {
    let coordinates =
        u64::try_from(geometry.coords_count()).map_err(|_| MakeValidError::IndexOverflow)?;
    if coordinates > limits.max_input_coordinates {
        return Err(MakeValidError::CoordinateLimit {
            actual: coordinates,
            limit: limits.max_input_coordinates,
        });
    }
    let segments = segment_count(geometry)?;
    let work = segments
        .checked_mul(segments)
        .ok_or(MakeValidError::WorkLimit {
            actual: u64::MAX,
            limit: limits.max_noding_work,
        })?;
    if work > limits.max_noding_work {
        return Err(MakeValidError::WorkLimit {
            actual: work,
            limit: limits.max_noding_work,
        });
    }
    Ok(())
}

fn output_geometry_count(geometry: &Geometry<f64>) -> Result<u64, MakeValidError> {
    let count = match geometry {
        Geometry::MultiPoint(points) => points.0.len(),
        Geometry::MultiLineString(lines) => lines.0.len(),
        Geometry::MultiPolygon(polygons) => polygons.0.len(),
        Geometry::GeometryCollection(collection) => {
            return collection.0.iter().try_fold(0_u64, |total, child| {
                total
                    .checked_add(output_geometry_count(child)?)
                    .ok_or(MakeValidError::IndexOverflow)
            });
        }
        _ => 1,
    };
    u64::try_from(count).map_err(|_| MakeValidError::IndexOverflow)
}

fn checked_limits_output(
    geometry: &Geometry<f64>,
    limits: MakeValidLimits,
) -> Result<(), MakeValidError> {
    let geometries = output_geometry_count(geometry)?;
    if geometries > limits.max_output_geometries {
        return Err(MakeValidError::OutputLimit {
            actual: geometries,
            limit: limits.max_output_geometries,
        });
    }
    let coordinates =
        u64::try_from(geometry.coords_count()).map_err(|_| MakeValidError::IndexOverflow)?;
    if coordinates > limits.max_output_coordinates {
        return Err(MakeValidError::OutputLimit {
            actual: coordinates,
            limit: limits.max_output_coordinates,
        });
    }
    Ok(())
}

fn structurally_valid_ring(ring: &LineString<f64>) -> bool {
    ring.0.len() >= 4 && ring.0.first() == ring.0.last()
}

fn validate_structure(geometry: &Geometry<f64>) -> Result<(), MakeValidError> {
    if geometry
        .coords_iter()
        .any(|coordinate| !coordinate.x.is_finite() || !coordinate.y.is_finite())
    {
        return Err(MakeValidError::NonFiniteCoordinate);
    }
    let validate_polygon = |polygon: &Polygon<f64>| {
        if !structurally_valid_ring(polygon.exterior())
            || polygon
                .interiors()
                .iter()
                .any(|ring| !structurally_valid_ring(ring))
        {
            Err(MakeValidError::InvalidStructure(
                "anello non chiuso o con meno di quattro coordinate",
            ))
        } else {
            Ok(())
        }
    };
    match geometry {
        Geometry::Polygon(polygon) => validate_polygon(polygon),
        Geometry::MultiPolygon(polygons) => {
            for polygon in &polygons.0 {
                validate_polygon(polygon)?;
            }
            Ok(())
        }
        Geometry::GeometryCollection(collection) => {
            for child in &collection.0 {
                validate_structure(child)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

fn fixed_ring(
    ring: &LineString<f64>,
    limits: MakeValidLimits,
    precision: f64,
) -> Result<MultiPolygon<f64>, MakeValidError> {
    let result = polygonize_linework_rust(
        &Geometry::LineString(ring.clone()),
        PolygonizeOptions {
            node_input: true,
            require_complete: false,
            limits: polygonize_limits(limits),
            precision,
        },
    )?;
    Ok(MultiPolygon::new(result.polygons))
}

fn as_polygonal_geometry(polygons: MultiPolygon<f64>) -> Geometry<f64> {
    let mut values = polygons.0;
    if values.len() == 1 {
        values.pop().map_or_else(
            || Geometry::MultiPolygon(MultiPolygon::empty()),
            Geometry::Polygon,
        )
    } else {
        Geometry::MultiPolygon(MultiPolygon::new(values))
    }
}

#[derive(Clone, Copy, Debug)]
struct OverlayNormalizer {
    minimum_x: f64,
    minimum_y: f64,
    span_x: f64,
    span_y: f64,
}

impl OverlayNormalizer {
    fn new(left: &MultiPolygon<f64>, right: &MultiPolygon<f64>) -> Self {
        let mut minimum_x = f64::INFINITY;
        let mut maximum_x = f64::NEG_INFINITY;
        let mut minimum_y = f64::INFINITY;
        let mut maximum_y = f64::NEG_INFINITY;
        for coordinate in left.coords_iter().chain(right.coords_iter()) {
            minimum_x = minimum_x.min(coordinate.x);
            maximum_x = maximum_x.max(coordinate.x);
            minimum_y = minimum_y.min(coordinate.y);
            maximum_y = maximum_y.max(coordinate.y);
        }
        if !minimum_x.is_finite() {
            return Self {
                minimum_x: 0.0,
                minimum_y: 0.0,
                span_x: 1.0,
                span_y: 1.0,
            };
        }
        let span_x = maximum_x - minimum_x;
        let span_y = maximum_y - minimum_y;
        Self {
            minimum_x,
            minimum_y,
            span_x: if span_x == 0.0 { 1.0 } else { span_x },
            span_y: if span_y == 0.0 { 1.0 } else { span_y },
        }
    }

    fn normalize(self, coordinate: Coord<f64>) -> Coord<f64> {
        Coord {
            x: (coordinate.x - self.minimum_x) / self.span_x,
            y: (coordinate.y - self.minimum_y) / self.span_y,
        }
    }

    fn restore(self, coordinate: Coord<f64>) -> Coord<f64> {
        Coord {
            x: coordinate.x * self.span_x + self.minimum_x,
            y: coordinate.y * self.span_y + self.minimum_y,
        }
    }

    /// Il lato di una cella della griglia intera dell'overlay in coordinate
    /// originali: la diagonale del passo `span * 2^-30` dei due assi (vedi
    /// [`checked_grid`]).
    fn grid_diagonal(self) -> f64 {
        let step = 2_f64.powi(-30);
        (self.span_x.abs() * step).hypot(self.span_y.abs() * step)
    }

    /// Riporta l'output dell'overlay nelle coordinate originali e lo aggancia
    /// ai vertici e ai lati assiali degli operandi **vicini**.
    ///
    /// Il raggio d'aggancio `r` e' la diagonale di un passo della griglia
    /// ([`Self::grid_diagonal`]). Un vertice va sul vertice d'ingresso piu'
    /// vicino entro `r` (distanza fra punti); altrimenti ogni coordinata va
    /// sull'ascissa di un lato verticale (o sull'ordinata di un lato
    /// orizzontale) d'ingresso entro `r` che gli passa accanto, cosi' un
    /// incrocio con un lato assiale resta esattamente sul lato. Lo
    /// spostamento dell'aggancio e' al piu' `sqrt(2) * r`, e con il mezzo
    /// passo d'arrotondamento della griglia resta sotto `2 * r`: il bilancio
    /// che [`checked_grid`] confronta con la precisione dichiarata.
    ///
    /// Il laboratorio agganciava ogni coordinata, asse per asse, a qualunque
    /// ascissa od ordinata d'ingresso entro `span * 2^-29` (due passi), anche
    /// di una componente lontana e anche quando il valore era gia' esatto:
    /// con `span_x = 2^23` m un incrocio a `x = 100` finiva a `100.0155`.
    // Un lato e' assiale solo se lo e' esattamente: il confronto esatto e'
    // il predicato voluto, non un'approssimazione.
    #[allow(clippy::float_cmp)]
    fn restore_multi_snapped(
        self,
        polygons: &MultiPolygon<f64>,
        left: &MultiPolygon<f64>,
        right: &MultiPolygon<f64>,
    ) -> MultiPolygon<f64> {
        let radius = self.grid_diagonal();
        let source_coordinates = left
            .coords_iter()
            .chain(right.coords_iter())
            .collect::<Vec<_>>();
        let mut vertical = Vec::new();
        let mut horizontal = Vec::new();
        for segment in polygonal_boundaries(left).chain(polygonal_boundaries(right)) {
            if segment.start.x == segment.end.x {
                vertical.push((
                    segment.start.x,
                    segment.start.y.min(segment.end.y),
                    segment.start.y.max(segment.end.y),
                ));
            } else if segment.start.y == segment.end.y {
                horizontal.push((
                    segment.start.y,
                    segment.start.x.min(segment.end.x),
                    segment.start.x.max(segment.end.x),
                ));
            }
        }
        let snap = |restored: Coord<f64>| {
            let mut nearest: Option<(Coord<f64>, f64)> = None;
            for source in &source_coordinates {
                let distance = (restored.x - source.x).hypot(restored.y - source.y);
                if distance <= radius && nearest.is_none_or(|(_, best)| distance < best) {
                    nearest = Some((*source, distance));
                }
            }
            if let Some((source, _)) = nearest {
                return source;
            }
            let axis = |value: f64, across: f64, sides: &[(f64, f64, f64)]| {
                let mut best = value;
                let mut best_distance = f64::INFINITY;
                for &(level, minimum, maximum) in sides {
                    let distance = (value - level).abs();
                    if distance <= radius
                        && distance < best_distance
                        && across >= minimum - radius
                        && across <= maximum + radius
                    {
                        best = level;
                        best_distance = distance;
                    }
                }
                best
            };
            Coord {
                x: axis(restored.x, restored.y, &vertical),
                y: axis(restored.y, restored.x, &horizontal),
            }
        };
        let restore_line = |line: &LineString<f64>| {
            LineString::new(
                line.0
                    .iter()
                    .copied()
                    .map(|coordinate| snap(self.restore(coordinate)))
                    .collect(),
            )
        };
        MultiPolygon::new(
            polygons
                .0
                .iter()
                .map(|polygon| {
                    Polygon::new(
                        restore_line(polygon.exterior()),
                        polygon.interiors().iter().map(restore_line).collect(),
                    )
                })
                .collect(),
        )
    }

    fn map_line(
        self,
        line: &LineString<f64>,
        transform: fn(Self, Coord<f64>) -> Coord<f64>,
    ) -> LineString<f64> {
        LineString::new(
            line.0
                .iter()
                .copied()
                .map(|coordinate| transform(self, coordinate))
                .collect(),
        )
    }

    fn map_polygon(
        self,
        polygon: &Polygon<f64>,
        transform: fn(Self, Coord<f64>) -> Coord<f64>,
    ) -> Polygon<f64> {
        Polygon::new(
            self.map_line(polygon.exterior(), transform),
            polygon
                .interiors()
                .iter()
                .map(|ring| self.map_line(ring, transform))
                .collect(),
        )
    }

    fn map_multi(
        self,
        polygons: &MultiPolygon<f64>,
        transform: fn(Self, Coord<f64>) -> Coord<f64>,
    ) -> MultiPolygon<f64> {
        MultiPolygon::new(
            polygons
                .0
                .iter()
                .map(|polygon| self.map_polygon(polygon, transform))
                .collect(),
        )
    }
}

/// Il controllo della griglia prima di ogni overlay.
///
/// Gli operandi sono normalizzati per asse su `[0, 1]^2`; su quel rettangolo
/// `i_float` 1.16.0 (`FloatPointAdapter::new`) sceglie il passo
/// `2^(round(log2(0.5)) - 29) = 2^-30` (meta' della dimensione maggiore,
/// logaritmo arrotondato a meta' lontano da zero, esponente `29 - log2`):
/// in coordinate originali il passo e' `span * 2^-30` per asse, e la sua
/// diagonale `d` ([`OverlayNormalizer::grid_diagonal`]).
///
/// Il bilancio dello spostamento di un vertice e' l'arrotondamento alla
/// griglia (mezzo passo per asse, al piu' `d / 2`) piu' l'aggancio di
/// [`OverlayNormalizer::restore_multi_snapped`] (al piu' `sqrt(2) * d`):
/// meno di `2 * d`. Se `2 * d` supera la precisione dichiarata l'overlay
/// non si esegue: [`MakeValidError::PrecisionInsufficient`]. Sotto la
/// precisione i vertici possono spostarsi e le feature piu' sottili possono
/// fondersi o sparire: errore dichiarato, non un rifiuto.
///
/// Prima della griglia, la spaziatura dei `f64` alle coordinate degli
/// operandi ([`super::precision::coordinate_abbastanza_fitte`]): una
/// coordinata riportata dalla griglia si arrotonda al `f64` piu' vicino, e
/// a `2^52` l'arrotondamento da solo vale mezza unita'.
///
/// Gli agganci interni di `i_overlay` durante lo split dei segmenti (raggio
/// che cresce a ogni giro, `split::snap_radius`) non sono nel bilancio: li
/// limita il controllo finale [`checked_displacement`].
fn checked_grid(normalizer: OverlayNormalizer, precision: f64) -> Result<(), MakeValidError> {
    let magnitude = [
        normalizer.minimum_x,
        normalizer.minimum_x + normalizer.span_x,
        normalizer.minimum_y,
        normalizer.minimum_y + normalizer.span_y,
    ]
    .into_iter()
    .fold(0.0_f64, |massimo, value| massimo.max(value.abs()));
    if !super::precision::coordinate_abbastanza_fitte(magnitude, precision)
        || 2.0 * normalizer.grid_diagonal() > precision
    {
        return Err(MakeValidError::PrecisionInsufficient);
    }
    Ok(())
}

/// Un lato d'ingresso degli operandi, indicizzato per il controllo finale.
struct OperandEdge {
    start: Coord<f64>,
    end: Coord<f64>,
}

impl RTreeObject for OperandEdge {
    type Envelope = AABB<[f64; 2]>;

    fn envelope(&self) -> Self::Envelope {
        AABB::from_corners(
            [self.start.x.min(self.end.x), self.start.y.min(self.end.y)],
            [self.start.x.max(self.end.x), self.start.y.max(self.end.y)],
        )
    }
}

/// Controllo finale di ogni overlay: ogni vertice dell'output sta entro la
/// precisione da un lato d'ingresso di uno dei due operandi, con il margine
/// d'arrotondamento di [`super::precision::punto_entro_segmento`];
/// altrimenti [`MakeValidError::PrecisionInsufficient`].
///
/// Limita lo spostamento dei vertici qualunque cosa abbia fatto `i_overlay`
/// dentro (gli agganci con raggio `2^(k/2)` passi al giro `k` non hanno un
/// tetto a priori). Non dice che un vertice sta sul lato **giusto** ne' che
/// un lato dell'output segue il linework fra i suoi estremi: vedi README
/// («Limiti dichiarati»). I lati stanno in un `RTree`: ogni vertice
/// interroga solo quelli nel suo quadrato di lato `2p`.
fn checked_displacement(
    output: &MultiPolygon<f64>,
    left: &MultiPolygon<f64>,
    right: &MultiPolygon<f64>,
    precision: f64,
) -> Result<(), MakeValidError> {
    let edges = polygonal_boundaries(left)
        .chain(polygonal_boundaries(right))
        .map(|line| OperandEdge {
            start: line.start,
            end: line.end,
        })
        .collect::<Vec<_>>();
    let index = RTree::bulk_load(edges);
    for vertex in output.coords_iter() {
        let query = AABB::from_corners(
            [vertex.x - precision, vertex.y - precision],
            [vertex.x + precision, vertex.y + precision],
        );
        let near = index.locate_in_envelope_intersecting(&query).any(|edge| {
            super::precision::punto_entro_segmento(vertex, edge.start, edge.end, precision)
        });
        if !near {
            return Err(MakeValidError::PrecisionInsufficient);
        }
    }
    Ok(())
}

/// Riporta l'output normalizzato nelle coordinate originali e ne verifica lo
/// spostamento.
fn finished_overlay(
    normalizer: OverlayNormalizer,
    normalized: &MultiPolygon<f64>,
    left: &MultiPolygon<f64>,
    right: &MultiPolygon<f64>,
    precision: f64,
) -> Result<MultiPolygon<f64>, MakeValidError> {
    let restored = normalizer.restore_multi_snapped(normalized, left, right);
    checked_displacement(&restored, left, right, precision)?;
    Ok(restored)
}

fn normalized_union(
    left: &MultiPolygon<f64>,
    right: &MultiPolygon<f64>,
    precision: f64,
) -> Result<MultiPolygon<f64>, MakeValidError> {
    if !normalized_intersects(left, right) {
        let mut polygons = left.0.clone();
        polygons.extend(right.0.iter().cloned());
        return Ok(MultiPolygon::new(polygons));
    }
    let normalizer = OverlayNormalizer::new(left, right);
    checked_grid(normalizer, precision)?;
    let normalized = normalizer
        .map_multi(left, OverlayNormalizer::normalize)
        .union(&normalizer.map_multi(right, OverlayNormalizer::normalize));
    finished_overlay(normalizer, &normalized, left, right, precision)
}

fn normalized_difference(
    left: &MultiPolygon<f64>,
    right: &MultiPolygon<f64>,
    precision: f64,
) -> Result<MultiPolygon<f64>, MakeValidError> {
    let normalizer = OverlayNormalizer::new(left, right);
    checked_grid(normalizer, precision)?;
    let normalized = normalizer
        .map_multi(left, OverlayNormalizer::normalize)
        .difference(&normalizer.map_multi(right, OverlayNormalizer::normalize));
    finished_overlay(normalizer, &normalized, left, right, precision)
}

fn normalized_xor(
    left: &MultiPolygon<f64>,
    right: &MultiPolygon<f64>,
    precision: f64,
) -> Result<MultiPolygon<f64>, MakeValidError> {
    let normalizer = OverlayNormalizer::new(left, right);
    checked_grid(normalizer, precision)?;
    let normalized = normalizer
        .map_multi(left, OverlayNormalizer::normalize)
        .xor(&normalizer.map_multi(right, OverlayNormalizer::normalize));
    finished_overlay(normalizer, &normalized, left, right, precision)
}

/// Se i due operandi si toccano, deciso sulle coordinate **originali** con i
/// predicati esatti di `geo` (`orient2d` esatto). Il laboratorio lo decideva
/// sulle coordinate normalizzate, dove l'arrotondamento puo' far toccare due
/// feature disgiunte o separare due che si toccano: in `structure_polygon`
/// questo sceglie fra buco da sottrarre e buco da promuovere, e un buco
/// esterno sottratto invece che promosso sparirebbe in silenzio. Il nome
/// resta quello della sorgente.
fn normalized_intersects(left: &MultiPolygon<f64>, right: &MultiPolygon<f64>) -> bool {
    left.intersects(right)
}

fn merge_polygon(
    area: &mut MultiPolygon<f64>,
    polygon: &Polygon<f64>,
    precision: f64,
) -> Result<(), MakeValidError> {
    if area.0.is_empty() {
        *area = MultiPolygon::new(vec![polygon.clone()]);
    } else {
        *area = normalized_union(area, &MultiPolygon::new(vec![polygon.clone()]), precision)?;
    }
    Ok(())
}

fn merge_multipolygon(
    area: &mut MultiPolygon<f64>,
    polygons: &MultiPolygon<f64>,
    precision: f64,
) -> Result<(), MakeValidError> {
    if area.0.is_empty() {
        *area = polygons.clone();
    } else if !polygons.0.is_empty() {
        *area = normalized_union(area, polygons, precision)?;
    }
    Ok(())
}

fn cleaned_coordinates(line: &LineString<f64>) -> Vec<Coord<f64>> {
    let mut coordinates = Vec::with_capacity(line.0.len());
    for coordinate in &line.0 {
        if coordinates.last() != Some(coordinate) {
            coordinates.push(*coordinate);
        }
    }
    coordinates
}

fn fix_line_element(line: &LineString<f64>, keep_collapsed: bool) -> Option<Geometry<f64>> {
    let coordinates = cleaned_coordinates(line);
    match coordinates.as_slice() {
        [coordinate] if keep_collapsed => {
            Some(Geometry::Point(Point::new(coordinate.x, coordinate.y)))
        }
        [] | [_] => None,
        _ => Some(Geometry::LineString(LineString::new(coordinates))),
    }
}

fn fix_line(line: &LineString<f64>, keep_collapsed: bool) -> Geometry<f64> {
    fix_line_element(line, keep_collapsed)
        .unwrap_or_else(|| Geometry::LineString(LineString::new(Vec::new())))
}

fn geometry_from_components(mut components: Vec<Geometry<f64>>) -> Geometry<f64> {
    if components.len() == 1 {
        components
            .pop()
            .unwrap_or_else(|| Geometry::GeometryCollection(GeometryCollection::empty()))
    } else {
        Geometry::GeometryCollection(GeometryCollection::new_from(components))
    }
}

fn structure_polygon(
    polygon: &Polygon<f64>,
    keep_collapsed: bool,
    limits: MakeValidLimits,
    precision: f64,
) -> Result<Option<Geometry<f64>>, MakeValidError> {
    let mut fixed = fixed_ring(polygon.exterior(), limits, precision)?;
    if fixed.0.is_empty() {
        return Ok(if keep_collapsed {
            fix_line_element(polygon.exterior(), true)
        } else {
            None
        });
    }
    let shell = fixed.clone();
    let mut subtractive_holes = MultiPolygon::empty();
    let mut promoted_holes = MultiPolygon::empty();
    for hole in polygon.interiors() {
        let fixed_hole = fixed_ring(hole, limits, precision)?;
        if normalized_intersects(&shell, &fixed_hole) {
            subtractive_holes = normalized_union(&subtractive_holes, &fixed_hole, precision)?;
        } else {
            promoted_holes = normalized_union(&promoted_holes, &fixed_hole, precision)?;
        }
    }
    if !subtractive_holes.0.is_empty() {
        fixed = normalized_difference(&fixed, &subtractive_holes, precision)?;
    }
    if !promoted_holes.0.is_empty() {
        fixed = normalized_union(&fixed, &promoted_holes, precision)?;
    }
    Ok(Some(as_polygonal_geometry(fixed)))
}

fn normalize_collapsed_union(
    components: Vec<Geometry<f64>>,
    limits: MakeValidLimits,
    precision: f64,
) -> Result<Vec<Geometry<f64>>, MakeValidError> {
    let mut lines = Vec::new();
    let mut output = Vec::new();
    for component in components {
        match component {
            Geometry::LineString(line) => lines.push(line),
            Geometry::MultiLineString(multi) => lines.extend(multi.0),
            other => output.push(other),
        }
    }
    if lines.is_empty() {
        return Ok(output);
    }
    let result = polygonize_linework_rust(
        &Geometry::MultiLineString(MultiLineString::new(lines)),
        PolygonizeOptions {
            node_input: true,
            require_complete: false,
            limits: polygonize_limits(limits),
            precision,
        },
    )?;
    if !result.polygons.is_empty() {
        output.push(as_polygonal_geometry(MultiPolygon::new(result.polygons)));
    }
    let residuals = result
        .cut_edges
        .into_iter()
        .chain(result.dangles)
        .chain(result.invalid_ring_lines)
        .collect::<Vec<_>>();
    if let Some(lines) = line_geometry(residuals) {
        output.push(lines);
    }
    Ok(output)
}

fn structure(
    geometry: &Geometry<f64>,
    keep_collapsed: bool,
    limits: MakeValidLimits,
    precision: f64,
) -> Result<Geometry<f64>, MakeValidError> {
    let output = match geometry {
        Geometry::Point(point) => Geometry::Point(*point),
        Geometry::Line(line) => Geometry::Line(*line),
        Geometry::LineString(line) => fix_line(line, keep_collapsed),
        Geometry::MultiPoint(points) => Geometry::MultiPoint(points.clone()),
        Geometry::MultiLineString(lines) => {
            let mut components = Vec::new();
            for line in &lines.0 {
                if let Some(fixed) = fix_line_element(line, keep_collapsed) {
                    components.push(fixed);
                }
            }
            if components
                .iter()
                .all(|value| matches!(value, Geometry::LineString(_)))
            {
                let fixed_lines = components
                    .into_iter()
                    .filter_map(|value| match value {
                        Geometry::LineString(line) => Some(line),
                        _ => None,
                    })
                    .collect();
                Geometry::MultiLineString(MultiLineString::new(fixed_lines))
            } else {
                geometry_from_components(components)
            }
        }
        Geometry::Polygon(polygon) => {
            structure_polygon(polygon, keep_collapsed, limits, precision)?.unwrap_or_else(|| {
                Geometry::Polygon(Polygon::new(LineString::new(Vec::new()), vec![]))
            })
        }
        Geometry::MultiPolygon(polygons) => {
            let mut area = MultiPolygon::empty();
            let mut collapsed = Vec::new();
            for polygon in &polygons.0 {
                match structure_polygon(polygon, keep_collapsed, limits, precision)? {
                    Some(Geometry::Polygon(polygon)) => {
                        merge_polygon(&mut area, &polygon, precision)?;
                    }
                    Some(Geometry::MultiPolygon(polygons)) => {
                        merge_multipolygon(&mut area, &polygons, precision)?;
                    }
                    Some(other) => collapsed.push(other),
                    None => {}
                }
            }
            if !collapsed.is_empty() && (!area.0.is_empty() || collapsed.len() > 1) {
                collapsed = normalize_collapsed_union(collapsed, limits, precision)?;
            }
            if !area.0.is_empty() {
                collapsed.insert(0, as_polygonal_geometry(area));
            }
            geometry_from_components(collapsed)
        }
        Geometry::GeometryCollection(collection) => {
            let mut fixed = Vec::with_capacity(collection.0.len());
            for child in &collection.0 {
                // GeometryFixer di GEOS ripara i figli di una collection con
                // la configurazione predefinita e non propaga keep_collapsed.
                fixed.push(make_valid_geometry_rust_impl(
                    child,
                    RepairMethod::Structure,
                    false,
                    limits,
                    precision,
                )?);
            }
            Geometry::GeometryCollection(GeometryCollection::new_from(fixed))
        }
        Geometry::Rect(rectangle) => Geometry::Rect(*rectangle),
        Geometry::Triangle(triangle) => Geometry::Triangle(*triangle),
    };
    output
        .check_validation()
        .map_err(|error| MakeValidError::InvalidOutput(error.to_string()))?;
    Ok(output)
}

fn collect_polygon_elements<'a>(
    geometry: &'a Geometry<f64>,
    polygons: &mut Vec<&'a Polygon<f64>>,
) -> Result<(), MakeValidError> {
    match geometry {
        Geometry::Polygon(polygon) => polygons.push(polygon),
        Geometry::MultiPolygon(multi) => {
            for polygon in &multi.0 {
                polygons.push(polygon);
            }
        }
        other => return Err(MakeValidError::UnsupportedGeometry(geometry_type(other))),
    }
    Ok(())
}

fn ring_segment_signature(ring: &LineString<f64>) -> Result<Vec<SegmentKey>, MakeValidError> {
    let mut signature = Vec::new();
    signature
        .try_reserve_exact(ring.0.len().saturating_sub(1))
        .map_err(|_| MakeValidError::AllocationFailed("firma dell'anello"))?;
    for segment in ring.lines() {
        if let Some(key) = SegmentKey::new(segment.start, segment.end) {
            signature.push(key);
        }
    }
    signature.sort_unstable();
    signature.dedup();
    Ok(signature)
}

fn polygonal_boundaries(polygons: &MultiPolygon<f64>) -> impl Iterator<Item = geo::Line<f64>> + '_ {
    polygons.0.iter().flat_map(|polygon| {
        std::iter::once(polygon.exterior())
            .chain(polygon.interiors())
            .flat_map(LineString::lines)
    })
}

fn shares_collinear_boundary(left: &MultiPolygon<f64>, right: &MultiPolygon<f64>) -> bool {
    polygonal_boundaries(left).any(|left_segment| {
        polygonal_boundaries(right).any(|right_segment| {
            matches!(
                line_intersection(left_segment, right_segment),
                Some(LineIntersection::Collinear { intersection })
                    if intersection.start != intersection.end
            )
        })
    })
}

fn add_line_segments(
    line: &LineString<f64>,
    segments: &mut BTreeMap<SegmentKey, (Coord<f64>, Coord<f64>)>,
) {
    for segment in line.lines() {
        if let Some(key) = SegmentKey::new(segment.start, segment.end) {
            segments.entry(key).or_insert((segment.start, segment.end));
        }
    }
}

fn add_polygon_segments(
    polygon: &Polygon<f64>,
    segments: &mut BTreeMap<SegmentKey, (Coord<f64>, Coord<f64>)>,
) {
    add_line_segments(polygon.exterior(), segments);
    for ring in polygon.interiors() {
        add_line_segments(ring, segments);
    }
}

/// Dove sta un punto rispetto a un segmento, per la classificazione di
/// `linework`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum OnSegment {
    /// Sul segmento: esattamente (`orient2d` esatto e contenimento esatto
    /// nell'inviluppo) o entro la banda della precisione dichiarata.
    On,
    /// Fuori dalla banda.
    Off,
}

/// Banda della classificazione di `linework`: la precisione dichiarata `p`
/// (unita' delle coordinate; README, «Limiti dichiarati»).
///
/// Il laboratorio decideva «sul bordo dell'area» con una tolleranza
/// `64 * EPSILON * |coordinata|`, circa `1.5e-5` a `2^30` qualunque fosse
/// l'unita': piu' larga dei buchi sottili che GEOS conserva, che finivano
/// scartati come bordo (campagna differenziale traslata di `2^30`). Ora un
/// punto e' sul segmento se lo e' esattamente o se dista al piu' `p`: una
/// linea piu' vicina di `p` al bordo dell'area ne fa parte (errore
/// dichiarato), una piu' lontana resta.
#[derive(Clone, Copy, Debug)]
struct Band {
    x: f64,
    y: f64,
}

impl Band {
    const fn new(precision: f64) -> Self {
        Self {
            x: precision,
            y: precision,
        }
    }

    fn classify(self, point: Coord<f64>, start: Coord<f64>, end: Coord<f64>) -> OnSegment {
        if RobustKernel::orient2d(start, end, point) == Orientation::Collinear
            && point.x >= start.x.min(end.x)
            && point.x <= start.x.max(end.x)
            && point.y >= start.y.min(end.y)
            && point.y <= start.y.max(end.y)
        {
            return OnSegment::On;
        }
        // Distanza nella metrica scalata per asse, relativa a `start`.
        let px = (point.x - start.x) / self.x;
        let py = (point.y - start.y) / self.y;
        let dx = (end.x - start.x) / self.x;
        let dy = (end.y - start.y) / self.y;
        let length_squared = dx * dx + dy * dy;
        let t = if length_squared > 0.0 {
            ((px * dx + py * dy) / length_squared).clamp(0.0, 1.0)
        } else {
            0.0
        };
        if (px - t * dx).hypot(py - t * dy) <= 1.0 {
            OnSegment::On
        } else {
            OnSegment::Off
        }
    }
}

fn boundary_segments(area: &MultiPolygon<f64>) -> impl Iterator<Item = Line<f64>> + '_ {
    area.0.iter().flat_map(|polygon| {
        std::iter::once(polygon.exterior())
            .chain(polygon.interiors())
            .flat_map(LineString::lines)
    })
}

/// Il segmento sta su un lato del bordo dell'area (entrambi gli estremi
/// sopra lo stesso lato, nella banda)?
fn segment_is_area_boundary(
    start: Coord<f64>,
    end: Coord<f64>,
    area: &MultiPolygon<f64>,
    band: Band,
) -> bool {
    boundary_segments(area).any(|boundary| {
        band.classify(start, boundary.start, boundary.end) == OnSegment::On
            && band.classify(end, boundary.start, boundary.end) == OnSegment::On
    })
}

/// La coordinata sta su un lato dell'area o su una linea residua, nella
/// banda?
fn coordinate_is_represented(
    coordinate: Coord<f64>,
    area: &MultiPolygon<f64>,
    lines: &[LineString<f64>],
    band: Band,
) -> bool {
    boundary_segments(area)
        .chain(lines.iter().flat_map(LineString::lines))
        .any(|segment| band.classify(coordinate, segment.start, segment.end) == OnSegment::On)
}

fn validate_linework_output(geometry: &Geometry<f64>) -> Result<(), MakeValidError> {
    match geometry {
        Geometry::MultiPolygon(polygons) => {
            for polygon in &polygons.0 {
                Geometry::Polygon(polygon.clone())
                    .check_validation()
                    .map_err(|error| MakeValidError::InvalidOutput(error.to_string()))?;
            }
            Ok(())
        }
        Geometry::GeometryCollection(collection) => {
            for child in &collection.0 {
                validate_linework_output(child)?;
            }
            Ok(())
        }
        _ => geometry
            .check_validation()
            .map_err(|error| MakeValidError::InvalidOutput(error.to_string())),
    }
}

fn line_geometry(mut lines: Vec<LineString<f64>>) -> Option<Geometry<f64>> {
    if lines.len() == 1 {
        lines.pop().map(Geometry::LineString)
    } else if lines.is_empty() {
        None
    } else {
        Some(Geometry::MultiLineString(MultiLineString::new(lines)))
    }
}

fn point_geometry(mut points: Vec<Point<f64>>) -> Option<Geometry<f64>> {
    if points.len() == 1 {
        points.pop().map(Geometry::Point)
    } else if points.is_empty() {
        None
    } else {
        Some(Geometry::MultiPoint(MultiPoint::new(points)))
    }
}

fn linework(
    geometry: &Geometry<f64>,
    limits: MakeValidLimits,
    precision: f64,
) -> Result<Geometry<f64>, MakeValidError> {
    match geometry {
        Geometry::GeometryCollection(collection) => {
            let mut fixed = Vec::with_capacity(collection.0.len());
            for child in &collection.0 {
                fixed.push(make_valid_geometry_rust_impl(
                    child,
                    RepairMethod::Linework,
                    true,
                    limits,
                    precision,
                )?);
            }
            return Ok(Geometry::GeometryCollection(GeometryCollection::new_from(
                fixed,
            )));
        }
        Geometry::LineString(line) => return Ok(fix_line(line, true)),
        Geometry::MultiLineString(_) => return structure(geometry, true, limits, precision),
        Geometry::Polygon(_) | Geometry::MultiPolygon(_) => {}
        other => return Err(MakeValidError::UnsupportedGeometry(geometry_type(other))),
    }

    let mut polygons = Vec::new();
    collect_polygon_elements(geometry, &mut polygons)?;
    let band = Band::new(precision);
    let original_coordinates = polygons
        .iter()
        .flat_map(|polygon| {
            std::iter::once(polygon.exterior())
                .chain(polygon.interiors())
                .flat_map(|ring| ring.0.iter().copied())
        })
        .map(CoordKey::new)
        .collect::<BTreeSet<_>>();
    let mut boundaries = Vec::new();
    for polygon in &polygons {
        boundaries.push(polygon.exterior().clone());
        boundaries.extend(polygon.interiors().iter().cloned());
    }
    let result = polygonize_linework_rust(
        &Geometry::MultiLineString(MultiLineString::new(boundaries)),
        PolygonizeOptions {
            node_input: true,
            require_complete: false,
            limits: polygonize_limits(limits),
            precision,
        },
    )?;
    let mut area = MultiPolygon::empty();
    let mut retain_internal_edges = false;
    for polygon in &polygons {
        let mut seen_rings = BTreeSet::new();
        let shell_signature = ring_segment_signature(polygon.exterior())?;
        seen_rings.insert(shell_signature);
        let shell = fixed_ring(polygon.exterior(), limits, precision)?;
        let mut polygon_area = shell.clone();
        for ring in polygon.interiors() {
            if !seen_rings.insert(ring_segment_signature(ring)?) {
                continue;
            }
            let fixed = fixed_ring(ring, limits, precision)?;
            if fixed.0.is_empty() {
                continue;
            }
            if shares_collinear_boundary(&shell, &fixed) {
                retain_internal_edges = true;
                // Di un buco che condivide un lato con la shell diventa area
                // solo la **sporgenza**, la parte fuori dalla shell, senza
                // soglia: vuota, l'unione non si fa. Il laboratorio univa il
                // buco intero se l'area della differenza superava una soglia
                // globale, e una sporgenza di 1 m^2 da un buco di perimetro
                // 3.6 km restava sotto; unire il buco intero, d'altra parte,
                // reinseriva i buchi gia' sottratti che esso contiene (esito
                // dipendente dall'ordine dei buchi).
                let protrusion = normalized_difference(&fixed, &shell, precision)?;
                if !protrusion.0.is_empty() {
                    polygon_area = normalized_union(&polygon_area, &protrusion, precision)?;
                }
            } else {
                polygon_area = normalized_xor(&polygon_area, &fixed, precision)?;
            }
        }
        if !polygon_area.0.is_empty() {
            if shares_collinear_boundary(&area, &polygon_area) {
                retain_internal_edges = true;
                area = normalized_union(&area, &polygon_area, precision)?;
            } else {
                area = normalized_xor(&area, &polygon_area, precision)?;
            }
        }
    }
    let mut all_segments = BTreeMap::new();
    if retain_internal_edges {
        for polygon in &result.polygons {
            add_polygon_segments(polygon, &mut all_segments);
        }
    }
    let include_cut_edges = retain_internal_edges || area.0.is_empty();
    for line in &result.dangles {
        add_line_segments(line, &mut all_segments);
    }
    if include_cut_edges {
        for line in result.cut_edges.iter().chain(&result.invalid_ring_lines) {
            add_line_segments(line, &mut all_segments);
        }
    }
    let mut lines = Vec::new();
    for (start, end) in all_segments.into_values() {
        if !segment_is_area_boundary(start, end, &area, band) {
            lines
                .try_reserve(1)
                .map_err(|_| MakeValidError::AllocationFailed("residui linework"))?;
            lines.push(LineString::new(vec![start, end]));
        }
    }
    let mut collapse_points = Vec::new();
    for key in &original_coordinates {
        let coordinate = Coord {
            x: f64::from_bits(key.x),
            y: f64::from_bits(key.y),
        };
        if !coordinate_is_represented(coordinate, &area, &lines, band) {
            collapse_points.push(Point::from(coordinate));
        }
    }

    let mut components = Vec::new();
    if !area.0.is_empty() {
        components.push(as_polygonal_geometry(area));
    }
    if let Some(line_output) = line_geometry(lines) {
        components.push(line_output);
    }
    if let Some(point_output) = point_geometry(collapse_points) {
        components.push(point_output);
    }
    let output = geometry_from_components(components);
    validate_linework_output(&output)?;
    Ok(output)
}

/// Ripara una geometria senza GEOS.
///
/// `Structure` ripara separatamente shell e anelli interni, quindi applica
/// overlay Rust. `Linework` noda tutto il bordo, estrae le facce e conserva i
/// residui lineari o puntuali. Gli input gia' validi passano invariati.
///
/// # Errors
///
/// Restituisce un errore per payload non finiti/strutturalmente malformati,
/// tipi non supportati, polygonize fallita o output ancora invalido.
fn make_valid_geometry_rust_impl(
    geometry: &Geometry<f64>,
    method: RepairMethod,
    keep_collapsed: bool,
    limits: MakeValidLimits,
    precision: f64,
) -> Result<Geometry<f64>, MakeValidError> {
    if !(precision.is_finite() && precision > 0.0) {
        return Err(MakeValidError::InvalidPrecision);
    }
    validate_structure(geometry)?;
    // Area positiva decisa in modo esatto (deviazione dichiarata in `super`:
    // il laboratorio usava `unsigned_area() > 0.0`, che la cancellazione
    // puo' azzerare). Un esito non decidibile e' un errore, mai una
    // decisione: tradurlo in «non positiva» avviava la riparazione, e
    // l'overlay poteva svuotare un poligono valido.
    let positive = |polygon: &Polygon<f64>| {
        super::exact::area_positiva(polygon).map_err(|_| MakeValidError::NumericRange)
    };
    let all_positive = |polygons: &[Polygon<f64>]| -> Result<bool, MakeValidError> {
        for polygon in polygons {
            if !positive(polygon)? {
                return Ok(false);
            }
        }
        Ok(true)
    };
    let non_degenerate_area = match geometry {
        Geometry::Polygon(polygon) => positive(polygon)?,
        Geometry::MultiPolygon(polygons) => all_positive(&polygons.0)?,
        Geometry::GeometryCollection(collection) => {
            let mut all = true;
            for child in &collection.0 {
                let child_positive = match child {
                    Geometry::Polygon(polygon) => positive(polygon)?,
                    Geometry::MultiPolygon(polygons) => all_positive(&polygons.0)?,
                    _ => true,
                };
                if !child_positive {
                    all = false;
                    break;
                }
            }
            all
        }
        _ => true,
    };
    if non_degenerate_area && geometry.check_validation().is_ok() {
        checked_limits_output(geometry, limits)?;
        return Ok(geometry.clone());
    }
    let output = match method {
        RepairMethod::Linework => linework(geometry, limits, precision),
        RepairMethod::Structure => structure(geometry, keep_collapsed, limits, precision),
    }?;
    checked_limits_output(&output, limits)?;
    Ok(output)
}

/// Ripara una geometria senza GEOS usando limiti non restrittivi.
///
/// `precision` e' la precisione dichiarata nelle unita' delle coordinate (1
/// cm a terra con un CRS, vedi `super::precision`): nessun valore
/// predefinito.
///
/// # Errors
///
/// Restituisce un errore per payload non finiti/strutturalmente malformati,
/// tipi non supportati, polygonize fallita o output ancora invalido;
/// [`MakeValidError::PrecisionInsufficient`] se la griglia di un overlay e'
/// piu' grossa della precisione.
pub fn make_valid_geometry_rust(
    geometry: &Geometry<f64>,
    method: RepairMethod,
    keep_collapsed: bool,
    precision: f64,
) -> Result<Geometry<f64>, MakeValidError> {
    make_valid_geometry_rust_impl(
        geometry,
        method,
        keep_collapsed,
        MakeValidLimits::unlimited(),
        precision,
    )
}

/// Variante limitata di [`make_valid_geometry_rust`].
///
/// Il budget di noding usa conservativamente il quadrato del numero totale
/// di segmenti. Questo domina la somma dei lavori quadratici eseguiti sui
/// singoli anelli e impedisce di aggirare il limite spezzando l'input in una
/// collection.
///
/// # Errors
///
/// Oltre agli errori della riparazione, restituisce un errore prima del
/// kernel se coordinate o lavoro eccedono i limiti e dopo il kernel se
/// geometrie o coordinate di output eccedono i limiti dichiarati.
pub fn make_valid_geometry_rust_with_limits(
    geometry: &Geometry<f64>,
    method: RepairMethod,
    keep_collapsed: bool,
    limits: MakeValidLimits,
    precision: f64,
) -> Result<Geometry<f64>, MakeValidError> {
    checked_preflight(geometry, limits)?;
    let output =
        make_valid_geometry_rust_impl(geometry, method, keep_collapsed, limits, precision)?;
    checked_limits_output(&output, limits)?;
    Ok(output)
}

/// Variante per l'integrazione controllata che richiede tutti i budget.
///
/// # Errors
///
/// Restituisce [`MakeValidError::UnboundedLimitConfiguration`] se almeno un
/// limite e' lasciato a [`u64::MAX`]; altrimenti propaga gli errori di
/// [`make_valid_geometry_rust_with_limits`].
pub fn make_valid_geometry_rust_bounded(
    geometry: &Geometry<f64>,
    method: RepairMethod,
    keep_collapsed: bool,
    limits: MakeValidLimits,
    precision: f64,
) -> Result<Geometry<f64>, MakeValidError> {
    if !limits.is_fully_bounded() {
        return Err(MakeValidError::UnboundedLimitConfiguration);
    }
    make_valid_geometry_rust_with_limits(geometry, method, keep_collapsed, limits, precision)
}

#[cfg(test)]
mod tests {
    use super::*;
    use geo::{polygon, Area};

    /// Controllo finale degli overlay: un vertice d'uscita a 2 cm da ogni
    /// lato degli operandi (quello che un aggancio interno di `i_overlay`
    /// con raggio cresciuto potrebbe produrre) e' un errore; a meno di 1 cm
    /// passa.
    #[test]
    fn overlay_vertex_beyond_the_precision_is_rejected() {
        let left = MultiPolygon::new(vec![polygon![
            (x: 0.0, y: 0.0), (x: 10.0, y: 0.0), (x: 10.0, y: 10.0),
            (x: 0.0, y: 10.0), (x: 0.0, y: 0.0)
        ]]);
        let right = MultiPolygon::new(vec![polygon![
            (x: 5.0, y: 5.0), (x: 15.0, y: 5.0), (x: 15.0, y: 15.0),
            (x: 5.0, y: 15.0), (x: 5.0, y: 5.0)
        ]]);
        let spostato = |scarto: f64| {
            MultiPolygon::new(vec![polygon![
                (x: 0.0, y: 0.0), (x: 10.0, y: 0.0), (x: 10.0, y: 5.0),
                (x: 5.0 + scarto, y: 5.0 + scarto), (x: 0.0, y: 10.0), (x: 0.0, y: 0.0)
            ]])
        };
        assert!(checked_displacement(&spostato(0.0), &left, &right, 0.01).is_ok());
        assert!(checked_displacement(&spostato(0.005), &left, &right, 0.01).is_ok());
        assert!(matches!(
            checked_displacement(&spostato(0.02), &left, &right, 0.01),
            Err(MakeValidError::PrecisionInsufficient)
        ));
    }

    /// Precisione dei test del laboratorio: coordinate astratte fino a
    /// qualche decina di unita', un milionesimo di unita'.
    const PRECISION: f64 = 1e-6;

    #[test]
    fn repairs_bow_tie_with_both_methods() -> Result<(), MakeValidError> {
        let input = Geometry::Polygon(polygon![
            (x: 0.0, y: 0.0), (x: 2.0, y: 2.0),
            (x: 0.0, y: 2.0), (x: 2.0, y: 0.0),
            (x: 0.0, y: 0.0)
        ]);
        for method in [RepairMethod::Structure, RepairMethod::Linework] {
            let output = make_valid_geometry_rust(&input, method, false, PRECISION)?;
            if (output.unsigned_area() - 2.0).abs() > 1e-12 {
                return Err(MakeValidError::InvalidOutput(format!(
                    "area inattesa: {}",
                    output.unsigned_area()
                )));
            }
        }
        Ok(())
    }

    #[test]
    fn limited_api_fails_before_or_after_the_kernel() -> Result<(), MakeValidError> {
        let input = Geometry::Polygon(polygon![
            (x: 0.0, y: 0.0), (x: 2.0, y: 2.0),
            (x: 0.0, y: 2.0), (x: 2.0, y: 0.0),
            (x: 0.0, y: 0.0)
        ]);
        let coordinate_limited = make_valid_geometry_rust_with_limits(
            &input,
            RepairMethod::Structure,
            false,
            MakeValidLimits {
                max_input_coordinates: 4,
                ..MakeValidLimits::unlimited()
            },
            PRECISION,
        );
        if !matches!(
            coordinate_limited,
            Err(MakeValidError::CoordinateLimit { .. })
        ) {
            return Err(MakeValidError::InvalidOutput(
                "limite coordinate input non applicato".to_owned(),
            ));
        }
        let work_limited = make_valid_geometry_rust_with_limits(
            &input,
            RepairMethod::Linework,
            false,
            MakeValidLimits {
                max_noding_work: 15,
                ..MakeValidLimits::unlimited()
            },
            PRECISION,
        );
        if !matches!(work_limited, Err(MakeValidError::WorkLimit { .. })) {
            return Err(MakeValidError::InvalidOutput(
                "limite lavoro non applicato".to_owned(),
            ));
        }
        let output_limited = make_valid_geometry_rust_with_limits(
            &input,
            RepairMethod::Structure,
            false,
            MakeValidLimits {
                max_output_coordinates: 7,
                ..MakeValidLimits::unlimited()
            },
            PRECISION,
        );
        if !matches!(output_limited, Err(MakeValidError::OutputLimit { .. })) {
            return Err(MakeValidError::InvalidOutput(
                "limite coordinate output non applicato".to_owned(),
            ));
        }
        let geometry_limited = make_valid_geometry_rust_with_limits(
            &input,
            RepairMethod::Structure,
            false,
            MakeValidLimits {
                max_output_geometries: 1,
                ..MakeValidLimits::unlimited()
            },
            PRECISION,
        );
        if !matches!(geometry_limited, Err(MakeValidError::OutputLimit { .. })) {
            return Err(MakeValidError::InvalidOutput(
                "limite geometrie output non applicato".to_owned(),
            ));
        }
        for method in [RepairMethod::Structure, RepairMethod::Linework] {
            let first = make_valid_geometry_rust(&input, method, true, PRECISION)?;
            let second = make_valid_geometry_rust(&input, method, true, PRECISION)?;
            if first != second {
                return Err(MakeValidError::InvalidOutput(
                    "esecuzioni identiche non deterministiche".to_owned(),
                ));
            }
        }
        let non_finite = make_valid_geometry_rust(
            &Geometry::Point(Point::new(f64::NAN, 0.0)),
            RepairMethod::Structure,
            false,
            PRECISION,
        );
        if !matches!(non_finite, Err(MakeValidError::NonFiniteCoordinate)) {
            return Err(MakeValidError::InvalidOutput(
                "coordinata non finita accettata".to_owned(),
            ));
        }
        Ok(())
    }

    /// Triangolo valido con area doppia esatta 1 ma `2^54 - 2^54 = 0` nella
    /// somma in `f64`: il controllo "area non nulla" del passthrough deve
    /// essere esatto, o la geometria valida passa dalla riparazione.
    #[test]
    fn valid_triangle_with_cancelled_area_passes_through() -> Result<(), MakeValidError> {
        let big = 134_217_728.0; // 2^27
        let input = Geometry::Polygon(Polygon::new(
            LineString::from(vec![
                (0.0, 0.0),
                (big + 1.0, big),
                (big, big - 1.0),
                (0.0, 0.0),
            ]),
            vec![],
        ));
        if input.check_validation().is_err() {
            return Err(MakeValidError::InvalidOutput(
                "fixture non valida".to_owned(),
            ));
        }
        for method in [RepairMethod::Structure, RepairMethod::Linework] {
            let output = make_valid_geometry_rust(&input, method, false, PRECISION)?;
            if output != input {
                return Err(MakeValidError::InvalidOutput(format!(
                    "{method:?}: passthrough mancato"
                )));
            }
        }
        Ok(())
    }

    fn square(minimum: f64, maximum_x: f64, minimum_y: f64, maximum_y: f64) -> Polygon<f64> {
        Polygon::new(
            LineString::from(vec![
                (minimum, minimum_y),
                (maximum_x, minimum_y),
                (maximum_x, maximum_y),
                (minimum, maximum_y),
                (minimum, minimum_y),
            ]),
            vec![],
        )
    }

    /// Il controllo della griglia: `span * 2^-30` per asse contro la
    /// precisione. Con 1 cm in metri, 20.000 km di estensione superano la
    /// griglia (circa 1,9 cm), 1.300 km no.
    #[test]
    fn grid_check_refuses_only_extents_coarser_than_the_precision() {
        let normalizer = |span: f64| OverlayNormalizer {
            minimum_x: 0.0,
            minimum_y: 0.0,
            span_x: span,
            span_y: 1.0,
        };
        assert!(matches!(
            checked_grid(normalizer(20_000_000.0), 0.01),
            Err(MakeValidError::PrecisionInsufficient)
        ));
        assert!(checked_grid(normalizer(1_300_000.0), 0.01).is_ok());
    }

    /// Primo controesempio della terza revisione, in metri con 1 cm: il
    /// buco largo `2^-40` m e' sotto la precisione e puo' sparire; il resto
    /// resta entro perimetro per precisione.
    #[test]
    fn sub_precision_hole_may_vanish_the_rest_is_kept() -> Result<(), MakeValidError> {
        let shell = MultiPolygon::new(vec![square(0.0, 1.0, 0.0, 1.0)]);
        let hole = MultiPolygon::new(vec![square(0.5, 0.5 + 2_f64.powi(-40), 0.25, 0.75)]);
        let result = normalized_difference(&shell, &hole, 0.01)?;
        let tolerance = 4.0 * 0.01;
        if (result.unsigned_area() - 1.0).abs() > tolerance {
            return Err(MakeValidError::InvalidOutput(format!(
                "area {}",
                result.unsigned_area()
            )));
        }
        Ok(())
    }

    /// Secondo controesempio: la cornice larga `2^-27` m accanto al
    /// triangolo e' sotto la precisione e puo' sparire; il triangolo resta.
    #[test]
    fn sub_precision_frame_may_vanish_the_triangle_is_kept() -> Result<(), MakeValidError> {
        let margin = 2_f64.powi(-27);
        let frame = MultiPolygon::new(vec![Polygon::new(
            square(0.0, 1.0, 0.0, 1.0).exterior().clone(),
            vec![square(margin, 1.0 - margin, margin, 1.0 - margin)
                .exterior()
                .clone()],
        )]);
        let triangle = MultiPolygon::new(vec![Polygon::new(
            LineString::from(vec![
                (0.0, 4.0),
                (1024.0, 4.0),
                (1024.0, -1024.0),
                (0.0, 4.0),
            ]),
            vec![],
        )]);
        let result = normalized_xor(&frame, &triangle, 0.01)?;
        let triangle_area = triangle.unsigned_area();
        // Perimetro complessivo circa 3.500 m, per 1 cm.
        let tolerance = 4.0 * 0.01;
        let area = result.unsigned_area();
        if area < triangle_area - 3_500.0 * tolerance
            || area > triangle_area + frame.unsigned_area() + 3_500.0 * tolerance
        {
            return Err(MakeValidError::InvalidOutput(format!("area {area}")));
        }
        Ok(())
    }

    /// Il controesempio di forma della terza revisione: con feature
    /// risolvibili l'overlay si esegue, e i due buchi restano due.
    #[test]
    fn resolvable_difference_keeps_both_holes() -> Result<(), MakeValidError> {
        let shell = MultiPolygon::new(vec![square(0.0, 10.0, 0.0, 10.0)]);
        let holes = MultiPolygon::new(vec![square(2.0, 3.0, 2.0, 3.0), square(4.0, 5.0, 2.0, 3.0)]);
        let result = normalized_difference(&shell, &holes, PRECISION)?;
        let [polygon] = result.0.as_slice() else {
            return Err(MakeValidError::InvalidOutput(
                "un poligono atteso".to_owned(),
            ));
        };
        let expected = Polygon::new(
            square(0.0, 10.0, 0.0, 10.0).exterior().clone(),
            vec![
                square(2.0, 3.0, 2.0, 3.0).exterior().clone(),
                square(4.0, 5.0, 2.0, 3.0).exterior().clone(),
            ],
        );
        if polygon.interiors().len() != 2
            || (polygon.unsigned_area() - expected.unsigned_area()).abs() > 1e-12
        {
            return Err(MakeValidError::InvalidOutput("buchi alterati".to_owned()));
        }
        Ok(())
    }

    /// Buco fuori dalla shell, a un ULP dal suo bordo: sulle coordinate
    /// normalizzate (estensione 3) i due si toccano, sulle originali no.
    /// `STRUCTURE` deve promuoverlo a poligono, non sottrarlo.
    #[test]
    fn hole_one_ulp_outside_the_shell_is_promoted() -> Result<(), MakeValidError> {
        // Bordo della shell e bordo del buco a un ULP: divisi per
        // l'estensione 3 arrotondano allo stesso `f64`.
        let edge = f64::from_bits(0x3FF9_A9A8_0EF2_B725);
        let gap = f64::from_bits(0x3FF9_A9A8_0EF2_B726);
        if (gap / 3.0).to_bits() != (edge / 3.0).to_bits() {
            return Err(MakeValidError::InvalidOutput(
                "la fixture non collide piu' dopo la normalizzazione".to_owned(),
            ));
        }
        let input = Geometry::Polygon(Polygon::new(
            square(0.0, edge, 0.0, 1.0).exterior().clone(),
            vec![square(gap, 3.0, 0.0, 1.0).exterior().clone()],
        ));
        let output = make_valid_geometry_rust(&input, RepairMethod::Structure, false, PRECISION)?;
        let Geometry::MultiPolygon(polygons) = &output else {
            return Err(MakeValidError::InvalidOutput(format!("{output:?}")));
        };
        if polygons.0.len() != 2 {
            return Err(MakeValidError::InvalidOutput(
                "buco non promosso".to_owned(),
            ));
        }
        Ok(())
    }

    #[test]
    fn applies_linework_parity_to_a_crossing_hole() -> Result<(), MakeValidError> {
        let source = Polygon::new(
            LineString::from(vec![
                (0.0, 0.0),
                (4.0, 0.0),
                (4.0, 4.0),
                (0.0, 4.0),
                (0.0, 0.0),
            ]),
            vec![LineString::from(vec![
                (3.0, 1.0),
                (5.0, 1.0),
                (5.0, 3.0),
                (3.0, 3.0),
                (3.0, 1.0),
            ])],
        );
        let shell = fixed_ring(source.exterior(), MakeValidLimits::unlimited(), PRECISION)?;
        let hole = fixed_ring(
            &source.interiors()[0],
            MakeValidLimits::unlimited(),
            PRECISION,
        )?;
        let selected = normalized_xor(&shell, &hole, 1.0)?;
        if (selected.unsigned_area() - 16.0).abs() > 1e-12 {
            return Err(MakeValidError::InvalidOutput(format!(
                "area selezionata={}",
                selected.unsigned_area()
            )));
        }
        Ok(())
    }
}
