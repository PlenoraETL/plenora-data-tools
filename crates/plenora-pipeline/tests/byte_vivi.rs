//! `byte_vivi` contro un calcolo indipendente su array noti: allocazioni
//! contate per intero e una volta sola, anche se raggiunte da slice,
//! colonne ripetute, tabelle diverse o figli condivisi.

use std::sync::Arc;

use plenora_core::arrow::array::builder::{Int64Builder, ListBuilder, StringBuilder};
use plenora_core::arrow::array::types::Int32Type;
use plenora_core::arrow::array::{
    Array, ArrayRef, DictionaryArray, Int32Array, Int64Array, RecordBatch, StringArray, StructArray,
};
use plenora_core::arrow::schema::{DataType, Field, Fields, Schema};
use plenora_pipeline::byte_vivi;

fn tabella(colonne: Vec<(&str, ArrayRef)>) -> RecordBatch {
    let campi: Vec<Field> = colonne
        .iter()
        .map(|(nome, colonna)| Field::new(*nome, colonna.data_type().clone(), true))
        .collect();
    RecordBatch::try_new(
        Arc::new(Schema::new(campi)),
        colonne.into_iter().map(|(_, colonna)| colonna).collect(),
    )
    .expect("tabella")
}

fn byte(tabelle: &[&RecordBatch]) -> u64 {
    byte_vivi(tabelle.iter().copied()).expect("byte vivi")
}

fn memoria(array: &dyn Array) -> u64 {
    u64::try_from(array.get_buffer_memory_size()).expect("byte")
}

/// Un vettore con capacità ben più grande della lunghezza: l'allocazione
/// conta per intero, non per le righe usate.
fn interi_con_capacita() -> (ArrayRef, u64) {
    let mut valori: Vec<i64> = Vec::with_capacity(100);
    valori.extend(0..10);
    let capacita = u64::try_from(valori.capacity() * std::mem::size_of::<i64>()).expect("byte");
    (Arc::new(Int64Array::from(valori)), capacita)
}

#[test]
fn un_allocazione_conta_per_la_capacita_non_per_la_vista() {
    let (colonna, capacita) = interi_con_capacita();
    assert!(capacita >= 800);
    let intera = tabella(vec![("x", colonna)]);
    assert_eq!(byte(&[&intera]), capacita);
    // Una slice di tre righe tiene viva tutta l'allocazione: contare la
    // vista (inizio e lunghezza) darebbe 24 byte.
    let fetta = intera.slice(2, 3);
    assert_eq!(byte(&[&fetta]), capacita);
    // Due viste disgiunte della stessa allocazione sono un'allocazione.
    assert_eq!(byte(&[&intera.slice(0, 2), &intera.slice(5, 4)]), capacita);
}

#[test]
fn un_allocazione_raggiunta_piu_volte_conta_una_volta() {
    let (colonna, capacita) = interi_con_capacita();
    let doppia = tabella(vec![("x", colonna.clone()), ("y", colonna.clone())]);
    assert_eq!(byte(&[&doppia]), capacita);
    let altra = tabella(vec![("z", colonna)]);
    assert_eq!(byte(&[&doppia, &altra]), capacita);
}

#[test]
fn tabelle_senza_condivisioni_sommano_la_memoria_dei_buffer() {
    let con_null: ArrayRef = Arc::new(Int64Array::from(vec![Some(1), None, Some(3), None]));
    let testo: ArrayRef = Arc::new(StringArray::from(vec![
        Some("alfa"),
        None,
        Some("gamma"),
        Some(""),
    ]));

    let mut costruttore = ListBuilder::new(Int64Builder::new());
    for riga in 0..4_i64 {
        for elemento in 0..riga {
            costruttore.values().append_value(elemento);
        }
        costruttore.append(riga % 2 == 0);
    }
    let lista: ArrayRef = Arc::new(costruttore.finish());

    let mut nomi = StringBuilder::new();
    for nome in ["a", "bb", "ccc", "dddd"] {
        nomi.append_value(nome);
    }
    let struttura: ArrayRef = Arc::new(StructArray::new(
        Fields::from(vec![
            Field::new("n", DataType::Int64, true),
            Field::new("s", DataType::Utf8, true),
        ]),
        vec![
            Arc::new(Int64Array::from(vec![Some(1), None, Some(3), Some(4)])),
            Arc::new(nomi.finish()),
        ],
        None,
    ));

    let dizionario: ArrayRef = Arc::new(
        DictionaryArray::<Int32Type>::try_new(
            Int32Array::from(vec![Some(0), Some(1), None, Some(0)]),
            Arc::new(StringArray::from(vec!["x", "yy"])),
        )
        .expect("dizionario"),
    );

    // Calcolo indipendente: la memoria dei buffer che Arrow stessa dichiara
    // per ogni array (capacità di valori, offset, validità e figli), sommata
    // perché nessuna allocazione è condivisa fra queste colonne.
    let colonne = [&con_null, &testo, &lista, &struttura, &dizionario];
    let atteso: u64 = colonne
        .iter()
        .map(|colonna| memoria(colonna.as_ref()))
        .sum();
    let tutte = tabella(vec![
        ("con_null", con_null.clone()),
        ("testo", testo.clone()),
        ("lista", lista.clone()),
        ("struttura", struttura.clone()),
        ("dizionario", dizionario.clone()),
    ]);
    assert_eq!(byte(&[&tutte]), atteso);
    for colonna in colonne {
        let sola = tabella(vec![("c", colonna.clone())]);
        assert_eq!(
            byte(&[&sola]),
            memoria(colonna.as_ref()),
            "{:?}",
            colonna.data_type()
        );
        // Le slice dei tipi annidati tengono vive le stesse allocazioni.
        assert_eq!(byte(&[&sola.slice(1, 2)]), memoria(colonna.as_ref()));
    }
}

#[test]
fn i_valori_condivisi_di_due_dizionari_contano_una_volta() {
    let valori: ArrayRef = Arc::new(StringArray::from(vec!["uno", "due", "tre"]));
    let chiavi_a = Int32Array::from(vec![0, 1, 2, 1]);
    let chiavi_b = Int32Array::from(vec![2, 2, 0]);
    let memoria_chiavi = memoria(&chiavi_a) + memoria(&chiavi_b);
    let a: ArrayRef = Arc::new(
        DictionaryArray::<Int32Type>::try_new(chiavi_a, valori.clone()).expect("dizionario a"),
    );
    let b: ArrayRef = Arc::new(
        DictionaryArray::<Int32Type>::try_new(chiavi_b, valori.clone()).expect("dizionario b"),
    );
    let prima = tabella(vec![("a", a)]);
    let seconda = tabella(vec![("b", b)]);
    assert_eq!(
        byte(&[&prima, &seconda]),
        memoria_chiavi + memoria(valori.as_ref())
    );
}

#[test]
fn nessuna_tabella_nessun_byte() {
    assert_eq!(byte(&[]), 0);
    let vuota = tabella(vec![("x", Arc::new(Int64Array::from(Vec::<i64>::new())))]);
    assert_eq!(byte(&[&vuota]), 0);
}
