//! Profilo PROJJSON 2D chiuso: nessun campo operativo viene ignorato.

use serde_json::{Map, Value};

use super::{
    Asse, Base, Conversione, Datum, Definizione, Ellissoide, Id, Identita, Meridiano, Parametro,
    Rifiuto, Sistema, Unita,
};

type Esito<T> = Result<T, Rifiuto>;

fn oggetto<'a>(v: &'a Value, campi: &[&str]) -> Esito<&'a Map<String, Value>> {
    let m = v.as_object().ok_or(Rifiuto::NonIntegrato)?;
    if m.keys().any(|k| !campi.contains(&k.as_str())) {
        return Err(Rifiuto::NonIntegrato);
    }
    Ok(m)
}

fn campo<'a>(m: &'a Map<String, Value>, k: &str) -> Esito<&'a Value> {
    m.get(k).ok_or(Rifiuto::NonIntegrato)
}

fn stringa(v: &Value) -> Esito<String> {
    v.as_str().map(str::to_owned).ok_or(Rifiuto::NonIntegrato)
}

fn testo(m: &Map<String, Value>, k: &str) -> Esito<String> {
    stringa(campo(m, k)?)
}

fn numero(v: &Value) -> Esito<String> {
    let n = v.as_number().ok_or(Rifiuto::NonIntegrato)?.to_string();
    super::numeri::normalizza(&n)?;
    Ok(n)
}

fn elenco(v: &Value) -> Esito<&Vec<Value>> {
    v.as_array().ok_or(Rifiuto::NonIntegrato)
}

fn id(m: &Map<String, Value>) -> Esito<Option<Id>> {
    let v = match (m.get("id"), m.get("ids")) {
        (None, None) => return Ok(None),
        (Some(v), None) => v,
        (None, Some(Value::Array(v))) if v.len() == 1 => &v[0],
        _ => return Err(Rifiuto::NonIntegrato),
    };
    let m = oggetto(v, &["authority", "code"])?;
    let code = match campo(m, "code")? {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.as_u64().ok_or(Rifiuto::NonIntegrato)?.to_string(),
        _ => return Err(Rifiuto::NonIntegrato),
    };
    Ok(Some(Id {
        authority: testo(m, "authority")?,
        code,
    }))
}

fn identita(m: &Map<String, Value>) -> Esito<Identita> {
    Ok(Identita {
        id: id(m)?,
        nome: testo(m, "name")?,
    })
}

fn simbolica(nome: &str) -> Esito<Unita> {
    let (code, tipo, fattore) = match nome {
        "metre" => ("9001", "linear", "1"),
        "degree" => ("9122", "angular", "0.0174532925199433"),
        "unity" => ("9201", "scale", "1"),
        _ => return Err(Rifiuto::NonIntegrato),
    };
    Ok(Unita {
        identita: Identita {
            id: Some(Id {
                authority: "EPSG".into(),
                code: code.into(),
            }),
            nome: nome.into(),
        },
        tipo: tipo.into(),
        fattore: fattore.into(),
    })
}

fn unita(v: &Value) -> Esito<Unita> {
    if let Some(s) = v.as_str() {
        return simbolica(s);
    }
    let m = oggetto(v, &["type", "name", "conversion_factor", "id", "ids"])?;
    let tipo = match testo(m, "type")?.as_str() {
        "LinearUnit" => "linear",
        "AngularUnit" => "angular",
        "ScaleUnit" => "scale",
        _ => return Err(Rifiuto::NonIntegrato),
    };
    Ok(Unita {
        identita: identita(m)?,
        tipo: tipo.into(),
        fattore: numero(campo(m, "conversion_factor")?)?,
    })
}

fn sistema(v: &Value) -> Esito<Sistema> {
    let m = oggetto(v, &["subtype", "axis"])?;
    let assi = elenco(campo(m, "axis")?)?;
    if assi.len() != 2 {
        return Err(Rifiuto::NonIntegrato);
    }
    let mut risultato = Vec::new();
    for (ordine, v) in (1..=2).zip(assi) {
        let a = oggetto(v, &["name", "abbreviation", "direction", "unit"])?;
        testo(a, "name")?;
        testo(a, "abbreviation")?;
        risultato.push(Asse {
            ordine,
            direzione: testo(a, "direction")?,
            unita: unita(campo(a, "unit")?)?,
        });
    }
    Ok(Sistema {
        tipo: testo(m, "subtype")?,
        assi: risultato,
    })
}

fn ellissoide(v: &Value) -> Esito<Ellissoide> {
    let m = oggetto(
        v,
        &[
            "name",
            "id",
            "ids",
            "semi_major_axis",
            "semi_minor_axis",
            "inverse_flattening",
        ],
    )?;
    let forma = match (
        m.contains_key("inverse_flattening"),
        m.contains_key("semi_minor_axis"),
    ) {
        (true, false) => "inverse_flattening",
        (false, true) => "semi_minor_axis",
        _ => return Err(Rifiuto::NonIntegrato),
    };
    Ok(Ellissoide {
        identita: identita(m)?,
        semiasse: numero(campo(m, "semi_major_axis")?)?,
        forma: forma.into(),
        seconda_misura: numero(campo(m, forma)?)?,
        unita: simbolica("metre")?,
    })
}

fn meridiano(v: Option<&Value>) -> Esito<Meridiano> {
    // PROJJSON definisce Greenwich quando prime_meridian e' assente.
    let Some(v) = v else {
        return Ok(Meridiano {
            identita: Identita {
                id: None,
                nome: "Greenwich".into(),
            },
            longitudine: "0".into(),
            unita: simbolica("degree")?,
        });
    };
    let m = oggetto(v, &["name", "id", "ids", "longitude"])?;
    let v = campo(m, "longitude")?;
    let (longitudine, unita) = if v.is_number() {
        (numero(v)?, simbolica("degree")?)
    } else {
        let m = oggetto(v, &["value", "unit"])?;
        (numero(campo(m, "value")?)?, unita(campo(m, "unit")?)?)
    };
    Ok(Meridiano {
        identita: identita(m)?,
        longitudine,
        unita,
    })
}

fn datum(m: &Map<String, Value>) -> Esito<Datum> {
    let (v, ensemble) = match (m.get("datum"), m.get("datum_ensemble")) {
        (Some(v), None) => (v, false),
        (None, Some(v)) => (v, true),
        _ => return Err(Rifiuto::NonIntegrato),
    };
    let campi: &[&str] = if ensemble {
        &[
            "name",
            "id",
            "ids",
            "ellipsoid",
            "prime_meridian",
            "members",
            "accuracy",
        ]
    } else {
        &["type", "name", "id", "ids", "ellipsoid", "prime_meridian"]
    };
    let d = oggetto(v, campi)?;
    if !ensemble && testo(d, "type")? != "GeodeticReferenceFrame" {
        return Err(Rifiuto::NonIntegrato);
    }
    let mut membri = Vec::new();
    let accuratezza = if ensemble {
        for v in elenco(campo(d, "members")?)? {
            membri.push(identita(oggetto(v, &["name", "id", "ids"])?)?);
        }
        let a = testo(d, "accuracy")?;
        super::numeri::normalizza(&a)?;
        Some(a)
    } else {
        None
    };
    Ok(Datum {
        identita: identita(d)?,
        tipo: if ensemble {
            "DatumEnsemble"
        } else {
            "GeodeticReferenceFrame"
        }
        .into(),
        membri,
        accuratezza,
        ellissoide: ellissoide(campo(d, "ellipsoid")?)?,
        meridiano: meridiano(d.get("prime_meridian"))?,
    })
}

fn conversione(v: &Value) -> Esito<Conversione> {
    let m = oggetto(v, &["name", "id", "ids", "method", "parameters"])?;
    testo(m, "name")?;
    let metodo = identita(oggetto(campo(m, "method")?, &["name", "id", "ids"])?)?;
    let mut parametri = Vec::new();
    for v in elenco(campo(m, "parameters")?)? {
        let p = oggetto(v, &["name", "id", "ids", "value", "unit"])?;
        parametri.push(Parametro {
            identita: identita(p)?,
            valore: numero(campo(p, "value")?)?,
            unita: unita(campo(p, "unit")?)?,
        });
    }
    Ok(Conversione {
        id: id(m)?,
        metodo,
        parametri,
    })
}

fn descrittivi(m: &Map<String, Value>) -> Esito<()> {
    for k in ["$schema", "scope", "area", "remarks"] {
        if let Some(v) = m.get(k) {
            stringa(v)?;
        }
    }
    if let Some(v) = m.get("bbox") {
        let campi = [
            "south_latitude",
            "west_longitude",
            "north_latitude",
            "east_longitude",
        ];
        let m = oggetto(v, &campi)?;
        for k in campi {
            numero(campo(m, k)?)?;
        }
    }
    Ok(())
}

fn modello(v: &Value) -> Esito<Definizione> {
    let m = oggetto(
        v,
        &[
            "$schema",
            "type",
            "name",
            "id",
            "ids",
            "datum",
            "datum_ensemble",
            "base_crs",
            "conversion",
            "coordinate_system",
            "scope",
            "area",
            "bbox",
            "remarks",
            "usages",
        ],
    )?;
    testo(m, "name")?;
    descrittivi(m)?;
    if let Some(v) = m.get("usages") {
        if ["scope", "area", "bbox"].iter().any(|k| m.contains_key(*k)) {
            return Err(Rifiuto::NonIntegrato);
        }
        let usages = elenco(v)?;
        if usages.is_empty() {
            return Err(Rifiuto::NonIntegrato);
        }
        for u in usages {
            descrittivi(oggetto(u, &["scope", "area", "bbox"])?)?;
        }
    }
    let tipo = testo(m, "type")?;
    let (b, conversione) = match tipo.as_str() {
        "GeographicCRS" if !m.contains_key("base_crs") && !m.contains_key("conversion") => {
            (m, None)
        }
        "ProjectedCRS" if !m.contains_key("datum") && !m.contains_key("datum_ensemble") => {
            let b = oggetto(
                campo(m, "base_crs")?,
                &[
                    "name",
                    "id",
                    "ids",
                    "datum",
                    "datum_ensemble",
                    "coordinate_system",
                ],
            )?;
            testo(b, "name")?;
            (b, Some(conversione(campo(m, "conversion")?)?))
        }
        _ => return Err(Rifiuto::NonIntegrato),
    };
    Ok(Definizione {
        id: id(m)?,
        tipo,
        base: Base {
            id: id(b)?,
            datum: datum(b)?,
            sistema: sistema(campo(b, "coordinate_system")?)?,
        },
        conversione,
        sistema: sistema(campo(m, "coordinate_system")?)?,
    })
}

/// Limiti prima dell'allocazione dell'albero; stringhe ed escape non sono nodi.
fn limiti(testo: &str) -> Esito<()> {
    let (mut profondita, mut nodi, mut stringa, mut escape) = (0_u32, 0_u32, false, false);
    for b in testo.bytes() {
        if stringa {
            if escape {
                escape = false;
            } else if b == b'\\' {
                escape = true;
            } else if b == b'"' {
                stringa = false;
            }
            continue;
        }
        match b {
            b'"' => {
                stringa = true;
                nodi += 1;
            }
            b'{' | b'[' => {
                profondita += 1;
                nodi += 1;
            }
            b'}' | b']' => {
                profondita = profondita.saturating_sub(1);
            }
            b',' | b':' => {
                nodi += 1;
            }
            _ => {}
        }
        if profondita > 32 || nodi > 8192 {
            return Err(Rifiuto::NumeroInvalido);
        }
    }
    Ok(())
}

pub(super) fn leggi(testo: &str) -> Esito<Definizione> {
    limiti(testo)?;
    crate::json::ensure_no_duplicate_keys(testo).map_err(|_| Rifiuto::NumeroInvalido)?;
    // Nessuna cifra puo' sparire nella conversione a Value. float_roundtrip
    // del workspace e questa guardia precedono ogni confronto decimale.
    crate::json::ensure_numbers_exact(testo).map_err(|_| Rifiuto::NumeroInvalido)?;
    let v = serde_json::from_str(testo).map_err(|_| Rifiuto::NumeroInvalido)?;
    modello(&v)
}
