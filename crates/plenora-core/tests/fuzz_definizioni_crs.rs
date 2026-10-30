//! Lo stesso oracolo del fuzzer gira anche sulla toolchain stabile.

#![allow(clippy::unwrap_used, clippy::expect_used)]

#[path = "../../../fuzz/fuzz_targets/comune/aggancio.rs"]
#[allow(dead_code)]
mod aggancio;
#[path = "../../../fuzz/fuzz_targets/comune/crs.rs"]
mod crs;
#[path = "../../../fuzz/fuzz_targets/comune/esiti.rs"]
mod esiti;

use plenora_core::crs::resolve_crs;
use serde_json::Value;

#[test]
fn corpus_reale_e_troncamenti_con_oracolo_fuzz() {
    let proj: Value = serde_json::from_str(include_str!(
        "../../plenora-io/data/projjson_integrati.json"
    ))
    .unwrap();
    let wkt: Value =
        serde_json::from_str(include_str!("../src/crs/definizioni/wkt_corpus.json")).unwrap();
    assert_eq!(proj.as_object().unwrap().len(), 169);
    assert_eq!(wkt.as_object().unwrap().len(), 169);
    for (id, documento) in proj.as_object().unwrap() {
        let json = documento.to_string();
        assert!(resolve_crs(&json, "crs").is_ok());
        crs::verifica(&json);
        crs::verifica(&format!(
            "{{\"name\":\"{}\",{}",
            esiti::SENTINELLA,
            &json[1..]
        ));
        for (forma, motivo) in [
            ("wkt2_2019", "esclusione_2019"),
            ("wkt2_2015", "esclusione_2015"),
            ("wkt1_assi", "esclusione_wkt1"),
            ("wkt1_gdal", "esclusione_wkt1_gdal"),
        ] {
            let testo = wkt[id][forma].as_str().unwrap();
            assert_eq!(resolve_crs(testo, "crs").is_ok(), wkt[id][motivo].is_null());
            crs::verifica(testo);
            for taglio in [0, 1, testo.len() / 2, testo.len() - 1] {
                if testo.is_char_boundary(taglio) {
                    crs::verifica(&testo[..taglio]);
                }
            }
            crs::verifica(&format!("{testo}{}", esiti::SENTINELLA));
        }
    }
}

#[test]
fn limiti_strutturali_e_testuali_sono_rifiuti_espliciti() {
    let casi = [
        format!("{}0{}", "{\"x\":".repeat(33), "}".repeat(33)),
        format!("{}0{}", "GEOGCRS[".repeat(33), "]".repeat(33)),
        format!("{{\"x\":[{}0]}}", "0,".repeat(8193)),
        format!("GEOGCRS[{}0]", "0,".repeat(8193)),
        format!("{{\"name\":\"{}\"}}", "x".repeat(65537)),
        format!("GEOGCRS[\"{}\"]", "x".repeat(65537)),
        "EPSG:4326\0".into(),
    ];
    for testo in casi {
        assert_eq!(
            resolve_crs(&testo, "crs").unwrap_err().code().as_str(),
            "CRS_INVALID"
        );
        crs::verifica(&testo);
    }
}
