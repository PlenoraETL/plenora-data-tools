//! Il corpo comune dei target `ordinamento` e `ordinamento_parallelo`:
//! `table.sort` contro un oracolo indipendente.
//!
//! Il payload genera una tabella con fino a tre chiavi, ognuna di uno dei
//! tipi ordinabili (`Int64`, `UInt64`, `Float64`, `Utf8`, `Boolean`,
//! `Date32`, `Timestamp`, `Decimal128`, `Binary`, `Dictionary(Int32, Utf8)`),
//! con null, valori ripetuti ed estremi, più la colonna `riga` con l'indice
//! d'ingresso. `ordinamento` resta sotto le 64 righe (migliaia di
//! esecuzioni al secondo); `ordinamento_parallelo` supera le 32 768, la
//! soglia del merge sort parallelo (mezzo secondo a esecuzione).
//!
//! L'oracolo è scritto qui, senza il codice del kernel: confronto sul valore
//! nativo di ogni tipo (`total_cmp` sui `Float64`, byte sui `Binary`, valore
//! sui dictionary, null logici compresi), null dopo ogni valore, il verso
//! discendente che rovescia l'intero confronto, ordinamento stabile di `std`.
//!
//! Invarianti: l'uscita è una permutazione dell'ingresso (stesse colonne,
//! righe spostate intere), la colonna `riga` è la permutazione dell'oracolo,
//! e ordinare l'uscita non la cambia (idempotenza).

use std::cmp::Ordering;
use std::sync::Arc;

use plenora_core::arrow::array::types::Int32Type;
use plenora_core::arrow::array::{
    Array, ArrayRef, BinaryArray, BooleanArray, Date32Array, Decimal128Array, DictionaryArray,
    Float64Array, Int32Array, Int64Array, RecordBatch, StringArray, TimestampMillisecondArray,
    UInt64Array,
};
use plenora_core::arrow::schema::{DataType, Field, Schema};
use plenora_kernels_table::aggregation::{sort, Sort};

/// Righe della modalità piccola.
const MAX_RIGHE: usize = 64;
/// Righe minime della modalità grande: la soglia del merge sort parallelo.
const SOGLIA_PARALLELA: usize = 32_768;
const TIPI: usize = 10;

/// Legge il payload un byte alla volta; finito il payload, ricomincia.
struct Byte<'a> {
    dati: &'a [u8],
    pos: usize,
}

impl Byte<'_> {
    fn prossimo(&mut self) -> u8 {
        if self.dati.is_empty() {
            return 0;
        }
        let valore = self.dati[self.pos % self.dati.len()];
        self.pos += 1;
        valore
    }
}

/// Una cella: `None` per null; il valore dal byte, in pochi valori distinti
/// perché le parità (e quindi la stabilità) contino.
fn cella(byte: &mut Byte<'_>) -> Option<u8> {
    let b = byte.prossimo();
    (b % 7 != 0).then_some(b % 13)
}

const INTERI: [i64; 13] = [
    i64::MIN,
    -9_007_199_254_740_993,
    -2,
    -1,
    0,
    1,
    2,
    3,
    7,
    42,
    9_007_199_254_740_992,
    9_007_199_254_740_993,
    i64::MAX,
];
const NUMERI: [f64; 13] = [
    f64::NEG_INFINITY,
    -1e308,
    -1.0,
    -0.0,
    0.0,
    5e-324,
    0.5,
    1.0,
    1.0 + f64::EPSILON,
    9_007_199_254_740_993.0,
    1e308,
    f64::INFINITY,
    f64::NAN,
];
const TESTI: [&str; 13] = [
    "",
    "A",
    "B",
    "a",
    "a\0",
    "aa",
    "b",
    "é",
    "e\u{301}",
    "z",
    "ß",
    "日",
    "\u{10FFFF}",
];

fn colonna(tipo: usize, valori: &[Option<u8>]) -> (DataType, ArrayRef) {
    let i = |v: &Option<u8>| v.map(|v| usize::from(v));
    match tipo {
        0 => (
            DataType::Int64,
            Arc::new(
                valori
                    .iter()
                    .map(|v| i(v).map(|v| INTERI[v]))
                    .collect::<Int64Array>(),
            ),
        ),
        1 => (
            DataType::UInt64,
            Arc::new(
                valori
                    .iter()
                    .map(|v| {
                        v.map(|v| {
                            if v == 12 {
                                u64::MAX
                            } else {
                                u64::from(v) << 60
                            }
                        })
                    })
                    .collect::<UInt64Array>(),
            ),
        ),
        2 => (
            DataType::Float64,
            Arc::new(
                valori
                    .iter()
                    .map(|v| i(v).map(|v| NUMERI[v]))
                    .collect::<Float64Array>(),
            ),
        ),
        3 => (
            DataType::Utf8,
            Arc::new(
                valori
                    .iter()
                    .map(|v| i(v).map(|v| TESTI[v]))
                    .collect::<StringArray>(),
            ),
        ),
        4 => (
            DataType::Boolean,
            Arc::new(
                valori
                    .iter()
                    .map(|v| v.map(|v| v % 2 == 0))
                    .collect::<BooleanArray>(),
            ),
        ),
        5 => (
            DataType::Date32,
            Arc::new(
                valori
                    .iter()
                    .map(|v| v.map(|v| (i32::from(v) - 6) * 40_000))
                    .collect::<Date32Array>(),
            ),
        ),
        6 => {
            let array = valori
                .iter()
                .map(|v| v.map(|v| (i64::from(v) - 6) * 1_000_000_000_000))
                .collect::<TimestampMillisecondArray>()
                .with_timezone("Europe/Rome");
            (array.data_type().clone(), Arc::new(array))
        }
        7 => {
            let array = valori
                .iter()
                .map(|v| v.map(|v| (i128::from(v) - 6) * 10_i128.pow(30)))
                .collect::<Decimal128Array>()
                .with_precision_and_scale(38, 4)
                .expect("decimale");
            (array.data_type().clone(), Arc::new(array))
        }
        8 => (
            DataType::Binary,
            Arc::new(
                valori
                    .iter()
                    .map(|v| v.map(|v| TESTI[usize::from(v)].as_bytes()))
                    .collect::<BinaryArray>(),
            ),
        ),
        _ => {
            // Chiavi su un dizionario con un valore null e un doppione: il
            // confronto è sul valore, e una chiave verso il null è null.
            let dizionario =
                StringArray::from(vec![Some("m"), None, Some("b"), Some("m"), Some("")]);
            let chiavi: Int32Array = valori.iter().map(|v| v.map(|v| i32::from(v % 5))).collect();
            let array = DictionaryArray::<Int32Type>::try_new(chiavi, Arc::new(dizionario))
                .expect("dictionary");
            (array.data_type().clone(), Arc::new(array))
        }
    }
}

/// Valore nativo della cella per l'oracolo; `None` per un null logico.
/// L'uguaglianza dei `Float64` è per bit: `NaN` è uguale a sé stesso, `-0.0`
/// e `0.0` no.
#[derive(Debug)]
enum Valore<'a> {
    Intero(i128),
    Numero(f64),
    Testo(&'a [u8]),
    Logico(bool),
}

impl PartialEq for Valore<'_> {
    fn eq(&self, altro: &Self) -> bool {
        match (self, altro) {
            (Valore::Numero(a), Valore::Numero(b)) => a.to_bits() == b.to_bits(),
            (Valore::Intero(a), Valore::Intero(b)) => a == b,
            (Valore::Testo(a), Valore::Testo(b)) => a == b,
            (Valore::Logico(a), Valore::Logico(b)) => a == b,
            _ => false,
        }
    }
}

fn valore(array: &ArrayRef, riga: usize) -> Option<Valore<'_>> {
    if array.is_null(riga) {
        return None;
    }
    let a = array.as_any();
    Some(if let Some(v) = a.downcast_ref::<Int64Array>() {
        Valore::Intero(i128::from(v.value(riga)))
    } else if let Some(v) = a.downcast_ref::<UInt64Array>() {
        Valore::Intero(i128::from(v.value(riga)))
    } else if let Some(v) = a.downcast_ref::<Float64Array>() {
        Valore::Numero(v.value(riga))
    } else if let Some(v) = a.downcast_ref::<StringArray>() {
        Valore::Testo(v.value(riga).as_bytes())
    } else if let Some(v) = a.downcast_ref::<BooleanArray>() {
        Valore::Logico(v.value(riga))
    } else if let Some(v) = a.downcast_ref::<Date32Array>() {
        Valore::Intero(i128::from(v.value(riga)))
    } else if let Some(v) = a.downcast_ref::<TimestampMillisecondArray>() {
        Valore::Intero(i128::from(v.value(riga)))
    } else if let Some(v) = a.downcast_ref::<Decimal128Array>() {
        Valore::Intero(v.value(riga))
    } else if let Some(v) = a.downcast_ref::<BinaryArray>() {
        Valore::Testo(v.value(riga))
    } else if let Some(v) = a.downcast_ref::<DictionaryArray<Int32Type>>() {
        let chiave = usize::try_from(v.keys().value(riga)).expect("chiave");
        let dizionario = v
            .values()
            .as_any()
            .downcast_ref::<StringArray>()
            .expect("Utf8");
        if dizionario.is_null(chiave) {
            return None;
        }
        Valore::Testo(dizionario.value(chiave).as_bytes())
    } else {
        unreachable!("tipo non generato")
    })
}

fn confronta_valori(a: &Valore<'_>, b: &Valore<'_>) -> Ordering {
    match (a, b) {
        (Valore::Intero(a), Valore::Intero(b)) => a.cmp(b),
        (Valore::Numero(a), Valore::Numero(b)) => a.total_cmp(b),
        (Valore::Testo(a), Valore::Testo(b)) => a.cmp(b),
        (Valore::Logico(a), Valore::Logico(b)) => a.cmp(b),
        _ => unreachable!("una colonna ha un tipo solo"),
    }
}

/// Il confronto dell'oracolo: null dopo ogni valore, poi rovesciato tutto in
/// discendente.
/// `chiavi[k][r]` e' il valore della chiave `k` alla riga `r`, calcolato una
/// volta sola prima dell'ordinamento.
fn confronta(chiavi: &[Vec<Option<Valore<'_>>>], ascendente: bool, a: usize, b: usize) -> Ordering {
    for colonna in chiavi {
        let ordine = match (&colonna[a], &colonna[b]) {
            (None, None) => Ordering::Equal,
            (None, Some(_)) => Ordering::Greater,
            (Some(_), None) => Ordering::Less,
            (Some(x), Some(y)) => confronta_valori(x, y),
        };
        let ordine = if ascendente { ordine } else { ordine.reverse() };
        if ordine != Ordering::Equal {
            return ordine;
        }
    }
    Ordering::Equal
}

fn colonna_riga(tabella: &RecordBatch) -> Vec<i64> {
    tabella
        .column_by_name("riga")
        .expect("colonna riga")
        .as_any()
        .downcast_ref::<Int64Array>()
        .expect("riga Int64")
        .values()
        .to_vec()
}

pub fn prova(dati: &[u8], grande: bool) {
    let Some((&testa, resto)) = dati.split_first() else {
        return;
    };
    let mut byte = Byte {
        dati: resto,
        pos: 0,
    };
    let ascendente = testa & 1 == 0;
    let numero_chiavi = 1 + usize::from(testa >> 1) % 3;
    let righe = if grande {
        SOGLIA_PARALLELA + usize::from(byte.prossimo()) * 16
    } else {
        usize::from(byte.prossimo()) % (MAX_RIGHE + 1)
    };
    let tipi: Vec<usize> = (0..numero_chiavi)
        .map(|_| usize::from(byte.prossimo()) % TIPI)
        .collect();

    let mut campi = Vec::new();
    let mut colonne: Vec<ArrayRef> = Vec::new();
    for (indice, tipo) in tipi.iter().enumerate() {
        let valori: Vec<Option<u8>> = (0..righe).map(|_| cella(&mut byte)).collect();
        let (tipo, array) = colonna(*tipo, &valori);
        campi.push(Field::new(format!("k{indice}"), tipo, true));
        colonne.push(array);
    }
    let chiavi = colonne.clone();
    campi.push(Field::new("riga", DataType::Int64, false));
    colonne.push(Arc::new(Int64Array::from_iter_values(
        (0..righe).map(|r| i64::try_from(r).expect("riga")),
    )));
    let tabella = RecordBatch::try_new(Arc::new(Schema::new(campi)), colonne).expect("tabella");

    let config = Sort {
        columns: (0..numero_chiavi).map(|k| format!("k{k}")).collect(),
        ascending: ascendente,
    };
    let ordinata = sort(&tabella, &config).expect("sort su tipi ordinabili");

    let mut attesa: Vec<usize> = (0..righe).collect();
    let valori: Vec<Vec<Option<Valore<'_>>>> = chiavi
        .iter()
        .map(|colonna| (0..righe).map(|r| valore(colonna, r)).collect())
        .collect();
    attesa.sort_by(|a, b| confronta(&valori, ascendente, *a, *b));
    let attesa: Vec<i64> = attesa
        .into_iter()
        .map(|r| i64::try_from(r).expect("riga"))
        .collect();
    let permutazione = colonna_riga(&ordinata);
    assert_eq!(permutazione, attesa, "permutazione diversa dall'oracolo");

    // Righe spostate intere: ogni colonna è quella d'ingresso permutata.
    assert_eq!(ordinata.schema(), tabella.schema());
    for (k, colonna) in ordinata.columns().iter().enumerate() {
        for (posizione, origine) in permutazione.iter().enumerate() {
            let origine = usize::try_from(*origine).expect("indice");
            assert_eq!(
                valore(colonna, posizione),
                valore(tabella.column(k), origine),
                "colonna {k}"
            );
        }
    }

    let di_nuovo = sort(&ordinata, &config).expect("sort dell'uscita");
    assert_eq!(di_nuovo, ordinata, "sort non idempotente");
}
