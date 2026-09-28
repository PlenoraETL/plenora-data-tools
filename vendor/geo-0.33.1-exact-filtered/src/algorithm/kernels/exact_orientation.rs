//! Experimental exact orientation sign for all finite binary64 coordinates.
//! No float arithmetic, allocation, external dependency, epsilon or scaling.
//! See exact-orientation-candidate/README.md for the capacity proof.
use std::cmp::Ordering;

const LIMBS: usize = 132;
type Magnitude = [u32; LIMBS];

#[derive(Clone, Copy)]
struct Dyadic {
    negative: bool,
    mantissa: u64,
    shift: usize,
}

fn decode(bits: u64) -> Option<Dyadic> {
    let exponent = ((bits >> 52) & 0x7ff) as usize;
    if exponent == 0x7ff {
        return None;
    }
    Some(Dyadic {
        negative: bits >> 63 != 0,
        mantissa: (bits & 0x000f_ffff_ffff_ffff) | if exponent == 0 { 0 } else { 1 << 52 },
        shift: exponent.saturating_sub(1),
    })
}

fn product(a: Dyadic, b: Dyadic) -> Magnitude {
    let coefficient = u128::from(a.mantissa) * u128::from(b.mantissa);
    let shift = a.shift + b.shift;
    let mut result = [0; LIMBS];
    // Both significands are <= 53 bits. No overflow in u128.
    for bit in 0..106 {
        if (coefficient >> bit) & 1 != 0 {
            let index = shift + bit;
            result[index / 32] |= 1u32 << (index % 32);
        }
    }
    result
}

fn compare(a: &Magnitude, b: &Magnitude) -> Ordering {
    a.iter().rev().cmp(b.iter().rev())
}

fn add(a: &mut Magnitude, b: &Magnitude) {
    let mut carry = 0u64;
    for (x, &y) in a.iter_mut().zip(b) {
        let sum = u64::from(*x) + u64::from(y) + carry;
        *x = sum as u32;
        carry = sum >> 32;
    }
    assert_eq!(carry, 0, "exact orientation capacity exceeded");
}

fn subtract(a: &mut Magnitude, b: &Magnitude) {
    let mut borrow = false;
    for (x, &y) in a.iter_mut().zip(b) {
        let (first, b1) = x.overflowing_sub(y);
        let (second, b2) = first.overflowing_sub(u32::from(borrow));
        *x = second;
        borrow = b1 || b2;
    }
    assert!(!borrow, "exact orientation negative magnitude");
}

/// Input order: ax, ay, bx, by, cx, cy. Returns -1/0/+1; rejects NaN/Inf.
/// Each finite f64 is an exact signed integer times 2^-1074. The expanded
/// determinant is evaluated as six signed integer products times 2^-2148.
pub fn orient2d_sign_bits(bits: [u64; 6]) -> Option<i8> {
    let mut values = [Dyadic { negative: false, mantissa: 0, shift: 0 }; 6];
    for (value, bits) in values.iter_mut().zip(bits) {
        *value = decode(bits)?;
    }
    let mut sum = [0; LIMBS];
    let mut negative = false;
    // ax*by + bx*cy + cx*ay - ay*bx - by*cx - cy*ax
    for (i, j, minus) in [(0, 3, false), (2, 5, false), (4, 1, false),
                           (1, 2, true), (3, 4, true), (5, 0, true)] {
        let term = product(values[i], values[j]);
        let term_negative = values[i].negative ^ values[j].negative ^ minus;
        if negative == term_negative {
            add(&mut sum, &term);
        } else {
            match compare(&sum, &term) {
                Ordering::Greater | Ordering::Equal => subtract(&mut sum, &term),
                Ordering::Less => {
                    let old = sum;
                    sum = term;
                    subtract(&mut sum, &old);
                    negative = term_negative;
                }
            }
        }
    }
    Some(if sum.iter().all(|&x| x == 0) { 0 } else if negative { -1 } else { 1 })
}
