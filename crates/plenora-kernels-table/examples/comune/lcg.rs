//! Il secondo generatore deterministico delle fixture di benchmark.
//!
//! LCG di Knuth (MMIX) con seme fisso: come per [`super::rng`], le fixture
//! devono essere le stesse a ogni corsa, altrimenti due misure non sono
//! confrontabili e la baseline non significa niente.
//!
//! # Perche' due generatori
//!
//! Non e' un doppione dello xorshift: i benchmark che lo usano producono
//! **altre** sequenze, e sostituirlo con l'altro cambierebbe i dati misurati.
//! Sono due generatori distinti che restano distinti.
//!
//! # Perche' ha un test suo
//!
//! Cambiare seme, moltiplicatore, incremento o l'ordine dell'aggiornamento non
//! rompe nessuna compilazione e non fa fallire nessun benchmark: cambia
//! soltanto i **dati** misurati, in silenzio. La sequenza e' quindi fissata in
//! `tests/impalcatura_bench.rs`, contro valori calcolati fuori da qui.

/// LCG di Knuth (MMIX), seme 42.
pub struct Lcg(u64);

impl Lcg {
    /// Il generatore col seme delle fixture.
    #[must_use]
    pub const fn seeded() -> Self {
        Self(42)
    }

    /// Il generatore con un seme dichiarato.
    ///
    /// Lo usano le fixture che derivano piu' flussi da uno stesso indice:
    /// il seme sta nella chiamata perche' e' li' che si legge da dove viene.
    #[must_use]
    pub const fn con_seme(seme: u64) -> Self {
        Self(seme)
    }

    /// Il valore successivo.
    ///
    /// Lo stato avanza **prima** della lettura, e si scartano gli undici bit
    /// bassi: sono i meno casuali di un LCG, e l'ordine delle due cose fa
    /// parte della sequenza tanto quanto le costanti.
    pub const fn next_u64(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 11
    }

    /// Un valore in `[0, bound)`.
    ///
    /// # Panics
    ///
    /// Panica se `bound` e' zero. Il chiamante deve fornire un limite
    /// positivo.
    pub const fn below(&mut self, bound: u64) -> u64 {
        self.next_u64() % bound
    }
}
