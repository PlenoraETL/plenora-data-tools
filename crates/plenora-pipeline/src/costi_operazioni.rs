//! Modello di costo delle operazioni tabellari: GENERATO da
//! `scripts/genera_costi_operazioni.py`, non si modifica a mano.
//!
//! Fonte: `data/misure/catalogo-memoria-tabellare-v3.json` (campagna
//! Windows, `PeakWorkingSet64`; SHA-256 in [`SHA256_CATALOGO`]).
//!
//! Per operazione e variante:
//!
//! ```text
//! picco = S * (a + max(r * righe_in, c * byte_in, p * righe_sx * righe_dx))
//! ```
//!
//! - `y_i = max(budget_estimate_bytes_i, output_new_buffer_bytes_i, 0)`
//!   per ogni punto osservato; il pavimento a zero assorbe i punti
//!   rumorosi o inconcludenti;
//! - `a` = `y` al campione piu' piccolo; `r`, `c`, `p` = inviluppo
//!   superiore `max_i (y_i - a) / unita'_i` (righe in ingresso, byte in
//!   ingresso deduplicati per allocazione, coppie di righe), in millesimi
//!   di byte arrotondati per eccesso;
//! - massimo componente per componente su tutti i profili misurati della
//!   variante (il peggiore, compreso l'avversario `distinct`);
//! - superlineari (`cross_join`, `fuzzy_join`): solo il termine a coppie;
//! - output sottoinsieme delle righe: `c` almeno 1 (copia intera);
//! - `S` = [`FATTORE_SICUREZZA`], per eccesso.
//!
//! Le varianti spilled sono misurate con `max_governed_memory_bytes` pari
//! a [`BUDGET_SPILL_MISURATO`]: il runner non passa ai kernel spilled un
//! margine piu' grande.
//!
//! Crescita per riga oltre la soglia in ogni profilo, fra gli ultimi
//! due campioni, per operazioni lineari nel modello (inviluppo sul
//! campione piu' grande; oltre il dominio misurato, estrapolazione):
//!
//! - `table.date_extract` wide: 1.53
//! - `table.string_length` wide: 2.34

use crate::budget::{Costo, CostoOperazione};

/// SHA-256 del catalogo da cui il modello e' generato.
pub const SHA256_CATALOGO: &str =
    "76acae0f4ff40779e1eb8459f79737457904d54d3200e6b27570abc196d1ca10";

/// Fattore di sicurezza `S` come frazione (numeratore, denominatore).
pub const FATTORE_SICUREZZA: (u64, u64) = (3, 2);

/// `max_governed_memory_bytes` dei profili spilled misurati.
pub const BUDGET_SPILL_MISURATO: u64 = 67_108_864;

/// Un modello per operazione misurata, in ordine di id.
pub static COSTI: &[CostoOperazione] = &[
    CostoOperazione {
        op: "table.add_row_number",
        in_memoria: Costo {
            a: 397_312,
            r_millesimi: 24_212,
            c_millesimi: 1_444,
            p_millesimi: 0,
        },
        spill: None,
        dipende_dai_dati: false,
        profili: &["narrow", "wide"],
    },
    CostoOperazione {
        op: "table.aggregate",
        in_memoria: Costo {
            a: 1_949_696,
            r_millesimi: 163_900,
            c_millesimi: 1_147,
            p_millesimi: 0,
        },
        spill: Some(Costo {
            a: 2_846_720,
            r_millesimi: 139_707,
            c_millesimi: 957,
            p_millesimi: 0,
        }),
        dipende_dai_dati: true,
        profili: &["distinct", "spilled_distinct", "spilled_wide", "wide"],
    },
    CostoOperazione {
        op: "table.align_schema",
        in_memoria: Costo {
            a: 507_904,
            r_millesimi: 40_616,
            c_millesimi: 2_421,
            p_millesimi: 0,
        },
        spill: None,
        dipende_dai_dati: false,
        profili: &["narrow", "wide"],
    },
    CostoOperazione {
        op: "table.anti_join",
        in_memoria: Costo {
            a: 946_176,
            r_millesimi: 39_353,
            c_millesimi: 2_346,
            p_millesimi: 0,
        },
        spill: None,
        dipende_dai_dati: true,
        profili: &["distinct", "narrow", "wide"],
    },
    CostoOperazione {
        op: "table.asof_join",
        in_memoria: Costo {
            a: 1_544_192,
            r_millesimi: 60_477,
            c_millesimi: 1_751,
            p_millesimi: 0,
        },
        spill: None,
        dipende_dai_dati: true,
        profili: &["narrow", "wide"],
    },
    CostoOperazione {
        op: "table.assert_cardinality",
        in_memoria: Costo {
            a: 4_096,
            r_millesimi: 4,
            c_millesimi: 1,
            p_millesimi: 0,
        },
        spill: None,
        dipende_dai_dati: false,
        profili: &["narrow", "wide"],
    },
    CostoOperazione {
        op: "table.assert_foreign_key",
        in_memoria: Costo {
            a: 1_048_576,
            r_millesimi: 57_623,
            c_millesimi: 3_435,
            p_millesimi: 0,
        },
        spill: None,
        dipende_dai_dati: false,
        profili: &["distinct", "narrow", "wide"],
    },
    CostoOperazione {
        op: "table.assert_metadata",
        in_memoria: Costo {
            a: 12_288,
            r_millesimi: 5_325,
            c_millesimi: 68,
            p_millesimi: 0,
        },
        spill: None,
        dipende_dai_dati: false,
        profili: &["narrow", "wide"],
    },
    CostoOperazione {
        op: "table.assert_not_null",
        in_memoria: Costo {
            a: 24_576,
            r_millesimi: 5_858,
            c_millesimi: 75,
            p_millesimi: 0,
        },
        spill: None,
        dipende_dai_dati: false,
        profili: &["narrow", "wide"],
    },
    CostoOperazione {
        op: "table.assert_range",
        in_memoria: Costo {
            a: 36_864,
            r_millesimi: 0,
            c_millesimi: 0,
            p_millesimi: 0,
        },
        spill: None,
        dipende_dai_dati: false,
        profili: &["narrow", "wide"],
    },
    CostoOperazione {
        op: "table.assert_regex",
        in_memoria: Costo {
            a: 475_136,
            r_millesimi: 5,
            c_millesimi: 1,
            p_millesimi: 0,
        },
        spill: None,
        dipende_dai_dati: false,
        profili: &["wide"],
    },
    CostoOperazione {
        op: "table.assert_schema",
        in_memoria: Costo {
            a: 28_672,
            r_millesimi: 25,
            c_millesimi: 1,
            p_millesimi: 0,
        },
        spill: None,
        dipende_dai_dati: false,
        profili: &["narrow", "wide"],
    },
    CostoOperazione {
        op: "table.assert_unique",
        in_memoria: Costo {
            a: 1_789_952,
            r_millesimi: 197_616,
            c_millesimi: 3_618,
            p_millesimi: 0,
        },
        spill: None,
        dipende_dai_dati: false,
        profili: &["distinct", "narrow", "wide"],
    },
    CostoOperazione {
        op: "table.bin",
        in_memoria: Costo {
            a: 1_286_144,
            r_millesimi: 119_617,
            c_millesimi: 7_088,
            p_millesimi: 0,
        },
        spill: None,
        dipende_dai_dati: false,
        profili: &["narrow", "wide"],
    },
    CostoOperazione {
        op: "table.coalesce",
        in_memoria: Costo {
            a: 172_032,
            r_millesimi: 5_016,
            c_millesimi: 57,
            p_millesimi: 0,
        },
        spill: None,
        dipende_dai_dati: false,
        profili: &["wide"],
    },
    CostoOperazione {
        op: "table.concat",
        in_memoria: Costo {
            a: 1_564_672,
            r_millesimi: 68_449,
            c_millesimi: 1_889,
            p_millesimi: 0,
        },
        spill: None,
        dipende_dai_dati: false,
        profili: &["narrow", "wide"],
    },
    CostoOperazione {
        op: "table.concat_by_name",
        in_memoria: Costo {
            a: 1_581_056,
            r_millesimi: 68_449,
            c_millesimi: 1_889,
            p_millesimi: 0,
        },
        spill: None,
        dipende_dai_dati: false,
        profili: &["narrow", "wide"],
    },
    CostoOperazione {
        op: "table.concat_columns",
        in_memoria: Costo {
            a: 765_952,
            r_millesimi: 55_909,
            c_millesimi: 585,
            p_millesimi: 0,
        },
        spill: None,
        dipende_dai_dati: false,
        profili: &["wide"],
    },
    CostoOperazione {
        op: "table.conditional",
        in_memoria: Costo {
            a: 688_128,
            r_millesimi: 48_366,
            c_millesimi: 2_358,
            p_millesimi: 0,
        },
        spill: None,
        dipende_dai_dati: false,
        profili: &["narrow", "wide"],
    },
    CostoOperazione {
        op: "table.cross_join",
        in_memoria: Costo {
            a: 417_792,
            r_millesimi: 0,
            c_millesimi: 0,
            p_millesimi: 133_120,
        },
        spill: None,
        dipende_dai_dati: true,
        profili: &["narrow", "wide"],
    },
    CostoOperazione {
        op: "table.date_add",
        in_memoria: Costo {
            a: 1_081_344,
            r_millesimi: 58_480,
            c_millesimi: 614,
            p_millesimi: 0,
        },
        spill: None,
        dipende_dai_dati: false,
        profili: &["wide"],
    },
    CostoOperazione {
        op: "table.date_diff",
        in_memoria: Costo {
            a: 229_376,
            r_millesimi: 15_926,
            c_millesimi: 167,
            p_millesimi: 0,
        },
        spill: None,
        dipende_dai_dati: false,
        profili: &["wide"],
    },
    CostoOperazione {
        op: "table.date_extract",
        in_memoria: Costo {
            a: 503_808,
            r_millesimi: 31_483,
            c_millesimi: 330,
            p_millesimi: 0,
        },
        spill: None,
        dipende_dai_dati: false,
        profili: &["wide"],
    },
    CostoOperazione {
        op: "table.date_format",
        in_memoria: Costo {
            a: 1_081_344,
            r_millesimi: 56_983,
            c_millesimi: 601,
            p_millesimi: 0,
        },
        spill: None,
        dipende_dai_dati: false,
        profili: &["wide"],
    },
    CostoOperazione {
        op: "table.dedup_advanced",
        in_memoria: Costo {
            a: 2_154_496,
            r_millesimi: 214_307,
            c_millesimi: 4_435,
            p_millesimi: 0,
        },
        spill: None,
        dipende_dai_dati: true,
        profili: &["distinct", "narrow", "wide"],
    },
    CostoOperazione {
        op: "table.distinct",
        in_memoria: Costo {
            a: 2_011_136,
            r_millesimi: 214_344,
            c_millesimi: 4_460,
            p_millesimi: 0,
        },
        spill: Some(Costo {
            a: 3_993_600,
            r_millesimi: 257_024,
            c_millesimi: 1_880,
            p_millesimi: 0,
        }),
        dipende_dai_dati: false,
        profili: &[
            "distinct",
            "narrow",
            "spilled_distinct",
            "spilled_wide",
            "wide",
        ],
    },
    CostoOperazione {
        op: "table.drop_columns",
        in_memoria: Costo {
            a: 53_248,
            r_millesimi: 5_284,
            c_millesimi: 67,
            p_millesimi: 0,
        },
        spill: None,
        dipende_dai_dati: false,
        profili: &["narrow", "wide"],
    },
    CostoOperazione {
        op: "table.except",
        in_memoria: Costo {
            a: 4_763_648,
            r_millesimi: 238_364,
            c_millesimi: 4_284,
            p_millesimi: 0,
        },
        spill: Some(Costo {
            a: 1_978_368,
            r_millesimi: 62_496,
            c_millesimi: 1_000,
            p_millesimi: 0,
        }),
        dipende_dai_dati: false,
        profili: &[
            "distinct",
            "narrow",
            "spilled_distinct",
            "spilled_wide",
            "wide",
        ],
    },
    CostoOperazione {
        op: "table.explode",
        in_memoria: Costo {
            a: 1_339_392,
            r_millesimi: 81_761,
            c_millesimi: 1_919,
            p_millesimi: 0,
        },
        spill: None,
        dipende_dai_dati: true,
        profili: &["wide"],
    },
    CostoOperazione {
        op: "table.expression",
        in_memoria: Costo {
            a: 946_176,
            r_millesimi: 71_767,
            c_millesimi: 4_278,
            p_millesimi: 0,
        },
        spill: None,
        dipende_dai_dati: false,
        profili: &["narrow", "wide"],
    },
    CostoOperazione {
        op: "table.fill_na",
        in_memoria: Costo {
            a: 163_840,
            r_millesimi: 5_590,
            c_millesimi: 64,
            p_millesimi: 0,
        },
        spill: None,
        dipende_dai_dati: false,
        profili: &["wide"],
    },
    CostoOperazione {
        op: "table.filter",
        in_memoria: Costo {
            a: 692_224,
            r_millesimi: 37_379,
            c_millesimi: 1_000,
            p_millesimi: 0,
        },
        spill: None,
        dipende_dai_dati: false,
        profili: &["narrow", "wide"],
    },
    CostoOperazione {
        op: "table.flatten_json",
        in_memoria: Costo {
            a: 774_144,
            r_millesimi: 54_575,
            c_millesimi: 571,
            p_millesimi: 0,
        },
        spill: None,
        dipende_dai_dati: true,
        profili: &["wide"],
    },
    CostoOperazione {
        op: "table.formula",
        in_memoria: Costo {
            a: 249_856,
            r_millesimi: 23_618,
            c_millesimi: 1_408,
            p_millesimi: 0,
        },
        spill: None,
        dipende_dai_dati: false,
        profili: &["narrow", "wide"],
    },
    CostoOperazione {
        op: "table.fuzzy_join",
        in_memoria: Costo {
            a: 454_656,
            r_millesimi: 0,
            c_millesimi: 0,
            p_millesimi: 395_947,
        },
        spill: None,
        dipende_dai_dati: true,
        profili: &["distinct", "wide"],
    },
    CostoOperazione {
        op: "table.hmac_sha256",
        in_memoria: Costo {
            a: 884_736,
            r_millesimi: 67_865,
            c_millesimi: 774,
            p_millesimi: 0,
        },
        spill: None,
        dipende_dai_dati: false,
        profili: &["wide"],
    },
    CostoOperazione {
        op: "table.intersect",
        in_memoria: Costo {
            a: 2_945_024,
            r_millesimi: 144_357,
            c_millesimi: 5_867,
            p_millesimi: 0,
        },
        spill: Some(Costo {
            a: 1_982_464,
            r_millesimi: 37_930,
            c_millesimi: 1_000,
            p_millesimi: 0,
        }),
        dipende_dai_dati: false,
        profili: &[
            "distinct",
            "narrow",
            "spilled_distinct",
            "spilled_wide",
            "wide",
        ],
    },
    CostoOperazione {
        op: "table.join",
        in_memoria: Costo {
            a: 1_908_736,
            r_millesimi: 98_057,
            c_millesimi: 5_845,
            p_millesimi: 0,
        },
        spill: None,
        dipende_dai_dati: true,
        profili: &["distinct", "narrow", "wide"],
    },
    CostoOperazione {
        op: "table.limit",
        in_memoria: Costo {
            a: 12_288,
            r_millesimi: 6_104,
            c_millesimi: 78,
            p_millesimi: 0,
        },
        spill: None,
        dipende_dai_dati: false,
        profili: &["narrow", "wide"],
    },
    CostoOperazione {
        op: "table.lookup",
        in_memoria: Costo {
            a: 622_592,
            r_millesimi: 40_280,
            c_millesimi: 422,
            p_millesimi: 0,
        },
        spill: None,
        dipende_dai_dati: false,
        profili: &["wide"],
    },
    CostoOperazione {
        op: "table.mask_data",
        in_memoria: Costo {
            a: 188_416,
            r_millesimi: 11_963,
            c_millesimi: 134,
            p_millesimi: 0,
        },
        spill: None,
        dipende_dai_dati: false,
        profili: &["wide"],
    },
    CostoOperazione {
        op: "table.md5_hash",
        in_memoria: Costo {
            a: 1_277_952,
            r_millesimi: 77_734,
            c_millesimi: 813,
            p_millesimi: 0,
        },
        spill: None,
        dipende_dai_dati: false,
        profili: &["wide"],
    },
    CostoOperazione {
        op: "table.melt",
        in_memoria: Costo {
            a: 598_016,
            r_millesimi: 40_281,
            c_millesimi: 2_401,
            p_millesimi: 0,
        },
        spill: None,
        dipende_dai_dati: true,
        profili: &["narrow", "wide"],
    },
    CostoOperazione {
        op: "table.pivot",
        in_memoria: Costo {
            a: 8_179_712,
            r_millesimi: 745_524,
            c_millesimi: 4_850,
            p_millesimi: 0,
        },
        spill: None,
        dipende_dai_dati: true,
        profili: &["distinct", "wide"],
    },
    CostoOperazione {
        op: "table.reconcile",
        in_memoria: Costo {
            a: 2_830_336,
            r_millesimi: 146_484,
            c_millesimi: 8_732,
            p_millesimi: 0,
        },
        spill: None,
        dipende_dai_dati: false,
        profili: &["distinct", "narrow", "wide"],
    },
    CostoOperazione {
        op: "table.rename",
        in_memoria: Costo {
            a: 77_824,
            r_millesimi: 5,
            c_millesimi: 1,
            p_millesimi: 0,
        },
        spill: None,
        dipende_dai_dati: false,
        profili: &["narrow", "wide"],
    },
    CostoOperazione {
        op: "table.reorder_columns",
        in_memoria: Costo {
            a: 49_152,
            r_millesimi: 0,
            c_millesimi: 0,
            p_millesimi: 0,
        },
        spill: None,
        dipende_dai_dati: false,
        profili: &["wide"],
    },
    CostoOperazione {
        op: "table.replace",
        in_memoria: Costo {
            a: 147_456,
            r_millesimi: 5_759,
            c_millesimi: 66,
            p_millesimi: 0,
        },
        spill: None,
        dipende_dai_dati: false,
        profili: &["wide"],
    },
    CostoOperazione {
        op: "table.rolling_window",
        in_memoria: Costo {
            a: 3_088_384,
            r_millesimi: 278_610,
            c_millesimi: 3_335,
            p_millesimi: 0,
        },
        spill: None,
        dipende_dai_dati: true,
        profili: &["distinct", "narrow", "wide"],
    },
    CostoOperazione {
        op: "table.sample",
        in_memoria: Costo {
            a: 229_376,
            r_millesimi: 8_111,
            c_millesimi: 465,
            p_millesimi: 0,
        },
        spill: None,
        dipende_dai_dati: false,
        profili: &["narrow", "wide"],
    },
    CostoOperazione {
        op: "table.select_columns",
        in_memoria: Costo {
            a: 53_248,
            r_millesimi: 5_530,
            c_millesimi: 70,
            p_millesimi: 0,
        },
        spill: None,
        dipende_dai_dati: false,
        profili: &["narrow", "wide"],
    },
    CostoOperazione {
        op: "table.semi_join",
        in_memoria: Costo {
            a: 1_822_720,
            r_millesimi: 88_093,
            c_millesimi: 2_423,
            p_millesimi: 0,
        },
        spill: None,
        dipende_dai_dati: true,
        profili: &["distinct", "narrow", "wide"],
    },
    CostoOperazione {
        op: "table.sha256_hash",
        in_memoria: Costo {
            a: 2_170_880,
            r_millesimi: 132_671,
            c_millesimi: 1_388,
            p_millesimi: 0,
        },
        spill: None,
        dipende_dai_dati: false,
        profili: &["wide"],
    },
    CostoOperazione {
        op: "table.sort",
        in_memoria: Costo {
            a: 1_175_552,
            r_millesimi: 124_990,
            c_millesimi: 1_789,
            p_millesimi: 0,
        },
        spill: Some(Costo {
            a: 1_699_840,
            r_millesimi: 124_944,
            c_millesimi: 1_000,
            p_millesimi: 0,
        }),
        dipende_dai_dati: false,
        profili: &[
            "distinct",
            "narrow",
            "spilled_distinct",
            "spilled_wide",
            "wide",
        ],
    },
    CostoOperazione {
        op: "table.split_column",
        in_memoria: Costo {
            a: 1_609_728,
            r_millesimi: 127_682,
            c_millesimi: 1_404,
            p_millesimi: 0,
        },
        spill: None,
        dipende_dai_dati: false,
        profili: &["wide"],
    },
    CostoOperazione {
        op: "table.stable_fingerprint",
        in_memoria: Costo {
            a: 868_352,
            r_millesimi: 67_865,
            c_millesimi: 774,
            p_millesimi: 0,
        },
        spill: None,
        dipende_dai_dati: false,
        profili: &["wide"],
    },
    CostoOperazione {
        op: "table.statistics",
        in_memoria: Costo {
            a: 1_904_640,
            r_millesimi: 200_020,
            c_millesimi: 1_228,
            p_millesimi: 0,
        },
        spill: None,
        dipende_dai_dati: false,
        profili: &["distinct", "wide"],
    },
    CostoOperazione {
        op: "table.string_extract",
        in_memoria: Costo {
            a: 708_608,
            r_millesimi: 4_990,
            c_millesimi: 57,
            p_millesimi: 0,
        },
        spill: None,
        dipende_dai_dati: false,
        profili: &["wide"],
    },
    CostoOperazione {
        op: "table.string_length",
        in_memoria: Costo {
            a: 249_856,
            r_millesimi: 18_854,
            c_millesimi: 198,
            p_millesimi: 0,
        },
        spill: None,
        dipende_dai_dati: false,
        profili: &["wide"],
    },
    CostoOperazione {
        op: "table.string_pad",
        in_memoria: Costo {
            a: 577_536,
            r_millesimi: 39_814,
            c_millesimi: 432,
            p_millesimi: 0,
        },
        spill: None,
        dipende_dai_dati: false,
        profili: &["wide"],
    },
    CostoOperazione {
        op: "table.table_diff",
        in_memoria: Costo {
            a: 4_939_776,
            r_millesimi: 223_804,
            c_millesimi: 4_508,
            p_millesimi: 0,
        },
        spill: None,
        dipende_dai_dati: false,
        profili: &["distinct", "narrow", "wide"],
    },
    CostoOperazione {
        op: "table.text_normalize",
        in_memoria: Costo {
            a: 389_120,
            r_millesimi: 40_109,
            c_millesimi: 441,
            p_millesimi: 0,
        },
        spill: None,
        dipende_dai_dati: false,
        profili: &["wide"],
    },
    CostoOperazione {
        op: "table.timezone_convert",
        in_memoria: Costo {
            a: 1_118_208,
            r_millesimi: 57_322,
            c_millesimi: 613,
            p_millesimi: 0,
        },
        spill: None,
        dipende_dai_dati: false,
        profili: &["wide"],
    },
    CostoOperazione {
        op: "table.top_n",
        in_memoria: Costo {
            a: 299_008,
            r_millesimi: 8_152,
            c_millesimi: 1_000,
            p_millesimi: 0,
        },
        spill: None,
        dipende_dai_dati: false,
        profili: &["narrow", "wide"],
    },
    CostoOperazione {
        op: "table.transpose",
        in_memoria: Costo {
            a: 221_184,
            r_millesimi: 1_712_128,
            c_millesimi: 19_747,
            p_millesimi: 0,
        },
        spill: None,
        dipende_dai_dati: true,
        profili: &["wide"],
    },
    CostoOperazione {
        op: "table.type_cast",
        in_memoria: Costo {
            a: 212_992,
            r_millesimi: 11_964,
            c_millesimi: 705,
            p_millesimi: 0,
        },
        spill: None,
        dipende_dai_dati: false,
        profili: &["narrow", "wide"],
    },
    CostoOperazione {
        op: "table.union_distinct",
        in_memoria: Costo {
            a: 8_589_312,
            r_millesimi: 428_258,
            c_millesimi: 8_623,
            p_millesimi: 0,
        },
        spill: Some(Costo {
            a: 5_214_208,
            r_millesimi: 243_175,
            c_millesimi: 1_622,
            p_millesimi: 0,
        }),
        dipende_dai_dati: false,
        profili: &[
            "distinct",
            "narrow",
            "spilled_distinct",
            "spilled_wide",
            "wide",
        ],
    },
    CostoOperazione {
        op: "table.unnest",
        in_memoria: Costo {
            a: 233_472,
            r_millesimi: 16_745,
            c_millesimi: 393,
            p_millesimi: 0,
        },
        spill: None,
        dipende_dai_dati: true,
        profili: &["wide"],
    },
    CostoOperazione {
        op: "table.uuid_generator",
        in_memoria: Costo {
            a: 1_232_896,
            r_millesimi: 97_285,
            c_millesimi: 5_799,
            p_millesimi: 0,
        },
        spill: None,
        dipende_dai_dati: false,
        profili: &["narrow", "wide"],
    },
    CostoOperazione {
        op: "table.validate_rules",
        in_memoria: Costo {
            a: 929_792,
            r_millesimi: 80_180,
            c_millesimi: 4_780,
            p_millesimi: 0,
        },
        spill: None,
        dipende_dai_dati: false,
        profili: &["narrow", "wide"],
    },
    CostoOperazione {
        op: "table.window_function",
        in_memoria: Costo {
            a: 3_084_288,
            r_millesimi: 280_044,
            c_millesimi: 3_350,
            p_millesimi: 0,
        },
        spill: None,
        dipende_dai_dati: true,
        profili: &["distinct", "narrow", "wide"],
    },
];
