//! Oracolo PROJJSON gia' versionato, tabelle operative e mutazioni esaustive.

use serde_json::{json, Value};

use super::*;
use crate::crs::integrati::{cerca, Direzione};
use crate::crs::riproiezione::tabella as operativa;

#[path = "metodi_generati.rs"]
mod metodi;

fn id(v: &Value) -> Option<Id> {
    v.get("id").map(|id| Id {
        authority: id["authority"].as_str().unwrap().into(),
        code: id["code"]
            .as_str()
            .map_or_else(|| id["code"].to_string(), str::to_owned),
    })
}

fn identita(v: &Value) -> Identita {
    Identita {
        id: id(v),
        nome: v["name"].as_str().unwrap().into(),
    }
}

fn numero(v: &Value) -> String {
    v.as_str().map_or_else(|| v.to_string(), str::to_owned)
}

fn unita(nome: &str) -> Unita {
    let (tipo, fattore) = match nome {
        "metre" => ("linear", "1"),
        "degree" => ("angular", "0.0174532925199433"),
        "unity" => ("scale", "1"),
        _ => panic!("unita' estranea al corpus"),
    };
    Unita {
        identita: Identita {
            id: None,
            nome: nome.into(),
        },
        tipo: tipo.into(),
        fattore: fattore.into(),
    }
}

fn sistema(v: &Value) -> Sistema {
    Sistema {
        tipo: v["subtype"].as_str().unwrap().into(),
        assi: v["axis"]
            .as_array()
            .unwrap()
            .iter()
            .enumerate()
            .map(|(i, a)| Asse {
                direzione: a["direction"].as_str().unwrap().into(),
                ordine: u32::try_from(i + 1).unwrap(),
                unita: unita(a["unit"].as_str().unwrap()),
            })
            .collect(),
    }
}

/// Adattatore solo di prova: proprieta' del corpus esterno, senza leggere
/// la descrizione attesa. Le omissioni PROJJSON sono Greenwich e metri.
fn oracolo(v: &Value) -> Definizione {
    let base = v.get("base_crs").unwrap_or(v);
    let d = base
        .get("datum_ensemble")
        .or_else(|| base.get("datum"))
        .unwrap();
    let e = &d["ellipsoid"];
    let forma = if e.get("inverse_flattening").is_some() {
        "inverse_flattening"
    } else {
        "semi_minor_axis"
    };
    let datum = Datum {
        identita: identita(d),
        tipo: if d.get("members").is_some() {
            "DatumEnsemble"
        } else {
            "GeodeticReferenceFrame"
        }
        .into(),
        membri: d.get("members").map_or_else(Vec::new, |m| {
            m.as_array().unwrap().iter().map(identita).collect()
        }),
        accuratezza: d.get("accuracy").map(numero),
        ellissoide: Ellissoide {
            identita: identita(e),
            semiasse: numero(&e["semi_major_axis"]),
            forma: forma.into(),
            seconda_misura: numero(&e[forma]),
            unita: unita("metre"),
        },
        meridiano: Meridiano {
            identita: Identita {
                id: None,
                nome: "Greenwich".into(),
            },
            longitudine: "0".into(),
            unita: unita("degree"),
        },
    };
    let conversione = v.get("conversion").map(|c| Conversione {
        id: id(c),
        metodo: identita(&c["method"]),
        parametri: c["parameters"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| Parametro {
                identita: identita(p),
                valore: numero(&p["value"]),
                unita: unita(p["unit"].as_str().unwrap()),
            })
            .collect(),
    });
    Definizione {
        id: id(v),
        tipo: v["type"].as_str().unwrap().into(),
        base: Base {
            id: id(base),
            datum,
            sistema: sistema(&base["coordinate_system"]),
        },
        conversione,
        sistema: sistema(&v["coordinate_system"]),
    }
}

#[test]
#[allow(clippy::float_cmp)] // Coerenza esatta dei double nelle tabelle generate.
fn ogni_crs_coincide_con_oracolo_e_tabelle_operative() {
    let corpus: std::collections::BTreeMap<String, Value> = serde_json::from_str(include_str!(
        "../../../../plenora-io/data/projjson_integrati.json"
    ))
    .unwrap();
    let tabella = tabella().unwrap();
    assert_eq!(tabella.len(), 169);
    assert_eq!(corpus.len(), tabella.len());
    let insieme: std::collections::BTreeSet<_> = crate::crs::builtin_crs_identifiers().collect();
    assert_eq!(
        corpus
            .keys()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>(),
        insieme
    );
    assert_eq!(
        tabella
            .iter()
            .map(|d| {
                let id = d.id.as_ref().unwrap();
                format!("{}:{}", id.authority, id.code)
            })
            .collect::<std::collections::BTreeSet<_>>(),
        insieme
    );
    let mut parametri = 0;
    for (chiave, v) in corpus {
        let indipendente = oracolo(&v);
        let identificativo = riconosci(&indipendente).unwrap_or_else(|e| panic!("{chiave}: {e:?}"));
        let attesa = tabella.iter().find(|d| d.id == indipendente.id).unwrap();
        assert_eq!(riconosci(attesa), Ok(identificativo));
        let integrato = cerca(identificativo).unwrap();
        assert_eq!(
            attesa.tipo,
            match integrato.kind {
                crate::crs::CrsKind::Geographic => "GeographicCRS",
                crate::crs::CrsKind::Projected => "ProjectedCRS",
            }
        );
        for (a, b) in attesa.sistema.assi.iter().zip(integrato.assi) {
            assert_eq!(
                a.direzione,
                match b.direzione {
                    Direzione::North => "north",
                    Direzione::East => "east",
                }
            );
        }
        let ellissoide = &attesa.base.datum.ellissoide;
        let a: f64 = ellissoide.semiasse.parse().unwrap();
        assert_eq!(a, integrato.ellissoide.semi_major_axis_metre);
        let seconda: f64 = ellissoide.seconda_misura.parse().unwrap();
        if ellissoide.forma == "inverse_flattening" {
            assert_eq!(seconda, integrato.ellissoide.inverse_flattening);
        } else {
            assert_eq!(
                seconda,
                a * (1.0 - 1.0 / integrato.ellissoide.inverse_flattening)
            );
        }
        let operativa = operativa::definizione(identificativo).unwrap();
        let datum = operativa::datum(operativa.datum).unwrap();
        assert_eq!(datum.nome, attesa.base.datum.identita.nome);
        assert_eq!(datum.ellissoide, integrato.ellissoide);
        if identificativo != Identificativo::OgcCrs84 {
            assert_eq!(
                attesa.base.id.as_ref().unwrap().code,
                operativa.datum.to_string()
            );
        }
        let (metodo, valori) = metodi::parametri(operativa.metodo);
        if let Some(conversione) = &attesa.conversione {
            assert_eq!(
                conversione.metodo.id.as_ref().unwrap().code,
                metodo.to_string()
            );
            assert_eq!(conversione.parametri.len(), valori.len());
            for (codice, valore) in valori {
                let p = conversione
                    .parametri
                    .iter()
                    .find(|p| p.identita.id.as_ref().unwrap().code == codice.to_string())
                    .unwrap();
                // PROJJSON serializza i parametri a 15 cifre significative;
                // la tabella operativa conserva il double dell'API PROJ.
                // Qui si verifica quella serializzazione, non si introduce
                // una tolleranza nel confronto delle definizioni.
                assert_eq!(
                    numeri::uguali(&p.valore, &format!("{valore:.14e}")),
                    Ok(true)
                );
                parametri += 1;
            }
        } else {
            assert_eq!(metodo, 0);
        }
    }
    println!("oracolo: 169 CRS, {parametri} parametri operativi");
}

fn foglie(v: &Value, percorso: &str, uscite: &mut Vec<String>) {
    match v {
        Value::Object(m) => {
            for (k, v) in m {
                if k != "nome" {
                    foglie(v, &format!("{percorso}/{k}"), uscite);
                }
            }
        }
        Value::Array(a) => {
            for (i, v) in a.iter().enumerate() {
                foglie(v, &format!("{percorso}/{i}"), uscite);
            }
        }
        Value::String(_) | Value::Number(_) => uscite.push(percorso.into()),
        Value::Null => {}
        Value::Bool(_) => panic!("tipo non previsto nel modello"),
    }
}

#[test]
fn ogni_campo_semantico_mutato_e_rifiutato() {
    let mut mutazioni = 0;
    for attesa in tabella().unwrap() {
        let v = serde_json::to_value(&attesa).unwrap();
        let mut percorsi = Vec::new();
        foglie(&v, "", &mut percorsi);
        for percorso in percorsi {
            let mut mutata = v.clone();
            let foglia = mutata.pointer_mut(&percorso).unwrap();
            *foglia = if foglia.is_number() {
                json!(999)
            } else {
                json!("123456789")
            };
            let mutata: Definizione = serde_json::from_value(mutata).unwrap();
            assert_eq!(
                confronto::confronta(&mutata, &attesa),
                Err(Rifiuto::NonIntegrato),
                "{percorso}"
            );
            mutazioni += 1;
        }
        let mut array = vec!["/sistema/assi", "/base/sistema/assi", "/base/datum/membri"];
        if attesa.conversione.is_some() {
            array.push("/conversione/parametri");
        }
        for percorso in array {
            let a = v.pointer(percorso).unwrap().as_array().unwrap();
            if a.is_empty() {
                continue;
            }
            for modo in 0..3 {
                let mut mutata = v.clone();
                let a = mutata
                    .pointer_mut(percorso)
                    .unwrap()
                    .as_array_mut()
                    .unwrap();
                match modo {
                    0 => {
                        a.pop();
                    }
                    1 => a.push(a[0].clone()),
                    _ => {
                        if a.len() > 1 {
                            a[1] = a[0].clone();
                        } else {
                            continue;
                        }
                    }
                }
                let mutata = serde_json::from_value(mutata).unwrap();
                assert_eq!(
                    confronto::confronta(&mutata, &attesa),
                    Err(Rifiuto::NonIntegrato)
                );
                mutazioni += 1;
            }
        }
    }
    println!("mutazioni semantiche rifiutate: {mutazioni}");
}

#[test]
fn identita_assi_alias_e_insiemi() {
    for mut d in tabella().unwrap() {
        let attesa = d.clone();
        d.id = None;
        assert_eq!(riconosci(&d), Err(Rifiuto::NonIntegrato));
        d = attesa.clone();
        d.sistema.assi.swap(0, 1);
        assert_eq!(
            confronto::confronta(&d, &attesa),
            Err(Rifiuto::NonIntegrato)
        );
        d = attesa.clone();
        d.base.datum.membri.reverse();
        if let Some(c) = &mut d.conversione {
            c.parametri.reverse();
        }
        assert_eq!(confronto::confronta(&d, &attesa), Ok(()));
        d.base.datum.identita.nome = "nome descrittivo".into();
        assert_eq!(confronto::confronta(&d, &attesa), Ok(()));
        d.base.datum.identita.id = None;
        assert_eq!(
            confronto::confronta(&d, &attesa),
            Err(Rifiuto::NonIntegrato)
        );
        d.base
            .datum
            .identita
            .nome
            .clone_from(&attesa.base.datum.identita.nome);
        assert_eq!(confronto::confronta(&d, &attesa), Ok(()));
    }
    let mut wgs = tabella()
        .unwrap()
        .into_iter()
        .find(|d| d.id.as_ref().unwrap().code == "4326")
        .unwrap();
    wgs.base.datum.tipo = "GeodeticReferenceFrame".into();
    wgs.base.datum.membri.clear();
    wgs.base.datum.accuratezza = None;
    assert_eq!(riconosci(&wgs), Err(Rifiuto::NonIntegrato));
    wgs.id.as_mut().unwrap().code = "4979".into();
    assert_eq!(riconosci(&wgs), Err(Rifiuto::NonIntegrato));
    assert_eq!(Rifiuto::NonIntegrato.codice(), "CRS_NOT_BUILTIN");
    assert_eq!(Rifiuto::NumeroInvalido.codice(), "CRS_INVALID");
}

#[test]
fn decimali_esatti_senza_double() {
    for (a, b) in [
        ("500000", "5e5"),
        ("500000.0", "+050e4"),
        ("-0", "0.0e-999"),
        ("1e9999", "10e9998"),
        ("1e-9999", "0.1e-9998"),
        ("1E+3", "1000"),
        ("9007199254740993", "9007199254740993.0"),
    ] {
        assert_eq!(numeri::uguali(a, b), Ok(true));
    }
    for (a, b) in [
        ("500000.000000000000000000001", "500000"),
        ("9007199254740992", "9007199254740993"),
        ("1e-999", "0"),
        ("-1", "1"),
    ] {
        assert_eq!(numeri::uguali(a, b), Ok(false));
    }
    for invalido in [
        "",
        "NaN",
        "inf",
        "1e",
        "1e+",
        "1.2.3",
        "--1",
        "+-1",
        "+-0",
        " 1",
        "1e1e2",
        "1.",
        ".1",
        "1e9223372036854775808",
        "10e9223372036854775807",
        "0.1e-9223372036854775808",
    ] {
        assert_eq!(numeri::normalizza(invalido), Err(Rifiuto::NumeroInvalido));
    }
    // Oracolo intero indipendente dalla normalizzazione: 20.001 valori,
    // quattro forme esatte ciascuno e il vicino decimale discordante.
    for intero in -10_000_i64..=10_000 {
        let canonico = intero.to_string();
        for forma in [
            format!("{intero}.000"),
            format!("{}e-3", intero * 1000),
            format!("{intero}e0"),
            format!("{intero}.0E+0"),
        ] {
            assert_eq!(numeri::uguali(&canonico, &forma), Ok(true));
        }
        assert_eq!(
            numeri::uguali(&canonico, &format!("{intero}.00000000000000000001")),
            Ok(false)
        );
    }
}

#[test]
fn numeri_attraverso_il_riconoscimento() {
    let attesa = tabella()
        .unwrap()
        .into_iter()
        .find(|d| d.id.as_ref().unwrap().code == "32632")
        .unwrap();
    for (token, errore) in [
        ("500000.000000000000000000001", Rifiuto::NonIntegrato),
        ("1e9999", Rifiuto::NonIntegrato),
        ("1e-9999", Rifiuto::NonIntegrato),
        ("1e9223372036854775808", Rifiuto::NumeroInvalido),
        ("+-0", Rifiuto::NumeroInvalido),
    ] {
        let mut d = attesa.clone();
        let parametro = d
            .conversione
            .as_mut()
            .unwrap()
            .parametri
            .iter_mut()
            .find(|p| p.identita.id.as_ref().unwrap().code == "8806")
            .unwrap();
        parametro.valore = token.into();
        assert_eq!(riconosci(&d), Err(errore));
    }
    for token in ["500000", "500000.0", "5e5"] {
        let mut d = attesa.clone();
        d.conversione
            .as_mut()
            .unwrap()
            .parametri
            .iter_mut()
            .find(|p| p.identita.id.as_ref().unwrap().code == "8806")
            .unwrap()
            .valore = token.into();
        assert_eq!(riconosci(&d), Ok(Identificativo::Epsg(32632)));
    }
    // WGS84 ed ETRS89 non si sostituiscono, anche quando la riproiezione
    // ammette una convenzione operativa tra i rispettivi datum.
    let mut wgs = attesa;
    let etrs = tabella()
        .unwrap()
        .into_iter()
        .find(|d| d.id.as_ref().unwrap().code == "25832")
        .unwrap();
    wgs.base.datum = etrs.base.datum;
    assert_eq!(riconosci(&wgs), Err(Rifiuto::NonIntegrato));
}
