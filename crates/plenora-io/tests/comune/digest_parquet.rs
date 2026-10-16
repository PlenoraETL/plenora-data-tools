//! Digest canonico condiviso dalle prove Parquet.
#![allow(dead_code, clippy::unwrap_used, clippy::panic)]
use plenora_core::arrow::array::cast::AsArray;
use plenora_core::arrow::array::{Array, RecordBatch};
use plenora_core::arrow::schema::{DataType, Schema};
use sha2::{Digest, Sha256};
use std::fmt::Write as _;

pub fn esadecimale(byte: &[u8]) -> String {
    byte.iter().fold(String::new(), |mut testo, b| {
        let _ = write!(testo, "{b:02x}");
        testo
    })
}

// --- Il digest canonico ------------------------------------------------------

/// Un pezzo del digest con la sua lunghezza: nessuna concatenazione è
/// ambigua.
fn pezzo(h: &mut Sha256, byte: &[u8]) {
    h.update((byte.len() as u64).to_le_bytes());
    h.update(byte);
}

/// Il valore `i` di `array` nel digest: un segno per il nullo, poi una forma
/// che dipende solo dal valore, non da come i buffer lo tengono (offset,
/// slice, viste, dizionari). Un tipo senza forma canonica ferma la prova.
fn valore(h: &mut Sha256, array: &dyn Array, i: usize) {
    if array.logical_nulls().is_some_and(|nulli| nulli.is_null(i)) {
        h.update([0]);
        return;
    }
    h.update([1]);
    match array.data_type() {
        DataType::Null => {}
        DataType::Boolean => h.update([u8::from(array.as_boolean().value(i))]),
        DataType::Utf8 => pezzo(h, array.as_string::<i32>().value(i).as_bytes()),
        DataType::LargeUtf8 => pezzo(h, array.as_string::<i64>().value(i).as_bytes()),
        DataType::Utf8View => pezzo(h, array.as_string_view().value(i).as_bytes()),
        DataType::Binary => pezzo(h, array.as_binary::<i32>().value(i)),
        DataType::LargeBinary => pezzo(h, array.as_binary::<i64>().value(i)),
        DataType::BinaryView => pezzo(h, array.as_binary_view().value(i)),
        DataType::FixedSizeBinary(_) => pezzo(h, array.as_fixed_size_binary().value(i)),
        DataType::List(_) => elementi(h, array.as_list::<i32>().value(i).as_ref()),
        DataType::LargeList(_) => elementi(h, array.as_list::<i64>().value(i).as_ref()),
        DataType::FixedSizeList(_, _) => {
            elementi(h, array.as_fixed_size_list().value(i).as_ref());
        }
        DataType::Map(_, _) => elementi(h, &array.as_map().value(i)),
        DataType::Struct(_) => {
            for figlio in array.as_struct().columns() {
                valore(h, figlio.as_ref(), i);
            }
        }
        DataType::Dictionary(_, _) => {
            let dizionario = array.as_any_dictionary();
            let chiave = dizionario.normalized_keys()[i];
            valore(h, dizionario.values().as_ref(), chiave);
        }
        tipo => match tipo.primitive_width() {
            // Interi, decimali, virgola mobile, date, ore, istanti, durate,
            // intervalli: i byte little-endian del valore.
            Some(larghezza) => {
                let dati = array.to_data();
                let inizio = (dati.offset() + i) * larghezza;
                pezzo(h, &dati.buffers()[0].as_slice()[inizio..inizio + larghezza]);
            }
            None => panic!("tipo senza forma canonica nel digest: {tipo}"),
        },
    }
}

fn elementi(h: &mut Sha256, figli: &dyn Array) {
    h.update((figli.len() as u64).to_le_bytes());
    for j in 0..figli.len() {
        valore(h, figli, j);
    }
}

/// Il digest canonico di una tabella letta: lo schema (nome, tipo, nullabilità
/// di ogni campo, senza metadati) e i valori, riga per riga, colonna per
/// colonna, nell'ordine letto; 32 cifre esadecimali (128 bit).
pub fn digest(schema: &Schema, blocchi: &[RecordBatch]) -> String {
    let mut h = Sha256::new();
    for campo in schema.fields() {
        pezzo(&mut h, campo.name().as_bytes());
        pezzo(&mut h, campo.data_type().to_string().as_bytes());
        h.update([u8::from(campo.is_nullable())]);
    }
    for blocco in blocchi {
        for riga in 0..blocco.num_rows() {
            for colonna in blocco.columns() {
                valore(&mut h, colonna.as_ref(), riga);
            }
        }
    }
    esadecimale(&h.finalize()[..16])
}

/// Per un file non riproducibile si omette solo lo sha256 del file.
pub fn senza_byte(resto: &str) -> String {
    resto
        .split_once('\t')
        .expect("riga con sha256")
        .1
        .to_owned()
}

/// Solo il file `DuckDB` con UUID casuali esclude la colonna `u` dal digest.
/// Le altre colonne, con schema e valori, restano confrontate.
pub fn digest_file(
    schema: &Schema,
    blocchi: &[RecordBatch],
    insieme: &str,
    relativo: &str,
) -> String {
    if insieme != "scrittori" || relativo != "scritti-flba/duckdb/duckdb-uuid-dec.parquet" {
        return digest(schema, blocchi);
    }
    let uuid = schema.index_of("u").expect("colonna UUID richiesta");
    assert_eq!(
        schema.field(uuid).data_type(),
        &DataType::FixedSizeBinary(16)
    );
    let indici: Vec<_> = (0..schema.fields().len()).filter(|i| *i != uuid).collect();
    let schema = schema.project(&indici).unwrap();
    let blocchi: Vec<_> = blocchi
        .iter()
        .map(|b| b.project(&indici).unwrap())
        .collect();
    digest(&schema, &blocchi)
}
