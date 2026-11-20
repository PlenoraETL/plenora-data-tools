//! Distanza fra insiemi sull'ellissoide del CRS geografico.
//!
//! Modello `shortest_geodesic`, GOP-004/005/006/026/028. Le primitive
//! direct/inverse sono G1 Karney 2013, G2 `GeographicLib`, non un oracolo
//! di minimizzazione: il minimo globale usa intervalli metrici conservativi.

#![allow(
    clippy::suboptimal_flops,
    reason = "Ordine esplicito delle operazioni nelle maggiorazioni numeriche"
)]
#![allow(
    clippy::float_cmp,
    reason = "Identita e simmetrie del modello geodetico"
)]

use std::sync::atomic::AtomicBool;
use std::time::Instant;

use geo::Geometry;
use plenora_core::crs::ResolvedCrs;
use thiserror::Error;

use crate::geodetica::EllissoideGeodetico;

use crate::geodesic_surface as surface;
use crate::geodesic_surface::metric::{minimum, GeometryIndex, Work};

/// Budget finiti, cumulativi; zero ammette zero lavoro.
#[derive(Clone, Copy, Debug)]
pub struct GeodesicDistanceLimits {
    /// Operazioni effettive: coordinate lette, primitive direct/inverse,
    /// aree di lati, visite dell'indice e iterazioni di raffinamento.
    pub max_work: u64,
    /// Capacita' dei vettori e della coda. Esclusi input preso in prestito,
    /// stack (profondita' limitata), overhead dell'allocatore e buffer
    /// transitori durante la riallocazione (vecchio e nuovo coesistenti).
    pub max_reserved_bytes: u64,
    /// Scadenza monotona; prevale sull'annullamento allo stesso checkpoint.
    pub deadline: Option<Instant>,
}

/// Risultato in metri; il certificato assoluto e' [`ABSOLUTE_ERROR_M`].
#[derive(Clone, Copy, Debug)]
pub struct GeodesicDistanceResult {
    /// Minimo fra gli insiemi pieni, inclusi gli interni dei poligoni.
    pub metres: f64,
    /// Lavoro effettivamente addebitato, compresa la validazione.
    pub work: u64,
    /// Massima capacita' viva addebitata.
    pub peak_reserved_bytes: u64,
}

/// Errore assoluto in metri; errore relativo zero, nessuna trasformazione CRS.
pub const ABSOLUTE_ERROR_M: f64 = 0.01;
/// Errori bounded senza valori delle geometrie.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum GeodesicDistanceError {
    /// Dominio CRS errato.
    #[error("distanza geodetica richiede un CRS geografico")]
    GeographicRequired,
    /// Ellissoide fuori dal dominio terrestre qualificato.
    #[error("ellissoide non supportato")]
    UnsupportedEllipsoid,
    /// Coordinate o struttura non valide.
    #[error("geometria geografica non valida")]
    InvalidGeometry,
    /// Operando vuoto.
    #[error("distanza da geometria vuota non definita")]
    EmptyGeometry,
    /// Tipo fuori dalla superficie XY Simple Features.
    #[error("tipo geometrico non supportato")]
    UnsupportedGeometry,
    /// Percorso minimo non unico.
    #[error("geodetica minima ambigua")]
    AmbiguousGeodesic,
    /// Topologia, ramo numerico o precisione non certificabile.
    #[error("GEOMETRY_ACCURACY_UNPROVEN")]
    AccuracyUnproven,
    /// Anelli che si toccano/incrociano o complemento ambiguo.
    #[error("superficie geodetica non supportata")]
    UnsupportedSurface,
    /// Lavoro esaurito.
    #[error("limite di lavoro geodetico superato")]
    WorkLimit,
    /// Capacita' viva esaurita.
    #[error("limite di memoria geodetica superato")]
    MemoryLimit,
    /// Prenotazione fallita.
    #[error("allocazione geodetica fallita")]
    Allocation,
    /// Annullamento cooperativo.
    #[error("distanza geodetica annullata")]
    Cancelled,
    /// Deadline raggiunta.
    #[error("scadenza della distanza geodetica raggiunta")]
    DeadlineExceeded,
}

/// Minima distanza geodetica fra Point, `MultiPoint`, `LineString`,
/// `MultiLineString`, Polygon e `MultiPolygon` XY, in metri sul datum risolto.
///
/// Lati come geodetiche minime, interni pieni meno i buchi, anelli semplici
/// con regione minore di mezzo ellissoide (GOP-004/005). Nessuna proiezione
/// o riparazione implicita. Vuoti e strutture non valide sono errori.
/// Il percorso planare e le API punto-punto esistenti restano indipendenti.
///
/// Certificato GOP-006: errore assoluto 0.01 m, relativo zero. Ricerca
/// globale branch-and-bound, non ottimizzazione locale: un arco di lunghezza
/// L e' nella palla ellissoidica (punto medio,L/2); per due palle il limite
/// inferiore e' max(0,d(c1,c2)-r1-r2), con margini numerici. L'indice unisce
/// palle per disuguaglianza triangolare. Sotto 1000 km usa anche il bound
/// di Taylor della distanza, con Hessiana maggiorata tramite m12 e M12
/// (G1, campi di Jacobi); prove differenziali e fixture Python indipendenti
/// controllano separatamente ottimizzazioni e risultati.
/// Raffina finche' upper-lower <=4 mm.
/// Un upper bound entro 1 cm restituisce zero, inclusi incroci e contatti. Il margine
/// numerico, inclusa la sensibilita' del ramo inverso, e' 0.5 mm; non c'e' errore
/// di trasformazione. Dominio qualificato: 6e6<=a<=7e6 m, 250<=1/f<=350.
///
/// I lati non unici G2 falliscono, mentre la distanza fra punti antipodali
/// rimane definita. Come nel densify, la fascia numerica delta lon>178 gradi,
/// 0<abs(lat1+lat2)<1e-12 gradi con azimut estremi diversi non e' certificata.
/// La separazione delle superfici dipende dal margine effettivo delle primitive;
/// orientamento/area o classificazione non certificabili falliscono.
/// Sui lati oltre 1000 km si richiede m12>=1024 m per limitare
/// l'amplificazione numerica vicino ai punti coniugati.
/// Per bordi quasi paralleli distanti meno di un metro Taylor non si
/// applica: la ricerca metrica puo' esaurire `max_work` (`WorkLimit`).
///
/// Nessun prodotto di cardinalita' decide l'ammissione. Ogni primitiva,
/// lettura di coordinate, visita e raffinamento controlla deadline e cancel;
/// al piu' una primitiva `GeographicLib` tra controlli, anche in validazione.
/// Risultato atomico; output scalare sullo stack, nessuna serializzazione.
///
/// # Errors
/// [`GeodesicDistanceError`] distingue CRS, dominio, struttura, ambiguita',
/// accuratezza, risorse, cancellazione e deadline.
pub fn geodesic_distance_with_limits(
    a: &Geometry,
    b: &Geometry,
    crs: &ResolvedCrs,
    limits: GeodesicDistanceLimits,
    cancelled: Option<&AtomicBool>,
) -> Result<GeodesicDistanceResult, GeodesicDistanceError> {
    // Il checkpoint precede la validazione del CRS anche se manca l'ellissoide.
    crate::noding::check_interruption(
        limits.deadline,
        cancelled,
        GeodesicDistanceError::DeadlineExceeded,
        GeodesicDistanceError::Cancelled,
    )?;
    if plenora_core::crs::validate_requirement(
        plenora_core::catalog::CrsRequirement::Geographic,
        &[crs],
    )
    .is_err()
    {
        return Err(GeodesicDistanceError::GeographicRequired);
    }
    let ellipsoid = EllissoideGeodetico::da_crs(crs)
        .map_err(|_| GeodesicDistanceError::UnsupportedEllipsoid)?;
    let parameters = ellipsoid.parametri();
    if !(6e6..=7e6).contains(&parameters.semi_major_axis_metre)
        || !(250.0..=350.0).contains(&parameters.inverse_flattening)
    {
        return Err(GeodesicDistanceError::UnsupportedEllipsoid);
    }
    let mut work = Work::new(limits, cancelled, &ellipsoid);
    let left = GeometryIndex::new(a, &mut work)?;
    let right = GeometryIndex::new(b, &mut work)?;
    let mut metres = minimum(
        left.nodes[left.root],
        right.nodes[right.root],
        &left.nodes,
        &right.nodes,
        false,
        &mut work,
    )?;
    if metres <= ABSOLUTE_ERROR_M {
        metres = 0.0;
    }
    if metres > 0.0
        && (surface::contains_any(&left, &right, &mut work)?
            || surface::contains_any(&right, &left, &mut work)?)
    {
        metres = 0.0;
    }
    work.checkpoint()?;
    Ok(GeodesicDistanceResult {
        metres,
        work: work.count,
        peak_reserved_bytes: work.peak,
    })
}

#[cfg(test)]
#[deny(clippy::disallowed_methods)]
mod tests;
