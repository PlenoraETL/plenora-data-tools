//! Sweep prestazionale dei kernel tabellari NON ancora ottimizzati
//! (filone ottimizzazioni kernel, Fase post-2A): 42 op `table.*` del
//! catalogo escluse quelle gia' ottimizzate (filter, sort, `fill_na`,
//! coalesce, `type_cast`, aggregate, `date_add`, `date_diff`, `date_format`,
//! `timezone_convert`, `date_extract`, `text_normalize`, join, `semi_join`,
//! `anti_join`, `string_extract`, formula, expression, melt, pivot).
//!
//! Stile di `bench_filter_sort.rs`: fixture deterministica (seed logico 42
//! via xorshift), mediana di 3 run, righe/s, peak RSS (`VmHWM`).
//!
//! Scala: 1M righe di default; le op con mediana < 1s a 1M vengono
//! rimisurate automaticamente a 10M (`escalate`). Op a scala fissa:
//! `transpose` (limitata a `max_columns` righe per contratto, quadratica),
//! `cross_join` (1k x 1k = 1M righe output), `distinct` (10M dichiarate).
//!
//! Uso: `bench_sweep` — scrive `benchmarks/sweep/sweep.json` e
//! `benchmarks/sweep/sweep.md` (relativi alla cwd, /work in Docker) e
//! stampa le stesse righe JSON su stdout.

use std::hint::black_box;
use std::sync::{Arc, OnceLock};
use std::time::Instant;

use plenora_core::arrow::array::{
    types::Int64Type, Array, ArrayRef, Float64Array, Int64Array, ListArray, RecordBatch,
    StringArray, StructArray,
};
use plenora_core::arrow::schema::{DataType, Field, Fields, Schema};
use plenora_kernels_table::aggregation::{
    dedup_advanced, distinct, rolling_window, window_function, DedupAdvanced, Distinct, Keep,
    RollingKind, RollingWindow, WindowFunction, WindowKind,
};
use plenora_kernels_table::analysis::{
    bin, flatten_json, lookup, sample, statistics, Bin, Bins, FlattenJson, Lookup, Sample, Stat,
    Statistics,
};
use plenora_kernels_table::cleansing::{replace, Replace};
use plenora_kernels_table::columns::{
    concat_columns, drop_columns, rename, reorder_columns, split_column, ConcatColumns, DropColumns,
    Rename, RenamePair, ReorderColumns, SplitColumn,
};
use plenora_kernels_table::filtering::{conditional, Condition, Conditional, Operator};
use plenora_kernels_table::governance::{
    assert_cardinality, assert_foreign_key, assert_metadata, reconcile, AssertCardinality,
    AssertMetadata, ForeignKey, Reconcile,
};
use plenora_kernels_table::joins::{asof_join, concat, cross_join, AsOfJoin, Concat, CrossJoin};
use plenora_kernels_table::quality::{
    assert_not_null, assert_range, assert_regex, assert_schema, assert_unique, AssertNotNull,
    AssertRange, AssertRegex, AssertSchema, AssertUnique, SchemaExpectation,
};
use plenora_kernels_table::reshape::{
    explode, table_diff, transpose, unnest, Explode, HeterogeneousTypePolicy, TableDiff, Transpose,
    Unnest,
};
use plenora_kernels_table::security::{
    md5_hash, mask_data, sha256_hash, HashNullPolicy, MaskData, MaskType, Masking, Md5Hash,
    Sha256Hash,
};
use plenora_kernels_table::setops::{except, intersect, union_distinct, SetOperation};
use plenora_kernels_table::strings::{string_length, string_pad, PadSide, StringLength, StringPad};
use plenora_kernels_table::utility::{add_row_number, uuid_generator, AddRowNumber, UuidGenerator};
use plenora_kernels_table::Limits;
use serde_json::{json, Value};

const M1: usize = 1_000_000;
const M10: usize = 10_000_000;
/// Soglia di escalation: sotto 1s a 1M righe l'op viene rimisurata a 10M.
const ESCALATION_SECONDS: f64 = 1.0;

/// Limiti allargati per le scale di benchmark (10M righe, container 10g).
fn bench_limits() -> Limits {
    Limits {
        max_rows: 40_000_000,
        max_memory_bytes: 6 * 1024 * 1024 * 1024,
        ..Limits::default()
    }
}

/// RNG deterministico (xorshift64*, stesso schema dello shuffle di `sample`).
struct Rng(u64);

impl Rng {
    const fn seeded() -> Self {
        Self(42)
    }

    const fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
}

/// Fixture base condivisa: `id` int64, `num` float64, `grp` utf8 (1024
/// gruppi), `text` utf8 (40 char esadecimali), `key` int64 (1M valori
/// distinti possibili), `path` utf8 ("pNNN/qNNN/rNNN" per `split_column`).
fn base_fixture(rows: usize) -> RecordBatch {
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
        [("source".to_owned(), "bench_sweep".to_owned())].into_iter().collect(),
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

/// Fixture destra per set operation con overlap 50% sulle righe intere:
/// righe identiche alla base nell'intervallo [rows/2, rows). Lo stream
/// xorshift della fixture base (9 draw per riga) e' precalcolato in O(n).
fn setop_right_fixture(rows: usize) -> RecordBatch {
    const DRAWS_PER_ROW: usize = 9;
    let mut base_rng = Rng::seeded();
    let stream = (0..rows * DRAWS_PER_ROW)
        .map(|_| base_rng.next())
        .collect::<Vec<_>>();
    let mut rng = Rng(43); // meta' non sovrapposta: seme diverso
    let mut ids = Vec::with_capacity(rows);
    let mut nums = Vec::with_capacity(rows);
    let mut groups = Vec::with_capacity(rows);
    let mut texts = Vec::with_capacity(rows);
    let mut keys = Vec::with_capacity(rows);
    let mut paths = Vec::with_capacity(rows);
    for row in 0..rows {
        let draws: [u64; DRAWS_PER_ROW] = if row >= rows / 2 {
            let base = row * DRAWS_PER_ROW;
            stream[base..base + DRAWS_PER_ROW].try_into().expect("draws")
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

/// Fixture destra per join/diff/FK: stessa chiave `id` 0..rows, `num`
/// perturbato sul 10% delle righe (per `table_diff`), colonna extra `rval`.
fn right_fixture(rows: usize) -> RecordBatch {
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

/// Fixture asof: timestamp int64 fitti; la destra e' sfasata di +1.
fn asof_fixture(rows: usize, offset: i64) -> RecordBatch {
    let ids = (0..rows)
        .map(|row| Some(2 * i64::try_from(row).unwrap_or(0) + offset))
        .collect::<Vec<_>>();
    // Bound evidente: row < rows e rows <= M10 = 10^7 (scale del bench,
    // costanti in testa al file) << 2^53: cast esatto in f64.
    #[allow(clippy::cast_precision_loss)]
    let vals = (0..rows)
        .map(|row| Some(row as f64))
        .collect::<Vec<_>>();
    RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("ts", DataType::Int64, false),
            Field::new("val", DataType::Float64, false),
        ])),
        vec![
            Arc::new(Int64Array::from(ids)),
            Arc::new(Float64Array::from(vals)),
        ],
    )
    .expect("fixture asof")
}

/// Fixture con colonna List<Int64> (liste corte, 0..5 elementi).
fn list_fixture(rows: usize) -> RecordBatch {
    let mut rng = Rng::seeded();
    let ids = (0..rows).map(|row| i64::try_from(row).ok()).collect::<Vec<_>>();
    // Bound evidente: row < rows <= M10 = 10^7 (scale del bench) e
    // value < length <= 4 (length = rng.next() % 5): entrambi i cast
    // entrano in i64 senza wrap.
    #[allow(clippy::cast_possible_wrap)]
    let lists = (0..rows)
        .map(|row| {
            let length = rng.next() % 5;
            Some(
                (0..length)
                    .map(|value| Some((row as i64) * 10 + value as i64))
                    .collect::<Vec<_>>(),
            )
        })
        .collect::<Vec<_>>();
    let list_array = ListArray::from_iter_primitive::<Int64Type, _, _>(lists);
    RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int64, false),
            Field::new("items", list_array.data_type().clone(), true),
        ])),
        vec![Arc::new(Int64Array::from(ids)), Arc::new(list_array)],
    )
    .expect("fixture list")
}

/// Fixture con colonna Struct{a int64, b float64, c utf8}.
fn struct_fixture(rows: usize) -> RecordBatch {
    let mut rng = Rng::seeded();
    let ids = (0..rows).map(|row| i64::try_from(row).ok()).collect::<Vec<_>>();
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

/// Fixture JSON annidati (3 livelli) per `flatten_json`.
fn json_fixture(rows: usize) -> RecordBatch {
    let mut rng = Rng::seeded();
    let ids = (0..rows).map(|row| i64::try_from(row).ok()).collect::<Vec<_>>();
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

/// Fixture transpose: 8 colonne Float64 x 4000 righe (il contratto limita
/// l'output a `max_columns` colonne = righe input + 1).
fn transpose_fixture(rows: usize) -> RecordBatch {
    let mut rng = Rng::seeded();
    // Bound evidente: draw % 1_000_000 <= 999_999 < 2^53, cast esatto in f64.
    #[allow(clippy::cast_precision_loss)]
    let columns = (0..8)
        .map(|_| {
            Arc::new(Float64Array::from(
                (0..rows)
                    .map(|_| Some((rng.next() % 1_000_000) as f64 / 100.0))
                    .collect::<Vec<_>>(),
            )) as ArrayRef
        })
        .collect::<Vec<_>>();
    let fields = (0..8)
        .map(|index| Field::new(format!("m{index}"), DataType::Float64, false))
        .collect::<Vec<_>>();
    RecordBatch::try_new(Arc::new(Schema::new(fields)), columns).expect("fixture transpose")
}

fn peak_rss_kib() -> Option<u64> {
    std::fs::read_to_string("/proc/self/status")
        .ok()?
        .lines()
        .find_map(|line| {
            line.strip_prefix("VmHWM:")?
                .split_whitespace()
                .next()?
                .parse()
                .ok()
        })
}

struct Measurement {
    op: &'static str,
    rows: usize,
    repetitions: usize,
    median_seconds: f64,
    rows_per_second: f64,
    output_rows: usize,
    peak_rss_kib: Option<u64>,
    note: String,
}

fn measure(
    op: &'static str,
    rows: usize,
    repetitions: usize,
    note: &str,
    execute: impl Fn() -> RecordBatch,
) -> Measurement {
    black_box(execute());
    let mut durations = Vec::with_capacity(repetitions);
    let mut output_rows = 0;
    for _ in 0..repetitions {
        let start = Instant::now();
        let output = execute();
        durations.push(start.elapsed().as_secs_f64());
        output_rows = output.num_rows();
        black_box(output);
    }
    durations.sort_by(f64::total_cmp);
    let median = durations[durations.len() / 2];
    #[allow(clippy::cast_precision_loss)]
    let rows_per_second = rows as f64 / median;
    let record = json!({
        "scenario": op,
        "rows": rows,
        "repetitions": repetitions,
        "median_seconds": median,
        "rows_per_second": rows_per_second,
        "output_rows": output_rows,
        "peak_rss_kib": peak_rss_kib(),
        "note": note,
    });
    println!("{}", serde_json::to_string(&record).expect("JSON"));
    Measurement {
        op,
        rows,
        repetitions,
        median_seconds: median,
        rows_per_second,
        output_rows,
        peak_rss_kib: peak_rss_kib(),
        note: note.to_owned(),
    }
}

static BASE_1M: OnceLock<RecordBatch> = OnceLock::new();
static BASE_10M: OnceLock<RecordBatch> = OnceLock::new();
static SETOP_RIGHT_1M: OnceLock<RecordBatch> = OnceLock::new();
static RIGHT_1M: OnceLock<RecordBatch> = OnceLock::new();

fn base_1m() -> &'static RecordBatch {
    BASE_1M.get_or_init(|| base_fixture(M1))
}

fn base_10m() -> &'static RecordBatch {
    BASE_10M.get_or_init(|| base_fixture(M10))
}

fn setop_right_1m() -> &'static RecordBatch {
    SETOP_RIGHT_1M.get_or_init(|| setop_right_fixture(M1))
}

fn right_1m() -> &'static RecordBatch {
    RIGHT_1M.get_or_init(|| right_fixture(M1))
}

/// Esegue uno scenario unario sulla fixture base a 1M; se `escalate` e la
/// mediana e' sotto soglia, rimisura a 10M e restituisce entrambe le misure.
fn sweep_unary(
    results: &mut Vec<Measurement>,
    op: &'static str,
    escalate: bool,
    note: &str,
    execute: impl Fn(&RecordBatch) -> RecordBatch + Copy,
) {
    let measured = measure(op, M1, 3, note, || execute(base_1m()));
    let fast = measured.median_seconds < ESCALATION_SECONDS;
    results.push(measured);
    if escalate && fast {
        let note10 = format!("{note} [10M]");
        let scaled = measure(op, M10, 3, &note10, || execute(base_10m()));
        results.push(scaled);
    }
}

fn write_outputs(results: &[Measurement]) {
    let directory = std::path::Path::new("benchmarks/sweep");
    std::fs::create_dir_all(directory).expect("mkdir benchmarks/sweep");

    let json_records: Vec<Value> = results
        .iter()
        .map(|entry| {
            json!({
                "op": entry.op,
                "rows": entry.rows,
                "repetitions": entry.repetitions,
                "median_seconds": entry.median_seconds,
                "rows_per_second": entry.rows_per_second,
                "output_rows": entry.output_rows,
                "peak_rss_kib": entry.peak_rss_kib,
                "note": entry.note,
            })
        })
        .collect();
    let document = json!({
        "benchmark": "bench_sweep",
        "seed": 42,
        "scale_default_rows": M1,
        "scale_escalated_rows": M10,
        "escalation_threshold_seconds": ESCALATION_SECONDS,
        "results": json_records,
    });
    std::fs::write(
        directory.join("sweep.json"),
        serde_json::to_string_pretty(&document).expect("JSON"),
    )
    .expect("write sweep.json");

    let mut sorted: Vec<&Measurement> = results.iter().collect();
    sorted.sort_by(|left, right| left.rows_per_second.total_cmp(&right.rows_per_second));
    let mut markdown = String::new();
    markdown.push_str(
        "# Sweep kernel tabellari non ottimizzati (bench_sweep)\n\n\
         Fixture deterministica (seed 42), mediana di 3 run, container Docker\n\
         `--cpus=4 --memory=10g`, release. Scala 1M righe; le op con mediana\n\
         < 1s a 1M sono rimisurate a 10M (righe `[10M]`). Classifica ordinata\n\
         per lentezza (righe/s crescenti). Il peak RSS e' il `VmHWM` di\n\
         processo (cumulativo, cresce con le fixture condivise a 10M).\n\n\
         | # | op | righe | mediana (s) | righe/s | righe output | peak RSS (MiB) | note |\n\
         |---|----|-------|-------------|---------|--------------|----------------|------|\n",
    );
    for (position, entry) in sorted.iter().enumerate() {
        markdown.push_str(&format!(
            "| {} | `{}` | {} | {:.4} | {:.0} | {} | {} | {} |\n",
            position + 1,
            entry.op,
            entry.rows,
            entry.median_seconds,
            entry.rows_per_second,
            entry.output_rows,
            entry
                .peak_rss_kib.map_or_else(|| "n/d".into(), |kib| format!("{}", kib / 1024)),
            entry.note,
        ));
    }
    std::fs::write(directory.join("sweep.md"), markdown).expect("write sweep.md");
}

#[allow(clippy::too_many_lines)]
fn main() {
    let limits = bench_limits();
    let mut results: Vec<Measurement> = Vec::new();

    // --- Analisi -------------------------------------------------------------
    let bin_config = Bin {
        column: "num".into(),
        bins: Bins::Count(20),
        labels: None,
        output_column: Some("num_bin".into()),
    };
    sweep_unary(&mut results, "table.bin", true, "20 bucket equal-width", |batch| {
        bin(batch, &bin_config).expect("bin")
    });

    let flatten_config = FlattenJson {
        column: "doc".into(),
        prefix: String::new(),
        max_level: 3,
        output_columns: Vec::new(),
    };
    let json_input = json_fixture(M1);
    results.push(measure("table.flatten_json", M1, 3, "JSON annidati 3 livelli", || {
        flatten_json(&json_input, &flatten_config, &limits).expect("flatten_json")
    }));

    let lookup_mapping = (0..1_024)
        .map(|index| (format!("g{index}"), Value::from(format!("c{index}"))))
        .collect();
    let lookup_config = Lookup {
        column: "grp".into(),
        mapping: lookup_mapping,
        default: Value::Null,
        output_column: Some("grp_code".into()),
    };
    sweep_unary(&mut results, "table.lookup", true, "mappa 1024 chiavi utf8", |batch| {
        lookup(batch, &lookup_config).expect("lookup")
    });

    let statistics_config = Statistics {
        column: "num".into(),
        group_by: Some("grp".into()),
        stats: vec![
            Stat::Count,
            Stat::Min,
            Stat::Max,
            Stat::Sum,
            Stat::Mean,
            Stat::Median,
            Stat::Std,
            Stat::Var,
            Stat::Q25,
            Stat::Q75,
        ],
        output_prefix: String::new(),
    };
    sweep_unary(&mut results, "table.statistics", true, "10 statistiche x 1024 gruppi", |batch| {
        statistics(batch, &statistics_config).expect("statistics")
    });

    let sample_config = Sample {
        n: 100,
        fraction: Some(0.1),
        random_state: Some(42),
        stratify_column: None,
    };
    sweep_unary(&mut results, "table.sample", true, "fraction 0.1", |batch| {
        sample(batch, &sample_config).expect("sample")
    });

    // --- Colonne --------------------------------------------------------------
    let drop_config = DropColumns {
        columns: vec!["path".into(), "key".into()],
    };
    sweep_unary(&mut results, "table.drop_columns", true, "drop 2 colonne su 6", |batch| {
        drop_columns(batch, &drop_config).expect("drop_columns")
    });

    let rename_config = Rename {
        renames: vec![
            RenamePair { old_name: "num".into(), new_name: "amount".into() },
            RenamePair { old_name: "grp".into(), new_name: "segment".into() },
        ],
    };
    sweep_unary(&mut results, "table.rename", true, "2 rinomini", |batch| {
        rename(batch, &rename_config).expect("rename")
    });

    let reorder_config = ReorderColumns {
        columns: vec![
            "path".into(),
            "key".into(),
            "text".into(),
            "grp".into(),
            "num".into(),
            "id".into(),
        ],
        alphabetical: false,
    };
    sweep_unary(&mut results, "table.reorder_columns", true, "ordine inverso", |batch| {
        reorder_columns(batch, &reorder_config).expect("reorder_columns")
    });

    let concat_columns_config = ConcatColumns {
        columns: vec!["grp".into(), "text".into()],
        output_column: "combined".into(),
        separator: "-".into(),
        skip_null: true,
    };
    sweep_unary(&mut results, "table.concat_columns", true, "2 colonne utf8", |batch| {
        concat_columns(batch, &concat_columns_config, &limits).expect("concat_columns")
    });

    let split_config = SplitColumn {
        column: "path".into(),
        delimiter: "/".into(),
        new_columns: vec!["p1".into(), "p2".into(), "p3".into()],
        max_splits: -1,
    };
    sweep_unary(&mut results, "table.split_column", true, "3 colonne su '/'", |batch| {
        split_column(batch, &split_config, &limits).expect("split_column")
    });

    // --- Cleansing -------------------------------------------------------------
    let replace_config = Replace {
        column: "grp".into(),
        old_value: "g42".into(),
        new_value: "group42".into(),
        regex: false,
    };
    sweep_unary(&mut results, "table.replace", true, "sostituzione letterale utf8", |batch| {
        replace(batch, &replace_config).expect("replace")
    });

    // --- Filtering ---------------------------------------------------------------
    let conditional_config = Conditional {
        column: "num".into(),
        conditions: vec![
            Condition { operator: Operator::Lt, value: json!(2500.0), result: json!("low") },
            Condition { operator: Operator::Lt, value: json!(5000.0), result: json!("mid") },
            Condition { operator: Operator::Lt, value: json!(7500.0), result: json!("high") },
        ],
        default_value: json!("top"),
        output_column: "band".into(),
    };
    sweep_unary(&mut results, "table.conditional", true, "3 condizioni numeriche", |batch| {
        conditional(batch, &conditional_config).expect("conditional")
    });

    // --- Aggregazione --------------------------------------------------------------
    let distinct_config = Distinct {
        subset: vec!["key".into()],
        keep: Keep::First,
    };
    results.push(measure(
        "table.distinct",
        M10,
        3,
        "subset key, ~1M valori distinti su 10M righe [10M dichiarate]",
        || distinct(base_10m(), &distinct_config).expect("distinct"),
    ));

    let dedup_config = DedupAdvanced {
        subset: vec!["key".into()],
        keep: Keep::First,
        order_column: Some("id".into()),
        ascending: true,
    };
    sweep_unary(&mut results, "table.dedup_advanced", false, "subset key, order id", |batch| {
        dedup_advanced(batch, &dedup_config).expect("dedup_advanced")
    });

    let rolling_config = RollingWindow {
        column: "num".into(),
        function: RollingKind::Mean,
        group_by: Some("grp".into()),
        order_column: Some("id".into()),
        window: 10,
        min_periods: 1,
        ddof: 1,
        output_column: "num_roll".into(),
    };
    sweep_unary(&mut results, "table.rolling_window", false, "mean w=10, partizione grp", |batch| {
        rolling_window(batch, &rolling_config).expect("rolling_window")
    });

    let window_config = WindowFunction {
        column: "num".into(),
        function: WindowKind::Rank,
        group_by: Some("grp".into()),
        order_column: Some("num".into()),
        offset: 1,
        buckets: None,
        output_column: Some("num_rank".into()),
    };
    sweep_unary(&mut results, "table.window_function", false, "rank, partizione grp, order num", |batch| {
        window_function(batch, &window_config).expect("window_function")
    });

    // --- Reshape ---------------------------------------------------------------------
    let explode_config = Explode {
        column: "items".into(),
        output_column: None,
        empty_policy: plenora_kernels_table::reshape::EmptyListPolicy::Null,
    };
    let list_input = list_fixture(M1);
    results.push(measure("table.explode", M1, 3, "List<Int64> 0..4 elementi (~2.5M out)", || {
        explode(&list_input, &explode_config, &limits).expect("explode")
    }));

    let unnest_config = Unnest {
        column: "payload".into(),
        prefix: String::new(),
        drop_source: true,
    };
    let struct_input = struct_fixture(M1);
    results.push(measure("table.unnest", M1, 3, "Struct{a,b,c}", || {
        unnest(&struct_input, &unnest_config, &limits).expect("unnest")
    }));

    let transpose_config = Transpose {
        id_column: None,
        output_columns: Vec::new(),
        type_policy: HeterogeneousTypePolicy::Reject,
    };
    let transpose_input = transpose_fixture(4_000);
    results.push(measure(
        "table.transpose",
        4_000,
        3,
        "8 colonne f64 x 4000 righe (contratto: righe <= max_columns; un take per colonna output)",
        || transpose(&transpose_input, &transpose_config, &limits).expect("transpose"),
    ));

    let table_diff_config = TableDiff {
        left_keys: vec!["id".into()],
        right_keys: vec!["id".into()],
        compare_columns: vec!["num".into()],
        include_unchanged: "no".into(),
        separator: ", ".into(),
    };
    results.push(measure("table.table_diff", M1, 3, "1M x 1M, chiave id, diff su num", || {
        table_diff(base_1m(), right_1m(), &table_diff_config, &limits).expect("table_diff")
    }));

    // --- Join --------------------------------------------------------------------------
    let concat_config = Concat { ignore_index: true };
    results.push(measure("table.concat", M1, 3, "500k + 500k righe", || {
        let half = base_1m().num_rows() / 2;
        let left = base_1m().slice(0, half);
        let right = base_1m().slice(half, base_1m().num_rows() - half);
        concat(&left, &right, &concat_config, &limits).expect("concat")
    }));

    let cross_join_config = CrossJoin {};
    let cross_left = base_fixture(1_000);
    let cross_right = right_fixture(1_000);
    results.push(measure(
        "table.cross_join",
        M1,
        3,
        "1000 x 1000 righe (righe = output)",
        || cross_join(&cross_left, &cross_right, &cross_join_config, &limits).expect("cross_join"),
    ));

    let asof_config = AsOfJoin {
        left_on: "ts".into(),
        right_on: "ts".into(),
        left_by: Vec::new(),
        right_by: Vec::new(),
        direction: plenora_kernels_table::joins::AsOfDirection::Backward,
        tolerance: None,
        allow_exact: true,
    };
    let asof_left = asof_fixture(M1, 0);
    let asof_right = asof_fixture(M1, 1);
    results.push(measure("table.asof_join", M1, 3, "1M x 1M backward su ts int64", || {
        asof_join(&asof_left, &asof_right, &asof_config, &limits).expect("asof_join")
    }));

    // --- Set operations ------------------------------------------------------------------
    let setop_config = SetOperation {};
    results.push(measure("table.union_distinct", M1, 3, "1M + 1M righe, overlap 50%", || {
        union_distinct(base_1m(), setop_right_1m(), &setop_config, &limits).expect("union_distinct")
    }));
    results.push(measure("table.intersect", M1, 3, "1M x 1M righe, overlap 50%", || {
        intersect(base_1m(), setop_right_1m(), &setop_config).expect("intersect")
    }));
    results.push(measure("table.except", M1, 3, "1M x 1M righe, overlap 50%", || {
        except(base_1m(), setop_right_1m(), &setop_config).expect("except")
    }));

    // --- Security --------------------------------------------------------------------------
    let md5_config = Md5Hash {
        columns: vec!["text".into()],
        output_column: "md5_hash".into(),
        normalize: true,
        null_policy: HashNullPolicy::Empty,
        null_literal: String::new(),
    };
    sweep_unary(&mut results, "table.md5_hash", true, "1 colonna utf8 40 char", |batch| {
        md5_hash(batch, &md5_config).expect("md5_hash")
    });

    let sha256_config = Sha256Hash {
        columns: vec!["text".into()],
        output_column: "sha256_hash".into(),
        normalize: true,
        null_policy: HashNullPolicy::Empty,
        null_literal: String::new(),
    };
    sweep_unary(&mut results, "table.sha256_hash", true, "1 colonna utf8 40 char", |batch| {
        sha256_hash(batch, &sha256_config).expect("sha256_hash")
    });

    let mask_config = MaskData {
        maskings: vec![Masking {
            column: "text".into(),
            mask_type: MaskType::Custom,
            chars_start: 3,
            chars_end: 3,
            mask_char: "*".into(),
        }],
        overwrite: true,
    };
    sweep_unary(&mut results, "table.mask_data", true, "mask custom 3+3 su text", |batch| {
        mask_data(batch, &mask_config).expect("mask_data")
    });

    // --- Strings ------------------------------------------------------------------------------
    let length_config = StringLength {
        column: "text".into(),
        output_column: Some("text_len".into()),
    };
    sweep_unary(&mut results, "table.string_length", true, "stringhe 40 char", |batch| {
        string_length(batch, &length_config).expect("string_length")
    });

    let pad_config = StringPad {
        column: "text".into(),
        width: 48,
        side: PadSide::Left,
        fill_char: "0".into(),
        output_column: Some("text_pad".into()),
    };
    sweep_unary(&mut results, "table.string_pad", true, "width 48 left su 40 char", |batch| {
        string_pad(batch, &pad_config, &limits).expect("string_pad")
    });

    // --- Utility ----------------------------------------------------------------------------------
    let row_number_config = AddRowNumber {
        output_column: "row_number".into(),
        start: 1,
        partition_column: None,
        order_column: None,
        ascending: true,
    };
    sweep_unary(&mut results, "table.add_row_number", true, "senza partizione", |batch| {
        add_row_number(batch, &row_number_config).expect("add_row_number")
    });

    let uuid_config = UuidGenerator {
        output_column: "uuid".into(),
    };
    sweep_unary(&mut results, "table.uuid_generator", true, "uuid v4 per riga", |batch| {
        uuid_generator(batch, &uuid_config).expect("uuid_generator")
    });

    // --- Quality --------------------------------------------------------------------------------------
    let not_null_config = AssertNotNull {
        columns: vec!["id".into(), "num".into()],
    };
    sweep_unary(&mut results, "table.assert_not_null", true, "2 colonne", |batch| {
        assert_not_null(batch, &not_null_config).expect("assert_not_null")
    });

    let range_config = AssertRange {
        column: "num".into(),
        min: Some(0.0),
        max: Some(10_000.0),
        inclusive_min: true,
        inclusive_max: true,
        allow_null: false,
    };
    sweep_unary(&mut results, "table.assert_range", true, "num in 0..10000", |batch| {
        assert_range(batch, &range_config).expect("assert_range")
    });

    let regex_config = AssertRegex {
        column: "text".into(),
        pattern: "^[0-9a-f]{40}$".into(),
        allow_null: false,
    };
    sweep_unary(&mut results, "table.assert_regex", true, "^[0-9a-f]{40}$", |batch| {
        assert_regex(batch, &regex_config).expect("assert_regex")
    });

    let schema_config = AssertSchema {
        fields: vec![
            SchemaExpectation { name: "id".into(), data_type: "int64".into(), nullable: None },
            SchemaExpectation { name: "num".into(), data_type: "float64".into(), nullable: None },
            SchemaExpectation { name: "grp".into(), data_type: "utf8".into(), nullable: None },
            SchemaExpectation { name: "text".into(), data_type: "utf8".into(), nullable: None },
            SchemaExpectation { name: "key".into(), data_type: "int64".into(), nullable: None },
            SchemaExpectation { name: "path".into(), data_type: "utf8".into(), nullable: None },
        ],
        allow_extra: false,
        ordered: true,
    };
    sweep_unary(&mut results, "table.assert_schema", true, "6 campi ordinati", |batch| {
        assert_schema(batch, &schema_config).expect("assert_schema")
    });

    let unique_config = AssertUnique {
        columns: vec!["id".into()],
        nulls_equal: true,
    };
    sweep_unary(&mut results, "table.assert_unique", false, "chiave id unica", |batch| {
        assert_unique(batch, &unique_config).expect("assert_unique")
    });

    // --- Governance --------------------------------------------------------------------------------------
    let cardinality_config = AssertCardinality {
        exact_rows: None,
        min_rows: Some(1),
        max_rows: None,
    };
    sweep_unary(&mut results, "table.assert_cardinality", true, "min_rows=1", |batch| {
        assert_cardinality(batch, &cardinality_config).expect("assert_cardinality")
    });

    let metadata_config = AssertMetadata {
        expected: [("source".to_owned(), "bench_sweep".to_owned())].into_iter().collect(),
        allow_extra: true,
    };
    sweep_unary(&mut results, "table.assert_metadata", true, "1 chiave metadata", |batch| {
        assert_metadata(batch, &metadata_config).expect("assert_metadata")
    });

    let foreign_key_config = ForeignKey {
        left_keys: vec!["key".into()],
        right_keys: vec!["id".into()],
        allow_null: false,
    };
    results.push(measure("table.assert_foreign_key", M1, 3, "1M chiavi vs 1M referenze", || {
        assert_foreign_key(base_1m(), right_1m(), &foreign_key_config, &limits)
            .expect("assert_foreign_key")
    }));

    let reconcile_config = Reconcile {
        left_keys: vec!["key".into()],
        right_keys: vec!["id".into()],
        nulls_equal: true,
    };
    results.push(measure("table.reconcile", M1, 3, "1M x 1M, frequenze chiave", || {
        reconcile(base_1m(), right_1m(), &reconcile_config, &limits).expect("reconcile")
    }));

    write_outputs(&results);
    eprintln!("sweep completato: {} misure -> benchmarks/sweep/", results.len());
}
