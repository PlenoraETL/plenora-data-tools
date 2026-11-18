//! Coordinate UTM WGS84 conformi a GEOMETRY-OPERATIONS-1.0 GOP-023.
//!
//! XY = longitudine/latitudine in gradi; nessun cambio implicito di datum.
//! Grammatica chiusa: `zone SP hemisphere SP easting SP northing`.
//! Zona 1..60 senza padding, emisfero N/S, componenti intere ASCII in metri
//! senza padding (1..8 cifre), un solo spazio ASCII; massimo 24 byte.
//! GOP-024 ammette solo la sintassi canonica: bande e UPS sono rifiutati.
//! Dominio diretto -80..=84, longitudine -180..=180; eccezioni Norvegia/Svalbard.
//! +180 seleziona 60, -180 seleziona 1; equatore (anche -0) emisfero N.
//! Risoluzione esplicita 1 m, arrotondamento half-even dei f64 proiettati.
//! Il testo rappresenta un punto arrotondato: nessun offset di cella.
//! Errore numerico massimo 0.01 m a terra; round-trip testuale <=0.722 m.
//! L'inversa ammette il collare di arrotondamento sui bordi del dominio:
//! distanza proiettata <=0.72 m per testo, <=0.01 m per coordinate numeriche.
//! Verifica la distanza congiunta alla regione del fuso senza spostare il punto.

#![allow(
    clippy::suboptimal_flops,
    clippy::float_cmp,
    reason = "Formule della fonte; confronti esatti identificano poli e antimeridiano"
)]

use plenora_core::crs::riproiezione::utm::{project_utm, project_utm_wgs84, GridEllipsoid};

const BANDS: &[u8; 20] = b"CDEFGHJKLMNPQRSTUVWX";

/// Coordinata completa in metri sul datum WGS84, non quantizzata.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UtmCoordinate {
    /// Zona UTM 1..60.
    pub zone: u8,
    /// Emisfero N (nord) oppure S (sud).
    pub hemisphere: char,
    /// Est in metri, incluso falso easting.
    pub easting: f64,
    /// Nord in metri, incluso falso northing.
    pub northing: f64,
}

/// Errori senza dati del chiamante, incluse le cause del core.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum UtmError {
    /// Zona non compresa in 1..60.
    #[error("invalid grid zone")]
    Zone,
    /// Emisfero diverso da N/S.
    #[error("invalid grid hemisphere")]
    Hemisphere,
    /// Coordinate non finite o fuori dominio della designazione.
    #[error("grid coordinate outside domain")]
    Domain,
    /// Risoluzione diversa da 1 metro.
    #[error("unsupported grid resolution")]
    Precision,
    /// Violazione della grammatica chiusa.
    #[error("invalid grid coordinate syntax")]
    Syntax,
    /// Testo oltre 24 byte.
    #[error("grid coordinate text too long")]
    Length,
    /// Calcolo non convergente o errore del motore di proiezione.
    #[error("grid projection failed")]
    Projection,
}

/// Converte longitudine/latitudine WGS84 in UTM.
///
/// # Errors
/// `Domain` per non finiti o fuori +/-180, -80..=84; `Projection` se il calcolo fallisce.
pub fn to_utm(lon: f64, lat: f64) -> Result<UtmCoordinate, UtmError> {
    to_utm_on(lon, lat, GridEllipsoid::Wgs84)
}

pub(crate) fn to_utm_on(
    lon: f64,
    lat: f64,
    ellipsoid: GridEllipsoid,
) -> Result<UtmCoordinate, UtmError> {
    if !lon.is_finite() || !lat.is_finite() || lon.abs() > 180.0 || lat.abs() > 90.0 {
        return Err(UtmError::Domain);
    }
    if !(-80.0..=84.0).contains(&lat) {
        return Err(UtmError::Domain);
    }
    // Confronti con bordi interi: lon+180 arrotonderebbe i vicini dei bordi.
    let mut zone = (1_u8..=60)
        .find(|z| lon < f64::from(*z) * 6.0 - 180.0)
        .unwrap_or(60);
    if (56.0..64.0).contains(&lat) && (3.0..12.0).contains(&lon) {
        zone = 32;
    } else if lat >= 72.0 && (0.0..42.0).contains(&lon) {
        zone = if lon < 9.0 {
            31
        } else if lon < 21.0 {
            33
        } else if lon < 33.0 {
            35
        } else {
            37
        };
    }
    let hemisphere = if lat >= 0.0 { 'N' } else { 'S' };
    let (easting, northing) = project_utm(ellipsoid, zone, lat >= 0.0, false, lon, lat)
        .map_err(|_| UtmError::Projection)?;
    Ok(UtmCoordinate {
        zone,
        hemisphere,
        easting,
        northing,
    })
}

fn project(zone: u8, north: bool, inverse: bool, x: f64, y: f64) -> Result<(f64, f64), UtmError> {
    project_utm_wgs84(zone, north, inverse, x, y).map_err(|_| UtmError::Projection)
}

// Limiti geografici della designazione, non dell'intero CRS proiettato.
pub(crate) fn bounds(zone: u8, band: char) -> Result<(f64, f64, f64, f64), UtmError> {
    if zone == 0 || zone > 60 {
        return Err(UtmError::Zone);
    }
    let index = BANDS
        .iter()
        .zip(0_u8..20)
        .find(|(b, _)| char::from(**b) == band)
        .map(|(_, i)| i)
        .ok_or(UtmError::Domain)?;
    let south = -80.0 + 8.0 * f64::from(index);
    let north = if band == 'X' { 84.0 } else { south + 8.0 };
    let mut west = f64::from(zone) * 6.0 - 186.0;
    let mut east = west + 6.0;
    if band == 'V' {
        if zone == 31 {
            east = 3.0;
        }
        if zone == 32 {
            west = 3.0;
        }
    }
    if band == 'X' {
        (west, east) = match zone {
            31 => (0.0, 9.0),
            33 => (9.0, 21.0),
            35 => (21.0, 33.0),
            37 => (33.0, 42.0),
            32 | 34 | 36 => return Err(UtmError::Domain),
            _ => (west, east),
        };
    }
    Ok((west, south, east, north))
}

/// Inverte una coordinata numerica completa, restituendo (lon, lat) WGS84.
///
/// # Errors
/// `Zone`, `Hemisphere`, `Domain` per designazione/coordinate invalide;
/// `Projection` per calcolo non convergente. Nessun dato negli errori.
pub fn from_utm(value: UtmCoordinate) -> Result<(f64, f64), UtmError> {
    inverse(value, 0.01)
}

fn inverse(value: UtmCoordinate, margin: f64) -> Result<(f64, f64), UtmError> {
    let UtmCoordinate {
        zone,
        hemisphere,
        easting,
        northing,
    } = value;
    if zone == 0 || zone > 60 {
        return Err(UtmError::Zone);
    }
    if !easting.is_finite() || !northing.is_finite() {
        return Err(UtmError::Domain);
    }
    let is_north = match hemisphere {
        'N' => true,
        'S' => false,
        _ => return Err(UtmError::Hemisphere),
    };
    if !(100_000.0..=900_000.0).contains(&easting) || !(0.0..=10_000_000.0).contains(&northing) {
        return Err(UtmError::Domain);
    }
    // Il core ha un dominio maggiore; gli ingressi arbitrari fuori da quello
    // sono comunque rifiuti di dominio, non errori interni della proiezione.
    let (mut lon, lat) =
        project_utm_wgs84(zone, is_north, true, easting, northing).map_err(|_| UtmError::Domain)?;
    if zone == 1 && lon > 0.0 {
        lon -= 360.0;
    }
    if zone == 60 && lon < 0.0 {
        lon += 360.0;
    }
    // Unione dei rettangoli delle bande dell'emisfero: include le eccezioni
    // senza dedurre una banda dal punto gia' arrotondato sul bordo.
    let mut closest = f64::INFINITY;
    for band in BANDS.iter().copied().map(char::from) {
        if (band >= 'N') != is_north {
            continue;
        }
        if let Ok((west, south, east, north)) = bounds(zone, band) {
            let (x, y) = project(
                zone,
                is_north,
                false,
                lon.clamp(west, east),
                lat.clamp(south, north),
            )?;
            closest = closest.min((x - easting).hypot(y - northing));
        }
    }
    if closest > margin {
        return Err(UtmError::Domain);
    }
    // Il collare dell'antimeridiano mantiene longitudini geografiche legali.
    if lon > 180.0 {
        lon -= 360.0;
    }
    if lon < -180.0 {
        lon += 360.0;
    }
    Ok((lon, lat))
}

/// Formatta WGS84 nel profilo GOP-023 con emisfero e risoluzione esplicita di 1 m.
///
/// # Errors
/// `Precision` se `resolution_m` != 1; gli altri rifiuti di `to_utm`.
pub fn format_utm(lon: f64, lat: f64, resolution_m: u8) -> Result<String, UtmError> {
    if resolution_m != 1 {
        return Err(UtmError::Precision);
    }
    let value = to_utm(lon, lat)?;
    Ok(format_coordinate(value))
}

fn format_coordinate(value: UtmCoordinate) -> String {
    format!(
        "{} {} {:.0} {:.0}",
        value.zone,
        value.hemisphere,
        value.easting.round_ties_even(),
        value.northing.round_ties_even() + 0.0
    )
}

/// Legge esclusivamente la grammatica canonica GOP-023, restituendo lon/lat.
/// Il punto e' il nodo numerico: non si aggiunge 0.5 m alle componenti.
///
/// # Errors
/// `Length` oltre 24 byte; `Syntax` per grammatica; `Zone`, `Hemisphere`, `Domain`
/// per valori invalidi; `Projection` per mancata convergenza.
pub fn parse_utm(text: &str) -> Result<(f64, f64), UtmError> {
    if text.len() > 24 {
        return Err(UtmError::Length);
    }
    if !text.is_ascii() || text.bytes().any(|b| b.is_ascii_whitespace() && b != b' ') {
        return Err(UtmError::Syntax);
    }
    let mut fields = text.split(' ');
    let zone = number(fields.next().ok_or(UtmError::Syntax)?)?;
    if zone == 0 || zone > 60 {
        return Err(UtmError::Zone);
    }
    let zone = u8::try_from(zone).map_err(|_| UtmError::Zone)?;
    let hemisphere = match fields.next().ok_or(UtmError::Syntax)? {
        "N" => 'N',
        "S" => 'S',
        _ => return Err(UtmError::Hemisphere),
    };
    let easting = number(fields.next().ok_or(UtmError::Syntax)?)?;
    let northing = number(fields.next().ok_or(UtmError::Syntax)?)?;
    if fields.next().is_some() {
        return Err(UtmError::Syntax);
    }
    inverse(
        UtmCoordinate {
            zone,
            hemisphere,
            easting: f64::from(easting),
            northing: f64::from(northing),
        },
        0.72,
    )
}

fn number(text: &str) -> Result<u32, UtmError> {
    if text.is_empty()
        || text.len() > 8
        || (text.len() > 1 && text.starts_with('0'))
        || !text.bytes().all(|b| b.is_ascii_digit())
    {
        return Err(UtmError::Syntax);
    }
    text.parse().map_err(|_| UtmError::Syntax)
}

#[cfg(test)]
#[deny(clippy::disallowed_methods)]
mod tests {
    use super::{format_coordinate, UtmCoordinate};

    #[test]
    #[deny(clippy::disallowed_methods)]
    fn half_even_components() {
        // GOP-023: i pareggi su ciascuna componente vanno all'intero pari.
        for (easting, northing, expected) in [
            (500_000.5, 4_000_001.5, "31 N 500000 4000002"),
            (500_001.5, 4_000_000.5, "31 N 500002 4000000"),
            (500_000.5_f64.next_down(), -0.0, "31 N 500000 0"),
            (500_000.5_f64.next_up(), 0.0, "31 N 500001 0"),
        ] {
            assert_eq!(
                format_coordinate(UtmCoordinate {
                    zone: 31,
                    hemisphere: 'N',
                    easting,
                    northing,
                }),
                expected
            );
        }
    }
}
