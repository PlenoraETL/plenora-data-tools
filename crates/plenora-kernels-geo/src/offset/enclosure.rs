//! Intervalli razionali chiusi: ogni operazione contiene il valore reale.
use super::{OffsetError, Work, P, Q};
use std::cmp::Ordering;

pub(super) struct Enclosure {
    pub low: Q,
    pub high: Q,
}
impl Enclosure {
    pub fn exact(q: Q) -> Result<Self, OffsetError> {
        Ok(Self {
            high: q.try_clone()?,
            low: q,
        })
    }
    pub fn copy(&self) -> Result<Self, OffsetError> {
        Ok(Self {
            low: self.low.try_clone()?,
            high: self.high.try_clone()?,
        })
    }
    pub fn add(&self, b: &Self) -> Result<Self, OffsetError> {
        Ok(Self {
            low: self.low.try_add(&b.low)?,
            high: self.high.try_add(&b.high)?,
        })
    }
    pub fn sub(&self, b: &Self) -> Result<Self, OffsetError> {
        Ok(Self {
            low: self.low.try_sub(&b.high)?,
            high: self.high.try_sub(&b.low)?,
        })
    }
    pub fn mul(&self, b: &Self) -> Result<Self, OffsetError> {
        let mut low = self.low.try_mul(&b.low)?;
        let mut high = low.try_clone()?;
        for (a, b) in [
            (&self.low, &b.high),
            (&self.high, &b.low),
            (&self.high, &b.high),
        ] {
            let q = a.try_mul(b)?;
            if q.try_cmp(&low)?.is_lt() {
                low = q.try_clone()?;
            }
            if q.try_cmp(&high)?.is_gt() {
                high = q;
            }
        }
        Ok(Self { low, high })
    }
    pub fn div(&self, b: &Self, work: &Work<'_>) -> Result<Self, OffsetError> {
        if b.low.sign().is_le() && b.high.sign().is_ge() {
            return Err(OffsetError::AccuracyUnproven);
        }
        let one = work.integer(1)?;
        self.mul(&Self {
            low: one.try_div(&b.high)?,
            high: one.try_div(&b.low)?,
        })
    }
    pub fn sign(&self) -> Result<Ordering, OffsetError> {
        let low = self.low.sign();
        let high = self.high.sign();
        if low == high {
            Ok(low)
        } else {
            Err(OffsetError::AccuracyUnproven)
        }
    }
    pub fn middle(&self) -> Result<Q, OffsetError> {
        Ok(self.low.try_midpoint(&self.high)?)
    }
    pub fn rounded(&self, work: &mut Work<'_>) -> Result<f64, OffsetError> {
        let low = match super::ordinate(&self.low, work) {
            Ok(value) => Some(value),
            Err(super::LineBooleanError::CoordinateOutOfRange) => None,
            Err(error) => return Err(error.into()),
        };
        let high = match super::ordinate(&self.high, work) {
            Ok(value) => Some(value),
            Err(super::LineBooleanError::CoordinateOutOfRange) => None,
            Err(error) => return Err(error.into()),
        };
        let (low, high) = match (low, high) {
            (Some(low), Some(high)) => (low, high),
            (None, None) if self.low.sign() == self.high.sign() => {
                return Err(OffsetError::CoordinateOutOfRange)
            }
            _ => return Err(OffsetError::AccuracyUnproven),
        };
        if low.to_bits() == high.to_bits() {
            Ok(low)
        } else {
            Err(OffsetError::AccuracyUnproven)
        }
    }
}

pub(super) struct EnclosedPoint {
    pub x: Enclosure,
    pub y: Enclosure,
}
impl EnclosedPoint {
    pub fn exact(p: &P) -> Result<Self, OffsetError> {
        Ok(Self {
            x: Enclosure::exact(p.x.try_clone()?)?,
            y: Enclosure::exact(p.y.try_clone()?)?,
        })
    }
    pub fn copy(&self) -> Result<Self, OffsetError> {
        Ok(Self {
            x: self.x.copy()?,
            y: self.y.copy()?,
        })
    }
    pub fn add(&self, b: &Self) -> Result<Self, OffsetError> {
        Ok(Self {
            x: self.x.add(&b.x)?,
            y: self.y.add(&b.y)?,
        })
    }
    pub fn sub(&self, b: &Self) -> Result<Self, OffsetError> {
        Ok(Self {
            x: self.x.sub(&b.x)?,
            y: self.y.sub(&b.y)?,
        })
    }
    pub fn scale(&self, q: &Enclosure) -> Result<Self, OffsetError> {
        Ok(Self {
            x: self.x.mul(q)?,
            y: self.y.mul(q)?,
        })
    }
    pub fn cross(&self, b: &Self) -> Result<Enclosure, OffsetError> {
        self.x.mul(&b.y)?.sub(&self.y.mul(&b.x)?)
    }
    pub fn dot(&self, b: &Self) -> Result<Enclosure, OffsetError> {
        self.x.mul(&b.x)?.add(&self.y.mul(&b.y)?)
    }
    pub fn middle(&self) -> Result<P, OffsetError> {
        Ok(P {
            x: self.x.middle()?,
            y: self.y.middle()?,
        })
    }
}

// max(|dx|,|dy|) <= hypot(dx,dy) <= 2*max(|dx|,|dy|).
// La bisezione usa solo confronti di quadrati razionali; nessuna sqrt f64.
pub(super) fn length(
    direction: &P,
    precision: u32,
    work: &mut Work<'_>,
) -> Result<Enclosure, OffsetError> {
    work.step()?;
    let square = direction
        .x
        .try_mul(&direction.x)?
        .try_add(&direction.y.try_mul(&direction.y)?)?;
    if let Some(root) = square.try_sqrt_exact()? {
        return Enclosure::exact(root);
    }
    let zero = work.integer(0)?;
    let x = if direction.x.sign().is_lt() {
        zero.try_sub(&direction.x)?
    } else {
        direction.x.try_clone()?
    };
    let y = if direction.y.sign().is_lt() {
        zero.try_sub(&direction.y)?
    } else {
        direction.y.try_clone()?
    };
    let mut low = if x.try_cmp(&y)?.is_gt() { x } else { y };
    let mut high = low.try_mul(&work.integer(2)?)?;
    for _ in 0..precision {
        work.step()?;
        let mid = low.try_midpoint(&high)?;
        match mid.try_mul(&mid)?.try_cmp(&square)? {
            Ordering::Equal => return Enclosure::exact(mid),
            Ordering::Less => low = mid,
            Ordering::Greater => high = mid,
        }
    }
    Ok(Enclosure { low, high })
}
