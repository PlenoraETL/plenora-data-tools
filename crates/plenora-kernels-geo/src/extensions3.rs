//! Kernel delle estensioni di catalogo v1.3 (`geo.coverage_validate`,
//! `geo.shared_paths`).
//!
//! Kernel puri su `geo::Geometry<f64>` piu' gli adapter di colonna
//! (`coverage_validate_rows`, `shared_paths_rows`) che mappano gli errori su
//! [`PlenoraError`] preservando i messaggi, come `extensions.rs` (v1.1) e
//! `extensions2.rs` (v1.2). Semantica di riferimento: `PostGIS`
//! (`ST_SharedPaths`, validazione di coperture). Caso d'uso: piantine di
//! edifici — stanze adiacenti, pareti condivise, grafo degli spazi.
//!
//! Scelte documentate (v1):
//!
//! - Entrambe le op accettano solo geometrie poligonali
//!   (Polygon/MultiPolygon) valide; gli altri tipi e le geometrie invalide
//!   sono rifiutati esplicitamente (fail-closed), come in `topology.rs`.
//! - Le coppie candidate sono selezionate con un R-tree sugli envelope
//!   (`rstar`, gia' dipendenza del crate); l'intersezione degli AABB include
//!   il contatto (zero-area), quindi anche i confini che si toccano sono
//!   candidati. Ogni coppia `(a, b)` con `a < b` e' esaminata una sola volta,
//!   in ordine lessicografico: l'output e' deterministico.
//! - `coverage_validate` rileva gli **overlap**: per ogni coppia candidata
//!   l'intersezione nativa (`BooleanOps`, come `topology.rs`) con area
//!   `> tolerance` produce una issue `overlap` con area e geometria della
//!   zona sovrapposta (Polygon se singola componente, `MultiPolygon` altrimenti).
//!   `tolerance` default 0: solo overlap di area strettamente positiva.
//!   **I buchi (gap) non sono rilevati in v1**: richiederebbero l'unione
//!   dell'intera copertura e la differenza con l'envelope/la convessa,
//!   con una definizione di "buco atteso" che dipende dal dominio (buchi
//!   interni vs bordi esterni); la scelta e' rinviata a una v2 con config
//!   dedicata. Il tipo issue e' comunque modellato come enum per estensione.
//! - `coverage_validate` e' fail-closed su `max_issues` (default
//!   [`DEFAULT_MAX_ISSUES`]): superato il limite fallisce, non tronca.
//! - `shared_paths` estrae i tratti di confine condivisi: per ogni coppia
//!   candidata, l'intersezione collineare dei boundary (anelli esterni E
//!   interni) segmento per segmento via `line_intersection` (algoritmo
//!   robusto tipo JTS gia' in `geo`); i contatti puntuali
//!   (`SinglePoint`) sono esclusi per costruzione. I segmenti collineari con
//!   lunghezza `<= tolerance` sono scartati (anti-rumore, default 0: tiene
//!   tutto cio' che ha lunghezza positiva); la coppia produce una riga solo
//!   se la lunghezza totale condivisa e' `>= min_length` (default 0).
//!   **Una riga per coppia** (non per singolo segmento): la geometria e' una
//!   `LineString` se il confine condiviso e' un segmento unico, altrimenti una
//!   `MultiLineString`; `shared_length` e' la somma delle lunghezze. Per il
//!   grafo delle adiacenze (un arco per coppia di stanze) questa e' la
//!   granularita' utile; lo splitting in tratti connessi separati alla
//!   `PostGIS` e' un possibile raffinamento (punto aperto).
//! - Le coppie con overlap di area non sono escluse da `shared_paths`: se
//!   condividono anche porzioni di boundary collineari, i tratti sono
//!   riportati (come in `PostGIS`).
//!
//! Complessita': la conferma per coppia di `shared_paths` e' O(n*m) sui
//! segmenti dei boundary (con pre-filtro bbox per segmento); accettabile per
//! piantine (poligoni piccoli), il filtro R-tree limita le coppie. Entrambi
//! i kernel sono sequenziali e deterministici; la parallelizzazione
//! (precedente: `spatial_join`) e' un follow-up di prestazioni.
//!
//! Errori: le condizioni dei kernel puri usano [`ExtensionV3Error`].

use geo::algorithm::line_intersection::{line_intersection, LineIntersection};
use geo::algorithm::validation::Validation;
use geo::{
    Area, BooleanOps, BoundingRect, CoordsIter, Geometry, Line, LineString, MultiLineString,
    MultiPolygon,
};
use plenora_core::arrow::array::BinaryArray;
use plenora_core::PlenoraError;
use rstar::{RTree, RTreeObject, AABB};
use thiserror::Error;

use crate::arrow_adapter::{decode_geometry_cell, encode_geometry, map_nullable};

/// Default di `max_issues` per `geo.coverage_validate`.
pub const DEFAULT_MAX_ISSUES: usize = 1_000;

#[derive(Debug, Error)]
pub enum ExtensionV3Error {
    #[error("parametro {name} non valido: {reason}")]
    InvalidParameter {
        name: &'static str,
        reason: &'static str,
    },
    #[error("geometria {index} non poligonale ({found}): attesa Polygon/MultiPolygon")]
    UnsupportedGeometry { index: usize, found: &'static str },
    #[error("geometria {index} non valida: {reason}")]
    InvalidGeometry { index: usize, reason: String },
    #[error("geometria {index} contiene coordinate NaN o infinite")]
    NonFiniteCoordinate { index: usize },
    #[error("geometria prodotta non valida: {0}")]
    InvalidOutput(String),
    #[error("conteggio non rappresentabile come uint64")]
    IndexOverflow,
    #[error("issues di copertura oltre il limite {limit}")]
    IssueLimit { limit: u64 },
    /// Invariante interna violata (R6: errore propagato, mai panic).
    #[error("internal error: {0}")]
    Internal(&'static str),
}

const fn invalid_parameter(name: &'static str, reason: &'static str) -> ExtensionV3Error {
    ExtensionV3Error::InvalidParameter { name, reason }
}

fn check_tolerance(tolerance: f64) -> Result<(), ExtensionV3Error> {
    if !tolerance.is_finite() || tolerance < 0.0 {
        return Err(invalid_parameter(
            "tolerance",
            "deve essere finita e non negativa",
        ));
    }
    Ok(())
}

fn check_max_issues(max_issues: usize) -> Result<u64, ExtensionV3Error> {
    if max_issues == 0 {
        return Err(invalid_parameter(
            "max_issues",
            "deve essere maggiore di zero",
        ));
    }
    u64::try_from(max_issues).map_err(|_| ExtensionV3Error::IndexOverflow)
}

fn u64_index(index: usize) -> Result<u64, ExtensionV3Error> {
    u64::try_from(index).map_err(|_| ExtensionV3Error::IndexOverflow)
}

const fn geometry_name(geometry: &Geometry<f64>) -> &'static str {
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

/// Elemento di copertura preparato: multipoligono validato + envelope.
struct CoverageElement {
    polygons: MultiPolygon<f64>,
    envelope: AABB<[f64; 2]>,
}

#[derive(Clone, Copy)]
struct IndexedEnvelope {
    index: usize,
    envelope: AABB<[f64; 2]>,
}

impl RTreeObject for IndexedEnvelope {
    type Envelope = AABB<[f64; 2]>;

    fn envelope(&self) -> Self::Envelope {
        self.envelope
    }
}

/// Valida una geometria poligonale e ne estrae multipoligono ed envelope;
/// `None` per le geometrie vuote (senza envelope, nessun candidato).
fn prepare_element(
    geometry: &Geometry<f64>,
    index: usize,
) -> Result<Option<CoverageElement>, ExtensionV3Error> {
    if geometry
        .coords_iter()
        .any(|coordinate| !coordinate.x.is_finite() || !coordinate.y.is_finite())
    {
        return Err(ExtensionV3Error::NonFiniteCoordinate { index });
    }
    geometry
        .check_validation()
        .map_err(|error| ExtensionV3Error::InvalidGeometry {
            index,
            reason: error.to_string(),
        })?;
    let polygons = match geometry {
        Geometry::Polygon(polygon) => MultiPolygon::new(vec![polygon.clone()]),
        Geometry::MultiPolygon(polygons) => polygons.clone(),
        other => {
            return Err(ExtensionV3Error::UnsupportedGeometry {
                index,
                found: geometry_name(other),
            })
        }
    };
    let Some(rect) = polygons.bounding_rect() else {
        return Ok(None);
    };
    Ok(Some(CoverageElement {
        polygons,
        envelope: AABB::from_corners(
            [rect.min().x, rect.min().y],
            [rect.max().x, rect.max().y],
        ),
    }))
}

fn prepare_elements(
    geometries: &[Option<Geometry<f64>>],
) -> Result<(Vec<Option<CoverageElement>>, RTree<IndexedEnvelope>), ExtensionV3Error> {
    let mut elements = Vec::with_capacity(geometries.len());
    let mut envelopes = Vec::with_capacity(geometries.len());
    for (index, geometry) in geometries.iter().enumerate() {
        let Some(geometry) = geometry else {
            elements.push(None);
            continue;
        };
        let element = prepare_element(geometry, index)?;
        if let Some(element) = &element {
            envelopes.push(IndexedEnvelope {
                index,
                envelope: element.envelope,
            });
        }
        elements.push(element);
    }
    Ok((elements, RTree::bulk_load(envelopes)))
}

/// Coppie candidate `(a, b)` con `a < b` in ordine lessicografico: envelope
/// che si intersecano o toccano (l'intersezione AABB di rstar include il
/// contatto zero-area).
fn candidate_pairs(
    elements: &[Option<CoverageElement>],
    tree: &RTree<IndexedEnvelope>,
) -> Vec<(usize, usize)> {
    let mut pairs = Vec::new();
    for (a, element) in elements.iter().enumerate() {
        let Some(element) = element else {
            continue;
        };
        let mut others: Vec<usize> = tree
            .locate_in_envelope_intersecting(&element.envelope)
            .map(|candidate| candidate.index)
            .filter(|b| *b > a)
            .collect();
        others.sort_unstable();
        pairs.extend(others.into_iter().map(|b| (a, b)));
    }
    pairs
}

// ---------------------------------------------------------------------------
// geo.coverage_validate
// ---------------------------------------------------------------------------

/// Tipo di issue di copertura (v1: solo overlap; i gap sono rinviati a v2).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CoverageIssueType {
    Overlap,
}

impl CoverageIssueType {
    #[must_use] 
    pub const fn name(self) -> &'static str {
        match self {
            Self::Overlap => "overlap",
        }
    }
}

/// Una issue di copertura: coppia `(index_a, index_b)` con `a < b`, area e
/// geometria della zona sovrapposta.
#[derive(Clone, Debug, PartialEq)]
pub struct CoverageIssue {
    pub issue_type: CoverageIssueType,
    pub index_a: u64,
    pub index_b: u64,
    pub area: f64,
    pub geometry: Geometry<f64>,
}

/// Normalizza l'intersezione di due multipoligoni: Polygon se singola
/// componente, `MultiPolygon` altrimenti. Precondizione: intersezione non
/// vuota (area positiva). Validata (fail-closed).
fn overlap_geometry(intersection: MultiPolygon<f64>) -> Result<Geometry<f64>, ExtensionV3Error> {
    let geometry = if intersection.0.len() == 1 {
        Geometry::Polygon(
            intersection
                .0
                .into_iter()
                .next()
                .ok_or(ExtensionV3Error::Internal("una componente"))?,
        )
    } else {
        Geometry::MultiPolygon(intersection)
    };
    geometry
        .check_validation()
        .map_err(|error| ExtensionV3Error::InvalidOutput(error.to_string()))?;
    Ok(geometry)
}

fn coverage_validate_elements(
    elements: &[Option<CoverageElement>],
    tree: &RTree<IndexedEnvelope>,
    tolerance: f64,
    max_issues: u64,
) -> Result<Vec<CoverageIssue>, ExtensionV3Error> {
    let mut issues = Vec::new();
    for (a, b) in candidate_pairs(elements, tree) {
        let left = &elements[a]
            .as_ref()
            .ok_or(ExtensionV3Error::Internal("coppia indicizzata"))?
            .polygons;
        let right = &elements[b]
            .as_ref()
            .ok_or(ExtensionV3Error::Internal("coppia indicizzata"))?
            .polygons;
        let intersection = left.intersection(right);
        let area = intersection.unsigned_area();
        if area > tolerance {
            if u64_index(issues.len())? >= max_issues {
                return Err(ExtensionV3Error::IssueLimit { limit: max_issues });
            }
            issues.push(CoverageIssue {
                issue_type: CoverageIssueType::Overlap,
                index_a: u64_index(a)?,
                index_b: u64_index(b)?,
                area,
                geometry: overlap_geometry(intersection)?,
            });
        }
    }
    Ok(issues)
}

/// Trova gli overlap di una copertura poligonale.
///
/// Per ogni coppia di geometrie con intersezione di area `> tolerance`
/// produce una issue `overlap` con indici (posizioni originali), area e
/// geometria della zona. Fail-closed su `max_issues` e su input non
/// poligonali/invalide.
///
/// # Errors
///
/// - `InvalidParameter`: `tolerance` non finita o negativa, oppure
///   `max_issues` uguale a zero.
/// - `UnsupportedGeometry`: una geometria non e' Polygon/MultiPolygon.
/// - `InvalidGeometry`: una geometria di input non supera la validazione
///   OGC.
/// - `NonFiniteCoordinate`: una geometria contiene coordinate NaN o
///   infinite.
/// - `IssueLimit`: le issue superano `max_issues` (fail-closed, non tronca).
/// - `InvalidOutput`: la geometria di overlap prodotta non e' valida.
/// - `IndexOverflow`: un conteggio non e' rappresentabile come `u64`.
pub fn coverage_validate(
    geometries: &[Geometry<f64>],
    tolerance: f64,
    max_issues: usize,
) -> Result<Vec<CoverageIssue>, ExtensionV3Error> {
    let refs: Vec<Option<Geometry<f64>>> = geometries.iter().cloned().map(Some).collect();
    coverage_validate_nullable(&refs, tolerance, max_issues)
}

/// Variante nullable: le righe `None` non partecipano mai, ma conservano la
/// loro posizione (gli indici delle issue sono quelli originali).
///
/// # Errors
///
/// Come [`coverage_validate`] (questa e' l'implementazione; la variante
/// non-null vi delega).
pub fn coverage_validate_nullable(
    geometries: &[Option<Geometry<f64>>],
    tolerance: f64,
    max_issues: usize,
) -> Result<Vec<CoverageIssue>, ExtensionV3Error> {
    check_tolerance(tolerance)?;
    let max_issues = check_max_issues(max_issues)?;
    let (elements, tree) = prepare_elements(geometries)?;
    coverage_validate_elements(&elements, &tree, tolerance, max_issues)
}

fn coverage_error(error: &ExtensionV3Error) -> PlenoraError {
    PlenoraError::Contract(format!("geo.coverage_validate: {error}"))
}

/// Riga di output di `geo.coverage_validate` con geometria codificata WKB
/// (adapter per il canone GeoArrow-WKB).
#[derive(Clone, Debug, PartialEq)]
pub struct CoverageIssueRow {
    pub issue_type: &'static str,
    pub index_a: u64,
    pub index_b: u64,
    pub area: f64,
    pub wkb: Vec<u8>,
}

/// Adapter di colonna per `geo.coverage_validate`: decodifica le celle WKB
/// non-null, rileva gli overlap e codifica la geometria di ogni issue.
///
/// # Errors
///
/// - `PlenoraError::Contract`: una cella WKB viola il contratto strutturale
///   (come `decode_geometry_cell`), il kernel rifiuta l'input (errori
///   `ExtensionV3Error` mappati preservando il messaggio) o la codifica WKB
///   di una issue fallisce.
/// - `PlenoraError::Unsupported`: una cella porta dimensioni Z/M o SRID non
///   preservabili nel protocollo 2D.
pub fn coverage_validate_rows(
    cells: &BinaryArray,
    tolerance: f64,
    max_issues: usize,
) -> Result<Vec<CoverageIssueRow>, PlenoraError> {
    let geometries = map_nullable(cells, |payload| decode_geometry_cell(payload).map(Some))?;
    let issues = coverage_validate_nullable(&geometries, tolerance, max_issues)
        .map_err(|error| coverage_error(&error))?;
    issues
        .iter()
        .map(|issue| {
            Ok(CoverageIssueRow {
                issue_type: issue.issue_type.name(),
                index_a: issue.index_a,
                index_b: issue.index_b,
                area: issue.area,
                wkb: encode_geometry(&issue.geometry)?,
            })
        })
        .collect()
}

// ---------------------------------------------------------------------------
// geo.shared_paths
// ---------------------------------------------------------------------------

/// Un confine condiviso tra una coppia di poligoni: indici (posizioni
/// originali, `a < b`), lunghezza totale e geometria del tratto (`LineString`
/// se segmento unico, `MultiLineString` altrimenti).
#[derive(Clone, Debug, PartialEq)]
pub struct SharedPath {
    pub index_a: u64,
    pub index_b: u64,
    pub shared_length: f64,
    pub geometry: Geometry<f64>,
}

fn segment_length(segment: &Line<f64>) -> f64 {
    (segment.end.x - segment.start.x).hypot(segment.end.y - segment.start.y)
}

/// Segmenti collineari condivisi dai boundary di due multipoligoni, in
/// ordine deterministico (anelli e segmenti del primo, poi del secondo).
/// I contatti puntuali (`SinglePoint`) sono esclusi per costruzione; i
/// segmenti con lunghezza `<= tolerance` sono scartati.
fn shared_boundary_segments(
    left: &MultiPolygon<f64>,
    right: &MultiPolygon<f64>,
    tolerance: f64,
) -> Vec<Line<f64>> {
    let mut segments = Vec::new();
    for left_polygon in &left.0 {
        for left_ring in std::iter::once(left_polygon.exterior()).chain(left_polygon.interiors()) {
            for left_segment in left_ring.lines() {
                let (left_min, left_max) = (
                    [
                        left_segment.start.x.min(left_segment.end.x),
                        left_segment.start.y.min(left_segment.end.y),
                    ],
                    [
                        left_segment.start.x.max(left_segment.end.x),
                        left_segment.start.y.max(left_segment.end.y),
                    ],
                );
                for right_polygon in &right.0 {
                    for right_ring in
                        std::iter::once(right_polygon.exterior()).chain(right_polygon.interiors())
                    {
                        for right_segment in right_ring.lines() {
                            // Pre-filtro bbox (inclusivo: il tocco conta).
                            if right_segment.start.x.min(right_segment.end.x) > left_max[0]
                                || right_segment.start.x.max(right_segment.end.x) < left_min[0]
                                || right_segment.start.y.min(right_segment.end.y) > left_max[1]
                                || right_segment.start.y.max(right_segment.end.y) < left_min[1]
                            {
                                continue;
                            }
                            if let Some(LineIntersection::Collinear { intersection }) =
                                line_intersection(left_segment, right_segment)
                            {
                                if segment_length(&intersection) > tolerance {
                                    segments.push(intersection);
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    segments
}

/// Tratti di confine condivisi tra le coppie di una copertura poligonale.
///
/// Una riga per coppia con lunghezza totale `>= min_length`; le coppie con
/// solo contatto puntuale o disjointe non producono righe.
///
/// # Errors
///
/// - `InvalidParameter`: `tolerance` o `min_length` non finite o negative.
/// - `UnsupportedGeometry`: una geometria non e' Polygon/MultiPolygon.
/// - `InvalidGeometry`: una geometria di input non supera la validazione
///   OGC.
/// - `NonFiniteCoordinate`: una geometria contiene coordinate NaN o
///   infinite.
/// - `InvalidOutput`: la geometria prodotta non e' valida.
/// - `IndexOverflow`: un conteggio non e' rappresentabile come `u64`.
pub fn shared_paths(
    geometries: &[Geometry<f64>],
    tolerance: f64,
    min_length: f64,
) -> Result<Vec<SharedPath>, ExtensionV3Error> {
    let refs: Vec<Option<Geometry<f64>>> = geometries.iter().cloned().map(Some).collect();
    shared_paths_nullable(&refs, tolerance, min_length)
}

/// Variante nullable: come [`coverage_validate_nullable`], le righe `None`
/// conservano la posizione senza partecipare.
///
/// # Errors
///
/// Come [`shared_paths`] (questa e' l'implementazione; la variante non-null
/// vi delega).
pub fn shared_paths_nullable(
    geometries: &[Option<Geometry<f64>>],
    tolerance: f64,
    min_length: f64,
) -> Result<Vec<SharedPath>, ExtensionV3Error> {
    check_tolerance(tolerance)?;
    if !min_length.is_finite() || min_length < 0.0 {
        return Err(invalid_parameter(
            "min_length",
            "deve essere finita e non negativa",
        ));
    }
    let (elements, tree) = prepare_elements(geometries)?;
    let mut paths = Vec::new();
    for (a, b) in candidate_pairs(&elements, &tree) {
        let left = &elements[a]
            .as_ref()
            .ok_or(ExtensionV3Error::Internal("coppia indicizzata"))?
            .polygons;
        let right = &elements[b]
            .as_ref()
            .ok_or(ExtensionV3Error::Internal("coppia indicizzata"))?
            .polygons;
        let segments = shared_boundary_segments(left, right, tolerance);
        let shared_length: f64 = segments.iter().map(segment_length).sum();
        if shared_length < min_length || shared_length == 0.0 {
            continue;
        }
        let geometry = if segments.len() == 1 {
            let segment = segments[0];
            Geometry::LineString(LineString::new(vec![segment.start, segment.end]))
        } else {
            Geometry::MultiLineString(MultiLineString::new(
                segments
                    .iter()
                    .map(|segment| LineString::new(vec![segment.start, segment.end]))
                    .collect(),
            ))
        };
        geometry
            .check_validation()
            .map_err(|error| ExtensionV3Error::InvalidOutput(error.to_string()))?;
        paths.push(SharedPath {
            index_a: u64_index(a)?,
            index_b: u64_index(b)?,
            shared_length,
            geometry,
        });
    }
    Ok(paths)
}

fn shared_paths_error(error: &ExtensionV3Error) -> PlenoraError {
    PlenoraError::Contract(format!("geo.shared_paths: {error}"))
}

/// Riga di output di `geo.shared_paths` con geometria codificata WKB.
#[derive(Clone, Debug, PartialEq)]
pub struct SharedPathRow {
    pub index_a: u64,
    pub index_b: u64,
    pub shared_length: f64,
    pub wkb: Vec<u8>,
}

/// Adapter di colonna per `geo.shared_paths`: decodifica le celle WKB
/// non-null, estrae i confini condivisi e codifica la geometria di ogni
/// tratto.
///
/// # Errors
///
/// - `PlenoraError::Contract`: una cella WKB viola il contratto strutturale
///   (come `decode_geometry_cell`), il kernel rifiuta l'input (errori
///   `ExtensionV3Error` mappati preservando il messaggio) o la codifica WKB
///   di un tratto fallisce.
/// - `PlenoraError::Unsupported`: una cella porta dimensioni Z/M o SRID non
///   preservabili nel protocollo 2D.
pub fn shared_paths_rows(
    cells: &BinaryArray,
    tolerance: f64,
    min_length: f64,
) -> Result<Vec<SharedPathRow>, PlenoraError> {
    let geometries = map_nullable(cells, |payload| decode_geometry_cell(payload).map(Some))?;
    let paths = shared_paths_nullable(&geometries, tolerance, min_length)
        .map_err(|error| shared_paths_error(&error))?;
    paths
        .iter()
        .map(|path| {
            Ok(SharedPathRow {
                index_a: path.index_a,
                index_b: path.index_b,
                shared_length: path.shared_length,
                wkb: encode_geometry(&path.geometry)?,
            })
        })
        .collect()
}

#[cfg(test)]
// Confronti float esatti intenzionali: le fixture sono costruite per
// produrre valori esatti (coordinate note, round-trip bit-esatti); il
// confronto per bit e' il contratto verificato, non un'approssimazione.
#[allow(clippy::float_cmp)]
mod tests {
    use super::*;
    use geo::{polygon, MultiPolygon as GeoMultiPolygon, Point};
    use plenora_core::arrow::array::BinaryArray as ArrowBinaryArray;

    fn rectangle(xmin: f64, ymin: f64, xmax: f64, ymax: f64) -> Geometry<f64> {
        Geometry::Polygon(polygon![
            (x: xmin, y: ymin), (x: xmax, y: ymin),
            (x: xmax, y: ymax), (x: xmin, y: ymax),
            (x: xmin, y: ymin),
        ])
    }

    fn assert_close(actual: f64, expected: f64) {
        assert!(
            (actual - expected).abs() < 1e-9,
            "atteso {expected}, ottenuto {actual}"
        );
    }

    fn wkb_cells(geometries: &[Option<Geometry<f64>>]) -> ArrowBinaryArray {
        let encoded: Vec<Option<Vec<u8>>> = geometries
            .iter()
            .map(|geometry| geometry.as_ref().map(|g| encode_geometry(g).expect("encode")))
            .collect();
        encoded
            .iter()
            .map(|cell| cell.as_deref())
            .collect::<ArrowBinaryArray>()
    }

    // --- geo.coverage_validate ---------------------------------------------

    #[test]
    fn coverage_validate_reports_exact_overlap_area_and_geometry() {
        let inputs = vec![rectangle(0.0, 0.0, 4.0, 4.0), rectangle(2.0, 0.0, 6.0, 4.0)];
        let issues = coverage_validate(&inputs, 0.0, DEFAULT_MAX_ISSUES).expect("issues");
        assert_eq!(issues.len(), 1);
        let issue = &issues[0];
        assert_eq!(issue.issue_type, CoverageIssueType::Overlap);
        assert_eq!(issue.issue_type.name(), "overlap");
        assert_eq!((issue.index_a, issue.index_b), (0, 1));
        assert_close(issue.area, 8.0);
        // Zona di overlap: rettangolo (2,0)-(4,4), area 8, singola componente.
        assert!(matches!(issue.geometry, Geometry::Polygon(_)));
        assert_close(issue.geometry.unsigned_area(), 8.0);
        let rect = issue.geometry.bounding_rect().expect("envelope");
        assert_eq!(rect.min().x, 2.0);
        assert_eq!(rect.max().x, 4.0);
        assert_eq!(rect.max().y, 4.0);
    }

    #[test]
    fn coverage_validate_ignores_disjoint_touching_and_gapped_pairs() {
        // Disjointi, solo bordo in comune (area nulla), e buco tra stanze
        // (gap: non rilevato in v1, scelta documentata nel modulo).
        let inputs = vec![
            rectangle(0.0, 0.0, 4.0, 4.0),
            rectangle(4.0, 0.0, 8.0, 4.0), // tocca la prima sul bordo x=4
            rectangle(10.0, 0.0, 14.0, 4.0), // gap (8..10) rispetto alla seconda
        ];
        assert!(coverage_validate(&inputs, 0.0, DEFAULT_MAX_ISSUES)
            .expect("issues")
            .is_empty());
    }

    #[test]
    fn coverage_validate_tolerance_filters_small_overlaps() {
        // Overlap di area 8.0: ignorato con tolerance >= 8 (confronto stretto).
        let inputs = vec![rectangle(0.0, 0.0, 4.0, 4.0), rectangle(2.0, 0.0, 6.0, 4.0)];
        assert!(coverage_validate(&inputs, 8.0, DEFAULT_MAX_ISSUES)
            .expect("issues")
            .is_empty());
        let issues = coverage_validate(&inputs, 7.9, DEFAULT_MAX_ISSUES).expect("issues");
        assert_eq!(issues.len(), 1);
        assert!(coverage_validate(&inputs, -1.0, DEFAULT_MAX_ISSUES).is_err());
        assert!(coverage_validate(&inputs, f64::NAN, DEFAULT_MAX_ISSUES).is_err());
    }

    #[test]
    fn coverage_validate_finds_overlaps_across_multipolygon_components() {
        let multi = Geometry::MultiPolygon(GeoMultiPolygon::new(vec![
            match rectangle(0.0, 0.0, 2.0, 2.0) {
                Geometry::Polygon(p) => p,
                _ => unreachable!(),
            },
            match rectangle(10.0, 10.0, 14.0, 14.0) {
                Geometry::Polygon(p) => p,
                _ => unreachable!(),
            },
        ]));
        let inputs = vec![multi, rectangle(1.0, 1.0, 3.0, 3.0)];
        let issues = coverage_validate(&inputs, 0.0, DEFAULT_MAX_ISSUES).expect("issues");
        assert_eq!(issues.len(), 1);
        assert_eq!((issues[0].index_a, issues[0].index_b), (0, 1));
        assert_close(issues[0].area, 1.0);
    }

    #[test]
    fn coverage_validate_is_fail_closed_on_max_issues() {
        // Tre rettangoli a coppie sovrapposte: 3 issue (0,1), (0,2), (1,2).
        let inputs = vec![
            rectangle(0.0, 0.0, 4.0, 4.0),
            rectangle(2.0, 0.0, 6.0, 4.0),
            rectangle(1.0, 2.0, 5.0, 6.0),
        ];
        let issues = coverage_validate(&inputs, 0.0, 3).expect("tre issue ammesse");
        assert_eq!(issues.len(), 3);
        assert_eq!(
            issues
                .iter()
                .map(|issue| (issue.index_a, issue.index_b))
                .collect::<Vec<_>>(),
            vec![(0, 1), (0, 2), (1, 2)]
        );
        assert!(matches!(
            coverage_validate(&inputs, 0.0, 2),
            Err(ExtensionV3Error::IssueLimit { limit: 2 })
        ));
        assert!(matches!(
            coverage_validate(&inputs, 0.0, 0),
            Err(ExtensionV3Error::InvalidParameter {
                name: "max_issues",
                ..
            })
        ));
    }

    #[test]
    fn coverage_validate_rejects_non_polygonal_invalid_and_non_finite_inputs() {
        let point = vec![Geometry::Point(Point::new(0.0, 0.0))];
        assert!(matches!(
            coverage_validate(&point, 0.0, DEFAULT_MAX_ISSUES),
            Err(ExtensionV3Error::UnsupportedGeometry {
                index: 0,
                found: "Point"
            })
        ));
        let bowtie = vec![Geometry::Polygon(polygon![
            (x: 0.0, y: 0.0), (x: 2.0, y: 2.0),
            (x: 0.0, y: 2.0), (x: 2.0, y: 0.0),
            (x: 0.0, y: 0.0),
        ])];
        assert!(matches!(
            coverage_validate(&bowtie, 0.0, DEFAULT_MAX_ISSUES),
            Err(ExtensionV3Error::InvalidGeometry { index: 0, .. })
        ));
        let nan = vec![Geometry::Point(Point::new(f64::NAN, 0.0))];
        assert!(matches!(
            coverage_validate(&nan, 0.0, DEFAULT_MAX_ISSUES),
            Err(ExtensionV3Error::NonFiniteCoordinate { index: 0 })
        ));
    }

    #[test]
    fn coverage_validate_rows_preserves_positions_and_encodes_wkb() {
        let inputs = vec![
            Some(rectangle(0.0, 0.0, 4.0, 4.0)),
            None,
            Some(rectangle(2.0, 0.0, 6.0, 4.0)),
        ];
        let cells = wkb_cells(&inputs);
        let rows = coverage_validate_rows(&cells, 0.0, DEFAULT_MAX_ISSUES).expect("righe");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].issue_type, "overlap");
        assert_eq!((rows[0].index_a, rows[0].index_b), (0, 2));
        assert_close(rows[0].area, 8.0);
        let geometry = crate::geometry_from_wkb(&rows[0].wkb).expect("decode");
        assert_close(geometry.unsigned_area(), 8.0);
        assert!(coverage_validate_rows(&cells, -1.0, DEFAULT_MAX_ISSUES).is_err());
    }

    // --- geo.shared_paths ----------------------------------------------------

    #[test]
    fn shared_paths_finds_the_full_wall_between_adjacent_rooms() {
        // Due stanze rettangolari adiacenti: parete condivisa (4,0)-(4,3).
        let inputs = vec![rectangle(0.0, 0.0, 4.0, 3.0), rectangle(4.0, 0.0, 8.0, 3.0)];
        let paths = shared_paths(&inputs, 0.0, 0.0).expect("tratti");
        assert_eq!(paths.len(), 1);
        let path = &paths[0];
        assert_eq!((path.index_a, path.index_b), (0, 1));
        assert_close(path.shared_length, 3.0);
        let Geometry::LineString(line) = &path.geometry else {
            panic!("attesa LineString, ottenuto {:?}", path.geometry);
        };
        let mut endpoints: Vec<(f64, f64)> =
            line.0.iter().map(|coord| (coord.x, coord.y)).collect();
        endpoints.sort_by(|a, b| a.partial_cmp(b).expect("confronto"));
        assert_eq!(endpoints, vec![(4.0, 0.0), (4.0, 3.0)]);
    }

    #[test]
    fn shared_paths_excludes_point_only_contacts_and_disjoint_pairs() {
        let inputs = vec![
            rectangle(0.0, 0.0, 4.0, 4.0),
            rectangle(4.0, 4.0, 8.0, 8.0), // solo il vertice (4,4) in comune
            rectangle(20.0, 0.0, 24.0, 4.0), // disgiunta
        ];
        assert!(shared_paths(&inputs, 0.0, 0.0).expect("tratti").is_empty());
    }

    #[test]
    fn shared_paths_measures_partial_walls_and_t_junctions() {
        // La seconda stanza copre solo la meta' alta della parete est.
        let inputs = vec![rectangle(0.0, 0.0, 4.0, 4.0), rectangle(4.0, 2.0, 8.0, 4.0)];
        let paths = shared_paths(&inputs, 0.0, 0.0).expect("tratti");
        assert_eq!(paths.len(), 1);
        assert_close(paths[0].shared_length, 2.0);

        // Tre stanze: (0,1) condividono 3.0, (0,2) condividono 1.0 sulla
        // stessa parete (T-junction lato sinistro); (1,2) condividono il
        // tratto orizzontale pieno y=3, lunghezza 4.0.
        let inputs = vec![
            rectangle(0.0, 0.0, 4.0, 4.0),
            rectangle(4.0, 0.0, 8.0, 3.0),
            rectangle(4.0, 3.0, 8.0, 4.0),
        ];
        let paths = shared_paths(&inputs, 0.0, 0.0).expect("tratti");
        assert_eq!(paths.len(), 3);
        assert_eq!((paths[0].index_a, paths[0].index_b), (0, 1));
        assert_close(paths[0].shared_length, 3.0);
        assert_eq!((paths[1].index_a, paths[1].index_b), (0, 2));
        assert_close(paths[1].shared_length, 1.0);
        assert_eq!((paths[2].index_a, paths[2].index_b), (1, 2));
        assert_close(paths[2].shared_length, 4.0);
    }

    #[test]
    fn shared_paths_applies_min_length_and_tolerance() {
        let inputs = vec![rectangle(0.0, 0.0, 4.0, 4.0), rectangle(4.0, 0.0, 8.0, 4.0)];
        // Tratto da 4.0: min_length oltre il tratto esclude la riga.
        assert!(shared_paths(&inputs, 0.0, 4.1).expect("tratti").is_empty());
        assert_eq!(shared_paths(&inputs, 0.0, 4.0).expect("tratti").len(), 1);
        // tolerance >= lunghezza del segmento: scartato (confronto stretto).
        assert!(shared_paths(&inputs, 4.0, 0.0).expect("tratti").is_empty());
        assert_eq!(shared_paths(&inputs, 3.9, 0.0).expect("tratti").len(), 1);
        assert!(shared_paths(&inputs, -1.0, 0.0).is_err());
        assert!(shared_paths(&inputs, 0.0, -1.0).is_err());
        assert!(shared_paths(&inputs, f64::NAN, 0.0).is_err());
    }

    #[test]
    fn shared_paths_covers_multipolygons_and_holes() {
        // Un MultiPolygon la cui seconda componente condivide la parete con
        // la seconda stanza; indici posizionali della riga madre.
        let Geometry::Polygon(component) = rectangle(10.0, 0.0, 14.0, 3.0) else {
            unreachable!()
        };
        let Geometry::Polygon(first) = rectangle(0.0, 0.0, 4.0, 3.0) else {
            unreachable!()
        };
        let multi = Geometry::MultiPolygon(GeoMultiPolygon::new(vec![first, component]));
        let inputs = vec![multi, rectangle(14.0, 0.0, 18.0, 3.0)];
        let paths = shared_paths(&inputs, 0.0, 0.0).expect("tratti");
        assert_eq!(paths.len(), 1);
        assert_eq!((paths[0].index_a, paths[0].index_b), (0, 1));
        assert_close(paths[0].shared_length, 3.0);

        // Confine condiviso lungo l'anello interno (buco) di un poligono.
        let ring = LineString::from(vec![
            (0.0, 0.0),
            (10.0, 0.0),
            (10.0, 10.0),
            (0.0, 10.0),
            (0.0, 0.0),
        ]);
        let hole = LineString::from(vec![
            (2.0, 2.0),
            (6.0, 2.0),
            (6.0, 6.0),
            (2.0, 6.0),
            (2.0, 2.0),
        ]);
        let donut = Geometry::Polygon(geo::Polygon::new(ring, vec![hole]));
        let inner_room = rectangle(6.0, 2.0, 8.0, 6.0); // a est del buco
        let paths = shared_paths(&[donut, inner_room], 0.0, 0.0).expect("tratti");
        assert_eq!(paths.len(), 1);
        assert_close(paths[0].shared_length, 4.0);
    }

    #[test]
    fn shared_paths_merges_collinear_chains_into_a_multilinestring() {
        // Parete condivisa a L (due tratti perpendicolari): MultiLineString,
        // lunghezza totale = somma dei tratti.
        let l_shape = Geometry::Polygon(polygon![
            (x: 0.0, y: 0.0), (x: 8.0, y: 0.0), (x: 8.0, y: 4.0),
            (x: 4.0, y: 4.0), (x: 4.0, y: 8.0), (x: 0.0, y: 8.0),
            (x: 0.0, y: 0.0),
        ]);
        let room = rectangle(4.0, 4.0, 8.0, 8.0); // riempie la nicchia nord-est
        let paths = shared_paths(&[l_shape, room], 0.0, 0.0).expect("tratti");
        assert_eq!(paths.len(), 1);
        assert_close(paths[0].shared_length, 8.0);
        let Geometry::MultiLineString(lines) = &paths[0].geometry else {
            panic!("attesa MultiLineString, ottenuto {:?}", paths[0].geometry);
        };
        assert_eq!(lines.0.len(), 2);
    }

    #[test]
    fn shared_paths_rejects_non_polygonal_inputs_and_encodes_rows() {
        let point = vec![Geometry::Point(Point::new(0.0, 0.0))];
        assert!(matches!(
            shared_paths(&point, 0.0, 0.0),
            Err(ExtensionV3Error::UnsupportedGeometry { .. })
        ));

        let inputs = vec![
            None,
            Some(rectangle(0.0, 0.0, 4.0, 3.0)),
            Some(rectangle(4.0, 0.0, 8.0, 3.0)),
        ];
        let cells = wkb_cells(&inputs);
        let rows = shared_paths_rows(&cells, 0.0, 0.0).expect("righe");
        assert_eq!(rows.len(), 1);
        assert_eq!((rows[0].index_a, rows[0].index_b), (1, 2));
        assert_close(rows[0].shared_length, 3.0);
        let geometry = crate::geometry_from_wkb(&rows[0].wkb).expect("decode");
        assert!(matches!(geometry, Geometry::LineString(_)));
    }
}
