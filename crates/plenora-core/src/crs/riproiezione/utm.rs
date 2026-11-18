//! Proiezione WGS84 UTM per codec di griglia, senza trasformazioni di datum.
//!
//! Riusa Krueger/Karney del core. Il collare di 0.001 gradi serve alla
//! quantizzazione testuale dei bordi; il codec verifica il proprio dominio.

use super::proiezioni::Proiezione;
use super::tabella::MetodoProiezione;
use crate::crs::{CrsError, Ellipsoid};

/// Inversa delle celle MGRS WGS84, compresi gli angoli esterni al fuso.
///
/// Riusa la TM del core; non estende il dominio del codec UTM di punti.
/// Il rettangolo della cella deve essere validato dal chiamante.
/// Errore numerico entro 1 cm nel dominio limitato qui sotto.
///
/// # Errors
/// `ReprojectionNotConverged` per zona, coordinate o dominio non validi.
pub fn inverse_utm_cell_wgs84(
    zone: u8,
    north: bool,
    x: f64,
    y: f64,
) -> Result<(f64, f64), CrsError> {
    let failure = || CrsError::ReprojectionNotConverged;
    if !(1..=60).contains(&zone)
        || !x.is_finite()
        || !y.is_finite()
        || !(100_000.0..=900_000.0).contains(&x)
        || !(-100_000.0..=10_100_000.0).contains(&y)
    {
        return Err(failure());
    }
    let central = f64::from(zone).mul_add(6.0, -183.0);
    let projection = Proiezione::nuova(
        &MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: central,
            k0: 0.9996,
            falsa_est: 500_000.0,
            falsa_nord: if north { 0.0 } else { 10_000_000.0 },
        },
        Ellipsoid {
            semi_major_axis_metre: 6_378_137.0,
            inverse_flattening: 298.257_223_563,
        },
    )
    .map_err(|_| failure())?;
    let (lon, lat) = projection.indietro(x, y).map_err(|_| failure())?;
    let delta = (lon - central + 180.0).rem_euclid(360.0) - 180.0;
    if lat.abs() > 85.0 || delta.abs() > 18.0 {
        return Err(failure());
    }
    Ok((lon, lat))
}

/// Proietta lon/lat (gradi) o inverte easting/northing (metri) in WGS84.
///
/// Zona 1..60; distanza dal meridiano centrale <= 9 gradi, latitudine
/// -80.001..84.001 con collare di 0.001 gradi all'equatore. Errore numerico
/// entro 1 cm nel dominio, indipendente dalla quantizzazione del chiamante.
/// L'inversa richiede inoltre E in [0, 1000000] e N in [-1, 10000001] metri;
/// il dominio diretto del metodo e' piu' ampio del dominio di un codec UTM.
///
/// # Errors
/// `ReprojectionConfig` per zona invalida; `ReprojectionNotConverged`
/// per coordinate non finite, fuori dominio o proiezione non calcolabile.
pub fn project_utm_wgs84(
    zone: u8,
    north: bool,
    inverse: bool,
    x: f64,
    y: f64,
) -> Result<(f64, f64), CrsError> {
    if !(1..=60).contains(&zone) {
        return Err(CrsError::ReprojectionConfig("invalid UTM zone"));
    }
    let failure = || CrsError::ReprojectionNotConverged;
    if !x.is_finite() || !y.is_finite() {
        return Err(failure());
    }
    let central = f64::from(zone).mul_add(6.0, -183.0);
    let projection = Proiezione::nuova(
        &MetodoProiezione::TrasversaDiMercatore {
            lat0: 0.0,
            lon0: central,
            k0: 0.9996,
            falsa_est: 500_000.0,
            falsa_nord: if north { 0.0 } else { 10_000_000.0 },
        },
        Ellipsoid {
            semi_major_axis_metre: 6_378_137.0,
            inverse_flattening: 298.257_223_563,
        },
    )
    .map_err(|_| failure())?;
    let (lon, lat) = if inverse {
        if !(0.0..=1_000_000.0).contains(&x) || !(-1.0..=10_000_001.0).contains(&y) {
            return Err(failure());
        }
        projection.indietro(x, y).map_err(|_| failure())?
    } else {
        (x, y)
    };
    let delta = (lon - central + 180.0).rem_euclid(360.0) - 180.0;
    let latitude = if north {
        -0.001..=84.001
    } else {
        -80.001..=0.001
    };
    if !latitude.contains(&lat) || delta.abs() > 9.0 || lon.abs() > 180.0 {
        return Err(failure());
    }
    if inverse {
        Ok((lon, lat))
    } else {
        projection.avanti(lon, lat).map_err(|_| failure())
    }
}
