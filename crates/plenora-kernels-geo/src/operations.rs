//! Kernel geometrici puri su `geo::Geometry<f64>`, indipendenti dagli
//! adapter Arrow: misure, trasformazioni 1:1, `explode`, `boundary`.
//!
//! Ogni kernel valida l'ingresso con la validazione OGC dietro la barriera
//! dei panici, e le trasformazioni validano anche l'uscita: una geometria
//! sbagliata e' un errore, mai un risultato. I null e la traduzione di
//! [`OperationError`] in `PlenoraError` non si definiscono qui: li decide il
//! runner (`plenora-pipeline`), che chiama questi kernel sulle righe.

use crate::rust_backend::buffer::{buffer_controllato, ErroreBuffer, Estremita};
use crate::rust_backend::griglia;
use crate::rust_backend::precision::Precision;
use crate::ValidazioneProtetta as _;
use geo::algorithm::line_measures::{Distance, Euclidean, Length};
use geo::{
    Area, BoundingRect, Coord, CoordsIter, Geometry, InteriorPoint, LineString, MapCoords,
    MultiLineString, MultiPoint, Simplify, SimplifyVwPreserve,
};
use std::collections::BTreeMap;
use thiserror::Error;
use wkt::ToWkt;

mod rdp;

/// Le estremita' delle linee nel buffer ([`buffer_with_cap`]); le giunzioni
/// sono sempre tonde.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BufferCapStyle {
    /// Arco attorno all'estremo, con il passo angolare dalla precisione.
    Round,
    /// Taglio netto all'estremo: nessun buffer oltre la linea, nessun
    /// buffer per i punti.
    Flat,
    /// Quadrato che sporge di `|distance|` oltre l'estremo.
    Square,
}

/// L'algoritmo di [`simplify_with_policy`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SimplifyPolicy {
    /// Ramer-Douglas-Peucker di `geo`: `tolerance` e' una distanza; nessuna
    /// garanzia di topologia (un risultato non valido e' un errore).
    DouglasPeucker,
    /// Visvalingam-Whyatt con conservazione della topologia di `geo`
    /// (`simplify_vw_preserve`): la soglia e' un'**area** (unita' del CRS al
    /// quadrato; `min_area` nella config di `geo.simplify`), quella del
    /// triangolo di ogni vertice con i due vicini: il vertice si toglie se
    /// l'area non la supera. Non e' la distanza di `GEOSTopologyPreserveSimplify`.
    PreserveTopology,
}

/// Gli errori dei kernel di questo modulo. Nessun messaggio porta
/// coordinate o valori delle geometrie.
#[derive(Debug, Error)]
pub enum OperationError {
    /// Un parametro fuori dominio (nome e ragione statici).
    #[error("parametro {name} non valido: {reason}")]
    InvalidParameter {
        /// Il nome del parametro.
        name: &'static str,
        /// Perche' non e' valido.
        reason: &'static str,
    },
    /// La geometria prodotta non supera la validazione OGC; porta la
    /// ragione classificata, mai il testo di `geo`.
    #[error("geometria prodotta non valida: {0}")]
    InvalidOutput(String),
    /// La geometria d'ingresso non supera la validazione OGC; porta la
    /// ragione classificata, mai il testo di `geo`.
    #[error("geometria di input non valida: {0}")]
    InvalidInput(String),
    /// Il writer WKT rifiuta la geometria; il testo e' fisso.
    #[error("serializzazione WKT fallita: {0}")]
    WktSerialization(String),
    /// Invariante interna violata: errore propagato, mai panico.
    #[error("internal error: {0}")]
    Internal(&'static str),
    /// La validazione OGC non ha concluso: `geo` si e' interrotta.
    ///
    /// **Non** e' una geometria invalida. Nessuno ha dimostrato che l'ingresso
    /// sia sbagliato, e accusarlo manderebbe chi legge a correggere un errore
    /// che non ha commesso. Porta la *forma* del payload, mai il contenuto.
    #[error("validazione OGC non conclusa: {0} (contenuto non pubblicato)")]
    ValidazioneNonConclusa(&'static str),
    /// Un calcolo di `geo` non ha concluso su una geometria **valida**: si e'
    /// interrotto dietro la barriera (`crate::calcolo_protetto`), per esempio
    /// dentro `relate` o, nel buffer, in `i_overlay`.
    ///
    /// Come [`Self::ValidazioneNonConclusa`], non accusa l'ingresso: la
    /// geometria ha superato la validazione, ed e' la dipendenza a non
    /// reggere. Porta la *forma* del payload, mai il contenuto.
    #[error("calcolo geometrico non concluso: {0} (contenuto non pubblicato)")]
    CalcoloNonConcluso(&'static str),
    /// Lo spostamento che la griglia di `i_overlay` introdurrebbe nel buffer
    /// supera la precisione dichiarata (vedi [`buffer_with_cap`]). Nessun
    /// dato nel messaggio.
    #[error("geometria troppo estesa per la precisione dichiarata")]
    PrecisionInsufficient,
}

impl From<griglia::PrecisioneInsufficiente> for OperationError {
    fn from(_: griglia::PrecisioneInsufficiente) -> Self {
        Self::PrecisionInsufficient
    }
}

fn ensure_valid(geometry: &Geometry<f64>) -> Result<(), OperationError> {
    geometry.validazione_protetta().map_err(|esito| {
        esito.separa(
            |ragione| OperationError::InvalidInput(ragione.to_string()),
            OperationError::ValidazioneNonConclusa,
        )
    })
}

fn validate_output(geometry: Geometry<f64>) -> Result<Geometry<f64>, OperationError> {
    geometry.validazione_protetta().map_err(|esito| {
        esito.separa(
            |ragione| OperationError::InvalidOutput(ragione.to_string()),
            OperationError::ValidazioneNonConclusa,
        )
    })?;
    Ok(geometry)
}

/// Area planare senza segno, nelle unita' delle coordinate al quadrato
/// (kernel di `geo.area`).
///
/// Il CRS e' affare del chiamante: coordinate geografiche vanno proiettate
/// prima. Un poligono conta l'esterno meno i buchi; una collezione somma le
/// aree dei membri senza unirli.
///
/// # Errors
///
/// - `InvalidInput`: la geometria di input non supera la validazione OGC;
/// - `ValidazioneNonConclusa`, `CalcoloNonConcluso`: la validazione OGC o il
///   calcolo di `geo` vanno in panico dentro la barriera.
pub fn area(geometry: &Geometry<f64>) -> Result<f64, OperationError> {
    ensure_valid(geometry)?;
    protetto(|| geometry.unsigned_area())
}

/// Un calcolo di `geo` dietro la barriera dei panici
/// ([`crate::calcolo_protetto`]): un panico e' `CalcoloNonConcluso`, con la
/// sola forma del payload.
fn protetto<T>(calcolo: impl FnOnce() -> T) -> Result<T, OperationError> {
    crate::calcolo_protetto(calcolo).map_err(OperationError::CalcoloNonConcluso)
}

/// Lunghezza planare (kernel di `geo.length`), con la semantica di Shapely
/// per i poligoni: la lunghezza di un poligono e' la somma degli anelli,
/// esterno e buchi. Punti 0, collezioni la somma dei membri.
///
/// # Errors
///
/// - `InvalidInput`: la geometria di input non supera la validazione OGC;
/// - `ValidazioneNonConclusa`, `CalcoloNonConcluso`: la validazione OGC o il
///   calcolo di `geo` vanno in panico dentro la barriera.
pub fn length(geometry: &Geometry<f64>) -> Result<f64, OperationError> {
    ensure_valid(geometry)?;
    protetto(|| length_unchecked(geometry))
}

fn length_unchecked(geometry: &Geometry<f64>) -> f64 {
    match geometry {
        Geometry::Point(_) | Geometry::MultiPoint(_) => 0.0,
        Geometry::Line(line) => Euclidean.length(line),
        Geometry::LineString(line) => Euclidean.length(line),
        Geometry::MultiLineString(lines) => Euclidean.length(lines),
        Geometry::Polygon(polygon) => {
            Euclidean.length(polygon.exterior())
                + polygon
                    .interiors()
                    .iter()
                    .map(|ring| Euclidean.length(ring))
                    .sum::<f64>()
        }
        Geometry::MultiPolygon(polygons) => polygons
            .iter()
            .map(|polygon| length_unchecked(&Geometry::Polygon(polygon.clone())))
            .sum(),
        Geometry::GeometryCollection(collection) => collection.iter().map(length_unchecked).sum(),
        Geometry::Rect(rect) => length_unchecked(&Geometry::Polygon(rect.to_polygon())),
        Geometry::Triangle(triangle) => length_unchecked(&Geometry::Polygon(triangle.to_polygon())),
    }
}

/// Perimetro (kernel di `geo.perimeter`): la stessa semantica di
/// [`length`], come Manipola lo definisce con `GeoSeries.length`; una
/// linea vale quindi la sua lunghezza, non 0.
///
/// # Errors
///
/// Come [`length`].
pub fn perimeter(geometry: &Geometry<f64>) -> Result<f64, OperationError> {
    length(geometry)
}

/// Distanza euclidea planare minima fra due geometrie (kernel di
/// `geo.distance`), 0 se si intersecano; `None` se una delle due non ha
/// coordinate (geometria vuota).
///
/// # Errors
///
/// - `InvalidInput`: una delle due geometrie non supera la validazione OGC;
/// - `ValidazioneNonConclusa`, `CalcoloNonConcluso`: la validazione OGC o il
///   calcolo di `geo` vanno in panico dentro la barriera.
pub fn distance(
    left: &Geometry<f64>,
    right: &Geometry<f64>,
) -> Result<Option<f64>, OperationError> {
    ensure_valid(left)?;
    ensure_valid(right)?;
    if left.coords_count() == 0 || right.coords_count() == 0 {
        return Ok(None);
    }
    protetto(|| Euclidean.distance(left, right)).map(Some)
}

/// Rettangolo d'ingombro come `[min_x, min_y, max_x, max_y]` (kernel di
/// `geo.bounds_extractor`); `None` per geometrie senza coordinate. Valori
/// copiati dalle coordinate, esatti.
///
/// # Errors
///
/// - `InvalidInput`: la geometria di input non supera la validazione OGC;
/// - `ValidazioneNonConclusa`: la validazione OGC va in panico dentro la
///   barriera.
pub fn bounds(geometry: &Geometry<f64>) -> Result<Option<[f64; 4]>, OperationError> {
    ensure_valid(geometry)?;
    Ok(geometry.bounding_rect().map(|rect| {
        let min = rect.min();
        let max = rect.max();
        [min.x, min.y, max.x, max.y]
    }))
}

/// Numero di coordinate della geometria, su tutte le parti (kernel di
/// `geo.vertex_count`): ogni anello conta anche il vertice di chiusura.
///
/// # Errors
///
/// - `InvalidInput`: la geometria di input non supera la validazione OGC;
/// - `ValidazioneNonConclusa`: la validazione OGC va in panico dentro la
///   barriera.
/// - `Internal`: invariante interna violata (`usize` non rappresentabile in
///   `u64`; mai sui target supportati).
pub fn vertex_count(geometry: &Geometry<f64>) -> Result<u64, OperationError> {
    ensure_valid(geometry)?;
    u64::try_from(geometry.coords_count())
        .map_err(|_| OperationError::Internal("usize always fits in u64 on supported targets"))
}

/// Zero con segno normalizzato: `-0.0` diventa `+0.0`, ogni altro valore
/// resta bit-identico (NaN e infiniti inclusi).
///
/// IEEE 754 ha due codifiche dello stesso valore zero e `-0.0 == 0.0` e' vero,
/// quindi la normalizzazione non cambia alcuna semantica numerica.
fn normalize_signed_zero(value: f64) -> f64 {
    // Confronto float voluto: e' esattamente la definizione di "questo valore
    // e' zero", e per lo zero il confronto IEEE e' esatto per costruzione.
    #[allow(clippy::float_cmp)]
    if value == 0.0 {
        0.0
    } else {
        value
    }
}

/// `true` se una qualsiasi coordinata porta uno zero negativo.
///
/// Serve a evitare la copia della geometria nel caso normale: la scansione non
/// alloca.
fn has_negative_zero(geometry: &Geometry<f64>) -> bool {
    geometry
        .coords_iter()
        .any(|coord| is_negative_zero(coord.x) || is_negative_zero(coord.y))
}

fn is_negative_zero(value: f64) -> bool {
    // `is_sign_negative` da solo e' vero anche per i negativi ordinari: serve
    // la congiunzione con "vale zero".
    #[allow(clippy::float_cmp)]
    let zero = value == 0.0;
    zero && value.is_sign_negative()
}

/// Un punto che appartiene alla geometria (kernel di
/// `geo.point_on_surface`).
///
/// E' `interior_point` di `geo`: per i poligoni il punto medio del tratto
/// interno piu' lungo di una linea di scansione orizzontale, per linee e
/// punti un vertice. `None` per geometrie senza coordinate.
///
/// # Errors
///
/// - `InvalidInput`: la geometria di input non supera la validazione OGC;
/// - `ValidazioneNonConclusa`, `CalcoloNonConcluso`: la validazione OGC o il
///   calcolo di `geo` vanno in panico dentro la barriera.
pub fn point_on_surface(geometry: &Geometry<f64>) -> Result<Option<Geometry<f64>>, OperationError> {
    ensure_valid(geometry)?;

    // Con `-0.0` e `0.0` presenti insieme la sweep line di `interior_point`
    // (`geo` 0.33.1, `algorithm/sweep/mod.rs:169`) viola la propria
    // invariante sugli intervalli: e' un `debug_assert!`, quindi in release
    // il punto restituito e' sbagliato in silenzio. Il poligono e' valido e
    // `check_validation()` non intercetta il caso: lo zero si normalizza su
    // una copia di lavoro, e la geometria del chiamante resta intatta.
    // Copertura: il test `point_on_surface_survives_negative_zero_coordinates`.
    //
    // `interior_point` passa poi da `relate`, che puo' andare in panico anche
    // su una geometria valida: gira dentro [`crate::calcolo_protetto`].
    let punto = if has_negative_zero(geometry) {
        let normalized = geometry.map_coords(|coord| Coord {
            x: normalize_signed_zero(coord.x),
            y: normalize_signed_zero(coord.y),
        });
        crate::calcolo_protetto(|| normalized.interior_point())
    } else {
        crate::calcolo_protetto(|| geometry.interior_point())
    }
    .map_err(OperationError::CalcoloNonConcluso)?;
    Ok(punto.map(Geometry::Point))
}

/// Serializzazione WKT della geometria (kernel di `geo.to_wkt`): ogni
/// coordinata nella forma decimale piu' breve che rilegge lo stesso `f64`.
///
/// Usa l'API fallibile `try_wkt_string`: `wkt_string()` puo' panicare su un
/// anello interno orfano (poligono con interni ma senza esterno), che
/// `try_wkt_string()` rifiuta con un errore. Il testo reso e' fisso: il
/// payload di `wkt` non attraversa il confine.
///
/// # Errors
///
/// - `InvalidInput`: la geometria di input non supera la validazione OGC;
/// - `ValidazioneNonConclusa`: la validazione OGC va in panico dentro la
///   barriera.
/// - `WktSerialization`: l'encoder ha rifiutato la geometria (es. anello
///   interno orfano) — testo statico, nessun dettaglio della dipendenza.
pub fn to_wkt(geometry: &Geometry<f64>) -> Result<String, OperationError> {
    ensure_valid(geometry)?;
    geometry
        .try_wkt_string()
        .map_err(|_| OperationError::WktSerialization("geometria non serializzabile".to_string()))
}

/// Buffer planare della geometria con estremita' arrotondate
/// (`BufferCapStyle::Round`), il default di `geo.buffer`.
///
/// # Errors
///
/// Come [`buffer_with_cap`].
pub fn buffer(
    geometry: &Geometry<f64>,
    distance: f64,
    precision: Precision,
) -> Result<Geometry<f64>, OperationError> {
    buffer_with_cap(geometry, distance, BufferCapStyle::Round, precision)
}

/// Buffer planare della geometria con lo stile di estremita' richiesto
/// (kernel di `geo.buffer`); l'uscita e' sempre un `MultiPolygon`, anche
/// vuoto.
///
/// `precision` e' la precisione dichiarata nelle unita' delle coordinate
/// (`Precision::from_crs` con un CRS, altrimenti esplicita). Il buffer e'
/// quello di `geo::Buffer` con giunzioni tonde, gli archi scelti dalla
/// precisione (freccia `max(p / 2, 0.001 |d|)`, deviazione dichiarata oltre
/// 5 m con 1 cm) e le componenti sotto la griglia bufferizzate come punti o
/// come anelli, con la griglia controllata a priori
/// (`rust_backend::buffer`, nessun controllo a posteriori; README,
/// «Precisione delle operazioni geografiche: 1 cm a terra»). Distanza
/// nulla: l'unione delle parti areali; negativa: l'erosione delle sole
/// parti areali (punti e linee spariscono); estremita' piatte su soli punti:
/// vuoto.
///
/// # Errors
///
/// - `InvalidInput`: la geometria di input non supera la validazione OGC;
/// - `InvalidParameter`: `distance` non e' finita (NaN o infinita);
/// - `PrecisionInsufficient`: la griglia, piu' il rientro delle direzioni
///   intere di `i_float`, supererebbe meta' della precisione;
/// - `InvalidOutput`: la geometria prodotta non supera la validazione OGC;
/// - `ValidazioneNonConclusa`, `CalcoloNonConcluso`: la validazione OGC o il
///   calcolo di `geo` vanno in panico dentro la barriera.
pub fn buffer_with_cap(
    geometry: &Geometry<f64>,
    distance: f64,
    cap_style: BufferCapStyle,
    precision: Precision,
) -> Result<Geometry<f64>, OperationError> {
    ensure_valid(geometry)?;
    if !distance.is_finite() {
        return Err(OperationError::InvalidParameter {
            name: "distance",
            reason: "deve essere finita",
        });
    }
    let estremita = match cap_style {
        BufferCapStyle::Round => Estremita::Tonde,
        BufferCapStyle::Flat => Estremita::Piatte,
        BufferCapStyle::Square => Estremita::Quadrate,
    };
    let result = buffer_controllato(geometry, distance, estremita, precision).map_err(
        |errore| match errore {
            ErroreBuffer::PrecisioneInsufficiente => OperationError::PrecisionInsufficient,
            ErroreBuffer::CalcoloNonConcluso(forma) => OperationError::CalcoloNonConcluso(forma),
        },
    )?;
    validate_output(Geometry::MultiPolygon(result))
}

/// Semplificazione della geometria con Douglas-Peucker
/// (`SimplifyPolicy::DouglasPeucker`), il default di `geo.simplify`.
///
/// # Errors
///
/// Come [`simplify_with_policy`].
pub fn simplify(geometry: &Geometry<f64>, tolerance: f64) -> Result<Geometry<f64>, OperationError> {
    simplify_with_policy(geometry, tolerance, SimplifyPolicy::DouglasPeucker)
}

/// Semplificazione della geometria con la politica richiesta (kernel di
/// `geo.simplify`).
///
/// `tolerance` e' la soglia dell'algoritmo: una distanza con
/// [`SimplifyPolicy::DouglasPeucker`], un'area (unita' al quadrato) con
/// [`SimplifyPolicy::PreserveTopology`].
///
/// Linee, anelli e loro multi si semplificano; punti e
/// multi-punti restano invariati; una collezione si semplifica membro per
/// membro. Con `tolerance` 0 la geometria non cambia. I vertici d'uscita
/// sono vertici d'ingresso.
///
/// Le coordinate vicine ai limiti di `f64` (modulo oltre `1e150`, o non
/// nullo e sotto `1e-150`) sono elaborate in uno spazio scalato
/// uniformemente e riportate alle unita' originali, per evitare overflow e
/// underflow nei kernel a distanza quadratica. La soglia si scala con le
/// coordinate: una distanza per il fattore, un'area per il suo quadrato.
///
/// # Errors
///
/// - `InvalidInput`: la geometria di input non supera la validazione OGC;
/// - `InvalidParameter`: `tolerance` non e' finita oppure e' negativa, o
///   (area, coordinate scalate) positiva ma non rappresentabile nello spazio
///   scalato;
/// - `InvalidOutput`: la geometria semplificata non supera la validazione
///   OGC;
/// - `Internal`: una distanza del percorso Douglas-Peucker non e'
///   rappresentabile; nessun risultato parziale viene restituito;
/// - `ValidazioneNonConclusa`, `CalcoloNonConcluso`: la validazione OGC o il
///   calcolo di `geo` vanno in panico dentro la barriera.
pub fn simplify_with_policy(
    geometry: &Geometry<f64>,
    tolerance: f64,
    policy: SimplifyPolicy,
) -> Result<Geometry<f64>, OperationError> {
    ensure_valid(geometry)?;
    if !tolerance.is_finite() || tolerance < 0.0 {
        return Err(OperationError::InvalidParameter {
            name: "tolerance",
            reason: "deve essere finita e non negativa",
        });
    }
    if let Geometry::GeometryCollection(values) = geometry {
        return validate_output(Geometry::GeometryCollection(
            values
                .iter()
                .map(|value| simplify_with_policy(value, tolerance, policy))
                .collect::<Result<Vec<_>, _>>()?
                .into(),
        ));
    }

    // I kernel a distanza quadratica possono traboccare (o perdere cifre)
    // con coordinate finite vicine ai limiti di `f64`: si lavora in uno
    // spazio scalato uniformemente e si torna alle unita' originali dopo.
    let scale = geometry.coords_iter().fold(0.0_f64, |maximum, coordinate| {
        maximum.max(coordinate.x.abs()).max(coordinate.y.abs())
    });
    let normalize = scale > 1e150 || (scale > 0.0 && scale < 1e-150);
    let working = if normalize {
        geometry.map_coords(|coordinate| Coord {
            x: coordinate.x / scale,
            y: coordinate.y / scale,
        })
    } else {
        geometry.clone()
    };
    let working_tolerance = if normalize {
        // Una distanza si scala con il fattore, un'area con il suo quadrato
        // (in due divisioni: `scale * scale` traboccherebbe).
        let value = match policy {
            SimplifyPolicy::DouglasPeucker => tolerance / scale,
            SimplifyPolicy::PreserveTopology => tolerance / scale / scale,
        };
        if value.is_finite() {
            if tolerance > 0.0 && !value.is_normal() {
                // Una soglia positiva che nello spazio scalato diventa zero,
                // o subnormale (cifre perse), cambierebbe in silenzio quali
                // vertici cadono: meglio dirlo che semplificare diversamente.
                return Err(OperationError::InvalidParameter {
                    name: "tolerance",
                    reason: "non rappresentabile nella scala delle coordinate",
                });
            }
            value
        } else {
            f64::MAX
        }
    } else {
        tolerance
    };

    if policy == SimplifyPolicy::DouglasPeucker {
        protetto(|| rdp::accerta_distanze(&working, working_tolerance))??;
    }
    let simplified = protetto(|| match (&working, policy) {
        (Geometry::LineString(value), SimplifyPolicy::DouglasPeucker) => {
            Geometry::LineString(value.simplify(working_tolerance))
        }
        (Geometry::MultiLineString(value), SimplifyPolicy::DouglasPeucker) => {
            Geometry::MultiLineString(value.simplify(working_tolerance))
        }
        (Geometry::Polygon(value), SimplifyPolicy::DouglasPeucker) => {
            Geometry::Polygon(value.simplify(working_tolerance))
        }
        (Geometry::MultiPolygon(value), SimplifyPolicy::DouglasPeucker) => {
            Geometry::MultiPolygon(value.simplify(working_tolerance))
        }
        (Geometry::LineString(value), SimplifyPolicy::PreserveTopology) => {
            Geometry::LineString(value.simplify_vw_preserve(working_tolerance))
        }
        (Geometry::MultiLineString(value), SimplifyPolicy::PreserveTopology) => {
            Geometry::MultiLineString(value.simplify_vw_preserve(working_tolerance))
        }
        (Geometry::Polygon(value), SimplifyPolicy::PreserveTopology) => {
            Geometry::Polygon(value.simplify_vw_preserve(working_tolerance))
        }
        (Geometry::MultiPolygon(value), SimplifyPolicy::PreserveTopology) => {
            Geometry::MultiPolygon(value.simplify_vw_preserve(working_tolerance))
        }
        (value, _) => value.clone(),
    })?;
    let simplified = if normalize {
        simplified.map_coords(|coordinate| Coord {
            x: coordinate.x * scale,
            y: coordinate.y * scale,
        })
    } else {
        simplified
    };
    validate_output(simplified)
}

/// Spezza un livello di multi-parte o di collezione, nell'ordine delle parti
/// (kernel di `geo.explode`).
///
/// Una geometria semplice da' esattamente se
/// stessa; un multi o una collezione vuoti nessuna parte; una collezione
/// annidata resta una parte.
///
/// # Errors
///
/// - `InvalidInput`: la geometria di input non supera la validazione OGC;
/// - `ValidazioneNonConclusa`: la validazione OGC va in panico dentro la
///   barriera.
pub fn explode(geometry: &Geometry<f64>) -> Result<Vec<Geometry<f64>>, OperationError> {
    ensure_valid(geometry)?;
    Ok(match geometry {
        Geometry::MultiPoint(values) => values.iter().copied().map(Geometry::Point).collect(),
        Geometry::MultiLineString(values) => {
            values.iter().cloned().map(Geometry::LineString).collect()
        }
        Geometry::MultiPolygon(values) => values.iter().cloned().map(Geometry::Polygon).collect(),
        Geometry::GeometryCollection(values) => values.iter().cloned().collect(),
        value => vec![value.clone()],
    })
}

/// Confine OGC della geometria (kernel di `geo.boundary`).
///
/// Anelli dei poligoni come `MultiLineString`, estremi delle linee aperte
/// come
/// `MultiPoint` (regola mod-2 per le `MultiLineString`), collezione vuota per
/// i punti, collezione dei confini per una `GeometryCollection`.
///
/// # Errors
///
/// - `InvalidInput`: la geometria di input non supera la validazione OGC;
/// - `InvalidOutput`: il boundary prodotto non supera la validazione OGC;
/// - `ValidazioneNonConclusa`: la validazione OGC va in panico dentro la
///   barriera.
pub fn boundary(geometry: &Geometry<f64>) -> Result<Geometry<f64>, OperationError> {
    ensure_valid(geometry)?;
    let output = boundary_unchecked(geometry);
    validate_output(output)
}

fn boundary_unchecked(geometry: &Geometry<f64>) -> Geometry<f64> {
    match geometry {
        Geometry::Point(_) | Geometry::MultiPoint(_) => {
            Geometry::GeometryCollection(Vec::<Geometry<f64>>::new().into())
        }
        Geometry::Line(line) => {
            Geometry::MultiPoint(MultiPoint::new(vec![line.start_point(), line.end_point()]))
        }
        Geometry::LineString(line) => line_string_boundary(line),
        Geometry::Polygon(polygon) => Geometry::MultiLineString(MultiLineString::new(
            std::iter::once(polygon.exterior().clone())
                .chain(polygon.interiors().iter().cloned())
                .collect(),
        )),
        Geometry::MultiPolygon(polygons) => Geometry::MultiLineString(MultiLineString::new(
            polygons
                .iter()
                .flat_map(|polygon| {
                    std::iter::once(polygon.exterior().clone())
                        .chain(polygon.interiors().iter().cloned())
                })
                .collect(),
        )),
        Geometry::MultiLineString(lines) => multi_line_string_boundary(lines),
        Geometry::GeometryCollection(values) => Geometry::GeometryCollection(
            values
                .iter()
                .map(boundary_unchecked)
                .collect::<Vec<_>>()
                .into(),
        ),
        Geometry::Rect(rect) => boundary_unchecked(&Geometry::Polygon(rect.to_polygon())),
        Geometry::Triangle(triangle) => {
            boundary_unchecked(&Geometry::Polygon(triangle.to_polygon()))
        }
    }
}

fn multi_line_string_boundary(lines: &MultiLineString<f64>) -> Geometry<f64> {
    let mut endpoints: BTreeMap<(u64, u64), (geo::Point<f64>, bool)> = BTreeMap::new();
    for line in lines {
        if line.0.len() < 2 || line.is_closed() {
            continue;
        }
        for coordinate in [line.0[0], line.0[line.0.len() - 1]] {
            let canonical_x = if coordinate.x == 0.0 {
                0.0
            } else {
                coordinate.x
            };
            let canonical_y = if coordinate.y == 0.0 {
                0.0
            } else {
                coordinate.y
            };
            let key = (canonical_x.to_bits(), canonical_y.to_bits());
            endpoints
                .entry(key)
                .and_modify(|(_, odd)| *odd = !*odd)
                .or_insert_with(|| (geo::Point::new(canonical_x, canonical_y), true));
        }
    }
    Geometry::MultiPoint(MultiPoint::new(
        endpoints
            .into_values()
            .filter_map(|(point, odd)| odd.then_some(point))
            .collect(),
    ))
}

fn line_string_boundary(line: &LineString<f64>) -> Geometry<f64> {
    if line.0.len() < 2 || line.is_closed() {
        return Geometry::MultiPoint(MultiPoint::new(Vec::new()));
    }
    Geometry::MultiPoint(MultiPoint::new(vec![
        geo::Point::from(line.0[0]),
        geo::Point::from(line.0[line.0.len() - 1]),
    ]))
}

#[cfg(test)]
mod prove_della_soglia_scalata {
    use super::*;
    use geo::LineString;

    /// Sul percorso scalato (coordinate oltre `1e150`) l'area si scala con
    /// il quadrato del fattore: il risultato e' quello delle stesse
    /// coordinate e della stessa area alla scala naturale. Prima si scalava
    /// come una distanza, e la soglia valeva `1e150` volte di piu'.
    #[test]
    fn l_area_si_scala_con_il_quadrato() {
        let fattore = 1e152;
        let linea = |k: f64| {
            Geometry::LineString(LineString::from(vec![
                (0.0, 0.0),
                (10.0 * k, 1.0 * k),
                (20.0 * k, 0.0),
                (30.0 * k, 5.0 * k),
                (40.0 * k, 0.0),
            ]))
        };
        // Triangoli di area 10 e 50 (in unita' naturali): con soglia 20
        // cade solo il primo vertice interno.
        let naturale =
            simplify_with_policy(&linea(1.0), 20.0, SimplifyPolicy::PreserveTopology).unwrap();
        let scalata = simplify_with_policy(
            &linea(fattore),
            20.0 * fattore * fattore,
            SimplifyPolicy::PreserveTopology,
        )
        .unwrap();
        let conta = |g: &Geometry<f64>| g.coords_iter().count();
        assert_eq!(conta(&naturale), 4);
        assert_eq!(conta(&scalata), conta(&naturale));
    }

    /// Una soglia d'area positiva che nello spazio scalato diventa zero si
    /// rifiuta, invece di non semplificare.
    #[test]
    fn un_area_che_si_annulla_nella_scala_si_rifiuta() {
        let linea = Geometry::LineString(LineString::from(vec![
            (0.0, 0.0),
            (1e200, 1e199),
            (2e200, 0.0),
        ]));
        assert!(matches!(
            simplify_with_policy(&linea, 1e-300, SimplifyPolicy::PreserveTopology),
            Err(OperationError::InvalidParameter { .. })
        ));
    }
}

#[cfg(test)]
// Confronti float esatti intenzionali: le fixture sono costruite per
// produrre valori esatti (coordinate note, round-trip bit-esatti); il
// confronto per bit e' il contratto verificato, non un'approssimazione.
#[allow(clippy::float_cmp)]
mod tests {
    use super::*;
    use crate::test_support::{rect, rect_polygon};
    use geo::{
        line_string, polygon, Contains, GeometryCollection, Line, MultiPolygon, Point, Rect,
        Triangle,
    };
    use proptest::prelude::*;

    /// Precisione dei test: coordinate astratte, un milionesimo di unita'.
    fn precisione() -> Precision {
        Precision::new(1e-6).unwrap()
    }

    fn rectangle() -> Geometry<f64> {
        rect(0.0, 0.0, 4.0, 2.0)
    }

    /// Regressione trovata dal fuzz target `wkt_operations` del progetto
    /// d'origine.
    ///
    /// Poligono valido con `-0.0` accanto a `0.0`: la sweep line di
    /// `interior_point()` ne viola l'invariante (vedi `point_on_surface`).
    #[test]
    fn point_on_surface_survives_negative_zero_coordinates() {
        let with_negative_zero = Geometry::Polygon(polygon![
            (x: 5.0, y: 22.0), (x: 2.0, y: 7.0), (x: -0.0, y: 423.0),
            (x: 0.0, y: 3.0), (x: 9.0, y: 0.0), (x: 5.0, y: 22.0),
        ]);
        let with_positive_zero = Geometry::Polygon(polygon![
            (x: 5.0, y: 22.0), (x: 2.0, y: 7.0), (x: 0.0, y: 423.0),
            (x: 0.0, y: 3.0), (x: 9.0, y: 0.0), (x: 5.0, y: 22.0),
        ]);

        let interior = point_on_surface(&with_negative_zero)
            .expect("il poligono e' valido")
            .expect("un poligono con area ha un punto interno");

        // Le due geometrie sono numericamente identiche, quindi devono
        // produrre lo stesso punto: la normalizzazione non sposta il risultato.
        let reference = point_on_surface(&with_positive_zero)
            .expect("il poligono e' valido")
            .expect("un poligono con area ha un punto interno");
        assert_eq!(interior, reference);

        // Il punto appartiene davvero alla superficie.
        assert!(with_positive_zero.contains(&interior));
    }

    /// La normalizzazione tocca solo lo zero negativo e lascia tutto il resto
    /// bit-identico: non e' un arrotondamento.
    #[test]
    fn signed_zero_normalization_touches_only_negative_zero() {
        assert!(normalize_signed_zero(-0.0).is_sign_positive());
        assert_eq!(normalize_signed_zero(-0.0), 0.0);
        for value in [1.0_f64, -1.0, 0.0, f64::MIN, f64::MAX, 1e-308, -1e-308] {
            assert_eq!(normalize_signed_zero(value).to_bits(), value.to_bits());
        }
        assert!(normalize_signed_zero(f64::NAN).is_nan());
        assert!(is_negative_zero(-0.0));
        assert!(!is_negative_zero(0.0));
        assert!(!is_negative_zero(-1.0));
    }

    #[test]
    fn scalar_measurements_match_known_geometry() {
        let geometry = rectangle();
        assert_eq!(area(&geometry).unwrap(), 8.0);
        assert_eq!(length(&geometry).unwrap(), 12.0);
        assert_eq!(perimeter(&geometry).unwrap(), 12.0);
        assert_eq!(bounds(&geometry).unwrap(), Some([0.0, 0.0, 4.0, 2.0]));
        assert_eq!(vertex_count(&geometry).unwrap(), 5);
        assert!(to_wkt(&geometry).unwrap().starts_with("POLYGON("));
    }

    #[test]
    fn distance_and_interior_point_are_exact_for_simple_fixture() {
        let left = Geometry::Point(Point::new(0.0, 0.0));
        let right = Geometry::Point(Point::new(3.0, 4.0));
        assert_eq!(distance(&left, &right).unwrap(), Some(5.0));
        let point = point_on_surface(&rectangle())
            .unwrap()
            .expect("interior point");
        assert!(rectangle().contains(&point));
    }

    #[test]
    fn buffer_rejects_non_finite_distance_and_produces_valid_polygon() {
        assert!(buffer(&rectangle(), f64::NAN, precisione()).is_err());
        let result = buffer(&Geometry::Point(Point::new(0.0, 0.0)), 2.0, precisione()).unwrap();
        assert!(result.unsigned_area() > 12.0);
        assert!(result.unsigned_area() < 13.0);

        let line = Geometry::LineString(line_string![(x: 0.0, y: 0.0), (x: 2.0, y: 0.0)]);
        let flat = buffer_with_cap(&line, 1.0, BufferCapStyle::Flat, precisione()).unwrap();
        let square = buffer_with_cap(&line, 1.0, BufferCapStyle::Square, precisione()).unwrap();
        let round = buffer_with_cap(&line, 1.0, BufferCapStyle::Round, precisione()).unwrap();
        assert!((flat.unsigned_area() - 4.0).abs() < 1e-9);
        assert!(square.unsigned_area() > flat.unsigned_area());
        assert!(round.unsigned_area() > flat.unsigned_area());
    }

    #[test]
    fn simplify_fails_if_result_would_be_invalid() {
        let line = Geometry::LineString(line_string![
            (x: 0.0, y: 0.0), (x: 1.0, y: 0.01), (x: 2.0, y: 0.0)
        ]);
        let simplified = simplify(&line, 0.1).unwrap();
        assert_eq!(vertex_count(&simplified).unwrap(), 2);
        assert!(simplify(&line, -1.0).is_err());

        let preserved =
            simplify_with_policy(&rectangle(), 0.5, SimplifyPolicy::PreserveTopology).unwrap();
        assert!(preserved.validazione_protetta().is_ok());
    }

    #[test]
    fn simplify_normalizes_extreme_coordinates_without_panicking() {
        let line = Geometry::LineString(LineString::from(vec![
            (-5.488_802_840_312_24e303, -6.971_241_357_778_827e182),
            (-5.486_124_068_793_689e303, 7.064_166_183_585_296e-304),
            (0.0, 0.0),
        ]));
        for tolerance in [0.0, 1.0] {
            for policy in [
                SimplifyPolicy::DouglasPeucker,
                SimplifyPolicy::PreserveTopology,
            ] {
                if tolerance > 0.0 && policy == SimplifyPolicy::PreserveTopology {
                    // L'area 1 in coordinate di modulo 5e303 si annulla nello
                    // spazio scalato (il quadrato del fattore): rifiutata,
                    // invece di non togliere i vertici degeneri.
                    assert!(matches!(
                        simplify_with_policy(&line, tolerance, policy),
                        Err(OperationError::InvalidParameter { .. })
                    ));
                    continue;
                }
                let output = simplify_with_policy(&line, tolerance, policy).unwrap();
                assert!(output.validazione_protetta().is_ok());
                assert!(output
                    .coords_iter()
                    .all(|coordinate| coordinate.x.is_finite() && coordinate.y.is_finite()));
            }
        }
    }

    #[test]
    fn explode_preserves_component_order() {
        let collection = Geometry::GeometryCollection(GeometryCollection(vec![
            Geometry::Point(Point::new(1.0, 2.0)),
            Geometry::Point(Point::new(3.0, 4.0)),
        ]));
        assert_eq!(
            explode(&collection).unwrap(),
            vec![
                Geometry::Point(Point::new(1.0, 2.0)),
                Geometry::Point(Point::new(3.0, 4.0)),
            ]
        );
    }

    #[test]
    fn boundary_handles_open_and_closed_lines() {
        let open = Geometry::LineString(line_string![(x: 0.0, y: 0.0), (x: 2.0, y: 0.0)]);
        assert_eq!(vertex_count(&boundary(&open).unwrap()).unwrap(), 2);
        let closed = Geometry::LineString(line_string![
            (x: 0.0, y: 0.0), (x: 1.0, y: 0.0), (x: 0.0, y: 0.0)
        ]);
        assert_eq!(vertex_count(&boundary(&closed).unwrap()).unwrap(), 0);

        let touching = Geometry::MultiLineString(MultiLineString::new(vec![
            line_string![(x: 0.0, y: 0.0), (x: 1.0, y: 0.0)],
            line_string![(x: 1.0, y: 0.0), (x: 2.0, y: 0.0)],
        ]));
        assert_eq!(vertex_count(&boundary(&touching).unwrap()).unwrap(), 2);
    }

    #[test]
    fn measurements_simplify_explode_and_boundary_cover_all_geometry_families() {
        let Geometry::Polygon(polygon) = rectangle() else {
            unreachable!()
        };
        let values = vec![
            Geometry::Point(Point::new(0.0, 0.0)),
            Geometry::MultiPoint(MultiPoint::new(vec![Point::new(0.0, 0.0)])),
            Geometry::Line(Line::new((0.0, 0.0), (3.0, 4.0))),
            Geometry::LineString(line_string![(x: 0.0, y: 0.0), (x: 3.0, y: 4.0)]),
            Geometry::MultiLineString(MultiLineString::new(vec![line_string![
                (x: 0.0, y: 0.0), (x: 3.0, y: 4.0)
            ]])),
            Geometry::Polygon(polygon.clone()),
            Geometry::MultiPolygon(MultiPolygon::new(vec![polygon])),
            Geometry::GeometryCollection(GeometryCollection(vec![rectangle()])),
            Geometry::Rect(Rect::new((0.0, 0.0), (2.0, 1.0))),
            Geometry::Triangle(Triangle::new(
                (0.0, 0.0).into(),
                (2.0, 0.0).into(),
                (0.0, 1.0).into(),
            )),
        ];
        for value in &values {
            assert!(length(value).unwrap().is_finite());
            assert!(boundary(value).unwrap().validazione_protetta().is_ok());
            assert!(!explode(value).unwrap().is_empty());
            for policy in [
                SimplifyPolicy::DouglasPeucker,
                SimplifyPolicy::PreserveTopology,
            ] {
                assert!(simplify_with_policy(value, 0.01, policy)
                    .unwrap()
                    .validazione_protetta()
                    .is_ok());
            }
        }

        assert_eq!(
            distance(&Geometry::MultiPoint(MultiPoint::new(vec![])), &values[0]).unwrap(),
            None
        );
        assert_eq!(
            bounds(&Geometry::MultiPoint(MultiPoint::new(vec![]))).unwrap(),
            None
        );
        assert_eq!(
            point_on_surface(&Geometry::MultiPoint(MultiPoint::new(vec![]))).unwrap(),
            None
        );
    }

    #[test]
    fn multipart_explode_and_boundary_endpoint_parity_are_deterministic() {
        let Geometry::Polygon(polygon) = rectangle() else {
            unreachable!()
        };
        assert_eq!(
            explode(&Geometry::MultiPoint(MultiPoint::new(vec![
                Point::new(0.0, 0.0),
                Point::new(1.0, 1.0),
            ])))
            .unwrap()
            .len(),
            2
        );
        assert_eq!(
            explode(&Geometry::MultiLineString(MultiLineString::new(vec![
                line_string![(x: 0.0, y: 0.0), (x: 1.0, y: 0.0)],
                line_string![(x: 2.0, y: 0.0), (x: 3.0, y: 0.0)],
            ])))
            .unwrap()
            .len(),
            2
        );
        assert_eq!(
            explode(&Geometry::MultiPolygon(MultiPolygon::new(vec![
                polygon,
                rect_polygon(10.0, 0.0, 14.0, 2.0),
            ])))
            .unwrap()
            .len(),
            2
        );

        let duplicated = Geometry::MultiLineString(MultiLineString::new(vec![
            line_string![(x: -0.0, y: 0.0), (x: 1.0, y: 0.0)],
            line_string![(x: 0.0, y: -0.0), (x: 1.0, y: 0.0)],
            line_string![],
            line_string![(x: 5.0, y: 5.0), (x: 5.0, y: 5.0)],
        ]));
        assert_eq!(vertex_count(&boundary_unchecked(&duplicated)).unwrap(), 0);
    }

    /// `MultiPolygon` col primo (e unico) componente vuoto.
    ///
    /// E' valido OGC, quindi il gate d'ingresso di `to_wkt` non lo ferma:
    /// l'esito corretto e' `MULTIPOLYGON(EMPTY)`, non un errore ne' un panico.
    #[test]
    fn to_wkt_su_multipolygon_col_primo_componente_vuoto_non_panica() {
        let vuoto = geo::Polygon::new(LineString::from(Vec::<(f64, f64)>::new()), Vec::new());
        let orfano = Geometry::MultiPolygon(MultiPolygon::new(vec![vuoto]));

        assert_eq!(
            to_wkt(&orfano).expect("il componente vuoto e' un multipolygon vuoto valido"),
            "MULTIPOLYGON(EMPTY)"
        );
    }

    /// Il caso che l'encoder rifiuta: esterno vuoto con un interno non vuoto.
    ///
    /// Diverso dal caso sopra, dove non ci sono interni: la condizione del
    /// guardiano (`num_interiors() != 0 && esterno vuoto`) scatta solo qui.
    #[test]
    fn to_wkt_su_anello_interno_senza_esterno_e_rifiutato() {
        let interno = line_string![
            (x: 1.0, y: 1.0), (x: 2.0, y: 1.0),
            (x: 2.0, y: 2.0), (x: 1.0, y: 1.0),
        ];
        let orfano = Geometry::Polygon(geo::Polygon::new(
            LineString::from(Vec::<(f64, f64)>::new()),
            vec![interno],
        ));

        let errore = to_wkt(&orfano).expect_err("un interno senza esterno non e' serializzabile");
        assert!(
            matches!(&errore, OperationError::WktSerialization(testo) if testo == "geometria non serializzabile"),
            "atteso WktSerialization con testo statico, ottenuto: {errore:?}"
        );
    }

    /// Controprova: lo stesso poligono vuoto **da solo**, senza interni, non
    /// e' l'anello orfano di sopra — resta un vuoto legittimo. Senza questa
    /// prova le due sopra non distinguerebbero "vuoto legittimo" da "vuoto
    /// col guardiano attivato": proverebbero solo che *qualche* ingresso
    /// passa e *qualcuno* fallisce, non quale proprieta' decide.
    #[test]
    fn to_wkt_su_singolo_poligono_vuoto_senza_interni_passa_da_sempre() {
        let vuoto = Geometry::Polygon(geo::Polygon::new(
            LineString::from(Vec::<(f64, f64)>::new()),
            Vec::new(),
        ));
        assert_eq!(
            to_wkt(&vuoto).expect("un poligono vuoto senza interni e' sempre serializzabile"),
            "POLYGON EMPTY"
        );
    }

    // Z/M/ZM, streaming e round-trip non si esercitano qui: il confine WKB
    // (`crate::geometry_from_wkb`) rifiuta ogni payload Z/M/ZM, e
    // `to_wkt` espone solo la firma a `String`. Quei casi li copre la suite
    // upstream di `wkt` (`vendor/wkt-0.14.0-v2`).

    /// Zero componenti: il `MultiPolygon` vuoto stesso.
    ///
    /// La forma resa e' senza parentesi, solo `EMPTY` dopo il prefisso (vedi
    /// `wkt::to_wkt::geo_trait_impl::write_multi_polygon`).
    #[test]
    fn to_wkt_su_multipolygon_a_zero_componenti() {
        let vuoto = Geometry::MultiPolygon(MultiPolygon::new(Vec::new()));
        assert_eq!(
            to_wkt(&vuoto).expect("un multipolygon a zero componenti e' vuoto valido"),
            "MULTIPOLYGON EMPTY"
        );
    }

    /// Vuoti iniziali/intermedi/finali: la posizione del componente vuoto
    /// dentro un `MultiPolygon` con altri componenti ordinari non cambia
    /// l'esito — ne' fa scattare il rifiuto (nessun interno coinvolto), ne'
    /// sposta gli altri componenti nella stringa resa.
    #[test]
    fn to_wkt_su_multipolygon_con_vuoto_in_diverse_posizioni() {
        // Due quadrati DISTINTI e non sovrapposti: con componenti
        // sovrapposti `ensure_valid` rifiuterebbe l'ingresso prima del vuoto.
        let ordinario = || {
            geo::Polygon::new(
                line_string![
                    (x: 0.0, y: 0.0), (x: 4.0, y: 0.0),
                    (x: 4.0, y: 4.0), (x: 0.0, y: 4.0),
                    (x: 0.0, y: 0.0),
                ],
                Vec::new(),
            )
        };
        let ordinario2 = || {
            geo::Polygon::new(
                line_string![
                    (x: 10.0, y: 0.0), (x: 14.0, y: 0.0),
                    (x: 14.0, y: 4.0), (x: 10.0, y: 4.0),
                    (x: 10.0, y: 0.0),
                ],
                Vec::new(),
            )
        };
        let vuoto = || geo::Polygon::new(LineString::from(Vec::<(f64, f64)>::new()), Vec::new());
        // Doppie parentesi: la esterna delimita il poligono dentro il
        // MultiPolygon, l'interna la sequenza di coordinate dell'anello
        // esterno -- write_polygon_body/write_coord_sequence in
        // vendor/wkt-0.14.0-v2/src/to_wkt/geo_trait_impl.rs.
        let corpo_ordinario = "((0 0,4 0,4 4,0 4,0 0))";
        let corpo_ordinario2 = "((10 0,14 0,14 4,10 4,10 0))";

        let casi = [
            (
                vec![vuoto(), ordinario(), ordinario2()],
                format!("MULTIPOLYGON(EMPTY,{corpo_ordinario},{corpo_ordinario2})"),
            ),
            (
                vec![ordinario(), vuoto(), ordinario2()],
                format!("MULTIPOLYGON({corpo_ordinario},EMPTY,{corpo_ordinario2})"),
            ),
            (
                vec![ordinario(), ordinario2(), vuoto()],
                format!("MULTIPOLYGON({corpo_ordinario},{corpo_ordinario2},EMPTY)"),
            ),
        ];

        for (componenti, atteso) in casi {
            let geometria = Geometry::MultiPolygon(MultiPolygon::new(componenti));
            assert_eq!(
                to_wkt(&geometria).expect("il vuoto senza interni non fa mai rifiutare"),
                atteso
            );
        }
    }

    /// Controprova: un interno valido (esterno non vuoto) serializza per
    /// intero, e il guardiano su `InteriorWithoutExterior` non scatta.
    #[test]
    fn to_wkt_su_poligono_con_interno_valido_serializza_entrambi_gli_anelli() {
        let poligono = Geometry::Polygon(geo::Polygon::new(
            line_string![
                (x: 0.0, y: 0.0), (x: 4.0, y: 0.0),
                (x: 4.0, y: 4.0), (x: 0.0, y: 4.0),
                (x: 0.0, y: 0.0),
            ],
            vec![line_string![
                (x: 1.0, y: 1.0), (x: 2.0, y: 1.0),
                (x: 2.0, y: 2.0), (x: 1.0, y: 2.0),
                (x: 1.0, y: 1.0),
            ]],
        ));
        assert_eq!(
            to_wkt(&poligono).expect("esterno e interno non vuoti sono sempre serializzabili"),
            "POLYGON((0 0,4 0,4 4,0 4,0 0),(1 1,2 1,2 2,1 2,1 1))"
        );
    }

    /// L'anello orfano annidato in una `GeometryCollection`.
    ///
    /// Il rifiuto propaga attraverso la ricorsione di
    /// `write_geometry_collection`, non si ferma al primo membro valido.
    #[test]
    fn to_wkt_su_geometrycollection_con_membro_orfano_annidato_e_rifiutato() {
        let interno = line_string![
            (x: 1.0, y: 1.0), (x: 2.0, y: 1.0),
            (x: 2.0, y: 2.0), (x: 1.0, y: 1.0),
        ];
        let orfano = Geometry::Polygon(geo::Polygon::new(
            LineString::from(Vec::<(f64, f64)>::new()),
            vec![interno],
        ));
        let valido = Geometry::Point(Point::new(0.0, 0.0));
        let collection =
            Geometry::GeometryCollection(GeometryCollection::new_from(vec![valido, orfano]));

        let errore = to_wkt(&collection)
            .expect_err("un membro orfano annidato deve rifiutare l'intera collection");
        assert!(
            matches!(&errore, OperationError::WktSerialization(testo) if testo == "geometria non serializzabile"),
            "atteso WktSerialization con testo statico, ottenuto: {errore:?}"
        );
    }

    /// Il quadrato 4x4 dei test sul buffer con componenti vuoti.
    fn quadrato_4x4() -> geo::Polygon<f64> {
        rect_polygon(0.0, 0.0, 4.0, 4.0)
    }

    /// Il buffer con componenti vuoti riesce e coincide con quello della
    /// stessa geometria senza vuoti (oracolo): i vuoti non aggiungono ne'
    /// tolgono area. Sul quadrato 4x4 le aree hanno anche un valore atteso:
    /// 4 verso l'interno, 16 a distanza nulla, tra 32 e 32 + pi verso
    /// l'esterno (quattro fasce laterali piu' gli spigoli arrotondati,
    /// approssimati da poligoni inscritti).
    fn assert_buffer_come_senza_vuoti(
        con_vuoti: &Geometry<f64>,
        senza_vuoti: &Geometry<f64>,
        caso: &str,
    ) {
        for (distance, aree) in [
            (-1.0, 4.0..=4.0),
            (0.0, 16.0..=16.0),
            (1.0, 32.0..=32.0 + std::f64::consts::PI),
        ] {
            let risultato = buffer(con_vuoti, distance, precisione()).unwrap_or_else(|errore| {
                panic!("{caso}, distanza {distance}: buffer rifiutato: {errore}")
            });
            let oracolo = buffer(senza_vuoti, distance, precisione()).unwrap_or_else(|errore| {
                panic!("{caso}, distanza {distance}: oracolo rifiutato: {errore}")
            });
            assert_eq!(risultato, oracolo, "{caso}, distanza {distance}");
            let area = risultato.unsigned_area();
            assert!(
                aree.contains(&area),
                "{caso}, distanza {distance}: area {area} fuori da {aree:?}"
            );
        }
    }

    /// Solo vuoti: il buffer riesce e da' il `MultiPolygon` vuoto, a ogni
    /// distanza.
    fn assert_buffer_vuoto(geometria: &Geometry<f64>, caso: &str) {
        for distance in [-1.0, 0.0, 1.0] {
            let risultato = buffer(geometria, distance, precisione()).unwrap_or_else(|errore| {
                panic!("{caso}, distanza {distance}: buffer rifiutato: {errore}")
            });
            assert_eq!(
                risultato,
                Geometry::MultiPolygon(MultiPolygon::new(Vec::new())),
                "{caso}, distanza {distance}"
            );
        }
    }

    /// "Tutti-vuoti": nessun componente ordinario a fare da controllo, ogni
    /// elemento del `MultiPolygon` e' vuoto. Distinto dal caso di un vuoto
    /// accanto a un ordinario (sotto): qui non c'e' alcun percorso non-vuoto
    /// che possa mascherare un problema sui vuoti.
    #[test]
    fn buffer_su_multipolygon_con_tutti_i_componenti_vuoti_non_panica() {
        let vuoto = || geo::Polygon::new(LineString::from(Vec::<(f64, f64)>::new()), Vec::new());
        let geometria = Geometry::MultiPolygon(MultiPolygon::new(vec![vuoto(), vuoto(), vuoto()]));

        assert_buffer_vuoto(&geometria, "tutti vuoti");
    }

    /// Geometria interamente vuota, zero componenti: non un componente
    /// vuoto dentro un `MultiPolygon` non vuoto, ma il `MultiPolygon` vuoto
    /// stesso.
    #[test]
    fn buffer_su_multipolygon_a_zero_componenti_non_panica() {
        let geometria = Geometry::MultiPolygon(MultiPolygon::new(Vec::new()));

        assert_buffer_vuoto(&geometria, "zero componenti");
    }

    /// Componente vuoto accanto a uno ordinario, passato a `buffer`: in coda,
    /// all'inizio o in mezzo, l'esito non dipende dalla sua posizione.
    ///
    /// L'offset planare di `i_overlay` 9 salta i percorsi con meno di tre
    /// punti prima di calcolarne l'area.
    #[test]
    fn buffer_su_multipolygon_con_vuoto_in_diverse_posizioni_non_panica() {
        let ordinario = quadrato_4x4;
        let ordinario2 = || rect_polygon(10.0, 0.0, 14.0, 4.0);
        let vuoto = || geo::Polygon::new(LineString::from(Vec::<(f64, f64)>::new()), Vec::new());

        // `distance` negativa, nulla e positiva (offset interno/esterno):
        // l'esito e' quello della geometria senza il vuoto.
        let solo_ordinario = Geometry::MultiPolygon(MultiPolygon::new(vec![ordinario()]));
        let vuoto_in_coda = Geometry::MultiPolygon(MultiPolygon::new(vec![ordinario(), vuoto()]));
        assert_buffer_come_senza_vuoti(&vuoto_in_coda, &solo_ordinario, "vuoto in coda");
        let vuoto_iniziale = Geometry::MultiPolygon(MultiPolygon::new(vec![vuoto(), ordinario()]));
        assert_buffer_come_senza_vuoti(&vuoto_iniziale, &solo_ordinario, "vuoto iniziale");

        // Componenti DISTINTI e non sovrapposti: due copie dello stesso
        // quadrato sarebbero un `MultiPolygon` invalido, rifiutato prima del
        // vuoto. Con ingresso valido il buffer deve riuscire davvero: niente
        // `let _ =`, che assolverebbe un rifiuto per la ragione sbagliata.
        let vuoto_centrale =
            Geometry::MultiPolygon(MultiPolygon::new(vec![ordinario(), vuoto(), ordinario2()]));
        for distance in [-1.0, 0.0, 1.0] {
            let risultato =
                buffer(&vuoto_centrale, distance, precisione()).unwrap_or_else(|errore| {
                    panic!("buffer con vuoto centrale a distanza {distance}: {errore}")
                });
            assert!(
                risultato.unsigned_area() > 0.0,
                "buffer con vuoto centrale a distanza {distance}: area non positiva"
            );
        }
    }

    /// Collection: lo stesso componente vuoto raggiunge l'offset anche
    /// annidato dentro una `GeometryCollection` invece che come
    /// `MultiPolygon` di primo livello — `geo::Buffer` per
    /// `GeometryCollection` itera i propri membri e delega a ciascuno.
    #[test]
    fn buffer_su_geometrycollection_con_componente_vuoto_non_panica() {
        let vuoto = geo::Polygon::new(LineString::from(Vec::<(f64, f64)>::new()), Vec::new());
        let multipolygon_con_vuoto = MultiPolygon::new(vec![quadrato_4x4(), vuoto]);
        let geometria = Geometry::GeometryCollection(GeometryCollection::new_from(vec![
            Geometry::MultiPolygon(multipolygon_con_vuoto),
        ]));
        let senza_vuoti = Geometry::MultiPolygon(MultiPolygon::new(vec![quadrato_4x4()]));

        assert_buffer_come_senza_vuoti(&geometria, &senza_vuoti, "collection con vuoto");
    }

    /// Controllo: senza vuoti il buffer riesce alle stesse tre distanze.
    ///
    /// Il caso di base degli oracoli sopra, fuori da ogni `MultiPolygon`. La
    /// soglia su area > 0 non dipende dall'implementazione dell'offset
    /// planare.
    #[test]
    fn buffer_su_poligono_ordinario_senza_vuoti_riesce_alle_stesse_distanze() {
        let ordinario = rect(0.0, 0.0, 4.0, 4.0);

        for distance in [-1.0, 0.0, 1.0] {
            let risultato = buffer(&ordinario, distance, precisione()).unwrap_or_else(|errore| {
                panic!("buffer ordinario a distanza {distance}: {errore}")
            });
            assert!(
                risultato.unsigned_area() > 0.0,
                "buffer ordinario a distanza {distance}: area non positiva"
            );
        }
    }

    proptest! {
        #[test]
        fn rectangle_measurements_hold_for_generated_inputs(
            x in -10_000_i32..10_000,
            y in -10_000_i32..10_000,
            width in 1_u16..1000,
            height in 1_u16..1000,
        ) {
            let x = f64::from(x);
            let y = f64::from(y);
            let width = f64::from(width);
            let height = f64::from(height);
            let geometry = rect(x, y, x + width, y + height);
            prop_assert_eq!(area(&geometry).unwrap(), width * height);
            prop_assert_eq!(length(&geometry).unwrap(), 2.0 * (width + height));
            prop_assert_eq!(bounds(&geometry).unwrap(), Some([x, y, x + width, y + height]));
            prop_assert_eq!(vertex_count(&geometry).unwrap(), 5);
            let interior = point_on_surface(&geometry).unwrap().unwrap();
            prop_assert!(geometry.contains(&interior));
        }
    }
}
