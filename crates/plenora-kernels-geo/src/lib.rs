//! plenora-kernels-geo — kernel geografici su `geo::Geometry<f64>` e adapter
//! Arrow per il canone GeoArrow-WKB.
//!
//! Contiene il validatore WKB strutturale, i kernel puri (Rust puro: niente
//! backend GEOS/PROJ), [`arrow_adapter`] per la rappresentazione
//! GeoArrow-WKB e [`analyze`] per l'inferenza a secco dei contratti. Le
//! operazioni geo di un piano le esegue il runner (`plenora-pipeline`), che
//! chiama questi kernel riga per riga o sulla tabella intera.
//!
//! [`memory_estimate`] da' una STIMA della memoria nativa delle geometrie
//! decodificate, mai un conteggio preciso; [`geometry_contract`] fissa la
//! dimensione esatta del WKB ISO XY e la validazione strutturale su
//! `Geometry`. Gli errori pubblici sono [`plenora_core::PlenoraError`]; i
//! moduli di kernel hanno anche errori propri
//! (`operations::OperationError`, `construction::ConstructionError`, ...).

pub mod advanced;
pub mod analysis;
pub mod analyze;
pub mod arrow_adapter;
pub mod cluster;
pub mod construction;
pub mod crs;
pub mod decoded_size;
pub mod equality;
pub mod extended;
pub mod extended_algorithms;
pub mod extensions;
pub mod extensions2;
pub mod extensions3;
pub mod geodetica;
pub mod geometry_contract;
pub mod margine;
pub mod memory_estimate;
pub mod operations;
pub mod predicates;
pub mod riproiezione;
pub mod rust_backend;
pub mod spatial_join;
#[cfg(test)]
mod test_support;
pub mod topology;
mod triangolazione;
mod validazione_ogc;
pub mod wkb_decoder;

use geo::{
    BoundingRect, Centroid, ConvexHull, Coord, CoordsIter, Geometry, LineString, MapCoords, Point,
};
use geozero::{CoordDimensions, ToWkb};
use plenora_core::contract::{GeometryDimensions, GeometryEncoding};
use plenora_core::PlenoraError;
use serde::{Deserialize, Serialize};

/// Le trasformazioni 1:1 di [`transform_geometry`] e [`transform_wkb`]:
/// i kernel di `geo.centroid`, `geo.convex_hull` e `geo.envelope`.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Operation {
    /// Il centroide (`geo.centroid`): un `Point`, baricentro pesato per
    /// area, lunghezza o numero di punti secondo la dimensione piu' alta
    /// presente; errore su una geometria vuota.
    Centroid,
    /// L'inviluppo convesso (`geo.convex_hull`): un `Polygon`; errore se
    /// l'inviluppo e' degenere (punto, segmento, punti collineari).
    ConvexHull,
    /// Il rettangolo d'ingombro (`geo.envelope`): `Polygon`, oppure `Point`
    /// o `LineString` se una o entrambe le dimensioni sono nulle; errore su
    /// una geometria vuota.
    Envelope,
}

impl Operation {
    /// Tutte le varianti, nell'ordine di dichiarazione.
    pub const ALL: [Self; 3] = [Self::Centroid, Self::ConvexHull, Self::Envelope];

    /// Il nome breve, senza il prefisso `geo.` (lo stesso di serde).
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Centroid => "centroid",
            Self::ConvexHull => "convex_hull",
            Self::Envelope => "envelope",
        }
    }
}

/// Costruttori degli errori di geometria su `PlenoraError`.
fn empty_geometry(operation: &'static str) -> PlenoraError {
    PlenoraError::InvalidPlan(format!("geometria vuota non supportata da {operation}"))
}

fn wkb_serialization(error: impl std::fmt::Display) -> PlenoraError {
    PlenoraError::InvalidPlan(format!("serializzazione WKB fallita: {error}"))
}

fn unsupported_wkb_dimension() -> PlenoraError {
    PlenoraError::Unsupported(
        "WKB contiene dimensioni Z/M o SRID non preservabili nel protocollo 2D".to_owned(),
    )
}

/// Errore dedicato di coerenza: il type code dichiara una
/// dimensionalita' diversa da quella attesa dal contratto. Mai un
/// passthrough silenzioso: la divergenza e' sempre un errore esplicito.
fn wkb_dimension_mismatch() -> PlenoraError {
    PlenoraError::InvalidPlan(
        "dimensionalita' WKB incoerente con la dimensionalita' attesa dal contratto".to_owned(),
    )
}

pub(crate) fn non_finite_coordinate() -> PlenoraError {
    PlenoraError::InvalidPlan("WKB contiene coordinate NaN o infinite".to_owned())
}

pub(crate) fn invalid_wkb_structure(reason: &'static str) -> PlenoraError {
    PlenoraError::InvalidPlan(format!("struttura WKB non valida: {reason}"))
}

pub(crate) const fn geometry_type_name(geometry: &Geometry<f64>) -> &'static str {
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

fn invalid_geometry(error: impl std::fmt::Display) -> PlenoraError {
    PlenoraError::InvalidPlan(format!("geometria OGC non valida: {error}"))
}

/// Validazione OGC di una geometria, **dietro una barriera**.
///
/// # Errors
///
/// [`PlenoraError::InvalidPlan`] se la geometria non e' valida secondo OGC;
/// [`PlenoraError::Internal`] se la validazione **non conclude**: nessuno ha
/// dimostrato che l'ingresso sia invalido, come per il panico di un kernel
/// nell'executor.
pub(crate) fn valida_ogc<G>(geometria: &G) -> Result<(), PlenoraError>
where
    G: validazione_ogc::ValidazioneOgc + AnelliSemplici,
    G::Error: std::fmt::Display,
{
    match geometria.validazione_protetta() {
        Ok(()) => Ok(()),
        Err(EsitoValidazione::NonValida(ragione)) => Err(invalid_geometry(ragione)),
        Err(esito @ EsitoValidazione::NonConclusa(_)) => {
            Err(PlenoraError::Internal(esito.to_string()))
        }
    }
}

/// Perche' una geometria non e' utilizzabile, in **vocabolario nostro**.
///
/// I messaggi di `geo` interpolano indici dell'ingresso, cioe' fatti sui dati
/// di chi chiama: come per ogni dipendenza, il testo si legge per
/// classificare e non attraversa il confine. Una forma non riconosciuta
/// cade su [`Self::NonSpecificata`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RagioneNonValida {
    /// Una coordinata non finita: NaN o infinito.
    CoordinataNonFinita,
    /// Meno punti distinti di quanti la forma ne richieda.
    PuntiDistintiInsufficienti,
    /// Un anello che interseca se stesso.
    AutoIntersezione,
    /// Due anelli che si intersecano su una linea o su un'area.
    AnelliIntersecanti,
    /// Un anello interno che esce dal proprio esterno.
    AnelloInternoFuori,
    /// Due poligoni di una multi-geometria che si sovrappongono.
    PoligoniSovrapposti,
    /// Non valida, in una forma che questa classificazione non distingue.
    NonSpecificata,
}

impl RagioneNonValida {
    /// Classifica leggendo il testo della dipendenza, senza propagarlo.
    fn dal_testo(testo: &str) -> Self {
        // L'ordine conta solo dove due forme potrebbero comparire insieme;
        // le voci sono disgiunte nel vocabolario di `geo`.
        if testo.contains("non-finite") || testo.contains("non finite") {
            Self::CoordinataNonFinita
        } else if testo.contains("distinct points") {
            Self::PuntiDistintiInsufficienti
        } else if testo.contains("self-intersection") {
            Self::AutoIntersezione
        } else if testo.contains("intersect on") {
            Self::AnelliIntersecanti
        } else if testo.contains("not contained within") {
            Self::AnelloInternoFuori
        } else if testo.contains("overlap") {
            Self::PoligoniSovrapposti
        } else {
            Self::NonSpecificata
        }
    }
}

impl std::fmt::Display for RagioneNonValida {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::CoordinataNonFinita => "coordinata non finita",
            Self::PuntiDistintiInsufficienti => "punti distinti insufficienti",
            Self::AutoIntersezione => "anello con auto-intersezione",
            Self::AnelliIntersecanti => "anelli che si intersecano",
            Self::AnelloInternoFuori => "anello interno fuori dal proprio esterno",
            Self::PoligoniSovrapposti => "poligoni sovrapposti",
            Self::NonSpecificata => "forma non valida non ulteriormente distinta",
        })
    }
}

/// Come e' andata una validazione **protetta**: due esiti che non si
/// confondono.
///
/// Un validatore interrotto non ha dimostrato niente sull'ingresso: un tipo
/// solo per i due casi spingerebbe chi classifica verso `InvalidPlan`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EsitoValidazione {
    /// La validazione ha concluso: la geometria non e' valida.
    NonValida(RagioneNonValida),
    /// La validazione non ha concluso, e questa e' la forma del payload.
    NonConclusa(&'static str),
}

impl std::fmt::Display for EsitoValidazione {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NonValida(ragione) => write!(f, "{ragione}"),
            Self::NonConclusa(forma) => {
                write!(f, "la validazione non ha potuto concludere: {forma}")
            }
        }
    }
}

impl EsitoValidazione {
    /// Separa i due casi verso l'errore del chiamante.
    ///
    /// `to_string()` farebbe collassare i due casi in un testo; qui sono due
    /// argomenti, e il compilatore non lascia dimenticarne uno.
    pub(crate) fn separa<E>(
        self,
        invalida: impl FnOnce(RagioneNonValida) -> E,
        interrotta: impl FnOnce(&'static str) -> E,
    ) -> E {
        match self {
            Self::NonValida(ragione) => invalida(ragione),
            Self::NonConclusa(forma) => interrotta(forma),
        }
    }
}

/// La validazione OGC, dietro la barriera, per chi ha un errore proprio.
///
/// E' un tratto perche' ogni sito mappa il guasto sul proprio tipo d'errore
/// (`AdvancedError`, `ClusterError`, ...): il metodo affianca
/// `check_validation` e lascia intatte le mappature.
pub(crate) trait ValidazioneProtetta {
    /// Come `check_validation`, ma **non va in panico**.
    ///
    /// # Errors
    ///
    /// [`EsitoValidazione`], che tiene distinti «non valida» e «non conclusa».
    fn validazione_protetta(&self) -> std::result::Result<(), EsitoValidazione>;
}

impl<G> ValidazioneProtetta for G
where
    G: validazione_ogc::ValidazioneOgc + AnelliSemplici,
    G::Error: std::fmt::Display,
{
    fn validazione_protetta(&self) -> std::result::Result<(), EsitoValidazione> {
        // Prima delle punte, poi `geo`: un anello collassato non deve
        // arrivare a `relate`, che su di lui rende matrici prive di senso o va
        // in panico (vedi [`AnelliSemplici`]).
        if self.ha_un_anello_con_punta() {
            return Err(EsitoValidazione::NonValida(
                RagioneNonValida::AutoIntersezione,
            ));
        }
        // `check_validation` di `geo` puo' andare in panico: la sua `relate`
        // chiama `panic!` quando due conclusioni sullo stesso punto si
        // contraddicono, anche su geometrie invalide in ingresso. Gli anelli
        // passano dalla stessa sequenza con la ricerca rapida delle
        // auto-intersezioni (vedi `validazione_ogc`), che chiama la stessa
        // `relate`: stessa barriera.
        let esito =
            plenora_core::panic_policy::barriera_di_dipendenza(std::panic::AssertUnwindSafe(
                || validazione_ogc::ValidazioneOgc::valida_ogc_rapida(self),
            ));
        match esito {
            Ok(Ok(())) => Ok(()),
            Ok(Err(causa)) => Err(EsitoValidazione::NonValida(RagioneNonValida::dal_testo(
                &causa.to_string(),
            ))),
            // Il payload porta coordinate dell'ingresso: si pubblica la sua
            // forma, non il contenuto. L'hook di panico di `std` lo stampa
            // prima di `catch_unwind`, quindi chi ospita il crate deve
            // installare la politica di `plenora_core::panic_policy`.
            Err(payload) => Err(EsitoValidazione::NonConclusa(
                plenora_core::panic_policy::forma_payload(&*payload),
            )),
        }
    }
}

/// Gli anelli di un poligono che **tornano indietro** su se stessi.
///
/// `geo` 0.33.1 non le vede: la sua ricerca di auto-intersezioni salta le
/// coppie di segmenti adiacenti, e una punta (il segmento che ripercorre il
/// precedente) sta proprio fra due adiacenti. Su un anello collassato
/// `relate` rende matrici prive di senso, e la validazione dei buchi accetta
/// poligoni invalidi, oppure va in panico.
///
/// Il controllo e' esatto: collinearita' dal segno di `orient2d` del kernel
/// robusto, verso da un confronto di coordinate, nessuna tolleranza.
pub(crate) trait AnelliSemplici {
    /// Se un anello di un poligono contenuto ha una punta.
    fn ha_un_anello_con_punta(&self) -> bool;
}

/// Una punta nell'anello, cercata sui vertici distinti consecutivi.
///
/// Un anello con coordinate non finite non si giudica qui: e' `geo` a
/// rifiutarlo con la ragione giusta, e una NaN renderebbe collineare
/// qualunque terna.
fn anello_con_punta(anello: &geo::LineString<f64>) -> bool {
    use geo::algorithm::kernels::{Kernel, Orientation, RobustKernel};

    let mut vertici: Vec<geo::Coord<f64>> = Vec::with_capacity(anello.0.len());
    for &punto in &anello.0 {
        if !punto.x.is_finite() || !punto.y.is_finite() {
            return false;
        }
        if vertici.last() != Some(&punto) {
            vertici.push(punto);
        }
    }
    // L'anello chiuso ripete il primo vertice in coda: il giro lo riprende.
    while vertici.len() > 1 && vertici.first() == vertici.last() {
        vertici.pop();
    }
    let quanti = vertici.len();
    // Meno di tre vertici distinti: lo rifiuta `geo`, con la ragione propria.
    if quanti < 3 {
        return false;
    }
    (0..quanti).any(|indice| {
        let precedente = vertici[(indice + quanti - 1) % quanti];
        let centro = vertici[indice];
        let successivo = vertici[(indice + 1) % quanti];
        RobustKernel::orient2d(precedente, centro, successivo) == Orientation::Collinear
            && stesso_verso(precedente, centro, successivo)
    })
}

/// Fra tre punti **collineari** e distinti dal centro, se il primo e il
/// terzo stanno dalla stessa parte di `centro`: la punta.
///
/// Sulla retta la posizione si legge da una coordinata sola: la `x`, o la `y`
/// se la retta e' verticale. Se la retta non e' verticale e il terzo ha la
/// stessa `x` del centro, allora coincide col centro, che la deduplicazione
/// dei vertici esclude.
// Confronti float esatti intenzionali: si chiede se due coordinate siano la
// stessa, non se siano vicine, perche' il controllo non ha tolleranza.
#[allow(clippy::float_cmp)]
fn stesso_verso(primo: geo::Coord<f64>, centro: geo::Coord<f64>, terzo: geo::Coord<f64>) -> bool {
    if primo.x == centro.x {
        (primo.y > centro.y) == (terzo.y > centro.y)
    } else {
        (primo.x > centro.x) == (terzo.x > centro.x)
    }
}

fn poligono_con_punta(poligono: &geo::Polygon<f64>) -> bool {
    std::iter::once(poligono.exterior())
        .chain(poligono.interiors())
        .any(anello_con_punta)
}

/// Un triangolo coi tre vertici collineari, col segno **esatto**.
///
/// La validazione di `Triangle` in `geo` 0.33.1 confronta `robust::orient2d`
/// con zero: su coordinate estreme il determinante e' NaN e un triangolo
/// degenere passa. `RobustKernel` da' il segno esatto su ogni `f64` finito;
/// le coordinate non finite restano a `geo`.
fn triangolo_degenere(triangolo: &geo::Triangle<f64>) -> bool {
    use geo::algorithm::kernels::{Kernel, Orientation, RobustKernel};

    // Gli accessori rendono i campi come sono, senza riordinare: in
    // `geo-types` 0.7.19 `v1()` e' `self.0`, e cosi' gli altri due. Il
    // riordino antiorario lo fa solo `Triangle::new`, che qui non serve.
    let vertici = [triangolo.v1(), triangolo.v2(), triangolo.v3()];
    vertici
        .iter()
        .all(|vertice| vertice.x.is_finite() && vertice.y.is_finite())
        && RobustKernel::orient2d(vertici[0], vertici[1], vertici[2]) == Orientation::Collinear
}

impl AnelliSemplici for geo::Polygon<f64> {
    fn ha_un_anello_con_punta(&self) -> bool {
        poligono_con_punta(self)
    }
}

impl AnelliSemplici for geo::MultiPolygon<f64> {
    fn ha_un_anello_con_punta(&self) -> bool {
        self.iter().any(poligono_con_punta)
    }
}

impl AnelliSemplici for geo::GeometryCollection<f64> {
    fn ha_un_anello_con_punta(&self) -> bool {
        self.iter().any(AnelliSemplici::ha_un_anello_con_punta)
    }
}

impl AnelliSemplici for Geometry<f64> {
    fn ha_un_anello_con_punta(&self) -> bool {
        match self {
            Self::Polygon(poligono) => poligono_con_punta(poligono),
            Self::MultiPolygon(poligoni) => poligoni.ha_un_anello_con_punta(),
            Self::GeometryCollection(collezione) => collezione.ha_un_anello_con_punta(),
            Self::Triangle(triangolo) => triangolo_degenere(triangolo),
            // Un `Rect` degenere ha la propria validazione in `geo`, e
            // l'inviluppo di un punto e' un `Rect` degenere legittimo.
            Self::Rect(_)
            | Self::Point(_)
            | Self::Line(_)
            | Self::LineString(_)
            | Self::MultiPoint(_)
            | Self::MultiLineString(_) => false,
        }
    }
}

/// Un calcolo di `geo` che passa da `relate`, **dietro una barriera**.
///
/// Serve anche su geometrie valide: `relate` va in panico su un «topology
/// position conflict» con poligoni che `check_validation` accetta (trovati
/// dal fuzz target `wkt_operations` del progetto d'origine), e ci passano
/// `interior_point` e i predicati spaziali. Il lavoro contiene la sola chiamata a `geo`: e' una
/// [`barriera_di_dipendenza`](plenora_core::panic_policy::barriera_di_dipendenza).
///
/// # Errors
///
/// La forma del payload, mai il contenuto: il messaggio di `geo` porta le
/// coordinate che hanno provocato la contraddizione.
pub(crate) fn calcolo_protetto<T>(
    calcolo: impl FnOnce() -> T,
) -> std::result::Result<T, &'static str> {
    // Unwind safety: il calcolo legge geometrie in prestito immutabile, e un
    // risultato parziale non esce dalla barriera.
    plenora_core::panic_policy::barriera_di_dipendenza(std::panic::AssertUnwindSafe(calcolo))
        .map_err(|payload| plenora_core::panic_policy::forma_payload(&*payload))
}

#[cfg(test)]
mod prove_del_calcolo_protetto {
    use plenora_core::panic_policy::dentro_una_barriera_di_dipendenza;

    /// Il panico diventa la forma del payload, mai il contenuto; e durante il
    /// calcolo la barriera e' una barriera di dipendenza, riconoscibile da
    /// `dentro_una_barriera_di_dipendenza`.
    #[test]
    fn il_panico_diventa_la_forma_e_la_barriera_e_di_dipendenza() {
        let esito: Result<(), _> = super::calcolo_protetto(|| {
            assert!(dentro_una_barriera_di_dipendenza());
            std::panic::panic_any("coordinate 12 52 segrete".to_owned())
        });
        let forma = esito.expect_err("il panico deve diventare un errore");
        assert!(!forma.contains("segrete"), "contenuto pubblicato: {forma}");
        assert!(!dentro_una_barriera_di_dipendenza());
        assert_eq!(super::calcolo_protetto(|| 7).expect("nessun panico"), 7);
    }
}

/// Dimensione massima di un payload WKB: 64 MiB.
pub const MAX_WKB_BYTES: usize = 64 * 1024 * 1024;
/// Numero massimo di geometrie (radice e figli annidati) in un payload WKB.
pub const MAX_WKB_COMPONENTS: u64 = 100_000;
/// Profondita' massima di annidamento (multi-geometrie) di default.
pub const MAX_WKB_DEPTH: usize = 64;

/// Flag EWKB (estensione PostGIS): ordinata Z presente.
const EWKB_Z_FLAG: u32 = 0x8000_0000;
/// Flag EWKB (estensione PostGIS): ordinata M presente.
const EWKB_M_FLAG: u32 = 0x4000_0000;
/// Flag EWKB (estensione PostGIS): SRID presente. Mai ammesso: lo SRID non
/// e' preservabile dal validatore strutturale.
const EWKB_SRID_FLAG: u32 = 0x2000_0000;
/// Maschera del tipo base nella forma EWKB (16 bit bassi).
const EWKB_TYPE_MASK: u32 = 0x0000_FFFF;
/// Bit alti riservati nella forma EWKB: se attivi, il type code non e' ne'
/// ISO ne' EWKB valido e va rifiutato.
const EWKB_RESERVED_MASK: u32 = 0x1FFF_0000;

/// Decodifica una geometria dal canone GeoArrow-WKB.
///
/// Una sola passata: il decoder validante
/// ([`wkb_decoder::decode_validated`]) esegue validazione strutturale e
/// costruzione nella stessa camminata sui byte; poi la validazione OGC
/// della geometria risultante.
///
/// # Errors
///
/// `PlenoraError::InvalidPlan` se il payload viola il contratto WKB (struttura
/// non valida, coordinate NaN o infinite, geometria OGC non valida) o se il
/// decode fallisce; `PlenoraError::Unsupported` se il payload porta
/// dimensioni Z/M o SRID non preservabili nel protocollo 2D;
/// `PlenoraError::Internal` se la validazione OGC non conclude (panico di
/// `geo` dentro la barriera).
pub fn geometry_from_wkb(payload: &[u8]) -> Result<Geometry<f64>, PlenoraError> {
    let geometry = wkb_decoder::decode_validated(payload)?;
    valida_ogc(&geometry)?;
    Ok(geometry)
}

pub(crate) struct WkbCursor<'a> {
    payload: &'a [u8],
    offset: usize,
}

impl<'a> WkbCursor<'a> {
    const fn new(payload: &'a [u8]) -> Self {
        Self { payload, offset: 0 }
    }

    const fn remaining(&self) -> usize {
        self.payload.len().saturating_sub(self.offset)
    }

    fn read_u8(&mut self) -> Result<u8, PlenoraError> {
        let value = *self
            .payload
            .get(self.offset)
            .ok_or_else(|| invalid_wkb_structure("byte mancante"))?;
        self.offset += 1;
        Ok(value)
    }

    fn read_u32(&mut self, little_endian: bool) -> Result<u32, PlenoraError> {
        let bytes: [u8; 4] = *self
            .payload
            .get(self.offset..)
            .and_then(|tail| tail.first_chunk::<4>())
            .ok_or_else(|| invalid_wkb_structure("uint32 troncato"))?;
        self.offset += 4;
        Ok(if little_endian {
            u32::from_le_bytes(bytes)
        } else {
            u32::from_be_bytes(bytes)
        })
    }

    fn read_f64(&mut self, little_endian: bool) -> Result<f64, PlenoraError> {
        let bytes: [u8; 8] = *self
            .payload
            .get(self.offset..)
            .and_then(|tail| tail.first_chunk::<8>())
            .ok_or_else(|| invalid_wkb_structure("float64 troncato"))?;
        self.offset += 8;
        Ok(if little_endian {
            f64::from_le_bytes(bytes)
        } else {
            f64::from_be_bytes(bytes)
        })
    }

    /// Salta `bytes` byte senza leggerli: usato per le ordinate extra (Z/M),
    /// che il validatore stride-aware non decodifica ne' reinterpreta.
    fn skip(&mut self, bytes: usize) -> Result<(), PlenoraError> {
        if self.remaining() < bytes {
            return Err(invalid_wkb_structure("coordinata troncata"));
        }
        self.offset += bytes;
        Ok(())
    }

    /// Legge una coordinata con lo `stride` dichiarato (byte per coordinata
    /// interleaved, vedi [`GeometryDimensions::coordinate_stride`]): X e Y
    /// sono decodificate e validate (NaN e infiniti vietati), le ordinate
    /// extra (Z/M) sono saltate via stride e mai lette.
    fn read_coordinate(
        &mut self,
        little_endian: bool,
        stride: usize,
    ) -> Result<(f64, f64), PlenoraError> {
        let x = self.read_f64(little_endian)?;
        let y = self.read_f64(little_endian)?;
        if !x.is_finite() || !y.is_finite() {
            return Err(non_finite_coordinate());
        }
        self.skip(stride - 16)?;
        Ok((x, y))
    }
}

pub(crate) fn checked_count(
    value: u32,
    remaining: usize,
    minimum_item_bytes: usize,
) -> Result<usize, PlenoraError> {
    let count = value as usize;
    if count > remaining / minimum_item_bytes.max(1) {
        return Err(invalid_wkb_structure(
            "conteggio elementi oltre i byte disponibili",
        ));
    }
    Ok(count)
}

/// Interpreta il type code WKB e ne deriva tipo base e stride coordinata,
/// verificando la coerenza con la dimensionalita' attesa.
///
/// Forme ammesse: ISO `tipo + 1000 * dimensione` con dimensione 0..=3, ed
/// EWKB con flag Z ([`EWKB_Z_FLAG`]) e/o M ([`EWKB_M_FLAG`]) e tipo base nei
/// 16 bit bassi, senza altri bit alti. Il flag SRID EWKB e' sempre
/// rifiutato (lo SRID non e' preservabile); i codici dimensione ISO oltre 3
/// danno [`unsupported_wkb_dimension`]. Un EWKB senza flag Z/M e senza SRID
/// e' byte-identico a WKB ISO e passa come `xy`: il gate sta sui type code,
/// non sulla chiave `encoding` del metadato `geo`.
///
/// Con `Xy` ogni marcatore dimensionale da' [`unsupported_wkb_dimension`];
/// con `Unknown` (mai trattata come `Xy`) lo stride si deriva dal type
/// code, geometria per geometria; con `Xyz`/`Xym`/`Xyzm` una divergenza da'
/// [`wkb_dimension_mismatch`], mai un passthrough.
pub(crate) fn parse_wkb_type_code(
    raw_type: u32,
    expected: GeometryDimensions,
) -> Result<(u32, usize), PlenoraError> {
    if raw_type & EWKB_SRID_FLAG != 0 {
        return Err(unsupported_wkb_dimension());
    }
    let (geometry_type, actual) = if raw_type & (EWKB_Z_FLAG | EWKB_M_FLAG) != 0 {
        if raw_type & EWKB_RESERVED_MASK != 0 {
            return Err(invalid_wkb_structure("tipo geometria non supportato"));
        }
        let has_z = raw_type & EWKB_Z_FLAG != 0;
        let has_m = raw_type & EWKB_M_FLAG != 0;
        // Almeno uno dei due flag e' attivo (guardia sopra): XY non occorre.
        let dimensions = match (has_z, has_m) {
            (true, false) => GeometryDimensions::Xyz,
            (false, true) => GeometryDimensions::Xym,
            (true, true) => GeometryDimensions::Xyzm,
            (false, false) => GeometryDimensions::Xy,
        };
        (raw_type & EWKB_TYPE_MASK, dimensions)
    } else {
        let dimensions = match raw_type / 1000 {
            0 => GeometryDimensions::Xy,
            1 => GeometryDimensions::Xyz,
            2 => GeometryDimensions::Xym,
            3 => GeometryDimensions::Xyzm,
            _ => return Err(unsupported_wkb_dimension()),
        };
        (raw_type % 1000, dimensions)
    };
    let coherent = match expected {
        GeometryDimensions::Unknown => actual,
        GeometryDimensions::Xy if actual != GeometryDimensions::Xy => {
            return Err(unsupported_wkb_dimension());
        }
        expected if expected == actual => actual,
        _ => return Err(wkb_dimension_mismatch()),
    };
    // `coherent` non e' mai `Unknown` (o e' `actual`, o e' `expected`
    // uguale ad `actual`): lo stride garantito esiste sempre.
    let stride = coherent
        .coordinate_stride()
        .ok_or_else(|| invalid_wkb_structure("tipo geometria non supportato"))?;
    Ok((geometry_type, stride))
}

#[derive(Clone, Copy)]
enum EmbeddedSridPolicy {
    Reject,
    Match(Option<u32>),
}

fn type_code_without_embedded_srid(
    cursor: &mut WkbCursor<'_>,
    little_endian: bool,
    raw_type: u32,
    policy: EmbeddedSridPolicy,
) -> Result<u32, PlenoraError> {
    if raw_type & EWKB_SRID_FLAG == 0 {
        return Ok(raw_type);
    }
    let EmbeddedSridPolicy::Match(expected_srid) = policy else {
        return Err(unsupported_wkb_dimension());
    };
    let embedded_srid = cursor.read_u32(little_endian)?;
    let expected_srid = expected_srid.ok_or_else(|| {
        PlenoraError::Crs("SRID EWKB embedded senza autorita' CRS governata".to_owned())
    })?;
    if embedded_srid != expected_srid {
        return Err(PlenoraError::Crs(
            "SRID EWKB embedded incoerente con lo SRID dichiarato".to_owned(),
        ));
    }
    Ok(raw_type & !EWKB_SRID_FLAG)
}

/// Validatore strutturale stride-aware: annidamento, conteggi, bound
/// sui byte e finitezza di X/Y sono verificati con lo stride della
/// dimensionalita' attesa (o derivato dal type code, se `Unknown`).
///
/// Le ordinate extra (Z/M) sono saltate via stride, mai lette ne' validate:
/// un NaN in Z/M passa, perche' il kernel non elabora quelle ordinate. La
/// chiusura degli anelli si valuta sulle sole X/Y.
fn validate_wkb_geometry_with_dimensions(
    cursor: &mut WkbCursor<'_>,
    depth: usize,
    max_depth: usize,
    components: &mut u64,
    expected: GeometryDimensions,
    srid_policy: EmbeddedSridPolicy,
) -> Result<u32, PlenoraError> {
    if depth > max_depth {
        return Err(invalid_wkb_structure(
            "annidamento geometrie oltre il limite",
        ));
    }
    *components = components
        .checked_add(1)
        .ok_or_else(|| invalid_wkb_structure("conteggio componenti oltre il limite"))?;
    if *components > MAX_WKB_COMPONENTS {
        return Err(invalid_wkb_structure(
            "conteggio componenti oltre il limite",
        ));
    }
    let byte_order = cursor.read_u8()?;
    let little_endian = match byte_order {
        0 => false,
        1 => true,
        _ => return Err(invalid_wkb_structure("byte order non valido")),
    };
    let raw_type = cursor.read_u32(little_endian)?;
    let raw_type = type_code_without_embedded_srid(cursor, little_endian, raw_type, srid_policy)?;
    let (geometry_type, stride) = parse_wkb_type_code(raw_type, expected)?;
    match geometry_type {
        1 => {
            cursor.read_coordinate(little_endian, stride)?;
        }
        2 => {
            let count = cursor.read_u32(little_endian)?;
            let count = checked_count(count, cursor.remaining(), stride)?;
            if count == 1 {
                return Err(invalid_wkb_structure(
                    "LineString deve essere vuota o avere almeno due coordinate",
                ));
            }
            for _ in 0..count {
                cursor.read_coordinate(little_endian, stride)?;
            }
        }
        3 => {
            let rings = cursor.read_u32(little_endian)?;
            let rings = checked_count(rings, cursor.remaining(), 4)?;
            for _ in 0..rings {
                let count = cursor.read_u32(little_endian)?;
                let count = checked_count(count, cursor.remaining(), stride)?;
                if count < 4 {
                    return Err(invalid_wkb_structure(
                        "anello poligonale con meno di quattro coordinate",
                    ));
                }
                let first = cursor.read_coordinate(little_endian, stride)?;
                let mut last = first;
                for _ in 1..count {
                    last = cursor.read_coordinate(little_endian, stride)?;
                }
                if first != last {
                    return Err(invalid_wkb_structure("anello poligonale non chiuso"));
                }
            }
        }
        4..=7 => {
            let children = cursor.read_u32(little_endian)?;
            let children = checked_count(children, cursor.remaining(), 5)?;
            for _ in 0..children {
                let child_type = validate_wkb_geometry_with_dimensions(
                    cursor,
                    depth + 1,
                    max_depth,
                    components,
                    expected,
                    srid_policy,
                )?;
                let valid_child = match geometry_type {
                    4 => child_type == 1,
                    5 => child_type == 2,
                    6 => child_type == 3,
                    7 => true,
                    _ => {
                        return Err(invalid_wkb_structure("tipo geometria non supportato"));
                    }
                };
                if !valid_child {
                    return Err(invalid_wkb_structure(
                        "tipo figlio incompatibile con multi-geometria",
                    ));
                }
            }
        }
        _ => {
            return Err(invalid_wkb_structure("tipo geometria non supportato"));
        }
    }
    Ok(geometry_type)
}

/// Byte di un WKB scritto in esadecimale; `None` se la stringa non lo e'.
///
/// Lavora sui byte e non affetta mai la stringa: una lunghezza in byte pari
/// non garantisce confini di carattere (`"a\u{e9}b"`), e affettare fuori da
/// un confine e' un panic che il gate anti-panic di clippy non vede.
/// L'input arriva dalla configurazione di un piano, quindi da fuori.
///
/// `None` per stringa vuota, di lunghezza dispari, o con un byte che non e'
/// una cifra esadecimale ASCII. Il chiamante lo traduce nel proprio errore.
#[must_use]
pub fn wkb_hex_to_bytes(hex: &str) -> Option<Vec<u8>> {
    const fn cifra(byte: u8) -> Option<u8> {
        match byte {
            b'0'..=b'9' => Some(byte - b'0'),
            b'a'..=b'f' => Some(byte - b'a' + 10),
            b'A'..=b'F' => Some(byte - b'A' + 10),
            _ => None,
        }
    }

    let cifre = hex.as_bytes();
    if cifre.is_empty() || !cifre.len().is_multiple_of(2) {
        return None;
    }
    let mut bytes = Vec::with_capacity(cifre.len() / 2);
    // `as_chunks::<2>()` invece di `chunks_exact(2)`: il tipo diventa
    // `[u8; 2]`, quindi l'indicizzazione e' totale per costruzione invece che
    // per convenzione. Il resto (un'eventuale cifra spaiata) e' gia' escluso
    // dal controllo di parita' qui sopra.
    for coppia in cifre.as_chunks::<2>().0 {
        let (Some(alto), Some(basso)) = (cifra(coppia[0]), cifra(coppia[1])) else {
            return None;
        };
        bytes.push((alto << 4) | basso);
    }
    Some(bytes)
}

/// Valida un payload WKB contro il contratto strutturale.
///
/// Verifica limiti di byte, annidamento, conteggi e finitezza delle
/// coordinate, con dimensionalita' attesa `Xy` e profondita' massima
/// [`MAX_WKB_DEPTH`].
///
/// # Errors
///
/// `PlenoraError::InvalidPlan` se la struttura WKB non e' valida (byte o
/// conteggi oltre i limiti, anelli non chiusi, coordinate NaN o infinite,
/// byte residui); `PlenoraError::Unsupported` se il payload porta
/// dimensioni Z/M o SRID non preservabili nel protocollo 2D.
pub fn validate_wkb_contract(payload: &[u8]) -> Result<(), PlenoraError> {
    validate_wkb_contract_with_depth(payload, MAX_WKB_DEPTH)
}

/// Variante con profondita' di annidamento configurabile.
///
/// Il limite e' quello di `max_geometry_depth` dei `Limits` del piano per
/// chi lo passa (oggi nessun chiamante del workspace); il default di
/// [`validate_wkb_contract`] resta [`MAX_WKB_DEPTH`].
///
/// # Errors
///
/// Come [`validate_wkb_contract`]; in piu' `PlenoraError::InvalidPlan` se
/// l'annidamento delle geometrie supera `max_depth`.
pub fn validate_wkb_contract_with_depth(
    payload: &[u8],
    max_depth: usize,
) -> Result<(), PlenoraError> {
    validate_wkb_contract_for_dimensions_with_depth(payload, GeometryDimensions::Xy, max_depth)
}

/// Variante stride-aware, con dimensionalita' attesa esplicita.
///
/// La validazione strutturale usa lo stride della dimensionalita'
/// dichiarata e rifiuta ogni type code incoerente con essa (errore
/// dedicato). Con [`GeometryDimensions::Unknown`] i byte sono preservati e
/// la dimensionalita' e' derivata dal type code di ogni geometria; il flag
/// SRID EWKB resta rifiutato in ogni caso. Le ordinate extra (Z/M) non sono
/// lette ne' validate.
///
/// In produzione nessun chiamante passa la dimensionalita' del contratto di
/// colonna: usano tutti [`validate_wkb_contract`], cioe' `Xy`.
///
/// # Errors
///
/// `PlenoraError::InvalidPlan` se la struttura WKB non e' valida o se il type
/// code e' incoerente con la dimensionalita' attesa;
/// `PlenoraError::Unsupported` se il payload porta dimensioni Z/M non
/// dichiarate o il flag SRID EWKB.
pub fn validate_wkb_contract_for_dimensions(
    payload: &[u8],
    dimensions: GeometryDimensions,
) -> Result<(), PlenoraError> {
    validate_wkb_contract_for_dimensions_with_depth(payload, dimensions, MAX_WKB_DEPTH)
}

/// Come [`validate_wkb_contract_for_dimensions`], con profondita' di
/// annidamento configurabile (come [`validate_wkb_contract_with_depth`]).
///
/// # Errors
///
/// Come [`validate_wkb_contract_for_dimensions`]; in piu'
/// `PlenoraError::InvalidPlan` se l'annidamento delle geometrie supera
/// `max_depth`.
pub fn validate_wkb_contract_for_dimensions_with_depth(
    payload: &[u8],
    dimensions: GeometryDimensions,
    max_depth: usize,
) -> Result<(), PlenoraError> {
    if payload.len() > MAX_WKB_BYTES {
        return Err(invalid_wkb_structure("WKB oltre il limite di 64 MiB"));
    }
    let mut cursor = WkbCursor::new(payload);
    let mut components = 0_u64;
    validate_wkb_geometry_with_dimensions(
        &mut cursor,
        0,
        max_depth,
        &mut components,
        dimensions,
        EmbeddedSridPolicy::Reject,
    )?;
    if cursor.remaining() != 0 {
        return Err(invalid_wkb_structure("byte residui dopo la geometria"));
    }
    Ok(())
}

/// Valida WKB/EWKB al confine di trasporto senza riscrivere le celle (oggi
/// senza chiamanti nel workspace).
///
/// A differenza dei decoder dei kernel geometrici, questo gate ammette il
/// flag SRID soltanto per encoding EWKB dichiarato e verifica ogni SRID
/// embedded contro l'autorita' governata. I decoder elaboranti rifiutano lo
/// SRID embedded, che non possono preservare.
///
/// # Errors
///
/// `PlenoraError::Crs` se uno SRID embedded diverge da `expected_srid`; le
/// altre varianti coincidono con
/// [`validate_wkb_contract_for_dimensions_with_depth`].
pub fn validate_wkb_transport_for_dimensions_with_depth(
    payload: &[u8],
    dimensions: GeometryDimensions,
    encoding: GeometryEncoding,
    expected_srid: Option<u32>,
    max_depth: usize,
) -> Result<(), PlenoraError> {
    if payload.len() > MAX_WKB_BYTES {
        return Err(invalid_wkb_structure("WKB oltre il limite di 64 MiB"));
    }
    let mut cursor = WkbCursor::new(payload);
    let mut components = 0_u64;
    let srid_policy = match encoding {
        GeometryEncoding::Wkb => EmbeddedSridPolicy::Reject,
        GeometryEncoding::Ewkb => EmbeddedSridPolicy::Match(expected_srid),
    };
    validate_wkb_geometry_with_dimensions(
        &mut cursor,
        0,
        max_depth,
        &mut components,
        dimensions,
        srid_policy,
    )?;
    if cursor.remaining() != 0 {
        return Err(invalid_wkb_structure("byte residui dopo la geometria"));
    }
    Ok(())
}

// float_cmp: i confronti esatti min/max individuano gli envelope degeneri
// (larghezza o altezza nulle) per costruzione — min e max provengono dalle
// stesse coordinate, quindi l'uguaglianza esatta e' il criterio voluto e un
// margine epsilon cambierebbe la geometria prodotta.
#[allow(clippy::float_cmp)]
fn envelope(geometry: &Geometry<f64>) -> Result<Geometry<f64>, PlenoraError> {
    let rect = geometry
        .bounding_rect()
        .ok_or_else(|| empty_geometry("envelope"))?;
    let min = rect.min();
    let max = rect.max();

    if min.x == max.x && min.y == max.y {
        return Ok(Geometry::Point(Point::new(min.x, min.y)));
    }
    if min.x == max.x || min.y == max.y {
        return Ok(Geometry::LineString(LineString::from(vec![min, max])));
    }
    Ok(Geometry::Polygon(rect.to_polygon()))
}

/// Un calcolo di `geo` di una trasformazione, dietro la barriera: il panico
/// diventa `Internal` con la sola forma del payload.
fn trasformazione_protetta<T>(
    operazione: &'static str,
    calcolo: impl FnOnce() -> T,
) -> Result<T, PlenoraError> {
    calcolo_protetto(calcolo).map_err(|forma| {
        PlenoraError::Internal(format!(
            "{operazione}: calcolo di geo non concluso: {forma}"
        ))
    })
}

fn robust_convex_hull(geometry: &Geometry<f64>) -> Result<Geometry<f64>, PlenoraError> {
    // Gli orientamenti di `geo` possono traboccare con coordinate finite ma
    // vicine ai limiti di `f64`. Si divide sempre per il modulo massimo (non
    // solo vicino ai limiti): la scala uniforme conserva l'inviluppo e tiene
    // ogni determinante in un intervallo sicuro. Il ritorno moltiplica per
    // la stessa scala: qualche ulp per coordinata, nessuno se la scala e'
    // una potenza di 2.
    let scale = geometry.coords_iter().fold(0.0_f64, |maximum, coordinate| {
        maximum.max(coordinate.x.abs()).max(coordinate.y.abs())
    });
    if scale == 0.0 {
        return trasformazione_protetta("convex_hull", || {
            Geometry::Polygon(geometry.convex_hull())
        });
    }
    let normalized = geometry.map_coords(|coordinate| Coord {
        x: coordinate.x / scale,
        y: coordinate.y / scale,
    });
    let hull = trasformazione_protetta("convex_hull", || normalized.convex_hull())?;
    Ok(Geometry::Polygon(hull.map_coords(|coordinate| Coord {
        x: coordinate.x * scale,
        y: coordinate.y * scale,
    })))
}

/// Applica l'operazione (`Operation::Centroid`, `ConvexHull`, `Envelope`)
/// a una geometria gia' decodificata.
///
/// E' il kernel di `geo.centroid`, `geo.convex_hull` e `geo.envelope`; la
/// geometria si valida in ingresso e in uscita.
///
/// # Errors
///
/// - `PlenoraError::InvalidPlan` se la geometria in ingresso o quella
///   prodotta non supera la validazione OGC (un inviluppo convesso degenere,
///   da un punto, un segmento o punti collineari, e' un poligono non
///   valido), o se l'operazione non e' definita su una geometria vuota
///   (centroide o envelope);
/// - `PlenoraError::Internal` se la validazione OGC o il calcolo di `geo`
///   vanno in panico dentro la barriera: il messaggio porta la sola forma
///   del payload.
pub fn transform_geometry(
    operation: Operation,
    geometry: &Geometry<f64>,
) -> Result<Geometry<f64>, PlenoraError> {
    valida_ogc(geometry)?;
    transform_geometry_validated(operation, geometry)
}

fn transform_geometry_validated(
    operation: Operation,
    geometry: &Geometry<f64>,
) -> Result<Geometry<f64>, PlenoraError> {
    let output = match operation {
        Operation::Centroid => trasformazione_protetta("centroid", || geometry.centroid())?
            .map(Geometry::Point)
            .ok_or_else(|| empty_geometry("centroid")),
        Operation::ConvexHull => robust_convex_hull(geometry),
        Operation::Envelope => envelope(geometry),
    }?;
    valida_ogc(&output)?;
    Ok(output)
}

/// Applica l'operazione direttamente su un payload WKB.
///
/// Decode ([`geometry_from_wkb`], con la validazione OGC), trasforma (come
/// [`transform_geometry`]), ri-encode nel canone XY e ri-validazione del
/// risultato contro il contratto ([`validate_wkb_contract`]).
///
/// # Errors
///
/// `PlenoraError::InvalidPlan` per gli errori di decode, trasformazione,
/// serializzazione WKB o validazione del risultato;
/// `PlenoraError::Unsupported` se il payload in ingresso porta dimensioni
/// Z/M o SRID non preservabili nel protocollo 2D;
/// `PlenoraError::Internal` come in [`transform_geometry`].
pub fn transform_wkb(operation: Operation, payload: &[u8]) -> Result<Vec<u8>, PlenoraError> {
    let geometry = geometry_from_wkb(payload)?;
    let transformed = transform_geometry_validated(operation, &geometry)?;
    let output = transformed
        .to_wkb(CoordDimensions::xy())
        .map_err(|error| wkb_serialization(error.to_string()))?;
    validate_wkb_contract(&output)?;
    Ok(output)
}

/// Validazione OGC di una geometria gia' decodificata.
///
/// La STESSA chiamata e lo STESSO messaggio del controllo in coda a
/// [`geometry_from_wkb`], per chi ha una geometria in memoria e vuole il
/// verdetto che il decode darebbe.
///
/// # Errors
///
/// `PlenoraError::InvalidPlan` se la geometria non supera la validazione
/// OGC; `PlenoraError::Internal` se la validazione non conclude (panico di
/// `geo` dentro la barriera: nessuno ha dimostrato che la geometria sia
/// invalida).
pub fn check_geometry_valid(geometry: &Geometry<f64>) -> Result<(), PlenoraError> {
    valida_ogc(geometry)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{
        multipolygon_wkb_le, point_wkb_le, polygon_wkb_le, push_coordinate, push_count,
        push_header, rect,
    };
    use geo::{line_string, polygon, Area};
    use proptest::prelude::*;

    fn round_trip(operation: Operation, geometry: &Geometry<f64>) -> Geometry<f64> {
        let payload = geometry
            .to_wkb(CoordDimensions::xy())
            .expect("encode fixture");
        geometry_from_wkb(&transform_wkb(operation, &payload).expect("transform"))
            .expect("decode result")
    }

    /// La condizione d'errore e' identificata dal messaggio.
    fn is_contract_error(result: &Result<Geometry<f64>, PlenoraError>, message: &str) -> bool {
        matches!(result, Err(PlenoraError::InvalidPlan(reason)) if reason == message)
    }

    /// Una lunghezza in byte pari non garantisce confini di carattere
    /// (`"a\u{e9}b"`): la decodifica di un esadecimale da config non va mai in
    /// panic.
    #[test]
    fn il_wkb_esadecimale_non_va_mai_in_panic_su_input_ostile() {
        // Lunghezza pari in byte, confine di carattere no.
        assert_eq!(wkb_hex_to_bytes("a\u{e9}b"), None, "il caso del crash");

        // La stessa forma con altri caratteri multi-byte, di lunghezza pari.
        for ostile in [
            "\u{e9}\u{e9}",       // 4 byte, nessun indice pari e' un confine oltre lo 0
            "0\u{e9}0",           // 4 byte, indice 2 dentro la codifica
            "\u{1F642}",          // 4 byte, un solo carattere
            "\u{1F642}\u{1F642}", // 8 byte
            "ab\u{e9}",           // 4 byte, il taglio cade dentro l'ultimo
        ] {
            assert_eq!(wkb_hex_to_bytes(ostile), None, "input ostile: {ostile:?}");
        }

        // Lunghezza dispari e cifre non esadecimali: errore, non panic.
        for invalido in ["", "0", "abc", "zz", "0g", "00 ", " 00", "-1", "0\u{0}"] {
            assert_eq!(wkb_hex_to_bytes(invalido), None, "invalido: {invalido:?}");
        }

        // Gli esadecimali VALIDI continuano a decodificare, maiuscoli inclusi.
        assert_eq!(wkb_hex_to_bytes("00"), Some(vec![0x00]));
        assert_eq!(wkb_hex_to_bytes("ff"), Some(vec![0xff]));
        assert_eq!(wkb_hex_to_bytes("FF"), Some(vec![0xff]));
        assert_eq!(wkb_hex_to_bytes("0aFf"), Some(vec![0x0a, 0xff]));
        assert_eq!(
            wkb_hex_to_bytes("0101000000000000000000f03f0000000000000040"),
            Some(vec![
                0x01, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xf0, 0x3f, 0x00,
                0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x40,
            ]),
            "un POINT(1 2) reale attraversa la decodifica"
        );
    }

    #[test]
    fn geometry_type_name_covers_every_geo_variant() {
        let variants = [
            (Geometry::Point(Point::new(0.0, 0.0)), "Point"),
            (
                Geometry::Line(geo::Line::new((0.0, 0.0), (1.0, 1.0))),
                "Line",
            ),
            (
                Geometry::LineString(LineString::new(Vec::new())),
                "LineString",
            ),
            (
                Geometry::Polygon(geo::Polygon::new(LineString::new(Vec::new()), Vec::new())),
                "Polygon",
            ),
            (
                Geometry::MultiPoint(geo::MultiPoint::new(Vec::new())),
                "MultiPoint",
            ),
            (
                Geometry::MultiLineString(geo::MultiLineString::new(Vec::new())),
                "MultiLineString",
            ),
            (
                Geometry::MultiPolygon(geo::MultiPolygon::new(Vec::new())),
                "MultiPolygon",
            ),
            (
                Geometry::GeometryCollection(Vec::<Geometry<f64>>::new().into()),
                "GeometryCollection",
            ),
            (
                Geometry::Rect(geo::Rect::new((0.0, 0.0), (1.0, 1.0))),
                "Rect",
            ),
            (
                Geometry::Triangle(geo::Triangle::new(
                    geo::Coord { x: 0.0, y: 0.0 },
                    geo::Coord { x: 1.0, y: 0.0 },
                    geo::Coord { x: 0.0, y: 1.0 },
                )),
                "Triangle",
            ),
        ];

        for (geometry, expected) in variants {
            assert_eq!(geometry_type_name(&geometry), expected);
        }
    }

    #[test]
    fn centroid_transforms_polygon_to_expected_point() {
        let input = rect(0.0, 0.0, 4.0, 2.0);
        let result = round_trip(Operation::Centroid, &input);
        assert_eq!(result, Geometry::Point(Point::new(2.0, 1.0)));
    }

    #[test]
    fn envelope_preserves_degenerate_dimension() {
        let point = Geometry::Point(Point::new(2.0, 3.0));
        assert_eq!(round_trip(Operation::Envelope, &point), point);

        let line = Geometry::LineString(line_string![(x: 1.0, y: 4.0), (x: 5.0, y: 4.0)]);
        assert_eq!(round_trip(Operation::Envelope, &line), line);
    }

    #[test]
    fn convex_hull_contains_input_area() {
        let input = Geometry::LineString(line_string![
            (x: 0.0, y: 0.0),
            (x: 4.0, y: 0.0),
            (x: 2.0, y: 3.0),
            (x: 2.0, y: 1.0),
        ]);
        let result = round_trip(Operation::ConvexHull, &input);
        assert!(result.unsigned_area() > 0.0);
    }

    #[test]
    fn convex_hull_normalizes_extreme_finite_coordinates_without_panicking() {
        let input = Geometry::LineString(LineString::from(vec![
            (-3.477_300_121_932_381e-164, 2.781_342_323_781_663e-309),
            (1.344_974_619_049_452e-284, 6.354_280_840_450_530_5e-183),
            (2.639_614_224_254_873e-309, 3.236_069_361_538_085e-111),
            (-5.488_802_840_312_24e303, -6.971_241_357_778_827e182),
            (-5.486_124_068_793_689e303, 7.064_166_183_585_296e-304),
        ]));
        let hull = transform_geometry(Operation::ConvexHull, &input).unwrap();
        assert!(hull.validazione_protetta().is_ok());
        assert!(hull
            .coords_iter()
            .all(|coordinate| coordinate.x.is_finite() && coordinate.y.is_finite()));
        let payload = input.to_wkb(CoordDimensions::xy()).unwrap();
        assert!(transform_wkb(Operation::ConvexHull, &payload).is_ok());
    }

    #[test]
    fn rejects_non_finite_and_dimensional_wkb() {
        let nan_point = point_wkb_le(f64::NAN, 1.0);
        assert!(is_contract_error(
            &geometry_from_wkb(&nan_point),
            "WKB contiene coordinate NaN o infinite"
        ));

        let mut z_point = Vec::new();
        push_header(&mut z_point, 1001);
        push_coordinate(&mut z_point, 1.0, 2.0, &[3.0]);
        assert!(matches!(
            geometry_from_wkb(&z_point),
            Err(PlenoraError::Unsupported(message))
            if message == "WKB contiene dimensioni Z/M o SRID non preservabili nel protocollo 2D"
        ));
    }

    #[test]
    fn rejects_dimensional_wkb_hidden_in_collection() {
        let mut collection = Vec::new();
        push_header(&mut collection, 7);
        push_count(&mut collection, 1);
        push_header(&mut collection, 1001);
        push_coordinate(&mut collection, 1.0, 2.0, &[3.0]);
        assert!(matches!(
            geometry_from_wkb(&collection),
            Err(PlenoraError::Unsupported(message))
            if message == "WKB contiene dimensioni Z/M o SRID non preservabili nel protocollo 2D"
        ));
    }

    #[test]
    fn rejects_unclosed_or_too_short_polygon_rings() {
        let polygon = polygon_wkb_le(&[(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)]);
        assert!(is_contract_error(
            &geometry_from_wkb(&polygon),
            "struttura WKB non valida: anello poligonale non chiuso"
        ));

        let short = polygon_wkb_le(&[(0.0, 0.0); 3]);
        assert!(is_contract_error(
            &geometry_from_wkb(&short),
            "struttura WKB non valida: anello poligonale con meno di quattro coordinate"
        ));
    }

    /// La validazione OGC legge il testo di `geo` per classificare, senza
    /// pubblicarlo (errori senza dati, AGENTS.md); il canary passa
    /// dal confine pubblico reale (`geometry_from_wkb`).
    ///
    /// Qui `geo` interpola un **indice posizionale** (`"polygons at indices I
    /// e J overlap"`, `algorithm/validation/multi_polygon.rs` vendorizzato),
    /// un fatto sui dati di chi chiama quanto una coordinata. Le stringhe
    /// fisse di [`RagioneNonValida`] non possono interpolare alcun valore.
    #[test]
    fn ogc_validation_classifies_overlap_without_leaking_the_member_index() {
        // Due quadrati sovrapposti come membri 0 e 1 di una MultiPolygon:
        // "indices 0 and 1" e' la coppia che il testo grezzo di `geo`
        // interpolerebbe.
        let quadrato_a =
            polygon_wkb_le(&[(0.0, 0.0), (2.0, 0.0), (2.0, 2.0), (0.0, 2.0), (0.0, 0.0)]);
        // (1,1)-(3,3), sovrapposto ad A
        let quadrato_b =
            polygon_wkb_le(&[(1.0, 1.0), (3.0, 1.0), (3.0, 3.0), (1.0, 3.0), (1.0, 1.0)]);
        let multipoly = multipolygon_wkb_le(&[quadrato_a, quadrato_b]);

        let errore = geometry_from_wkb(&multipoly)
            .expect_err("due poligoni sovrapposti in una MultiPolygon non sono validi")
            .to_string();
        // Due asserzioni distinte: l'assenza del dato E la presenza della
        // causa classificata, non una sola delle due.
        assert!(
            !errore.contains("indices") && !errore.contains("0 and 1") && !errore.contains("0 e 1"),
            "l'indice del membro sovrapposto non deve attraversare il confine pubblico: {errore}"
        );
        assert_eq!(
            errore,
            "contract violation: geometria OGC non valida: poligoni sovrapposti"
        );
    }

    #[test]
    fn wkb_validator_covers_endianness_truncation_counts_and_trailing_bytes() {
        let mut big_endian_point = vec![0_u8];
        big_endian_point.extend_from_slice(&1_u32.to_be_bytes());
        big_endian_point.extend_from_slice(&2.0_f64.to_be_bytes());
        big_endian_point.extend_from_slice(&3.0_f64.to_be_bytes());
        assert_eq!(
            geometry_from_wkb(&big_endian_point).unwrap(),
            Geometry::Point(Point::new(2.0, 3.0))
        );

        for malformed in [
            vec![],
            vec![2],
            vec![1, 1, 0],
            vec![1, 1, 0, 0, 0, 0],
            vec![1, 2, 0, 0, 0, 1, 0, 0, 0],
            vec![1, 2, 0, 0, 0, 2, 0, 0, 0],
        ] {
            assert!(geometry_from_wkb(&malformed).is_err(), "{malformed:?}");
        }

        let mut trailing = big_endian_point.clone();
        trailing.push(0xff);
        assert!(is_contract_error(
            &geometry_from_wkb(&trailing),
            "struttura WKB non valida: byte residui dopo la geometria"
        ));

        let mut unsupported = Vec::new();
        push_header(&mut unsupported, 99);
        assert!(is_contract_error(
            &geometry_from_wkb(&unsupported),
            "struttura WKB non valida: tipo geometria non supportato"
        ));
    }

    #[test]
    fn wkb_validator_accepts_valid_multi_types_and_rejects_wrong_children() {
        for geometry in [
            Geometry::MultiPoint(geo::MultiPoint::new(vec![Point::new(1.0, 2.0)])),
            Geometry::MultiLineString(geo::MultiLineString::new(vec![line_string![
                (x: 0.0, y: 0.0), (x: 1.0, y: 1.0)
            ]])),
            Geometry::MultiPolygon(geo::MultiPolygon::new(vec![polygon![
                (x: 0.0, y: 0.0), (x: 1.0, y: 0.0), (x: 0.0, y: 1.0), (x: 0.0, y: 0.0)
            ]])),
            Geometry::GeometryCollection(geo::GeometryCollection(vec![Geometry::Point(
                Point::new(1.0, 2.0),
            )])),
        ] {
            let payload = geometry.to_wkb(CoordDimensions::xy()).unwrap();
            assert_eq!(geometry_from_wkb(&payload).unwrap(), geometry);
        }

        for parent_type in [4_u32, 5, 6] {
            let mut payload = Vec::new();
            push_header(&mut payload, parent_type);
            push_count(&mut payload, 1);
            let wrong_child = if parent_type == 4 {
                Geometry::LineString(line_string![(x: 0.0, y: 0.0), (x: 1.0, y: 1.0)])
            } else {
                Geometry::Point(Point::new(0.0, 0.0))
            };
            payload.extend_from_slice(&wrong_child.to_wkb(CoordDimensions::xy()).unwrap());
            assert!(is_contract_error(
                &geometry_from_wkb(&payload),
                "struttura WKB non valida: tipo figlio incompatibile con multi-geometria"
            ));
        }
    }

    #[test]
    fn validate_wkb_contract_with_depth_enforces_the_configurable_limit() {
        // GC(GC(Point)): il punto e' a profondita' 2.
        let nested = Geometry::GeometryCollection(geo::GeometryCollection(vec![
            Geometry::GeometryCollection(geo::GeometryCollection(vec![Geometry::Point(
                Point::new(1.0, 2.0),
            )])),
        ]));
        let payload = nested.to_wkb(CoordDimensions::xy()).unwrap();
        assert!(validate_wkb_contract_with_depth(&payload, MAX_WKB_DEPTH).is_ok());
        assert!(validate_wkb_contract_with_depth(&payload, 2).is_ok());
        assert!(matches!(
            validate_wkb_contract_with_depth(&payload, 1),
            Err(PlenoraError::InvalidPlan(reason))
                if reason == "struttura WKB non valida: annidamento geometrie oltre il limite"
        ));
        // Il default di validate_wkb_contract e' MAX_WKB_DEPTH.
        assert!(validate_wkb_contract(&payload).is_ok());
    }

    // ---- Test stride-aware (fixture in `test_support`) ----

    #[test]
    fn dimensional_wkb_validates_with_matching_expected_dimensions() {
        // Punto ISO ed EWKB nelle tre dimensionalita' estese.
        for (iso_type, ewkb_type, expected, extra) in [
            (
                1001_u32,
                0x8000_0001,
                GeometryDimensions::Xyz,
                &[7.0_f64][..],
            ),
            (2001, 0x4000_0001, GeometryDimensions::Xym, &[8.0][..]),
            (3001, 0xC000_0001, GeometryDimensions::Xyzm, &[7.0, 8.0][..]),
        ] {
            for raw_type in [iso_type, ewkb_type] {
                let mut payload = Vec::new();
                push_header(&mut payload, raw_type);
                push_coordinate(&mut payload, 1.0, 2.0, extra);
                assert!(
                    validate_wkb_contract_for_dimensions(&payload, expected).is_ok(),
                    "type code {raw_type:#x} con {expected}"
                );
                // Unknown: byte preservati, dimensionalita' dal type code.
                assert!(validate_wkb_contract_for_dimensions(
                    &payload,
                    GeometryDimensions::Unknown
                )
                .is_ok());
            }
        }
        // Big-endian ZM: lo stride non dipende dall'endianness.
        let mut big_endian = vec![0_u8];
        big_endian.extend_from_slice(&3001_u32.to_be_bytes());
        for value in [1.0_f64, 2.0, 3.0, 4.0] {
            big_endian.extend_from_slice(&value.to_be_bytes());
        }
        assert!(
            validate_wkb_contract_for_dimensions(&big_endian, GeometryDimensions::Xyzm).is_ok()
        );
    }

    #[test]
    fn dimensional_linestring_polygon_and_nested_collection_validate_with_stride() {
        // LineString ZM con due coordinate.
        let mut line = Vec::new();
        push_header(&mut line, 3002);
        push_count(&mut line, 2);
        push_coordinate(&mut line, 0.0, 0.0, &[5.0, 9.0]);
        push_coordinate(&mut line, 1.0, 1.0, &[6.0, 10.0]);
        assert!(validate_wkb_contract_for_dimensions(&line, GeometryDimensions::Xyzm).is_ok());

        // Poligono Z con anello esterno e un buco.
        let mut polygon = Vec::new();
        push_header(&mut polygon, 1003);
        push_count(&mut polygon, 2);
        push_count(&mut polygon, 4);
        for (x, y) in [(0.0, 0.0), (4.0, 0.0), (4.0, 4.0), (0.0, 0.0)] {
            push_coordinate(&mut polygon, x, y, &[1.0]);
        }
        push_count(&mut polygon, 4);
        for (x, y) in [(1.0, 1.0), (2.0, 1.0), (1.0, 2.0), (1.0, 1.0)] {
            push_coordinate(&mut polygon, x, y, &[2.0]);
        }
        assert!(validate_wkb_contract_for_dimensions(&polygon, GeometryDimensions::Xyz).is_ok());

        // Collection annidata ZM: GC(MultiPoint ZM, Point ZM).
        let mut collection = Vec::new();
        push_header(&mut collection, 3007);
        push_count(&mut collection, 2);
        push_header(&mut collection, 3004);
        push_count(&mut collection, 1);
        push_header(&mut collection, 3001);
        push_coordinate(&mut collection, 3.0, 4.0, &[5.0, 6.0]);
        push_header(&mut collection, 3001);
        push_coordinate(&mut collection, 7.0, 8.0, &[9.0, 10.0]);
        assert!(
            validate_wkb_contract_for_dimensions(&collection, GeometryDimensions::Xyzm).is_ok()
        );
    }

    #[test]
    fn truncation_inside_extra_ordinates_is_rejected() {
        let mut point = Vec::new();
        push_header(&mut point, 3001);
        push_coordinate(&mut point, 1.0, 2.0, &[3.0, 4.0]);
        assert!(validate_wkb_contract_for_dimensions(&point, GeometryDimensions::Xyzm).is_ok());
        // Troncato a meta' dell'ordinata M (ultimi 4 byte).
        assert!(validate_wkb_contract_for_dimensions(
            &point[..point.len() - 4],
            GeometryDimensions::Xyzm
        )
        .is_err());
        // Troncato a meta' dell'ordinata Z.
        assert!(validate_wkb_contract_for_dimensions(
            &point[..point.len() - 12],
            GeometryDimensions::Xyzm
        )
        .is_err());
        // Troncato esattamente dopo Y: le ordinate extra mancano del tutto.
        assert!(validate_wkb_contract_for_dimensions(
            &point[..point.len() - 16],
            GeometryDimensions::Xyzm
        )
        .is_err());
    }

    #[test]
    fn type_code_incoherent_with_expected_dimensions_gets_dedicated_error() {
        // Dichiara Xy ma il type code e' Z (ISO): rifiuto del wrapper XY.
        let mut z_point = Vec::new();
        push_header(&mut z_point, 1001);
        push_coordinate(&mut z_point, 1.0, 2.0, &[3.0]);
        assert!(matches!(
            validate_wkb_contract(&z_point),
            Err(PlenoraError::Unsupported(message))
                if message == "WKB contiene dimensioni Z/M o SRID non preservabili nel protocollo 2D"
        ));
        // Dichiara Xyz/Xym/Xyzm ma il type code e' XY: errore dedicato.
        let mut xy_point = Vec::new();
        push_header(&mut xy_point, 1);
        push_coordinate(&mut xy_point, 1.0, 2.0, &[]);
        for expected in [
            GeometryDimensions::Xyz,
            GeometryDimensions::Xym,
            GeometryDimensions::Xyzm,
        ] {
            assert!(matches!(
                validate_wkb_contract_for_dimensions(&xy_point, expected),
                Err(PlenoraError::InvalidPlan(message))
                    if message == "dimensionalita' WKB incoerente con la dimensionalita' attesa dal contratto"
            ));
        }
        // Dichiara Xym ma il type code porta Z (ISO 1001): stesso errore.
        assert!(matches!(
            validate_wkb_contract_for_dimensions(&z_point, GeometryDimensions::Xym),
            Err(PlenoraError::InvalidPlan(message))
                if message == "dimensionalita' WKB incoerente con la dimensionalita' attesa dal contratto"
        ));
    }

    #[test]
    fn point_count_is_checked_against_the_real_stride() {
        // LineString Z che dichiara 3 coordinate ma ne contiene 2 (48 byte):
        // con stride 16 basterebbero, con lo stride reale 24 no. Il
        // validatore non deve desincronizzarsi: rifiuta.
        let mut line = Vec::new();
        push_header(&mut line, 1002);
        push_count(&mut line, 3);
        push_coordinate(&mut line, 0.0, 0.0, &[1.0]);
        push_coordinate(&mut line, 1.0, 1.0, &[2.0]);
        assert!(matches!(
            validate_wkb_contract_for_dimensions(&line, GeometryDimensions::Xyz),
            Err(PlenoraError::InvalidPlan(message))
                if message == "struttura WKB non valida: conteggio elementi oltre i byte disponibili"
        ));
        // Conteggio ostile con stride ZM: sempre rifiutato, mai desync.
        let mut hostile = Vec::new();
        push_header(&mut hostile, 3002);
        hostile.extend_from_slice(&u32::MAX.to_le_bytes());
        push_coordinate(&mut hostile, 0.0, 0.0, &[1.0, 2.0]);
        assert!(validate_wkb_contract_for_dimensions(&hostile, GeometryDimensions::Xyzm).is_err());
    }

    #[test]
    fn ewkb_srid_flag_is_rejected_for_every_expected_dimension() {
        // Punto EWKB con SRID (flag 0x2000_0000) + valore SRID + XY.
        let mut payload = Vec::new();
        push_header(&mut payload, 0x2000_0001);
        payload.extend_from_slice(&4326_u32.to_le_bytes());
        push_coordinate(&mut payload, 1.0, 2.0, &[]);
        for expected in [
            GeometryDimensions::Xy,
            GeometryDimensions::Xyz,
            GeometryDimensions::Xym,
            GeometryDimensions::Xyzm,
            GeometryDimensions::Unknown,
        ] {
            assert!(matches!(
                validate_wkb_contract_for_dimensions(&payload, expected),
                Err(PlenoraError::Unsupported(_))
            ));
        }
        // SRID combinato con Z: sempre rifiutato.
        let mut payload_z = Vec::new();
        push_header(&mut payload_z, 0xA000_0001);
        payload_z.extend_from_slice(&4326_u32.to_le_bytes());
        push_coordinate(&mut payload_z, 1.0, 2.0, &[3.0]);
        assert!(matches!(
            validate_wkb_contract_for_dimensions(&payload_z, GeometryDimensions::Unknown),
            Err(PlenoraError::Unsupported(_))
        ));
    }

    #[test]
    fn transport_gate_matches_big_endian_srid_without_weakening_kernel_gate() {
        let mut payload = vec![0_u8];
        payload.extend_from_slice(&0x2000_0001_u32.to_be_bytes());
        payload.extend_from_slice(&4326_u32.to_be_bytes());
        payload.extend_from_slice(&1.0_f64.to_be_bytes());
        payload.extend_from_slice(&2.0_f64.to_be_bytes());

        assert!(validate_wkb_transport_for_dimensions_with_depth(
            &payload,
            GeometryDimensions::Xy,
            GeometryEncoding::Ewkb,
            Some(4326),
            MAX_WKB_DEPTH,
        )
        .is_ok());
        assert!(matches!(
            validate_wkb_transport_for_dimensions_with_depth(
                &payload,
                GeometryDimensions::Xy,
                GeometryEncoding::Wkb,
                Some(4326),
                MAX_WKB_DEPTH,
            ),
            Err(PlenoraError::Unsupported(_))
        ));
        assert!(matches!(
            validate_wkb_transport_for_dimensions_with_depth(
                &payload,
                GeometryDimensions::Xy,
                GeometryEncoding::Ewkb,
                None,
                MAX_WKB_DEPTH,
            ),
            Err(PlenoraError::Crs(_) | PlenoraError::CrsCoded { .. })
        ));
        assert!(matches!(
            validate_wkb_transport_for_dimensions_with_depth(
                &payload,
                GeometryDimensions::Xy,
                GeometryEncoding::Ewkb,
                Some(32632),
                MAX_WKB_DEPTH,
            ),
            Err(PlenoraError::Crs(_) | PlenoraError::CrsCoded { .. })
        ));
        assert!(matches!(
            validate_wkb_contract_for_dimensions(&payload, GeometryDimensions::Xy),
            Err(PlenoraError::Unsupported(_))
        ));
        assert!(matches!(
            validate_wkb_transport_for_dimensions_with_depth(
                &payload[..5],
                GeometryDimensions::Xy,
                GeometryEncoding::Ewkb,
                Some(4326),
                MAX_WKB_DEPTH,
            ),
            Err(PlenoraError::InvalidPlan(_))
        ));
    }

    #[test]
    fn non_finite_xy_is_rejected_with_z_present_but_z_is_never_read() {
        // NaN in X con Z presente: rifiutato.
        let mut payload = Vec::new();
        push_header(&mut payload, 1001);
        push_coordinate(&mut payload, f64::NAN, 2.0, &[3.0]);
        assert!(matches!(
            validate_wkb_contract_for_dimensions(&payload, GeometryDimensions::Xyz),
            Err(PlenoraError::InvalidPlan(message))
                if message == "WKB contiene coordinate NaN o infinite"
        ));
        // Inf in Y con Z presente: rifiutato.
        let mut payload = Vec::new();
        push_header(&mut payload, 1001);
        push_coordinate(&mut payload, 1.0, f64::INFINITY, &[3.0]);
        assert!(matches!(
            validate_wkb_contract_for_dimensions(&payload, GeometryDimensions::Xyz),
            Err(PlenoraError::InvalidPlan(message))
                if message == "WKB contiene coordinate NaN o infinite"
        ));
        // NaN nell'ordinata Z: accettato, perche' Z non e' mai letta ne'
        // reinterpretata: i byte sono preservati senza elaborare le ordinate
        // extra; vedi il doc-comment di validate_wkb_geometry_with_dimensions).
        let mut payload = Vec::new();
        push_header(&mut payload, 1001);
        push_coordinate(&mut payload, 1.0, 2.0, &[f64::NAN]);
        assert!(validate_wkb_contract_for_dimensions(&payload, GeometryDimensions::Xyz).is_ok());
    }

    #[test]
    fn depth_limit_applies_to_dimensional_collections() {
        // GC(GC(Point ZM)): il punto e' a profondita' 2.
        let mut outer = Vec::new();
        push_header(&mut outer, 3007);
        push_count(&mut outer, 1);
        push_header(&mut outer, 3007);
        push_count(&mut outer, 1);
        push_header(&mut outer, 3001);
        push_coordinate(&mut outer, 1.0, 2.0, &[3.0, 4.0]);
        assert!(validate_wkb_contract_for_dimensions_with_depth(
            &outer,
            GeometryDimensions::Xyzm,
            2
        )
        .is_ok());
        assert!(matches!(
            validate_wkb_contract_for_dimensions_with_depth(&outer, GeometryDimensions::Xyzm, 1),
            Err(PlenoraError::InvalidPlan(message))
                if message == "struttura WKB non valida: annidamento geometrie oltre il limite"
        ));
        // Default MAX_WKB_DEPTH anche per la variante stride-aware.
        assert!(validate_wkb_contract_for_dimensions(&outer, GeometryDimensions::Xyzm).is_ok());
    }

    #[test]
    fn unknown_dimensions_preserve_bytes_and_derive_stride_per_geometry() {
        // Unknown: byte preservati, dimensionalita' dal type code di
        // ogni geometria; una collection puo' mescolare dimensionalita'.
        let mut collection = Vec::new();
        push_header(&mut collection, 7);
        push_count(&mut collection, 3);
        push_header(&mut collection, 1);
        push_coordinate(&mut collection, 1.0, 2.0, &[]);
        push_header(&mut collection, 1001);
        push_coordinate(&mut collection, 3.0, 4.0, &[5.0]);
        push_header(&mut collection, 0xC000_0001);
        push_coordinate(&mut collection, 6.0, 7.0, &[8.0, 9.0]);
        assert!(
            validate_wkb_contract_for_dimensions(&collection, GeometryDimensions::Unknown).is_ok()
        );
        // Unknown non accetta strutture sbagliate: troncamento rifiutato.
        assert!(validate_wkb_contract_for_dimensions(
            &collection[..collection.len() - 8],
            GeometryDimensions::Unknown
        )
        .is_err());
        // Type code con bit alti riservati (non ISO, non EWKB): rifiutato.
        let mut garbage = Vec::new();
        push_header(&mut garbage, 0x8001_0001);
        push_coordinate(&mut garbage, 1.0, 2.0, &[3.0]);
        assert!(matches!(
            validate_wkb_contract_for_dimensions(&garbage, GeometryDimensions::Unknown),
            Err(PlenoraError::InvalidPlan(message))
                if message == "struttura WKB non valida: tipo geometria non supportato"
        ));
    }

    #[test]
    fn ring_closure_is_checked_on_xy_only() {
        // Anello chiuso in X/Y con Z diverse agli estremi: accettato, perche'
        // Z non e' letta: le ordinate extra non sono elaborate.
        let mut polygon = Vec::new();
        push_header(&mut polygon, 1003);
        push_count(&mut polygon, 1);
        push_count(&mut polygon, 4);
        push_coordinate(&mut polygon, 0.0, 0.0, &[1.0]);
        push_coordinate(&mut polygon, 4.0, 0.0, &[2.0]);
        push_coordinate(&mut polygon, 4.0, 4.0, &[3.0]);
        push_coordinate(&mut polygon, 0.0, 0.0, &[99.0]);
        assert!(validate_wkb_contract_for_dimensions(&polygon, GeometryDimensions::Xyz).is_ok());
        // Anello non chiuso in X/Y: rifiutato anche con Z coerenti.
        let mut open = Vec::new();
        push_header(&mut open, 1003);
        push_count(&mut open, 1);
        push_count(&mut open, 4);
        push_coordinate(&mut open, 0.0, 0.0, &[1.0]);
        push_coordinate(&mut open, 4.0, 0.0, &[1.0]);
        push_coordinate(&mut open, 4.0, 4.0, &[1.0]);
        push_coordinate(&mut open, 0.0, 4.0, &[1.0]);
        assert!(matches!(
            validate_wkb_contract_for_dimensions(&open, GeometryDimensions::Xyz),
            Err(PlenoraError::InvalidPlan(message))
                if message == "struttura WKB non valida: anello poligonale non chiuso"
        ));
    }

    #[test]
    fn xy_wrapper_still_rejects_ewkb_flags_like_before() {
        // Il wrapper a sola XY rifiuta i flag EWKB con `unsupported_wkb_dimension`.
        for raw_type in [0x8000_0001_u32, 0x4000_0001, 0xC000_0001, 0x2000_0001] {
            let mut payload = Vec::new();
            push_header(&mut payload, raw_type);
            payload.extend_from_slice(&0_u32.to_le_bytes()); // eventuale SRID
            push_coordinate(&mut payload, 1.0, 2.0, &[3.0, 4.0]);
            assert!(matches!(
                validate_wkb_contract(&payload),
                Err(PlenoraError::Unsupported(message))
                    if message == "WKB contiene dimensioni Z/M o SRID non preservabili nel protocollo 2D"
            ));
        }
    }

    proptest! {
        #[test]
        fn arbitrary_wkb_bytes_never_panic_and_successes_remain_roundtrippable(
            payload in proptest::collection::vec(any::<u8>(), 0..4096)
        ) {
            if let Ok(geometry) = geometry_from_wkb(&payload) {
                prop_assert!(geometry.validazione_protetta().is_ok());
                for operation in Operation::ALL {
                    if let Ok(output) = transform_wkb(operation, &payload) {
                        prop_assert!(geometry_from_wkb(&output).is_ok());
                    }
                }
            }
        }

        #[test]
        fn single_byte_mutations_of_valid_wkb_never_escape_the_contract(
            index in any::<usize>(), replacement in any::<u8>()
        ) {
            let mut payload = rect(0.0, 0.0, 4.0, 4.0).to_wkb(CoordDimensions::xy()).unwrap();
            let position = index % payload.len();
            payload[position] = replacement;
            if let Ok(geometry) = geometry_from_wkb(&payload) {
                prop_assert!(geometry.validazione_protetta().is_ok());
                let encoded = geometry.to_wkb(CoordDimensions::xy()).unwrap();
                prop_assert!(validate_wkb_contract(&encoded).is_ok());
            }
        }
    }

    /// Rilancia questo binario di test sul solo `nome_test`, con
    /// `variabile=valore` nell'ambiente: e' il ramo figlio dei casi a
    /// sottoprocesso qui sotto.
    fn esegui_figlio_di_test(
        nome_test: &str,
        variabile: &str,
        valore: &str,
    ) -> std::process::Output {
        let ese = std::env::current_exe().expect("current_exe");
        std::process::Command::new(ese)
            .arg("--exact")
            .arg(nome_test)
            // Senza `--nocapture` la libreria di test intercetta lo
            // stderr del thread, e l'hook di `std` scriverebbe nel suo
            // buffer invece che sul canale reale.
            .arg("--nocapture")
            .env(variabile, valore)
            .output()
            .expect("il figlio parte")
    }

    /// Che cosa esce davvero da **stderr** quando `validazione_protetta`
    /// contiene un panico — modulo unitario, non `tests/` d'integrazione.
    ///
    /// Sta sotto `#[cfg(test)]` e non dietro una feature `test-support`, che
    /// lascerebbe un `panic!` raggiungibile in una superficie pubblica: il
    /// sottoprocesso rilancia questo stesso binario di test.
    ///
    /// Serve un processo perche' l'hook di `std` stampa **prima**
    /// dell'unwinding, e un `catch_unwind` non vede se il payload e' uscito;
    /// l'hook e' stato globale e legherebbe gli altri test a questo.
    mod barriera_privacy_processo {
        use crate::ValidazioneProtetta as _;

        const VARIABILE: &str = "PLENORA_TEST_BARRIERA_PRIVACY";

        /// Frammenti del payload sintetico che stderr non deve mai portare.
        const FRAMMENTI: [&str; 3] = ["1.5", "-2.5", "COORD"];

        struct Uscita {
            stderr: String,
            riuscita: bool,
            stato: String,
        }

        fn esegui_figlio(politica: &str) -> Uscita {
            let uscita = super::esegui_figlio_di_test(
                "tests::barriera_privacy_processo::il_ramo_figlio_non_e_un_test_vero",
                VARIABILE,
                politica,
            );
            Uscita {
                stderr: String::from_utf8_lossy(&uscita.stderr).into_owned(),
                riuscita: uscita.status.success(),
                stato: format!("{}", uscita.status),
            }
        }

        impl Uscita {
            fn stderr_di_una_corsa_riuscita(&self, quale: &str) -> &str {
                assert!(
                    self.riuscita,
                    "il figlio «{quale}» non e' arrivato in fondo ({}); \
                     qualunque cosa abbia stampato non dimostra niente:\n{}",
                    self.stato, self.stderr
                );
                &self.stderr
            }
        }

        /// La riga sanitizzata, verificata per **intero**.
        fn pretendi_la_riga_sanitizzata(stderr: &str) {
            let righe: Vec<&str> = stderr.lines().filter(|r| !r.trim().is_empty()).collect();
            assert_eq!(
                righe.len(),
                1,
                "atteso esattamente una riga su stderr, trovate {}:\n{stderr}",
                righe.len()
            );
            let riga = righe[0];
            let Some(resto) = riga.strip_prefix("plenora: panico interno a ") else {
                panic!("prefisso inatteso: {riga}");
            };
            let Some((posizione, coda)) = resto.split_once(" (") else {
                panic!("manca la forma del payload: {riga}");
            };
            assert_eq!(
                coda,
                "payload dinamico (contenuto non pubblicato)); \
                 nessun contenuto del payload viene pubblicato",
                "coda inattesa: {riga}"
            );
            assert!(
                posizione.contains("plenora-kernels-geo") && posizione.contains("lib.rs:"),
                "posizione inattesa: {posizione}"
            );
        }

        #[test]
        fn con_la_politica_sanitizzata_stderr_non_porta_il_payload() {
            let uscita = esegui_figlio("sanitized");
            let stderr = uscita.stderr_di_una_corsa_riuscita("sanitized");
            for frammento in FRAMMENTI {
                assert!(
                    !stderr.contains(frammento),
                    "stderr pubblica «{frammento}»:\n{stderr}"
                );
            }
            pretendi_la_riga_sanitizzata(stderr);
        }

        #[test]
        fn senza_politica_il_payload_esce_davvero() {
            let uscita = esegui_figlio("default");
            let stderr = uscita.stderr_di_una_corsa_riuscita("default");
            assert!(
                FRAMMENTI.iter().any(|frammento| stderr.contains(frammento)),
                "l'hook predefinito deve pubblicare il payload, o il confronto \
                 non dimostra niente:\n{stderr}"
            );
        }

        /// Il ramo figlio: non e' un caso, e il nome lo dice. Gira solo
        /// quando la variabile e' presente.
        ///
        /// Pretende l'**esito tipizzato** di `validazione_protetta`
        /// (`EsitoValidazione::NonConclusa` col testo esatto), cosi' vede anche
        /// un cambio di variante.
        #[test]
        fn il_ramo_figlio_non_e_un_test_vero() {
            struct TipoDiProva;

            impl crate::AnelliSemplici for TipoDiProva {
                fn ha_un_anello_con_punta(&self) -> bool {
                    false
                }
            }

            // Il ramo predefinito: `check_validation`, che qui va in panico.
            impl crate::validazione_ogc::ValidazioneOgc for TipoDiProva {}

            impl geo::algorithm::validation::Validation for TipoDiProva {
                type Error = std::convert::Infallible;

                fn check_validation(&self) -> std::result::Result<(), Self::Error> {
                    let finta_coordinata = (1.5_f64, -2.5_f64);
                    panic!(
                        "test-support: coordinate fittizie COORD{finta_coordinata:?}, \
                         nessun dato reale"
                    );
                }

                fn visit_validation<T>(
                    &self,
                    _visitor: Box<dyn FnMut(Self::Error) -> std::result::Result<(), T> + '_>,
                ) -> std::result::Result<(), T> {
                    Ok(())
                }
            }

            let Ok(politica) = std::env::var(VARIABILE) else {
                return;
            };
            if politica == "sanitized" {
                assert!(
                    plenora_core::panic_policy::install(
                        plenora_core::panic_policy::PanicPolicy::Sanitized
                    ),
                    "la politica sanitizzata non e' stata installata: cio' che segue \
                     uscirebbe dall'hook predefinito"
                );
            }
            let esito = TipoDiProva.validazione_protetta();
            assert_eq!(
                esito,
                Err(super::super::EsitoValidazione::NonConclusa(
                    "payload dinamico (contenuto non pubblicato)"
                )),
                "atteso l'esito tipizzato NonConclusa esatto"
            );
        }
    }

    /// Il logging di `relate` nel `geo` vendorizzato resta statico anche con
    /// un logger attivo a Trace: stessa coppia di quadrati, matrice ed elenco
    /// dei messaggi ammessi della sonda di privacy del progetto d'origine, con
    /// `a.relate(&b)` diretto.
    ///
    /// Sottoprocesso perche' `log::set_logger` si installa una sola volta per
    /// processo.
    mod prova_logging_relate {
        use geo::{LineString, Polygon, Relate};
        use std::sync::Mutex;

        const VARIABILE: &str = "PLENORA_TEST_LOGGING_RELATE";

        /// I siti di log statici di `relate`, copiati dall'elenco degli eventi
        /// ammessi misurato nel progetto d'origine.
        const MESSAGGI_AMMESSI: [&str; 12] = [
            "geo.relate.edge_end_bundle_star.0",
            "geo.relate.edge_end_bundle_star.1",
            "geo.relate.edge_end_bundle_star.2",
            "geo.relate.edge_end_bundle_star.3",
            "geo.relate.geometry_graph.0",
            "geo.relate.geometry_graph.1",
            "geo.relate.geometry_graph.2",
            "geo.relate.node.0",
            "geo.relate.relate_operation.0",
            "geo.relate.relate_operation.1",
            "geo.relate.relate_operation.2",
            "geo.relate.topology_position.0",
        ];

        struct Collector(Mutex<Vec<(String, String)>>);

        impl log::Log for Collector {
            fn enabled(&self, _: &log::Metadata) -> bool {
                true
            }
            fn log(&self, record: &log::Record) {
                self.0
                    .lock()
                    .unwrap()
                    .push((record.target().to_owned(), record.args().to_string()));
            }
            fn flush(&self) {}
        }

        static LOGGER: Collector = Collector(Mutex::new(Vec::new()));

        /// Esito e stdout del figlio su `nome_test`.
        fn esegui_figlio_con(nome_test: &str, variabile: &str, valore: &str) -> (bool, String) {
            let uscita = super::esegui_figlio_di_test(nome_test, variabile, valore);
            (
                uscita.status.success(),
                String::from_utf8_lossy(&uscita.stdout).into_owned(),
            )
        }

        fn esegui_figlio() -> (bool, String) {
            esegui_figlio_con(
                "tests::prova_logging_relate::il_ramo_figlio_non_e_un_test_vero",
                VARIABILE,
                "1",
            )
        }

        /// Controllo positivo, messaggi ammessi, logger attivo a Trace:
        /// nessuna delle tre condizioni da sola prova che il gate ha
        /// esercitato `relate`.
        #[test]
        fn relate_su_quadrati_sovrapposti_non_emette_testo_non_ammesso() {
            let (riuscito, stdout) = esegui_figlio();
            assert!(
                riuscito,
                "il figlio non e' arrivato in fondo; stdout:\n{stdout}"
            );
            assert!(
                stdout.contains("logger-attivo-confermato"),
                "il figlio non ha confermato il controllo positivo sul logger:\n{stdout}"
            );
            assert!(
                stdout.contains("record-geo-non-vuoti"),
                "nessun record con target `geo::`: il gate non ha esercitato relate:\n{stdout}"
            );
            assert!(
                stdout.contains("tutti-i-messaggi-ammessi"),
                "almeno un messaggio non appartiene all'elenco ammesso:\n{stdout}"
            );
        }

        /// Il ramo figlio: gira solo quando la variabile e' presente.
        #[test]
        fn il_ramo_figlio_non_e_un_test_vero() {
            if std::env::var(VARIABILE).is_err() {
                return;
            }
            log::set_logger(&LOGGER).expect("nessun altro logger deve essere gia' installato qui");
            log::set_max_level(log::LevelFilter::Trace);
            log::info!(target: "prova_logging_relate", "logger-active");

            // Stessa coppia della sonda d'origine: due quadrati che si
            // sovrappongono, Relate diretto.
            let quadrato = |x: f64, y: f64| {
                Polygon::new(
                    LineString::from(vec![
                        (x, y),
                        (x + 2., y),
                        (x + 2., y + 2.),
                        (x, y + 2.),
                        (x, y),
                    ]),
                    vec![],
                )
            };
            let a = quadrato(12_345.678_901_234_5, 87_654.321_098_765_4);
            let b = quadrato(12_346.678_901_234_5, 87_655.321_098_765_4);
            let matrice = a.relate(&b);
            assert_eq!(
                format!("{matrice:?}"),
                "IntersectionMatrix(212101212)",
                "matrice attesa dal probe consegnato, ottenuta diversa"
            );

            let records = LOGGER.0.lock().unwrap();
            assert!(
                records
                    .iter()
                    .any(|(t, m)| t == "prova_logging_relate" && m == "logger-active"),
                "il controllo positivo sul logger non e' stato registrato"
            );
            println!("logger-attivo-confermato");

            let geo_records: Vec<_> = records
                .iter()
                .filter(|(t, _)| t.starts_with("geo::"))
                .collect();
            assert!(
                !geo_records.is_empty(),
                "nessun record con target geo::: relate non e' stato esercitato dal logger"
            );
            println!("record-geo-non-vuoti={}", geo_records.len());

            let non_ammessi: Vec<_> = geo_records
                .iter()
                .filter(|(_, messaggio)| !MESSAGGI_AMMESSI.contains(&messaggio.as_str()))
                .collect();
            assert!(
                non_ammessi.is_empty(),
                "messaggi non nell'elenco ammesso: {non_ammessi:?}"
            );
            println!("tutti-i-messaggi-ammessi");
            drop(records);
        }

        // ---------------------------------------------------------------
        // Originali e ridotti A/B: stesso gate, quattro reperti reali.
        // ---------------------------------------------------------------
        //
        // I quadrati provano che il gate funziona; questi reperti che reggono
        // su dati reali, con un verdetto specifico per fixture. Decodifica
        // grezza (`geozero`), perche' il bersaglio e' `check_validation` di
        // `geo` e non il cancello strutturale. Verdetti e conteggi vengono
        // dalle uscite della sonda d'origine, in debug e in release.
        const REPERTO_ORIGINALE_A: &[u8] = include_bytes!("../tests/fixtures/reperto_a.wkb");
        const REPERTO_ORIGINALE_B: &[u8] = include_bytes!("../tests/fixtures/reperto_b.wkb");
        const REPERTO_RIDOTTO_A: &[u8] = &[
            1, 6, 0, 0, 0, 2, 0, 0, 0, 1, 3, 0, 0, 0, 1, 0, 0, 0, 6, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 12, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 5, 46, 254, 255, 255, 253, 15, 0, 0, 16, 0, 0, 44, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 212, 0, 0, 0, 4, 0, 4, 0, 0, 8, 116, 116, 116, 116, 116, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 3, 0, 0, 0, 1, 0, 0, 0, 4, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 212, 0, 0, 0,
            0, 0, 4, 0, 0, 8, 116, 116, 116, 116, 116, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0,
        ];
        const REPERTO_RIDOTTO_B: &[u8] = &[
            0, 0, 0, 0, 6, 0, 0, 0, 2, 0, 0, 0, 0, 3, 0, 0, 0, 1, 0, 0, 0, 4, 1, 1, 1, 1, 1, 1, 1,
            1, 1, 1, 1, 1, 1, 1, 1, 0, 1, 0, 0, 0, 0, 0, 0, 0, 4, 1, 1, 1, 1, 0, 8, 1, 1, 1, 64, 1,
            1, 1, 1, 1, 1, 1, 65, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 0, 0,
            0, 0, 0, 3, 0, 0, 0, 1, 0, 0, 0, 4, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 0, 1,
            0, 50, 0, 0, 0, 0, 0, 4, 1, 1, 1, 1, 0, 8, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 9, 1, 1,
            1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 0,
        ];

        const VARIABILE_REPERTO: &str = "PLENORA_TEST_LOGGING_REPERTO";

        fn esegui_figlio_reperto(reperto: &str) -> (bool, String) {
            esegui_figlio_con(
                "tests::prova_logging_relate::il_ramo_figlio_reperto_non_e_un_test_vero",
                VARIABILE_REPERTO,
                reperto,
            )
        }

        fn verifica_reperto(reperto: &str, verdetto_atteso: &str, record_geo_attesi: usize) {
            let (riuscito, stdout) = esegui_figlio_reperto(reperto);
            assert!(
                riuscito,
                "il figlio ({reperto}) non e' arrivato in fondo; stdout:\n{stdout}"
            );
            assert!(
                stdout.contains("logger-attivo-confermato"),
                "{reperto}: controllo positivo sul logger mancante:\n{stdout}"
            );
            assert!(
                stdout.contains(&format!("record-geo={record_geo_attesi}")),
                "{reperto}: atteso record-geo={record_geo_attesi}, stdout:\n{stdout}"
            );
            assert!(
                stdout.contains("tutti-i-messaggi-ammessi"),
                "{reperto}: almeno un messaggio non ammesso, stdout:\n{stdout}"
            );
            assert!(
                stdout.contains(&format!("verdetto={verdetto_atteso}")),
                "{reperto}: verdetto inatteso, stdout:\n{stdout}"
            );
        }

        /// Originale A: il poligono 2 e' auto-intersecante, non un conflitto a
        /// coppia.
        #[test]
        fn originale_a_verdetto_specifico_e_soli_messaggi_ammessi() {
            verifica_reperto(
                "originale_a",
                "Err(InvalidMultiPolygon(InvalidPolygon(GeometryIndex(2), SelfIntersection(Exterior))))",
                9,
            );
        }

        /// Originale B: `ElementsOverlaps(1, 2)`, il conflitto a coppia.
        #[test]
        fn originale_b_verdetto_specifico_e_soli_messaggi_ammessi() {
            verifica_reperto(
                "originale_b",
                "Err(InvalidMultiPolygon(ElementsOverlaps(GeometryIndex(1), GeometryIndex(2))))",
                9,
            );
        }

        /// Ridotto A: **valido**, senza il terzo poligono invalido
        /// dell'originale.
        #[test]
        fn ridotto_a_e_valido_e_soli_messaggi_ammessi() {
            verifica_reperto("ridotto_a", "Ok(())", 9);
        }

        /// Ridotto B: `ElementsOverlaps(0, 1)`, invalido come l'originale con
        /// un poligono in meno a monte.
        #[test]
        fn ridotto_b_verdetto_specifico_e_soli_messaggi_ammessi() {
            verifica_reperto(
                "ridotto_b",
                "Err(InvalidMultiPolygon(ElementsOverlaps(GeometryIndex(0), GeometryIndex(1))))",
                9,
            );
        }

        /// Il ramo figlio dei reperti: logger fresco per processo, decodifica
        /// grezza, `check_validation` diretto.
        #[test]
        fn il_ramo_figlio_reperto_non_e_un_test_vero() {
            use geo::algorithm::validation::Validation as _;
            use geo::Geometry;
            use geozero::{wkb::Wkb, ToGeo};

            let Ok(reperto) = std::env::var(VARIABILE_REPERTO) else {
                return;
            };
            log::set_logger(&LOGGER).expect("nessun altro logger deve essere gia' installato qui");
            log::set_max_level(log::LevelFilter::Trace);
            log::info!(target: "prova_logging_relate", "logger-active");

            let payload: &[u8] = match reperto.as_str() {
                "originale_a" => REPERTO_ORIGINALE_A,
                "originale_b" => REPERTO_ORIGINALE_B,
                "ridotto_a" => REPERTO_RIDOTTO_A,
                "ridotto_b" => REPERTO_RIDOTTO_B,
                altro => panic!("reperto sconosciuto: {altro}"),
            };
            let geometria: Geometry<f64> = Wkb(payload).to_geo().expect("decodifica grezza");
            // `check_validation` diretto: interessa il verdetto di `geo`, che
            // su questi reperti non va in panico (barriera_validazione.rs).
            let esito = geometria.check_validation();

            let records = LOGGER.0.lock().unwrap();
            assert!(
                records
                    .iter()
                    .any(|(t, m)| t == "prova_logging_relate" && m == "logger-active"),
                "il controllo positivo sul logger non e' stato registrato"
            );
            println!("logger-attivo-confermato");

            let geo_records: Vec<_> = records
                .iter()
                .filter(|(t, _)| t.starts_with("geo::"))
                .collect();
            println!("record-geo={}", geo_records.len());

            let non_ammessi: Vec<_> = geo_records
                .iter()
                .filter(|(_, messaggio)| !MESSAGGI_AMMESSI.contains(&messaggio.as_str()))
                .collect();
            assert!(
                non_ammessi.is_empty(),
                "messaggi non nell'elenco ammesso: {non_ammessi:?}"
            );
            println!("tutti-i-messaggi-ammessi");
            drop(records);

            println!("verdetto={esito:?}");
        }
    }
}
