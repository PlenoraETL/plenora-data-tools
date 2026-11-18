//! Densificazione di percorsi e anelli sul datum del CRS geografico.

#![allow(
    clippy::float_cmp,
    reason = "Identita geometrica e casi di simmetria esatti G2"
)]

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use geo::{Coord, LineString};
use geographiclib_rs::{DirectGeodesic, InverseGeodesic};
use plenora_core::crs::{CrsKind, ResolvedCrs};
use thiserror::Error;

use crate::geodetica::EllissoideGeodetico;

mod geometry;
mod validation;
pub use geometry::{geodesic_densify_geometry_with_limits, GeodesicGeometryDensifyLimits};

/// Limiti cumulativi dell'output, verificati prima delle prenotazioni.
#[derive(Clone, Copy, Debug)]
pub struct GeodesicDensifyLimits {
    /// Include tutti i vertici originali, ripetizioni e chiusure.
    pub max_points: u64,
    /// Capacita' richiesta dei vettori di parti e coordinate, in byte.
    /// Esclude input, stack e overhead dell'allocatore.
    pub max_reserved_bytes: u64,
    /// Scadenza monotona; prevale sull'annullamento allo stesso checkpoint.
    pub deadline: Option<Instant>,
}

/// Fallimenti senza coordinate o valori dell'ingresso.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum GeodesicDensifyError {
    /// Tipo fuori dal dominio GOP-007 (`unsupported`).
    #[error("tipo geometrico non supportato")]
    UnsupportedGeometry,
    /// Struttura o relazione di contenimento non valida (`data_mapping`).
    #[error("superficie geografica non valida")]
    InvalidGeometry,
    /// Contatti, incroci o complemento ambiguo (`unsupported`).
    #[error("superficie geodetica non supportata")]
    UnsupportedSurface,
    /// Lavoro di validazione esaurito (`resource_limit`).
    #[error("limite di lavoro geodetico superato")]
    WorkLimit,
    /// Il CRS non e' geografico.
    #[error("densify geodetico richiede un CRS geografico")]
    GeographicRequired,
    /// Parametri dell'ellissoide fuori dal dominio numerico qualificato.
    #[error("ellissoide geodetico non supportato")]
    UnsupportedEllipsoid,
    /// Passo non positivo o non finito.
    #[error("lunghezza massima non valida")]
    InvalidMaximum,
    /// Coordinate fuori dal dominio lon/lat o non finite.
    #[error("coordinate geografiche non valide")]
    InvalidCoordinate,
    /// Piu' geodetiche minime, secondo G2 Multiple shortest geodesics.
    #[error("geodetica minima ambigua")]
    AmbiguousGeodesic,
    /// Budget punti o rappresentazione del numero di divisioni esauriti.
    #[error("limite dei punti geodetici superato")]
    PointLimit,
    /// Budget delle capacita' richieste esaurito.
    #[error("limite di memoria geodetica superato")]
    MemoryLimit,
    /// L'allocatore ha rifiutato la prenotazione.
    #[error("allocazione geodetica fallita")]
    Allocation,
    /// Precisione o lunghezza delle divisioni non certificabile.
    #[error("GEOMETRY_ACCURACY_UNPROVEN")]
    AccuracyUnproven,
    /// Annullamento cooperativo.
    #[error("densify geodetico annullato")]
    Cancelled,
    /// Scadenza raggiunta.
    #[error("scadenza del densify geodetico raggiunta")]
    DeadlineExceeded,
}

// G1, sezione 7 e G2 Background: errore delle primitive terrestri <15 nm.
// Un micrometro copre anche conversioni angolari e composizione direct/inverse.
const ROUND_OFF_M: f64 = 1e-6;

struct DensifyWork<'a> {
    limits: GeodesicDensifyLimits,
    cancelled: Option<&'a AtomicBool>,
    points: u64,
    bytes: u64,
}

impl DensifyWork<'_> {
    fn checkpoint(&self) -> Result<(), GeodesicDensifyError> {
        if self
            .limits
            .deadline
            .is_some_and(|end| Instant::now() >= end)
        {
            return Err(GeodesicDensifyError::DeadlineExceeded);
        }
        if self
            .cancelled
            .is_some_and(|flag| flag.load(Ordering::Relaxed))
        {
            return Err(GeodesicDensifyError::Cancelled);
        }
        Ok(())
    }

    fn charge(&mut self, points: u64, bytes: u64) -> Result<(), GeodesicDensifyError> {
        self.points = self
            .points
            .checked_add(points)
            .ok_or(GeodesicDensifyError::PointLimit)?;
        if self.points > self.limits.max_points {
            return Err(GeodesicDensifyError::PointLimit);
        }
        self.bytes = self
            .bytes
            .checked_add(bytes)
            .ok_or(GeodesicDensifyError::MemoryLimit)?;
        if self.bytes > self.limits.max_reserved_bytes {
            return Err(GeodesicDensifyError::MemoryLimit);
        }
        Ok(())
    }
}

fn coordinate(point: Coord<f64>) -> Result<(), GeodesicDensifyError> {
    if !((-180.0..=180.0).contains(&point.x) && (-90.0..=90.0).contains(&point.y)) {
        return Err(GeodesicDensifyError::InvalidCoordinate);
    }
    Ok(())
}

fn coincident(start: Coord<f64>, end: Coord<f64>) -> bool {
    start.y == end.y
        && (start.x == end.x
            || start.y.abs() == 90.0
            || (start.x == 180.0 && end.x == -180.0)
            || (start.x == -180.0 && end.x == 180.0))
}

fn canonical_coordinate(latitude: f64, longitude: f64) -> Coord<f64> {
    Coord {
        x: if latitude.abs() == 90.0 {
            0.0
        } else if longitude == 180.0 {
            -180.0
        } else {
            longitude
        },
        y: latitude,
    }
}

struct GeodesicEdge {
    length: f64,
    azimuth: f64,
    divisions: u32,
}

fn edge(
    start: Coord<f64>,
    end: Coord<f64>,
    ellipsoid: &EllissoideGeodetico,
    maximum: f64,
) -> Result<GeodesicEdge, GeodesicDensifyError> {
    coordinate(start)?;
    coordinate(end)?;
    let (length, azimuth, final_azimuth, _): (f64, f64, f64, f64) = ellipsoid
        .geodetica()
        .inverse(start.y, start.x, end.y, end.x);
    if !(length.is_finite() && azimuth.is_finite() && final_azimuth.is_finite()) {
        return Err(GeodesicDensifyError::AccuracyUnproven);
    }
    // G2 Multiple shortest geodesics: ellissoide oblato, latitudini opposte
    // con azimut diversi; poli opposti. I coincidenti restano invariati.
    if length > 0.0 && start.y == -end.y && (start.y.abs() == 90.0 || azimuth != final_azimuth) {
        return Err(GeodesicDensifyError::AmbiguousGeodesic);
    }
    // La scelta del ramo non e' certificabile quando AngRound puo' cancellare
    // la perturbazione dalla simmetria antipodale (G1 sezione 5).
    let longitude_gap = (end.x - start.x).abs();
    let longitude_gap = longitude_gap.min(360.0 - longitude_gap);
    if longitude_gap > 178.0
        && (start.y + end.y).abs() < 1e-12
        && start.y != -end.y
        && azimuth != final_azimuth
    {
        return Err(GeodesicDensifyError::AccuracyUnproven);
    }
    if length == 0.0 && !coincident(start, end) {
        return Err(GeodesicDensifyError::AccuracyUnproven);
    }
    let divisions = if length == 0.0 || length + ROUND_OFF_M <= maximum {
        1
    } else {
        if (length / maximum).ceil() > f64::from(u32::MAX) {
            return Err(GeodesicDensifyError::PointLimit);
        }
        let available = 4.0_f64.mul_add(-ROUND_OFF_M, maximum);
        if available <= 0.0 {
            return Err(GeodesicDensifyError::AccuracyUnproven);
        }
        // Salto al primo intero compatibile con il certificato, poi verifica.
        let count = (length / available).ceil().max(2.0);
        if !count.is_finite() || count > f64::from(u32::MAX) {
            return Err(GeodesicDensifyError::PointLimit);
        }
        // Il controllo precedente rende esatta la conversione intera.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let mut divisions = count as u32;
        loop {
            if 4.0_f64.mul_add(ROUND_OFF_M, length / f64::from(divisions)) <= maximum {
                break;
            }
            divisions = divisions
                .checked_add(1)
                .ok_or(GeodesicDensifyError::PointLimit)?;
        }
        divisions
    };
    Ok(GeodesicEdge {
        length,
        azimuth,
        divisions,
    })
}

/// Densifica parti lineari e anelli espliciti lungo le geodetiche minime.
///
/// XY e' longitudine/latitudine in gradi; il passo e' in metri sul datum
/// del CRS, senza riproiezione. Ogni parte e' indipendente; un anello e'
/// una `LineString` gia' chiusa. Preserva esattamente ordine, vertici,
/// duplicati, chiusura e parti vuote. Non interpreta le parti come superfici.
///
/// Per lunghezza L, parte da n=max(1,ceil(L/h)); le divisioni sono a i*L/n
/// dal primo estremo. Se il margine numerico di 4 micrometri non permette
/// di certificare il massimo, aumenta n fino a soddisfare il certificato. Un lato entro un micrometro
/// dalla soglia riceve almeno due divisioni. Ogni tratto prodotto viene
/// verificato con inverse e margine di un micrometro; se non certificabile,
/// fallisce atomicamente. Il passo non e' un limite all'errore della forma.
/// La precisione posizionale dichiarata e' 1 cm; nessun errore di proiezione.
///
/// Longitudini inserite in [-180,180), poli inseriti con longitudine zero;
/// nessun taglio al seam. I consumatori devono interpretare archi geodetici.
/// Rifiuta i casi non unici G2. Rifiuta con `AccuracyUnproven` anche la
/// fascia numerica con delta longitudine >178 gradi, somma delle latitudini
/// non nulla ma inferiore in modulo a 1e-12 gradi e azimut estremi diversi:
/// il ramo dei quasi-antipodi unici non e' certificabile in quella fascia.
/// Dominio qualificato: 6e6<=a<=7e6 metri e 250<=1/f<=350 (ellissoidi terrestri).
///
/// Senza piano temporaneo: conta prima tutto l'output, poi per ciascuna
/// parte ricalcola la capacita', prenota ed emette. Massimo `u32::MAX` divisioni per lato. Nessun
/// prodotto fra cardinalita' decide l'ammissione. Controlla cancellazione
/// e deadline all'ingresso, per parte, per lato e per punto emesso.
///
/// # Errors
/// Errori tipizzati di dominio, ambiguita', risorse, precisione e interruzione
/// in [`GeodesicDensifyError`]. Un errore non restituisce un prefisso.
pub fn geodesic_densify_with_limits(
    parts: &[LineString<f64>],
    crs: &ResolvedCrs,
    maximum_m: f64,
    limits: GeodesicDensifyLimits,
    cancelled: Option<&AtomicBool>,
) -> Result<Vec<LineString<f64>>, GeodesicDensifyError> {
    let mut work = DensifyWork {
        limits,
        cancelled,
        points: 0,
        bytes: 0,
    };
    work.checkpoint()?;
    if crs.kind() != CrsKind::Geographic {
        return Err(GeodesicDensifyError::GeographicRequired);
    }
    if !(maximum_m.is_finite() && maximum_m > 0.0) {
        return Err(GeodesicDensifyError::InvalidMaximum);
    }
    let ellipsoid =
        EllissoideGeodetico::da_crs(crs).map_err(|_| GeodesicDensifyError::UnsupportedEllipsoid)?;
    let parameters = ellipsoid.parametri();
    if !(6e6..=7e6).contains(&parameters.semi_major_axis_metre)
        || !(250.0..=350.0).contains(&parameters.inverse_flattening)
    {
        return Err(GeodesicDensifyError::UnsupportedEllipsoid);
    }
    for part in parts {
        work.checkpoint()?;
        work.charge(0, size_of::<LineString<f64>>() as u64)?;
        if let Some(&first) = part.0.first() {
            coordinate(first)?;
            work.charge(1, size_of::<Coord<f64>>() as u64)?;
        }
        for pair in part.0.windows(2) {
            work.checkpoint()?;
            let segment = edge(pair[0], pair[1], &ellipsoid, maximum_m)?;
            let count = u64::from(segment.divisions);
            work.charge(count, count * size_of::<Coord<f64>>() as u64)?;
        }
    }
    let mut output = Vec::new();
    output
        .try_reserve_exact(parts.len())
        .map_err(|_| GeodesicDensifyError::Allocation)?;
    for part in parts {
        work.checkpoint()?;
        output.push(emit_part(part, &ellipsoid, maximum_m, &work)?);
    }
    work.checkpoint()?;
    Ok(output)
}

#[cfg(test)]
#[deny(clippy::disallowed_methods)]
mod tests;

#[cfg(test)]
#[deny(clippy::disallowed_methods)]
mod geometry_tests;

fn emit_part(
    part: &LineString<f64>,
    ellipsoid: &EllissoideGeodetico,
    maximum_m: f64,
    work: &DensifyWork<'_>,
) -> Result<LineString<f64>, GeodesicDensifyError> {
    let mut count = usize::from(!part.0.is_empty());
    for pair in part.0.windows(2) {
        work.checkpoint()?;
        let divisions = edge(pair[0], pair[1], ellipsoid, maximum_m)?.divisions;
        count = count
            .checked_add(usize::try_from(divisions).map_err(|_| GeodesicDensifyError::PointLimit)?)
            .ok_or(GeodesicDensifyError::PointLimit)?;
    }
    let mut points = Vec::new();
    points
        .try_reserve_exact(count)
        .map_err(|_| GeodesicDensifyError::Allocation)?;
    if let Some(&first) = part.0.first() {
        points.push(first);
    }
    for pair in part.0.windows(2) {
        work.checkpoint()?;
        let segment = edge(pair[0], pair[1], ellipsoid, maximum_m)?;
        let mut previous = pair[0];
        for i in 1..=segment.divisions {
            work.checkpoint()?;
            let point = if i == segment.divisions {
                pair[1]
            } else {
                let (latitude, longitude): (f64, f64) = ellipsoid.geodetica().direct(
                    pair[0].y,
                    pair[0].x,
                    segment.azimuth,
                    segment.length * (f64::from(i) / f64::from(segment.divisions)),
                );
                canonical_coordinate(latitude, longitude)
            };
            coordinate(point)?;
            let distance: f64 = ellipsoid
                .geodetica()
                .inverse(previous.y, previous.x, point.y, point.x);
            if !distance.is_finite()
                || (!coincident(previous, point) && distance + ROUND_OFF_M > maximum_m)
            {
                return Err(GeodesicDensifyError::AccuracyUnproven);
            }
            points.push(point);
            previous = point;
        }
    }

    Ok(LineString(points))
}
