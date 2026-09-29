//! La precisione dichiarata delle operazioni geografiche: **1 cm a terra**,
//! fissa, come il modello a precisione fissa di GEOS o `gridSize = 0.01` di
//! `PostGIS` in metri.
//!
//! Sotto questa soglia un risultato puo' differire dall'esatto (vertici
//! spostati, feature sottili fuse o sparite); sopra, ogni errore e'
//! esplicito. La politica e' in README («Limiti dichiarati») e in AGENTS.md,
//! regola 1.
//!
//! La precisione si esprime nelle unita' delle coordinate, con la sola
//! funzione del workspace che la calcola,
//! [`ResolvedCrs::precisione_coordinate`]:
//!
//! - CRS proiettato: `0.01 / horizontal_unit_to_metre`;
//! - CRS geografico: 1 cm in gradi all'equatore, il valore piu' severo:
//!   `0.01 / 111_319.49`, circa `8.98e-8` gradi.
//!
//! Le funzioni dei kernel chiamate senza CRS ricevono la precisione come
//! argomento esplicito: nessun valore predefinito.

use plenora_core::crs::ResolvedCrs;

use super::RustBackendError;

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

    /// 1 cm a terra nelle unita' delle coordinate del CRS: delega a
    /// [`ResolvedCrs::precisione_coordinate`], nessuna copia dei valori.
    ///
    /// # Errors
    ///
    /// [`RustBackendError::InvalidPrecision`] se il CRS non ha una
    /// precisione (proiettato senza un'unita' lineare finita e positiva, o
    /// quoziente che non e' un `f64` normale e positivo).
    pub fn from_crs(crs: &ResolvedCrs) -> Result<Self, RustBackendError> {
        crs.precisione_coordinate()
            .map_or(Err(RustBackendError::InvalidPrecision), Self::new)
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
        assert!((gradi.value() - 0.01 / 111_319.49).abs() < 1e-22);
        // Stesso valore del core, bit per bit: una sola fonte.
        for (kind, unit) in [
            (CrsKind::Projected, Some(1.0)),
            (CrsKind::Projected, Some(0.3048)),
            (CrsKind::Geographic, None),
        ] {
            let resolved = crs(kind, unit);
            assert_eq!(
                Precision::from_crs(&resolved).unwrap().value().to_bits(),
                resolved.precisione_coordinate().unwrap().to_bits()
            );
        }
    }

    #[test]
    fn senza_unita_lineare_o_non_positiva_e_un_errore() {
        assert!(Precision::from_crs(&crs(CrsKind::Projected, None)).is_err());
        assert!(Precision::from_crs(&crs(CrsKind::Projected, Some(0.0))).is_err());
        assert!(Precision::from_crs(&crs(CrsKind::Projected, Some(f64::from_bits(1)))).is_err());
        for value in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            assert!(Precision::new(value).is_err());
        }
    }
}
