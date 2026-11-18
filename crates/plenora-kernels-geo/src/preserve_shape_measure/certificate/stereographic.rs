//! EPSG GN 7-2 3.3.1.1: sfera di Gauss; Snyder cap. 21: inversa sferica.
//! Dominio e maggioranti della composizione in docs/maggioranti-forma.md.

use super::transverse::{arctangent, cosine, square_root};
use super::{inverse_hyperbolic_tangent, sine, ShapeInterval, ShapeMeasureError};
use plenora_core::crs::riproiezione::StereographicShapeBounds;

fn radians(degrees: f64) -> Result<ShapeInterval, ShapeMeasureError> {
    if !degrees.is_finite() {
        return Err(ShapeMeasureError::AccuracyUnproven);
    }
    ShapeInterval::exact(degrees)
        .mul(ShapeInterval::around(std::f64::consts::PI))
        .div_positive(ShapeInterval::exact(180.0))
}

fn isometric(
    phi: ShapeInterval,
    eccentricity: ShapeInterval,
) -> Result<ShapeInterval, ShapeMeasureError> {
    let sin = sine(phi)?;
    Ok(inverse_hyperbolic_tangent(sin)?
        .sub(eccentricity.mul(inverse_hyperbolic_tangent(eccentricity.mul(sin))?)))
}

/// Rettangolo convesso |u|,|v|<0.1, con divisore R*k0.
pub struct StereographicCertificate {
    parameters: StereographicShapeBounds,
    eccentricity: ShapeInterval,
    exponent: ShapeInterval,
    psi_origin: ShapeInterval,
    sin_origin: ShapeInterval,
    cos_origin: ShapeInterval,
    divisor: ShapeInterval,
    longitude: ShapeInterval,
}

impl StereographicCertificate {
    pub(crate) fn new(
        parameters: StereographicShapeBounds,
        inverse_flattening: f64,
    ) -> Result<Self, ShapeMeasureError> {
        let one = ShapeInterval::exact(1.0);
        let f = one.div_positive(ShapeInterval::exact(inverse_flattening))?;
        let q = f.mul(ShapeInterval::exact(2.0).sub(f));
        let origin = radians(parameters.latitude_origin)?;
        let longitude = radians(parameters.longitude_origin)?;
        if !(6e6..=7e6).contains(&parameters.semi_major_axis)
            || q.lo < 0.0
            || q.hi > 0.008
            || origin.max_abs() >= 0.92
            || longitude.max_abs() >= 2.0
            || !parameters.central_scale.is_finite()
            || !parameters.false_easting.is_finite()
            || parameters.false_easting.abs() > 2e7
            || !parameters.false_northing.is_finite()
            || parameters.false_northing.abs() > 2e7
        {
            return Err(ShapeMeasureError::AccuracyUnproven);
        }
        let eccentricity = square_root(q)?;
        let sin = sine(origin)?;
        let cos = cosine(origin)?;
        let cos_squared = cos.mul(cos);
        let exponent = square_root(
            one.add(
                q.mul(cos_squared.mul(cos_squared))
                    .div_positive(one.sub(q))?,
            ),
        )?;
        // EPSG: atanh(sin(chi0)) = atanh(sin(phi0)/n), dopo cancellazione di ln(c).
        let sin_origin = sin.div_positive(exponent)?;
        let cos_origin = square_root(one.sub(sin_origin.mul(sin_origin)))?;
        if exponent.lo < 1.0
            || exponent.hi > 1.01
            || sin_origin.max_abs() >= sine(ShapeInterval::exact(0.92))?.lo
            || cos_origin.lo <= 0.605
        {
            return Err(ShapeMeasureError::AccuracyUnproven);
        }
        let divisor = ShapeInterval::exact(parameters.semi_major_axis)
            .mul(square_root(one.sub(q))?)
            .mul(ShapeInterval::exact(parameters.central_scale))
            .div_positive(one.sub(q.mul(sin.mul(sin))))?;
        if divisor.lo <= parameters.semi_major_axis * 0.5
            || divisor.hi >= parameters.semi_major_axis * 2.0
        {
            return Err(ShapeMeasureError::AccuracyUnproven);
        }
        Ok(Self {
            parameters,
            eccentricity,
            exponent,
            psi_origin: isometric(origin, eccentricity)?,
            sin_origin,
            cos_origin,
            divisor,
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
            .div_positive(self.divisor)?;
        let v = ShapeInterval::exact(y)
            .sub(ShapeInterval::exact(self.parameters.false_northing))
            .div_positive(self.divisor)?;
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
        let quarter_squared = u.mul(u).add(v.mul(v)).mul(ShapeInterval::exact(0.25));
        let denominator = one.add(quarter_squared);
        let east = u.div_positive(denominator)?;
        let meridian = v.div_positive(denominator)?;
        let normal = one.sub(quarter_squared).div_positive(denominator)?;
        let north = self
            .cos_origin
            .mul(normal)
            .sub(self.sin_origin.mul(meridian));
        let vertical = self
            .sin_origin
            .mul(normal)
            .add(self.cos_origin.mul(meridian));
        let phi = radians(latitude)?;
        if phi.max_abs() >= 1.2 {
            return Err(ShapeMeasureError::AccuracyUnproven);
        }
        let target_psi = self.psi_origin.add(
            inverse_hyperbolic_tangent(vertical)?
                .sub(inverse_hyperbolic_tangent(self.sin_origin)?)
                .div_positive(self.exponent)?,
        );
        let latitude_error =
            (isometric(phi, self.eccentricity)?.sub(target_psi).max_abs() * 1.01).next_up();
        let longitude_error = arctangent(east.div_positive(north)?)?
            .div_positive(self.exponent)?
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

    // C1: EPSG GN 7-2 3.3.1.1, l'origine si inverte in (lambda0,phi0).
    #[test]
    #[deny(clippy::disallowed_methods)]
    fn stereographic_origin_residual_and_altered_angles() {
        for latitude in [-52.0, 0.0, 52.0] {
            let certificate = StereographicCertificate::new(
                StereographicShapeBounds {
                    semi_major_axis: 6_377_397.155,
                    latitude_origin: latitude,
                    longitude_origin: 5.0,
                    central_scale: 0.999_907_9,
                    false_easting: 155_000.0,
                    false_northing: 463_000.0,
                },
                299.152_81,
            )
            .unwrap();
            assert_eq!(
                certificate.certify(155_000.0, 463_000.0, 5.0, latitude),
                Ok(())
            );
            for (lon, lat) in [
                (5.00001, latitude),
                (5.0, latitude + 0.00001),
                (f64::NAN, latitude),
                (5.0, f64::INFINITY),
            ] {
                assert_eq!(
                    certificate.certify(155_000.0, 463_000.0, lon, lat),
                    Err(ShapeMeasureError::AccuracyUnproven)
                );
            }
            for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
                assert_eq!(
                    certificate.check_domain(value, 463_000.0),
                    Err(ShapeMeasureError::CrsDomain)
                );
                assert_eq!(
                    certificate.check_domain(155_000.0, value),
                    Err(ShapeMeasureError::CrsDomain)
                );
            }
        }
    }
}
