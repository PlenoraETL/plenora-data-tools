//! Oracolo razionale pre-contabilizzazione, solo per prove differenziali.
use super::bounds;
use num_bigint::{BigInt, Sign};
use std::cmp::Ordering;
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct Q {
    pub(super) n: BigInt,
    pub(super) d: BigInt,
}

impl Q {
    pub fn new(mut n: BigInt, mut d: BigInt) -> Self {
        // Sonda del DAG aritmetico: misura PRIMA del gcd, non i soli
        // valori normalizzati. Non pretende di misurare lo heap di BigInt.
        #[cfg(test)]
        assert!(n.bits() <= bounds::INTERMEDIATE_BITS && d.bits() <= bounds::INTERMEDIATE_BITS);
        if d.sign() == Sign::Minus {
            n = -n;
            d = -d;
        }
        let mut a = if n.sign() == Sign::Minus {
            -&n
        } else {
            n.clone()
        };
        let mut b = d.clone();
        while b != BigInt::from(0) {
            let r = &a % &b;
            a = b;
            b = r;
        }
        Self {
            n: n / &a,
            d: d / a,
        }
    }
    pub fn int(n: i32) -> Self {
        Self {
            n: BigInt::from(n),
            d: BigInt::from(1),
        }
    }
    pub fn float(x: f64) -> Self {
        let bits = x.to_bits();
        let exponent = ((bits >> 52) & 2047) as i32;
        let fraction = bits & ((1_u64 << 52) - 1);
        let mantissa = if exponent == 0 {
            fraction
        } else {
            fraction | (1_u64 << 52)
        };
        let shift = if exponent == 0 {
            -1074
        } else {
            exponent - 1075
        };
        let mut n = BigInt::from(mantissa);
        if bits >> 63 != 0 {
            n = -n;
        }
        if shift >= 0 {
            Self::new(n << shift.unsigned_abs(), BigInt::from(1))
        } else {
            Self::new(n, BigInt::from(1) << shift.unsigned_abs())
        }
    }
    pub fn add(&self, b: &Self) -> Self {
        Self::new(&self.n * &b.d + &b.n * &self.d, &self.d * &b.d)
    }
    pub fn sub(&self, b: &Self) -> Self {
        Self::new(&self.n * &b.d - &b.n * &self.d, &self.d * &b.d)
    }
    pub fn mul(&self, b: &Self) -> Self {
        Self::new(&self.n * &b.n, &self.d * &b.d)
    }
    pub fn div(&self, b: &Self) -> Self {
        Self::new(&self.n * &b.d, &self.d * &b.n)
    }
    pub fn sign(&self) -> Ordering {
        self.n.cmp(&BigInt::from(0))
    }
    pub fn midpoint(&self, b: &Self) -> Self {
        self.add(b).div(&Self::int(2))
    }
}
impl Ord for Q {
    fn cmp(&self, b: &Self) -> Ordering {
        let left = &self.n * &b.d;
        let right = &b.n * &self.d;
        #[cfg(test)]
        assert!(
            left.bits() <= bounds::INTERMEDIATE_BITS && right.bits() <= bounds::INTERMEDIATE_BITS
        );
        left.cmp(&right)
    }
}
impl PartialOrd for Q {
    fn partial_cmp(&self, b: &Self) -> Option<Ordering> {
        Some(self.cmp(b))
    }
}
