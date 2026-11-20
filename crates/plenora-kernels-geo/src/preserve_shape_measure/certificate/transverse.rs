//! TM esatta via continuazione dell'arco meridiano, senza trascendenti libm.
//! Dimostrazione, dominio e resti in `docs/maggioranti-forma.md`.

use plenora_core::crs::riproiezione::TransverseShapeBounds;

use super::{inverse_hyperbolic_tangent, logarithm, sine, ShapeInterval, ShapeMeasureError};

#[derive(Clone, Copy)]
struct ComplexEnclosure {
    real: ShapeInterval,
    imag: ShapeInterval,
}

impl ComplexEnclosure {
    const fn real(real: ShapeInterval) -> Self {
        Self {
            real,
            imag: ShapeInterval::exact(0.0),
        }
    }

    fn add(self, rhs: Self) -> Self {
        Self {
            real: self.real.add(rhs.real),
            imag: self.imag.add(rhs.imag),
        }
    }

    fn sub(self, rhs: Self) -> Self {
        Self {
            real: self.real.sub(rhs.real),
            imag: self.imag.sub(rhs.imag),
        }
    }

    fn mul(self, rhs: Self) -> Self {
        Self {
            real: self.real.mul(rhs.real).sub(self.imag.mul(rhs.imag)),
            imag: self.real.mul(rhs.imag).add(self.imag.mul(rhs.real)),
        }
    }

    fn scale(self, scale: ShapeInterval) -> Self {
        Self {
            real: self.real.mul(scale),
            imag: self.imag.mul(scale),
        }
    }

    fn widen(self, radius: f64) -> Self {
        Self {
            real: self.real.widen(radius),
            imag: self.imag.widen(radius),
        }
    }

    fn intersect(self, rhs: Self) -> Result<Self, ShapeMeasureError> {
        let result = Self {
            real: ShapeInterval {
                lo: self.real.lo.max(rhs.real.lo),
                hi: self.real.hi.min(rhs.real.hi),
            },
            imag: ShapeInterval {
                lo: self.imag.lo.max(rhs.imag.lo),
                hi: self.imag.hi.min(rhs.imag.hi),
            },
        };
        if result.real.lo > result.real.hi || result.imag.lo > result.imag.hi {
            return Err(ShapeMeasureError::AccuracyUnproven);
        }
        Ok(result)
    }
}

pub(super) fn cosine(x: ShapeInterval) -> Result<ShapeInterval, ShapeMeasureError> {
    if x.max_abs() >= 1.5 {
        return Err(ShapeMeasureError::AccuracyUnproven);
    }
    let square = x.mul(x).neg();
    let mut term = ShapeInterval::exact(1.0);
    let mut sum = term;
    for k in 1..=12 {
        #[cfg(test)]
        crate::noding::gop028::cycle("preserve_shape_measure/certificate/transverse.rs::cosine:1");
        term = term
            .mul(square)
            .div_positive(ShapeInterval::exact(f64::from((2 * k - 1) * 2 * k)))?;
        sum = sum.add(term);
    }
    Ok(sum.widen(1e-21))
}

fn hyperbolic(x: ShapeInterval) -> Result<(ShapeInterval, ShapeInterval), ShapeMeasureError> {
    if x.max_abs() > 0.21 {
        return Err(ShapeMeasureError::AccuracyUnproven);
    }
    let square = x.mul(x);
    let mut sinh = x;
    let mut cosh = ShapeInterval::exact(1.0);
    let mut odd = sinh;
    let mut even = cosh;
    for k in 1..=12 {
        #[cfg(test)]
        crate::noding::gop028::cycle(
            "preserve_shape_measure/certificate/transverse.rs::hyperbolic:1",
        );
        odd = odd
            .mul(square)
            .div_positive(ShapeInterval::exact(f64::from(2 * k * (2 * k + 1))))?;
        even = even
            .mul(square)
            .div_positive(ShapeInterval::exact(f64::from((2 * k - 1) * 2 * k)))?;
        sinh = sinh.add(odd);
        cosh = cosh.add(even);
    }
    Ok((sinh.widen(1e-30), cosh.widen(1e-30)))
}

fn complex_trig(
    w: ComplexEnclosure,
) -> Result<(ComplexEnclosure, ComplexEnclosure), ShapeMeasureError> {
    let sin = sine(w.real)?;
    let cos = cosine(w.real)?;
    let (sinh, cosh) = hyperbolic(w.imag)?;
    Ok((
        ComplexEnclosure {
            real: sin.mul(cosh),
            imag: cos.mul(sinh),
        },
        ComplexEnclosure {
            real: cos.mul(cosh),
            imag: sin.mul(sinh).neg(),
        },
    ))
}

pub(super) fn square_root(x: ShapeInterval) -> Result<ShapeInterval, ShapeMeasureError> {
    if x.lo < 0.0 || !x.hi.is_finite() {
        return Err(ShapeMeasureError::AccuracyUnproven);
    }
    Ok(ShapeInterval {
        lo: x.lo.sqrt().next_down(),
        hi: x.hi.sqrt().next_up(),
    })
}

pub(super) fn arctangent(mut x: ShapeInterval) -> Result<ShapeInterval, ShapeMeasureError> {
    let one = ShapeInterval::exact(1.0);
    if x.max_abs() > 8.0 {
        return Err(ShapeMeasureError::AccuracyUnproven);
    }
    for _ in 0..2 {
        #[cfg(test)]
        crate::noding::gop028::cycle(
            "preserve_shape_measure/certificate/transverse.rs::arctangent:1",
        );
        x = x.div_positive(one.add(square_root(one.add(x.mul(x)))?))?;
    }
    if x.max_abs() > 0.4 {
        return Err(ShapeMeasureError::AccuracyUnproven);
    }
    let square = x.mul(x).neg();
    let mut power = x;
    let mut sum = x;
    for k in 1..=40 {
        #[cfg(test)]
        crate::noding::gop028::cycle(
            "preserve_shape_measure/certificate/transverse.rs::arctangent:2",
        );
        power = power.mul(square);
        sum = sum.add(power.div_positive(ShapeInterval::exact(f64::from(2 * k + 1)))?);
    }
    Ok(sum.widen(1e-32).mul(ShapeInterval::exact(4.0)))
}

fn complex_log(x: ComplexEnclosure) -> Result<ComplexEnclosure, ShapeMeasureError> {
    if x.real.lo <= 0.0 {
        return Err(ShapeMeasureError::AccuracyUnproven);
    }
    let norm = x.real.mul(x.real).add(x.imag.mul(x.imag));
    // La riduzione reale ammette argomenti fino a 3; divide il quadrato per 2.
    let real = logarithm(norm.mul(ShapeInterval::exact(0.5)))?
        .add(logarithm(ShapeInterval::exact(2.0))?)
        .mul(ShapeInterval::exact(0.5));
    let imag = arctangent(x.imag.div_positive(x.real)?)?;
    Ok(ComplexEnclosure { real, imag })
}

fn complex_atanh(x: ComplexEnclosure) -> Result<ComplexEnclosure, ShapeMeasureError> {
    let one = ComplexEnclosure::real(ShapeInterval::exact(1.0));
    Ok(complex_log(one.add(x))?
        .sub(complex_log(one.sub(x))?)
        .scale(ShapeInterval::exact(0.5)))
}

// J_k = integral_0^w sin(t)^(2k) dt; c_k=(3/2)_k/k!.
// |w|<1.23, q<=.008, |Im w|<=.21: coda dopo k=12 <1e-24.
fn meridian_correction(
    w: ComplexEnclosure,
    q: ShapeInterval,
) -> Result<ComplexEnclosure, ShapeMeasureError> {
    let (sin, cos) = complex_trig(w)?;
    let square = sin.mul(sin);
    let mut odd_power = sin;
    let mut integral = w;
    let mut coefficient = ShapeInterval::exact(1.0);
    let mut sum = w.scale(q.neg());
    for k in 1..=12 {
        #[cfg(test)]
        crate::noding::gop028::cycle(
            "preserve_shape_measure/certificate/transverse.rs::meridian_correction:1",
        );
        integral = integral
            .scale(ShapeInterval::exact(f64::from(2 * k - 1)))
            .sub(odd_power.mul(cos))
            .scale(ShapeInterval::exact(1.0).div_positive(ShapeInterval::exact(f64::from(2 * k)))?);
        coefficient = coefficient
            .mul(q)
            .mul(ShapeInterval::exact(f64::from(2 * k + 1)))
            .div_positive(ShapeInterval::exact(f64::from(2 * k)))?;
        sum = sum.add(integral.scale(coefficient.mul(ShapeInterval::exact(1.0).sub(q))));
        odd_power = odd_power.mul(square);
    }
    Ok(sum.widen(1e-24))
}

/// Certificato esatto: |Re z|<1.1, |Im z|<.1, origine senza taglio.
pub struct TransverseCertificate {
    parameters: TransverseShapeBounds,
    q: ShapeInterval,
    e: ShapeInterval,
    divisor: ShapeInterval,
    origin: ShapeInterval,
    longitude: ShapeInterval,
}

impl TransverseCertificate {
    /// Fattori terreno/piano sulla striscia locale normalizzata.
    /// Derivazione quadratica di log b in docs/buffer-proiezione.md.
    pub(crate) fn local_metric_bounds(
        &self,
        lower: (f64, f64),
        upper: (f64, f64),
    ) -> Result<(f64, f64), ShapeMeasureError> {
        if self.parameters.central_scale != 1.0
            || self.parameters.latitude_origin != 0.0
            || self.parameters.false_easting != 0.0
            || self.parameters.false_northing != 0.0
            || lower.0 > upper.0
            || lower.1 > upper.1
        {
            return Err(ShapeMeasureError::AccuracyUnproven);
        }
        let first = self.target(lower.0, lower.1)?;
        let last = self.target(upper.0, upper.1)?;
        let horizontal = first.imag.max_abs().max(last.imag.max_abs());
        if first.real.max_abs().max(last.real.max_abs()) >= 0.9 || horizontal >= 0.03 {
            return Err(ShapeMeasureError::AccuracyUnproven);
        }
        let deviation = ((64.0 * horizontal).next_up() * horizontal).next_up();
        let minimum = (1.0 - deviation).next_down();
        Ok((minimum, (1.0 / minimum).next_up()))
    }

    pub(crate) fn new(
        parameters: TransverseShapeBounds,
        inverse_flattening: f64,
    ) -> Result<Self, ShapeMeasureError> {
        let one = ShapeInterval::exact(1.0);
        let flattening = one.div_positive(ShapeInterval::exact(inverse_flattening))?;
        let q = flattening.mul(ShapeInterval::exact(2.0).sub(flattening));
        let radians = ShapeInterval::around(std::f64::consts::PI)
            .div_positive(ShapeInterval::exact(180.0))?;
        let phi0 = ShapeInterval::exact(parameters.latitude_origin).mul(radians);
        let longitude = ShapeInterval::exact(parameters.longitude_origin).mul(radians);
        if q.hi > 0.008 || phi0.max_abs() >= 1.1 || longitude.max_abs() >= 2.5 {
            return Err(ShapeMeasureError::AccuracyUnproven);
        }
        let origin = phi0.add(meridian_correction(ComplexEnclosure::real(phi0), q)?.real);
        Ok(Self {
            parameters,
            q,
            e: square_root(q)?,
            origin,
            longitude,
            divisor: ShapeInterval::exact(parameters.semi_major_axis)
                .mul(ShapeInterval::exact(parameters.central_scale)),
        })
    }

    pub(crate) fn scale(&self) -> f64 {
        1.0 / (self.parameters.semi_major_axis * self.parameters.central_scale)
    }

    fn target(&self, x: f64, y: f64) -> Result<ComplexEnclosure, ShapeMeasureError> {
        if !x.is_finite() || !y.is_finite() {
            return Err(ShapeMeasureError::CrsDomain);
        }
        let target = ComplexEnclosure {
            real: ShapeInterval::exact(y)
                .sub(ShapeInterval::exact(self.parameters.false_northing))
                .div_positive(self.divisor)?
                .add(self.origin),
            imag: ShapeInterval::exact(x)
                .sub(ShapeInterval::exact(self.parameters.false_easting))
                .div_positive(self.divisor)?,
        };
        if target.real.max_abs() >= 1.1 || target.imag.max_abs() >= 0.1 {
            return Err(ShapeMeasureError::AccuracyUnproven);
        }
        Ok(target)
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
        let target = self.target(x, y)?;
        let mut root = target.widen(0.1);
        for _ in 0..24 {
            #[cfg(test)]
            crate::noding::gop028::cycle(
                "preserve_shape_measure/certificate/transverse.rs::certify:1",
            );
            root = root.intersect(target.sub(meridian_correction(root, self.q)?))?;
        }
        let (sin, _) = complex_trig(root)?;
        let exact = complex_atanh(sin)?.sub(complex_atanh(sin.scale(self.e))?.scale(self.e));
        let radians = ShapeInterval::around(std::f64::consts::PI)
            .div_positive(ShapeInterval::exact(180.0))?;
        let sin = sine(ShapeInterval::exact(latitude).mul(radians))?;
        let computed = inverse_hyperbolic_tangent(sin)?
            .sub(self.e.mul(inverse_hyperbolic_tangent(self.e.mul(sin))?));
        let latitude_error = (exact.real.sub(computed).max_abs() * 1.01).next_up();
        let longitude_error = exact
            .imag
            .sub(
                ShapeInterval::exact(longitude)
                    .mul(radians)
                    .sub(self.longitude),
            )
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

    // C1: identita' analitiche cos(0)=1, atan(1)=pi/4, M(0)=0 e M(w)/a=w se q=0.
    #[test]
    #[deny(clippy::disallowed_methods)]
    fn transverse_intervals_enclose_analytic_identities() {
        let cosine = cosine(ShapeInterval::exact(0.0)).unwrap();
        assert!(cosine.lo <= 1.0 && cosine.hi >= 1.0);
        let angle = arctangent(ShapeInterval::exact(1.0)).unwrap();
        assert!(angle.lo <= std::f64::consts::FRAC_PI_4);
        assert!(angle.hi >= std::f64::consts::FRAC_PI_4);
        for (w, q) in [
            (ComplexEnclosure::real(ShapeInterval::exact(0.0)), 0.007),
            (
                ComplexEnclosure {
                    real: ShapeInterval::exact(0.8),
                    imag: ShapeInterval::exact(0.05),
                },
                0.0,
            ),
        ] {
            let correction = meridian_correction(w, ShapeInterval::exact(q)).unwrap();
            assert!(correction.real.lo <= 0.0 && correction.real.hi >= 0.0);
            assert!(correction.imag.lo <= 0.0 && correction.imag.hi >= 0.0);
        }
    }

    // C1: EPSG 9807, origine UTM32N e residuo esatto dell'arco meridiano.
    #[test]
    #[deny(clippy::disallowed_methods)]
    fn transverse_residual_rejects_altered_angles_and_nonfinite_input() {
        let certificate = TransverseCertificate::new(
            TransverseShapeBounds {
                semi_major_axis: 6_378_137.0,
                central_scale: 0.9996,
                latitude_origin: 0.0,
                longitude_origin: 9.0,
                false_easting: 500_000.0,
                false_northing: 0.0,
            },
            298.257_223_563,
        )
        .unwrap();
        assert_eq!(certificate.certify(500_000.0, 0.0, 9.0, 0.0), Ok(()));
        for (longitude, latitude) in [(9.00001, 0.0), (9.0, 0.00001)] {
            assert_eq!(
                certificate.certify(500_000.0, 0.0, longitude, latitude),
                Err(ShapeMeasureError::AccuracyUnproven)
            );
        }
        for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert_eq!(
                certificate.check_domain(value, 0.0),
                Err(ShapeMeasureError::CrsDomain)
            );
            assert_eq!(
                certificate.check_domain(500_000.0, value),
                Err(ShapeMeasureError::CrsDomain)
            );
        }
    }
}
