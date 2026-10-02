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
//!   colonne, più tabelle o più slice. È la misura del budget del runner.
//! - [`byte_viste`] e [`byte_dati`]: **quanti byte costa copiare** una
//!   colonna o una tabella, cioè la somma delle lunghezze delle sue viste. Serve alle stime per riga di un output
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

use std::collections::BTreeMap;

use arrow_data::ArrayData;

use crate::arrow::array::{Array, RecordBatch};
use crate::arrow::DataType;
use crate::{PlenoraError, Result};

/// Un buffer visto da una colonna: inizio e capacità dell'allocazione,
/// lunghezza della vista.
struct Buffer {
    allocazione: usize,
    capacita: usize,
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
/// buffer, figli compresi, contate ogni volta che la colonna le raggiunge.
///
/// È il costo di una copia della colonna, non la memoria tenuta viva (per
/// quella [`byte_vivi`]): due figli che sono lo stesso array contano due
/// volte, perché una copia li duplica; la slice di un buffer che Arrow non
/// affetta (i valori di un `Utf8`) conta tutti i valori; la capacità oltre
/// la lunghezza non conta. Satura a `usize::MAX`, che per una stima vale
/// «oltre qualunque budget».
#[must_use]
pub fn byte_viste(colonna: &dyn Array) -> usize {
    let mut totale = 0_usize;
    per_ogni_buffer(colonna, |buffer| {
        totale = totale.saturating_add(buffer.lunghezza);
    });
    totale
}

/// Byte di dati di una tabella: [`byte_viste`] di ogni colonna, anche di
/// colonne che sono lo stesso array (una copia le duplica).
#[must_use]
pub fn byte_dati(tabella: &RecordBatch) -> usize {
    tabella.columns().iter().fold(0_usize, |totale, colonna| {
        totale.saturating_add(byte_viste(colonna.as_ref()))
    })
}

/// Arrotondamento delle capacità dei buffer che Arrow alloca.
const ARROTONDAMENTO: u64 = 64;
/// Buffer di un nodo della copia contati per il loro arrotondamento, per
/// eccesso: validità, offset, valori, dati delle viste.
const BUFFER_PER_NODO: u64 = 4;

fn in_u64(byte: usize) -> u64 {
    u64::try_from(byte).unwrap_or(u64::MAX)
}

/// I nodi di un tipo: lui e i figli, ricorsivamente (struct, liste, map,
/// dizionari, run-end, union).
fn nodi(tipo: &DataType) -> u64 {
    let figli = match tipo {
        DataType::Struct(campi) => campi.iter().fold(0_u64, |totale, campo| {
            totale.saturating_add(nodi(campo.data_type()))
        }),
        DataType::Union(campi, _) => campi.iter().fold(0_u64, |totale, (_, campo)| {
            totale.saturating_add(nodi(campo.data_type()))
        }),
        DataType::List(campo)
        | DataType::LargeList(campo)
        | DataType::ListView(campo)
        | DataType::LargeListView(campo)
        | DataType::FixedSizeList(campo, _)
        | DataType::Map(campo, _) => nodi(campo.data_type()),
        DataType::Dictionary(_, valori) => nodi(valori),
        DataType::RunEndEncoded(estremi, valori) => {
            nodi(estremi.data_type()).saturating_add(nodi(valori.data_type()))
        }
        _ => 0,
    };
    figli.saturating_add(1)
}

/// Byte di una bitmap di validità piena su ogni nodo di una colonna, con la
/// lunghezza di ogni nodo (i figli di una lista o i valori di un dizionario
/// possono essere più lunghi della colonna).
fn validita_piena(dati: &ArrayData) -> u64 {
    dati.child_data()
        .iter()
        .fold(in_u64(dati.len()).div_ceil(8), |totale, figlio| {
            totale.saturating_add(validita_piena(figlio))
        })
}

/// Picco di memoria dell'unione di più blocchi (`concat_batches`), per
/// eccesso: i blocchi restano vivi mentre si alloca la copia.
///
/// - Blocchi: `get_array_memory_size` di ogni colonna (capacità dei buffer,
///   anche condivisi più volte: per eccesso).
/// - Copia: i byte di dati dei blocchi ([`byte_dati`]: offset, valori,
///   viste, dizionari, figli), più una bitmap di validità piena su ogni
///   nodo (Arrow la materializza sull'intera lunghezza appena un blocco ha
///   dei null), il tutto per due (i buffer della copia crescono per
///   raddoppio), più l'arrotondamento a 64 byte di ogni buffer di ogni nodo.
///   Le viste (`Utf8View`) condividono i buffer di dati dei blocchi: la
///   stima li conta comunque, per eccesso.
/// - Percorso generico (`FixedSizeList`, `Union`): i figli preallocati dalla
///   capacità del padre, anche vuoti ([`preallocati`]).
///
/// Un conto per input, non per allocazione; i transitori interni di
/// `concat` (la fusione dei dizionari) non ci sono. Lo prova un oracolo
/// (`tests/picco_unione.rs`) contro l'unione vera.
#[must_use]
pub fn picco_unione(blocchi: &[RecordBatch]) -> u64 {
    let mut originali = 0_u64;
    let mut copia = 0_u64;
    for blocco in blocchi {
        copia = copia.saturating_add(in_u64(byte_dati(blocco)));
        for colonna in blocco.columns() {
            originali = originali.saturating_add(in_u64(colonna.get_array_memory_size()));
            copia = copia.saturating_add(validita_piena(&colonna.to_data()));
        }
    }
    // I buffer della copia crescono per raddoppio (`MutableBuffer`): la
    // capacità finale arriva al doppio di ciò che serve.
    let totale = originali.saturating_add(copia.saturating_mul(2));
    let nodi_dello_schema = blocchi.first().map_or(0, |blocco| {
        blocco.schema().fields().iter().fold(0_u64, |somma, campo| {
            somma.saturating_add(nodi(campo.data_type()))
        })
    });
    let righe = blocchi.iter().fold(0_u64, |somma, blocco| {
        somma.saturating_add(in_u64(blocco.num_rows()))
    });
    let generici = blocchi.first().map_or(0, |blocco| {
        blocco.schema().fields().iter().fold(0_u64, |somma, campo| {
            somma.saturating_add(preallocati(campo.data_type(), righe, false))
        })
    });
    totale.saturating_add(generici).saturating_add(
        nodi_dello_schema.saturating_mul((BUFFER_PER_NODO + 1).saturating_mul(ARROTONDAMENTO)),
    )
}

/// Byte per riga contati per ogni nodo preallocato, per eccesso: il più
/// largo fra i valori fissi (16 byte, `Decimal128`, viste) e gli offset.
const LARGHEZZA_PREALLOCATA: u64 = 16;

/// I figli che il percorso generico di `concat` (`MutableArrayData`, per
/// `FixedSizeList` e `Union`) prealloca dalla capacità del padre, anche se
/// vuoti: [`LARGHEZZA_PREALLOCATA`] byte per riga del padre (per la
/// dimensione fissa della lista) per ogni discendente, a ogni livello.
/// Sotto un nodo generico anche liste, struct e map ereditano la capacità.
fn preallocati(tipo: &DataType, righe: u64, generico: bool) -> u64 {
    let figlio = |campo: &DataType, righe: u64, generico: bool| {
        let proprio = if generico {
            righe.saturating_mul(LARGHEZZA_PREALLOCATA)
        } else {
            0
        };
        proprio.saturating_add(preallocati(campo, righe, generico))
    };
    match tipo {
        DataType::FixedSizeList(campo, dimensione) => figlio(
            campo.data_type(),
            righe.saturating_mul(u64::try_from(*dimensione).unwrap_or(u64::MAX)),
            true,
        ),
        DataType::Union(campi, _) => campi.iter().fold(0_u64, |somma, (_, campo)| {
            somma.saturating_add(figlio(campo.data_type(), righe, true))
        }),
        DataType::List(campo)
        | DataType::LargeList(campo)
        | DataType::ListView(campo)
        | DataType::LargeListView(campo)
        | DataType::Map(campo, _) => figlio(campo.data_type(), righe, generico),
        DataType::Struct(campi) => campi.iter().fold(0_u64, |somma, campo| {
            somma.saturating_add(figlio(campo.data_type(), righe, generico))
        }),
        _ => 0,
    }
}
