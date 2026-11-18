//! MGRS WGS84, `standard_v1`, GOP-021/023/024/025.
//!
//! Riferimento completo ASCII maiuscolo, zona UTM a due cifre oppure A/B/Y/Z,
//! due lettere di quadrato e 0..5 cifre per asse. Massimo 15 byte.
//! Troncamento alla cella contenente; nessuna trasformazione di datum.
//! Fonte: `NGA.STND.0037_2.0.0` (2014), capitolo 3 e appendici A/B/C.
//! Il rappresentante e' nel rettangolo proiettato intero, anche quando questo
//! attraversa il bordo della designazione. Quantizzazione < lato per asse;
//! errore numerico della proiezione <= 0.01 m, non accuratezza del punto sorgente.

#![allow(
    clippy::suboptimal_flops,
    clippy::float_cmp,
    reason = "Formule di griglia e confronti esatti sui bordi"
)]

use crate::{
    coordinate_utm::{bounds, to_utm_on},
    grid_text::CellRepresentative,
};
use plenora_core::crs::riproiezione::{
    ups::project_ups_wgs84,
    utm::{inverse_utm_cell, project_utm, GridEllipsoid},
};

const BANDS: &[u8] = b"CDEFGHJKLMNPQRSTUVWX";
const ROWS: &[u8] = b"ABCDEFGHJKLMNPQRSTUV";
const COLUMNS: [&[u8]; 3] = [b"ABCDEFGH", b"JKLMNPQR", b"STUVWXYZ"];

/// Errori GOP-025 senza dati del chiamante.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum MgrsError {
    /// Testo, coordinate o combinazione di designatori invalidi.
    #[error("invalid grid reference")]
    DataMapping,
    /// Precisione non compresa in 0..5.
    #[error("invalid grid precision")]
    InvalidConfiguration,
    /// Input oltre il limite dichiarato di byte.
    #[error("grid reference length limit")]
    ResourceLimit,
    /// Calcolo numerico non certificabile.
    #[error("grid projection failed")]
    Execution,
}

/// Rettangolo sorgente in metri interi; estremi superiori esclusi.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MgrsCell {
    /// 1..60 per UTM, zero per UPS.
    pub zone: u8,
    /// Banda C..X oppure zona polare A/B/Y/Z.
    pub band: char,
    /// Minimo easting, metri.
    pub west: u32,
    /// Minimo northing, metri.
    pub south: u32,
    /// Massimo easting, metri.
    pub east: u32,
    /// Massimo northing, metri.
    pub north: u32,
}

/// Cella e rappresentante WGS84 (XY longitudine/latitudine in gradi).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DecodedMgrs {
    /// Cella nel sistema proiettato sorgente WGS84.
    pub cell: MgrsCell,
    /// Politica richiesta, senza default.
    pub representative: CellRepresentative,
    /// Longitudine in gradi.
    pub longitude: f64,
    /// Latitudine in gradi.
    pub latitude: f64,
}

fn position(alphabet: &[u8], letter: u8) -> Result<u32, MgrsError> {
    alphabet
        .iter()
        .position(|b| *b == letter)
        .and_then(|i| u32::try_from(i).ok())
        .ok_or(MgrsError::DataMapping)
}

fn letter(alphabet: &[u8], index: u32) -> Result<char, MgrsError> {
    usize::try_from(index)
        .ok()
        .and_then(|i| alphabet.get(i))
        .copied()
        .map(char::from)
        .ok_or(MgrsError::DataMapping)
}

// NGA figure B-1/B-2: colonne ovest/est e origine delle righe in centinaia di km.
#[allow(
    clippy::type_complexity,
    reason = "Due alfabeti e due origini delle tabelle NGA"
)]
const fn polar(band: char) -> Result<(&'static [u8], &'static [u8], u32, u32), MgrsError> {
    match band {
        'A' => Ok((b"JKLPQRSTUXYZ", b"ABCDEFGHJKLMNPQRSTUVWXYZ", 8, 8)),
        'B' => Ok((b"ABCFGHJKLPQR", b"ABCDEFGHJKLMNPQRSTUVWXYZ", 20, 8)),
        'Y' => Ok((b"RSTUXYZ", b"ABCDEFGHJKLMNP", 13, 13)),
        'Z' => Ok((b"ABCFGHJ", b"ABCDEFGHJKLMNP", 20, 13)),
        _ => Err(MgrsError::DataMapping),
    }
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "Intero troncato verificato nel dominio u32"
)]
fn metre(value: f64) -> Result<u32, MgrsError> {
    if !value.is_finite() || !(0.0..=10_000_000.0).contains(&value) {
        return Err(MgrsError::DataMapping);
    }
    Ok(value.floor() as u32)
}

/// Formatta un punto WGS84 nella cella MGRS contenente.
/// Precisione obbligatoria 0..5 cifre per asse; nessun arrotondamento.
///
/// # Errors
/// `InvalidConfiguration` per precisione, `DataMapping` per coordinate,
/// `Execution` per proiezione non calcolabile.
pub fn format_mgrs(longitude: f64, latitude: f64, digits: u8) -> Result<String, MgrsError> {
    format_on(longitude, latitude, digits, GridEllipsoid::Wgs84)
}

pub(crate) fn format_on(
    longitude: f64,
    latitude: f64,
    digits: u8,
    ellipsoid: GridEllipsoid,
) -> Result<String, MgrsError> {
    if digits > 5 {
        return Err(MgrsError::InvalidConfiguration);
    }
    if !longitude.is_finite()
        || !latitude.is_finite()
        || longitude.abs() > 180.0
        || latitude.abs() > 90.0
    {
        return Err(MgrsError::DataMapping);
    }
    let (zone, band, x, y) = if (-80.0..=84.0).contains(&latitude) {
        let utm = to_utm_on(longitude, latitude, ellipsoid).map_err(|_| MgrsError::Execution)?;
        let band = BANDS
            .iter()
            .zip(0..20)
            .find(|(_, i)| latitude < -72.0 + 8.0 * f64::from(*i))
            .map_or('X', |(b, _)| char::from(*b));
        // La TM puo' perdere la distanza sub-nanometrica dal meridiano centrale.
        // Conserva il lato geografico quando il risultato coincide con 500 km.
        let central = f64::from(utm.zone) * 6.0 - 183.0;
        let easting = if longitude < central && utm.easting == 500_000.0 {
            utm.easting.next_down()
        } else {
            utm.easting
        };
        (utm.zone, band, easting, utm.northing)
    } else {
        if ellipsoid != GridEllipsoid::Wgs84 {
            return Err(MgrsError::InvalidConfiguration);
        }
        let (mut x, mut y) = project_ups_wgs84(latitude > 0.0, false, longitude, latitude)
            .map_err(|_| MgrsError::Execution)?;
        // La somma del falso est/nord puo' perdere il lato degli assi.
        // Un ULP conserva il troncamento senza spostamenti oltre 0.24 nm.
        if latitude.abs() != 90.0 {
            if x == 2_000_000.0 && longitude > -180.0 && longitude < 0.0 {
                x = x.next_down();
            }
            if y == 2_000_000.0
                && ((latitude > 0.0 && longitude.abs() < 90.0)
                    || (latitude < 0.0 && longitude.abs() > 90.0))
            {
                y = y.next_down();
            }
        }
        let band = match (latitude > 0.0, x < 2_000_000.0) {
            (true, true) => 'Y',
            (true, false) => 'Z',
            (false, true) => 'A',
            (false, false) => 'B',
        };
        (0, band, x, y)
    };
    // Il falso northing puo' arrotondare a 10^7 entro un ULP dall'equatore.
    // Il limite aperto sud resta nell'ultima cella, con spostamento < 2 nm.
    let northing = if zone != 0 && band < 'N' {
        metre(y)?.min(9_999_999)
    } else {
        metre(y)?
    };
    encode(zone, band, metre(x)?, northing, digits)
}

fn encode(zone: u8, band: char, x: u32, y: u32, digits: u8) -> Result<String, MgrsError> {
    let (prefix, col, row) = if zone == 0 {
        let (cols, rows, x0, y0) = polar(band)?;
        (
            band.to_string(),
            letter(
                cols,
                (x / 100_000)
                    .checked_sub(x0)
                    .ok_or(MgrsError::DataMapping)?,
            )?,
            letter(
                rows,
                (y / 100_000)
                    .checked_sub(y0)
                    .ok_or(MgrsError::DataMapping)?,
            )?,
        )
    } else {
        let cols = COLUMNS[usize::from((zone - 1) % 3)];
        (
            format!("{zone:02}{band}"),
            letter(
                cols,
                (x / 100_000).checked_sub(1).ok_or(MgrsError::DataMapping)?,
            )?,
            letter(
                ROWS,
                (y / 100_000 + if zone.is_multiple_of(2) { 5 } else { 0 }) % 20,
            )?,
        )
    };
    if digits == 0 {
        return Ok(format!("{prefix}{col}{row}"));
    }
    let unit = 10_u32.pow(u32::from(5 - digits));
    let width = usize::from(digits);
    Ok(format!(
        "{prefix}{col}{row}{:0width$}{:0width$}",
        x % 100_000 / unit,
        y % 100_000 / unit
    ))
}

fn inverse(
    cell: MgrsCell,
    x: f64,
    y: f64,
    ellipsoid: GridEllipsoid,
) -> Result<(f64, f64), MgrsError> {
    if cell.zone == 0 {
        project_ups_wgs84(cell.band >= 'Y', true, x, y)
    } else {
        inverse_utm_cell(ellipsoid, cell.zone, cell.band >= 'N', x, y)
    }
    .map_err(|_| MgrsError::Execution)
}

/// Legge esclusivamente un riferimento completo canonico, restituendo la cella.
/// Il rappresentante non viene agganciato al bordo della banda o del fuso.
///
/// # Errors
/// `ResourceLimit` oltre 15 byte; `DataMapping` per grammatica o cella
/// inesistente; `Execution` per calcolo numerico non certificabile.
pub fn parse_mgrs(
    text: &str,
    representative: CellRepresentative,
) -> Result<DecodedMgrs, MgrsError> {
    parse_on(text, representative, GridEllipsoid::Wgs84)
}

pub(crate) fn parse_on(
    text: &str,
    representative: CellRepresentative,
    ellipsoid: GridEllipsoid,
) -> Result<DecodedMgrs, MgrsError> {
    if text.len() > 15 {
        return Err(MgrsError::ResourceLimit);
    }
    if !text.is_ascii() || text.len() < 3 {
        return Err(MgrsError::DataMapping);
    }
    let bytes = text.as_bytes();
    let (zone, band, offset) = if bytes[0].is_ascii_digit() {
        if bytes.len() < 5 || !bytes[1].is_ascii_digit() {
            return Err(MgrsError::DataMapping);
        }
        let zone = (bytes[0] - b'0') * 10 + bytes[1] - b'0';
        bounds(zone, char::from(bytes[2])).map_err(|_| MgrsError::DataMapping)?;
        (zone, char::from(bytes[2]), 3)
    } else {
        (0, char::from(bytes[0]), 1)
    };
    if zone == 0 && ellipsoid != GridEllipsoid::Wgs84 {
        return Err(MgrsError::InvalidConfiguration);
    }
    let tail = &text[offset + 2..];
    if tail.len() > 10 || !tail.len().is_multiple_of(2) || !tail.bytes().all(|b| b.is_ascii_digit())
    {
        return Err(MgrsError::DataMapping);
    }
    let digits = u8::try_from(tail.len() / 2).map_err(|_| MgrsError::DataMapping)?;
    let side = 10_u32.pow(u32::from(5 - digits));
    let (ex, ny) = tail.split_at(usize::from(digits));
    let number = |s: &str| -> Result<u32, MgrsError> {
        if s.is_empty() {
            Ok(0)
        } else {
            s.parse().map_err(|_| MgrsError::DataMapping)
        }
    };
    let (col, row) = (bytes[offset], bytes[offset + 1]);
    let (x, y) = if zone == 0 {
        let (cols, rows, x0, y0) = polar(band)?;
        (
            (position(cols, col)? + x0) * 100_000,
            (position(rows, row)? + y0) * 100_000,
        )
    } else {
        (
            (position(COLUMNS[usize::from((zone - 1) % 3)], col)? + 1) * 100_000,
            ((position(ROWS, row)? + 20 - if zone.is_multiple_of(2) { 5 } else { 0 }) % 20)
                * 100_000,
        )
    };
    let mut cell = MgrsCell {
        zone,
        band,
        west: x + number(ex)? * side,
        south: y + number(ny)? * side,
        east: 0,
        north: 0,
    };
    cell.east = cell.west + side;
    cell.north = cell.south + side;
    cell = resolve_cell(cell, ellipsoid)?;
    let offset = if representative == CellRepresentative::Center {
        f64::from(side) / 2.0
    } else {
        0.0
    };
    let (longitude, latitude) = inverse(
        cell,
        f64::from(cell.west) + offset,
        f64::from(cell.south) + offset,
        ellipsoid,
    )?;
    Ok(DecodedMgrs {
        cell,
        representative,
        longitude,
        latitude,
    })
}

fn resolve_cell(mut cell: MgrsCell, ellipsoid: GridEllipsoid) -> Result<MgrsCell, MgrsError> {
    if cell.zone == 0 {
        let (_, edge) = project_ups_wgs84(
            cell.band >= 'Y',
            false,
            0.0,
            if cell.band >= 'Y' { 84.0 } else { -80.0 },
        )
        .map_err(|_| MgrsError::Execution)?;
        let radius = (edge - 2_000_000.0).abs();
        let x = 2_000_000.0_f64.clamp(f64::from(cell.west), f64::from(cell.east));
        let y = 2_000_000.0_f64.clamp(f64::from(cell.south), f64::from(cell.north));
        if (x - 2_000_000.0).hypot(y - 2_000_000.0) >= radius {
            return Err(MgrsError::DataMapping);
        }
    } else {
        let mut found = None;
        for cycle in 0..5 {
            let mut candidate = cell;
            candidate.south += cycle * 2_000_000;
            candidate.north += cycle * 2_000_000;
            if intersects(candidate, ellipsoid)? {
                if found.is_some() {
                    return Err(MgrsError::DataMapping);
                }
                found = Some(candidate);
            }
        }
        cell = found.ok_or(MgrsError::DataMapping)?;
    }
    Ok(cell)
}

fn inside(cell: MgrsCell, x: f64, y: f64) -> bool {
    x >= f64::from(cell.west)
        && x < f64::from(cell.east)
        && y >= f64::from(cell.south)
        && y < f64::from(cell.north)
}

// Il bordo del rettangolo geografico e' diviso al meridiano centrale:
// ogni arco ha entrambe le componenti proiettate monotone. L'intersezione
// degli intervalli parametrici verifica anche celle attraversate senza vertici interni.
fn intersects(cell: MgrsCell, ellipsoid: GridEllipsoid) -> Result<bool, MgrsError> {
    let (west, south, east, north) =
        bounds(cell.zone, cell.band).map_err(|_| MgrsError::DataMapping)?;
    let project = |lon, lat| {
        project_utm(ellipsoid, cell.zone, cell.band >= 'N', false, lon, lat)
            .map_err(|_| MgrsError::Execution)
    };
    let central = f64::from(cell.zone) * 6.0 - 183.0;
    let split = central.clamp(west, east);
    // Estremi del northing su paralleli e meridiano centrale.
    let low = project(split, south)?
        .1
        .min(project(west, south)?.1)
        .min(project(east, south)?.1);
    let high = project(split, north)?
        .1
        .max(project(west, north)?.1)
        .max(project(east, north)?.1);
    if f64::from(cell.north) <= low || f64::from(cell.south) > high {
        return Ok(false);
    }
    let cx = f64::from(cell.west) + f64::from(cell.east - cell.west) / 2.0;
    let cy = f64::from(cell.south) + f64::from(cell.north - cell.south) / 2.0;
    if let Ok((lon, lat)) = inverse(cell, cx, cy, ellipsoid) {
        let lon = central + (lon - central + 180.0).rem_euclid(360.0) - 180.0;
        if lon >= west && lon < east && lat >= south && lat < north {
            return Ok(true);
        }
    }
    for (a, b) in [
        ((west, south), (west, north)),
        ((east, south), (east, north)),
        ((west, south), (split, south)),
        ((split, south), (east, south)),
        ((west, north), (split, north)),
        ((split, north), (east, north)),
    ] {
        let eval = |t: f64| project(a.0 + t * (b.0 - a.0), a.1 + t * (b.1 - a.1));
        let start = eval(0.0)?;
        let end = eval(1.0)?;
        let mut lo = 0.0_f64;
        let mut hi = 1.0_f64;
        for (axis, minimum, maximum) in [(0, cell.west, cell.east), (1, cell.south, cell.north)] {
            let component = |p: (f64, f64)| if axis == 0 { p.0 } else { p.1 };
            let s = component(start);
            let e = component(end);
            if s.min(e) >= f64::from(maximum) || s.max(e) < f64::from(minimum) {
                hi = -1.0;
                break;
            }
            if s == e {
                continue;
            }
            for boundary in [f64::from(minimum), f64::from(maximum)] {
                if boundary <= s.min(e) || boundary >= s.max(e) {
                    continue;
                }
                let mut left = 0.0;
                let mut right = 1.0;
                for _ in 0..52 {
                    let middle = f64::midpoint(left, right);
                    if (component(eval(middle)?) < boundary) == (e > s) {
                        left = middle;
                    } else {
                        right = middle;
                    }
                }
                let root = f64::midpoint(left, right);
                if (boundary == f64::from(minimum)) == (e > s) {
                    lo = lo.max(root);
                } else {
                    hi = hi.min(root);
                }
            }
        }
        if lo < hi {
            let point = eval(f64::midpoint(lo, hi))?;
            let excluded =
                (a.0 == east && b.0 == east) || (north != 84.0 && a.1 == north && b.1 == north);
            if inside(cell, point.0, point.1)
                && (!excluded
                    || (point.0 > f64::from(cell.west) && point.1 > f64::from(cell.south)))
            {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

#[cfg(test)]
#[deny(clippy::disallowed_methods)]
mod tests {
    use super::{encode, metre};

    #[test]
    #[deny(clippy::disallowed_methods)]
    fn exact_square_and_digit_boundaries() {
        // NGA figura B-3 set 1, equatore: EA a 500km, FA a 600km.
        for (easting, expected) in [
            (600_000.0_f64.next_down(), "31NEA9999900000"),
            (600_000.0, "31NFA0000000000"),
            (600_000.0_f64.next_up(), "31NFA0000000000"),
            (500_009.999, "31NEA0000900000"),
            (500_010.0, "31NEA0001000000"),
        ] {
            assert_eq!(
                encode(31, 'N', metre(easting).expect("finite metres"), 0, 5).expect("exact cell"),
                expected
            );
        }
        // NGA figura B-3: il cambio di riga avviene esattamente a 100km.
        assert_eq!(
            encode(31, 'N', 500_000, 99_999, 5).expect("last metre"),
            "31NEA0000099999"
        );
        assert_eq!(
            encode(31, 'N', 500_000, 100_000, 5).expect("next square"),
            "31NEB0000000000"
        );
    }
}
