//! Riparazione geometrica (`LINEWORK` e `STRUCTURE`) in Rust puro, sopra il
//! polygonize di [`super::polygonize`]; `STRUCTURE` anche sopra l'overlay di
//! `geo`.
//!
//! `LINEWORK` e' la regola di `MakeValid` di GEOS come operazioni esatte
//! sull'insieme dei lati nodati ([`linework`]); `STRUCTURE` quella di
//! `GeometryFixer`, con buchi e parti in ordine canonico. Nessuno dei due
//! dipende dall'ordine di anelli e parti d'ingresso.
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

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use super::polygonize::{
    polygonize_linework_rust, PolygonizeError, PolygonizeLimits, PolygonizeOptions,
};
use geo::algorithm::validation::Validation;
use geo::coordinate_position::{CoordPos, CoordinatePosition};
use geo::kernels::{Kernel, Orientation, RobustKernel};
use geo::{
    BooleanOps, Coord, CoordsIter, Geometry, GeometryCollection, InteriorPoint, Intersects,
    LineString, MultiLineString, MultiPoint, MultiPolygon, Point, Polygon,
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
    // Un anello collassato in un punto non ha area (GEOS: buffer nullo
    // vuoto); il polygonize lo rifiuterebbe come linea invalida.
    if ring
        .0
        .iter()
        .all(|coordinate| ring.0.first().is_some_and(|first| first == coordinate))
    {
        return Ok(MultiPolygon::empty());
    }
    let result = polygonize_linework_rust(
        &Geometry::LineString(ring.clone()),
        PolygonizeOptions {
            node_input: true,
            require_complete: false,
            limits: polygonize_limits(limits),
            precision,
        },
    )?;
    // `GeometryFixer::fixRing` di GEOS: `bufferByZero(poligono, true)`, il
    // buffer nullo dell'anello e del suo inverso, cioe' le regioni con numero
    // di avvolgimento dell'anello diverso da zero. Il laboratorio prendeva
    // tutte le facce del polygonize: un anello che gira dentro se stesso (la
    // cornice con l'isola, avvolgimento 0 dentro) riempiva anche l'isola, e
    // da buco sottraeva troppo in silenzio. Ogni faccia ha avvolgimento
    // costante; lo si calcola in modo esatto in un suo punto interno, e
    // l'unione delle facce tenute si ricostruisce dal bordo, senza overlay.
    let mut boundary = Segments::new();
    for face in &result.polygons {
        let sample = face
            .interior_point()
            .ok_or(MakeValidError::InternalInvariant(
                "faccia dell'anello senza punto interno",
            ))?;
        if winding_number(sample.0, ring)? != 0 {
            add_polygon_parity(face, &mut boundary);
        }
    }
    area_from_boundary(&boundary, limits, precision)
}

/// Numero di avvolgimento esatto di `point` rispetto all'anello chiuso
/// (`orient2d` esatto, confronti di ordinate esatti), per simulazione di
/// semplicita': il punto e' spostato di `e * (d, 1)` con `0 < d << e`
/// infinitesimi. Il campione di una faccia puo' stare su un lato
/// dell'anello che il polygonize ha tolto (un tratto ripercorso, un dangle):
/// quel lato ha la faccia da entrambe le parti e un contributo netto nullo,
/// quindi qualunque spostamento infinitesimo da' l'avvolgimento della faccia.
/// Con lo spostamento le ordinate si confrontano con le disuguaglianze della
/// regola semiaperta, e su un lato collineare l'orientamento e' il segno di
/// `(b - a) x (d, 1)`: `b.x - a.x`, o per un lato verticale `-(b.y - a.y)`.
// I lati verticali si riconoscono dal confronto esatto delle ascisse.
#[allow(clippy::float_cmp)]
fn winding_number(point: Coord<f64>, ring: &LineString<f64>) -> Result<i64, MakeValidError> {
    let mut winding = 0_i64;
    for segment in ring.lines() {
        let orientation = match RobustKernel::orient2d(segment.start, segment.end, point) {
            Orientation::Collinear => {
                if segment.end.x > segment.start.x
                    || (segment.end.x == segment.start.x && segment.end.y < segment.start.y)
                {
                    Orientation::CounterClockwise
                } else {
                    Orientation::Clockwise
                }
            }
            other => other,
        };
        if segment.start.y <= point.y {
            if segment.end.y > point.y && orientation == Orientation::CounterClockwise {
                winding = winding
                    .checked_add(1)
                    .ok_or(MakeValidError::IndexOverflow)?;
            }
        } else if segment.end.y <= point.y && orientation == Orientation::Clockwise {
            winding = winding
                .checked_sub(1)
                .ok_or(MakeValidError::IndexOverflow)?;
        }
    }
    Ok(winding)
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

    /// Il modulo massimo delle coordinate degli operandi.
    fn magnitude(self) -> f64 {
        [
            self.minimum_x,
            self.minimum_x + self.span_x,
            self.minimum_y,
            self.minimum_y + self.span_y,
        ]
        .into_iter()
        .fold(0.0_f64, |massimo, value| massimo.max(value.abs()))
    }

    /// Lo spostamento massimo di una coordinata riportata dall'overlay, in
    /// coordinate originali: la diagonale del passo per asse `span * 2^-49 +
    /// 4 ulp(M)` (vedi [`checked_grid`]).
    fn grid_diagonal(self) -> f64 {
        let step = 2_f64.powi(-49);
        let rounding = 4.0 * super::griglia::ulp(self.magnitude());
        self.span_x
            .abs()
            .mul_add(step, rounding)
            .hypot(self.span_y.abs().mul_add(step, rounding))
    }

    /// Riporta l'output dell'overlay nelle coordinate originali e lo aggancia
    /// ai vertici e ai lati assiali degli operandi **vicini**.
    ///
    /// Il raggio d'aggancio `r` e' lo spostamento massimo di una coordinata
    /// riportata ([`Self::grid_diagonal`]). Un vertice va sul vertice
    /// d'ingresso piu' vicino entro `r` (distanza fra punti); altrimenti ogni
    /// coordinata va sull'ascissa di un lato verticale (o sull'ordinata di un
    /// lato orizzontale) d'ingresso entro `r` che gli passa accanto, cosi' un
    /// incrocio con un lato assiale resta esattamente sul lato. Lo
    /// spostamento dell'aggancio e' al piu' `sqrt(2) * r`, e con quello del
    /// ritorno (al piu' `r`) resta sotto `(1 + sqrt(2)) * r`: il bilancio che
    /// [`checked_grid`] confronta con la precisione dichiarata.
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
/// il motore `i64` di `i_overlay` 9.0.0 (`i_float` 5.0.0,
/// `FloatPointAdapter::with_iter_conservative`) sceglie il passo
/// `2^(ceil(log2(0.5)) - 61) = 2^-62`, sotto la spaziatura dei `f64` in `[0,
/// 1]`: lo spostamento di una coordinata normalizzata lo fanno gli
/// arrotondamenti. Per asse, in unita' normalizzate: la normalizzazione
/// (sottrazione e divisione, al piu' `2^-52`), i due passaggi `f64 -> i64 ->
/// f64` dell'overlay (ingresso e pulizia del risultato, al piu' `3 * 2^-52`
/// ciascuno con modulo `1`, vedi [`super::griglia`]) e i passi della
/// griglia (arrotondamenti, primo aggancio: pochi `2^-62`), meno di `2^-49`
/// in tutto; il ritorno `u * span + min` aggiunge al piu' `3 ulp(M)` (`span
/// <= 2 M`). In coordinate originali lo spostamento per asse e' quindi al
/// piu' `span * 2^-49 + 4 ulp(M)`, e la diagonale dei due assi `d`
/// ([`OverlayNormalizer::grid_diagonal`]). Con `i32` (`i_overlay` 4.5, passo
/// `2^-30`) era `span * 2^-30`: oltre circa 5.400 km su un asse l'overlay
/// era rifiutato.
///
/// Il bilancio dello spostamento di un vertice e' il ritorno (al piu' `d`)
/// piu' l'aggancio di [`OverlayNormalizer::restore_multi_snapped`] (al piu'
/// `sqrt(2) * d`): `(1 + sqrt(2)) * d`. Se supera la precisione dichiarata
/// l'overlay non si esegue: [`MakeValidError::PrecisionInsufficient`]. In
/// metri con 1 cm non scatta prima della guardia di spaziatura (modulo di
/// circa `2^39` m, estensione fino a `2^40` m: `(1 + sqrt(2)) d` circa 8,3
/// mm). Sotto la precisione i vertici possono spostarsi
/// e le feature piu' sottili possono fondersi o sparire: errore dichiarato,
/// non un rifiuto.
///
/// Prima della griglia, la spaziatura dei `f64` alle coordinate degli
/// operandi ([`super::precision::coordinate_abbastanza_fitte`]): una
/// coordinata riportata dalla griglia si arrotonda al `f64` piu' vicino, e
/// a `2^52` l'arrotondamento da solo vale mezza unita'.
///
/// Gli overlay di una riparazione sono in catena (unioni dei buchi e delle
/// parti, differenza): lo spostamento di ognuno si sottrae dal
/// [`Bilancio`] della riparazione, e l'overlay che lo porterebbe sotto zero
/// non si esegue. Contano solo gli overlay eseguiti davvero (un'unione di
/// operandi disgiunti non passa da `i_overlay` e non consuma nulla); la
/// somma e' prudente, perche' non tutti gli overlay stanno sulla stessa
/// catena. La guardia di spaziatura resta sulla precisione intera.
///
/// Gli agganci interni di `i_overlay` durante lo split dei segmenti (raggio
/// che cresce a ogni giro, `split::snap_radius`) non sono nel bilancio, e
/// nessun controllo a posteriori li limita (README, «Limiti dichiarati»).
fn checked_grid(
    normalizer: OverlayNormalizer,
    bilancio: &mut Bilancio,
) -> Result<(), MakeValidError> {
    let spostamento = (1.0 + std::f64::consts::SQRT_2) * normalizer.grid_diagonal();
    if !super::precision::coordinate_abbastanza_fitte(normalizer.magnitude(), bilancio.precisione)
        || !matches!(
            spostamento.partial_cmp(&bilancio.residuo),
            Some(std::cmp::Ordering::Less | std::cmp::Ordering::Equal)
        )
    {
        return Err(MakeValidError::PrecisionInsufficient);
    }
    bilancio.residuo -= spostamento;
    Ok(())
}

/// Il bilancio della precisione degli overlay di una riparazione
/// `STRUCTURE` (vedi [`checked_grid`]): la precisione e quanto ne resta.
#[derive(Clone, Copy, Debug)]
struct Bilancio {
    precisione: f64,
    residuo: f64,
}

impl Bilancio {
    const fn nuovo(precisione: f64) -> Self {
        Self {
            precisione,
            residuo: precisione,
        }
    }
}

/// Un lato d'ingresso, indicizzato per le decisioni di `LINEWORK`.
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

/// Riporta l'output normalizzato nelle coordinate originali, agganciato
/// agli operandi ([`OverlayNormalizer::restore_multi_snapped`]).
fn finished_overlay(
    normalizer: OverlayNormalizer,
    normalized: &MultiPolygon<f64>,
    left: &MultiPolygon<f64>,
    right: &MultiPolygon<f64>,
) -> MultiPolygon<f64> {
    normalizer.restore_multi_snapped(normalized, left, right)
}

fn normalized_union(
    left: &MultiPolygon<f64>,
    right: &MultiPolygon<f64>,
    bilancio: &mut Bilancio,
) -> Result<MultiPolygon<f64>, MakeValidError> {
    if !normalized_intersects(left, right) {
        let mut polygons = left.0.clone();
        polygons.extend(right.0.iter().cloned());
        return Ok(MultiPolygon::new(polygons));
    }
    let normalizer = OverlayNormalizer::new(left, right);
    checked_grid(normalizer, bilancio)?;
    let normalized = normalizer
        .map_multi(left, OverlayNormalizer::normalize)
        .union(&normalizer.map_multi(right, OverlayNormalizer::normalize));
    Ok(finished_overlay(normalizer, &normalized, left, right))
}

fn normalized_difference(
    left: &MultiPolygon<f64>,
    right: &MultiPolygon<f64>,
    bilancio: &mut Bilancio,
) -> Result<MultiPolygon<f64>, MakeValidError> {
    let normalizer = OverlayNormalizer::new(left, right);
    checked_grid(normalizer, bilancio)?;
    let normalized = normalizer
        .map_multi(left, OverlayNormalizer::normalize)
        .difference(&normalizer.map_multi(right, OverlayNormalizer::normalize));
    Ok(finished_overlay(normalizer, &normalized, left, right))
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
    bilancio: &mut Bilancio,
) -> Result<(), MakeValidError> {
    if area.0.is_empty() {
        *area = MultiPolygon::new(vec![polygon.clone()]);
    } else {
        *area = normalized_union(area, &MultiPolygon::new(vec![polygon.clone()]), bilancio)?;
    }
    Ok(())
}

fn merge_multipolygon(
    area: &mut MultiPolygon<f64>,
    polygons: &MultiPolygon<f64>,
    bilancio: &mut Bilancio,
) -> Result<(), MakeValidError> {
    if area.0.is_empty() {
        *area = polygons.clone();
    } else if !polygons.0.is_empty() {
        *area = normalized_union(area, polygons, bilancio)?;
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
    bilancio: &mut Bilancio,
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
    // Buchi in ordine canonico: le unioni sono commutative, ma ogni overlay
    // arrotonda sulla propria griglia, e l'ordine d'ingresso non deve
    // decidere l'arrotondamento.
    for hole in canonical_order(polygon.interiors(), ring_key)? {
        let fixed_hole = fixed_ring(hole, limits, precision)?;
        if normalized_intersects(&shell, &fixed_hole) {
            subtractive_holes = normalized_union(&subtractive_holes, &fixed_hole, bilancio)?;
        } else {
            promoted_holes = normalized_union(&promoted_holes, &fixed_hole, bilancio)?;
        }
    }
    if !subtractive_holes.0.is_empty() {
        fixed = normalized_difference(&fixed, &subtractive_holes, bilancio)?;
    }
    if !promoted_holes.0.is_empty() {
        fixed = normalized_union(&fixed, &promoted_holes, bilancio)?;
    }
    Ok(Some(as_polygonal_geometry(fixed)))
}

/// Dove sta un lato nodato rispetto all'area.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SideOfArea {
    Inside,
    Boundary,
    Outside,
}

/// Il lato nodato (senza incroci propri con il bordo dell'area) sta dentro,
/// sul bordo o fuori? Sul bordo se entrambi gli estremi stanno entro la
/// precisione da uno stesso lato del bordo (un lato piu' vicino di 1 cm al
/// bordo ne fa parte: politica del centimetro). Altrimenti decidono tre
/// punti interni al lato con il predicato esatto di `geo`; se non concordano
/// e' un errore, mai un lato indovinato.
fn side_of_area(
    start: Coord<f64>,
    end: Coord<f64>,
    area: &MultiPolygon<f64>,
    index: &RTree<OperandEdge>,
    precision: f64,
) -> Result<SideOfArea, MakeValidError> {
    let query = AABB::from_corners(
        [
            start.x.min(end.x) - precision,
            start.y.min(end.y) - precision,
        ],
        [
            start.x.max(end.x) + precision,
            start.y.max(end.y) + precision,
        ],
    );
    if index.locate_in_envelope_intersecting(&query).any(|edge| {
        super::precision::punto_entro_segmento(start, edge.start, edge.end, precision)
            && super::precision::punto_entro_segmento(end, edge.start, edge.end, precision)
    }) {
        return Ok(SideOfArea::Boundary);
    }
    let mut side = None;
    for fraction in [0.25, 0.5, 0.75] {
        let sample = Coord {
            x: start.x + (end.x - start.x) * fraction,
            y: start.y + (end.y - start.y) * fraction,
        };
        let here = match area.coordinate_position(&sample) {
            CoordPos::Inside => SideOfArea::Inside,
            CoordPos::Outside => SideOfArea::Outside,
            CoordPos::OnBoundary => continue,
        };
        match side {
            None => side = Some(here),
            Some(previous) if previous != here => {
                return Err(MakeValidError::InternalInvariant(
                    "lato collassato dentro e fuori dall'area",
                ))
            }
            Some(_) => {}
        }
    }
    Ok(side.unwrap_or(SideOfArea::Boundary))
}

/// L'unione di `GeometryFixer::fixMultiPolygon` di GEOS fra l'area e le
/// parti collassate (linee e punti di `keep_collapsed`): le linee nodate fra
/// loro e con il bordo dell'area, meno cio' che l'area copre; i punti non
/// coperti ne' dall'area ne' dalle linee. Il laboratorio polygonizzava le
/// linee e ne dava le facce come area: tre poligoni collassati che formano
/// un triangolo diventavano un triangolo pieno, e una linea dentro l'area
/// restava. Qui le linee restano linee.
fn normalize_collapsed_union(
    components: Vec<Geometry<f64>>,
    area: &MultiPolygon<f64>,
    limits: MakeValidLimits,
    precision: f64,
) -> Result<Vec<Geometry<f64>>, MakeValidError> {
    let mut lines = Segments::new();
    let mut points = BTreeMap::new();
    let mut output = Vec::new();
    for component in components {
        match component {
            Geometry::LineString(line) => add_line_segments(&line, &mut lines),
            Geometry::MultiLineString(multi) => {
                for line in &multi.0 {
                    add_line_segments(line, &mut lines);
                }
            }
            Geometry::Point(point) => {
                points.insert(CoordKey::new(point.0), point);
            }
            other => output.push(other),
        }
    }
    let mut kept = Segments::new();
    if !lines.is_empty() {
        // Nodare le linee fra loro e con il bordo dell'area.
        let mut linework = lines;
        for polygon in &area.0 {
            add_polygon_segments(polygon, &mut linework);
        }
        let noded = result_segments(&polygonize_segments(&linework, true, limits, precision)?);
        let index = RTree::bulk_load(
            polygonal_boundaries(area)
                .map(|line| OperandEdge {
                    start: line.start,
                    end: line.end,
                })
                .collect(),
        );
        for (key, (start, end)) in noded {
            if side_of_area(start, end, area, &index, precision)? == SideOfArea::Outside {
                kept.insert(key, (start, end));
            }
        }
    }
    let mut lines = Vec::new();
    for (start, end) in kept.values() {
        lines.push(LineString::new(vec![*start, *end]));
    }
    let mut kept_points = Vec::new();
    for point in points.into_values() {
        let on_line = kept.values().any(|(start, end)| {
            RobustKernel::orient2d(*start, *end, point.0) == Orientation::Collinear
                && point.x() >= start.x.min(end.x)
                && point.x() <= start.x.max(end.x)
                && point.y() >= start.y.min(end.y)
                && point.y() <= start.y.max(end.y)
        });
        if !on_line && area.coordinate_position(&point.0) == CoordPos::Outside {
            kept_points.push(point);
        }
    }
    if let Some(lines) = line_geometry(lines) {
        output.push(lines);
    }
    if let Some(points) = point_geometry(kept_points) {
        output.push(points);
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
            let mut bilancio = Bilancio::nuovo(precision);
            structure_polygon(polygon, keep_collapsed, limits, precision, &mut bilancio)?
                .unwrap_or_else(|| {
                    Geometry::Polygon(Polygon::new(LineString::new(Vec::new()), vec![]))
                })
        }
        Geometry::MultiPolygon(polygons) => {
            let mut bilancio = Bilancio::nuovo(precision);
            let mut area = MultiPolygon::empty();
            let mut collapsed = Vec::new();
            // Poligoni in ordine canonico, come i buchi in `structure_polygon`:
            // l'area e' l'unione delle parti riparate (`GeometryFixer`), e
            // l'ordine d'ingresso non decide ne' l'arrotondamento ne' l'ordine
            // delle parti collassate.
            for polygon in canonical_order(&polygons.0, polygon_key)? {
                match structure_polygon(polygon, keep_collapsed, limits, precision, &mut bilancio)?
                {
                    Some(Geometry::Polygon(polygon)) => {
                        merge_polygon(&mut area, &polygon, &mut bilancio)?;
                    }
                    Some(Geometry::MultiPolygon(polygons)) => {
                        merge_multipolygon(&mut area, &polygons, &mut bilancio)?;
                    }
                    Some(other) => collapsed.push(other),
                    None => {}
                }
            }
            if !collapsed.is_empty() && (!area.0.is_empty() || collapsed.len() > 1) {
                collapsed = normalize_collapsed_union(collapsed, &area, limits, precision)?;
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

/// Chiave canonica di un anello: insieme dei lati, insieme dei vertici e,
/// a parita', la sequenza. Due anelli con la stessa chiave sono lo stesso
/// anello, quindi l'ordine fra loro non conta.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct RingKey {
    segments: Vec<SegmentKey>,
    vertices: Vec<CoordKey>,
    sequence: Vec<CoordKey>,
}

fn ring_key(ring: &LineString<f64>) -> Result<RingKey, MakeValidError> {
    let mut sequence = Vec::new();
    sequence
        .try_reserve_exact(ring.0.len())
        .map_err(|_| MakeValidError::AllocationFailed("chiave dell'anello"))?;
    sequence.extend(ring.0.iter().copied().map(CoordKey::new));
    let mut vertices = sequence.clone();
    vertices.sort_unstable();
    vertices.dedup();
    Ok(RingKey {
        segments: ring_segment_signature(ring)?,
        vertices,
        sequence,
    })
}

/// Chiave canonica di un poligono: la shell, poi i buchi in ordine canonico.
fn polygon_key(polygon: &Polygon<f64>) -> Result<(RingKey, Vec<RingKey>), MakeValidError> {
    let mut holes = Vec::new();
    holes
        .try_reserve_exact(polygon.interiors().len())
        .map_err(|_| MakeValidError::AllocationFailed("chiavi dei buchi"))?;
    for hole in polygon.interiors() {
        holes.push(ring_key(hole)?);
    }
    holes.sort_unstable();
    Ok((ring_key(polygon.exterior())?, holes))
}

/// Gli elementi nell'ordine delle loro chiavi canoniche: l'esito di chi li
/// scorre non dipende dall'ordine d'ingresso. A chiave uguale gli elementi
/// sono uguali.
fn canonical_order<'a, T, K: Ord>(
    items: impl IntoIterator<Item = &'a T>,
    key: impl Fn(&T) -> Result<K, MakeValidError>,
) -> Result<Vec<&'a T>, MakeValidError>
where
    T: 'a,
{
    let mut keyed = Vec::new();
    for item in items {
        keyed
            .try_reserve(1)
            .map_err(|_| MakeValidError::AllocationFailed("ordine canonico"))?;
        keyed.push((key(item)?, item));
    }
    keyed.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(keyed.into_iter().map(|(_, item)| item).collect())
}

fn polygonal_boundaries(polygons: &MultiPolygon<f64>) -> impl Iterator<Item = geo::Line<f64>> + '_ {
    polygons.0.iter().flat_map(|polygon| {
        std::iter::once(polygon.exterior())
            .chain(polygon.interiors())
            .flat_map(LineString::lines)
    })
}

/// Lati non orientati del linework, con le coordinate nel verso della chiave.
/// L'ordine della mappa e' quello delle chiavi: ogni funzione che la consuma
/// vede gli stessi lati nello stesso ordine qualunque sia l'ordine degli
/// anelli e dei poligoni d'ingresso.
type Segments = BTreeMap<SegmentKey, (Coord<f64>, Coord<f64>)>;

fn add_line_segments(line: &LineString<f64>, segments: &mut Segments) {
    for segment in line.lines() {
        if let Some(key) = SegmentKey::new(segment.start, segment.end) {
            segments.entry(key).or_insert((segment.start, segment.end));
        }
    }
}

fn add_polygon_segments(polygon: &Polygon<f64>, segments: &mut Segments) {
    add_line_segments(polygon.exterior(), segments);
    for ring in polygon.interiors() {
        add_line_segments(ring, segments);
    }
}

/// Aggiunge il lato se manca, lo toglie se c'e': somma modulo 2 di catene.
fn toggle_segment(segments: &mut Segments, key: SegmentKey, coordinates: (Coord<f64>, Coord<f64>)) {
    if segments.remove(&key).is_none() {
        segments.insert(key, coordinates);
    }
}

/// I lati come `MultiLineString` di segmenti, nell'ordine delle chiavi.
fn segment_lines(segments: &Segments) -> Result<Geometry<f64>, MakeValidError> {
    let mut lines = Vec::new();
    lines
        .try_reserve_exact(segments.len())
        .map_err(|_| MakeValidError::AllocationFailed("lati del linework"))?;
    for (start, end) in segments.values() {
        lines.push(LineString::new(vec![*start, *end]));
    }
    Ok(Geometry::MultiLineString(MultiLineString::new(lines)))
}

/// Il polygonize di un insieme di lati.
///
/// L'ingresso e' l'insieme ordinato dei lati, due coordinate per lato: una
/// funzione dell'insieme, non dell'ordine o del verso degli anelli da cui
/// viene. Il limite di coordinate d'ingresso si applica alla geometria
/// (`checked_preflight`); qui vale per la sua rappresentazione per lati, che
/// ne ha al piu' il doppio, e per i lati nodati, gia' limitati dall'output del
/// primo polygonize.
fn polygonize_segments(
    segments: &Segments,
    node_input: bool,
    limits: MakeValidLimits,
    precision: f64,
) -> Result<super::polygonize::PolygonizeResult, MakeValidError> {
    polygonize_segments_with_rounded(segments, node_input, limits, precision)
        .map(|(result, _)| result)
}

/// [`polygonize_segments`] con gli incroci arrotondati del noding.
fn polygonize_segments_with_rounded(
    segments: &Segments,
    node_input: bool,
    limits: MakeValidLimits,
    precision: f64,
) -> Result<(super::polygonize::PolygonizeResult, Vec<Coord<f64>>), MakeValidError> {
    let coordinates = u64::try_from(segments.len())
        .map_err(|_| MakeValidError::IndexOverflow)?
        .checked_mul(2)
        .ok_or(MakeValidError::IndexOverflow)?;
    let mut polygonize = polygonize_limits(limits);
    polygonize.max_input_coordinates = polygonize.max_input_coordinates.max(coordinates);
    Ok(super::polygonize::polygonize_linework_rust_with_rounded(
        &segment_lines(segments)?,
        PolygonizeOptions {
            node_input,
            require_complete: false,
            limits: polygonize,
            precision,
        },
    )?)
}

/// Tutti i lati di un polygonize: facce, cut edge, dangle, anelli invalidi.
fn result_segments(result: &super::polygonize::PolygonizeResult) -> Segments {
    let mut segments = Segments::new();
    for polygon in &result.polygons {
        add_polygon_segments(polygon, &mut segments);
    }
    for line in result
        .cut_edges
        .iter()
        .chain(&result.dangles)
        .chain(&result.invalid_ring_lines)
    {
        add_line_segments(line, &mut segments);
    }
    segments
}

/// `BuildArea` di GEOS sulle facce di un polygonize, restituito come
/// **bordo** dell'area costruita.
///
/// GEOS (`operation/polygonize/BuildArea.cpp`) prende le facce del
/// polygonizer, da' a una faccia per genitore la faccia che ha un buco
/// uguale al suo guscio (`findFaceHoles`), tiene le facce con un numero pari
/// di antenati e le unisce. L'uguaglianza di due anelli dello stesso grafo
/// nodato e' l'uguaglianza dei loro insiemi di lati. Le facce di un
/// polygonize hanno interni disgiunti, quindi il bordo dell'unione delle
/// facce tenute e' l'insieme dei lati che compaiono un numero dispari di
/// volte nei loro anelli: nessun overlay.
///
/// GEOS sceglie il genitore scorrendo le facce per area dell'inviluppo
/// decrescente; qui il genitore e' la faccia con quel buco, senza ordine, e le
/// due regole coincidono salvo inviluppi di area uguale fra guscio e buco.
fn build_area_boundary(faces: &[Polygon<f64>]) -> Result<Segments, MakeValidError> {
    let mut shells = BTreeMap::new();
    for (index, face) in faces.iter().enumerate() {
        if shells
            .insert(ring_segment_signature(face.exterior())?, index)
            .is_some()
        {
            return Err(MakeValidError::InternalInvariant(
                "due facce con lo stesso guscio",
            ));
        }
    }
    let mut parents = vec![None; faces.len()];
    for (index, face) in faces.iter().enumerate() {
        for hole in face.interiors() {
            if let Some(&child) = shells.get(&ring_segment_signature(hole)?) {
                if child == index {
                    return Err(MakeValidError::InternalInvariant(
                        "faccia con un buco uguale al proprio guscio",
                    ));
                }
                if parents[child].is_none() {
                    parents[child] = Some(index);
                }
            }
        }
    }
    let mut boundary = Segments::new();
    for (index, face) in faces.iter().enumerate() {
        let mut depth = 0_usize;
        let mut ancestor = parents[index];
        while let Some(parent) = ancestor {
            depth += 1;
            if depth > faces.len() {
                return Err(MakeValidError::InternalInvariant(
                    "ciclo nella gerarchia delle facce",
                ));
            }
            ancestor = parents[parent];
        }
        if depth.is_multiple_of(2) {
            for ring in std::iter::once(face.exterior()).chain(face.interiors()) {
                for segment in ring.lines() {
                    if let Some(key) = SegmentKey::new(segment.start, segment.end) {
                        toggle_segment(&mut boundary, key, (segment.start, segment.end));
                    }
                }
            }
        }
    }
    Ok(boundary)
}

/// La regione limitata il cui bordo, come catena modulo 2, e' `boundary`.
///
/// Le facce del polygonize di `boundary` sono le facce della sua
/// disposizione; ogni lato separa una faccia dentro da una fuori, e il lato
/// che compare in una sola faccia ha fuori la regione illimitata. Si colorano
/// le facce per adiacenza (pari-dispari), e il bordo delle facce dentro deve
/// tornare esattamente `boundary`: ogni incoerenza e' un errore interno, mai
/// un'area indovinata.
fn area_from_boundary(
    boundary: &Segments,
    limits: MakeValidLimits,
    precision: f64,
) -> Result<MultiPolygon<f64>, MakeValidError> {
    if boundary.is_empty() {
        return Ok(MultiPolygon::empty());
    }
    let result = polygonize_segments(boundary, false, limits, precision)?;
    if result.residual_count()? != 0 {
        return Err(MakeValidError::InternalInvariant(
            "bordo dell'area con lati che non separano facce",
        ));
    }
    let faces = result.polygons;
    let mut face_segments = Vec::new();
    face_segments
        .try_reserve_exact(faces.len())
        .map_err(|_| MakeValidError::AllocationFailed("lati delle facce dell'area"))?;
    let mut incidence = BTreeMap::<SegmentKey, Vec<usize>>::new();
    for (index, face) in faces.iter().enumerate() {
        let mut keys = Vec::new();
        for ring in std::iter::once(face.exterior()).chain(face.interiors()) {
            for segment in ring.lines() {
                if let Some(key) = SegmentKey::new(segment.start, segment.end) {
                    keys.push(key);
                    incidence.entry(key).or_default().push(index);
                }
            }
        }
        face_segments.push(keys);
    }
    let mut inside = vec![None; faces.len()];
    let mut queue = VecDeque::new();
    for incident in incidence.values() {
        match incident.as_slice() {
            [face] => {
                if inside[*face].is_none() {
                    inside[*face] = Some(true);
                    queue.push_back(*face);
                }
            }
            [left, right] if left != right => {}
            _ => {
                return Err(MakeValidError::InternalInvariant(
                    "lato del bordo dell'area senza due lati distinti",
                ))
            }
        }
    }
    while let Some(face) = queue.pop_front() {
        let Some(state) = inside[face] else {
            return Err(MakeValidError::InternalInvariant(
                "faccia in coda senza colore",
            ));
        };
        for key in &face_segments[face] {
            let incident = incidence.get(key).ok_or(MakeValidError::InternalInvariant(
                "lato della faccia assente dall'incidenza",
            ))?;
            for &other in incident {
                if other == face {
                    continue;
                }
                match inside[other] {
                    None => {
                        inside[other] = Some(!state);
                        queue.push_back(other);
                    }
                    Some(value) if value == state => {
                        return Err(MakeValidError::InternalInvariant(
                            "facce adiacenti dallo stesso lato del bordo",
                        ));
                    }
                    Some(_) => {}
                }
            }
        }
    }
    let mut polygons = Vec::new();
    let mut check = Segments::new();
    for (face, state) in faces.into_iter().zip(inside) {
        match state {
            Some(true) => {
                add_polygon_parity(&face, &mut check);
                polygons.push(face);
            }
            Some(false) => {}
            None => {
                return Err(MakeValidError::InternalInvariant(
                    "faccia dell'area non raggiunta",
                ))
            }
        }
    }
    if !check.keys().eq(boundary.keys()) {
        return Err(MakeValidError::InternalInvariant(
            "il bordo dell'area costruita non e' quello calcolato",
        ));
    }
    Ok(MultiPolygon::new(polygons))
}

fn add_polygon_parity(polygon: &Polygon<f64>, segments: &mut Segments) {
    for ring in std::iter::once(polygon.exterior()).chain(polygon.interiors()) {
        for segment in ring.lines() {
            if let Some(key) = SegmentKey::new(segment.start, segment.end) {
                toggle_segment(segments, key, (segment.start, segment.end));
            }
        }
    }
}

fn validate_linework_output(geometry: &Geometry<f64>) -> Result<(), MakeValidError> {
    match geometry {
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

/// Nessun punto d'incrocio arrotondato del linework nodato sta entro la
/// precisione da un altro vertice o da un lato che non gli e' incidente.
///
/// In `LINEWORK` la gerarchia delle facce (quale buco e' uguale a quale
/// guscio, quali componenti si toccano) decide l'area di regioni intere.
/// Con coordinate d'ingresso i predicati sono esatti, qui come in GEOS; un
/// incrocio arrotondato invece si sposta di qualche ULP, e se accanto c'e'
/// un'altra feature lo spostamento puo' creare o togliere una scheggia, un
/// contatto, un'uguaglianza di anelli: l'area cambia di metri quadrati, e
/// GEOS, che arrotonda a modo suo, decide diversamente (campagna
/// differenziale, generatore di anelli annidati, seme 1 caso 109: incrocio
/// arrotondato a `3e-16` da un vertice, 52 m^2 di differenza). La topologia
/// e' decisa sotto la precisione: errore esplicito.
fn checked_rounded_nodes(
    rounded: &[Coord<f64>],
    edges: &Segments,
    precision: f64,
) -> Result<(), MakeValidError> {
    if rounded.is_empty() {
        return Ok(());
    }
    let index = RTree::bulk_load(
        edges
            .values()
            .map(|(start, end)| OperandEdge {
                start: *start,
                end: *end,
            })
            .collect(),
    );
    for vertex in rounded {
        let query = AABB::from_corners(
            [vertex.x - precision, vertex.y - precision],
            [vertex.x + precision, vertex.y + precision],
        );
        let crowded = index.locate_in_envelope_intersecting(&query).any(|edge| {
            edge.start != *vertex
                && edge.end != *vertex
                && super::precision::punto_entro_segmento(*vertex, edge.start, edge.end, precision)
        });
        if crowded {
            return Err(MakeValidError::PrecisionInsufficient);
        }
    }
    Ok(())
}

/// `MakeValid` `LINEWORK` di GEOS (`operation/valid/MakeValid.cpp`,
/// `MakeValidPoly`), sui lati invece che con overlay.
///
/// GEOS noda il bordo di tutto il poligono o multipoligono (un'unione, che
/// fonde i lati ripetuti), poi ripete: `BuildArea` sui lati rimasti, area
/// := area XOR area nuova, lati := lati meno il bordo dell'area nuova;
/// finche' `BuildArea` non costruisce nulla. Escono l'area, i lati rimasti e
/// i vertici d'ingresso che il noding ha perso.
///
/// Qui ogni passo e' un'operazione esatta sull'insieme dei lati nodati `E0`:
/// le aree costruite sono unioni di facce di `E0`, e il bordo di uno XOR e'
/// la somma modulo 2 dei bordi, quindi l'area finale e' la regione il cui
/// bordo e' lo XOR dei bordi delle aree costruite ([`area_from_boundary`]).
/// Nessun overlay, nessuna griglia, nessuna soglia: l'unico calcolo con
/// arrotondamento e' il noding di `polygonize`, sotto il suo controllo di
/// precisione. L'esito e' una funzione dell'insieme dei lati d'ingresso,
/// quindi non dipende dall'ordine di anelli e poligoni ne' dal loro verso.
///
/// Non e' il pari-dispari su tutti gli anelli: dove GEOS diverge da quella
/// regola (un buco che condivide un lato con la shell, poligoni di un
/// multipoligono che si sovrappongono) si segue GEOS.
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
    // Il bordo di GEOS (`getBoundary`): tutti gli anelli di tutti i poligoni,
    // come insieme di lati e insieme di vertici.
    let mut input_segments = Segments::new();
    let mut input_vertices = BTreeSet::new();
    for polygon in &polygons {
        add_polygon_segments(polygon, &mut input_segments);
        for ring in std::iter::once(polygon.exterior()).chain(polygon.interiors()) {
            input_vertices.extend(ring.0.iter().copied().map(CoordKey::new));
        }
    }
    // `nodeLineWithFirstCoordinate`: il linework nodato, lati ripetuti fusi.
    let (mut edges, mut faces) = if input_segments.is_empty() {
        (Segments::new(), Vec::new())
    } else {
        let (noded, rounded) =
            polygonize_segments_with_rounded(&input_segments, true, limits, precision)?;
        let edges = result_segments(&noded);
        checked_rounded_nodes(&rounded, &edges, precision)?;
        (edges, noded.polygons)
    };
    // I vertici d'ingresso che non sono vertici del linework nodato (anelli
    // collassati in un punto): `collapse_points` di GEOS.
    let noded_vertices = edges
        .keys()
        .flat_map(|key| [key.0, key.1])
        .collect::<BTreeSet<_>>();
    let mut collapse_points = Vec::new();
    for key in input_vertices.difference(&noded_vertices) {
        collapse_points
            .try_reserve(1)
            .map_err(|_| MakeValidError::AllocationFailed("punti collassati"))?;
        collapse_points.push(Point::new(f64::from_bits(key.x), f64::from_bits(key.y)));
    }
    // Il ciclo di `MakeValidPoly`. Le facce del primo giro sono quelle del
    // noding; ogni giro toglie almeno un lato, quindi i giri sono al piu' i
    // lati.
    let mut area_boundary = Segments::new();
    while !faces.is_empty() {
        let boundary = build_area_boundary(&faces)?;
        if boundary.is_empty() {
            return Err(MakeValidError::InternalInvariant(
                "area costruita senza bordo",
            ));
        }
        for (key, coordinates) in boundary {
            if edges.remove(&key).is_none() {
                return Err(MakeValidError::InternalInvariant(
                    "bordo dell'area fuori dai lati rimasti",
                ));
            }
            toggle_segment(&mut area_boundary, key, coordinates);
        }
        faces = if edges.is_empty() {
            Vec::new()
        } else {
            polygonize_segments(&edges, false, limits, precision)?.polygons
        };
    }
    let area = area_from_boundary(&area_boundary, limits, precision)?;
    let mut lines = Vec::new();
    lines
        .try_reserve_exact(edges.len())
        .map_err(|_| MakeValidError::AllocationFailed("residui linework"))?;
    for (start, end) in edges.into_values() {
        lines.push(LineString::new(vec![start, end]));
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
/// overlay Rust. `Linework` noda tutto il bordo e costruisce l'area a giri
/// come GEOS, sui lati, conservando i residui lineari o puntuali. Gli input
/// gia' validi passano invariati.
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

    /// Il controllo della griglia: `span * 2^-49 + 4 ulp(M)` per asse
    /// contro la precisione. Con 1 cm in metri 1.300 e 20.000 km passano
    /// (con `i_overlay` 4.5 e il passo `span * 2^-30` 20.000 km erano
    /// rifiutati). Il controllo scatta solo al margine della guardia di
    /// spaziatura: modulo appena sotto `2^40` (`ulp = 2^-13`), estensione
    /// doppia del modulo e precisione `2^-7` (la minima che la guardia
    /// ammette), `(1 + sqrt(2)) d` circa `1,07e-2`; coordinate a `2^45` m
    /// con 1 cm sono rifiutate dalla guardia.
    #[test]
    fn grid_check_refuses_only_extents_coarser_than_the_precision() {
        let normalizer = |minimum: f64, span: f64| OverlayNormalizer {
            minimum_x: minimum,
            minimum_y: 0.0,
            span_x: span,
            span_y: 1.0,
        };
        assert!(checked_grid(normalizer(0.0, 20_000_000.0), &mut Bilancio::nuovo(0.01)).is_ok());
        assert!(checked_grid(normalizer(0.0, 1_300_000.0), &mut Bilancio::nuovo(0.01)).is_ok());
        let modulo = 2_f64.powi(40) - 2_f64.powi(-13);
        assert!(checked_grid(
            normalizer(-modulo, 2.0 * modulo),
            &mut Bilancio::nuovo(2_f64.powi(-6))
        )
        .is_ok());
        assert!(matches!(
            checked_grid(
                normalizer(-modulo, 2.0 * modulo),
                &mut Bilancio::nuovo(2_f64.powi(-7))
            ),
            Err(MakeValidError::PrecisionInsufficient)
        ));
        assert!(matches!(
            checked_grid(normalizer(2_f64.powi(45), 10.0), &mut Bilancio::nuovo(0.01)),
            Err(MakeValidError::PrecisionInsufficient)
        ));
    }

    /// Primo controesempio della terza revisione, in metri con 1 cm: il
    /// buco largo `2^-40` m e' sotto la precisione e puo' sparire; il resto
    /// resta entro perimetro per precisione.
    #[test]
    fn sub_precision_hole_may_vanish_the_rest_is_kept() -> Result<(), MakeValidError> {
        let shell = MultiPolygon::new(vec![square(0.0, 1.0, 0.0, 1.0)]);
        let hole = MultiPolygon::new(vec![square(0.5, 0.5 + 2_f64.powi(-40), 0.25, 0.75)]);
        let result = normalized_difference(&shell, &hole, &mut Bilancio::nuovo(0.01))?;
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
        let result = normalized_union(&frame, &triangle, &mut Bilancio::nuovo(0.01))?;
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
        let result = normalized_difference(&shell, &holes, &mut Bilancio::nuovo(PRECISION))?;
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

    /// `LINEWORK` su un buco che attraversa la shell: GEOS costruisce
    /// l'unione (18), poi l'anello interno `[3, 4] x [1, 3]` (2) e ne fa lo
    /// XOR, 16. I lati dell'anello interno sono il bordo della seconda area
    /// e non restano come linee: esce un multipoligono solo.
    #[test]
    fn linework_crossing_hole_is_the_geos_symmetric_difference() -> Result<(), MakeValidError> {
        let source = Geometry::Polygon(Polygon::new(
            square(0.0, 4.0, 0.0, 4.0).exterior().clone(),
            vec![square(3.0, 5.0, 1.0, 3.0).exterior().clone()],
        ));
        let output = make_valid_geometry_rust(&source, RepairMethod::Linework, false, PRECISION)?;
        if (output.unsigned_area() - 16.0).abs() > 1e-12 {
            return Err(MakeValidError::InvalidOutput(format!(
                "area selezionata={}",
                output.unsigned_area()
            )));
        }
        if !matches!(output, Geometry::MultiPolygon(_)) {
            return Err(MakeValidError::InvalidOutput(format!("{output:?}")));
        }
        Ok(())
    }
}
