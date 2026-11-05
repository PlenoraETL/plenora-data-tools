//! Stime di memoria su batch letti da Arrow IPC.
//!
//! In un batch letto da IPC tutte le colonne sono viste dello stesso buffer
//! del messaggio, e ogni buffer dichiara come capacità l'intero messaggio.
//! Le stime contavano quella capacità una volta per buffer: circa dieci
//! volte la memoria reale su una tabella larga. La misura del budget del
//! runner è il maggiore fra `byte_vivi` (ogni allocazione una volta) e
//! `byte_dati` (il costo di una copia): qui si verifica su batch letti da
//! IPC e su colonne che sono lo stesso array.

#[macro_use]
#[path = "../../../tests/comune/esito.rs"]
mod esito;

use std::io::Cursor;
use std::sync::Arc;

use plenora_core::arrow::array::{
    Array, ArrayRef, Float64Array, Int64Array, RecordBatch, StringArray,
};
use plenora_core::arrow::ipc::reader::StreamReader;
use plenora_core::arrow::ipc::writer::StreamWriter;
use plenora_core::arrow::schema::{DataType, Field, Schema};
use plenora_kernels_table::{batch_bytes_per_row, column_bytes_per_row};

const RIGHE: usize = 20_000;

/// La misura dei byte d'ingresso del runner (`byte_ingresso` in
/// `plenora-pipeline`): il maggiore fra le allocazioni tenute vive, ognuna
/// una volta, e il costo di una copia.
fn stima_runner(batch: &RecordBatch) -> usize {
    plenora_core::memoria::byte_vivi(std::iter::once(batch))
        .ok()
        .and_then(|byte| usize::try_from(byte).ok())
        .unwrap_or(usize::MAX)
        .max(plenora_core::memoria::byte_dati(batch))
}

/// Dieci colonne: interi, reali e testi.
fn tabella_larga() -> RecordBatch {
    let mut campi = Vec::new();
    let mut colonne: Vec<ArrayRef> = Vec::new();
    for indice in 0..4 {
        campi.push(Field::new(format!("i{indice}"), DataType::Int64, false));
        colonne.push(Arc::new(Int64Array::from_iter_values(
            (0..RIGHE).map(|riga| i64::try_from(riga % (7 + indice)).expect("riga")),
        )));
    }
    for indice in 0..3 {
        campi.push(Field::new(format!("f{indice}"), DataType::Float64, false));
        colonne.push(Arc::new(Float64Array::from_iter_values(
            (0..RIGHE).map(|riga| f64::from(u32::try_from(riga).expect("riga")) * 0.5),
        )));
    }
    for indice in 0..3 {
        campi.push(Field::new(format!("s{indice}"), DataType::Utf8, false));
        colonne.push(Arc::new(StringArray::from_iter_values(
            (0..RIGHE).map(|riga| format!("gruppo-{}-{indice}", riga % 97)),
        )));
    }
    RecordBatch::try_new(Arc::new(Schema::new(campi)), colonne).expect("tabella")
}

/// La tabella scritta e riletta come un solo messaggio IPC.
fn riletta(tabella: &RecordBatch) -> RecordBatch {
    let mut testo = Vec::new();
    {
        let mut scrittore =
            StreamWriter::try_new(&mut testo, &tabella.schema()).expect("scrittore");
        scrittore.write(tabella).expect("scrittura");
        scrittore.finish().expect("chiusura");
    }
    let mut lettore = StreamReader::try_new(Cursor::new(testo), None).expect("lettore");
    let letta = lettore.next().expect("un batch").expect("lettura");
    assert!(lettore.next().is_none());
    letta
}

fn vecchia_stima(batch: &RecordBatch) -> usize {
    batch
        .columns()
        .iter()
        .map(Array::get_array_memory_size)
        .sum()
}

#[test]
fn la_stima_di_un_batch_letto_da_ipc_conta_il_messaggio_una_volta() {
    let originale = tabella_larga();
    let letta = riletta(&originale);
    assert_eq!(letta, originale);
    let stima = stima_runner(&letta);
    let dati: usize = originale
        .columns()
        .iter()
        .map(|colonna| plenora_core::memoria::byte_viste(colonna.as_ref()))
        .sum();
    // Il messaggio contiene i dati di tutte le colonne più il padding.
    assert!(stima >= dati, "{stima} < {dati}");
    assert!(stima <= dati + dati / 10, "{stima} oltre i dati {dati}");
    // La somma per colonna lo contava una volta per buffer.
    assert!(vecchia_stima(&letta) > 5 * stima);
    // Un batch costruito in memoria: stessa misura dell'originale, a meno
    // della capacità inutilizzata dei builder.
    assert!(stima_runner(&originale) >= dati);
}

#[test]
fn le_stime_per_riga_non_dipendono_da_come_il_batch_e_stato_letto() {
    let originale = tabella_larga();
    let letta = riletta(&originale);
    // Sette colonne a 8 byte; tre Utf8 con (n+1) offset i32 e
    // i byte delle stringhe costruite dalla fixture, senza bitmap null.
    let byte_testi: usize = (0..RIGHE)
        .map(|riga| format!("gruppo-{}-0", riga % 97).len())
        .sum();
    let testo_per_riga = (4 * (RIGHE + 1) + byte_testi).div_ceil(RIGHE);
    let attese = [
        8,
        8,
        8,
        8,
        8,
        8,
        8,
        testo_per_riga,
        testo_per_riga,
        testo_per_riga,
    ];
    for tabella in [&originale, &letta] {
        for (colonna, attesa) in tabella.columns().iter().zip(attese) {
            assert_eq!(column_bytes_per_row(colonna.as_ref()), attesa);
        }
        assert_successo!(batch_bytes_per_row(tabella), attese.iter().sum::<usize>());
    }
}

#[test]
fn colonne_che_sono_lo_stesso_array_contano_ciascuna() {
    // Otto colonne sullo stesso array: una allocazione tenuta viva, otto
    // copie in `select_rows` e nel sort.
    let colonna: ArrayRef = Arc::new(Int64Array::from_iter_values(0..10_000));
    let campi: Vec<Field> = (0..8)
        .map(|indice| Field::new(format!("c{indice}"), DataType::Int64, false))
        .collect();
    let batch =
        RecordBatch::try_new(Arc::new(Schema::new(campi)), vec![colonna; 8]).expect("batch");
    let una = plenora_core::memoria::byte_vivi(std::iter::once(&batch)).expect("byte");
    let copia = plenora_kernels_table::select_rows(&batch, &(0..10_000).collect::<Vec<_>>())
        .expect("copia");
    let byte_copia = plenora_core::memoria::byte_vivi(std::iter::once(&copia)).expect("byte");
    assert!(byte_copia >= 8 * una - 8 * 64);
    assert!(stima_runner(&batch) >= 80_000 * 8);
}
