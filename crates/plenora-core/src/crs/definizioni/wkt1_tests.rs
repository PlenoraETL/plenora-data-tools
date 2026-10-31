//! Corpus PROJ con provenienza, esclusioni e mutazioni WKT1 indipendenti.

use crate::crs::{builtin_crs_identifiers, resolve_crs};

fn corpus() -> serde_json::Value {
    serde_json::from_str(include_str!("wkt_corpus.json")).unwrap()
}

fn wkt(id: &str) -> String {
    corpus()[id]["wkt1_assi"].as_str().unwrap().into()
}

fn rifiuto(t: &str, codice: &str) {
    let e = resolve_crs(t, "crs").unwrap_err();
    assert_eq!(e.code().as_str(), codice);
    assert!(!e.to_string().contains("SENTINELLA"));
}

#[test]
fn corpus_completo_con_esclusioni_esplicite() {
    let corpus = corpus();
    assert_eq!(corpus.as_object().unwrap().len(), 169);
    let mut accettati = 0;
    for id in builtin_crs_identifiers() {
        for (campo, motivo) in [
            ("wkt1_assi", "esclusione_wkt1"),
            ("wkt1_gdal", "esclusione_wkt1_gdal"),
        ] {
            let voce = &corpus[&id];
            let t = voce[campo].as_str().unwrap();
            if !voce[motivo].is_null() {
                assert!(!voce[motivo].as_str().unwrap().is_empty());
                rifiuto(t, "CRS_NOT_BUILTIN");
                continue;
            }
            {
                let a = resolve_crs(t, "crs").unwrap_or_else(|e| panic!("{id}: {e}"));
                let b = resolve_crs(&id, "crs").unwrap();
                assert!(a.semantically_equals(&b));
                assert_eq!(a.definition(), t);
                assert_eq!(a.authority_axis_order(), b.authority_axis_order());
                assert_eq!(a.validity_domain(), b.validity_domain());
                assert_eq!(a.area_of_use(), b.area_of_use());
                if campo == "wkt1_assi" {
                    accettati += 1;
                }
            }
        }
    }
    assert_eq!(accettati, 32);
}

#[test]
fn assi_id_unita_e_struttura_non_si_inferiscono() {
    let t = wkt("EPSG:7791");
    for (da, a) in [
        ("6378137", "6378138"),
        ("298.257222101", "298.257222102"),
        ("PRIMEM[\"Greenwich\",0", "PRIMEM[\"Greenwich\",1"),
        ("\"1132\"", "\"6326\""),
        ("\"7019\"", "\"7030\""),
        ("\"8901\"", "\"8902\""),
        ("\"6706\"", "\"4326\""),
        ("\"7791\"", "\"7792\""),
        (",AUTHORITY[\"EPSG\",\"7791\"]", ""),
        ("Transverse_Mercator", "SENTINELLA"),
        ("0.0174532925199433", "0.01745329251994331"),
        ("UNIT[\"metre\",1", "UNIT[\"metre\",2"),
        ("\"9122\"", "\"9001\""),
        ("\"9001\"", "\"9002\""),
        (",AXIS[\"Latitude\",NORTH]", ""),
        (",AXIS[\"Longitude\",EAST]", ""),
        (",AXIS[\"Easting\",EAST]", ""),
        (",AXIS[\"Northing\",NORTH]", ""),
        ("AXIS[\"Latitude\",NORTH]", "AXIS[\"Latitude\",SOUTH]"),
        ("AXIS[\"Easting\",EAST]", "AXIS[\"Easting\",NORTH]"),
        (
            "AXIS[\"Northing\",NORTH]",
            "AXIS[\"Northing\",NORTH],AXIS[\"z\",UP]",
        ),
        (
            "AUTHORITY[\"EPSG\",\"1132\"]",
            "TOWGS84[0,0,0],AUTHORITY[\"EPSG\",\"1132\"]",
        ),
        ("PROJECTION[", "EXTENSION[\"SENTINELLA\",\"x\"],PROJECTION["),
        (
            "PARAMETER[\"false_easting\",500000]",
            "PARAMETER[\"false_easting\",500000],PARAMETER[\"false_easting\",500000]",
        ),
        ("PARAMETER[\"false_easting\",500000],", ""),
        (
            "PARAMETER[\"false_easting\",500000]",
            "PARAMETER[\"SENTINELLA\",500000]",
        ),
        (
            "AUTHORITY[\"EPSG\",\"7791\"]",
            "AUTHORITY[\"EPSG\",\"7791\"],AUTHORITY[\"EPSG\",\"7791\"]",
        ),
        ("AUTHORITY[\"EPSG\",\"7791\"]", "ID[\"EPSG\",7791]"),
    ] {
        assert!(t.contains(da), "{da}");
        rifiuto(&t.replace(da, a), "CRS_NOT_BUILTIN");
    }
    // Anche aggiungendo tutti gli assi, i vecchi datum non sono ensemble.
    for id in [
        "EPSG:4326",
        "EPSG:4258",
        "OGC:CRS84",
        "EPSG:32632",
        "EPSG:25832",
    ] {
        rifiuto(&wkt(id), "CRS_NOT_BUILTIN");
    }
}

#[test]
fn ogni_parametro_wkt1_e_confrontato_esattamente() {
    for voce in corpus().as_object().unwrap().values() {
        if !voce["esclusione_wkt1"].is_null() {
            continue;
        }
        let t = voce["wkt1_assi"].as_str().unwrap();
        for (inizio, _) in t.match_indices("PARAMETER[\"") {
            let nome_fine = t[inizio + 11..].find('"').unwrap() + inizio + 11;
            let inizio_numero = nome_fine + 2;
            let fine = t[inizio_numero..].find(']').unwrap() + inizio_numero;
            let mut mutato = t.to_owned();
            mutato.replace_range(inizio_numero..fine, "123456789.123456789");
            rifiuto(&mutato, "CRS_NOT_BUILTIN");
        }
    }
}

#[test]
fn numeri_alias_e_errori_lessicali_wkt1() {
    let t = wkt("EPSG:7791");
    for valore in ["500000.0", "+5e5", "50000000e-2"] {
        resolve_crs(&t.replace("500000", valore), "crs").unwrap();
    }
    for valore in ["500000.00000000001", "1e99999", "1e-99999"] {
        rifiuto(&t.replace("500000", valore), "CRS_NOT_BUILTIN");
    }
    for valore in [
        "1e99999999999999999999",
        "1e",
        "+-1",
        "1.2.3",
        "NaN",
        "Infinity",
        "\"500000\"",
    ] {
        rifiuto(&t.replace("500000", valore), "CRS_INVALID");
    }
    for variante in [
        t.replace('[', "(").replace(']', ")"),
        t.replace("PROJCS", "projcs").replace("NORTH", "north"),
    ] {
        resolve_crs(&variante, "crs").unwrap();
    }
    for variante in [
        format!("{t} SENTINELLA"),
        t.replacen(']', ")", 1),
        t.replacen(',', ",,", 1),
    ] {
        rifiuto(&variante, "CRS_INVALID");
    }
    // Senza AUTHORITY annidate valgono solo i nomi canonici del registro.
    let t = wkt("EPSG:4265").replace(",AUTHORITY[\"EPSG\",\"6265\"]", "");
    rifiuto(&t, "CRS_NOT_BUILTIN");
    resolve_crs(&t.replace("Monte_Mario", "Monte Mario"), "crs").unwrap();
}

#[test]
fn authority_del_metodo_deve_concordare_con_alias_e_tabella() {
    for (id, alias, codice) in [
        ("EPSG:7791", "Transverse_Mercator", "9807"),
        ("EPSG:2154", "Lambert_Conformal_Conic_2SP", "9802"),
        (
            "EPSG:2056",
            "Hotine_Oblique_Mercator_Azimuth_Center",
            "9815",
        ),
        ("EPSG:28992", "Oblique_Stereographic", "9809"),
    ] {
        let t = wkt(id);
        let originale = format!("PROJECTION[\"{alias}\"]");
        let corretto = format!("PROJECTION[\"{alias}\",AUTHORITY[\"EPSG\",\"{codice}\"]]");
        resolve_crs(&t.replace(&originale, &corretto), "crs").unwrap();
        for errato in [
            corretto.replace(codice, "9999"),
            corretto.replace("EPSG", "SENTINELLA"),
        ] {
            rifiuto(&t.replace(&originale, &errato), "CRS_NOT_BUILTIN");
        }
    }
    rifiuto(
        &wkt("EPSG:7791").replace(
            "PROJECTION[\"Transverse_Mercator\"]",
            "PROJECTION[\"Oblique_Stereographic\",AUTHORITY[\"EPSG\",\"9807\"]]",
        ),
        "CRS_NOT_BUILTIN",
    );
}
