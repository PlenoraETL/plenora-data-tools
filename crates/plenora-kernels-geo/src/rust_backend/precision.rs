//! La precisione dichiarata delle operazioni geografiche: **1 cm a terra**,
//! fissa, come il modello a precisione fissa di GEOS o `gridSize = 0.01` di
//! `PostGIS` in metri.
//!
//! Sotto questa soglia un risultato puo' differire dall'esatto (vertici
//! spostati, feature sottili fuse o sparite); sopra, ogni errore e'
//! esplicito. La politica e' in README («Limiti dichiarati») e in AGENTS.md,
//! regola 1.
//!
//! La precisione si esprime nelle unita' delle coordinate:
//!
//! - CRS proiettato: `0.01 / horizontal_unit_to_metre`;
//! - CRS geografico: 1 cm in gradi all'equatore, il valore piu' severo:
//!   `0.01 / 111_319.490_793_273_57` (lunghezza di un grado di longitudine
//!   all'equatore sull'ellissoide WGS84, `2 * pi * 6_378_137 / 360`), circa
//!   `8.98e-8` gradi.
//!
//! Le funzioni dei kernel chiamate senza CRS ricevono la precisione come
//! argomento esplicito: nessun valore predefinito.

use plenora_core::crs::{CrsKind, ResolvedCrs};

use super::RustBackendError;

/// Un centimetro, in metri: la precisione dichiarata a terra.
pub const PRECISIONE_IN_METRI: f64 = 0.01;

/// Metri per grado di longitudine all'equatore sull'ellissoide WGS84.
pub const METRI_PER_GRADO_ALL_EQUATORE: f64 = 111_319.490_793_273_57;

/// La precisione dichiarata, nelle unita' delle coordinate: un numero finito
/// e positivo per costruzione.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Precision(f64);

impl Precision {
    /// Una precisione esplicita nelle unita' delle coordinate.
    ///
    /// # Errors
    ///
    /// [`RustBackendError::InvalidPrecision`] se il valore non e' finito e
    /// positivo.
    pub fn new(value: f64) -> Result<Self, RustBackendError> {
        if value.is_finite() && value > 0.0 {
            Ok(Self(value))
        } else {
            Err(RustBackendError::InvalidPrecision)
        }
    }

    /// 1 cm a terra nelle unita' delle coordinate del CRS.
    ///
    /// # Errors
    ///
    /// [`RustBackendError::InvalidPrecision`] se un CRS proiettato non ha
    /// un'unita' lineare finita e positiva.
    pub fn from_crs(crs: &ResolvedCrs) -> Result<Self, RustBackendError> {
        match crs.kind() {
            CrsKind::Projected => match crs.horizontal_unit_to_metre() {
                Some(unit) if unit.is_finite() && unit > 0.0 => {
                    Self::new(PRECISIONE_IN_METRI / unit)
                }
                _ => Err(RustBackendError::InvalidPrecision),
            },
            CrsKind::Geographic => Self::new(PRECISIONE_IN_METRI / METRI_PER_GRADO_ALL_EQUATORE),
        }
    }

    /// Il valore nelle unita' delle coordinate.
    #[must_use]
    pub const fn value(self) -> f64 {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use plenora_core::crs::CrsKind;
    use serde_json::json;

    fn crs(kind: CrsKind, unit: Option<f64>) -> ResolvedCrs {
        ResolvedCrs::from_resolved_parts("EPSG:0".to_owned(), json!({}), kind, unit)
    }

    #[test]
    fn un_centimetro_nelle_unita_del_crs() {
        let metri = Precision::from_crs(&crs(CrsKind::Projected, Some(1.0))).unwrap();
        assert!((metri.value() - 0.01).abs() < 1e-18);
        let piedi = Precision::from_crs(&crs(CrsKind::Projected, Some(0.3048))).unwrap();
        assert!((piedi.value() - 0.01 / 0.3048).abs() < 1e-17);
        let gradi = Precision::from_crs(&crs(CrsKind::Geographic, None)).unwrap();
        assert!((gradi.value() - 8.983_152_841_195_214e-8).abs() < 1e-19);
    }

    #[test]
    fn senza_unita_lineare_o_non_positiva_e_un_errore() {
        assert!(Precision::from_crs(&crs(CrsKind::Projected, None)).is_err());
        assert!(Precision::from_crs(&crs(CrsKind::Projected, Some(0.0))).is_err());
        for value in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            assert!(Precision::new(value).is_err());
        }
    }
}
