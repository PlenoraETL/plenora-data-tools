//! Proiezione ellissoidale UTM per codec di griglia, senza trasformazioni di datum.
//!
//! Riusa Krueger/Karney del core. Il collare di 0.001 gradi serve alla
//! quantizzazione testuale dei bordi; il codec verifica il proprio dominio.

use super::proiezioni::Proiezione;
use super::tabella::MetodoProiezione;
use crate::crs::{CrsError, Ellipsoid};

/// Ellissoidi ammessi dai codec di griglia; non esegue trasformazioni di datum.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GridEllipsoid {
    /// WGS84 per MGRS e UTM.
    Wgs84,
    /// GRS80 per USNG NAD83.
    Grs80,
    /// Clarke 1866 per USNG NAD27 (FGDC annesso E).
    Clarke1866,
}

impl GridEllipsoid {
    // EPSG 7030, 7019, 7008; stessi parametri del registro CRS integrato.
    const fn parameters(self) -> Ellipsoid {
        match self {
            Self::Wgs84 => Ellipsoid {
                semi_major_axis_metre: 6_378_137.0,
                inverse_flattening: 298.257_223_563,
            },
            Self::Grs80 => Ellipsoid {
                semi_major_axis_metre: 6_378_137.0,
                inverse_flattening: 298.257_222_101,
            },
            Self::Clarke1866 => Ellipsoid {
                semi_major_axis_metre: 6_378_206.4,
                inverse_flattening: 294.978_698_213_898_2,
            },
        }
    }
}

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
    inverse_utm_cell(GridEllipsoid::Wgs84, zone, north, x, y)
}

/// Variante sul solo ellissoide selezionato, senza cambio di datum.
/// Stessi limiti e garanzia numerica della variante WGS84.
///
/// # Errors
/// Rifiuta coordinate, zona o calcoli fuori dal dominio qualificato.
pub fn inverse_utm_cell(
    ellipsoid: GridEllipsoid,
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
        ellipsoid.parameters(),
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
    project_utm(GridEllipsoid::Wgs84, zone, north, inverse, x, y)
}

/// Variante sul solo ellissoide selezionato, senza cambio di datum.
/// Stessi limiti e garanzia numerica della variante WGS84.
///
/// # Errors
/// Rifiuta coordinate, zona o calcoli fuori dal dominio qualificato.
pub fn project_utm(
    ellipsoid: GridEllipsoid,
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
        ellipsoid.parameters(),
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
