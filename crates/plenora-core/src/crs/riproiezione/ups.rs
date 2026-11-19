//! UPS WGS84: NGA.STND.0037 (2014), tabelle 4/5; stereografica ellissoidale.
//! Scala al polo 0.994, falso est/nord 2000000 m, meridiano origine zero.

#![allow(
    clippy::many_single_char_names,
    clippy::similar_names,
    clippy::suboptimal_flops,
    clippy::float_cmp,
    reason = "Formule ellissoidali e assi esatti"
)]

use crate::crs::CrsError;

/// Proiezione UPS WGS84, lon/lat in gradi e coordinate griglia in metri.
///
/// Il dominio matematico 79..90 gradi assoluti include le celle di bordo.
/// Il codec verifica separatamente il dominio MGRS 80S/84N.
/// Errore numerico entro 0.01 m, distinto dalla quantizzazione testuale.
///
/// # Errors
/// `ReprojectionNotConverged` per non finiti, dominio o mancata convergenza.
pub fn project_ups_wgs84(
    north: bool,
    inverse: bool,
    x: f64,
    y: f64,
) -> Result<(f64, f64), CrsError> {
    let fail = || CrsError::ReprojectionNotConverged;
    if !x.is_finite() || !y.is_finite() {
        return Err(fail());
    }
    let f = 1.0 / 298.257_223_563;
    let e = f64::sqrt(f * (2.0 - f));
    let c = ((1.0 + e).powf(1.0 + e) * (1.0 - e).powf(1.0 - e)).sqrt();
    let scale = 2.0 * 6_378_137.0 * 0.994 / c;
    let sign = if north { 1.0 } else { -1.0 };
    if inverse {
        let dx = x - 2_000_000.0;
        let dy = (2_000_000.0 - y) * sign;
        let rho = dx.hypot(dy);
        if rho == 0.0 {
            return Ok((0.0, sign * 90.0));
        }
        let t = rho / scale;
        let mut phi = std::f64::consts::FRAC_PI_2 - 2.0 * t.atan();
        for _ in 0..16 {
            let es = e * phi.sin();
            let next = std::f64::consts::FRAC_PI_2
                - 2.0 * (t * ((1.0 - es) / (1.0 + es)).powf(e / 2.0)).atan();
            if (next - phi).abs() < 1e-15 {
                let lat = sign * next.to_degrees();
                if !(79.0..=90.0).contains(&(lat * sign)) {
                    return Err(fail());
                }
                let lon = dx.atan2(dy).to_degrees();
                return Ok((if lon == 180.0 { -180.0 } else { lon }, lat));
            }
            phi = next;
        }
        Err(fail())
    } else {
        if x.abs() > 180.0 || !(79.0..=90.0).contains(&(y * sign)) {
            return Err(fail());
        }
        let phi = y.abs().to_radians();
        let es = e * phi.sin();
        let t =
            ((90.0 - y.abs()).to_radians() / 2.0).tan() / ((1.0 - es) / (1.0 + es)).powf(e / 2.0);
        let rho = scale * t;
        let (sin, cos) = match x {
            0.0 => (0.0, 1.0),
            180.0 | -180.0 => (0.0, -1.0),
            90.0 => (1.0, 0.0),
            -90.0 => (-1.0, 0.0),
            _ => x.to_radians().sin_cos(),
        };
        Ok((2_000_000.0 + rho * sin, 2_000_000.0 - sign * rho * cos))
    }
}
