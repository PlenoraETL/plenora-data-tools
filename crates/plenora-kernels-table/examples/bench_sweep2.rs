//! Sweep prestazionale, seconda ondata (`bench_sweep2)`: 26 kernel tabellari
//! residui (mai ottimizzati) + i 9 kernel delle estensioni v1.1/v1.2/v1.3
//! (`select_columns`, `limit`, `top_n`, `stable_fingerprint`, `align_schema`,
//! `concat_by_name`, `validate_rules`, `hmac_sha256`, `fuzzy_join`).
//!
//! Stesse fixture e stesso protocollo di `bench_sweep.rs` (confrontabilita'):
//! seed logico 42 via xorshift, mediana di 3 run, righe/s, peak RSS
//! (`VmHWM`). Scala: 1M righe di default; le op con mediana < 1s a 1M sono
//! rimisurate automaticamente a 10M (`escalate`). Op a scala fissa:
//! `transpose` (limitata a `max_columns` righe per contratto), `cross_join`
//! (1k x 1k = 1M righe output), `concat`/`concat_by_name` (1M righe totali
//! sugli input), `fuzzy_join` (1M sinistra x 10k destra: la complessita' e'
//! dominata dal prodotto riga x candidati di blocco, escalation disattivata).
//!
//! Config rappresentative dei nuovi kernel: `top_n` n=100, `fuzzy_join`
//! `jaro_winkler` con blocking prefix su anagrafica sintetica (fixture di
//! `bench_fuzzy_join`), `hmac_sha256` con chiave da variabile d'ambiente,
//! `validate_rules` 5 regole miste, `concat_by_name` 3 input a schemi
//! permutati, `align_schema` 20 colonne.
//!
//! Uso: `bench_sweep2` — scrive `benchmarks/sweep/sweep2.json` e
//! `benchmarks/sweep/sweep2.md` (relativi alla cwd, /work in Docker) e
//! stampa le stesse righe JSON su stdout.

#[path = "comune/mod.rs"]
mod comune;

use comune::fixture::{right_fixture, struct_fixture};

use comune::rng::Rng;
use comune::{measure_record, Measurement};

use std::fmt::Write as _;
use std::sync::{Arc, OnceLock};

use plenora_core::arrow::array::{
    types::Int64Type, Array, ArrayRef, Float64Array, Int64Array, ListArray, RecordBatch,
    StringArray,
};
use plenora_core::arrow::schema::{DataType, Field, Schema};
use plenora_kernels_table::aggregation::{top_n, TopN};
use plenora_kernels_table::analysis::{bin, lookup, sample, Bin, Bins, Lookup, Sample};
use plenora_kernels_table::cleansing::{replace, Replace};
use plenora_kernels_table::columns::{
    align_schema, concat_columns, select_columns, split_column, AlignColumn, AlignSchema,
    AlignType, ConcatColumns, SelectColumns, SplitColumn,
};
use plenora_kernels_table::filtering::{conditional, Condition, Conditional, Operator};
use plenora_kernels_table::fuzzy::{fuzzy_join, FuzzyBlocking, FuzzyHow, FuzzyJoin, FuzzyMetric};
use plenora_kernels_table::governance::{
    assert_cardinality, assert_metadata, validate_rules, AssertCardinality, AssertMetadata,
    RuleOperator, RuleSeverity, ValidateOutputMode, ValidateRule, ValidateRules,
};
use plenora_kernels_table::joins::{
    asof_join, concat, concat_by_name, cross_join, AsOfJoin, Concat, ConcatByName, CrossJoin,
};
use plenora_kernels_table::quality::{
    assert_not_null, assert_range, assert_regex, assert_schema, assert_unique, AssertNotNull,
    AssertRange, AssertRegex, AssertSchema, AssertUnique, SchemaExpectation,
};
use plenora_kernels_table::reshape::{
    explode, transpose, unnest, Explode, HeterogeneousTypePolicy, Transpose, Unnest,
};
use plenora_kernels_table::security::{
    hmac_sha256, md5_hash, sha256_hash, stable_fingerprint, FingerprintAlgorithm, HashNullPolicy,
    HmacNullPolicy, HmacSha256, Md5Hash, Sha256Hash, StableFingerprint,
};
use plenora_kernels_table::strings::{string_length, string_pad, PadSide, StringLength, StringPad};
use plenora_kernels_table::utility::{
    add_row_number, limit, uuid_generator, AddRowNumber, Limit, UuidGenerator,
};
use plenora_kernels_table::Limits;
use serde_json::{json, Value};

const M1: usize = 1_000_000;
const M10: usize = 10_000_000;
/// Soglia di escalation: sotto 1s a 1M righe l'op viene rimisurata a 10M.
const ESCALATION_SECONDS: f64 = 1.0;
/// Variabile d'ambiente con la chiave HMAC del benchmark (impostata in main).
const HMAC_KEY_ENV: &str = "PLENORA_BENCH_HMAC_KEY";

/// Limiti allargati per le scale di benchmark (10M righe, container 10g).
fn bench_limits() -> Limits {
    Limits {
        max_rows: 40_000_000,
        max_governed_memory_bytes: 6 * 1024 * 1024 * 1024,
        ..Limits::default()
    }
}

/// Fixture base condivisa: identica a `bench_sweep.rs` (seed 42, 9 draw per
/// riga) per confrontabilita' con lo sweep precedente.
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
        std::iter::once(("source".to_owned(), "bench_sweep2".to_owned())).collect(),
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

/// Fixture a 20 colonne per `align_schema`: 7 int64, 7 float64, 6 utf8
/// (`c00`..`c19`, tipo determinato dall'indice: i%3 -> int64/float64/utf8).
fn wide_fixture(rows: usize) -> RecordBatch {
    let mut rng = Rng::seeded();
    let mut columns: Vec<ArrayRef> = Vec::with_capacity(20);
    let mut fields: Vec<Field> = Vec::with_capacity(20);
    for index in 0..20 {
        let name = format!("c{index:02}");
        match index % 3 {
            0 => {
                let values = (0..rows)
                    .map(|_| Some(rng.next().cast_signed()))
                    .collect::<Vec<_>>();
                fields.push(Field::new(&name, DataType::Int64, false));
                columns.push(Arc::new(Int64Array::from(values)));
            }
            1 => {
                // Bound evidente: draw % 1_000_000 <= 999_999 < 2^53, cast esatto in f64.
                #[allow(clippy::cast_precision_loss)]
                let values = (0..rows)
                    .map(|_| Some((rng.next() % 1_000_000) as f64 / 100.0))
                    .collect::<Vec<_>>();
                fields.push(Field::new(&name, DataType::Float64, false));
                columns.push(Arc::new(Float64Array::from(values)));
            }
            _ => {
                let values = (0..rows)
                    .map(|_| format!("{:016x}", rng.next()))
                    .collect::<Vec<_>>();
                fields.push(Field::new(&name, DataType::Utf8, false));
                columns.push(Arc::new(StringArray::from(values)));
            }
        }
    }
    RecordBatch::try_new(Arc::new(Schema::new(fields)), columns).expect("fixture wide")
}

/// Config `align_schema` rappresentativa: 20 colonne dichiarate — le 18
/// colonne `c00`..`c17` in ordine permutato (rotazione di 5), piu' una
/// colonna assente con `default` scalare e una assente senza default (null);
/// `c18`/`c19` non dichiarate sono scartate (`keep_extra = false`).
fn align_config() -> AlignSchema {
    let align_type_of = |index: usize| match index % 3 {
        0 => AlignType::Int64,
        1 => AlignType::Float64,
        _ => AlignType::Utf8,
    };
    let mut columns: Vec<AlignColumn> = (0..18)
        .map(|position| {
            let index = (position + 5) % 18;
            AlignColumn {
                name: format!("c{index:02}"),
                align_type: align_type_of(index),
                default: None,
            }
        })
        .collect();
    columns.push(AlignColumn {
        name: "c_default".into(),
        align_type: AlignType::Float64,
        default: Some(json!(0.5)),
    });
    columns.push(AlignColumn {
        name: "c_added".into(),
        align_type: AlignType::Int64,
        default: None,
    });
    AlignSchema {
        columns,
        keep_extra: false,
    }
}

/// Fixture asof: timestamp int64 fitti; la destra e' sfasata di +1.
fn asof_fixture(rows: usize, offset: i64) -> RecordBatch {
    let ids = (0..rows)
        .map(|row| Some(2 * i64::try_from(row).unwrap_or(0) + offset))
        .collect::<Vec<_>>();
    // Bound evidente: row < rows e rows <= `M10` = 10^7 (scale del bench,
    // costanti in testa al file) << 2^53: cast esatto in f64.
    #[allow(clippy::cast_precision_loss)]
    let vals = (0..rows).map(|row| Some(row as f64)).collect::<Vec<_>>();
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
    let ids = (0..rows)
        .map(|row| i64::try_from(row).ok())
        .collect::<Vec<_>>();
    // Bound evidente: row < rows <= `M10` = 10^7 (scale del bench) e
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

// ---------------------------------------------------------------------------
// Fixture anagrafica per fuzzy_join (identica a `bench_fuzzy_join.rs`:
// LCG Knuth MMIX, seed logico 42, sillabe italiane, errori di battitura).
// ---------------------------------------------------------------------------

/// LCG deterministico (Knuth MMIX), seed logico 42.
struct Lcg(u64);

impl Lcg {
    const fn seeded() -> Self {
        Self(42)
    }

    const fn next_u64(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 11
    }

    const fn below(&mut self, bound: u64) -> u64 {
        self.next_u64() % bound
    }
}

const SYLLABLES: &[&str] = &[
    "mar", "ros", "ber", "lan", "gio", "van", "fer", "col", "ric", "fra", "gal", "ben", "pas",
    "lom", "tre", "vis", "zan", "qui", "dor", "mel", "nar", "fos", "bru", "car", "sil", "tam",
    "ver", "oni", "etti", "ini",
];

/// Nome sintetico da tre sillabe (lettera iniziale maiuscola).
fn name_of(seed: u64) -> String {
    let mut lcg = Lcg(seed.wrapping_add(1));
    // Bound evidente: below(n) < n e n = SYLLABLES.len() deriva da usize,
    // quindi il risultato rientra in usize.
    #[allow(clippy::cast_possible_truncation)]
    let pick = |lcg: &mut Lcg| SYLLABLES[lcg.below(SYLLABLES.len() as u64) as usize];
    let first = pick(&mut lcg);
    let mut name = String::with_capacity(12);
    let mut chars = first.chars();
    if let Some(head) = chars.next() {
        name.extend(head.to_uppercase());
    }
    name.push_str(chars.as_str());
    name.push_str(pick(&mut lcg));
    name.push_str(pick(&mut lcg));
    name
}

/// Errore di battitura deterministico: trasposizione di due caratteri
/// adiacenti o sostituzione con una lettera vicina.
fn typo(name: &str, seed: u64) -> String {
    let mut lcg = Lcg(seed.wrapping_add(7));
    let mut chars: Vec<char> = name.chars().collect();
    if chars.len() < 2 {
        return name.to_owned();
    }
    // Bound evidente: below(n) < n e n = chars.len() - 1 deriva da usize,
    // quindi il risultato rientra in usize.
    #[allow(clippy::cast_possible_truncation)]
    let position = lcg.below((chars.len() - 1) as u64) as usize;
    if lcg.below(2) == 0 {
        chars.swap(position, position + 1);
    } else {
        let replacement = (b'a' + u8::try_from(lcg.below(26)).unwrap_or(0)) as char;
        chars[position + 1] = replacement;
    }
    chars.into_iter().collect()
}

fn anagrafica(rows: usize, names: &[String], payload_prefix: i64) -> RecordBatch {
    let payload = (0..rows)
        .map(|row| Some(payload_prefix + i64::try_from(row).unwrap_or(0)))
        .collect::<Vec<_>>();
    RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("name", DataType::Utf8, false),
            Field::new("payload", DataType::Int64, true),
        ])),
        vec![
            Arc::new(StringArray::from(names.to_vec())),
            Arc::new(Int64Array::from(payload)),
        ],
    )
    .expect("fixture anagrafica")
}

static BASE_1M: OnceLock<RecordBatch> = OnceLock::new();
static BASE_10M: OnceLock<RecordBatch> = OnceLock::new();
static WIDE_1M: OnceLock<RecordBatch> = OnceLock::new();
static WIDE_10M: OnceLock<RecordBatch> = OnceLock::new();

fn base_1m() -> &'static RecordBatch {
    BASE_1M.get_or_init(|| base_fixture(M1))
}

fn base_10m() -> &'static RecordBatch {
    BASE_10M.get_or_init(|| base_fixture(M10))
}

fn wide_1m() -> &'static RecordBatch {
    WIDE_1M.get_or_init(|| wide_fixture(M1))
}

fn wide_10m() -> &'static RecordBatch {
    WIDE_10M.get_or_init(|| wide_fixture(M10))
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
    let measured = measure_record(op, M1, 3, note, || execute(base_1m()));
    let fast = measured.median_seconds < ESCALATION_SECONDS;
    results.push(measured);
    if escalate && fast {
        let note10 = format!("{note} [10M]");
        let scaled = measure_record(op, M10, 3, &note10, || execute(base_10m()));
        results.push(scaled);
    }
}

/// Come `sweep_unary` ma sulla fixture a 20 colonne (`align_schema`).
fn sweep_wide(
    results: &mut Vec<Measurement>,
    op: &'static str,
    escalate: bool,
    note: &str,
    execute: impl Fn(&RecordBatch) -> RecordBatch + Copy,
) {
    let measured = measure_record(op, M1, 3, note, || execute(wide_1m()));
    let fast = measured.median_seconds < ESCALATION_SECONDS;
    results.push(measured);
    if escalate && fast {
        let note10 = format!("{note} [10M]");
        let scaled = measure_record(op, M10, 3, &note10, || execute(wide_10m()));
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
        "benchmark": "bench_sweep2",
        "seed": 42,
        "scale_default_rows": M1,
        "scale_escalated_rows": M10,
        "escalation_threshold_seconds": ESCALATION_SECONDS,
        "results": json_records,
    });
    std::fs::write(
        directory.join("sweep2.json"),
        serde_json::to_string_pretty(&document).expect("JSON"),
    )
    .expect("write sweep2.json");

    let mut sorted: Vec<&Measurement> = results.iter().collect();
    sorted.sort_by(|left, right| left.rows_per_second.total_cmp(&right.rows_per_second));
    let mut markdown = String::new();
    markdown.push_str(
        "# Sweep kernel tabellari, seconda ondata (bench_sweep2)\n\n\
         26 op residui (mai ottimizzate) + 9 op nuove (estensioni v1.1-v1.3).\n\
         Fixture deterministica identica a `bench_sweep` (seed 42), mediana di\n\
         3 run, container Docker `--cpus=4 --memory=10g`, release. Scala 1M\n\
         righe; le op con mediana < 1s a 1M sono rimisurate a 10M (righe\n\
         `[10M]`). Classifica ordinata per lentezza (righe/s crescenti). Il\n\
         peak RSS e' il `VmHWM` di processo (cumulativo, cresce con le fixture\n\
         condivise a 10M).\n\n\
         | # | op | righe | mediana (s) | righe/s | righe output | peak RSS (MiB) | note |\n\
         |---|----|-------|-------------|---------|--------------|----------------|------|\n",
    );
    for (position, entry) in sorted.iter().enumerate() {
        writeln!(
            markdown,
            "| {} | `{}` | {} | {:.4} | {:.0} | {} | {} | {} |",
            position + 1,
            entry.op,
            entry.rows,
            entry.median_seconds,
            entry.rows_per_second,
            entry.output_rows,
            entry
                .peak_rss_kib
                .map_or_else(|| "n/d".into(), |kib| format!("{}", kib / 1024)),
            entry.note,
        )
        .expect("markdown");
    }
    std::fs::write(directory.join("sweep2.md"), markdown).expect("write sweep2.md");
}

#[allow(clippy::too_many_lines)]
fn main() {
    let limits = bench_limits();
    let mut results: Vec<Measurement> = Vec::new();
    // Chiave HMAC del benchmark: solo il NOME della variabile entra nella
    // config del kernel, il valore resta fuori da piano e output.
    std::env::set_var(HMAC_KEY_ENV, "bench-sweep2-hmac-key-seed-42");

    // =====================================================================
    // Kernel residui (stesse config dello sweep precedente)
    // =====================================================================

    // --- Analisi -------------------------------------------------------------
    let bin_config = Bin {
        column: "num".into(),
        bins: Bins::Count(20),
        labels: None,
        output_column: Some("num_bin".into()),
    };
    sweep_unary(
        &mut results,
        "table.bin",
        true,
        "20 bucket equal-width",
        |batch| bin(batch, &bin_config).expect("bin"),
    );

    let lookup_mapping = (0..1_024)
        .map(|index| (format!("g{index}"), Value::from(format!("c{index}"))))
        .collect();
    let lookup_config = Lookup {
        column: "grp".into(),
        mapping: lookup_mapping,
        default: Value::Null,
        output_column: Some("grp_code".into()),
    };
    sweep_unary(
        &mut results,
        "table.lookup",
        true,
        "mappa 1024 chiavi utf8",
        |batch| lookup(batch, &lookup_config).expect("lookup"),
    );

    let sample_config = Sample {
        n: 100,
        fraction: Some(0.1),
        random_state: Some(42),
        stratify_column: None,
    };
    sweep_unary(
        &mut results,
        "table.sample",
        true,
        "fraction 0.1",
        |batch| sample(batch, &sample_config).expect("sample"),
    );

    // --- Colonne --------------------------------------------------------------
    let concat_columns_config = ConcatColumns {
        columns: vec!["grp".into(), "text".into()],
        output_column: "combined".into(),
        separator: "-".into(),
        skip_null: true,
    };
    sweep_unary(
        &mut results,
        "table.concat_columns",
        true,
        "2 colonne utf8",
        |batch| concat_columns(batch, &concat_columns_config, &limits).expect("concat_columns"),
    );

    let split_config = SplitColumn {
        column: "path".into(),
        delimiter: "/".into(),
        new_columns: vec!["p1".into(), "p2".into(), "p3".into()],
        max_splits: -1,
    };
    sweep_unary(
        &mut results,
        "table.split_column",
        true,
        "3 colonne su '/'",
        |batch| split_column(batch, &split_config, &limits).expect("split_column"),
    );

    // --- Cleansing -------------------------------------------------------------
    let replace_config = Replace {
        column: "grp".into(),
        old_value: "g42".into(),
        new_value: "group42".into(),
        regex: false,
    };
    sweep_unary(
        &mut results,
        "table.replace",
        true,
        "sostituzione letterale utf8",
        |batch| replace(batch, &replace_config).expect("replace"),
    );

    // --- Filtering ---------------------------------------------------------------
    let conditional_config = Conditional {
        column: "num".into(),
        conditions: vec![
            Condition {
                operator: Operator::Lt,
                value: json!(2500.0),
                result: json!("low"),
            },
            Condition {
                operator: Operator::Lt,
                value: json!(5000.0),
                result: json!("mid"),
            },
            Condition {
                operator: Operator::Lt,
                value: json!(7500.0),
                result: json!("high"),
            },
        ],
        default_value: json!("top"),
        output_column: "band".into(),
    };
    sweep_unary(
        &mut results,
        "table.conditional",
        true,
        "3 condizioni numeriche",
        |batch| conditional(batch, &conditional_config).expect("conditional"),
    );

    // --- Reshape ---------------------------------------------------------------------
    let explode_config = Explode {
        column: "items".into(),
        output_column: None,
        empty_policy: plenora_kernels_table::reshape::EmptyListPolicy::Null,
    };
    let list_input = list_fixture(M1);
    results.push(measure_record(
        "table.explode",
        M1,
        3,
        "List<Int64> 0..4 elementi (~2.5M out)",
        || explode(&list_input, &explode_config, &limits).expect("explode"),
    ));

    let unnest_config = Unnest {
        column: "payload".into(),
        prefix: String::new(),
        drop_source: true,
    };
    let struct_input = struct_fixture(M1);
    results.push(measure_record(
        "table.unnest",
        M1,
        3,
        "Struct{a,b,c}",
        || unnest(&struct_input, &unnest_config, &limits).expect("unnest"),
    ));

    let transpose_config = Transpose {
        id_column: None,
        output_columns: Vec::new(),
        type_policy: HeterogeneousTypePolicy::Reject,
    };
    let transpose_input = transpose_fixture(4_000);
    results.push(measure_record(
        "table.transpose",
        4_000,
        3,
        "8 colonne f64 x 4000 righe (contratto: righe <= max_columns; un take per colonna output)",
        || transpose(&transpose_input, &transpose_config, &limits).expect("transpose"),
    ));

    // --- Join --------------------------------------------------------------------------
    let concat_config = Concat { ignore_index: true };
    results.push(measure_record(
        "table.concat",
        M1,
        3,
        "500k + 500k righe",
        || {
            let half = base_1m().num_rows() / 2;
            let left = base_1m().slice(0, half);
            let right = base_1m().slice(half, base_1m().num_rows() - half);
            concat(&left, &right, &concat_config, &limits).expect("concat")
        },
    ));

    let cross_join_config = CrossJoin {};
    let cross_left = base_fixture(1_000);
    let cross_right = right_fixture(1_000);
    results.push(measure_record(
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
    results.push(measure_record(
        "table.asof_join",
        M1,
        3,
        "1M x 1M backward su ts int64",
        || asof_join(&asof_left, &asof_right, &asof_config, &limits).expect("asof_join"),
    ));

    // --- Security --------------------------------------------------------------------------
    let md5_config = Md5Hash {
        columns: vec!["text".into()],
        output_column: "md5_hash".into(),
        normalize: true,
        null_policy: HashNullPolicy::Empty,
        null_literal: String::new(),
    };
    sweep_unary(
        &mut results,
        "table.md5_hash",
        true,
        "1 colonna utf8 40 char",
        |batch| md5_hash(batch, &md5_config).expect("md5_hash"),
    );

    let sha256_config = Sha256Hash {
        columns: vec!["text".into()],
        output_column: "sha256_hash".into(),
        normalize: true,
        null_policy: HashNullPolicy::Empty,
        null_literal: String::new(),
    };
    sweep_unary(
        &mut results,
        "table.sha256_hash",
        true,
        "1 colonna utf8 40 char",
        |batch| sha256_hash(batch, &sha256_config).expect("sha256_hash"),
    );

    // --- Strings ------------------------------------------------------------------------------
    let length_config = StringLength {
        column: "text".into(),
        output_column: Some("text_len".into()),
    };
    sweep_unary(
        &mut results,
        "table.string_length",
        true,
        "stringhe 40 char",
        |batch| string_length(batch, &length_config).expect("string_length"),
    );

    let pad_config = StringPad {
        column: "text".into(),
        width: 48,
        side: PadSide::Left,
        fill_char: "0".into(),
        output_column: Some("text_pad".into()),
    };
    sweep_unary(
        &mut results,
        "table.string_pad",
        true,
        "width 48 left su 40 char",
        |batch| string_pad(batch, &pad_config, &limits).expect("string_pad"),
    );

    // --- Utility ----------------------------------------------------------------------------------
    let row_number_config = AddRowNumber {
        output_column: "row_number".into(),
        start: 1,
        partition_column: None,
        order_column: None,
        ascending: true,
    };
    sweep_unary(
        &mut results,
        "table.add_row_number",
        true,
        "senza partizione",
        |batch| add_row_number(batch, &row_number_config).expect("add_row_number"),
    );

    let uuid_config = UuidGenerator {
        output_column: "uuid".into(),
    };
    sweep_unary(
        &mut results,
        "table.uuid_generator",
        true,
        "uuid v4 per riga",
        |batch| uuid_generator(batch, &uuid_config).expect("uuid_generator"),
    );

    // --- Quality --------------------------------------------------------------------------------------
    let not_null_config = AssertNotNull {
        columns: vec!["id".into(), "num".into()],
    };
    sweep_unary(
        &mut results,
        "table.assert_not_null",
        true,
        "2 colonne",
        |batch| assert_not_null(batch, &not_null_config).expect("assert_not_null"),
    );

    let range_config = AssertRange {
        column: "num".into(),
        min: Some(0.0),
        max: Some(10_000.0),
        inclusive_min: true,
        inclusive_max: true,
        allow_null: false,
    };
    sweep_unary(
        &mut results,
        "table.assert_range",
        true,
        "num in 0..10000",
        |batch| assert_range(batch, &range_config).expect("assert_range"),
    );

    let regex_config = AssertRegex {
        column: "text".into(),
        pattern: "^[0-9a-f]{40}$".into(),
        allow_null: false,
    };
    sweep_unary(
        &mut results,
        "table.assert_regex",
        true,
        "^[0-9a-f]{40}$",
        |batch| assert_regex(batch, &regex_config).expect("assert_regex"),
    );

    let schema_config = AssertSchema {
        fields: vec![
            SchemaExpectation {
                name: "id".into(),
                data_type: "int64".into(),
                nullable: None,
            },
            SchemaExpectation {
                name: "num".into(),
                data_type: "float64".into(),
                nullable: None,
            },
            SchemaExpectation {
                name: "grp".into(),
                data_type: "utf8".into(),
                nullable: None,
            },
            SchemaExpectation {
                name: "text".into(),
                data_type: "utf8".into(),
                nullable: None,
            },
            SchemaExpectation {
                name: "key".into(),
                data_type: "int64".into(),
                nullable: None,
            },
            SchemaExpectation {
                name: "path".into(),
                data_type: "utf8".into(),
                nullable: None,
            },
        ],
        allow_extra: false,
        ordered: true,
    };
    sweep_unary(
        &mut results,
        "table.assert_schema",
        true,
        "6 campi ordinati",
        |batch| assert_schema(batch, &schema_config).expect("assert_schema"),
    );

    let unique_config = AssertUnique {
        columns: vec!["id".into()],
        nulls_equal: true,
    };
    sweep_unary(
        &mut results,
        "table.assert_unique",
        false,
        "chiave id unica",
        |batch| assert_unique(batch, &unique_config).expect("assert_unique"),
    );

    // --- Governance --------------------------------------------------------------------------------------
    let cardinality_config = AssertCardinality {
        exact_rows: None,
        min_rows: Some(1),
        max_rows: None,
    };
    sweep_unary(
        &mut results,
        "table.assert_cardinality",
        true,
        "min_rows=1",
        |batch| assert_cardinality(batch, &cardinality_config).expect("assert_cardinality"),
    );

    let metadata_config = AssertMetadata {
        expected: std::iter::once(("source".to_owned(), "bench_sweep2".to_owned())).collect(),
        allow_extra: true,
    };
    sweep_unary(
        &mut results,
        "table.assert_metadata",
        true,
        "1 chiave metadata",
        |batch| assert_metadata(batch, &metadata_config).expect("assert_metadata"),
    );

    // =====================================================================
    // Kernel nuovi (estensioni v1.1/v1.2/v1.3)
    // =====================================================================

    let select_config = SelectColumns {
        columns: vec!["text".into(), "id".into(), "num".into()],
    };
    sweep_unary(
        &mut results,
        "table.select_columns",
        true,
        "3 colonne su 6, ordine permutato",
        |batch| select_columns(batch, &select_config).expect("select_columns"),
    );

    let limit_config = Limit {
        n: 500_000,
        offset: 100,
    };
    sweep_unary(
        &mut results,
        "table.limit",
        true,
        "n=500k offset=100 (slice zero-copy)",
        |batch| limit(batch, &limit_config).expect("limit"),
    );

    let top_n_config = TopN {
        columns: vec!["num".into()],
        n: 100,
        descending: true,
    };
    sweep_unary(
        &mut results,
        "table.top_n",
        true,
        "n=100 desc su num",
        |batch| top_n(batch, &top_n_config).expect("top_n"),
    );

    let fingerprint_config = StableFingerprint {
        columns: Vec::new(), // tutte le colonne, ordine di schema
        output_column: "fingerprint".into(),
        algorithm: FingerprintAlgorithm::Sha256,
    };
    sweep_unary(
        &mut results,
        "table.stable_fingerprint",
        true,
        "sha256 su tutte e 6 le colonne",
        |batch| stable_fingerprint(batch, &fingerprint_config).expect("stable_fingerprint"),
    );

    let align = align_config();
    sweep_wide(
        &mut results,
        "table.align_schema",
        true,
        "20 colonne: 18 permutate + 1 default + 1 null, 2 scartate",
        |batch| align_schema(batch, &align).expect("align_schema"),
    );

    let concat_by_name_config = ConcatByName { strict: false };
    let third = base_1m().num_rows() / 3;
    let cbn_a = base_1m().slice(0, third);
    // Input 2: stesse colonne in ordine permutato.
    let cbn_b = select_columns(
        &base_1m().slice(third, third),
        &SelectColumns {
            columns: vec![
                "path".into(),
                "key".into(),
                "text".into(),
                "grp".into(),
                "num".into(),
                "id".into(),
            ],
        },
    )
    .expect("cbn input 2");
    // Input 3: senza la colonna `path` (contribuisce con null).
    let cbn_c = select_columns(
        &base_1m().slice(2 * third, base_1m().num_rows() - 2 * third),
        &SelectColumns {
            columns: vec![
                "num".into(),
                "id".into(),
                "grp".into(),
                "text".into(),
                "key".into(),
            ],
        },
    )
    .expect("cbn input 3");
    results.push(measure_record(
        "table.concat_by_name",
        M1,
        3,
        "3 input ~333k righe, schemi permutati, 1 colonna assente nel terzo",
        || {
            concat_by_name(&[&cbn_a, &cbn_b, &cbn_c], &concat_by_name_config, &limits)
                .expect("concat_by_name")
        },
    ));

    let rules_config = ValidateRules {
        rules: vec![
            ValidateRule {
                name: "num_range".into(),
                operator: RuleOperator::Range,
                column: Some("num".into()),
                value: Some(json!("0,10000")),
                severity: RuleSeverity::Error,
            },
            ValidateRule {
                name: "text_hex".into(),
                operator: RuleOperator::Regex,
                column: Some("text".into()),
                value: Some(json!("^[0-9a-f]{40}$")),
                severity: RuleSeverity::Error,
            },
            ValidateRule {
                name: "id_notnull".into(),
                operator: RuleOperator::Notnull,
                column: Some("id".into()),
                value: None,
                severity: RuleSeverity::Error,
            },
            ValidateRule {
                name: "key_lt".into(),
                operator: RuleOperator::Lt,
                column: Some("key".into()),
                value: Some(json!(1_000_000)),
                severity: RuleSeverity::Warning,
            },
            ValidateRule {
                name: "grp_ne".into(),
                operator: RuleOperator::Ne,
                column: Some("grp".into()),
                value: Some(json!("g999999")),
                severity: RuleSeverity::Warning,
            },
        ],
        output_mode: ValidateOutputMode::Annotate,
    };
    sweep_unary(
        &mut results,
        "table.validate_rules",
        true,
        "5 regole (range/regex/notnull/lt/ne), annotate",
        |batch| validate_rules(batch, &rules_config).expect("validate_rules"),
    );

    let hmac_config = HmacSha256 {
        columns: vec!["id".into(), "text".into()],
        key_env: HMAC_KEY_ENV.into(),
        output_column: "hmac".into(),
        null_policy: HmacNullPolicy::Empty,
    };
    sweep_unary(
        &mut results,
        "table.hmac_sha256",
        true,
        "2 colonne (id+text), chiave da env",
        |batch| hmac_sha256(batch, &hmac_config).expect("hmac_sha256"),
    );

    // fuzzy_join: anagrafica sintetica (fixture di bench_fuzzy_join), scala
    // fissa 1M x 10k: escalation disattivata perche' il costo e' ~lineare
    // nelle righe sinistre ma con costante alta (score per candidato).
    let fuzzy_right_rows = 10_000;
    let clean: Vec<String> = (0..fuzzy_right_rows)
        .map(|row| name_of(u64::try_from(row).unwrap_or(0)))
        .collect();
    let mut lcg = Lcg::seeded();
    let dirty: Vec<String> = (0..M1)
        .map(|row| {
            // Bound evidente: below(n) < n e n = fuzzy_right_rows deriva da
            // usize, quindi il risultato rientra in usize.
            #[allow(clippy::cast_possible_truncation)]
            let source = &clean[lcg.below(fuzzy_right_rows as u64) as usize];
            if lcg.below(4) == 0 {
                source.clone() // ~25% gia' puliti
            } else {
                typo(source, u64::try_from(row).unwrap_or(0))
            }
        })
        .collect();
    let fuzzy_left = anagrafica(M1, &dirty, 0);
    let fuzzy_right = anagrafica(fuzzy_right_rows, &clean, 1_000_000);
    let fuzzy_config = FuzzyJoin {
        left_key: "name".into(),
        right_key: "name".into(),
        metric: FuzzyMetric::JaroWinkler,
        threshold: 0.85,
        blocking: FuzzyBlocking::Prefix,
        blocking_param: None, // default 2
        how: FuzzyHow::Inner,
        score_column: None,
        max_candidates: Some(2_000),
        case_sensitive: false,
    };
    let fuzzy_limits = Limits {
        max_rows: 1_000_000_000,
        ..Limits::default()
    };
    results.push(measure_record(
        "table.fuzzy_join",
        M1,
        3,
        "1M x 10k anagrafica, jaro_winkler prefix(2), soglia 0.85",
        || fuzzy_join(&fuzzy_left, &fuzzy_right, &fuzzy_config, &fuzzy_limits).expect("fuzzy_join"),
    ));

    write_outputs(&results);
    eprintln!(
        "sweep2 completato: {} misure -> benchmarks/sweep/",
        results.len()
    );
}
