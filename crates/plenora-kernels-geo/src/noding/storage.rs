//! Prenotazioni vive: il proprietario della capacita' possiede la ricevuta.

use crate::predicates::PredicateError;
use std::cell::Cell;
use std::rc::Rc;

#[derive(Debug)]
pub(super) struct Budget {
    limit: u64,
    pub(super) deadline: Option<std::time::Instant>,
    live: Cell<u64>,
    peak: Cell<u64>,
    attempted_peak: Cell<u64>,
}

impl Budget {
    pub(super) const fn new(limit: u64) -> Self {
        Self {
            limit,
            deadline: None,
            live: Cell::new(0),
            peak: Cell::new(0),
            attempted_peak: Cell::new(0),
        }
    }

    fn reserve(&self, bytes: u64) -> Result<(), PredicateError> {
        let next = self
            .live
            .get()
            .checked_add(bytes)
            .filter(|next| *next <= self.limit)
            .ok_or(PredicateError::ReservationLimit)?;
        self.live.set(next);
        self.peak.set(self.peak.get().max(next));
        Ok(())
    }

    #[cfg(test)]
    #[deny(clippy::disallowed_methods)]
    pub(super) const fn peak(&self) -> u64 {
        self.peak.get()
    }

    #[cfg(test)]
    #[deny(clippy::disallowed_methods)]
    pub(super) const fn live(&self) -> u64 {
        self.live.get()
    }

    #[cfg(test)]
    #[deny(clippy::disallowed_methods)]
    pub(super) const fn attempted_peak(&self) -> u64 {
        self.attempted_peak.get()
    }
}

#[derive(Debug)]
pub(super) struct Lease {
    budget: Rc<Budget>,
    bytes: u64,
}

impl Lease {
    /// Addebito immediatamente successivo alla creazione di un valore.
    /// Un rifiuto registra anche il picco tentato, senza lasciare ricevute.
    pub(super) fn created(budget: &Rc<Budget>, bytes: u64) -> Result<Self, PredicateError> {
        let attempted = budget
            .live
            .get()
            .checked_add(bytes)
            .ok_or(PredicateError::ReservationLimit)?;
        budget
            .attempted_peak
            .set(budget.attempted_peak.get().max(attempted));
        budget.reserve(bytes)?;
        Ok(Self {
            budget: Rc::clone(budget),
            bytes,
        })
    }

    pub(super) const fn budget(&self) -> &Rc<Budget> {
        &self.budget
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        // Ogni ricevuta rilascia una sola volta cio' che ha prenotato.
        self.budget.live.set(self.budget.live.get() - self.bytes);
    }
}

pub struct CountedVec<T> {
    // I campi vengono distrutti in ordine: capacita' prima, ricevuta dopo.
    values: Vec<T>,
    lease: Option<Lease>,
}

impl<T> Default for CountedVec<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> CountedVec<T> {
    pub const fn new() -> Self {
        Self {
            values: Vec::new(),
            lease: None,
        }
    }

    pub(super) fn push(&mut self, value: T, budget: &Rc<Budget>) -> Result<(), PredicateError> {
        if self
            .lease
            .as_ref()
            .is_some_and(|lease| !Rc::ptr_eq(&lease.budget, budget))
        {
            return Err(PredicateError::CalcoloNonConcluso(
                "proprietario del budget",
            ));
        }
        if self.values.len() == self.values.capacity() {
            let requested = self
                .values
                .capacity()
                .max(1)
                .checked_mul(if self.values.capacity() == 0 { 1 } else { 2 })
                .ok_or(PredicateError::ReservationLimit)?;
            let width = u64::try_from(std::mem::size_of::<T>())
                .map_err(|_| PredicateError::ReservationLimit)?;
            let additional = requested - self.values.capacity();
            let bytes = u64::try_from(additional)
                .map_err(|_| PredicateError::ReservationLimit)?
                .checked_mul(width)
                .ok_or(PredicateError::ReservationLimit)?;
            budget.reserve(bytes)?;
            // Ricevuta creata PRIMA della crescita. Un errore del sistema
            // rilascia la prenotazione senza alterare la ricevuta esistente.
            let mut added = Lease {
                budget: Rc::clone(budget),
                bytes,
            };
            // GlobalAlloc::realloc puo' allocare il nuovo buffer prima di
            // liberare quello vecchio. Prenota anche la vecchia capacita'
            // durante la crescita: il rifiuto avviene PRIMA di allocare.
            let overlap_bytes = u64::try_from(self.values.capacity())
                .map_err(|_| PredicateError::ReservationLimit)?
                .checked_mul(width)
                .ok_or(PredicateError::ReservationLimit)?;
            budget.reserve(overlap_bytes)?;
            let overlap = Lease {
                budget: Rc::clone(budget),
                bytes: overlap_bytes,
            };
            self.values
                .try_reserve_exact(additional)
                .map_err(|_| PredicateError::AllocationFailed)?;
            drop(overlap);
            // Il contratto portabile di Vec consente sovraprenotazioni.
            // Rifiutare esplicitamente un backend che le applicasse: non
            // restituire mai successo con capacita' non contabilizzata.
            // Questo controllo a posteriori NON certifica il picco fisico
            // dell'allocatore e non sostituisce il lavoro restante GOP-027.
            if self.values.capacity() != requested {
                self.values = Vec::new();
                self.lease = None;
                return Err(PredicateError::CalcoloNonConcluso(
                    "capacita' della prenotazione",
                ));
            }
            if let Some(lease) = &mut self.lease {
                lease.bytes += bytes;
                added.bytes = 0;
            } else {
                self.lease = Some(added);
            }
        }
        self.values.push(value);
        Ok(())
    }

    // Trasferisce il buffer all'output mantenendo la ricevuta fino al ritorno.
    pub fn export(self, work: &mut super::Work<'_>) -> Result<Vec<T>, PredicateError> {
        if let Some(lease) = self.lease {
            if !Rc::ptr_eq(lease.budget(), &work.budget) {
                return Err(PredicateError::CalcoloNonConcluso(
                    "proprietario del budget",
                ));
            }
            work.step()?;
            work.exports.push(lease, &work.budget)?;
        }
        Ok(self.values)
    }

    pub fn pop(&mut self) -> Option<T> {
        self.values.pop()
    }

    pub fn dedup_by(
        &mut self,
        mut equal: impl FnMut(&T, &T) -> Result<bool, PredicateError>,
        work: &mut super::Work<'_>,
    ) -> Result<(), PredicateError> {
        let mut kept = 0;
        for read in 0..self.len() {
            work.step()?;
            if kept == 0 || !equal(&self[read], &self[kept - 1])? {
                self.values.swap(kept, read);
                kept += 1;
            }
        }
        while self.len() > kept {
            work.step()?;
            self.pop();
        }
        Ok(())
    }
}

impl<T> std::ops::Deref for CountedVec<T> {
    type Target = [T];
    fn deref(&self) -> &[T] {
        &self.values
    }
}
impl<T> std::ops::DerefMut for CountedVec<T> {
    fn deref_mut(&mut self) -> &mut [T] {
        &mut self.values
    }
}
impl<'a, T> IntoIterator for &'a CountedVec<T> {
    type Item = &'a T;
    type IntoIter = std::slice::Iter<'a, T>;
    fn into_iter(self) -> Self::IntoIter {
        self.values.iter()
    }
}

pub struct BufferIter<T> {
    values: std::vec::IntoIter<T>,
    _lease: Option<Lease>,
}
impl<T> Iterator for BufferIter<T> {
    type Item = T;
    fn next(&mut self) -> Option<T> {
        self.values.next()
    }
}
impl<T> IntoIterator for CountedVec<T> {
    type Item = T;
    type IntoIter = BufferIter<T>;
    fn into_iter(self) -> Self::IntoIter {
        BufferIter {
            values: self.values.into_iter(),
            _lease: self.lease,
        }
    }
}

#[cfg(test)]
#[deny(clippy::disallowed_methods)]
mod tests {
    use super::*;

    /// GOP-027: la capacita' rimane viva dopo pop e durante `IntoIter`;
    /// il drop la libera, consentendo una successiva prenotazione uguale.
    #[test]
    fn capacity_lifetime_and_failed_growth() {
        let budget = Rc::new(Budget::new(8));
        let mut values = CountedVec::new();
        values.push(1_u64, &budget).expect("prima prenotazione");
        assert!(matches!(
            values.push(2, &budget),
            Err(PredicateError::ReservationLimit)
        ));
        assert_eq!(values.pop(), Some(1));
        assert_eq!(budget.live.get(), 8);
        values.push(3, &budget).expect("riusa la capacita'");
        let mut iter = values.into_iter();
        assert_eq!(iter.next(), Some(3));
        assert_eq!(budget.live.get(), 8);
        drop(iter);
        assert_eq!(budget.live.get(), 0);
        let mut next = CountedVec::new();
        next.push(4_u64, &budget).expect("riusa il budget");
        assert_eq!(budget.peak(), 8);
    }

    /// Un buffer non puo' trasferire una ricevuta a un bilancio diverso.
    #[test]
    fn changing_budget_is_rejected_before_growth() {
        let first = Rc::new(Budget::new(8));
        let second = Rc::new(Budget::new(8));
        let mut values = CountedVec::new();
        values.push(1_u64, &first).expect("prima prenotazione");
        assert!(matches!(
            values.push(2, &second),
            Err(PredicateError::CalcoloNonConcluso(_))
        ));
        assert_eq!(second.live.get(), 0);
        drop(values);
        assert_eq!(first.live.get(), 0);
    }

    /// Prova della capacita' esposta dal backend della toolchain fissata;
    /// non e' una prova del picco interno dell'allocatore di sistema.
    #[test]
    fn requested_capacity_matches_the_pinned_vector_backend() {
        let count = 1024_u64;
        let width = std::mem::size_of::<u64>() as u64;
        let peak = (count + count / 2) * width;
        let budget = Rc::new(Budget::new(peak));
        let mut values = CountedVec::new();
        for i in 0..count {
            values.push(i, &budget).expect("capacita' richiesta");
            assert_eq!(budget.live.get(), (i + 1).next_power_of_two() * width);
        }
        assert_eq!(values.values.capacity(), 1024);
        assert_eq!(budget.peak(), peak);
    }

    /// La crescita fallisce prima di allocare se manca il solo vecchio
    /// buffer transitorio; la capacita' e la ricevuta originali sopravvivono.
    #[test]
    fn growth_overlap_is_reserved_before_allocation() {
        let width = std::mem::size_of::<u64>() as u64;
        let budget = Rc::new(Budget::new(3 * width - 1));
        let mut values = CountedVec::new();
        values.push(1_u64, &budget).expect("primo elemento");
        assert!(matches!(
            values.push(2, &budget),
            Err(PredicateError::ReservationLimit)
        ));
        assert_eq!(values.values.capacity(), 1);
        assert_eq!(&*values, &[1]);
        assert_eq!(budget.live(), width);
        drop(values);
        assert_eq!(budget.live(), 0);
    }
}
