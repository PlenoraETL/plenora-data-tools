//! Certificazione residuale con sole operazioni IEEE 754 arrotondate.
//! Serie e resti: `docs/maggioranti-forma.md`, nessuna chiamata libm.

use super::ShapeMeasureError;

mod azimuthal;
pub(super) use azimuthal::AzimuthalCertificate;
mod lambert;
pub(super) use lambert::LambertCertificate;
mod transverse;
pub(super) use transverse::TransverseCertificate;

#[derive(Clone, Copy)]
struct ShapeInterval {
    lo: f64,
    hi: f64,
}

impl ShapeInterval {
    const fn exact(value: f64) -> Self {
        Self {
            lo: value,
            hi: value,
        }
    }

    const fn around(value: f64) -> Self {
        Self {
            lo: value.next_down(),
            hi: value.next_up(),
        }
    }

    fn add(self, rhs: Self) -> Self {
        Self {
            lo: (self.lo + rhs.lo).next_down(),
            hi: (self.hi + rhs.hi).next_up(),
        }
    }

    fn neg(self) -> Self {
        Self {
            lo: -self.hi,
            hi: -self.lo,
        }
    }

    fn sub(self, rhs: Self) -> Self {
        self.add(rhs.neg())
    }

    fn mul(self, rhs: Self) -> Self {
        let products = [
            self.lo * rhs.lo,
            self.lo * rhs.hi,
            self.hi * rhs.lo,
            self.hi * rhs.hi,
        ];
        Self {
            lo: products
                .into_iter()
                .fold(f64::INFINITY, f64::min)
                .next_down(),
            hi: products
                .into_iter()
                .fold(f64::NEG_INFINITY, f64::max)
                .next_up(),
        }
    }

    fn div_positive(self, rhs: Self) -> Result<Self, ShapeMeasureError> {
        if rhs.lo <= 0.0 || !rhs.hi.is_finite() {
            return Err(ShapeMeasureError::AccuracyUnproven);
        }
        Ok(self.mul(Self {
            lo: (1.0 / rhs.hi).next_down(),
            hi: (1.0 / rhs.lo).next_up(),
        }))
    }

    fn widen(self, radius: f64) -> Self {
        self.add(Self {
            lo: -radius,
            hi: radius,
        })
    }

    const fn max_abs(self) -> f64 {
        self.lo.abs().max(self.hi.abs())
    }
}

// Taylor alternata fino al grado 25: |resto| <= 1.5^27/27! < 6e-24.
fn sine(phi: ShapeInterval) -> Result<ShapeInterval, ShapeMeasureError> {
    if phi.max_abs() >= 1.5 {
        return Err(ShapeMeasureError::AccuracyUnproven);
    }
    let squared = phi.mul(phi).neg();
    let mut term = phi;
    let mut sum = phi;
    for n in 1..=12 {
        term = term
            .mul(squared)
            .div_positive(ShapeInterval::exact(f64::from(2 * n * (2 * n + 1))))?;
        sum = sum.add(term);
    }
    Ok(sum.widen(6e-24))
}

// ln(z)=2 sum t^(2n+1)/(2n+1), t=(z-1)/(z+1).
// |t|<=0.334: resto dopo n=24 < 1e-24.
fn log_series(z: ShapeInterval) -> Result<ShapeInterval, ShapeMeasureError> {
    let one = ShapeInterval::exact(1.0);
    let t = z.sub(one).div_positive(z.add(one))?;
    if t.max_abs() > 0.334 {
        return Err(ShapeMeasureError::AccuracyUnproven);
    }
    let squared = t.mul(t);
    let mut power = t;
    let mut sum = t;
    for n in 1..25 {
        power = power.mul(squared);
        sum = sum.add(power.div_positive(ShapeInterval::exact(f64::from(2 * n + 1)))?);
    }
    Ok(sum.mul(ShapeInterval::exact(2.0)).widen(1e-24))
}

fn logarithm(mut z: ShapeInterval) -> Result<ShapeInterval, ShapeMeasureError> {
    if !(z.lo > 0.001 && z.hi < 3.0) {
        return Err(ShapeMeasureError::AccuracyUnproven);
    }
    let mut exponent = 0_i32;
    for _ in 0..10 {
        if z.lo >= 0.75 {
            break;
        }
        z = z.mul(ShapeInterval::exact(2.0));
        exponent -= 1;
    }
    for _ in 0..10 {
        if z.hi <= 1.5 {
            break;
        }
        z = z.mul(ShapeInterval::exact(0.5));
        exponent += 1;
    }
    let ln_two = log_series(ShapeInterval::exact(2.0))?;
    Ok(log_series(z)?.add(ln_two.mul(ShapeInterval::exact(f64::from(exponent)))))
}

fn inverse_hyperbolic_tangent(z: ShapeInterval) -> Result<ShapeInterval, ShapeMeasureError> {
    let one = ShapeInterval::exact(1.0);
    Ok(logarithm(one.add(z))?
        .sub(logarithm(one.sub(z))?)
        .mul(ShapeInterval::exact(0.5)))
}

/// Restituisce solo se il residuo prova un errore angolare <=128 epsilon.
pub(super) fn mercator_residual(
    latitude: f64,
    y: f64,
    divisor: f64,
    false_northing: f64,
    inverse_flattening: f64,
    first_bound: f64,
) -> Result<(), ShapeMeasureError> {
    let one = ShapeInterval::exact(1.0);
    let f = one.div_positive(ShapeInterval::exact(inverse_flattening))?;
    let e2 = f.mul(ShapeInterval::exact(2.0).sub(f));
    // sqrt e' correttamente arrotondata IEEE 754; i due vicini includono gli estremi.
    let e = ShapeInterval {
        lo: e2.lo.sqrt().next_down(),
        hi: e2.hi.sqrt().next_up(),
    };
    let phi = ShapeInterval::exact(latitude)
        .mul(ShapeInterval::around(std::f64::consts::PI))
        .div_positive(ShapeInterval::exact(180.0))?;
    let s = sine(phi)?;
    let isometric =
        inverse_hyperbolic_tangent(s)?.sub(e.mul(inverse_hyperbolic_tangent(e.mul(s))?));
    let psi = ShapeInterval::exact(y)
        .sub(ShapeInterval::exact(false_northing))
        .div_positive(ShapeInterval::around(divisor))?;
    let error = (isometric.sub(psi).max_abs() * first_bound).next_up();
    if !error.is_finite() || error > 128.0 * f64::EPSILON {
        return Err(ShapeMeasureError::AccuracyUnproven);
    }
    Ok(())
}

#[cfg(test)]
#[deny(clippy::disallowed_methods)]
mod tests {
    use super::*;

    // C1: identita' analitiche sin(pi/6)=1/2, ln(1)=0 e ln(2).
    #[test]
    #[deny(clippy::disallowed_methods)]
    fn interval_series_enclose_analytic_values() {
        let angle = ShapeInterval::around(std::f64::consts::PI)
            .div_positive(ShapeInterval::exact(6.0))
            .unwrap();
        for (phi, expected) in [(angle, 0.5), (angle.neg(), -0.5)] {
            let result = sine(phi).unwrap();
            assert!(result.lo <= expected && result.hi >= expected);
        }
        for (x, expected) in [
            (1.0, 0.0),
            (2.0, std::f64::consts::LN_2),
            (0.5, -std::f64::consts::LN_2),
        ] {
            let result = logarithm(ShapeInterval::exact(x)).unwrap();
            assert!(result.lo <= expected && result.hi >= expected);
        }
    }

    // C1: Snyder 7-7, psi(0)=0; una latitudine alterata non supera il certificato.
    #[test]
    #[deny(clippy::disallowed_methods)]
    fn residual_rejects_an_inaccurate_inverse() {
        assert_eq!(
            mercator_residual(0.0, 123.0, 6_378_137.0, 123.0, 298.257_223_563, 1.01),
            Ok(())
        );
        assert_eq!(
            mercator_residual(0.001, 123.0, 6_378_137.0, 123.0, 298.257_223_563, 1.01),
            Err(ShapeMeasureError::AccuracyUnproven)
        );
    }

    // C1: Snyder 7-7, y(85 gradi) da formula Python indipendente.
    // L'ampiezza dell'intervallo supera la precisione, pur dentro il dominio CRS.
    #[test]
    #[deny(clippy::disallowed_methods)]
    fn near_domain_edge_rejects_unproven_numerical_precision() {
        assert_eq!(
            mercator_residual(
                85.0,
                19_929_239.113_379_15,
                6_378_137.0,
                0.0,
                298.257_223_563,
                1.01
            ),
            Err(ShapeMeasureError::AccuracyUnproven)
        );
    }
}
