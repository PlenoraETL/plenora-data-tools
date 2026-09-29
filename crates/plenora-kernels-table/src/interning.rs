//! Tabella di chiavi binarie in un'arena contigua.
//!
//! Sostituisce `HashSet<Box<[u8]>>`/`HashMap<String, _>` nei kernel che
//! raggruppano o deduplicano per chiave di riga: una sola allocazione per
//! tutte le chiavi (l'arena) invece di una per chiave distinta, e un indice
//! per chiave (`usize`, in ordine di prima apparizione) da usare come
//! identita' di gruppo.
//!
//! # Esattezza
//!
//! L'hash seleziona soltanto i candidati: l'uguaglianza di due chiavi si
//! decide **sempre** sul confronto dei byte nell'arena. Due chiavi distinte
//! con lo stesso hash a 64 bit stanno nella stessa catena (`successivo`) e
//! restano due identita' diverse; una collisione costa tempo, mai un
//! risultato.
//!
//! L'hash (`hashing::hash_chiave`) e' deterministico: l'ordine degli indici
//! dipende solo dall'ordine di inserimento, non dall'hash.

use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hasher};

use crate::hashing::hash_chiave;

/// Fine catena in `successivo`.
const NESSUNO: usize = usize::MAX;

/// Hasher identita' per le teste di catena: la chiave della mappa e' gia'
/// l'hash finalizzato di `hash_chiave`, rimescolarlo non aggiunge nulla.
#[derive(Default)]
struct HashGiaCalcolato(u64);

impl Hasher for HashGiaCalcolato {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        // La mappa usa solo `u64` (via `write_u64`); il ramo generico resta
        // totale e deterministico per qualunque altro uso.
        for &byte in bytes {
            self.0 = self.0.rotate_left(8) ^ u64::from(byte);
        }
    }

    fn write_u64(&mut self, value: u64) {
        self.0 = value;
    }
}

/// Tabella di chiavi binarie con identita' esatta per confronto di byte.
///
/// Gli indici vanno da `0` a `len() - 1` nell'ordine in cui le chiavi sono
/// state inserite la prima volta.
pub struct KeyInterner {
    /// Byte di tutte le chiavi, concatenati nell'ordine degli indici.
    arena: Vec<u8>,
    /// `fini[i]`: offset di fine della chiave `i` nell'arena (l'inizio e' la
    /// fine della precedente, o zero).
    fini: Vec<usize>,
    /// Prossima chiave con lo stesso hash, o `NESSUNO`.
    successivo: Vec<usize>,
    /// Hash -> ultima chiave inserita con quell'hash (testa della catena).
    teste: HashMap<u64, usize, BuildHasherDefault<HashGiaCalcolato>>,
}

impl KeyInterner {
    /// Tabella vuota con spazio per `chiavi` chiavi distinte (stima).
    pub fn with_capacity(chiavi: usize) -> Self {
        Self {
            arena: Vec::new(),
            fini: Vec::with_capacity(chiavi),
            successivo: Vec::with_capacity(chiavi),
            teste: HashMap::with_capacity_and_hasher(chiavi, BuildHasherDefault::default()),
        }
    }

    /// Numero di chiavi distinte.
    pub const fn len(&self) -> usize {
        self.fini.len()
    }

    /// Byte della chiave `indice`, `None` fuori intervallo.
    pub fn chiave(&self, indice: usize) -> Option<&[u8]> {
        let fine = *self.fini.get(indice)?;
        let inizio = match indice.checked_sub(1) {
            Some(precedente) => *self.fini.get(precedente)?,
            None => 0,
        };
        self.arena.get(inizio..fine)
    }

    fn cerca_con_hash(&self, chiave: &[u8], hash: u64) -> Option<usize> {
        let mut candidato = self.teste.get(&hash).copied().unwrap_or(NESSUNO);
        while candidato != NESSUNO {
            // Gli indici delle catene nascono da `inserisci`: sono sempre
            // validi, e `None` qui non potrebbe mai uguagliare una chiave.
            if self.chiave(candidato) == Some(chiave) {
                return Some(candidato);
            }
            candidato = self.successivo.get(candidato).copied().unwrap_or(NESSUNO);
        }
        None
    }

    /// Indice della chiave, se presente.
    pub fn cerca(&self, chiave: &[u8]) -> Option<usize> {
        self.cerca_con_hash(chiave, hash_chiave(chiave))
    }

    /// Indice della chiave e `true` se e' stata appena inserita.
    pub fn inserisci(&mut self, chiave: &[u8]) -> (usize, bool) {
        self.inserisci_con_hash(chiave, hash_chiave(chiave))
    }

    /// `inserisci` con l'hash gia' calcolato: separato perche' i test
    /// possano forzare le collisioni sul percorso vero.
    fn inserisci_con_hash(&mut self, chiave: &[u8], hash: u64) -> (usize, bool) {
        if let Some(indice) = self.cerca_con_hash(chiave, hash) {
            return (indice, false);
        }
        let indice = self.fini.len();
        self.arena.extend_from_slice(chiave);
        self.fini.push(self.arena.len());
        let testa = self.teste.insert(hash, indice).unwrap_or(NESSUNO);
        self.successivo.push(testa);
        (indice, true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indici_in_ordine_di_prima_apparizione() {
        let mut tabella = KeyInterner::with_capacity(0);
        assert_eq!(tabella.inserisci(b"b"), (0, true));
        assert_eq!(tabella.inserisci(b"a"), (1, true));
        assert_eq!(tabella.inserisci(b"b"), (0, false));
        assert_eq!(tabella.inserisci(b""), (2, true));
        assert_eq!(tabella.inserisci(b""), (2, false));
        assert_eq!(tabella.len(), 3);
        assert_eq!(tabella.chiave(0), Some(b"b".as_slice()));
        assert_eq!(tabella.chiave(1), Some(b"a".as_slice()));
        assert_eq!(tabella.chiave(2), Some(b"".as_slice()));
        assert_eq!(tabella.chiave(3), None);
        assert_eq!(tabella.cerca(b"a"), Some(1));
        assert_eq!(tabella.cerca(b"c"), None);
    }

    #[test]
    fn le_collisioni_di_hash_restano_identita_distinte() {
        // Collisione forzata: le chiavi entrano con lo stesso hash, e solo il
        // confronto dei byte le separa, lungo la catena di `inserisci`.
        let mut tabella = KeyInterner::with_capacity(4);
        let hash = 42_u64;
        for (atteso, chiave) in [b"x".as_slice(), b"y", b"z"].iter().enumerate() {
            assert_eq!(tabella.inserisci_con_hash(chiave, hash), (atteso, true));
        }
        for (atteso, chiave) in [b"x".as_slice(), b"y", b"z"].iter().enumerate() {
            assert_eq!(tabella.inserisci_con_hash(chiave, hash), (atteso, false));
        }
        assert_eq!(tabella.cerca_con_hash(b"x", hash), Some(0));
        assert_eq!(tabella.cerca_con_hash(b"y", hash), Some(1));
        assert_eq!(tabella.cerca_con_hash(b"z", hash), Some(2));
        assert_eq!(tabella.cerca_con_hash(b"w", hash), None);
    }

    #[test]
    fn coincide_con_un_hashmap_su_chiavi_pseudocasuali() {
        let mut tabella = KeyInterner::with_capacity(16);
        let mut riferimento: std::collections::HashMap<Vec<u8>, usize> =
            std::collections::HashMap::new();
        let mut stato = 0x9e37_79b9_7f4a_7c15_u64;
        for _ in 0..20_000 {
            stato ^= stato << 13;
            stato ^= stato >> 7;
            stato ^= stato << 17;
            let lunghezza = usize::try_from(stato % 5).unwrap_or(0);
            let chiave = (0..lunghezza)
                .map(|posizione| u8::try_from((stato >> (posizione * 3)) & 0x3).unwrap_or(0))
                .collect::<Vec<_>>();
            let prossimo = riferimento.len();
            let atteso = *riferimento.entry(chiave.clone()).or_insert(prossimo);
            let (indice, nuovo) = tabella.inserisci(&chiave);
            assert_eq!(indice, atteso);
            assert_eq!(nuovo, atteso == prossimo);
        }
        assert_eq!(tabella.len(), riferimento.len());
    }
}
