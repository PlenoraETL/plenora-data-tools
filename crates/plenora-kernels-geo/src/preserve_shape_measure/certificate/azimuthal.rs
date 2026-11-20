//! LAEA ellissoidica: Snyder 1987 cap. 24; EPSG GN 7-2, 3.3.2.
//! Dominio e maggioranti della composizione in docs/maggioranti-forma.md.

use super::transverse::{arctangent, cosine, square_root};
use super::{sine, ShapeInterval, ShapeMeasureError};
use plenora_core::crs::riproiezione::AzimuthalShapeBounds;

fn radians(degrees: f64) -> Result<ShapeInterval, ShapeMeasureError> {
    if !degrees.is_finite() {
        return Err(ShapeMeasureError::AccuracyUnproven);
    }
    ShapeInterval::exact(degrees)
        .mul(ShapeInterval::around(std::f64::consts::PI))
        .div_positive(ShapeInterval::exact(180.0))
}

// atanh(e*s)/e = sum q^k*s^(2k+1)/(2k+1); q<=0.008, coda <1e-28.
fn authalic(s: ShapeInterval, q: ShapeInterval) -> Result<ShapeInterval, ShapeMeasureError> {
    if s.max_abs() > 1.0 || q.lo < 0.0 || q.hi > 0.008 {
        return Err(ShapeMeasureError::AccuracyUnproven);
    }
    let one = ShapeInterval::exact(1.0);
    let ratio = q.mul(s.mul(s));
    let mut power = s;
    let mut sum = s;
    for k in 1..=12 {
        #[cfg(test)]
        crate::noding::gop028::cycle("preserve_shape_measure/certificate/azimuthal.rs::authalic:1");
        power = power.mul(ratio);
        sum = sum.add(power.div_positive(ShapeInterval::exact(f64::from(2 * k + 1)))?);
    }
    Ok(one
        .sub(q)
        .mul(s.div_positive(one.sub(ratio))?.add(sum.widen(1e-28))))
}

/// Rettangolo convesso |u|,|v|<0.1 sulla sfera autalica normalizzata.
pub struct AzimuthalCertificate {
    parameters: AzimuthalShapeBounds,
    q: ShapeInterval,
    qp: ShapeInterval,
    sin_origin: ShapeInterval,
    cos_origin: ShapeInterval,
    x_divisor: ShapeInterval,
    y_divisor: ShapeInterval,
    longitude: ShapeInterval,
}

impl AzimuthalCertificate {
    pub(crate) fn new(
        parameters: AzimuthalShapeBounds,
        inverse_flattening: f64,
    ) -> Result<Self, ShapeMeasureError> {
        let one = ShapeInterval::exact(1.0);
        let f = one.div_positive(ShapeInterval::exact(inverse_flattening))?;
        let q = f.mul(ShapeInterval::exact(2.0).sub(f));
        let origin = radians(parameters.latitude_origin)?;
        let longitude = radians(parameters.longitude_origin)?;
        if !(6e6..=7e6).contains(&parameters.semi_major_axis)
            || origin.max_abs() >= 0.92
            || longitude.max_abs() >= 2.0
            || !parameters.false_easting.is_finite()
            || parameters.false_easting.abs() > 2e7
            || !parameters.false_northing.is_finite()
            || parameters.false_northing.abs() > 2e7
        {
            return Err(ShapeMeasureError::AccuracyUnproven);
        }
        let qp = authalic(one, q)?;
        let sin = sine(origin)?;
        let sin_origin = authalic(sin, q)?.div_positive(qp)?;
        let cos_origin = square_root(one.sub(sin_origin.mul(sin_origin)))?;
        // |beta0|<0.92 e cos(beta0)>0.605, verificati senza asin libm.
        if sin_origin.max_abs() >= sine(ShapeInterval::exact(0.92))?.lo || cos_origin.lo <= 0.605 {
            return Err(ShapeMeasureError::AccuracyUnproven);
        }
        let m = cosine(origin)?.div_positive(square_root(one.sub(q.mul(sin.mul(sin))))?)?;
        let axis = ShapeInterval::exact(parameters.semi_major_axis);
        let x_divisor = axis.mul(m).div_positive(cos_origin)?;
        let y_divisor = axis
            .mul(qp)
            .mul(cos_origin)
            .div_positive(m.mul(ShapeInterval::exact(2.0)))?;
        if x_divisor.lo <= parameters.semi_major_axis * 0.5
            || y_divisor.lo <= parameters.semi_major_axis * 0.5
            || x_divisor.hi >= parameters.semi_major_axis * 2.0
            || y_divisor.hi >= parameters.semi_major_axis * 2.0
        {
            return Err(ShapeMeasureError::AccuracyUnproven);
        }
        Ok(Self {
            parameters,
            q,
            qp,
            sin_origin,
            cos_origin,
            x_divisor,
            y_divisor,
            longitude,
        })
    }

    pub(crate) fn scale(&self) -> f64 {
        2.0 / self.parameters.semi_major_axis
    }

    fn target(&self, x: f64, y: f64) -> Result<(ShapeInterval, ShapeInterval), ShapeMeasureError> {
        if !x.is_finite() || !y.is_finite() {
            return Err(ShapeMeasureError::CrsDomain);
        }
        let u = ShapeInterval::exact(x)
            .sub(ShapeInterval::exact(self.parameters.false_easting))
            .div_positive(self.x_divisor)?;
        let v = ShapeInterval::exact(y)
            .sub(ShapeInterval::exact(self.parameters.false_northing))
            .div_positive(self.y_divisor)?;
        if u.max_abs() >= 0.1 || v.max_abs() >= 0.1 {
            return Err(ShapeMeasureError::AccuracyUnproven);
        }
        Ok((u, v))
    }

    pub(crate) fn check_domain(&self, x: f64, y: f64) -> Result<(), ShapeMeasureError> {
        self.target(x, y).map(|_| ())
    }

    pub(crate) fn certify(
        &self,
        x: f64,
        y: f64,
        longitude: f64,
        latitude: f64,
    ) -> Result<(), ShapeMeasureError> {
        let (u, v) = self.target(x, y)?;
        let one = ShapeInterval::exact(1.0);
        let squared = u.mul(u).add(v.mul(v));
        let cos_distance = one.sub(squared.mul(ShapeInterval::exact(0.5)));
        let half_cosine = square_root(one.sub(squared.mul(ShapeInterval::exact(0.25))))?;
        let east = u.mul(half_cosine);
        let north = self
            .cos_origin
            .mul(cos_distance)
            .sub(self.sin_origin.mul(v.mul(half_cosine)));
        let vertical = self
            .sin_origin
            .mul(cos_distance)
            .add(self.cos_origin.mul(v.mul(half_cosine)));
        let phi = radians(latitude)?;
        if phi.max_abs() >= 1.1 {
            return Err(ShapeMeasureError::AccuracyUnproven);
        }
        let latitude_error = (authalic(sine(phi)?, self.q)?
            .sub(self.qp.mul(vertical))
            .max_abs()
            * 1.13)
            .next_up();
        let longitude_error = arctangent(east.div_positive(north)?)?
            .add(self.longitude)
            .sub(radians(longitude)?)
            .max_abs();
        if !latitude_error.is_finite()
            || !longitude_error.is_finite()
            || latitude_error.max(longitude_error) > 2048.0 * f64::EPSILON
        {
            return Err(ShapeMeasureError::AccuracyUnproven);
        }
        Ok(())
    }
}

#[cfg(test)]
#[deny(clippy::disallowed_methods)]
mod tests {
    use super::*;

    // C1: Snyder cap. 24, l'origine si inverte in (lambda0,phi0), anche nell'emisfero sud.
    #[test]
    #[deny(clippy::disallowed_methods)]
    fn azimuthal_origin_residual_and_altered_angles() {
        for latitude in [-52.0, 0.0, 52.0] {
            let certificate = AzimuthalCertificate::new(
                AzimuthalShapeBounds {
                    semi_major_axis: 6_378_137.0,
                    latitude_origin: latitude,
                    longitude_origin: 10.0,
                    false_easting: 4_321_000.0,
                    false_northing: 3_210_000.0,
                },
                298.257_222_101,
            )
            .unwrap();
            assert_eq!(
                certificate.certify(4_321_000.0, 3_210_000.0, 10.0, latitude),
                Ok(())
            );
            for (lon, lat) in [
                (10.00001, latitude),
                (10.0, latitude + 0.00001),
                (f64::NAN, latitude),
                (10.0, f64::INFINITY),
            ] {
                assert_eq!(
                    certificate.certify(4_321_000.0, 3_210_000.0, lon, lat),
                    Err(ShapeMeasureError::AccuracyUnproven)
                );
            }
            for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
                assert_eq!(
                    certificate.check_domain(value, 3_210_000.0),
                    Err(ShapeMeasureError::CrsDomain)
                );
                assert_eq!(
                    certificate.check_domain(4_321_000.0, value),
                    Err(ShapeMeasureError::CrsDomain)
                );
            }
        }
    }
}
