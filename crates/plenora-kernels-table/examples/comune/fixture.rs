//! Le fixture di benchmark che piu' file costruivano allo stesso modo.
//!
//! Spostate **verbatim**: stesso ordine di estrazioni dal generatore, stesso
//! schema, stessi null. Una fixture riscritta invece che spostata misurerebbe
//! un carico diverso da quello su cui la baseline e' stata raccolta, e non
//! ci sarebbe niente a dirlo.
//!
//! Cio' che resta nei singoli file: le fixture che esistono in una copia
//! sola, le quattro varianti di `base_fixture` che non coincidono, e
//! `list_fixture`, che dipende da una costante dichiarata nel suo file.

use plenora_core::arrow::array::{
    ArrayRef, Float64Array, Int64Array, RecordBatch, StringArray, StructArray,
};
use plenora_core::arrow::schema::{DataType, Field, Fields, Schema};
use std::sync::Arc;

use super::rng::Rng;

pub fn base_fixture(rows: usize) -> RecordBatch {
    let mut rng = Rng::seeded();
    let mut ids = Vec::with_capacity(rows);
    let mut nums = Vec::with_capacity(rows);
    let mut groups = Vec::with_capacity(rows);
    let mut texts = Vec::with_capacity(rows);
    let mut keys = Vec::with_capacity(rows);
    let mut paths = Vec::with_capacity(rows);
    for row in 0..rows {
        ids.push(i64::try_from(row).ok());
        // Bound evidente: draw % 1_000_000 <= 999_999 < 2^53, cast esatto in f64.
        #[allow(clippy::cast_precision_loss)]
        nums.push(Some((rng.next() % 1_000_000) as f64 / 100.0));
        groups.push(format!("g{}", rng.next() % 1_024));
        texts.push(format!(
            "{:016x}{:016x}{:08x}",
            rng.next(),
            rng.next(),
            rng.next() & 0xffff_ffff
        ));
        // Bound evidente: draw % 1_000_000 <= 999_999, entra in i64 senza wrap.
        #[allow(clippy::cast_possible_wrap)]
        keys.push((rng.next() % 1_000_000) as i64);
        paths.push(format!(
            "p{:03}/q{:03}/r{:03}",
            rng.next() % 500,
            rng.next() % 500,
            rng.next() % 500
        ));
    }
    let schema = Schema::new_with_metadata(
        vec![
            Field::new("id", DataType::Int64, false),
            Field::new("num", DataType::Float64, false),
            Field::new("grp", DataType::Utf8, false),
            Field::new("text", DataType::Utf8, false),
            Field::new("key", DataType::Int64, false),
            Field::new("path", DataType::Utf8, false),
        ],
        std::iter::once(("source".to_owned(), "bench_sweep".to_owned())).collect(),
    );
    RecordBatch::try_new(
        Arc::new(schema),
        vec![
            Arc::new(Int64Array::from(ids)),
            Arc::new(Float64Array::from(nums)),
            Arc::new(StringArray::from(groups)),
            Arc::new(StringArray::from(texts)),
            Arc::new(Int64Array::from(keys)),
            Arc::new(StringArray::from(paths)),
        ],
    )
    .expect("fixture base")
}

pub fn setop_right_fixture(rows: usize) -> RecordBatch {
    const DRAWS_PER_ROW: usize = 9;
    let mut base_rng = Rng::seeded();
    let stream = (0..rows * DRAWS_PER_ROW)
        .map(|_| base_rng.next())
        .collect::<Vec<_>>();
    let mut rng = Rng::con_seme(43); // meta' non sovrapposta: seme diverso
    let mut ids = Vec::with_capacity(rows);
    let mut nums = Vec::with_capacity(rows);
    let mut groups = Vec::with_capacity(rows);
    let mut texts = Vec::with_capacity(rows);
    let mut keys = Vec::with_capacity(rows);
    let mut paths = Vec::with_capacity(rows);
    for row in 0..rows {
        let draws: [u64; DRAWS_PER_ROW] = if row >= rows / 2 {
            let base = row * DRAWS_PER_ROW;
            stream[base..base + DRAWS_PER_ROW]
                .try_into()
                .expect("draws")
        } else {
            [(); DRAWS_PER_ROW].map(|()| rng.next())
        };
        ids.push(i64::try_from(row).ok());
        // Bound evidente: draws[0] % 1_000_000 <= 999_999 < 2^53, cast esatto in f64.
        #[allow(clippy::cast_precision_loss)]
        nums.push(Some((draws[0] % 1_000_000) as f64 / 100.0));
        groups.push(format!("g{}", draws[1] % 1_024));
        texts.push(format!(
            "{:016x}{:016x}{:08x}",
            draws[2],
            draws[3],
            draws[4] & 0xffff_ffff
        ));
        // Bound evidente: draws[5] % 1_000_000 <= 999_999, entra in i64 senza wrap.
        #[allow(clippy::cast_possible_wrap)]
        keys.push((draws[5] % 1_000_000) as i64);
        paths.push(format!(
            "p{:03}/q{:03}/r{:03}",
            draws[6] % 500,
            draws[7] % 500,
            draws[8] % 500
        ));
    }
    let schema = base_fixture(1).schema();
    RecordBatch::try_new(
        schema,
        vec![
            Arc::new(Int64Array::from(ids)),
            Arc::new(Float64Array::from(nums)),
            Arc::new(StringArray::from(groups)),
            Arc::new(StringArray::from(texts)),
            Arc::new(Int64Array::from(keys)),
            Arc::new(StringArray::from(paths)),
        ],
    )
    .expect("fixture setop destra")
}

pub fn right_fixture(rows: usize) -> RecordBatch {
    let mut rng = Rng::seeded();
    let mut ids = Vec::with_capacity(rows);
    let mut nums = Vec::with_capacity(rows);
    let mut rvals = Vec::with_capacity(rows);
    for row in 0..rows {
        ids.push(i64::try_from(row).ok());
        // Bound evidente: draw % 1_000_000 <= 999_999 < 2^53, cast esatto in f64.
        #[allow(clippy::cast_precision_loss)]
        let base = (rng.next() % 1_000_000) as f64 / 100.0;
        nums.push(Some(if row % 10 == 0 { base + 1.0 } else { base }));
        rvals.push(format!("r{:016x}", rng.next()));
    }
    RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int64, false),
            Field::new("num", DataType::Float64, false),
            Field::new("rval", DataType::Utf8, false),
        ])),
        vec![
            Arc::new(Int64Array::from(ids)),
            Arc::new(Float64Array::from(nums)),
            Arc::new(StringArray::from(rvals)),
        ],
    )
    .expect("fixture destra")
}

pub fn struct_fixture(rows: usize) -> RecordBatch {
    let mut rng = Rng::seeded();
    let ids = (0..rows)
        .map(|row| i64::try_from(row).ok())
        .collect::<Vec<_>>();
    // Reinterpretazione bit a bit intenzionale: colonna random a pieno range.
    let a = (0..rows)
        .map(|_| Some(rng.next().cast_signed()))
        .collect::<Vec<_>>();
    // Bound evidente: draw % 10_000 <= 9_999 < 2^53, cast esatto in f64.
    #[allow(clippy::cast_precision_loss)]
    let b = (0..rows)
        .map(|_| Some((rng.next() % 10_000) as f64))
        .collect::<Vec<_>>();
    let c = (0..rows)
        .map(|_| format!("{:016x}", rng.next()))
        .collect::<Vec<_>>();
    let fields = Fields::from(vec![
        Field::new("a", DataType::Int64, true),
        Field::new("b", DataType::Float64, true),
        Field::new("c", DataType::Utf8, true),
    ]);
    let structure = StructArray::new(
        fields.clone(),
        vec![
            Arc::new(Int64Array::from(a)) as ArrayRef,
            Arc::new(Float64Array::from(b)) as ArrayRef,
            Arc::new(StringArray::from(c)) as ArrayRef,
        ],
        None,
    );
    RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int64, false),
            Field::new("payload", DataType::Struct(fields), false),
        ])),
        vec![Arc::new(Int64Array::from(ids)), Arc::new(structure)],
    )
    .expect("fixture struct")
}

pub fn json_fixture(rows: usize) -> RecordBatch {
    let mut rng = Rng::seeded();
    let ids = (0..rows)
        .map(|row| i64::try_from(row).ok())
        .collect::<Vec<_>>();
    let docs = (0..rows)
        .map(|_| {
            format!(
                "{{\"a\":{},\"b\":{{\"c\":{},\"d\":{{\"e\":\"{:08x}\",\"f\":[1,2,3]}}}},\"g\":\"{:08x}\"}}",
                rng.next() % 1000,
                rng.next() % 1000,
                rng.next() & 0xffff_ffff,
                rng.next() & 0xffff_ffff
            )
        })
        .collect::<Vec<_>>();
    RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int64, false),
            Field::new("doc", DataType::Utf8, false),
        ])),
        vec![
            Arc::new(Int64Array::from(ids)),
            Arc::new(StringArray::from(docs)),
        ],
    )
    .expect("fixture json")
}
