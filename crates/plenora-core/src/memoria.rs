//! Byte Arrow di tabelle e colonne: la misura unica del workspace.
//!
//! Due misure, per due domande diverse.
//!
//! - [`byte_vivi`] (e [`Allocazioni`]): **quanta memoria tengono vive** delle
//!   tabelle. Ogni buffer Arrow è una vista su un'allocazione condivisa per
//!   conteggio di riferimenti: una slice, una rinomina, una selezione di
//!   colonne o un batch letto da Arrow IPC (dove tutte le colonne sono viste
//!   dello stesso buffer del messaggio) riusano la stessa allocazione. Il
//!   conto si fa quindi per **allocazione**: la chiave è l'inizio
//!   (`Buffer::data_ptr`), il valore la capacità (`Buffer::capacity`), e
//!   ogni allocazione si somma una volta sola, anche se la raggiungono più
//!   colonne, più tabelle o più slice. È la misura del budget del runner e
//!   della stima con cui i kernel decidono lo spill
//!   (`plenora_kernels_table::spill::estimated_batch_bytes`).
//! - [`byte_viste`]: **quanti byte di dati** usa una colonna, cioè la somma
//!   delle lunghezze delle sue viste. Serve alle stime per riga di un output
//!   ancora da costruire (`column_bytes_per_row` dei kernel): la capacità di
//!   un'allocazione condivisa non appartiene a nessuna colonna in
//!   particolare. Contare la capacità per colonna gonfiava di circa dieci
//!   volte i batch letti da IPC, dove ogni buffer di ogni colonna dichiara
//!   come capacità l'intero messaggio.
//!
//! Base di `byte_vivi`: `buffers.rs` di
//! `plenora-memory-lab/operations/table_catalog`, corretto. Quello contava
//! `as_ptr`/`len`, cioè la **vista**: una slice contava solo la sua parte e
//! due viste disgiunte della stessa allocazione non si riconoscevano come
//! una.
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
//! il limite è dichiarato nel README di `plenora-data-tools2` («Runner»).

use std::collections::{BTreeMap, BTreeSet};

use crate::arrow::array::{Array, RecordBatch};
use crate::{PlenoraError, Result};

/// Un buffer visto da una colonna: inizio e capacità dell'allocazione,
/// inizio e lunghezza della vista.
struct Buffer {
    allocazione: usize,
    capacita: usize,
    vista: usize,
    lunghezza: usize,
}

/// Visita i buffer di una colonna, a ogni livello: valori, offset,
/// validità, figli.
fn per_ogni_buffer(colonna: &dyn Array, mut su_buffer: impl FnMut(Buffer)) {
    let mut da_visitare = vec![colonna.to_data()];
    while let Some(dati) = da_visitare.pop() {
        let validita = dati.nulls().map(|nulli| nulli.inner().inner());
        for buffer in dati.buffers().iter().chain(validita) {
            su_buffer(Buffer {
                allocazione: buffer.data_ptr().as_ptr().addr(),
                capacita: buffer.capacity(),
                vista: buffer.as_ptr().addr(),
                lunghezza: buffer.len(),
            });
        }
        da_visitare.extend(dati.child_data().iter().cloned());
    }
}

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
        // dichiarato nel modulo.
        let voce = self.per_inizio.entry(inizio).or_insert(0);
        *voce = (*voce).max(capacita);
    }

    /// Aggiunge le allocazioni di una colonna, figli compresi.
    pub fn aggiungi_colonna(&mut self, colonna: &dyn Array) {
        per_ogni_buffer(colonna, |buffer| {
            self.registra(buffer.allocazione, buffer.capacita);
        });
    }

    /// Aggiunge le allocazioni di tutte le colonne di una tabella.
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

/// Byte di dati di una colonna: somma delle lunghezze delle viste dei suoi
/// buffer, figli compresi, ogni vista (inizio e lunghezza) una volta.
///
/// Non è memoria tenuta viva (per quella [`byte_vivi`]): la slice di un
/// buffer che Arrow non affetta (i valori di un `Utf8`) conta tutti i
/// valori, la capacità oltre la lunghezza non conta. Satura a `usize::MAX`,
/// che per una stima vale «oltre qualunque budget».
#[must_use]
pub fn byte_viste(colonna: &dyn Array) -> usize {
    let mut viste: BTreeSet<(usize, usize)> = BTreeSet::new();
    per_ogni_buffer(colonna, |buffer| {
        if buffer.lunghezza > 0 {
            viste.insert((buffer.vista, buffer.lunghezza));
        }
    });
    viste.iter().fold(0_usize, |totale, (_, lunghezza)| {
        totale.saturating_add(*lunghezza)
    })
}
