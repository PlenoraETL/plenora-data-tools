//! Algoritmi geometrici estesi, con limiti di lavoro e di output dichiarati.
//!
//! I kernel di `geo.densify`, `geo.snap_to_grid`, `geo.delaunay`,
//! `geo.line_interpolate_point`, `geo.line_substring`,
//! `geo.frechet_distance`, `geo.bearing`, `geo.geodesic_area`,
//! `geo.geometry_diagnostics`, `geo.line_merge` e lo split lineare di
//! `geo.split`. Lavorano su una geometria (o una coppia) alla volta: il
//! passaggio da e verso le colonne Arrow e i limiti per piano spettano al
//! chiamante, che passa ogni limite come argomento esplicito.
//!
//! Ogni kernel, salvo [`geometry_diagnostics`], rifiuta un ingresso con
//! coordinate non finite o non valido per la validazione OGC prima di
//! calcolare, e i kernel che producono geometrie rivalidano l'uscita.

use crate::geodetica::EllissoideGeodetico;
use geo::algorithm::line_measures::{Densify, Euclidean, FrechetDistance, InterpolateLine, Length};
use geo::algorithm::orient::{Direction, Orient};
use geo::line_intersection::{line_intersection, LineIntersection};
use geo::{
    Coord, CoordsIter, Geometry, Line, LineString, MapCoords, MultiPolygon, Point, Polygon,
    Triangle,
};
use geographiclib_rs::{InverseGeodesic as _, PolygonArea, Winding};
use plenora_core::crs::GROUND_PRECISION_METRES;
use rstar::{RTree, RTreeObject, AABB};
use serde::Serialize;
use spade::Triangulation as _;
use std::collections::HashMap;
use thiserror::Error;

/// Errori dei kernel di questo modulo. Nessun messaggio riporta coordinate
/// o valori delle righe.
#[derive(Debug, Error)]
pub enum ExtendedAlgorithmError {
    /// Un parametro fuori dal suo dominio: `name` lo nomina, `reason` dice
    /// quale vincolo viola.
    #[error("parametro {name} non valido: {reason}")]
    InvalidParameter {
        /// Nome del parametro.
        name: &'static str,
        /// Vincolo violato.
        reason: &'static str,
    },
    /// La geometria d'ingresso ha coordinate NaN o infinite o non supera la
    /// validazione OGC (la ragione e' la classificazione di
    /// `RagioneNonValida`, senza coordinate).
    #[error("geometria di input non valida: {0}")]
    InvalidInput(String),
    /// La geometria calcolata ha coordinate non finite o non supera la
    /// validazione OGC: il kernel non la pubblica.
    #[error("geometria prodotta non valida: {0}")]
    InvalidOutput(String),
    /// Il tipo della geometria non e' tra quelli che l'operazione accetta.
    // Il tipo della cella (`actual`) non entra nel messaggio («errori
    // senza dati»).
    #[error("tipo geometria non supportato da {operation}")]
    UnsupportedGeometry {
        /// Nome breve dell'operazione.
        operation: &'static str,
        /// Tipo `geo` della geometria rifiutata.
        actual: &'static str,
    },
    /// Le coordinate d'ingresso (`actual`) superano il limite passato dal
    /// chiamante (`limit`).
    #[error("coordinate oltre il limite di {limit}: {actual}")]
    CoordinateLimit { actual: u64, limit: u64 },
    /// Coordinate, parti o triangoli d'uscita (`actual`, stimati o
    /// prodotti) oltre il limite passato dal chiamante (`limit`).
    #[error("output oltre il limite di {limit}: {actual}")]
    OutputLimit { actual: u64, limit: u64 },
    /// Il lavoro quadratico (`actual`: coppie di coordinate, test
    /// d'intersezione) supera il limite passato dal chiamante (`limit`);
    /// `actual` vale `u64::MAX` se il prodotto non e' rappresentabile.
    #[error("lavoro quadratico oltre il limite di {limit}: {actual}")]
    WorkLimit { actual: u64, limit: u64 },
    /// La triangolazione di `spade` non si e' costruita: coordinata fuori
    /// dal dominio dei predicati esatti (zero o modulo in `[2^-142, 2^201]`),
    /// con il messaggio di `geo` 0.33.1, oppure vertici persi o fusi.
    #[error("triangolazione fallita: {0}")]
    Triangulation(String),
    /// Longitudine fuori da `[-180, 180]` o latitudine fuori da `[-90, 90]`.
    #[error("coordinate geografiche fuori intervallo lon/lat")]
    InvalidGeographicCoordinate,
    /// `bearing`: l'azimut non e' definito (punti coincidenti, origine su
    /// un polo, geodetica piu' breve non unica); porta il caso, senza
    /// coordinate.
    #[error("azimut non definito: {0}")]
    AzimutNonDefinito(&'static str),
    /// Un conteggio (coordinate, parti, triangoli) non sta in `u64`.
    #[error("conteggio non rappresentabile come uint64")]
    IndexOverflow,
    /// Invariante interna violata: errore propagato, mai un panico.
    #[error("internal error: {0}")]
    Internal(&'static str),
    /// La validazione OGC non ha concluso: `geo` si e' interrotta.
    ///
    /// **Non** e' una geometria invalida. Nessuno ha dimostrato che l'ingresso
    /// sia sbagliato, e accusarlo manderebbe chi legge a correggere un errore
    /// che non ha commesso. Porta la *forma* del payload, mai il contenuto.
    #[error("validazione OGC non conclusa: {0} (contenuto non pubblicato)")]
    ValidazioneNonConclusa(&'static str),
    /// Un calcolo di `geo` o `rstar` e' andato in panico dentro
    /// `crate::calcolo_protetto`: non accusa l'ingresso, porta la *forma*
    /// del payload, mai il contenuto.
    #[error("calcolo non concluso: {0} (contenuto non pubblicato)")]
    CalcoloNonConcluso(&'static str),
}

/// Un calcolo di `geo` o `rstar` dietro la barriera dei panici.
fn protetto<T>(calcolo: impl FnOnce() -> T) -> Result<T, ExtendedAlgorithmError> {
    crate::calcolo_protetto(calcolo).map_err(ExtendedAlgorithmError::CalcoloNonConcluso)
}

use crate::geometry_type_name as geometry_type;
use crate::EsitoValidazione;
use crate::ValidazioneProtetta as _;

fn validate_input(geometry: &Geometry<f64>) -> Result<(), ExtendedAlgorithmError> {
    if geometry
        .coords_iter()
        .any(|coordinate| !coordinate.x.is_finite() || !coordinate.y.is_finite())
    {
        return Err(ExtendedAlgorithmError::InvalidInput(
            "coordinate NaN o infinite".to_owned(),
        ));
    }
    geometry.validazione_protetta().map_err(|esito| {
        esito.separa(
            |ragione| ExtendedAlgorithmError::InvalidInput(ragione.to_string()),
            ExtendedAlgorithmError::ValidazioneNonConclusa,
        )
    })
}

fn validate_output(geometry: Geometry<f64>) -> Result<Geometry<f64>, ExtendedAlgorithmError> {
    if geometry
        .coords_iter()
        .any(|coordinate| !coordinate.x.is_finite() || !coordinate.y.is_finite())
    {
        return Err(ExtendedAlgorithmError::InvalidOutput(
            "coordinate NaN o infinite".to_owned(),
        ));
    }
    geometry.validazione_protetta().map_err(|esito| {
        esito.separa(
            |ragione| ExtendedAlgorithmError::InvalidOutput(ragione.to_string()),
            ExtendedAlgorithmError::ValidazioneNonConclusa,
        )
    })?;
    Ok(geometry)
}

fn coordinate_count(geometry: &Geometry<f64>) -> Result<u64, ExtendedAlgorithmError> {
    u64::try_from(geometry.coords_count()).map_err(|_| ExtendedAlgorithmError::IndexOverflow)
}

fn checked_densified_line_count(
    line: &LineString<f64>,
    max_segment_length: f64,
) -> Result<u64, ExtendedAlgorithmError> {
    if line.0.is_empty() {
        return Ok(0);
    }
    let mut total = 1_u64;
    for segment in line.lines() {
        let dx = segment.end.x - segment.start.x;
        let dy = segment.end.y - segment.start.y;
        let length = dx.hypot(dy);
        let pieces = (length / max_segment_length).ceil();
        // Soglia 2^64: esatta in f64 e uguale a `u64::MAX as f64`, che
        // arrotonda per eccesso.
        if !pieces.is_finite() || pieces > 18_446_744_073_709_551_616.0 {
            return Err(ExtendedAlgorithmError::IndexOverflow);
        }
        // Guardia sopra: pieces finito, in [0, 2^64] e a valore intero
        // (ceil di un rapporto non negativo, max_segment_length > 0); il
        // cast saturante non puo' perdere segno ne' troncare.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let pieces_u64 = (pieces as u64).max(1);
        total = total
            .checked_add(pieces_u64)
            .ok_or(ExtendedAlgorithmError::IndexOverflow)?;
    }
    Ok(total)
}

fn densified_count(
    geometry: &Geometry<f64>,
    max_segment_length: f64,
) -> Result<u64, ExtendedAlgorithmError> {
    match geometry {
        Geometry::Point(_) => Ok(1),
        Geometry::Line(_) => Err(ExtendedAlgorithmError::UnsupportedGeometry {
            operation: "densify",
            actual: "Line",
        }),
        Geometry::LineString(line) => checked_densified_line_count(line, max_segment_length),
        Geometry::Polygon(polygon) => {
            let mut total = checked_densified_line_count(polygon.exterior(), max_segment_length)?;
            for ring in polygon.interiors() {
                total = total
                    .checked_add(checked_densified_line_count(ring, max_segment_length)?)
                    .ok_or(ExtendedAlgorithmError::IndexOverflow)?;
            }
            Ok(total)
        }
        Geometry::MultiPoint(points) => {
            u64::try_from(points.0.len()).map_err(|_| ExtendedAlgorithmError::IndexOverflow)
        }
        Geometry::MultiLineString(lines) => lines.0.iter().try_fold(0_u64, |total, line| {
            total
                .checked_add(checked_densified_line_count(line, max_segment_length)?)
                .ok_or(ExtendedAlgorithmError::IndexOverflow)
        }),
        Geometry::MultiPolygon(polygons) => polygons.0.iter().try_fold(0_u64, |total, polygon| {
            let count = densified_count(&Geometry::Polygon(polygon.clone()), max_segment_length)?;
            total
                .checked_add(count)
                .ok_or(ExtendedAlgorithmError::IndexOverflow)
        }),
        Geometry::GeometryCollection(collection) => {
            collection.0.iter().try_fold(0_u64, |total, child| {
                total
                    .checked_add(densified_count(child, max_segment_length)?)
                    .ok_or(ExtendedAlgorithmError::IndexOverflow)
            })
        }
        Geometry::Rect(_) | Geometry::Triangle(_) => {
            Err(ExtendedAlgorithmError::UnsupportedGeometry {
                operation: "densify",
                actual: geometry_type(geometry),
            })
        }
    }
}

/// Densifica i lati fino a `max_segment_length`.
///
/// Ogni lato di lunghezza euclidea `L` si divide in
/// `ceil(L / max_segment_length)` parti uguali, con i vertici nuovi a
/// `inizio + (fine - inizio) * k / n` (`Densify` di `geo` con la metrica
/// `Euclidean`). I vertici d'ingresso restano con i loro bit, anche i
/// duplicati consecutivi. Punti e multipunti escono invariati; le collezioni
/// si densificano membro per membro.
///
/// Il numero di coordinate d'uscita si calcola prima di allocare e si
/// riconta dopo: entrambi entro `max_output_coordinates`. In una
/// `GeometryCollection` il limite vale per il totale e, di nuovo, per ogni
/// membro.
///
/// # Errors
///
/// Nell'ordine in cui si controllano:
///
/// - `InvalidInput`: coordinate NaN o infinite, o geometria OGC non valida;
///   `ValidazioneNonConclusa` se la validazione non conclude;
/// - `InvalidParameter`: `max_segment_length` non finita o non positiva;
/// - `UnsupportedGeometry`: `Line`, `Rect` o `Triangle` in input;
/// - `IndexOverflow`: conteggio delle coordinate densificate non
///   rappresentabile come `u64`;
/// - `OutputLimit`: coordinate stimate o prodotte oltre
///   `max_output_coordinates`;
/// - `CalcoloNonConcluso`: la densificazione di `geo` e' andata in panico;
/// - `InvalidOutput`: geometria prodotta non valida.
pub fn densify(
    geometry: &Geometry<f64>,
    max_segment_length: f64,
    max_output_coordinates: u64,
) -> Result<Geometry<f64>, ExtendedAlgorithmError> {
    validate_input(geometry)?;
    if !max_segment_length.is_finite() || max_segment_length <= 0.0 {
        return Err(ExtendedAlgorithmError::InvalidParameter {
            name: "max_segment_length",
            reason: "deve essere finita e maggiore di zero",
        });
    }
    let estimated = densified_count(geometry, max_segment_length)?;
    if estimated > max_output_coordinates {
        return Err(ExtendedAlgorithmError::OutputLimit {
            actual: estimated,
            limit: max_output_coordinates,
        });
    }
    let output = match geometry {
        Geometry::Point(_) | Geometry::MultiPoint(_) => geometry.clone(),
        Geometry::LineString(line) => {
            Geometry::LineString(protetto(|| Euclidean.densify(line, max_segment_length))?)
        }
        Geometry::Polygon(polygon) => {
            Geometry::Polygon(protetto(|| Euclidean.densify(polygon, max_segment_length))?)
        }
        Geometry::MultiLineString(lines) => {
            Geometry::MultiLineString(protetto(|| Euclidean.densify(lines, max_segment_length))?)
        }
        Geometry::MultiPolygon(polygons) => Geometry::MultiPolygon(protetto(|| {
            Euclidean.densify(polygons, max_segment_length)
        })?),
        Geometry::GeometryCollection(collection) => Geometry::GeometryCollection(
            collection
                .0
                .iter()
                .map(|child| densify(child, max_segment_length, max_output_coordinates))
                .collect::<Result<Vec<_>, _>>()?
                .into(),
        ),
        Geometry::Line(_) | Geometry::Rect(_) | Geometry::Triangle(_) => {
            return Err(ExtendedAlgorithmError::UnsupportedGeometry {
                operation: "densify",
                actual: geometry_type(geometry),
            });
        }
    };
    let actual = coordinate_count(&output)?;
    if actual > max_output_coordinates {
        return Err(ExtendedAlgorithmError::OutputLimit {
            actual,
            limit: max_output_coordinates,
        });
    }
    validate_output(output)
}

/// Porta ogni coordinata sul nodo piu' vicino di una griglia.
///
/// La griglia ha passo `grid_size` e origine in `(0, 0)`:
/// `round(x / grid_size) * grid_size`
/// per asse (a meta' strada si arrotonda lontano da zero), con `-0.0` reso
/// `0.0`.
///
/// Non ripara e non semplifica: i vertici consecutivi che cadono sullo
/// stesso nodo restano duplicati, e un collasso che rende la geometria non
/// valida (linea ridotta a un punto, anello degenere o auto-intersecato) si
/// rifiuta invece di essere corretto in silenzio.
///
/// # Errors
///
/// - `InvalidInput`: coordinate NaN o infinite, o geometria OGC non valida;
///   `ValidazioneNonConclusa` se la validazione non conclude;
/// - `InvalidParameter`: `grid_size` non finita o non positiva;
/// - `InvalidOutput`: coordinata non finita dopo l'arrotondamento
///   (overflow), o geometria prodotta non valida (collasso che viola la
///   validita' OGC).
pub fn snap_to_grid(
    geometry: &Geometry<f64>,
    grid_size: f64,
) -> Result<Geometry<f64>, ExtendedAlgorithmError> {
    validate_input(geometry)?;
    if !grid_size.is_finite() || grid_size <= 0.0 {
        return Err(ExtendedAlgorithmError::InvalidParameter {
            name: "grid_size",
            reason: "deve essere finita e maggiore di zero",
        });
    }
    let output: Geometry<f64> = geometry.try_map_coords(|coordinate| {
        let x = (coordinate.x / grid_size).round() * grid_size;
        let y = (coordinate.y / grid_size).round() * grid_size;
        if x.is_finite() && y.is_finite() {
            Ok(Coord {
                x: if x == 0.0 { 0.0 } else { x },
                y: if y == 0.0 { 0.0 } else { y },
            })
        } else {
            Err(ExtendedAlgorithmError::InvalidOutput(
                "overflow durante lo snap".to_owned(),
            ))
        }
    })?;
    validate_output(output)
}

/// Triangolazione Delaunay non vincolata dell'input, come poligoni.
///
/// I vertici sono tutte le coordinate della geometria (anche quelle di
/// linee e anelli), i duplicati contano una volta. Ogni triangolo e' un
/// anello chiuso antiorario `[a, b, c, a]` che parte dal vertice comparso
/// per primo nell'ingresso; i triangoli sono in ordine lessicografico della
/// prima comparsa dei loro tre vertici. Stesso ingresso, stessa uscita.
/// L'ordine e' parte del contratto (`DefinedOrder` nel catalogo,
/// `semantic_version` 2).
///
/// Nessuna precisione come argomento: i vertici d'uscita sono i punti
/// d'ingresso con i loro bit e i predicati di `spade` sono esatti, quindi
/// nessuna coordinata calcolata puo' spostarsi.
///
/// Costruita con il caricamento in blocco di `spade` (`crate::triangolazione`):
/// sugli ingressi senza quattro punti cocircolari i triangoli sono quelli
/// dell'inserimento incrementale di `geo` 0.33.1, con gli stessi bit; sugli
/// ingressi degeneri e' un'altra triangolazione di Delaunay valida (docs/limiti.md,
/// «geo.delaunay e geo.voronoi»).
///
/// Meno di tre punti distinti, o punti tutti collineari, danno zero
/// triangoli, senza errore.
///
/// # Errors
///
/// - `InvalidInput`: coordinate NaN o infinite, o geometria OGC non valida;
///   `ValidazioneNonConclusa` se la validazione non conclude;
/// - `IndexOverflow`: conteggio non rappresentabile come `u64`;
/// - `CoordinateLimit`: coordinate di input (duplicati compresi) oltre
///   `max_input_coordinates`;
/// - `Triangulation`: triangolazione fallita (coordinata fuori dal dominio
///   di `spade`, `[2^-142, 2^201]` in modulo o zero, per il primo punto
///   fuori in ordine d'ingresso; oppure vertici persi o fusi dal
///   caricamento in blocco);
/// - `CalcoloNonConcluso`: la triangolazione e' andata in panico;
/// - `OutputLimit`: triangoli prodotti oltre `max_triangles`;
/// - `InvalidOutput`: triangolo prodotto non valido.
pub fn delaunay(
    geometry: &Geometry<f64>,
    max_input_coordinates: u64,
    max_triangles: u64,
) -> Result<Vec<Polygon<f64>>, ExtendedAlgorithmError> {
    validate_input(geometry)?;
    let coordinates = coordinate_count(geometry)?;
    if coordinates > max_input_coordinates {
        return Err(ExtendedAlgorithmError::CoordinateLimit {
            actual: coordinates,
            limit: max_input_coordinates,
        });
    }
    let punti: Vec<Coord<f64>> = geometry.coords_iter().collect();
    let triangoli = protetto(|| {
        crate::triangolazione::triangola(&punti).map(|costruita| triangoli_canonici(&costruita))
    })?
    .map_err(errore_di_triangolazione)?;
    let actual =
        u64::try_from(triangoli.len()).map_err(|_| ExtendedAlgorithmError::IndexOverflow)?;
    if actual > max_triangles {
        return Err(ExtendedAlgorithmError::OutputLimit {
            actual,
            limit: max_triangles,
        });
    }
    triangoli
        .into_iter()
        .map(|triangle| {
            let polygon = triangle.to_polygon();
            validate_output(Geometry::Polygon(polygon.clone()))?;
            Ok(polygon)
        })
        .collect()
}

/// Le facce interne come triangoli nell'ordine canonico di [`delaunay`]:
/// ogni faccia antioraria (come `spade` la da') ruotata a partire dal
/// vertice di rango minimo, le facce ordinate per i ranghi dei tre vertici.
/// Due facce distinte non hanno la stessa terna, quindi l'ordine e' totale.
fn triangoli_canonici(costruita: &crate::triangolazione::Triangolazione) -> Vec<Triangle<f64>> {
    let mut facce: Vec<[crate::triangolazione::Sito; 3]> = costruita
        .triangolazione
        .inner_faces()
        .map(|faccia| faccia.vertices().map(|vertice| *vertice.data()))
        .collect();
    for faccia in &mut facce {
        let [a, b, c] = *faccia;
        *faccia = if b.rango < a.rango && b.rango < c.rango {
            [b, c, a]
        } else if c.rango < a.rango && c.rango < b.rango {
            [c, a, b]
        } else {
            [a, b, c]
        };
    }
    facce.sort_unstable_by_key(|faccia| faccia.map(|sito| sito.rango));
    facce
        .into_iter()
        .map(|[a, b, c]| Triangle::new(a.coordinata, b.coordinata, c.coordinata))
        .collect()
}

/// Lo stesso messaggio che `geo` 0.33.1 dava da
/// `unconstrained_triangulation` (`SpadeError(TooSmall)`, ...): nessun dato.
fn errore_di_triangolazione(
    errore: crate::triangolazione::ErroreTriangolazione,
) -> ExtendedAlgorithmError {
    use crate::triangolazione::ErroreTriangolazione;
    use geo::algorithm::triangulate_delaunay::TriangulationError;
    ExtendedAlgorithmError::Triangulation(match errore {
        ErroreTriangolazione::Inserimento(inserimento) => {
            TriangulationError::SpadeError(inserimento).to_string()
        }
        ErroreTriangolazione::VerticiInattesi => {
            "vertici della triangolazione diversi dai punti distinti".to_owned()
        }
    })
}

fn validate_ratio(value: f64, name: &'static str) -> Result<(), ExtendedAlgorithmError> {
    if !value.is_finite() || !(0.0..=1.0).contains(&value) {
        return Err(ExtendedAlgorithmError::InvalidParameter {
            name,
            reason: "deve essere finito e compreso tra zero e uno",
        });
    }
    Ok(())
}

/// Punto sulla linea alla frazione `ratio` della lunghezza dall'inizio.
///
/// La lunghezza e' quella euclidea: la distanza `ratio * L` si percorre
/// lato per lato e il punto
/// si interpola sul lato in cui cade. `ratio` 0 da' il primo vertice con i
/// suoi bit; 1 l'ultimo, a meno di qualche `ulp` (la distanza residua
/// sull'ultimo lato e' una differenza di somme in `f64`).
///
/// `None` se la linea e' vuota (l'unica linea valida senza un punto).
///
/// # Errors
///
/// - `InvalidParameter`: `ratio` non finito o fuori dall'intervallo [0, 1];
/// - `InvalidInput`: coordinate NaN o infinite, o linea non valida (meno di
///   due punti distinti); `ValidazioneNonConclusa` se la validazione non
///   conclude;
/// - `CalcoloNonConcluso`: l'interpolazione di `geo` e' andata in panico.
pub fn line_interpolate_point(
    line: &LineString<f64>,
    ratio: f64,
) -> Result<Option<Point<f64>>, ExtendedAlgorithmError> {
    validate_ratio(ratio, "ratio")?;
    validate_input(&Geometry::LineString(line.clone()))?;
    protetto(|| Euclidean.point_at_ratio_from_start(line, ratio))
}

/// Porzione di linea tra le frazioni `start_ratio` e `end_ratio` della
/// lunghezza euclidea.
///
/// Il primo e l'ultimo punto sono quelli di [`line_interpolate_point`] alle
/// due frazioni; in mezzo restano, con i loro bit, i vertici d'ingresso la
/// cui distanza cumulata dall'inizio e' strettamente fra le due distanze
/// (un vertice uguale al precedente non si ripete). `None` se la linea e'
/// vuota; un `Point` se le due frazioni sono uguali (`==`: `-0.0` e `0.0`
/// sono uguali).
///
/// # Errors
///
/// - `InvalidParameter`: frazione non finita o fuori dall'intervallo
///   [0, 1], oppure `start_ratio` maggiore di `end_ratio`;
/// - `InvalidInput`: coordinate NaN o infinite, o linea non valida;
///   `ValidazioneNonConclusa` se la validazione non conclude;
/// - `CalcoloNonConcluso`: interpolazione o lunghezza di `geo` andate in
///   panico;
/// - `InvalidOutput`: porzione prodotta non valida (per esempio due
///   frazioni diverse i cui punti coincidono in `f64`: una linea di un solo
///   punto distinto).
pub fn line_substring(
    line: &LineString<f64>,
    start_ratio: f64,
    end_ratio: f64,
) -> Result<Option<Geometry<f64>>, ExtendedAlgorithmError> {
    validate_ratio(start_ratio, "start_ratio")?;
    validate_ratio(end_ratio, "end_ratio")?;
    if start_ratio > end_ratio {
        return Err(ExtendedAlgorithmError::InvalidParameter {
            name: "start_ratio/end_ratio",
            reason: "start_ratio non puo superare end_ratio",
        });
    }
    validate_input(&Geometry::LineString(line.clone()))?;
    let Some(start) = protetto(|| Euclidean.point_at_ratio_from_start(line, start_ratio))? else {
        return Ok(None);
    };
    // Uguaglianza esatta intenzionale: rapporti uguali come numeri (`==`,
    // quindi `-0.0 == 0.0`) definiscono il caso degenere (punto), senza
    // tolleranze implicite.
    #[allow(clippy::float_cmp)]
    if start_ratio == end_ratio {
        return Ok(Some(Geometry::Point(start)));
    }
    let Some(end) = protetto(|| Euclidean.point_at_ratio_from_start(line, end_ratio))? else {
        return Ok(None);
    };
    let total = protetto(|| geo::algorithm::line_measures::Length::length(&Euclidean, line))?;
    if total == 0.0 {
        return Ok(Some(Geometry::Point(start)));
    }
    let start_distance = start_ratio * total;
    let end_distance = end_ratio * total;
    let mut coordinates = vec![start.0];
    let mut traversed = 0.0;
    for segment in line.lines() {
        let segment_length =
            (segment.end.x - segment.start.x).hypot(segment.end.y - segment.start.y);
        traversed += segment_length;
        if traversed > start_distance
            && traversed < end_distance
            && coordinates.last() != Some(&segment.end)
        {
            coordinates.push(segment.end);
        }
    }
    if coordinates.last() != Some(&end.0) {
        coordinates.push(end.0);
    }
    validate_output(Geometry::LineString(LineString::new(coordinates))).map(Some)
}

/// Distanza di Fréchet discreta tra due linee.
///
/// E' `FrechetDistance` di `geo` con la metrica `Euclidean`: si accoppiano
/// solo i vertici, quindi un
/// vertice in piu' su un lato dritto puo' cambiare il risultato (la distanza
/// continua sarebbe minore o uguale). Simmetrica; il verso delle linee conta.
///
/// Il lavoro quadratico, `vertici(left) * vertici(right)`, e' limitato da
/// `max_coordinate_pairs` prima del calcolo; `None` se una delle due linee e'
/// vuota.
///
/// # Errors
///
/// - `InvalidInput`: coordinate NaN o infinite, o linea non valida (prima
///   `left`, poi `right`); `ValidazioneNonConclusa` se la validazione non
///   conclude;
/// - `IndexOverflow`: conteggio coordinate non rappresentabile come `u64`;
/// - `WorkLimit`: coppie di coordinate oltre `max_coordinate_pairs` (o
///   prodotto non rappresentabile come `u64`);
/// - `CalcoloNonConcluso`: il calcolo di `geo` e' andato in panico.
pub fn frechet_distance(
    left: &LineString<f64>,
    right: &LineString<f64>,
    max_coordinate_pairs: u64,
) -> Result<Option<f64>, ExtendedAlgorithmError> {
    validate_input(&Geometry::LineString(left.clone()))?;
    validate_input(&Geometry::LineString(right.clone()))?;
    let left_count =
        u64::try_from(left.0.len()).map_err(|_| ExtendedAlgorithmError::IndexOverflow)?;
    let right_count =
        u64::try_from(right.0.len()).map_err(|_| ExtendedAlgorithmError::IndexOverflow)?;
    if left_count == 0 || right_count == 0 {
        return Ok(None);
    }
    let actual = left_count
        .checked_mul(right_count)
        .ok_or(ExtendedAlgorithmError::WorkLimit {
            actual: u64::MAX,
            limit: max_coordinate_pairs,
        })?;
    if actual > max_coordinate_pairs {
        return Err(ExtendedAlgorithmError::WorkLimit {
            actual,
            limit: max_coordinate_pairs,
        });
    }
    Ok(Some(protetto(|| Euclidean.frechet_distance(left, right))?))
}

fn validate_geographic_geometry(geometry: &Geometry<f64>) -> Result<(), ExtendedAlgorithmError> {
    validate_input(geometry)?;
    if geometry.coords_iter().any(|coordinate| {
        !(-180.0..=180.0).contains(&coordinate.x) || !(-90.0..=90.0).contains(&coordinate.y)
    }) {
        return Err(ExtendedAlgorithmError::InvalidGeographicCoordinate);
    }
    Ok(())
}

/// Azimut geodetico iniziale, in gradi, da `origin` a `destination`.
///
/// E' la direzione della geodetica piu' breve in partenza da `origin`,
/// misurata in senso orario dal nord (nord 0, est 90, sud 180, ovest 270),
/// in `[0, 360)`.
///
/// Coordinate `x` = longitudine, `y` = latitudine, in gradi. Il calcolo e'
/// il problema inverso di Karney (2013), `geographiclib-rs`,
/// sull'ellissoide del datum del CRS (`ellissoide`, costruito con
/// [`EllissoideGeodetico::da_crs`]), mai su un ellissoide di comodo.
///
/// Dove l'azimut non e' definito il kernel **rifiuta**, invece di rendere
/// il valore convenzionale di `geographiclib` (180 per due punti
/// coincidenti, 0 per due antipodi):
///
/// - punti coincidenti (distanza geodetica nulla, compresi `-180` e `180`
///   alla stessa latitudine): nessuna direzione;
/// - origine su un polo (latitudine `±90`): ogni direzione e' sud (o nord),
///   e l'azimut dipenderebbe solo dalla longitudine scritta;
/// - geodetica piu' breve non unica: destinazione sul luogo di taglio
///   dell'origine (latitudine opposta e differenza di longitudine vicina a
///   180, antipodi compresi). Le due geodetiche partono con azimut diversi
///   (la seconda, per simmetria, con l'azimut d'arrivo della prima); si
///   rifiuta quando le due direzioni si separano, alla distanza della
///   destinazione, di piu' della precisione di 1 cm.
///
/// # Errors
///
/// - `InvalidInput`: coordinate NaN o infinite; `ValidazioneNonConclusa` se
///   la validazione non conclude;
/// - `InvalidGeographicCoordinate`: longitudine fuori da [-180, 180] o
///   latitudine fuori da [-90, 90];
/// - `AzimutNonDefinito`: i tre casi sopra;
/// - `InvalidOutput`: azimut non finito (mai atteso);
/// - `CalcoloNonConcluso`: il calcolo e' andato in panico.
pub fn geodesic_bearing_degrees(
    origin: Point<f64>,
    destination: Point<f64>,
    ellissoide: &EllissoideGeodetico,
) -> Result<f64, ExtendedAlgorithmError> {
    validate_geographic_geometry(&Geometry::MultiPoint(vec![origin, destination].into()))?;
    // Il polo e' un valore esatto della latitudine: il confronto e' esatto.
    #[allow(clippy::float_cmp)]
    let su_un_polo = origin.y().abs() == 90.0;
    if su_un_polo {
        return Err(ExtendedAlgorithmError::AzimutNonDefinito(
            "origine su un polo",
        ));
    }
    let (distanza, azimut_partenza, azimut_arrivo, _arco): (f64, f64, f64, f64) = protetto(|| {
        ellissoide
            .geodetica()
            .inverse(origin.y(), origin.x(), destination.y(), destination.x())
    })?;
    if !(distanza.is_finite() && azimut_partenza.is_finite() && azimut_arrivo.is_finite()) {
        return Err(ExtendedAlgorithmError::InvalidOutput(
            "azimut NaN o infinito".to_owned(),
        ));
    }
    if distanza == 0.0 {
        return Err(ExtendedAlgorithmError::AzimutNonDefinito(
            "punti coincidenti",
        ));
    }
    // Luogo di taglio: solo con latitudini opposte la simmetria (riflessione
    // sull'equatore e sul meridiano medio) porta la geodetica in una seconda,
    // che parte con l'azimut d'arrivo della prima. Unica se le due
    // coincidono entro 1 cm alla distanza della destinazione.
    #[allow(clippy::float_cmp)] // Latitudini opposte esatte: e' la simmetria.
    let opposte = destination.y() == -origin.y();
    if opposte {
        let scarto = scarto_angolare_gradi(azimut_partenza, azimut_arrivo).to_radians();
        if scarto * distanza > GROUND_PRECISION_METRES {
            return Err(ExtendedAlgorithmError::AzimutNonDefinito(
                "geodetica piu' breve non unica (luogo di taglio, antipodi)",
            ));
        }
    }
    // Stessa normalizzazione di `Bearing` di `geo`: `[0, 360)`.
    let azimut = (azimut_partenza + 360.0) % 360.0;
    if !azimut.is_finite() {
        return Err(ExtendedAlgorithmError::InvalidOutput(
            "azimut NaN o infinito".to_owned(),
        ));
    }
    Ok(azimut)
}

/// La differenza fra due azimut in gradi, in `[0, 180]`.
fn scarto_angolare_gradi(primo: f64, secondo: f64) -> f64 {
    let scarto = (primo - secondo).rem_euclid(360.0);
    scarto.min(360.0 - scarto)
}

/// Area geodetica, in metri quadrati, di poligoni e multi-poligoni
/// sull'ellissoide del datum del CRS (`ellissoide`, costruito con
/// [`EllissoideGeodetico::da_crs`]).
///
/// Coordinate `x` = longitudine, `y` = latitudine, in gradi; i lati sono
/// geodetiche fra vertici consecutivi. Ogni poligono si orienta prima (esterno
/// antiorario, buchi orari, nel piano lon/lat), quindi il verso d'ingresso
/// non conta; l'area e' quella dell'esterno meno quella dei buchi (il
/// calcolo di `geodesic_area_unsigned` di `geo`, algoritmo di Karney, sul
/// `PolygonArea` di `geographiclib-rs` dell'ellissoide dato). Un
/// `MultiPolygon` somma le aree dei suoi poligoni, in ordine. Un poligono o
/// un multi-poligono vuoto da' `-0.0`.
///
/// # Errors
///
/// - `InvalidInput`: coordinate NaN o infinite, o geometria OGC non valida;
///   `ValidazioneNonConclusa` se la validazione non conclude;
/// - `InvalidGeographicCoordinate`: coordinate fuori intervallo lon/lat;
/// - `InvalidInput`: un lato con differenza di longitudine di almeno 180
///   gradi (poligono sull'antimeridiano o attorno a un polo); un lato
///   troppo lungo o troppo vicino a un polo, o due lati piu' vicini dello
///   scarto fra geodetiche e corde, per cui la topologia delle geodetiche
///   non e' garantita uguale a quella del piano lon/lat
///   (`geodetica::verifica_topologia_geodetica`); un anello che, letto come
///   geodetiche, gira al contrario del piano o copre mezzo ellissoide;
/// - `UnsupportedGeometry`: geometria diversa da `Polygon`/`MultiPolygon`;
/// - `CalcoloNonConcluso`: il calcolo e' andato in panico;
/// - `InvalidOutput`: area NaN o infinita.
pub fn geodesic_area_m2(
    geometry: &Geometry<f64>,
    ellissoide: &EllissoideGeodetico,
) -> Result<f64, ExtendedAlgorithmError> {
    validate_geographic_geometry(geometry)?;
    // L'interno si sceglie orientando nel piano lon/lat. Un lato di 180
    // gradi o piu' di longitudine (antimeridiano, anello attorno a un polo)
    // la geodetica lo percorre dall'altra parte: si rifiuta subito. Gli altri
    // casi in cui le geodetiche girano al contrario del piano li rifiuta
    // `area_poligono`, dal segno dell'area di ogni anello.
    let poligoni: &[Polygon<f64>] = match geometry {
        Geometry::Polygon(polygon) => std::slice::from_ref(polygon),
        Geometry::MultiPolygon(MultiPolygon(polygons)) => polygons,
        _ => &[],
    };
    if poligoni
        .iter()
        .flat_map(|polygon| std::iter::once(polygon.exterior()).chain(polygon.interiors()))
        .flat_map(LineString::lines)
        .any(|lato| (lato.end.x - lato.start.x).abs() >= 180.0)
    {
        return Err(ExtendedAlgorithmError::InvalidInput(
            "lato di almeno 180 gradi di longitudine (antimeridiano o polo): interno ambiguo"
                .to_owned(),
        ));
    }
    // La topologia delle geodetiche (anelli semplici, buchi dentro
    // l'esterno, parti disgiunte) deve essere quella del piano lon/lat, dove
    // la validazione OGC l'ha verificata: altrimenti l'area sommerebbe o
    // sottrarrebbe regioni sbagliate, in silenzio.
    let anelli: Vec<&LineString<f64>> = poligoni
        .iter()
        .flat_map(|polygon| std::iter::once(polygon.exterior()).chain(polygon.interiors()))
        .collect();
    protetto(|| crate::geodetica::verifica_topologia_geodetica(&anelli, ellissoide))?
        .map_err(|motivo| ExtendedAlgorithmError::InvalidInput(motivo.to_owned()))?;
    let area = match geometry {
        Geometry::Polygon(polygon) => {
            protetto(|| area_poligono(&polygon.orient(Direction::Default), ellissoide))?
                .map_err(|motivo| ExtendedAlgorithmError::InvalidInput(motivo.to_owned()))?
        }
        Geometry::MultiPolygon(MultiPolygon(polygons)) => {
            // Stessa somma di prima (`Iterator::sum`), sui valori protetti.
            let aree = polygons
                .iter()
                .map(|polygon| {
                    protetto(|| area_poligono(&polygon.orient(Direction::Default), ellissoide))?
                        .map_err(|motivo| ExtendedAlgorithmError::InvalidInput(motivo.to_owned()))
                })
                .collect::<Result<Vec<f64>, _>>()?;
            aree.into_iter().sum()
        }
        _ => {
            return Err(ExtendedAlgorithmError::UnsupportedGeometry {
                operation: "geodesic_area",
                actual: geometry_type(geometry),
            })
        }
    };
    if !area.is_finite() {
        return Err(ExtendedAlgorithmError::InvalidOutput(
            "area NaN o infinita".to_owned(),
        ));
    }
    Ok(area)
}

/// L'area di un poligono orientato nel piano lon/lat (esterno antiorario,
/// buchi orari), sull'ellissoide dato: il calcolo di
/// `geodesic_area_unsigned` di `geo` 0.33.1 (`geodesic_area(poly, sign =
/// false, reverse = false, exterior_only = false)`) con una verifica in piu'.
///
/// Ogni anello si calcola **con segno** nel verso atteso: un'area positiva e
/// minore di mezzo ellissoide vuol dire che l'anello, letto come geodetiche,
/// gira davvero nel verso del piano, e allora il valore e' lo stesso, al
/// bit, di `compute(false)` (la riduzione con e senza segno coincide in
/// `(0, A/2)`). Un'area non positiva vuol dire che le geodetiche girano al
/// contrario (un lato lungo che passa dall'altra parte di un vertice, per
/// esempio il triangolo (0 30, 170 30, 85 31)): `compute(false)` renderebbe
/// l'area del complemento sul globo, in silenzio. `Err` in quel caso, e per
/// un anello di mezzo ellissoide o piu' (il segno non distingue piu'
/// l'interno).
fn area_poligono(
    polygon: &Polygon<f64>,
    ellissoide: &EllissoideGeodetico,
) -> Result<f64, &'static str> {
    let geodetica = ellissoide.geodetica();
    let area_anello = |anello: &LineString<f64>, verso: Winding| {
        let mut calcolo = PolygonArea::new(geodetica, verso);
        for punto in anello.points() {
            calcolo.add_point(punto.y(), punto.x());
        }
        let (_perimetro, area, _punti) = calcolo.compute(true);
        // Un anello vuoto (poligono vuoto) ha area nulla, come prima.
        #[allow(clippy::float_cmp)]
        let vuoto = area == 0.0 && anello.0.len() < 4;
        if area > 0.0 || vuoto {
            Ok(area)
        } else {
            Err(
                "anello che, letto come geodetiche, gira nel verso opposto a quello del piano \
                 lon/lat o copre mezzo ellissoide: interno ambiguo",
            )
        }
    };
    let esterna = area_anello(polygon.exterior(), Winding::CounterClockwise)?;
    let mut interne = 0.0;
    for anello in polygon.interiors() {
        interne += area_anello(anello, Winding::Clockwise)?;
    }
    Ok(esterna - interne)
}

/// Il referto di [`geometry_diagnostics`]: un campo per ognuna delle dieci
/// colonne di `geo.geometry_diagnostics` (`bounds` ne da' quattro).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct GeometryDiagnostics {
    /// Tipo `geo` della geometria: `Point`, `LineString`, `Polygon`,
    /// `MultiPoint`, `MultiLineString`, `MultiPolygon`,
    /// `GeometryCollection` (dal WKB non arrivano `Line`, `Rect`,
    /// `Triangle`).
    pub geometry_type: &'static str,
    /// Coordinate della geometria, duplicati e vertici di chiusura degli
    /// anelli compresi.
    pub coordinate_count: u64,
    /// `coordinate_count == 0`.
    pub is_empty: bool,
    /// Ogni coordinata e' finita (niente NaN, niente infiniti).
    pub is_finite: bool,
    /// La geometria supera la validazione OGC; sempre `false` con
    /// coordinate non finite, che non si validano.
    pub is_valid: bool,
    /// `None` se valida; altrimenti la ragione classificata, senza
    /// coordinate: `coordinate NaN o infinite` oppure uno dei testi di
    /// `RagioneNonValida` (`punti distinti insufficienti`, `anello con
    /// auto-intersezione`, `anelli che si intersecano`, `anello interno fuori
    /// dal proprio esterno`, `poligoni sovrapposti`, `forma non valida non
    /// ulteriormente distinta`).
    pub validity_reason: Option<String>,
    /// Rettangolo d'ingombro `[minx, miny, maxx, maxy]`; `None` se la
    /// geometria e' vuota o ha coordinate non finite.
    pub bounds: Option<[f64; 4]>,
}

/// Referto diagnostico di una geometria: tipo, conteggio delle coordinate,
/// vuota, finita, valida e perche' no, rettangolo d'ingombro.
///
/// Accetta di proposito la topologia non valida e le coordinate non finite:
/// e' il suo mestiere descriverle. Non esegue alcun algoritmo sulle
/// coordinate non finite (niente validazione, niente rettangolo).
///
/// # Errors
///
/// - `ValidazioneNonConclusa`: la validazione OGC non conclude; il referto
///   non si scrive, perche' dichiarerebbe un verdetto che non esiste;
/// - `IndexOverflow`: conteggio delle coordinate non rappresentabile come
///   `u64`.
pub fn geometry_diagnostics(
    geometry: &Geometry<f64>,
) -> Result<GeometryDiagnostics, ExtendedAlgorithmError> {
    use geo::BoundingRect;

    let is_finite = geometry
        .coords_iter()
        .all(|coordinate| coordinate.x.is_finite() && coordinate.y.is_finite());
    // Una validazione che non conclude non produce una diagnosi: scriverla in
    // `is_valid`/`validity_reason` significherebbe dichiarare invalida una
    // geometria su cui nessuno ha deciso. Questa funzione accetta la topologia
    // invalida — e' il suo mestiere — ma non un verdetto che non esiste, e in
    // quel caso rende errore invece di un referto inventato.
    let validation = if is_finite {
        match geometry.validazione_protetta() {
            Ok(()) => Ok(()),
            Err(esito) => match esito {
                EsitoValidazione::NonValida(ragione) => Err(ragione.to_string()),
                EsitoValidazione::NonConclusa(forma) => {
                    return Err(ExtendedAlgorithmError::ValidazioneNonConclusa(forma));
                }
            },
        }
    } else {
        Err("coordinate NaN o infinite".to_owned())
    };
    let bounds = if is_finite {
        geometry
            .bounding_rect()
            .map(|rect| [rect.min().x, rect.min().y, rect.max().x, rect.max().y])
    } else {
        None
    };
    let coordinate_count = coordinate_count(geometry)?;
    Ok(GeometryDiagnostics {
        geometry_type: geometry_type(geometry),
        coordinate_count,
        is_empty: coordinate_count == 0,
        is_finite,
        is_valid: validation.is_ok(),
        validity_reason: validation.err(),
        bounds,
    })
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
struct EndpointKey {
    x: u64,
    y: u64,
}

impl EndpointKey {
    fn new(coordinate: Coord<f64>) -> Self {
        fn canonical_bits(value: f64) -> u64 {
            if value == 0.0 {
                0.0_f64.to_bits()
            } else {
                value.to_bits()
            }
        }
        Self {
            x: canonical_bits(coordinate.x),
            y: canonical_bits(coordinate.y),
        }
    }
}

#[derive(Clone, Copy)]
struct MergeEdge<'a> {
    line: &'a LineString<f64>,
    start: EndpointKey,
    end: EndpointKey,
}

fn collect_lines<'a>(
    geometry: &'a Geometry<f64>,
    operation: &'static str,
    output: &mut Vec<&'a LineString<f64>>,
) -> Result<(), ExtendedAlgorithmError> {
    match geometry {
        Geometry::LineString(line) => output.push(line),
        Geometry::MultiLineString(lines) => output.extend(lines.0.iter()),
        Geometry::GeometryCollection(collection) => {
            for child in &collection.0 {
                collect_lines(child, operation, output)?;
            }
        }
        _ => {
            return Err(ExtendedAlgorithmError::UnsupportedGeometry {
                operation,
                actual: geometry_type(geometry),
            });
        }
    }
    Ok(())
}

fn walk_merged_path(
    edges: &[MergeEdge<'_>],
    adjacency: &HashMap<EndpointKey, Vec<usize>>,
    used: &mut [bool],
    start_node: EndpointKey,
    first_edge: usize,
) -> LineString<f64> {
    let mut output = Vec::new();
    let mut current_node = start_node;
    let mut edge_index = first_edge;
    loop {
        if used[edge_index] {
            break;
        }
        let edge = &edges[edge_index];
        let forward = edge.start == current_node;
        if forward {
            for &coordinate in &edge.line.0 {
                if output.last() != Some(&coordinate) {
                    output.push(coordinate);
                }
            }
        } else {
            for &coordinate in edge.line.0.iter().rev() {
                if output.last() != Some(&coordinate) {
                    output.push(coordinate);
                }
            }
        }
        used[edge_index] = true;
        current_node = if forward { edge.end } else { edge.start };
        let incident = &adjacency[&current_node];
        if incident.len() != 2 {
            break;
        }
        let Some(next) = incident.iter().copied().find(|candidate| !used[*candidate]) else {
            break;
        };
        edge_index = next;
    }
    LineString::new(output)
}

/// Fonde le linee in percorsi massimali.
///
/// Due linee si uniscono solo in un estremo condiviso da esattamente due
/// linee (grado 2). Un nodo di grado diverso da due e' sempre un confine,
/// come nel line merge di GEOS e `PostGIS` (`ST_LineMerge`); gli estremi si
/// confrontano per bit, con `-0.0`
/// uguale a `0.0`, senza tolleranza.
///
/// Una linea percorsa al contrario si inverte per proseguire il percorso;
/// un vertice uguale al precedente non si ripete nelle giunzioni. Le linee
/// vuote si ignorano. Una linea chiusa (primo vertice uguale all'ultimo)
/// esce sempre da sola, com'e', e nel grado del suo nodo conta una volta.
/// L'uscita e' deterministica: prima, nell'ordine delle linee d'ingresso,
/// le linee chiuse e i percorsi che toccano un nodo di grado diverso da
/// due (ciascuno dal primo estremo della sua prima linea, se quello non ha
/// grado due, altrimenti dall'ultimo); poi gli anelli
/// fatti solo di nodi di grado due, ciascuno dalla prima linea non ancora
/// usata, a partire dal suo estremo minore (bit di `x`, poi di `y`, come
/// interi senza segno).
///
/// # Errors
///
/// - `InvalidInput`: coordinate NaN o infinite, o geometria OGC non valida;
///   `ValidazioneNonConclusa` se la validazione non conclude;
/// - `CoordinateLimit`: coordinate di input oltre `max_input_coordinates`;
/// - `UnsupportedGeometry`: geometria diversa da `LineString`,
///   `MultiLineString` o `GeometryCollection` (anche annidata);
/// - `IndexOverflow`: conteggio non rappresentabile come `u64`;
/// - `Internal`: invariante interna violata (linea non vuota senza estremi);
/// - `OutputLimit`: linee prodotte oltre `max_output_lines`;
/// - `InvalidOutput`: linea prodotta non valida.
pub fn line_merge(
    geometry: &Geometry<f64>,
    max_input_coordinates: u64,
    max_output_lines: u64,
) -> Result<Vec<LineString<f64>>, ExtendedAlgorithmError> {
    validate_input(geometry)?;
    let input_coordinates = coordinate_count(geometry)?;
    if input_coordinates > max_input_coordinates {
        return Err(ExtendedAlgorithmError::CoordinateLimit {
            actual: input_coordinates,
            limit: max_input_coordinates,
        });
    }
    let mut lines = Vec::new();
    collect_lines(geometry, "line_merge", &mut lines)?;
    lines.retain(|line| !line.0.is_empty());
    let mut edges = Vec::with_capacity(lines.len());
    // La mappa si interroga e non si itera mai: l'ordine d'uscita segue
    // l'ordine delle linee, non l'hash.
    let mut adjacency: HashMap<EndpointKey, Vec<usize>> = HashMap::new();
    for line in lines {
        let start = EndpointKey::new(line.0[0]);
        let end = EndpointKey::new(
            *line
                .0
                .last()
                .ok_or(ExtendedAlgorithmError::Internal("non-empty line"))?,
        );
        let index = edges.len();
        edges.push(MergeEdge { line, start, end });
        adjacency.entry(start).or_default().push(index);
        if end != start {
            adjacency.entry(end).or_default().push(index);
        }
    }

    let mut used = vec![false; edges.len()];
    let mut output = Vec::new();
    for index in 0..edges.len() {
        if used[index] {
            continue;
        }
        let edge = &edges[index];
        if edge.start == edge.end {
            used[index] = true;
            output.push(edge.line.clone());
            continue;
        }
        let start_degree = adjacency[&edge.start].len();
        let end_degree = adjacency[&edge.end].len();
        if start_degree != 2 || end_degree != 2 {
            let start = if start_degree == 2 {
                edge.end
            } else {
                edge.start
            };
            output.push(walk_merged_path(
                &edges, &adjacency, &mut used, start, index,
            ));
        }
    }
    for index in 0..edges.len() {
        if used[index] {
            continue;
        }
        let edge = &edges[index];
        let start = edge.start.min(edge.end);
        output.push(walk_merged_path(
            &edges, &adjacency, &mut used, start, index,
        ));
    }
    let actual = u64::try_from(output.len()).map_err(|_| ExtendedAlgorithmError::IndexOverflow)?;
    if actual > max_output_lines {
        return Err(ExtendedAlgorithmError::OutputLimit {
            actual,
            limit: max_output_lines,
        });
    }
    for line in &output {
        validate_output(Geometry::LineString(line.clone()))?;
    }
    Ok(output)
}

fn splitter_primitives(
    geometry: &Geometry<f64>,
    points: &mut Vec<Point<f64>>,
    lines: &mut Vec<Line<f64>>,
) -> Result<(), ExtendedAlgorithmError> {
    match geometry {
        Geometry::Point(point) => points.push(*point),
        Geometry::MultiPoint(multi) => points.extend(multi.0.iter().copied()),
        Geometry::LineString(line) => lines.extend(line.lines()),
        Geometry::MultiLineString(multi) => {
            for line in &multi.0 {
                lines.extend(line.lines());
            }
        }
        Geometry::Polygon(polygon) => {
            lines.extend(polygon.exterior().lines());
            for ring in polygon.interiors() {
                lines.extend(ring.lines());
            }
        }
        Geometry::MultiPolygon(multi) => {
            for polygon in &multi.0 {
                splitter_primitives(&Geometry::Polygon(polygon.clone()), points, lines)?;
            }
        }
        Geometry::GeometryCollection(collection) => {
            for child in &collection.0 {
                splitter_primitives(child, points, lines)?;
            }
        }
        _ => {
            return Err(ExtendedAlgorithmError::UnsupportedGeometry {
                operation: "split",
                actual: geometry_type(geometry),
            });
        }
    }
    Ok(())
}

fn ratio_on_source_segment(
    segment: Line<f64>,
    coordinate: Coord<f64>,
    distance_before: f64,
    total_length: f64,
) -> f64 {
    let segment_length = (segment.end.x - segment.start.x).hypot(segment.end.y - segment.start.y);
    if segment_length == 0.0 || total_length == 0.0 {
        return 0.0;
    }
    let local = (coordinate.x - segment.start.x).hypot(coordinate.y - segment.start.y);
    (distance_before + local.min(segment_length)) / total_length
}

#[derive(Clone, Copy)]
struct IndexedSegment {
    line: Line<f64>,
    distance_before: f64,
    length: f64,
    envelope: AABB<[f64; 2]>,
}

impl IndexedSegment {
    fn new(line: Line<f64>, distance_before: f64) -> Self {
        let length = (line.end.x - line.start.x).hypot(line.end.y - line.start.y);
        Self {
            line,
            distance_before,
            length,
            envelope: AABB::from_corners(
                [line.start.x.min(line.end.x), line.start.y.min(line.end.y)],
                [line.start.x.max(line.end.x), line.start.y.max(line.end.y)],
            ),
        }
    }
}

impl RTreeObject for IndexedSegment {
    type Envelope = AABB<[f64; 2]>;

    fn envelope(&self) -> Self::Envelope {
        self.envelope
    }
}

fn point_ratio_on_segment(
    segment: &IndexedSegment,
    point: Point<f64>,
    tolerance: f64,
    total_length: f64,
) -> Option<f64> {
    let dx = segment.line.end.x - segment.line.start.x;
    let dy = segment.line.end.y - segment.line.start.y;
    // Niente mul_add/FMA: la fusione cambia l'arrotondamento IEEE e
    // renderebbe il risultato diverso da piattaforma a piattaforma; la forma
    // non fusa e' il contratto numerico.
    #[allow(clippy::suboptimal_flops)]
    let length_squared = dx * dx + dy * dy;
    if length_squared == 0.0 {
        return None;
    }
    // Stessa forma non fusa, per lo stesso motivo.
    #[allow(clippy::suboptimal_flops)]
    let parameter = (((point.x() - segment.line.start.x) * dx
        + (point.y() - segment.line.start.y) * dy)
        / length_squared)
        .clamp(0.0, 1.0);
    let projected = Coord {
        x: segment.line.start.x + parameter * dx,
        y: segment.line.start.y + parameter * dy,
    };
    let distance = (point.x() - projected.x).hypot(point.y() - projected.y);
    let ratio = (segment.distance_before + parameter * segment.length) / total_length;
    // La proiezione puo' spostare di qualche ULP un punto decimale
    // esattamente collineare (per esempio x=14 su un segmento 0..100): una
    // tolleranza zero deve voler dire coincidenza topologica, non
    // uguaglianza dei bit.
    let coordinate_scale = segment
        .line
        .start
        .x
        .abs()
        .max(segment.line.start.y.abs())
        .max(segment.line.end.x.abs())
        .max(segment.line.end.y.abs())
        .max(point.x().abs())
        .max(point.y().abs())
        .max(1.0);
    let numeric_slack = coordinate_scale * f64::EPSILON * 16.0;
    if distance <= tolerance + numeric_slack {
        Some(ratio)
    } else {
        None
    }
}

fn expanded_point_envelope(point: Point<f64>, tolerance: f64) -> AABB<[f64; 2]> {
    let lower = |value: f64| {
        let result = value - tolerance;
        if result.is_finite() {
            result
        } else {
            -f64::MAX
        }
    };
    let upper = |value: f64| {
        let result = value + tolerance;
        if result.is_finite() {
            result
        } else {
            f64::MAX
        }
    };
    AABB::from_corners(
        [lower(point.x()), lower(point.y())],
        [upper(point.x()), upper(point.y())],
    )
}

/// Taglia una `LineString` con punti, linee o bordi di poligoni (le
/// sorgenti lineari di `geo.split`).
///
/// Un punto taglia dove dista dalla linea al piu' `tolerance` piu' un
/// margine numerico proporzionale al modulo delle coordinate; un lato del
/// tagliatore taglia nei punti d'incrocio e agli estremi di una
/// sovrapposizione. I tagli entro `tolerance` l'uno dall'altro (in
/// lunghezza) si fondono. Il lavoro si limita prima del ciclo quadratico
/// dei test d'intersezione, e le parti devono conservare la lunghezza della
/// sorgente.
///
/// # Errors
///
/// - `InvalidInput`: coordinate NaN o infinite, geometria OGC non valida, o
///   lunghezza della sorgente non finita per overflow numerico;
///   `ValidazioneNonConclusa` se la validazione non conclude;
/// - `CalcoloNonConcluso`: un calcolo di `geo` o `rstar` e' andato in
///   panico;
/// - `CoordinateLimit`: coordinate combinate (sorgente + splitter) oltre
///   `max_input_coordinates`;
/// - `InvalidParameter`: `tolerance` non finita o negativa;
/// - `UnsupportedGeometry`: splitter di tipo `Line`, `Rect` o `Triangle`;
/// - `IndexOverflow`: conteggio non rappresentabile come `u64`;
/// - `WorkLimit`: test di intersezione oltre `max_intersection_tests`;
/// - `OutputLimit`: parti o coordinate di output oltre i limiti richiesti;
/// - `InvalidOutput`: porzione prodotta non valida (propagata da
///   `line_substring`), o split che non conserva la lunghezza della
///   sorgente.
// Pipeline unica di split (walking dei segmenti, test di intersezione
// bounded, ricostruzione delle parti): lunghezza data dalla sequenza
// lineare dei passi del contratto, non da complessita' logica.
#[allow(clippy::too_many_lines)]
pub fn split_line(
    source: &LineString<f64>,
    splitter: &Geometry<f64>,
    tolerance: f64,
    max_input_coordinates: u64,
    max_intersection_tests: u64,
    max_output_parts: u64,
    max_output_coordinates: u64,
) -> Result<Vec<LineString<f64>>, ExtendedAlgorithmError> {
    validate_input(&Geometry::LineString(source.clone()))?;
    validate_input(splitter)?;
    let input_coordinates = u64::try_from(source.0.len())
        .map_err(|_| ExtendedAlgorithmError::IndexOverflow)?
        .checked_add(coordinate_count(splitter)?)
        .ok_or(ExtendedAlgorithmError::IndexOverflow)?;
    if input_coordinates > max_input_coordinates {
        return Err(ExtendedAlgorithmError::CoordinateLimit {
            actual: input_coordinates,
            limit: max_input_coordinates,
        });
    }
    if !tolerance.is_finite() || tolerance < 0.0 {
        return Err(ExtendedAlgorithmError::InvalidParameter {
            name: "tolerance",
            reason: "deve essere finita e non negativa",
        });
    }
    let total_length = protetto(|| Euclidean.length(source))?;
    if !total_length.is_finite() {
        return Err(ExtendedAlgorithmError::InvalidInput(
            "lunghezza non finita per overflow numerico".to_owned(),
        ));
    }
    if total_length == 0.0 {
        return Ok(vec![source.clone()]);
    }
    let mut distance_before = 0.0;
    let source_segments: Vec<_> = source
        .lines()
        .map(|line| {
            let indexed = IndexedSegment::new(line, distance_before);
            distance_before += indexed.length;
            indexed
        })
        .collect();
    let mut points = Vec::new();
    let mut splitter_segments = Vec::new();
    splitter_primitives(splitter, &mut points, &mut splitter_segments)?;
    let source_count =
        u64::try_from(source_segments.len()).map_err(|_| ExtendedAlgorithmError::IndexOverflow)?;
    let splitter_primitive_count = splitter_segments
        .len()
        .checked_add(points.len())
        .ok_or(ExtendedAlgorithmError::IndexOverflow)?;
    let splitter_work = u64::try_from(splitter_primitive_count)
        .map_err(|_| ExtendedAlgorithmError::IndexOverflow)?;
    let work =
        source_count
            .checked_mul(splitter_work)
            .ok_or(ExtendedAlgorithmError::WorkLimit {
                actual: u64::MAX,
                limit: max_intersection_tests,
            })?;
    if work > max_intersection_tests {
        return Err(ExtendedAlgorithmError::WorkLimit {
            actual: work,
            limit: max_intersection_tests,
        });
    }

    let mut ratios = Vec::new();
    let source_tree = protetto(|| RTree::bulk_load(source_segments.clone()))?;
    let coordinate_scale = source
        .coords_iter()
        .chain(splitter.coords_iter())
        .fold(1.0_f64, |scale, coordinate| {
            scale.max(coordinate.x.abs()).max(coordinate.y.abs())
        });
    // Niente mul_add/FMA: la fusione cambia l'arrotondamento IEEE e
    // renderebbe il risultato diverso da piattaforma a piattaforma; la forma
    // non fusa e' il contratto numerico.
    #[allow(clippy::suboptimal_flops)]
    let query_tolerance = (tolerance + coordinate_scale * f64::EPSILON * 16.0).min(f64::MAX);
    for point in points {
        let envelope = expanded_point_envelope(point, query_tolerance);
        let vicini: Vec<&IndexedSegment> = protetto(|| {
            source_tree
                .locate_in_envelope_intersecting(&envelope)
                .collect()
        })?;
        ratios.extend(
            vicini.into_iter().filter_map(|segment| {
                point_ratio_on_segment(segment, point, tolerance, total_length)
            }),
        );
    }
    let splitter_indexed: Vec<IndexedSegment> = splitter_segments
        .iter()
        .copied()
        .map(|line| IndexedSegment::new(line, 0.0))
        .collect();
    let splitter_tree = protetto(|| RTree::bulk_load(splitter_indexed))?;
    for source_segment in &source_segments {
        let incroci: Vec<&IndexedSegment> = protetto(|| {
            splitter_tree
                .locate_in_envelope_intersecting(&source_segment.envelope)
                .collect()
        })?;
        for splitter_segment in incroci {
            let (sorgente, taglio) = (source_segment.line, splitter_segment.line);
            match protetto(|| line_intersection(sorgente, taglio))? {
                Some(LineIntersection::SinglePoint { intersection, .. }) => {
                    ratios.push(ratio_on_source_segment(
                        source_segment.line,
                        intersection,
                        source_segment.distance_before,
                        total_length,
                    ));
                }
                Some(LineIntersection::Collinear { intersection }) => {
                    ratios.push(ratio_on_source_segment(
                        source_segment.line,
                        intersection.start,
                        source_segment.distance_before,
                        total_length,
                    ));
                    ratios.push(ratio_on_source_segment(
                        source_segment.line,
                        intersection.end,
                        source_segment.distance_before,
                        total_length,
                    ));
                }
                None => {}
            }
        }
    }
    ratios.retain(|ratio| ratio.is_finite() && *ratio > 0.0 && *ratio < 1.0);
    ratios.sort_by(f64::total_cmp);
    // La tolleranza del chiamante puo' fondere tagli vicini, per scelta.
    // Con tolleranza esatta (zero) si assorbono solo i duplicati numerici
    // attorno a un vertice condiviso, cosi' i pezzi minuscoli legittimi
    // restano.
    let ratio_tolerance = (tolerance / total_length).max(f64::EPSILON * 8.0);
    ratios.dedup_by(|left, right| (*left - *right).abs() <= ratio_tolerance);
    let part_count =
        u64::try_from(ratios.len() + 1).map_err(|_| ExtendedAlgorithmError::IndexOverflow)?;
    if part_count > max_output_parts {
        return Err(ExtendedAlgorithmError::OutputLimit {
            actual: part_count,
            limit: max_output_parts,
        });
    }
    let mut boundaries = Vec::with_capacity(ratios.len() + 2);
    boundaries.push(0.0);
    boundaries.extend(ratios);
    boundaries.push(1.0);
    let mut output = Vec::with_capacity(boundaries.len() - 1);
    for window in boundaries.windows(2) {
        let Some(piece) = line_substring(source, window[0], window[1])? else {
            continue;
        };
        let Geometry::LineString(piece) = piece else {
            continue;
        };
        output.push(piece);
    }
    let output_length: f64 = output
        .iter()
        .map(|line| protetto(|| Euclidean.length(line)))
        .collect::<Result<Vec<f64>, _>>()?
        .into_iter()
        .sum();
    let allowed_error = total_length.abs().max(1.0) * 1e-10;
    if (output_length - total_length).abs() > allowed_error {
        return Err(ExtendedAlgorithmError::InvalidOutput(
            "lo split non conserva la lunghezza".to_owned(),
        ));
    }
    let output_coordinates = output.iter().try_fold(0_u64, |total, line| {
        let count =
            u64::try_from(line.0.len()).map_err(|_| ExtendedAlgorithmError::IndexOverflow)?;
        total
            .checked_add(count)
            .ok_or(ExtendedAlgorithmError::IndexOverflow)
    })?;
    if output_coordinates > max_output_coordinates {
        return Err(ExtendedAlgorithmError::OutputLimit {
            actual: output_coordinates,
            limit: max_output_coordinates,
        });
    }
    Ok(output)
}

#[cfg(test)]
// Confronti float esatti intenzionali: le fixture sono costruite per
// produrre valori esatti (coordinate note, round-trip bit-esatti); il
// confronto per bit e' il contratto verificato, non un'approssimazione.
#[allow(clippy::float_cmp)]
mod tests {
    use super::*;
    use crate::test_support::rect;
    use geo::{line_string, polygon, Area};
    use proptest::prelude::*;

    #[test]
    fn densify_preflights_output_and_preserves_shape() {
        let line = Geometry::LineString(line_string![(x: 0.0, y: 0.0), (x: 10.0, y: 0.0)]);
        let output = densify(&line, 3.0, 10).unwrap();
        assert_eq!(output.coords_count(), 5);
        assert!(matches!(
            densify(&line, 0.001, 100),
            Err(ExtendedAlgorithmError::OutputLimit { .. })
        ));
    }

    #[test]
    fn snap_to_grid_rejects_collapsed_invalid_polygons() {
        let point = Geometry::Point(Point::new(1.24, -0.24));
        assert_eq!(
            snap_to_grid(&point, 0.5).unwrap(),
            Geometry::Point(Point::new(1.0, 0.0))
        );
        let tiny = rect(0.0, 0.0, 0.1, 0.1);
        assert!(snap_to_grid(&tiny, 1.0).is_err());
    }

    #[test]
    fn delaunay_is_bounded_and_covers_square() {
        let input = Geometry::MultiPoint(
            vec![
                Point::new(0.0, 0.0),
                Point::new(1.0, 0.0),
                Point::new(1.0, 1.0),
                Point::new(0.0, 1.0),
            ]
            .into(),
        );
        let triangles = delaunay(&input, 10, 10).unwrap();
        assert_eq!(triangles.len(), 2);
        assert!((triangles.iter().map(Area::unsigned_area).sum::<f64>() - 1.0).abs() < 1e-12);
        assert!(matches!(
            delaunay(&input, 3, 10),
            Err(ExtendedAlgorithmError::CoordinateLimit { .. })
        ));
    }

    #[test]
    fn linear_reference_is_deterministic() {
        let line = line_string![(x: 0.0, y: 0.0), (x: 0.0, y: 10.0), (x: 10.0, y: 10.0)];
        assert_eq!(
            line_interpolate_point(&line, 0.25).unwrap(),
            Some(Point::new(0.0, 5.0))
        );
        assert_eq!(
            line_substring(&line, 0.25, 0.75).unwrap(),
            Some(Geometry::LineString(line_string![
                (x: 0.0, y: 5.0), (x: 0.0, y: 10.0), (x: 5.0, y: 10.0)
            ]))
        );
        assert_eq!(
            line_substring(&line, 0.5, 0.5).unwrap(),
            Some(Geometry::Point(Point::new(0.0, 10.0)))
        );
    }

    #[test]
    fn frechet_bearing_and_geodesic_area_are_bounded() {
        let left = line_string![(x: 0.0, y: 0.0), (x: 2.0, y: 0.0)];
        let right = line_string![(x: 0.0, y: 1.0), (x: 2.0, y: 1.0)];
        assert_eq!(frechet_distance(&left, &right, 4).unwrap(), Some(1.0));
        assert!(matches!(
            frechet_distance(&left, &right, 3),
            Err(ExtendedAlgorithmError::WorkLimit { .. })
        ));
        assert_eq!(
            geodesic_bearing_degrees(
                Point::new(0.0, 0.0),
                Point::new(0.0, 2.0),
                &crate::geodetica::wgs84_di_prova()
            )
            .unwrap(),
            0.0
        );
        let square = rect(0.0, 0.0, 1.0, 1.0);
        let area = geodesic_area_m2(&square, &crate::geodetica::wgs84_di_prova()).unwrap();
        assert!(area > 12_000_000_000.0 && area < 13_000_000_000.0);
        let Geometry::Polygon(mut reversed) = square else {
            unreachable!()
        };
        reversed.exterior_mut(|ring| ring.0.reverse());
        assert!(
            (geodesic_area_m2(
                &Geometry::Polygon(reversed),
                &crate::geodetica::wgs84_di_prova()
            )
            .unwrap()
                - area)
                .abs()
                < 1e-6
        );
    }

    #[test]
    fn diagnostics_report_invalid_data_without_running_topology() {
        let valid = Geometry::Point(Point::new(2.0, 3.0));
        let report = geometry_diagnostics(&valid).unwrap();
        assert!(report.is_valid);
        assert_eq!(report.bounds, Some([2.0, 3.0, 2.0, 3.0]));

        let invalid = Geometry::Point(Point::new(f64::NAN, 3.0));
        let report = geometry_diagnostics(&invalid).unwrap();
        assert!(!report.is_finite);
        assert!(!report.is_valid);
        assert!(report.bounds.is_none());
    }

    #[test]
    fn line_merge_stops_at_branches_and_closes_cycles() {
        let chain = Geometry::MultiLineString(geo::MultiLineString(vec![
            line_string![(x: 0.0, y: 0.0), (x: 1.0, y: 0.0)],
            line_string![(x: 2.0, y: 0.0), (x: 1.0, y: 0.0)],
        ]));
        assert_eq!(line_merge(&chain, 100, 10).unwrap().len(), 1);

        let branch = Geometry::MultiLineString(geo::MultiLineString(vec![
            line_string![(x: 0.0, y: 0.0), (x: 1.0, y: 0.0)],
            line_string![(x: 1.0, y: 0.0), (x: 2.0, y: 0.0)],
            line_string![(x: 1.0, y: 0.0), (x: 1.0, y: 1.0)],
        ]));
        assert_eq!(line_merge(&branch, 100, 10).unwrap().len(), 3);
        assert!(matches!(
            line_merge(&branch, 100, 2),
            Err(ExtendedAlgorithmError::OutputLimit { .. })
        ));

        let ring = Geometry::MultiLineString(geo::MultiLineString(vec![
            line_string![(x: 0.0, y: 0.0), (x: 1.0, y: 0.0)],
            line_string![(x: 1.0, y: 0.0), (x: 1.0, y: 1.0)],
            line_string![(x: 1.0, y: 1.0), (x: 0.0, y: 1.0)],
            line_string![(x: 0.0, y: 1.0), (x: 0.0, y: 0.0)],
        ]));
        let merged = line_merge(&ring, 100, 10).unwrap();
        assert_eq!(merged.len(), 1);
        assert!(merged[0].is_closed());
    }

    #[test]
    fn split_line_handles_points_crossings_and_overlap_endpoints() {
        let source = line_string![(x: 0.0, y: 0.0), (x: 10.0, y: 0.0)];
        let points = Geometry::MultiPoint(vec![Point::new(2.0, 0.0), Point::new(7.0, 0.0)].into());
        let pieces = split_line(&source, &points, 0.0, 100, 100, 10, 100).unwrap();
        assert_eq!(pieces.len(), 3);
        assert_eq!(
            pieces
                .iter()
                .map(|line| Euclidean.length(line))
                .sum::<f64>(),
            10.0
        );
        assert!(matches!(
            split_line(&source, &points, 0.0, 100, 100, 2, 100),
            Err(ExtendedAlgorithmError::OutputLimit { .. })
        ));

        let cutters = Geometry::MultiLineString(geo::MultiLineString(vec![
            line_string![(x: 5.0, y: -1.0), (x: 5.0, y: 1.0)],
            line_string![(x: 8.0, y: 0.0), (x: 12.0, y: 0.0)],
        ]));
        let pieces = split_line(&source, &cutters, 0.0, 100, 100, 10, 100).unwrap();
        assert_eq!(pieces.len(), 3);
        assert_eq!(pieces[0].0.last().unwrap().x, 5.0);
        assert_eq!(pieces[1].0.last().unwrap().x, 8.0);
        assert!(matches!(
            split_line(&source, &cutters, 0.0, 100, 1, 10, 100),
            Err(ExtendedAlgorithmError::WorkLimit { .. })
        ));
    }

    #[test]
    fn split_line_cuts_every_occurrence_of_a_self_intersection_point() {
        let source = line_string![
            (x: 0.0, y: 0.0), (x: 2.0, y: 2.0),
            (x: 0.0, y: 2.0), (x: 2.0, y: 0.0)
        ];
        let splitter = Geometry::Point(Point::new(1.0, 1.0));
        let pieces = split_line(&source, &splitter, 0.0, 100, 100, 10, 100).unwrap();
        assert_eq!(pieces.len(), 3);
        let is_crossing = |coord: &Coord<f64>| {
            (coord.x - 1.0).abs() <= f64::EPSILON * 8.0
                && (coord.y - 1.0).abs() <= f64::EPSILON * 8.0
        };
        assert!(pieces[0].0.last().is_some_and(is_crossing));
        assert!(pieces[1].0.first().is_some_and(is_crossing));
        assert!(pieces[1].0.last().is_some_and(is_crossing));
    }

    #[test]
    fn split_line_preserves_distinct_sub_picometer_cuts_and_all_bounds() {
        let source = line_string![(x: 0.0, y: 0.0), (x: 1.0, y: 0.0)];
        let splitter =
            Geometry::MultiPoint(vec![Point::new(0.5, 0.0), Point::new(0.5 + 1e-13, 0.0)].into());
        let pieces = split_line(&source, &splitter, 0.0, 100, 100, 10, 100).unwrap();
        assert_eq!(pieces.len(), 3);
        assert!(Euclidean.length(&pieces[1]) > 0.0);
        assert!(matches!(
            split_line(&source, &splitter, 0.0, 3, 100, 10, 100),
            Err(ExtendedAlgorithmError::CoordinateLimit { .. })
        ));
        assert!(matches!(
            split_line(&source, &splitter, 0.0, 100, 100, 10, 5),
            Err(ExtendedAlgorithmError::OutputLimit { .. })
        ));

        let decimal_projection_source = line_string![(x: 0.0, y: 0.0), (x: 100.0, y: 0.0)];
        let decimal_point = Geometry::Point(Point::new(14.0, 0.0));
        assert_eq!(
            split_line(
                &decimal_projection_source,
                &decimal_point,
                0.0,
                100,
                100,
                10,
                100,
            )
            .unwrap()
            .len(),
            2
        );
        let off_line = Geometry::Point(Point::new(14.0, 1e-9));
        assert_eq!(
            split_line(
                &decimal_projection_source,
                &off_line,
                0.0,
                100,
                100,
                10,
                100,
            )
            .unwrap()
            .len(),
            1
        );
        assert_eq!(
            split_line(
                &decimal_projection_source,
                &off_line,
                1e-8,
                100,
                100,
                10,
                100,
            )
            .unwrap()
            .len(),
            2
        );
    }

    #[test]
    fn adversarial_geometry_families_and_numeric_overflow_fail_closed() {
        let polygon = polygon![
            exterior: [
                (x: 0.0, y: 0.0), (x: 4.0, y: 0.0),
                (x: 4.0, y: 4.0), (x: 0.0, y: 4.0),
                (x: 0.0, y: 0.0)
            ],
            interiors: [[
                (x: 1.0, y: 1.0), (x: 2.0, y: 1.0),
                (x: 2.0, y: 2.0), (x: 1.0, y: 2.0),
                (x: 1.0, y: 1.0)
            ]]
        ];
        let multi_polygon = Geometry::MultiPolygon(geo::MultiPolygon(vec![polygon.clone()]));
        let multi_line = Geometry::MultiLineString(geo::MultiLineString(vec![
            line_string![(x: 0.0, y: 0.0), (x: 2.0, y: 0.0)],
            line_string![(x: 0.0, y: 1.0), (x: 2.0, y: 1.0)],
        ]));
        let collection = Geometry::GeometryCollection(
            vec![Geometry::Polygon(polygon.clone()), multi_line.clone()].into(),
        );
        for geometry in [
            Geometry::Point(Point::new(0.0, 0.0)),
            Geometry::MultiPoint(vec![Point::new(0.0, 0.0), Point::new(1.0, 1.0)].into()),
            Geometry::Polygon(polygon.clone()),
            multi_polygon.clone(),
            multi_line.clone(),
            collection,
        ] {
            assert!(densify(&geometry, 0.75, 1_000).is_ok());
        }
        assert!(matches!(
            densify(
                &Geometry::LineString(line_string![
                    (x: -f64::MAX, y: 0.0), (x: f64::MAX, y: 0.0)
                ]),
                1.0,
                u64::MAX,
            ),
            Err(ExtendedAlgorithmError::IndexOverflow)
        ));
        assert!(snap_to_grid(&Geometry::Point(Point::new(f64::MAX, 0.0)), 0.1).is_err());

        let geodesic_multi = Geometry::MultiPolygon(geo::MultiPolygon(vec![polygon]));
        assert!(
            geodesic_area_m2(&geodesic_multi, &crate::geodetica::wgs84_di_prova()).unwrap() > 0.0
        );
        assert!(geodesic_area_m2(
            &Geometry::Point(Point::new(0.0, 0.0)),
            &crate::geodetica::wgs84_di_prova()
        )
        .is_err());
        assert!(
            geometry_diagnostics(&Geometry::GeometryCollection(
                Vec::<Geometry<f64>>::new().into()
            ))
            .unwrap()
            .is_empty
        );

        assert!(line_merge(
            &Geometry::GeometryCollection(Vec::<Geometry<f64>>::new().into()),
            10,
            10
        )
        .unwrap()
        .is_empty());
        assert!(line_merge(&multi_line, 100, 10).is_ok());
        assert!(line_merge(&multi_polygon, 100, 10).is_err());
    }

    #[test]
    fn split_supports_polygon_boundaries_collections_zero_length_and_huge_values() {
        let source = line_string![(x: -1.0, y: 1.0), (x: 5.0, y: 1.0)];
        let polygon = rect(0.0, 0.0, 4.0, 4.0);
        let pieces = split_line(&source, &polygon, 0.0, 100, 1_000, 10, 100).unwrap();
        assert_eq!(pieces.len(), 3);

        let multi_polygon = Geometry::MultiPolygon(geo::MultiPolygon(vec![match polygon {
            Geometry::Polygon(value) => value,
            _ => unreachable!(),
        }]));
        assert_eq!(
            split_line(&source, &multi_polygon, 0.0, 100, 1_000, 10, 100)
                .unwrap()
                .len(),
            3
        );
        let collection = Geometry::GeometryCollection(
            vec![
                Geometry::Point(Point::new(2.0, 1.0)),
                Geometry::LineString(line_string![(x: 3.0, y: 0.0), (x: 3.0, y: 2.0)]),
            ]
            .into(),
        );
        assert_eq!(
            split_line(&source, &collection, 0.0, 100, 1_000, 10, 100)
                .unwrap()
                .len(),
            3
        );

        let zero = LineString::new(Vec::new());
        assert_eq!(
            split_line(
                &zero,
                &Geometry::Point(Point::new(1.0, 1.0)),
                0.0,
                100,
                100,
                10,
                100,
            )
            .unwrap(),
            vec![zero]
        );
        let enormous = line_string![(x: -f64::MAX, y: 0.0), (x: f64::MAX, y: 0.0)];
        assert!(matches!(
            split_line(
                &enormous,
                &Geometry::Point(Point::new(0.0, 0.0)),
                0.0,
                100,
                100,
                10,
                100,
            ),
            Err(ExtendedAlgorithmError::InvalidInput(_))
        ));
    }

    proptest! {
        #[test]
        // Filtro esatto sugli input grezzi del generatore (coordinate
        // campionate, non stime): l'uguaglianza per bit e' la semantica.
        #[allow(clippy::float_cmp)]
        fn densify_never_exceeds_requested_segment_length(
            x1 in -100.0_f64..100.0,
            y1 in -100.0_f64..100.0,
            x2 in -100.0_f64..100.0,
            y2 in -100.0_f64..100.0,
            maximum in 0.5_f64..50.0,
        ) {
            prop_assume!(x1 != x2 || y1 != y2);
            let input = Geometry::LineString(LineString::from(vec![(x1, y1), (x2, y2)]));
            let output = densify(&input, maximum, 1_000).unwrap();
            let Geometry::LineString(output) = output else {
                unreachable!()
            };
            for segment in output.lines() {
                let length = (segment.end.x - segment.start.x)
                    .hypot(segment.end.y - segment.start.y);
                prop_assert!(length <= maximum * (1.0 + 1e-12));
            }
        }

        #[test]
        fn point_grid_snap_is_idempotent(
            x in -1_000_000.0_f64..1_000_000.0,
            y in -1_000_000.0_f64..1_000_000.0,
            grid in 0.01_f64..100.0,
        ) {
            let input = Geometry::Point(Point::new(x, y));
            let once = snap_to_grid(&input, grid).unwrap();
            let twice = snap_to_grid(&once, grid).unwrap();
            prop_assert_eq!(once, twice);
        }

        #[test]
        fn line_merge_conserves_randomly_oriented_chain(
            orientations in proptest::collection::vec(any::<bool>(), 1..128),
        ) {
            let lines = orientations
                .iter()
                .enumerate()
                .map(|(index, reverse)| {
                    // index < 128 (bound del generatore): esatto in f64.
                    #[allow(clippy::cast_precision_loss)]
                    let position = index as f64;
                    let start = Coord { x: position, y: 0.0 };
                    let end = Coord { x: position + 1.0, y: 0.0 };
                    if *reverse {
                        LineString::new(vec![end, start])
                    } else {
                        LineString::new(vec![start, end])
                    }
                })
                .collect();
            let input = Geometry::MultiLineString(geo::MultiLineString(lines));
            let merged = line_merge(&input, 1_000, 2).unwrap();
            prop_assert_eq!(merged.len(), 1);
            // orientations.len() <= 128 (bound del generatore): esatto in f64.
            #[allow(clippy::cast_precision_loss)]
            let expected_length = orientations.len() as f64;
            prop_assert!((Euclidean.length(&merged[0]) - expected_length).abs() < 1e-12);
        }

        #[test]
        fn split_line_conserves_length_for_distinct_integer_cuts(
            cuts in proptest::collection::btree_set(1_u16..999_u16, 0..64),
        ) {
            let source = LineString::from(vec![(0.0, 0.0), (1_000.0, 0.0)]);
            let points = cuts
                .iter()
                .map(|cut| Point::new(f64::from(*cut), 0.0))
                .collect::<Vec<_>>();
            let splitter = Geometry::MultiPoint(points.into());
            let pieces = split_line(&source, &splitter, 0.0, 1_000, 100_000, 100, 1_000).unwrap();
            prop_assert_eq!(pieces.len(), cuts.len() + 1);
            let length: f64 = pieces.iter().map(|piece| Euclidean.length(piece)).sum();
            prop_assert!((length - 1_000.0).abs() < 1e-9);
        }
    }
}

/// Oracolo di [`delaunay`] con il caricamento in blocco di `spade`: la
/// funzione di prima, ricopiata alla lettera, e' il riferimento.
#[cfg(test)]
mod oracolo_delaunay {
    use super::*;
    use geo::{MultiPoint, TriangulateDelaunayUnconstrained};

    /// `delaunay` di prima, alla lettera: `unconstrained_triangulation` di
    /// `geo` 0.33.1, inserimento incrementale.
    fn delaunay_con_geo(
        geometry: &Geometry<f64>,
        max_input_coordinates: u64,
        max_triangles: u64,
    ) -> Result<Vec<Polygon<f64>>, ExtendedAlgorithmError> {
        validate_input(geometry)?;
        let coordinates = coordinate_count(geometry)?;
        if coordinates > max_input_coordinates {
            return Err(ExtendedAlgorithmError::CoordinateLimit {
                actual: coordinates,
                limit: max_input_coordinates,
            });
        }
        let triangles = protetto(|| geometry.unconstrained_triangulation())?
            .map_err(|error| ExtendedAlgorithmError::Triangulation(error.to_string()))?;
        let actual =
            u64::try_from(triangles.len()).map_err(|_| ExtendedAlgorithmError::IndexOverflow)?;
        if actual > max_triangles {
            return Err(ExtendedAlgorithmError::OutputLimit {
                actual,
                limit: max_triangles,
            });
        }
        triangles
            .into_iter()
            .map(|triangle| {
                let polygon = triangle.to_polygon();
                validate_output(Geometry::Polygon(polygon.clone()))?;
                Ok(polygon)
            })
            .collect()
    }

    type Terna = [(u64, u64); 3];

    /// Un triangolo come terna di bit, ruotata (verso invariato) a partire
    /// dal vertice minimo per `total_cmp`: due triangoli con gli stessi
    /// vertici e lo stesso verso hanno la stessa terna.
    fn terna(poligono: &Polygon<f64>) -> Terna {
        let anello = &poligono.exterior().0;
        assert_eq!(anello.len(), 4, "triangolo non chiuso a quattro vertici");
        assert_eq!(anello.first(), anello.last());
        let minore = |a: &Coord<f64>, b: &Coord<f64>| a.x.total_cmp(&b.x).then(a.y.total_cmp(&b.y));
        let inizio = (0..3)
            .min_by(|&i, &j| minore(&anello[i], &anello[j]))
            .unwrap_or(0);
        [0, 1, 2].map(|k| {
            let c = anello[(inizio + k) % 3];
            (c.x.to_bits(), c.y.to_bits())
        })
    }

    fn insieme(triangoli: &[Polygon<f64>]) -> Vec<Terna> {
        let mut terne: Vec<Terna> = triangoli.iter().map(terna).collect();
        terne.sort_unstable();
        terne
    }

    /// Nuova contro vecchia: stesso errore, oppure stesso numero di
    /// triangoli; `true` se anche l'insieme e' lo stesso bit per bit.
    fn confronta(geometria: &Geometry<f64>, massimo: u64) -> bool {
        let nuova = delaunay(geometria, massimo, massimo);
        let vecchia = delaunay_con_geo(geometria, massimo, massimo);
        match (&nuova, &vecchia) {
            (Err(nuova), Err(vecchia)) => {
                assert_eq!(format!("{nuova:?}"), format!("{vecchia:?}"));
                true
            }
            (Ok(nuova), Ok(vecchia)) => {
                assert_eq!(nuova.len(), vecchia.len(), "{geometria:?}");
                insieme(nuova) == insieme(vecchia)
            }
            _ => panic!("esiti diversi: {nuova:?} contro {vecchia:?}"),
        }
    }

    fn multipunto(coppie: &[(f64, f64)]) -> Geometry<f64> {
        Geometry::MultiPoint(MultiPoint::from(coppie.to_vec()))
    }

    #[allow(clippy::cast_possible_truncation)]
    fn intero(valore: f64) -> i128 {
        assert!(valore.fract() == 0.0 && valore.abs() < 1.0e15, "non intero");
        i128::from(valore as i64)
    }

    /// Controllo esatto, in interi, che l'uscita nuova sia una
    /// triangolazione di Delaunay degli stessi punti: triangoli antiorari non
    /// degeneri con vertici d'ingresso, nessun punto d'ingresso
    /// strettamente dentro un cerchio circoscritto, e area totale uguale a
    /// quella dell'uscita di `geo` (che copre l'inviluppo convesso).
    ///
    /// Le coordinate sono interi per `scala` (una potenza di due, quindi la
    /// divisione e' esatta e i segni dei predicati sono quelli degli interi).
    fn delaunay_esatta(coppie: &[(f64, f64)], scala: f64) {
        let geometria = multipunto(coppie);
        let nuova = delaunay(&geometria, u64::MAX, u64::MAX).expect("delaunay");
        let vecchia = delaunay_con_geo(&geometria, u64::MAX, u64::MAX).expect("geo");
        assert_eq!(nuova.len(), vecchia.len());
        let punti: Vec<(i128, i128)> = coppie
            .iter()
            .map(|&(x, y)| (intero(x / scala), intero(y / scala)))
            .collect();
        let vertici = |poligono: &Polygon<f64>| {
            let anello = &poligono.exterior().0;
            [0, 1, 2].map(|k| (intero(anello[k].x / scala), intero(anello[k].y / scala)))
        };
        let doppia_area =
            |[a, b, c]: [(i128, i128); 3]| (b.0 - a.0) * (c.1 - a.1) - (b.1 - a.1) * (c.0 - a.0);
        let mut area_nuova = 0_i128;
        for poligono in &nuova {
            let [a, b, c] = vertici(poligono);
            assert!(doppia_area([a, b, c]) > 0, "triangolo non antiorario");
            area_nuova += doppia_area([a, b, c]);
            for v in [a, b, c] {
                assert!(punti.contains(&v), "vertice non d'ingresso");
            }
            for &d in &punti {
                let riga = |p: (i128, i128)| {
                    let (dx, dy) = (p.0 - d.0, p.1 - d.1);
                    (dx, dy, dx * dx + dy * dy)
                };
                let (r1, r2, r3) = (riga(a), riga(b), riga(c));
                let determinante = r1.0 * (r2.1 * r3.2 - r2.2 * r3.1)
                    - r1.1 * (r2.0 * r3.2 - r2.2 * r3.0)
                    + r1.2 * (r2.0 * r3.1 - r2.1 * r3.0);
                assert!(determinante <= 0, "punto dentro un cerchio circoscritto");
            }
        }
        let area_vecchia: i128 = vecchia.iter().map(|p| doppia_area(vertici(p))).sum();
        assert_eq!(area_nuova, area_vecchia);
    }

    struct Xorshift(u64);

    impl Xorshift {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }

        fn sotto(&mut self, limite: u64) -> u64 {
            self.next() % limite
        }

        #[allow(clippy::cast_precision_loss)]
        fn unitario(&mut self) -> f64 {
            (self.next() >> 11) as f64 / (1_u64 << 53) as f64
        }
    }

    #[allow(clippy::cast_precision_loss)]
    fn reale(valore: u64) -> f64 {
        valore as f64
    }

    // Niente mul_add: i punti di prova devono restare quelli scritti.
    #[allow(clippy::suboptimal_flops)]
    fn griglia(lato: u64, passo: f64, origine: (f64, f64)) -> Vec<(f64, f64)> {
        (0..lato)
            .flat_map(|i| {
                (0..lato).map(move |j| (origine.0 + reale(i) * passo, origine.1 + reale(j) * passo))
            })
            .collect()
    }

    fn cocircolari(raggio: i64, centro: (f64, f64)) -> Vec<(f64, f64)> {
        let mut punti = vec![centro];
        for x in -raggio..=raggio {
            for y in -raggio..=raggio {
                if x * x + y * y == raggio * raggio {
                    #[allow(clippy::cast_precision_loss)]
                    punti.push((centro.0 + x as f64, centro.1 + y as f64));
                }
            }
        }
        punti
    }

    /// Punti casuali continui e UTM al centimetro, anche dentro poligoni e
    /// linee (le coordinate di chiusura sono duplicati): senza quaterne
    /// cocircolari la triangolazione di Delaunay e' unica, e l'insieme dei
    /// triangoli deve essere quello di `geo` bit per bit.
    #[test]
    fn stessi_triangoli_su_ingressi_casuali() {
        let mut generatore = Xorshift(0x9E37_79B9_7F4A_7C15);
        for giro in 0..60_u64 {
            let quanti = 3 + generatore.sotto(500);
            let coppie: Vec<(f64, f64)> = (0..quanti)
                .map(|_| {
                    if giro % 2 == 0 {
                        (generatore.unitario() * 100.0, generatore.unitario() * 100.0)
                    } else {
                        (
                            500_000.0 + reale(generatore.sotto(1_000_000)) / 100.0,
                            4_500_000.0 + reale(generatore.sotto(1_000_000)) / 100.0,
                        )
                    }
                })
                .collect();
            assert!(confronta(&multipunto(&coppie), u64::MAX), "giro {giro}");
            let linea = Geometry::LineString(LineString::from(coppie.clone()));
            assert!(confronta(&linea, u64::MAX), "giro {giro}");
        }
        let poligono = Geometry::Polygon(Polygon::new(
            LineString::from(vec![
                (0.0, 0.0),
                (10.0, 0.3),
                (9.7, 10.0),
                (0.2, 9.1),
                (0.0, 0.0),
            ]),
            vec![LineString::from(vec![
                (2.0, 2.0),
                (3.1, 2.2),
                (2.9, 3.3),
                (2.0, 2.0),
            ])],
        ));
        assert!(confronta(&poligono, u64::MAX));
    }

    /// Griglie, punti interi cocircolari e reticoli con duplicati: la
    /// triangolazione di Delaunay non e' unica. Stesso numero di triangoli
    /// di `geo`, e controllo esatto in interi che sia di Delaunay.
    #[test]
    fn triangolazione_di_delaunay_valida_su_ingressi_degeneri() {
        for lato in [2_u64, 3, 4, 5, 8, 13, 20] {
            delaunay_esatta(&griglia(lato, 1.0, (0.0, 0.0)), 1.0);
            delaunay_esatta(&griglia(lato, 1.0, (500_000.0, 4_500_000.0)), 1.0);
            delaunay_esatta(&griglia(lato, 3.0, (-7.0, 11.0)), 1.0);
        }
        for raggio in [5, 25, 65, 325] {
            delaunay_esatta(&cocircolari(raggio, (0.0, 0.0)), 1.0);
            let mut senza_centro = cocircolari(raggio, (500_000.0, 4_500_000.0));
            senza_centro.remove(0);
            delaunay_esatta(&senza_centro, 1.0);
        }
        let mut generatore = Xorshift(0xD1B5_4A32_D192_ED03);
        for _ in 0..40 {
            let quanti = 3 + generatore.sotto(300);
            let lato = 2 + generatore.sotto(25);
            let coppie: Vec<(f64, f64)> = (0..quanti)
                .map(|_| (reale(generatore.sotto(lato)), reale(generatore.sotto(lato))))
                .collect();
            if delaunay(&multipunto(&coppie), u64::MAX, u64::MAX).is_ok_and(|t| !t.is_empty()) {
                delaunay_esatta(&coppie, 1.0);
            } else {
                confronta(&multipunto(&coppie), u64::MAX);
            }
        }
    }

    /// Gli stessi ingressi degeneri agli estremi del dominio di `spade`
    /// (`2^-142` e `2^190` per interi fino a `2^10`): i predicati di
    /// `robust` 1.2.0 lavorano sugli esponenti estremi, e il controllo in
    /// interi, esatto, dice se la triangolazione resta di Delaunay. Qui
    /// `geo` non e' un riferimento indipendente (usa gli stessi predicati).
    #[test]
    fn triangolazione_di_delaunay_valida_agli_estremi_del_dominio() {
        let mut generatore = Xorshift(0x0331_2026_0929_0001);
        for esponente in [-142, -100, 0, 150, 190] {
            let scala = 2.0_f64.powi(esponente);
            let scala_punti = |punti: Vec<(f64, f64)>| -> Vec<(f64, f64)> {
                punti
                    .into_iter()
                    .map(|(x, y)| (x * scala, y * scala))
                    .collect()
            };
            delaunay_esatta(&scala_punti(griglia(9, 1.0, (0.0, 0.0))), scala);
            delaunay_esatta(&scala_punti(griglia(6, 1.0, (1.0, 3.0))), scala);
            delaunay_esatta(&scala_punti(cocircolari(65, (100.0, 100.0))), scala);
            for _ in 0..10 {
                let quanti = 3 + generatore.sotto(200);
                let lato = 2 + generatore.sotto(1_000);
                let coppie: Vec<(f64, f64)> = (0..quanti)
                    .map(|_| (reale(generatore.sotto(lato)), reale(generatore.sotto(lato))))
                    .collect();
                if delaunay(
                    &multipunto(&scala_punti(coppie.clone())),
                    u64::MAX,
                    u64::MAX,
                )
                .is_ok_and(|t| !t.is_empty())
                {
                    delaunay_esatta(&scala_punti(coppie), scala);
                }
            }
        }
    }

    /// Pochi punti, collineari, duplicati (anche `-0.0` contro `0.0`, prima
    /// e dopo che i punti smettano di essere collineari), limiti: stessi
    /// errori e stessi triangoli, bit compresi.
    #[test]
    fn stessi_esiti_su_pochi_punti_collineari_e_duplicati() {
        let casi: &[&[(f64, f64)]] = &[
            &[],
            &[(1.0, 2.0)],
            &[(0.0, 0.0), (1.0, 0.0)],
            &[(0.0, 0.0), (1.0, 0.0), (0.0, 1.0)],
            &[(0.0, 0.0), (1.0, 1.0), (2.0, 2.0), (3.0, 3.0)],
            &[(0.0, 0.0), (1.0, 0.0), (2.0, 0.0), (1.0, 0.0), (-0.0, 0.0)],
            &[(0.0, 0.0), (-0.0, 0.0), (1.0, 0.0), (0.0, 1.0)],
            &[(0.0, 0.0), (1.0, 0.0), (-0.0, -0.0), (0.0, 1.0)],
            &[(0.0, 0.0), (1.0, 0.0), (2.0, 0.0), (-0.0, 0.0), (1.0, 1.0)],
            &[
                (0.0, -0.0),
                (0.0, 0.0),
                (0.0, 3.0),
                (0.0, -0.0),
                (-2.0, 1.0),
            ],
            &[
                (0.0, 0.0),
                (1.0, 0.0),
                (-0.0, -0.0),
                (0.0, 1.0),
                (0.0, -0.0),
            ],
            &[
                (-0.0, 0.0),
                (2.0, 0.0),
                (0.0, 2.0),
                (0.0, -0.0),
                (-0.0, -0.0),
            ],
            &[(1.0, -0.0), (1.0, 0.0), (0.0, 1.0), (3.0, 3.0), (1.0, -0.0)],
            &[
                (0.0, 0.0),
                (0.0, 0.0),
                (1.0, 0.0),
                (1.0, 0.0),
                (0.5, 1.0),
                (0.5, 1.0),
            ],
        ];
        for coppie in casi {
            assert!(confronta(&multipunto(coppie), u64::MAX), "{coppie:?}");
        }
        let quadrato = multipunto(&[(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0), (0.3, 0.6)]);
        for massimo in [0, 3, 4, 5] {
            confronta(&quadrato, massimo);
        }
        assert!(matches!(
            delaunay(&quadrato, 5, 3),
            Err(ExtendedAlgorithmError::OutputLimit {
                actual: 4,
                limit: 3
            })
        ));
    }

    /// Ai bordi del dominio di `spade` (`[2^-142, 2^201]` in modulo, o zero)
    /// e fuori: dentro, stessi triangoli; fuori, lo stesso errore di `geo`
    /// per il primo punto fuori in ordine d'ingresso.
    #[test]
    fn stessi_esiti_alle_scale_estreme() {
        let minimo = 2.0_f64.powi(-142);
        let massimo = 2.0_f64.powi(201);
        let mut generatore = Xorshift(0x2545_F491_4F6C_DD1D);
        for (scala, origine) in [
            (minimo * 1024.0, 0.0),
            (2.0_f64.powi(190), 0.0),
            (2.0_f64.powi(190), -massimo / 2.0),
            (1.0e-3, 1.0e15),
        ] {
            let coppie: Vec<(f64, f64)> = (0..200)
                .map(|_| {
                    (
                        origine + (reale(generatore.sotto(1 << 10)) * scala),
                        reale(generatore.sotto(1 << 10)) * scala,
                    )
                })
                .collect();
            assert!(confronta(&multipunto(&coppie), u64::MAX));
        }
        let fuori: &[&[(f64, f64)]] = &[
            &[(0.0, 0.0), (1.0e-300, 0.0), (0.0, 1.0)],
            &[(0.0, 0.0), (1.0, 0.0), (0.0, minimo / 2.0)],
            &[(0.0, 0.0), (massimo * 2.0, 0.0), (0.0, 1.0)],
            &[(0.0, 0.0), (1.0, 1.0e61), (1.0e-300, 1.0)],
            &[(1.0e-300, 1.0e61), (0.0, 0.0), (1.0, 1.0)],
            &[(f64::from_bits(1), 0.0), (1.0, 0.0), (0.0, 1.0)],
        ];
        for coppie in fuori {
            assert!(confronta(&multipunto(coppie), u64::MAX));
            assert!(matches!(
                delaunay(&multipunto(coppie), u64::MAX, u64::MAX),
                Err(ExtendedAlgorithmError::Triangulation(_))
            ));
        }
    }

    /// Il contratto d'uscita: ogni triangolo e' un anello chiuso antiorario
    /// che parte dal suo vertice comparso per primo, i triangoli sono in
    /// ordine di prima comparsa dei vertici, e due chiamate danno gli stessi
    /// bit, anche sugli ingressi dove la triangolazione non e' unica.
    #[test]
    // Il rango e' l'uguaglianza di `spade` (`==`), e l'orientazione e' un
    // controllo di segno scritto come nel testo.
    #[allow(clippy::float_cmp, clippy::suboptimal_flops)]
    fn ordine_canonico_e_deterministico() {
        let mut griglia_e_casuali = griglia(12, 1.0, (0.0, 0.0));
        let mut generatore = Xorshift(0x5DEE_CE66_D1CE_4E5B);
        griglia_e_casuali
            .extend((0..200).map(|_| (generatore.unitario() * 11.0, generatore.unitario() * 11.0)));
        griglia_e_casuali.reverse();
        let geometria = multipunto(&griglia_e_casuali);
        let triangoli = delaunay(&geometria, u64::MAX, u64::MAX).expect("delaunay");
        let rango = |c: &Coord<f64>| {
            griglia_e_casuali
                .iter()
                .position(|&(x, y)| x == c.x && y == c.y)
                .expect("vertice d'ingresso")
        };
        let ranghi: Vec<[usize; 3]> = triangoli
            .iter()
            .map(|poligono| {
                let anello = &poligono.exterior().0;
                assert_eq!(anello.len(), 4);
                assert_eq!(anello[0], anello[3]);
                let [a, b, c] = [0, 1, 2].map(|k| anello[k]);
                let orientazione = (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x);
                assert!(orientazione > 0.0, "triangolo non antiorario");
                let ranghi = [rango(&a), rango(&b), rango(&c)];
                assert!(ranghi[0] < ranghi[1] && ranghi[0] < ranghi[2]);
                ranghi
            })
            .collect();
        assert!(ranghi.windows(2).all(|coppia| coppia[0] < coppia[1]));
        for _ in 0..3 {
            assert_eq!(
                format!(
                    "{:?}",
                    delaunay(&geometria, u64::MAX, u64::MAX).expect("delaunay")
                ),
                format!("{triangoli:?}")
            );
        }
    }
}
