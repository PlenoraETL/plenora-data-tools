//! Contratto comune dei maggioranti; dimostrazioni in `docs/maggioranti-forma.md`.

/// Inversa separabile: lambda affine in x, phi funzione della sola y.
/// Valida sul dominio del CRS integrato, prima della riduzione di longitudine.
#[derive(Clone, Copy, Debug)]
pub struct SeparableShapeBounds {
    /// Codice EPSG del metodo; zero indica coordinate geografiche.
    pub method: u16,
    /// Scala s: lambda'=s dx, psi'=s dy, per un segmento parametrizzato.
    pub angular_scale: f64,
    /// Termine costante di `lambda=s*x+longitude_offset`, in radianti.
    pub longitude_offset: f64,
    /// Divisore proiettato a*k0, arrotondato una volta; 1 per geografico.
    pub projected_divisor: f64,
    /// Falsa ordinata del metodo, esatta nella rappresentazione del registro.
    pub false_northing: f64,
    /// Maggiorante di |d phi / d psi|.
    pub latitude_first: f64,
    /// Maggiorante di |d² phi / d psi²|.
    pub latitude_second: f64,
}

/// Qualificazione analitica distinta dalla sola disponibilita' dell'inversa.
#[derive(Clone, Copy, Debug)]
pub enum InverseShapeCertificate {
    /// Maggioranti per la regola comune di composizione della catena.
    Separable(SeparableShapeBounds),
    /// TM esatta sul rettangolo normalizzato dichiarato in maggioranti-forma.
    TransverseMercator(TransverseShapeBounds),
    /// Nessun certificato: il consumatore deve rifiutare la misura.
    Unproven {
        /// Codice EPSG del metodo.
        method: u16,
        /// Motivo statico, senza dati dell'ingresso.
        reason: &'static str,
    },
}

/// Parametri esatti del registro per il certificato residuale TM EPSG 9807.
#[derive(Clone, Copy, Debug)]
pub struct TransverseShapeBounds {
    /// Semiasse maggiore in metri.
    pub semi_major_axis: f64,
    /// Scala al meridiano centrale.
    pub central_scale: f64,
    /// Latitudine dell'origine in gradi.
    pub latitude_origin: f64,
    /// Longitudine del meridiano centrale in gradi.
    pub longitude_origin: f64,
    /// Falsa ascissa in metri.
    pub false_easting: f64,
    /// Falsa ordinata in metri.
    pub false_northing: f64,
}

impl TransverseShapeBounds {
    /// |d lambda/dt| <= 3 |dz/dt|, z normalizzato da a*k0.
    pub const LONGITUDE_FIRST: f64 = 3.0;
    /// |d2 lambda/dt2| <= 9 |dz/dt|^2 lungo una retta.
    pub const LONGITUDE_SECOND: f64 = 9.0;
    /// |d phi/dt| <= 4 |dz/dt|.
    pub const LATITUDE_FIRST: f64 = 4.0;
    /// |d2 phi/dt2| <= 20 |dz/dt|^2 lungo una retta.
    pub const LATITUDE_SECOND: f64 = 20.0;
}

#[cfg(test)]
#[deny(clippy::disallowed_methods)]
mod tests {
    use super::*;
    use crate::crs::riproiezione::{epsg, proiezioni::Proiezione, tabella};

    // C1: EPSG GN 7-2, metodi della tabella integrata; enumerazione indipendente.
    #[test]
    #[deny(clippy::disallowed_methods)]
    fn every_invertible_method_has_an_explicit_certificate_entry() {
        let mut found = std::collections::BTreeSet::new();
        for definition in epsg::DEFINIZIONI {
            let datum = tabella::datum(definition.datum).unwrap();
            let projection = Proiezione::nuova(&definition.metodo, datum.ellissoide).unwrap();
            let method = match projection.shape_certificate() {
                InverseShapeCertificate::Separable(bounds) => {
                    assert!(bounds.angular_scale > 0.0);
                    assert!(bounds.latitude_first >= 1.0);
                    assert!(bounds.latitude_second >= 0.0);
                    bounds.method
                }
                InverseShapeCertificate::Unproven { method, reason } => {
                    assert!(!reason.is_empty());
                    method
                }
                InverseShapeCertificate::TransverseMercator(bounds) => {
                    assert!(bounds.central_scale > 0.0);
                    9807
                }
            };
            found.insert(method);
        }
        assert_eq!(found, [0, 1024, 9802, 9804, 9807, 9809, 9815, 9820].into());
    }
}
