//! LCC 2SP esatta: composizione polare e inversa isometrica separabile.
//! Snyder 1987 capitolo 15; EPSG GN 7-2, 3.1.1.1; maggioranti-forma.md.

use plenora_core::crs::riproiezione::LambertShapeBounds;

use super::transverse::{arctangent, cosine, square_root};
use super::{inverse_hyperbolic_tangent, logarithm, sine, ShapeInterval, ShapeMeasureError};

fn quotient(x: ShapeInterval, y: ShapeInterval) -> Result<ShapeInterval, ShapeMeasureError> {
    if y.hi < 0.0 {
        x.neg().div_positive(y.neg())
    } else {
        x.div_positive(y)
    }
}

fn radians(degrees: f64) -> Result<ShapeInterval, ShapeMeasureError> {
    if !degrees.is_finite() {
        return Err(ShapeMeasureError::AccuracyUnproven);
    }
    ShapeInterval::exact(degrees)
        .mul(ShapeInterval::around(std::f64::consts::PI))
        .div_positive(ShapeInterval::exact(180.0))
}

fn isometric(phi: ShapeInterval, e: ShapeInterval) -> Result<ShapeInterval, ShapeMeasureError> {
    let sin = sine(phi)?;
    Ok(inverse_hyperbolic_tangent(sin)?.sub(e.mul(inverse_hyperbolic_tangent(e.mul(sin))?)))
}

// Taylor grado 40: per |x|<2, e^2*2^41/41! < 1e-35.
pub(super) fn exponential(x: ShapeInterval) -> Result<ShapeInterval, ShapeMeasureError> {
    if x.max_abs() >= 2.0 {
        return Err(ShapeMeasureError::AccuracyUnproven);
    }
    let mut term = ShapeInterval::exact(1.0);
    let mut sum = term;
    for k in 1..=40 {
        #[cfg(test)]
        crate::noding::gop028::cycle(
            "preserve_shape_measure/certificate/lambert.rs::exponential:1",
        );
        term = term
            .mul(x)
            .div_positive(ShapeInterval::exact(f64::from(k)))?;
        sum = sum.add(term);
    }
    Ok(sum.widen(1e-35))
}

/// Dominio convesso |u|<1/4, 3/4<v<3/2, normalizzato da a e sign(n).
pub struct LambertCertificate {
    parameters: LambertShapeBounds,
    eccentricity: ShapeInterval,
    cone: ShapeInterval,
    sign: ShapeInterval,
    origin_isometric: ShapeInterval,
    origin_radius: ShapeInterval,
    longitude: ShapeInterval,
}

impl LambertCertificate {
    pub(crate) fn new(
        parameters: LambertShapeBounds,
        inverse_flattening: f64,
    ) -> Result<Self, ShapeMeasureError> {
        let one = ShapeInterval::exact(1.0);
        let flattening = one.div_positive(ShapeInterval::exact(inverse_flattening))?;
        let q = flattening.mul(ShapeInterval::exact(2.0).sub(flattening));
        let eccentricity = square_root(q)?;
        let origin = radians(parameters.latitude_origin)?;
        let longitude = radians(parameters.longitude_origin)?;
        let first = radians(parameters.standard_parallels[0])?;
        let second = radians(parameters.standard_parallels[1])?;
        if !(6e6..=7e6).contains(&parameters.semi_major_axis)
            || q.lo < 0.0
            || q.hi > 0.008
            || origin.max_abs().max(first.max_abs()).max(second.max_abs()) >= 1.2
            || longitude.max_abs() >= 2.0
            || !parameters.false_easting.is_finite()
            || parameters.false_easting.abs() > 2e7
            || !parameters.false_northing.is_finite()
            || parameters.false_northing.abs() > 2e7
        {
            return Err(ShapeMeasureError::AccuracyUnproven);
        }
        let log_m = |phi| -> Result<ShapeInterval, ShapeMeasureError> {
            let sin = sine(phi)?;
            Ok(logarithm(cosine(phi)?)?
                .sub(logarithm(one.sub(q.mul(sin.mul(sin))))?.mul(ShapeInterval::exact(0.5))))
        };
        let psi_first = isometric(first, eccentricity)?;
        let cone = quotient(
            log_m(first)?.sub(log_m(second)?),
            isometric(second, eccentricity)?.sub(psi_first),
        )?;
        let sign = ShapeInterval::exact(if cone.hi < 0.0 { -1.0 } else { 1.0 });
        let absolute_cone = cone.mul(sign);
        if absolute_cone.lo < 0.5 || absolute_cone.hi > 0.9 {
            return Err(ShapeMeasureError::AccuracyUnproven);
        }
        let origin_isometric = isometric(origin, eccentricity)?;
        let sin_first = sine(first)?;
        let m_first =
            cosine(first)?.div_positive(square_root(one.sub(q.mul(sin_first.mul(sin_first))))?)?;
        let origin_radius =
            quotient(m_first, cone)?.mul(exponential(cone.mul(psi_first.sub(origin_isometric)))?);
        let absolute_radius = origin_radius.mul(sign);
        if absolute_radius.lo <= 0.75 || absolute_radius.hi >= 1.5 {
            return Err(ShapeMeasureError::AccuracyUnproven);
        }
        Ok(Self {
            parameters,
            eccentricity,
            cone,
            sign,
            origin_isometric,
            origin_radius,
            longitude,
        })
    }

    pub(crate) fn scale(&self) -> f64 {
        1.0 / self.parameters.semi_major_axis
    }

    fn target(&self, x: f64, y: f64) -> Result<(ShapeInterval, ShapeInterval), ShapeMeasureError> {
        if !x.is_finite() || !y.is_finite() {
            return Err(ShapeMeasureError::CrsDomain);
        }
        let axis = ShapeInterval::exact(self.parameters.semi_major_axis);
        let u = ShapeInterval::exact(x)
            .sub(ShapeInterval::exact(self.parameters.false_easting))
            .div_positive(axis)?
            .mul(self.sign);
        let v = self
            .origin_radius
            .sub(
                ShapeInterval::exact(y)
                    .sub(ShapeInterval::exact(self.parameters.false_northing))
                    .div_positive(axis)?,
            )
            .mul(self.sign);
        if u.max_abs() >= 0.25 || v.lo <= 0.75 || v.hi >= 1.5 {
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
        self.check_domain(x, y)?;
        let absolute_radius = self.origin_radius.mul(self.sign);
        let axis = ShapeInterval::exact(self.parameters.semi_major_axis);
        let u = ShapeInterval::exact(x)
            .sub(ShapeInterval::exact(self.parameters.false_easting))
            .div_positive(axis)?
            .mul(self.sign)
            .div_positive(absolute_radius)?;
        let v = ShapeInterval::exact(1.0).sub(
            ShapeInterval::exact(y)
                .sub(ShapeInterval::exact(self.parameters.false_northing))
                .div_positive(axis)?
                .mul(self.sign)
                .div_positive(absolute_radius)?,
        );
        // Forma relativa all'origine: evita la sottrazione di due logaritmi correlati.
        let log_ratio = logarithm(u.mul(u).add(v.mul(v)).mul(ShapeInterval::exact(0.5)))?
            .add(logarithm(ShapeInterval::exact(2.0))?)
            .mul(ShapeInterval::exact(0.5));
        let psi = self.origin_isometric.sub(quotient(log_ratio, self.cone)?);
        let angle = quotient(arctangent(u.div_positive(v)?)?, self.cone)?;
        let latitude_error = (psi
            .sub(isometric(radians(latitude)?, self.eccentricity)?)
            .max_abs()
            * 1.01)
            .next_up();
        let longitude_error = angle.add(self.longitude).sub(radians(longitude)?).max_abs();
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

    // C1: Snyder cap. 15, origine e simmetria nord/sud delle equazioni 15-7--15-11.
    #[test]
    #[deny(clippy::disallowed_methods)]
    fn lambert_residual_rejects_altered_angles_and_handles_both_cones() {
        for sign in [-1.0, 1.0] {
            let certificate = LambertCertificate::new(
                LambertShapeBounds {
                    semi_major_axis: 6_378_137.0,
                    latitude_origin: sign * 46.5,
                    longitude_origin: 3.0,
                    standard_parallels: [sign * 49.0, sign * 44.0],
                    false_easting: 700_000.0,
                    false_northing: 6_600_000.0,
                },
                298.257_222_101,
            )
            .unwrap();
            assert_eq!(
                certificate.certify(700_000.0, 6_600_000.0, 3.0, sign * 46.5),
                Ok(())
            );
            for (longitude, latitude) in [(3.00001, sign * 46.5), (3.0, sign * 46.50001)] {
                assert_eq!(
                    certificate.certify(700_000.0, 6_600_000.0, longitude, latitude),
                    Err(ShapeMeasureError::AccuracyUnproven)
                );
            }
            for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
                assert_eq!(
                    certificate.check_domain(value, 6_600_000.0),
                    Err(ShapeMeasureError::CrsDomain)
                );
                assert_eq!(
                    certificate.check_domain(700_000.0, value),
                    Err(ShapeMeasureError::CrsDomain)
                );
                assert_eq!(
                    certificate.certify(700_000.0, 6_600_000.0, value, 46.5),
                    Err(ShapeMeasureError::AccuracyUnproven)
                );
            }
        }
    }

    // C1: identita' exp(0)=1 e exp(x)*exp(-x)=1, Taylor con resto analitico.
    #[test]
    #[deny(clippy::disallowed_methods)]
    fn lambert_exponential_encloses_identities_and_rejects_unsupported_range() {
        for value in [0.0, 0.5, 1.5, -1.5] {
            let product = exponential(ShapeInterval::exact(value))
                .unwrap()
                .mul(exponential(ShapeInterval::exact(-value)).unwrap());
            assert!(product.lo <= 1.0 && product.hi >= 1.0);
        }
        assert!(matches!(
            exponential(ShapeInterval::exact(2.0)),
            Err(ShapeMeasureError::AccuracyUnproven)
        ));
    }
}
