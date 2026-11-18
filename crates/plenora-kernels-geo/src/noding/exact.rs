//! Aritmetica fallibile: ogni intero, compresi gcd e prodotti dei confronti,
//! possiede una ricevuta sul bilancio vivo dei contenitori.

use super::storage::{Budget, Lease};
use crate::predicates::PredicateError;
use num_bigint::{BigInt, Sign};
use std::cmp::Ordering;
use std::rc::Rc;

/// Limite del DAG dimostrato in `bounds.rs`, non una prenotazione anticipata.
pub(super) const MAX_BITS: u64 = super::bounds::INTERMEDIATE_BITS;
const LIMB_BITS: u64 = usize::BITS as u64;
pub(super) const MAX_VALUE_BYTES: u64 = super::bounds::MAX_VALUE_BYTES;

/// Unico proprietario degli interi del percorso di produzione.
///
/// `num-bigint =0.5.1` non espone la capacita': si addebitano i bit
/// arrotondati ai limb nativi, piu' l'header `BigInt`. La ricevuta segue
/// immediatamente ogni creazione e muore DOPO il valore. Il rifiuto puo'
/// superare il bilancio contabile di un solo valore, al massimo
/// `ceil(145173 / usize::BITS) * size_of::<usize>() + size_of::<BigInt>()`.
/// Questo limite contabile non misura gli scratch privati del backend.
/// Nessun `Deref`, operatore, clone infallibile o accesso pubblico al backend.
#[derive(Debug)]
struct CountedInt {
    value: BigInt,
    lease: Lease,
}

#[cfg(test)]
#[deny(clippy::disallowed_methods)]
mod tests {
    use super::*;
    use crate::noding::oracle;

    /// GOP-027: clone e temporanei possiedono ricevute distinte; il
    /// risultato puo' sopravvivere al Work perche' possiede il bilancio.
    #[test]
    fn clone_drop_and_arithmetic_share_the_live_budget() {
        let budget = Rc::new(Budget::new(u64::MAX));
        let value = Q::float_in(f64::MIN_POSITIVE, &budget).expect("valore");
        let one = budget.live();
        assert!(one > 2 * std::mem::size_of::<BigInt>() as u64);
        let copy = value.try_clone().expect("copia");
        assert_eq!(budget.live(), 2 * one);
        let sum = value.try_add(&copy).expect("somma");
        assert!(budget.peak() > budget.live());
        drop(value);
        drop(copy);
        assert!(budget.live() > 0);
        drop(sum);
        assert_eq!(budget.live(), 0);
        let value = Q::int_in(1, &budget).expect("uno");
        let observer = Rc::downgrade(&budget);
        drop(budget);
        assert!(observer.upgrade().is_some());
        drop(value);
        assert!(observer.upgrade().is_none());
    }

    /// GOP-027 e derivazione bounds.rs: rifiuto subito dopo un singolo
    /// intero, compreso il massimo matematico. Misura contabile, non heap.
    #[test]
    fn exact_threshold_and_single_value_overshoot() {
        assert_eq!(MAX_BITS, crate::noding::bounds::INTERMEDIATE_BITS);
        let run = |limit| {
            let budget = Rc::new(Budget::new(limit));
            let result = (|| {
                let a = Q::float_in(f64::MIN_POSITIVE, &budget)?;
                let b = Q::float_in(f64::MAX, &budget)?;
                a.try_add(&b)
            })();
            (result, budget)
        };
        let (result, budget) = run(u64::MAX);
        drop(result.expect("misura"));
        assert_eq!(budget.live(), 0);
        let peak = budget.peak();
        for limit in [peak, peak + 1] {
            let (result, budget) = run(limit);
            drop(result.expect("soglia ammessa"));
            assert_eq!(budget.live(), 0);
        }
        let (result, budget) = run(peak - 1);
        assert!(matches!(result, Err(PredicateError::ReservationLimit)));
        assert_eq!(budget.live(), 0);
        assert!(budget.attempted_peak() > peak - 1);
        assert!(budget.attempted_peak() - (peak - 1) <= MAX_VALUE_BYTES);
        let budget = Rc::new(Budget::new(0));
        let maximum = BigInt::from(1) << (MAX_BITS - 1);
        assert!(matches!(
            CountedInt::created(maximum, &budget),
            Err(PredicateError::ReservationLimit)
        ));
        assert_eq!(budget.attempted_peak(), MAX_VALUE_BYTES);
        assert_eq!(budget.live(), 0);
    }

    /// GOP-027: il secondo intero del clone puo' fallire; la prima
    /// ricevuta parziale viene comunque rilasciata, l'originale resta vivo.
    #[test]
    fn failed_clone_releases_partial_receipts_and_rejects_foreign_budgets() {
        let header = std::mem::size_of::<BigInt>() as u64;
        let per_integer = header + LIMB_BITS / 8;
        let budget = Rc::new(Budget::new(4 * per_integer - 1));
        let value = Q::int_in(1, &budget).expect("originale");
        assert!(matches!(
            value.try_clone(),
            Err(PredicateError::ReservationLimit)
        ));
        assert_eq!(budget.live(), 2 * per_integer);
        let other_budget = Rc::new(Budget::new(u64::MAX));
        let other = Q::int_in(1, &other_budget).expect("altro bilancio");
        assert!(matches!(
            value.try_add(&other),
            Err(PredicateError::CalcoloNonConcluso(_))
        ));
        drop(value);
        assert_eq!(budget.live(), 0);
    }

    /// GOP-027: somma a profondita' variabile, rifiuto prima del valore
    /// oltre il dominio fisso; nessun temporaneo eccede `MAX_VALUE_BYTES`.
    #[test]
    fn accumulator_rejects_before_oversized_integer_creation() {
        let budget = Rc::new(Budget::new(u64::MAX));
        let mut value = Q::int_in(1, &budget).expect("uno");
        let mut count = 0;
        loop {
            match value.try_add(&value) {
                Ok(next) => {
                    value = next;
                    count += 1;
                }
                Err(PredicateError::ReservationLimit) => break,
                Err(error) => panic!("errore inatteso: {error}"),
            }
        }
        assert!(count > 100_000);
        assert!(value.n.value.bits() <= MAX_BITS);
        let before = budget.attempted_peak();
        assert!(matches!(
            value.try_add(&value),
            Err(PredicateError::ReservationLimit)
        ));
        assert_eq!(before, budget.attempted_peak());
        drop(value);
        assert_eq!(budget.live(), 0);
        let largest =
            CountedInt::created(BigInt::from(1) << (MAX_BITS - 1), &budget).expect("massimo");
        let two = CountedInt::int(2, &budget).expect("due");
        assert!(matches!(
            largest.mul(&two),
            Err(PredicateError::ReservationLimit)
        ));
        assert!(matches!(
            largest.shift(1),
            Err(PredicateError::ReservationLimit)
        ));
    }

    fn same(actual: &Q, expected: &oracle::Q) {
        assert_eq!(actual.n.value, expected.n);
        assert_eq!(actual.d.value, expected.d);
    }

    /// IEEE 754 binary64, algebra delle frazioni e regola 3 di AGENTS: confronto
    /// input per input con il percorso generico precedente, compresi
    /// subnormali, esponenti estremi, zeri con segno e segni opposti.
    #[test]
    fn counted_arithmetic_matches_the_generic_oracle() {
        let budget = Rc::new(Budget::new(u64::MAX));
        let values = [
            -f64::MAX,
            -1.0,
            -f64::from_bits(1),
            -0.0,
            0.0,
            f64::from_bits(1),
            f64::MIN_POSITIVE,
            1.0,
            3.0,
            f64::MAX,
        ];
        for a in values {
            for b in values {
                let x = Q::float_in(a, &budget).expect("a");
                let y = Q::float_in(b, &budget).expect("b");
                let ox = oracle::Q::float(a);
                let oy = oracle::Q::float(b);
                same(&x, &ox);
                same(&y, &oy);
                same(&x.try_add(&y).expect("somma"), &ox.add(&oy));
                same(&x.try_sub(&y).expect("differenza"), &ox.sub(&oy));
                same(&x.try_mul(&y).expect("prodotto"), &ox.mul(&oy));
                same(&x.try_midpoint(&y).expect("medio"), &ox.midpoint(&oy));
                assert_eq!(x.try_cmp(&y).expect("confronto"), ox.cmp(&oy));
                if y.sign() == Ordering::Equal {
                    assert!(matches!(
                        x.try_div(&y),
                        Err(PredicateError::CalcoloNonConcluso(_))
                    ));
                } else {
                    same(&x.try_div(&y).expect("quoziente"), &ox.div(&oy));
                }
            }
        }
        assert_eq!(budget.live(), 0);
    }
}

impl PartialEq for CountedInt {
    fn eq(&self, other: &Self) -> bool {
        self.value == other.value
    }
}
impl Eq for CountedInt {}

impl CountedInt {
    fn same_budget(&self, b: &Self) -> Result<(), PredicateError> {
        if Rc::ptr_eq(self.lease.budget(), b.lease.budget()) {
            Ok(())
        } else {
            Err(PredicateError::CalcoloNonConcluso(
                "proprietario del budget",
            ))
        }
    }
    fn created(value: BigInt, budget: &Rc<Budget>) -> Result<Self, PredicateError> {
        if value.bits() > MAX_BITS {
            return Err(PredicateError::CalcoloNonConcluso("dominio aritmetico"));
        }
        let bytes = value.bits().div_ceil(LIMB_BITS) * (LIMB_BITS / 8)
            + std::mem::size_of::<BigInt>() as u64;
        debug_assert!(bytes <= MAX_VALUE_BYTES);
        let lease = Lease::created(budget, bytes)?;
        Ok(Self { value, lease })
    }

    fn int(value: i32, budget: &Rc<Budget>) -> Result<Self, PredicateError> {
        Self::created(BigInt::from(value), budget)
    }

    fn copy(&self) -> Result<Self, PredicateError> {
        Self::created(self.value.clone(), self.lease.budget())
    }

    fn neg(&self) -> Result<Self, PredicateError> {
        Self::created(-&self.value, self.lease.budget())
    }

    // Ogni primitiva rifiuta prima di creare un intero oltre MAX_BITS.
    // Vale anche per accumulatori con profondita' dipendente dall'ingresso.
    const fn bounded(bits: u64) -> Result<(), PredicateError> {
        if bits > MAX_BITS {
            Err(PredicateError::ReservationLimit)
        } else {
            Ok(())
        }
    }

    fn add(&self, b: &Self) -> Result<Self, PredicateError> {
        self.same_budget(b)?;
        Self::bounded(self.value.bits().max(b.value.bits()) + 1)?;
        Self::created(&self.value + &b.value, self.lease.budget())
    }

    fn sub(&self, b: &Self) -> Result<Self, PredicateError> {
        self.same_budget(b)?;
        Self::bounded(self.value.bits().max(b.value.bits()) + 1)?;
        Self::created(&self.value - &b.value, self.lease.budget())
    }

    fn mul(&self, b: &Self) -> Result<Self, PredicateError> {
        self.same_budget(b)?;
        Self::bounded(self.value.bits() + b.value.bits())?;
        Self::created(&self.value * &b.value, self.lease.budget())
    }

    fn div(&self, b: &Self) -> Result<Self, PredicateError> {
        self.same_budget(b)?;
        if b.value.sign() == Sign::NoSign {
            return Err(PredicateError::CalcoloNonConcluso("divisore nullo"));
        }
        Self::created(&self.value / &b.value, self.lease.budget())
    }

    fn rem(&self, b: &Self) -> Result<Self, PredicateError> {
        self.same_budget(b)?;
        if b.value.sign() == Sign::NoSign {
            return Err(PredicateError::CalcoloNonConcluso("divisore nullo"));
        }
        Self::created(&self.value % &b.value, self.lease.budget())
    }

    fn shift(&self, bits: u32) -> Result<Self, PredicateError> {
        Self::bounded(self.value.bits() + u64::from(bits))?;
        Self::created(&self.value << bits, self.lease.budget())
    }
}

/// Razionale canonico. Le operazioni e la copia sono fallibili; anche gli
/// interi temporanei hanno ricevute, prima di costruire il valore seguente.
#[derive(Debug, Eq, PartialEq)]
pub struct Q {
    n: CountedInt,
    d: CountedInt,
}

impl Q {
    pub(super) fn belongs_to(&self, budget: &Rc<Budget>) -> bool {
        Rc::ptr_eq(self.n.lease.budget(), budget) && Rc::ptr_eq(self.d.lease.budget(), budget)
    }
    #[cfg(test)]
    #[deny(clippy::disallowed_methods)]
    pub(super) fn bits(&self) -> (u64, u64) {
        (self.n.value.bits(), self.d.value.bits())
    }

    fn new(mut n: CountedInt, mut d: CountedInt) -> Result<Self, PredicateError> {
        n.same_budget(&d)?;
        if d.value.sign() == Sign::NoSign {
            return Err(PredicateError::CalcoloNonConcluso("denominatore nullo"));
        }
        if d.value.sign() == Sign::Minus {
            n = n.neg()?;
            d = d.neg()?;
        }
        let mut a = if n.value.sign() == Sign::Minus {
            n.neg()?
        } else {
            n.copy()?
        };
        let mut b = d.copy()?;
        while b.value.sign() != Sign::NoSign {
            let r = a.rem(&b)?;
            a = b;
            b = r;
        }
        Ok(Self {
            n: n.div(&a)?,
            d: d.div(&a)?,
        })
    }

    pub(super) fn int_in(value: i32, budget: &Rc<Budget>) -> Result<Self, PredicateError> {
        Ok(Self {
            n: CountedInt::int(value, budget)?,
            d: CountedInt::int(1, budget)?,
        })
    }

    pub(super) fn float_in(value: f64, budget: &Rc<Budget>) -> Result<Self, PredicateError> {
        if !value.is_finite() {
            return Err(PredicateError::CalcoloNonConcluso("coordinata non finita"));
        }
        let bits = value.to_bits();
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
        let mut n = CountedInt::created(BigInt::from(mantissa), budget)?;
        if bits >> 63 != 0 {
            n = n.neg()?;
        }
        let one = CountedInt::int(1, budget)?;
        if shift >= 0 {
            Self::new(n.shift(shift.unsigned_abs())?, one)
        } else {
            Self::new(n, one.shift(shift.unsigned_abs())?)
        }
    }

    pub fn try_clone(&self) -> Result<Self, PredicateError> {
        Ok(Self {
            n: self.n.copy()?,
            d: self.d.copy()?,
        })
    }

    pub fn try_add(&self, b: &Self) -> Result<Self, PredicateError> {
        Self::new(
            self.n.mul(&b.d)?.add(&b.n.mul(&self.d)?)?,
            self.d.mul(&b.d)?,
        )
    }

    pub fn try_sub(&self, b: &Self) -> Result<Self, PredicateError> {
        Self::new(
            self.n.mul(&b.d)?.sub(&b.n.mul(&self.d)?)?,
            self.d.mul(&b.d)?,
        )
    }

    pub fn try_mul(&self, b: &Self) -> Result<Self, PredicateError> {
        Self::new(self.n.mul(&b.n)?, self.d.mul(&b.d)?)
    }

    pub fn try_div(&self, b: &Self) -> Result<Self, PredicateError> {
        Self::new(self.n.mul(&b.d)?, self.d.mul(&b.n)?)
    }

    pub fn sign(&self) -> Ordering {
        match self.n.value.sign() {
            Sign::Minus => Ordering::Less,
            Sign::NoSign => Ordering::Equal,
            Sign::Plus => Ordering::Greater,
        }
    }

    pub fn try_midpoint(&self, b: &Self) -> Result<Self, PredicateError> {
        self.try_add(b)?
            .try_div(&Self::int_in(2, self.n.lease.budget())?)
    }

    pub fn try_cmp(&self, b: &Self) -> Result<Ordering, PredicateError> {
        let left = self.n.mul(&b.d)?;
        let right = b.n.mul(&self.d)?;
        Ok(left.value.cmp(&right.value))
    }
}
