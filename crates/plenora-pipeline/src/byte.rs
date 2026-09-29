//! Byte vivi: la memoria Arrow delle tabelle residenti, contata una volta.
//!
//! Ogni buffer Arrow è una vista su un'allocazione condivisa per conteggio di
//! riferimenti: una slice, una rinomina o una selezione di colonne riusano la
//! stessa allocazione. Il conto si fa quindi per **allocazione**, non per
//! vista: la chiave è l'inizio dell'allocazione (`Buffer::data_ptr`), il
//! valore la sua capacità (`Buffer::capacity`), e ogni allocazione si somma
//! una volta sola, anche se la raggiungono più colonne, più tabelle o più
//! slice.
//!
//! Base: `buffers.rs` di `plenora-memory-lab/operations/table_catalog`,
//! corretto. Quello contava `as_ptr`/`len`, cioè inizio e lunghezza della
//! **vista**: una slice contava solo la sua parte e due viste disgiunte della
//! stessa allocazione non si riconoscevano come una.
//!
//! **Esatto** per le allocazioni Arrow fatte da Rust: buffer di valori, di
//! offset, di validità, dei figli (liste, struct, dizionari, run-end,
//! viste), a ogni livello. **Escluso**: l'overhead dell'allocatore e le
//! strutture Rust (`ArrayData`, `Arc`, schemi).
//!
//! **Non esatto** per la memoria esterna (`Deallocation::Custom`: FFI,
//! `bytes::Bytes`): la «capacità» è la dimensione dichiarata
//! dall'importazione, cioè la vista importata, non l'allocazione. Due viste
//! della stessa allocazione esterna con inizi diversi si sommano entrambe,
//! la parte fuori dalle viste non si conta; con lo stesso inizio si tiene la
//! maggiore. L'API pubblica di Arrow non distingue le due deallocazioni, e
//! il limite è dichiarato nel README («Runner»).

use std::collections::BTreeMap;

use plenora_core::arrow::array::{Array, RecordBatch};
use plenora_core::{PlenoraError, Result};

/// Allocazioni raggiunte, per inizio: capacità in byte.
#[derive(Debug, Default)]
pub struct Allocazioni {
    per_inizio: BTreeMap<usize, usize>,
}

impl Allocazioni {
    fn registra(&mut self, inizio: usize, capacita: usize) {
        // Capacità zero: nessuna memoria. Il puntatore di un'allocazione
        // vuota è quello «pendente» dell'allineamento, condiviso da tutte.
        if capacita == 0 {
            return;
        }
        // Per un'allocazione di Rust (`Deallocation::Standard`) inizio e
        // capacità coincidono sempre fra le viste. Per una esterna
        // (`Custom`: FFI, `bytes::Bytes`) la capacità è la dimensione
        // dichiarata dall'importazione, che per due slice importate dello
        // stesso array può differire: si tiene la maggiore, il limite
        // dichiarato nel modulo e nel README.
        let voce = self.per_inizio.entry(inizio).or_insert(0);
        *voce = (*voce).max(capacita);
    }

    /// Aggiunge le allocazioni di una colonna, figli compresi.
    ///
    pub fn aggiungi_colonna(&mut self, colonna: &dyn Array) {
        let mut da_visitare = vec![colonna.to_data()];
        while let Some(dati) = da_visitare.pop() {
            for buffer in dati.buffers() {
                self.registra(buffer.data_ptr().as_ptr().addr(), buffer.capacity());
            }
            if let Some(nulli) = dati.nulls() {
                let buffer = nulli.buffer();
                self.registra(buffer.data_ptr().as_ptr().addr(), buffer.capacity());
            }
            da_visitare.extend(dati.child_data().iter().cloned());
        }
    }

    /// Aggiunge le allocazioni di tutte le colonne di una tabella.
    ///
    pub fn aggiungi_tabella(&mut self, tabella: &RecordBatch) {
        for colonna in tabella.columns() {
            self.aggiungi_colonna(colonna.as_ref());
        }
    }

    /// Somma delle capacità, ogni allocazione una volta.
    ///
    /// # Errors
    ///
    /// `Internal` se la somma non sta in `u64`.
    pub fn totale(&self) -> Result<u64> {
        self.per_inizio
            .values()
            .try_fold(0_u64, |totale, capacita| {
                u64::try_from(*capacita)
                    .ok()
                    .and_then(|capacita| totale.checked_add(capacita))
                    .ok_or_else(|| {
                        PlenoraError::Internal("somma dei byte vivi non rappresentabile".to_owned())
                    })
            })
    }
}

/// Byte Arrow delle tabelle date, ogni allocazione contata una volta.
///
/// # Errors
///
/// `Internal` se la somma non sta in `u64`.
pub fn byte_vivi<'a>(tabelle: impl IntoIterator<Item = &'a RecordBatch>) -> Result<u64> {
    let mut allocazioni = Allocazioni::default();
    for tabella in tabelle {
        allocazioni.aggiungi_tabella(tabella);
    }
    allocazioni.totale()
}
