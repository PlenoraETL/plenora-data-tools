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
    /// Nessun certificato: il consumatore deve rifiutare la misura.
    Unproven {
        /// Codice EPSG del metodo.
        method: u16,
        /// Motivo statico, senza dati dell'ingresso.
        reason: &'static str,
    },
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
            };
            found.insert(method);
        }
        assert_eq!(found, [0, 1024, 9802, 9804, 9807, 9809, 9815, 9820].into());
    }
}
