//! Il margine di memoria dei kernel geo: quanti byte un kernel puo' ancora
//! far crescere con i suoi risultati e i suoi insiemi candidati prima di
//! fermarsi con un errore esplicito.
//!
//! **Perche'.** Il runner prevede il picco di un passo dal modello di costo
//! (`plenora-pipeline`, «Budget di memoria»), ma alcuni profili crescono
//! con una grandezza che a secco non si conosce: le coppie candidate di un
//! join spaziale in cui ogni riga ne tocca molte, i pezzi di un overlay di
//! poligoni che si sovrappongono, le sovrapposizioni di una copertura, i
//! vicini equidistanti. Senza margine il kernel li alloca tutti, e il
//! runner se ne accorge solo dopo il passo, o mai se la memoria finisce
//! prima. Con il margine il kernel conta, nei punti in cui quei vettori
//! crescono, i byte che vi ha messo (piu' quelli che il chiamante spende
//! per ogni risultato nell'uscita, [`MargineMemoria::con_uscita_per_risultato`])
//! e si ferma con un `ResourceLimit` prima di superarlo.
//!
//! **Che cosa conta e che cosa no.** Si contano i risultati che il kernel
//! trattiene (coppie, pezzi, problemi, vicini, uscite del buffer) con stime
//! per eccesso della loro rappresentazione in memoria
//! ([`crate::memory_estimate`] per le geometrie), non il transitorio dentro
//! una chiamata di `geo` o `i_overlay`, che non si osserva; il controllo e'
//! deterministico (conteggi nell'ordine delle righe, o un totale che non
//! dipende dai thread). Nei kernel per riga (`buffer`) il margine vale per
//! ogni riga: righe in parallelo possono trattenere insieme fino a tanti
//! margini quanti sono i thread (limite dichiarato nel README).

use thiserror::Error;

/// Il margine di memoria passato a un kernel geo.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MargineMemoria {
    byte: u64,
    uscita_per_risultato: u64,
}

impl MargineMemoria {
    /// Nessun margine: i kernel chiamati senza runner, come prima.
    pub const ILLIMITATO: Self = Self {
        byte: u64::MAX,
        uscita_per_risultato: 0,
    };

    /// Un margine di `byte` byte.
    #[must_use]
    pub const fn byte(byte: u64) -> Self {
        Self {
            byte,
            uscita_per_risultato: 0,
        }
    }

    /// Lo stesso margine, con `byte` byte che il chiamante spendera' per
    /// ogni risultato nell'uscita (per esempio la riga sinistra ripetuta di
    /// un join): il kernel li conta insieme ai propri.
    #[must_use]
    pub const fn con_uscita_per_risultato(self, byte: u64) -> Self {
        Self {
            byte: self.byte,
            uscita_per_risultato: byte,
        }
    }

    /// I byte del margine.
    #[must_use]
    pub const fn byte_disponibili(self) -> u64 {
        self.byte
    }

    /// I byte di un risultato: quelli del kernel piu' quelli dell'uscita.
    #[must_use]
    pub const fn byte_per_risultato(self, nel_kernel: u64) -> u64 {
        nel_kernel.saturating_add(self.uscita_per_risultato)
    }

    /// Quanti risultati da `nel_kernel` byte (piu' l'uscita) entrano nel
    /// margine; nessun tetto ([`u64::MAX`]) senza margine.
    #[must_use]
    pub const fn risultati_massimi(self, nel_kernel: u64) -> u64 {
        let per_risultato = self.byte_per_risultato(nel_kernel);
        if per_risultato == 0 || self.byte == u64::MAX {
            u64::MAX
        } else {
            self.byte / per_risultato
        }
    }

    /// `Ok` se `previsti` byte stanno nel margine.
    ///
    /// # Errors
    ///
    /// [`MargineSuperato`] se non ci stanno.
    pub const fn verifica(self, previsti: u64) -> Result<(), MargineSuperato> {
        if previsti <= self.byte {
            Ok(())
        } else {
            Err(MargineSuperato {
                previsti,
                margine: self.byte,
            })
        }
    }
}

/// Un kernel geo si e' fermato prima di superare il margine di memoria.
/// Solo conteggi di byte, nessun valore dei dati.
#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
#[error(
    "memoria prevista {previsti} byte oltre il margine di {margine} byte \
     (max_governed_memory_bytes)"
)]
pub struct MargineSuperato {
    /// I byte che il kernel avrebbe trattenuto (almeno), oltre il margine.
    pub previsti: u64,
    /// Il margine.
    pub margine: u64,
}

/// Il tetto dei risultati di un kernel che ha gia' un limite di conteggio
/// (`max_pairs`, `max_results`): il minore fra quel limite e i risultati
/// che entrano nel margine.
#[derive(Clone, Copy, Debug)]
pub struct TettoRisultati {
    limite: u64,
    tetto_memoria: u64,
    byte_per_risultato: u64,
    margine: u64,
}

impl TettoRisultati {
    /// Il tetto per `limite` risultati da `nel_kernel` byte nel `margine`.
    #[must_use]
    pub const fn nuovo(limite: u64, margine: MargineMemoria, nel_kernel: u64) -> Self {
        Self {
            limite,
            tetto_memoria: margine.risultati_massimi(nel_kernel),
            byte_per_risultato: margine.byte_per_risultato(nel_kernel),
            margine: margine.byte_disponibili(),
        }
    }

    /// Quanti risultati si possono trattenere.
    #[must_use]
    pub const fn tetto(&self) -> u64 {
        if self.limite < self.tetto_memoria {
            self.limite
        } else {
            self.tetto_memoria
        }
    }

    /// Superato il tetto: `None` se a fermare e' il limite di conteggio
    /// (l'errore e' quello del kernel), il margine superato altrimenti.
    #[must_use]
    pub const fn superato_dalla_memoria(&self) -> Option<MargineSuperato> {
        if self.limite <= self.tetto_memoria {
            None
        } else {
            Some(MargineSuperato {
                previsti: self
                    .tetto_memoria
                    .saturating_add(1)
                    .saturating_mul(self.byte_per_risultato),
                margine: self.margine,
            })
        }
    }
}

/// Un contatore dei byte trattenuti, in un kernel sequenziale.
#[derive(Clone, Copy, Debug)]
pub struct ContatoreMargine {
    margine: MargineMemoria,
    usati: u64,
}

impl ContatoreMargine {
    /// Un contatore vuoto sul margine.
    #[must_use]
    pub const fn nuovo(margine: MargineMemoria) -> Self {
        Self { margine, usati: 0 }
    }

    /// Aggiunge un risultato da `nel_kernel` byte (piu' l'uscita).
    ///
    /// # Errors
    ///
    /// [`MargineSuperato`] se il totale supera il margine.
    pub fn aggiungi(&mut self, nel_kernel: u64) -> Result<(), MargineSuperato> {
        let totale = self
            .usati
            .saturating_add(self.margine.byte_per_risultato(nel_kernel));
        self.margine.verifica(totale)?;
        self.usati = totale;
        Ok(())
    }

    /// Un contatore sul margine con `usati` byte gia' contati.
    #[must_use]
    pub const fn con_usati(margine: MargineMemoria, usati: u64) -> Self {
        Self { margine, usati }
    }

    /// I byte contati finora.
    #[must_use]
    pub const fn usati(&self) -> u64 {
        self.usati
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn margine_e_contatore() {
        let margine = MargineMemoria::byte(100).con_uscita_per_risultato(10);
        assert_eq!(margine.byte_per_risultato(15), 25);
        assert_eq!(margine.risultati_massimi(15), 4);
        assert_eq!(MargineMemoria::byte(7).risultati_massimi(0), u64::MAX);
        assert!(margine.verifica(100).is_ok());
        assert_eq!(
            margine.verifica(101),
            Err(MargineSuperato {
                previsti: 101,
                margine: 100
            })
        );
        let mut contatore = ContatoreMargine::nuovo(margine);
        for _ in 0..4 {
            contatore.aggiungi(15).unwrap();
        }
        assert_eq!(contatore.usati(), 100);
        assert_eq!(
            contatore.aggiungi(0),
            Err(MargineSuperato {
                previsti: 110,
                margine: 100
            })
        );
        assert_eq!(contatore.usati(), 100);
        assert_eq!(MargineMemoria::ILLIMITATO.risultati_massimi(64), u64::MAX);
    }
}
