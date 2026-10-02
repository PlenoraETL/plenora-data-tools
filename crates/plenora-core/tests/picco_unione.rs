//! Oracolo di `memoria::picco_unione`: per blocchi generati (tipi, lunghezze,
//! fette, null assenti, sparsi o totali, mescolati fra i blocchi) la stima
//! non è mai sotto il picco vero di `concat_batches`, cioè le allocazioni
//! dei blocchi più quelle della tabella unita, contate per allocazione
//! (`byte_vivi`: capacità dei buffer, ogni allocazione una volta).
//!
//! Generatore deterministico (xorshift con seme fisso): stesso seme, stessi
//! casi.

use std::sync::Arc;

use plenora_core::arrow::array::types::Int32Type;
use plenora_core::arrow::array::{
    Array, ArrayRef, BinaryArray, BooleanArray, Date32Array, Decimal128Array, DictionaryArray,
    FixedSizeBinaryArray, Float64Array, Int32Array, Int64Array, Int8Array, LargeStringArray,
    ListArray, NullArray, RecordBatch, RunArray, StringArray, StringViewArray, StructArray,
    TimestampMicrosecondArray, UnionArray,
};
use plenora_core::arrow::schema::{DataType, Field, Fields, Schema, UnionFields};
use plenora_core::arrow::select::concat::concat_batches;
use plenora_core::memoria::{byte_vivi, picco_unione};

struct Generatore(u64);

impl Generatore {
    const fn prossimo(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    fn sotto(&mut self, limite: u64) -> usize {
        usize::try_from(self.prossimo() % limite.max(1)).expect("piccolo")
    }

    fn testo(&mut self, massimo: u64) -> String {
        let lunghezza = self.sotto(massimo + 1);
        (0..lunghezza)
            .map(|_| char::from(b'a' + u8::try_from(self.sotto(26)).expect("lettera")))
            .collect()
    }
}

/// Null del blocco: nessuno, circa un terzo, tutti.
#[derive(Clone, Copy)]
enum Nulli {
    Nessuno,
    Sparsi,
    Tutti,
}

impl Nulli {
    fn nullo(self, generatore: &mut Generatore) -> bool {
        match self {
            Self::Nessuno => false,
            Self::Sparsi => generatore.sotto(3) == 0,
            Self::Tutti => true,
        }
    }
}

const TIPI: usize = 17;

/// Una colonna del tipo `tipo` (indice) di `righe` righe.
#[allow(clippy::too_many_lines)] // Un ramo per tipo, in un punto solo.
fn colonna(tipo: usize, righe: usize, nulli: Nulli, generatore: &mut Generatore) -> ArrayRef {
    let mut presenze = || -> Vec<bool> { (0..righe).map(|_| !nulli.nullo(generatore)).collect() };
    match tipo {
        0 => {
            let presenze = presenze();
            Arc::new(Int8Array::from_iter(presenze.iter().enumerate().map(
                |(i, p)| p.then(|| i8::try_from(i % 100).expect("piccolo")),
            )))
        }
        1 => {
            let presenze = presenze();
            Arc::new(Int64Array::from_iter(
                presenze
                    .iter()
                    .enumerate()
                    .map(|(i, p)| p.then(|| i64::try_from(i).expect("indice"))),
            ))
        }
        2 => {
            let presenze = presenze();
            Arc::new(Float64Array::from_iter(
                presenze.iter().map(|p| p.then_some(1.5)),
            ))
        }
        3 => {
            let presenze = presenze();
            Arc::new(BooleanArray::from_iter(
                presenze
                    .iter()
                    .enumerate()
                    .map(|(i, p)| p.then_some(i % 2 == 0)),
            ))
        }
        4 => {
            let presenze = presenze();
            let valori: Vec<Option<String>> = presenze
                .iter()
                .map(|p| p.then(|| generatore.testo(20)))
                .collect();
            Arc::new(StringArray::from_iter(valori))
        }
        5 => {
            let presenze = presenze();
            let valori: Vec<Option<String>> = presenze
                .iter()
                .map(|p| p.then(|| generatore.testo(20)))
                .collect();
            Arc::new(LargeStringArray::from_iter(valori))
        }
        6 => {
            let presenze = presenze();
            let valori: Vec<Option<Vec<u8>>> = presenze
                .iter()
                .map(|p| p.then(|| generatore.testo(10).into_bytes()))
                .collect();
            Arc::new(BinaryArray::from_iter(valori))
        }
        7 => {
            let presenze = presenze();
            let valori: Vec<Option<String>> = presenze
                .iter()
                .map(|p| p.then(|| generatore.testo(40)))
                .collect();
            Arc::new(StringViewArray::from_iter(valori))
        }
        8 => {
            let presenze = presenze();
            Arc::new(Date32Array::from_iter(
                presenze.iter().map(|p| p.then_some(18_000)),
            ))
        }
        9 => {
            let presenze = presenze();
            Arc::new(
                TimestampMicrosecondArray::from_iter(presenze.iter().map(|p| p.then_some(1)))
                    .with_timezone("Europe/Rome"),
            )
        }
        10 => {
            let presenze = presenze();
            Arc::new(
                Decimal128Array::from_iter(presenze.iter().map(|p| p.then_some(125)))
                    .with_precision_and_scale(10, 2)
                    .expect("decimale"),
            )
        }
        11 => {
            let presenze = presenze();
            Arc::new(
                FixedSizeBinaryArray::try_from_sparse_iter_with_size(
                    presenze.iter().map(|p| p.then(|| vec![1_u8, 2, 3])),
                    3,
                )
                .expect("binario fisso"),
            )
        }
        12 => {
            let presenze = presenze();
            let elementi: Vec<Option<Vec<Option<i32>>>> = presenze
                .iter()
                .map(|p| {
                    p.then(|| {
                        (0..generatore.sotto(6))
                            .map(|j| (j % 3 != 0).then_some(7))
                            .collect()
                    })
                })
                .collect();
            Arc::new(ListArray::from_iter_primitive::<Int32Type, _, _>(elementi))
        }
        13 => {
            let presenze = presenze();
            let numeri = Int32Array::from_iter(presenze.iter().map(|p| p.then_some(3)));
            let testi: Vec<Option<String>> = presenze
                .iter()
                .map(|p| p.then(|| generatore.testo(8)))
                .collect();
            let struttura_nulli = Int8Array::from_iter(presenze.iter().map(|p| p.then_some(0)))
                .nulls()
                .cloned();
            Arc::new(
                StructArray::try_new(
                    Fields::from(vec![
                        Field::new("a", DataType::Int32, true),
                        Field::new("b", DataType::Utf8, true),
                    ]),
                    vec![Arc::new(numeri), Arc::new(StringArray::from_iter(testi))],
                    struttura_nulli,
                )
                .expect("struct"),
            )
        }
        14 => {
            let presenze = presenze();
            let parole = ["alfa", "beta", "gamma", "delta"];
            let valori: Vec<Option<&str>> = presenze
                .iter()
                .map(|p| p.then(|| parole[generatore.sotto(4)]))
                .collect();
            Arc::new(valori.into_iter().collect::<DictionaryArray<Int32Type>>())
        }
        15 => Arc::new(NullArray::new(righe)),
        _ => {
            // Run-end: corse di lunghezza 1..4 fino a `righe`.
            let mut estremi = Vec::new();
            let mut valori = Vec::new();
            let mut fine = 0_i32;
            let totale = i32::try_from(righe).expect("righe");
            while fine < totale {
                fine = (fine + i32::try_from(1 + generatore.sotto(4)).expect("corsa")).min(totale);
                estremi.push(fine);
                valori.push((!nulli.nullo(generatore)).then_some(i64::from(fine)));
            }
            if estremi.is_empty() {
                return Arc::new(
                    RunArray::<Int32Type>::try_new(
                        &Int32Array::from(Vec::<i32>::new()),
                        &Int64Array::from(Vec::<Option<i64>>::new()),
                    )
                    .expect("run-end vuoto"),
                );
            }
            Arc::new(
                RunArray::<Int32Type>::try_new(
                    &Int32Array::from(estremi),
                    &Int64Array::from(valori),
                )
                .expect("run-end"),
            )
        }
    }
}

/// Una union sparsa di due figli, fuori da `colonna` perché i suoi figli
/// hanno la lunghezza della union.
fn union_sparsa(righe: usize, nulli: Nulli, generatore: &mut Generatore) -> ArrayRef {
    let tipi: Vec<i8> = (0..righe)
        .map(|_| i8::try_from(generatore.sotto(2)).expect("tipo"))
        .collect();
    let campi = UnionFields::try_new(
        vec![0, 1],
        vec![
            Field::new("n", DataType::Int64, true),
            Field::new("s", DataType::Utf8, true),
        ],
    )
    .expect("campi");
    let numeri: ArrayRef = colonna(1, righe, nulli, generatore);
    let testi: ArrayRef = colonna(4, righe, nulli, generatore);
    Arc::new(UnionArray::try_new(campi, tipi.into(), None, vec![numeri, testi]).expect("union"))
}

/// Il picco vero: le allocazioni dei blocchi e della tabella unita, ognuna
/// una volta (`byte_vivi`, per allocazione). L'unione delle viste
/// (`Utf8View`) condivide i buffer di dati dei blocchi invece di copiarli:
/// `get_array_memory_size` della tabella unita li conterebbe due volte, ma
/// non sono memoria nuova.
fn picco_vero(blocchi: &[RecordBatch], unita: &RecordBatch) -> u64 {
    byte_vivi(blocchi.iter().chain(std::iter::once(unita))).expect("byte")
}

#[test]
fn la_stima_non_e_mai_sotto_il_picco_vero() {
    for seme in [
        0x9E37_79B9_7F4A_7C15,
        0xD1B5_4A32_D192_ED03,
        0x2545_F491_4F6C_DD1D,
    ] {
        oracolo(seme);
    }
}

fn oracolo(seme: u64) {
    let mut generatore = Generatore(seme);
    let mut provati = 0;
    for caso in 0..1_500 {
        let tipo = caso % (TIPI + 1);
        let numero_blocchi = 2 + generatore.sotto(4);
        let mut blocchi = Vec::new();
        for _ in 0..numero_blocchi {
            let nulli = match generatore.sotto(3) {
                0 => Nulli::Nessuno,
                1 => Nulli::Sparsi,
                _ => Nulli::Tutti,
            };
            // Righe piccole e, a volte, molte (bitmap oltre i 64 byte).
            let righe = if generatore.sotto(4) == 0 {
                generatore.sotto(5_000)
            } else {
                generatore.sotto(40)
            };
            let colonna = if tipo == TIPI {
                union_sparsa(righe, nulli, &mut generatore)
            } else {
                colonna(tipo, righe, nulli, &mut generatore)
            };
            // A volte una fetta: gli offset e i buffer restano quelli interi.
            let colonna = if righe > 2 && generatore.sotto(3) == 0 {
                let inizio = generatore.sotto(u64::try_from(righe / 2).expect("righe"));
                colonna.slice(inizio, righe - inizio - 1)
            } else {
                colonna
            };
            let schema = Arc::new(Schema::new(vec![Field::new(
                "c",
                colonna.data_type().clone(),
                true,
            )]));
            blocchi.push(RecordBatch::try_new(schema, vec![colonna]).expect("blocco"));
        }
        let schema = blocchi[0].schema();
        let Ok(unita) = concat_batches(&schema, &blocchi) else {
            continue;
        };
        provati += 1;
        let vero = picco_vero(&blocchi, &unita);
        let stima = picco_unione(&blocchi);
        assert!(
            stima >= vero,
            "caso {caso}, tipo {:?}: stima {stima} sotto il picco vero {vero}",
            schema.field(0).data_type()
        );
    }
    assert!(provati > 1_400, "casi provati: {provati}");
}

/// Il caso della verifica: due Int8, 100 000 righe senza null e una riga
/// nulla. Arrow materializza la bitmap su tutta la lunghezza.
#[test]
fn la_bitmap_piena_dell_unione_si_conta() {
    let senza: ArrayRef = Arc::new(Int8Array::from(vec![1_i8; 100_000]));
    let con: ArrayRef = Arc::new(Int8Array::from(vec![None::<i8>]));
    let schema = Arc::new(Schema::new(vec![Field::new("c", DataType::Int8, true)]));
    let blocchi = vec![
        RecordBatch::try_new(schema.clone(), vec![senza]).expect("blocco"),
        RecordBatch::try_new(schema.clone(), vec![con]).expect("blocco"),
    ];
    let unita = concat_batches(&schema, &blocchi).expect("unione");
    assert!(unita.column(0).nulls().is_some());
    let vero = picco_vero(&blocchi, &unita);
    assert!(picco_unione(&blocchi) >= vero);
}
