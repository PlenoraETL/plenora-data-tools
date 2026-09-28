//! Il generatore deterministico delle fixture di benchmark.
//!
//! Xorshift64 con seme fisso: le fixture devono essere le stesse a ogni
//! corsa, altrimenti due misure non sono confrontabili e la baseline non
//! significa niente.
//!
//! # Perche' ha un test suo
//!
//! Cambiare seme o costanti non rompe nessuna compilazione e non fa fallire
//! nessun benchmark: cambia soltanto i **dati** misurati, in silenzio, e la
//! serie storica delle baseline smette di parlare della stessa cosa senza che
//! nulla lo dica. La sequenza e' quindi fissata in
//! `tests/impalcatura_bench.rs`, contro valori calcolati fuori da qui.

/// Xorshift64, seme 42.
pub struct Rng(u64);

impl Rng {
    /// Il generatore col seme delle fixture.
    #[must_use]
    pub const fn seeded() -> Self {
        Self(42)
    }

    /// Il generatore con un seme dichiarato.
    ///
    /// Serve dove una fixture ne vuole due che non si sovrappongano: il
    /// secondo insieme nasce da un seme diverso, e il seme sta nella
    /// chiamata perche' e' li' che si legge perche' e' diverso.
    #[must_use]
    pub const fn con_seme(seme: u64) -> Self {
        Self(seme)
    }

    /// Il valore successivo.
    pub const fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
}
