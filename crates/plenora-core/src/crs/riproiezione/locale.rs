//! Proiezione di lavoro locale, senza costruire un CRS o cambiare datum.

use super::{errore_proiezione, tabella::MetodoProiezione, Proiezione, TransverseShapeBounds};
use crate::crs::{CrsError, CrsKind, ResolvedCrs};

/// TM del motore con scala centrale unitaria e false origini nulle.
/// Il chiamante deve certificare numericamente le conversioni e il dominio
/// continuo prima di usarle per un'operazione metrica.
#[derive(Clone, Debug)]
pub struct LocalTransverseMercator {
    projection: Proiezione,
    bounds: TransverseShapeBounds,
}

impl LocalTransverseMercator {
    /// Conserva l'ellissoide del CRS geografico ricevuto.
    /// # Errors
    /// CRS non geografico, parametri non terrestri o centro fuori profilo.
    pub fn new(crs: &ResolvedCrs, longitude: f64) -> Result<Self, CrsError> {
        let ellipsoid = crs.ellipsoid().ok_or(CrsError::ReprojectionConfig(
            "ellissoide locale non disponibile",
        ))?;
        if crs.kind() != CrsKind::Geographic
            || !longitude.is_finite()
            || longitude.abs() >= 140.0
            || !(6e6..=7e6).contains(&ellipsoid.semi_major_axis_metre)
            || !(250.0..=350.0).contains(&ellipsoid.inverse_flattening)
        {
            return Err(CrsError::ReprojectionConfig(
                "proiezione locale fuori profilo",
            ));
        }
        let bounds = TransverseShapeBounds {
            semi_major_axis: ellipsoid.semi_major_axis_metre,
            central_scale: 1.0,
            latitude_origin: 0.0,
            longitude_origin: longitude,
            false_easting: 0.0,
            false_northing: 0.0,
        };
        let projection = Proiezione::nuova(
            &MetodoProiezione::TrasversaDiMercatore {
                lat0: 0.0,
                lon0: longitude,
                k0: 1.0,
                falsa_est: 0.0,
                falsa_nord: 0.0,
            },
            ellipsoid,
        )
        .map_err(errore_proiezione)?;
        Ok(Self { projection, bounds })
    }

    /// Parametri originali per il certificato indipendente dell'inversa.
    #[must_use]
    pub const fn inverse_shape_bounds(&self) -> TransverseShapeBounds {
        self.bounds
    }

    /// Lon/lat in gradi sul ramo locale, senza riduzione attraverso il seam.
    /// # Errors
    /// Coordinate non finite o fuori dal ramo locale.
    pub fn forward(&self, longitude: f64, latitude: f64) -> Result<(f64, f64), CrsError> {
        if !longitude.is_finite()
            || !latitude.is_finite()
            || longitude.abs() >= 180.0
            || latitude.abs() >= 63.0
            || (longitude - self.bounds.longitude_origin).abs() >= 10.0
        {
            return Err(CrsError::ReprojectionConfig(
                "coordinate fuori dal ramo locale",
            ));
        }
        self.projection
            .avanti(longitude, latitude)
            .map_err(errore_proiezione)
    }

    /// Inversa numerica: il certificato residuale resta obbligo del chiamante.
    /// # Errors
    /// Coordinate non finite o esterne al rettangolo TM qualificato.
    pub fn inverse(&self, x: f64, y: f64) -> Result<(f64, f64), CrsError> {
        if !x.is_finite()
            || !y.is_finite()
            || (x / self.bounds.semi_major_axis).abs() >= 0.1
            || (y / self.bounds.semi_major_axis).abs() >= 1.1
        {
            return Err(CrsError::ReprojectionConfig(
                "coordinate fuori dal ramo locale",
            ));
        }
        self.projection.indietro(x, y).map_err(errore_proiezione)
    }
}
