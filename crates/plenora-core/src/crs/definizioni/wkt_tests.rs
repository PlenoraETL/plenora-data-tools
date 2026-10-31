//! Corpus indipendente PROJ, mutazioni e limiti del profilo WKT2.

use crate::crs::{builtin_crs_identifiers, resolve_crs};

fn corpus() -> serde_json::Value {
    serde_json::from_str(include_str!("wkt_corpus.json")).unwrap()
}

fn wkt(id: &str) -> String {
    corpus()[id]["wkt2_2019"].as_str().unwrap().into()
}

fn rifiuto(testo: &str, codice: &str) {
    let e = resolve_crs(testo, "crs").unwrap_err();
    assert_eq!(e.code().as_str(), codice, "{testo}");
    assert!(!e.to_string().contains("SENTINELLA"));
}

#[test]
fn tutti_i_169_crs_e_2015_fedeli() {
    let corpus = corpus();
    assert_eq!(corpus.as_object().unwrap().len(), 169);
    for (campo, attesi) in [("esclusione_2019", 168), ("esclusione_2015", 32)] {
        assert_eq!(
            corpus
                .as_object()
                .unwrap()
                .values()
                .filter(|v| v[campo].is_null())
                .count(),
            attesi
        );
    }
    assert!(!corpus["EPSG:4267"]["esclusione_2019"].is_null());
    for id in builtin_crs_identifiers() {
        for versione in ["wkt2_2019", "wkt2_2015"] {
            let testo = corpus[&id][versione].as_str().unwrap();
            let esclusione = if versione == "wkt2_2015" {
                "esclusione_2015"
            } else {
                "esclusione_2019"
            };
            if !corpus[&id][esclusione].is_null() {
                rifiuto(testo, "CRS_NOT_BUILTIN");
                continue;
            }
            let d = resolve_crs(testo, "crs").unwrap_or_else(|e| panic!("{id} {versione}: {e}"));
            let i = resolve_crs(&id, "crs").unwrap();
            assert!(d.semantically_equals(&i), "{id}");
            assert_eq!(d.definition(), testo);
            assert_eq!(d.authority_axis_order(), i.authority_axis_order());
            assert_eq!(d.validity_domain(), i.validity_domain());
            assert_eq!(d.area_of_use(), i.area_of_use());
        }
    }
}

#[test]
fn cifre_esatte_senza_arrotondamenti() {
    let t = wkt("EPSG:32632");
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
}

#[test]
fn mutazioni_non_nascoste_dall_id() {
    let t = wkt("EPSG:32632");
    for (da, a) in [
        ("6378137", "6378138"),
        ("298.257223563", "298.257223564"),
        ("ENSEMBLEACCURACY[2.0]", "ENSEMBLEACCURACY[1.0]"),
        (
            "MEMBER[\"World Geodetic System 1984 (G730)\"]",
            "MEMBER[\"SENTINELLA\"]",
        ),
        ("PRIMEM[\"Greenwich\",0", "PRIMEM[\"Greenwich\",1"),
        ("9807", "9808"),
        ("origin\",0,", "origin\",1,"),
        ("origin\",9,", "origin\",10,"),
        ("0.9996", "0.9997"),
        ("500000", "500001"),
        ("northing\",0,", "northing\",1,"),
        ("0.0174532925199433", "0.01745329251994331"),
        ("LENGTHUNIT[\"metre\",1]", "LENGTHUNIT[\"metre\",2]"),
        ("east,ORDER[1]", "north,ORDER[1]"),
        ("ORDER[1]", "ORDER[2]"),
        (",ORDER[1]", ""),
        ("Cartesian,2", "Cartesian,3"),
        ("ID[\"EPSG\",4326]", "ID[\"EPSG\",4258]"),
        ("ID[\"EPSG\",32632]", "ID[\"EPSG\",32631]"),
        (",ID[\"EPSG\",32632]", ""),
        ("ID[\"EPSG\",8801]", "ID[\"EPSG\",8802]"),
    ] {
        assert!(t.contains(da), "mutazione inefficace: {da}");
        rifiuto(&t.replace(da, a), "CRS_NOT_BUILTIN");
    }
}

#[test]
fn lessico_alias_e_limiti() {
    let t = wkt("EPSG:4326");
    for testo in [
        t.replace("GEOGCRS", "geographiccrs"),
        t.replace("GEOGCRS[\"WGS 84\"", "GEOGCRS[\"test \"\"quoted\"\"\""),
        t.replace('[', "(").replace(']', ")"),
        t.replace(',', ", \n\t"),
    ] {
        resolve_crs(&testo, "crs").unwrap();
    }
    for testo in [
        format!("{t} SENTINELLA"),
        t.replacen(']', ")", 1),
        t.replacen(',', ",,", 1),
        t.trim_end_matches(']').into(),
        format!("GEOGCRS[{}0{}]", "A[".repeat(33), "]".repeat(33)),
        format!("GEOGCRS[{}0]", "0,".repeat(8192)),
        format!("GEOGCRS[\"{}\"]", "a".repeat(65536)),
    ] {
        rifiuto(&testo, "CRS_INVALID");
    }
    for testo in ["BOUNDCRS[\"SENTINELLA\"]", "GEOGCRS[\"SENTINELLA\"]"] {
        rifiuto(testo, "CRS_NOT_BUILTIN");
    }
    // Nodi ben formati ma non previsti non vengono ignorati.
    rifiuto(
        &t.replace(
            "CS[ellipsoidal,2]",
            "DYNAMIC[FRAMEEPOCH[2010]],CS[ellipsoidal,2]",
        ),
        "CRS_NOT_BUILTIN",
    );
}

#[test]
fn confini_strutturali_e_unita_comune_degli_assi() {
    // Un documento sintattico al limite arriva al rifiuto semantico;
    // oltre il limite viene fermato dal lexer prima del modello.
    for (livelli, valido) in [(32, true), (33, false)] {
        let testo = format!("{}0{}", "A[".repeat(livelli), "]".repeat(livelli));
        assert!(matches!(
            super::wkt::leggi(&testo),
            Err(e) if e == if valido { super::Rifiuto::NonIntegrato } else { super::Rifiuto::NumeroInvalido }
        ));
    }
    for (valori, valido) in [(8191, true), (8192, false)] {
        let testo = format!("A[{}0]", "0,".repeat(valori - 1));
        assert!(matches!(
            super::wkt::leggi(&testo),
            Err(e) if e == if valido { super::Rifiuto::NonIntegrato } else { super::Rifiuto::NumeroInvalido }
        ));
    }
    let t = wkt("EPSG:4326");
    let unita = ",ANGLEUNIT[\"degree\",0.0174532925199433]";
    let (prima, assi) = t.split_once("CS[ellipsoidal,2]").unwrap();
    assert_eq!(assi.matches(unita).count(), 2);
    let comune = assi.replace(unita, "").replacen(
        "USAGE[",
        "ANGLEUNIT[\"degree\",0.0174532925199433],USAGE[",
        1,
    );
    resolve_crs(&format!("{prima}CS[ellipsoidal,2]{comune}"), "crs").unwrap();
    let misto = assi.replacen(
        "USAGE[",
        "ANGLEUNIT[\"degree\",0.0174532925199433],USAGE[",
        1,
    );
    rifiuto(
        &format!("{prima}CS[ellipsoidal,2]{misto}"),
        "CRS_NOT_BUILTIN",
    );
    rifiuto(
        &format!("{prima}CS[ellipsoidal,2]{}", assi.replacen(unita, "", 1)),
        "CRS_NOT_BUILTIN",
    );
}

#[test]
fn ogni_parametro_del_corpus_e_verificato() {
    for voce in corpus().as_object().unwrap().values() {
        if !voce["esclusione_2019"].is_null() {
            continue;
        }
        let t = voce["wkt2_2019"].as_str().unwrap();
        // Le fixture esterne hanno nomi canonici senza virgolette interne.
        // Cambia una sola cifra per volta, mantenendo intatto l'ID radice.
        for (inizio, _) in t.match_indices("PARAMETER[\"") {
            let nome_fine = t[inizio + 11..].find('"').unwrap() + inizio + 11;
            let numero_inizio = nome_fine + 2;
            let numero_fine = t[numero_inizio..].find(',').unwrap() + numero_inizio;
            let mut mutato = t.to_owned();
            mutato.replace_range(numero_inizio..numero_fine, "123456789.123456789");
            rifiuto(&mutato, "CRS_NOT_BUILTIN");
        }
    }
}

#[test]
fn identita_annidate_duplicati_metadati_e_default_degli_assi() {
    let t = wkt("EPSG:4326");
    let senza_order = t.replace(",ORDER[1]", "").replace(",ORDER[2]", "");
    resolve_crs(&senza_order, "crs").unwrap();
    for (da, a) in [
        ("LENGTHUNIT[\"metre\",1]", "LENGTHUNIT[\"metre\",1,ID[\"EPSG\",9002]]"),
        ("298.257223563,LENGTHUNIT[\"metre\",1]]", "298.257223563,LENGTHUNIT[\"metre\",1],ID[\"EPSG\",7019]]"),
        ("ENSEMBLEACCURACY[2.0]]", "ENSEMBLEACCURACY[2.0],ID[\"EPSG\",6258]]"),
        ("USAGE[SCOPE[\"Horizontal component of 3D system.\"],", "USAGE["),
        ("MEMBER[\"World Geodetic System 1984 (G730)\"]", "MEMBER[\"World Geodetic System 1984 (G730)\"],MEMBER[\"World Geodetic System 1984 (G730)\"]"),
        ("ID[\"EPSG\",4326]", "ID[\"EPSG\",4326],ID[\"EPSG\",4326]"),
    ] {
        assert!(t.contains(da));
        rifiuto(&t.replace(da, a), "CRS_NOT_BUILTIN");
    }
    let geografico = t.split("USAGE[").next().unwrap();
    rifiuto(
        &format!("{geografico}USAGE[SCOPE[\"SENTINELLA\"]],ID[\"EPSG\",4326]]"),
        "CRS_NOT_BUILTIN",
    );
    let proiettato = wkt("EPSG:32632");
    let parametro = "PARAMETER[\"False easting\",500000,LENGTHUNIT[\"metre\",1],ID[\"EPSG\",8806]]";
    rifiuto(
        &proiettato.replace(parametro, &format!("{parametro},{parametro}")),
        "CRS_NOT_BUILTIN",
    );
    rifiuto(
        &proiettato.replace(&format!("{parametro},"), ""),
        "CRS_NOT_BUILTIN",
    );
    // Senza ID restano sufficienti solo i nomi finiti documentati.
    let metodo = "METHOD[\"Transverse Mercator\",ID[\"EPSG\",9807]]";
    resolve_crs(
        &proiettato.replace(metodo, "METHOD[\"Transverse Mercator\"]"),
        "crs",
    )
    .unwrap();
    rifiuto(
        &proiettato.replace(metodo, "METHOD[\"SENTINELLA\"]"),
        "CRS_NOT_BUILTIN",
    );
}
