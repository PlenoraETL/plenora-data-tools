//! La precisione dichiarata delle operazioni geografiche: **1 cm a terra**,
//! fissa, come il modello a precisione fissa di GEOS o `gridSize = 0.01` di
//! `PostGIS` in metri.
//!
//! Sotto questa soglia un risultato puo' differire dall'esatto (vertici
//! spostati, feature sottili fuse o sparite); sopra, ogni errore e'
//! esplicito. La politica e' in docs/limiti.md («Limiti dichiarati») e in AGENTS.md,
//! regola 1.
//!
//! La precisione si esprime nelle unita' delle coordinate, con la sola
//! funzione del workspace che la calcola,
//! [`ResolvedCrs::precisione_coordinate`]:
//!
//! - CRS proiettato: `0.01 / horizontal_unit_to_metre`;
//! - CRS geografico: 1 cm in gradi sul raggio di curvatura massimo
//!   dell'ellissoide del datum, `a / (1 - f)` (ai poli): al piu' 1 cm a
//!   terra ovunque e in entrambe le direzioni; per WGS 84 circa
//!   `8.953e-8` gradi; senza ellissoide (CRS risolto dal chiamante) nessuna
//!   precisione, e il kernel che la chiede si rifiuta.
//!
//! Le funzioni dei kernel chiamate senza CRS ricevono la precisione come
//! argomento esplicito: nessun valore predefinito.

use geo::Coord;
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

/// Frazione della precisione che la spaziatura dei `f64` alle coordinate di
/// un'operazione puo' occupare: `p / 64`.
const SPAZIATURA_MASSIMA_IN_PRECISIONI: f64 = 1.0 / 64.0;

/// Le coordinate di modulo fino a `magnitude` sono abbastanza fitte per la
/// precisione `precision`?
///
/// Un punto calcolato (un incrocio, una coordinata riportata dalla griglia
/// dell'overlay) si arrotonda al `f64` piu' vicino: a `2^52` l'unita' in
/// ultima posizione e' 1, e l'incrocio esatto `(B, B + 1.5)` torna a 27.7
/// cm, qualunque sia il passo della griglia. Oltre `ulp(magnitude) > p /
/// 64` nessun kernel calcola: `PrecisionInsufficient`. In metri, con 1 cm,
/// e' un modulo di circa `2^39` m, fuori da ogni dominio di un CRS reale.
pub(crate) fn coordinate_abbastanza_fitte(magnitude: f64, precision: f64) -> bool {
    let magnitude = magnitude.abs();
    if !magnitude.is_finite() {
        return false;
    }
    let successivo = f64::from_bits(magnitude.to_bits().saturating_add(1));
    let spaziatura = successivo - magnitude;
    spaziatura.is_finite() && spaziatura <= precision * SPAZIATURA_MASSIMA_IN_PRECISIONI
}

/// Il modulo massimo delle coordinate, `0` senza coordinate; `NaN` se una
/// coordinata non e' un numero (e allora il controllo di spaziatura
/// rifiuta).
pub(crate) fn modulo_massimo(coordinate: impl IntoIterator<Item = Coord<f64>>) -> f64 {
    let mut massimo = 0.0_f64;
    for c in coordinate {
        if c.x.is_nan() || c.y.is_nan() {
            return f64::NAN;
        }
        massimo = massimo.max(c.x.abs()).max(c.y.abs());
    }
    massimo
}

/// Il punto dista dal segmento `start -> end` al piu' `budget`?
///
/// La distanza e' calcolata sulle differenze da `start` e maggiorata di un
/// margine d'arrotondamento relativo alle grandezze in gioco (`16 *
/// EPSILON`, piu' largo dei pochi arrotondamenti del calcolo): il controllo
/// puo' rifiutare un punto al limite, mai accettarne uno oltre.
pub(crate) fn punto_entro_segmento(
    point: Coord<f64>,
    start: Coord<f64>,
    end: Coord<f64>,
    budget: f64,
) -> bool {
    let point_x = point.x - start.x;
    let point_y = point.y - start.y;
    let direction_x = end.x - start.x;
    let direction_y = end.y - start.y;
    let length_squared = direction_x.mul_add(direction_x, direction_y * direction_y);
    let parameter = if length_squared > 0.0 {
        (point_x.mul_add(direction_x, point_y * direction_y) / length_squared).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let distance = parameter
        .mul_add(-direction_x, point_x)
        .hypot(parameter.mul_add(-direction_y, point_y));
    let rounding = 16.0
        * f64::EPSILON
        * (point_x.abs() + point_y.abs() + direction_x.abs() + direction_y.abs());
    let bound = distance + rounding;
    bound.is_finite() && bound <= budget
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
        // Un geografico senza ellissoide (risolto dal chiamante): nessuna
        // precisione, errore esplicito.
        assert!(Precision::from_crs(&crs(CrsKind::Geographic, None)).is_err());
        let wgs84 = plenora_core::crs::resolve_crs("EPSG:4326", "crs").unwrap();
        let gradi = Precision::from_crs(&wgs84).unwrap();
        let polare = 6_378_137.0 / (1.0 - 1.0 / 298.257_223_563);
        let atteso = 0.01 / f64::to_radians(polare);
        assert!(gradi.value() <= atteso && gradi.value() > atteso * (1.0 - 1e-11));
        // Stesso valore del core, bit per bit: una sola fonte.
        for (kind, unit) in [
            (CrsKind::Projected, Some(1.0)),
            (CrsKind::Projected, Some(0.3048)),
        ] {
            let resolved = crs(kind, unit);
            assert_eq!(
                Precision::from_crs(&resolved).unwrap().value().to_bits(),
                resolved.precisione_coordinate().unwrap().to_bits()
            );
        }
    }

    /// `ulp(2^39) = 2^-13` sta sotto `0.01 / 64`; `ulp(2^40) = 2^-12` no.
    #[test]
    fn spaziatura_delle_coordinate_rispetto_alla_precisione() {
        assert!(coordinate_abbastanza_fitte(2_f64.powi(39), 0.01));
        assert!(!coordinate_abbastanza_fitte(2_f64.powi(40), 0.01));
        assert!(!coordinate_abbastanza_fitte(2_f64.powi(52), 0.01));
        assert!(coordinate_abbastanza_fitte(0.0, 0.01));
        assert!(!coordinate_abbastanza_fitte(f64::MAX, 0.01));
        assert!(!coordinate_abbastanza_fitte(f64::NAN, 0.01));
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
