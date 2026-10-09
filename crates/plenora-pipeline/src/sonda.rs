//! Punti di sonda delle prove dei controlli: istanti del lavoro che una
//! prova non sa raggiungere dall'esterno se non a tempo (prima del lavoro,
//! fra due passi del piano, fra la scrittura di due output).
//!
//! Non è API: la registra solo il modulo nativo dell'SDK Python, nelle sue
//! prove. Senza sonda registrata ogni punto costa una lettura atomica
//! (`Acquire`) e non fa nient'altro: nessun lucchetto, nessuna allocazione.
//! La sonda è del processo; chi la registra la toglie.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use plenora_core::{PlenoraError, Result};

use crate::Interruzione;

/// Dove gira la sonda.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Punto {
    /// Nel thread del lavoro, prima che cominci (lo chiama chi esegue il
    /// lavoro, con [`chiama_prima`]).
    Prima,
    /// Fra due passi del piano: dopo un passo, prima del controllo
    /// dell'interruzione del passo successivo.
    FraPassi,
    /// Fra la scrittura di due output: dopo un output scritto, prima del
    /// controllo dell'interruzione del successivo.
    FraScritture,
}

impl Punto {
    /// Il nome stabile del punto.
    #[must_use]
    pub const fn nome(self) -> &'static str {
        match self {
            Self::Prima => "prima",
            Self::FraPassi => "fra_passi",
            Self::FraScritture => "fra_scritture",
        }
    }
}

/// Che cosa fa il lavoro dopo la sonda.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Azione {
    /// Prosegue: il controllo successivo decide.
    Prosegui,
    /// Porta la scadenza del lavoro ad adesso, se ne ha una: il controllo
    /// successivo scade. Solo a [`Punto::Prima`], dove l'interruzione è
    /// ancora di chi esegue il lavoro.
    AnticipaScadenza,
}

/// La sonda: riceve il punto e l'interruzione del lavoro.
pub type Sonda = Arc<dyn Fn(Punto, &Interruzione) -> Result<Azione> + Send + Sync>;

/// Se una sonda è registrata: il percorso normale legge solo questo.
static ATTIVA: AtomicBool = AtomicBool::new(false);
static SONDA: Mutex<Option<Sonda>> = Mutex::new(None);

/// Registra o toglie (`None`) la sonda. La precedente si rilascia fuori dal
/// lucchetto.
pub fn registra(sonda: Option<Sonda>) {
    let attiva = sonda.is_some();
    let precedente = SONDA
        .lock()
        .ok()
        .and_then(|mut registrata| std::mem::replace(&mut *registrata, sonda));
    ATTIVA.store(attiva, Ordering::Release);
    drop(precedente);
}

/// Lo snapshot della sonda registrata, letto una volta; `None` senza
/// lucchetto se non ce n'è.
fn attuale() -> Result<Option<Sonda>> {
    if !ATTIVA.load(Ordering::Acquire) {
        return Ok(None);
    }
    SONDA
        .lock()
        .map(|registrata| registrata.clone())
        .map_err(|_| PlenoraError::Internal("sonda del lavoro non leggibile".to_owned()))
}

/// Chiama la sonda a `punto`, se c'è.
///
/// # Errors
///
/// Quello della sonda; `Internal` se la sonda chiede di anticipare la
/// scadenza fuori da [`Punto::Prima`].
pub fn chiama(punto: Punto, interruzione: &Interruzione) -> Result<()> {
    let Some(sonda) = attuale()? else {
        return Ok(());
    };
    match sonda(punto, interruzione)? {
        Azione::Prosegui => Ok(()),
        Azione::AnticipaScadenza => Err(PlenoraError::Internal(
            "la sonda anticipa la scadenza solo prima del lavoro".to_owned(),
        )),
    }
}

/// Chiama la sonda a [`Punto::Prima`], se c'è, e applica la sua azione a
/// `interruzione`.
///
/// # Errors
///
/// Quello della sonda.
pub fn chiama_prima(interruzione: &mut Interruzione) -> Result<()> {
    let Some(sonda) = attuale()? else {
        return Ok(());
    };
    if sonda(Punto::Prima, interruzione)? == Azione::AnticipaScadenza {
        let adesso = Instant::now();
        interruzione.scadenza = interruzione.scadenza.map(|scadenza| scadenza.min(adesso));
    }
    Ok(())
}
