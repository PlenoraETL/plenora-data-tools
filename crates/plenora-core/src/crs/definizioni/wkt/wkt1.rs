//! WKT1 con assi espliciti anche nella base, senza equivalenze di datum.

use super::{
    Asse, Base, Campi, Conversione, Datum, Definizione, Ellissoide, Esito, Id, Identita, Meridiano,
    Nodo, Parametro, Rifiuto, Sistema, Unita,
};

fn authority(c: &mut Campi<'_>) -> Esito<Option<Id>> {
    let Some(n) = c.opzionale(&["AUTHORITY"]) else {
        return Ok(None);
    };
    let mut a = Campi::nuovo(n);
    let id = Id {
        authority: a.testo()?,
        code: a.testo()?,
    };
    a.fine()?;
    Ok(Some(id))
}

fn unita(c: &mut Campi<'_>, tipo: &str) -> Esito<Unita> {
    let mut u = Campi::nuovo(c.nodo(&["UNIT"])?);
    let nome = u.testo()?;
    let fattore = u.numero()?;
    let id = authority(&mut u)?;
    u.fine()?;
    Ok(Unita {
        identita: Identita { id, nome },
        tipo: tipo.into(),
        fattore,
    })
}

fn implicita(nome: &str, tipo: &str) -> Unita {
    Unita {
        identita: Identita {
            id: None,
            nome: nome.into(),
        },
        tipo: tipo.into(),
        fattore: "1".into(),
    }
}

fn sistema(c: &mut Campi<'_>, u: &Unita, tipo: &str) -> Esito<Sistema> {
    let mut assi = Vec::new();
    for ordine in 1..=2 {
        let mut a = Campi::nuovo(c.nodo(&["AXIS"])?);
        a.testo()?;
        let direzione = a.parola()?.to_ascii_lowercase();
        a.fine()?;
        assi.push(Asse {
            ordine,
            direzione,
            unita: u.clone(),
        });
    }
    Ok(Sistema {
        tipo: tipo.into(),
        assi,
    })
}

fn base(nodo: &Nodo) -> Esito<Base> {
    let mut campi = Campi::nuovo(nodo);
    campi.testo()?;
    let mut campi_datum = Campi::nuovo(campi.nodo(&["DATUM"])?);
    let nome = campi_datum.testo()?;
    let mut campi_ellissoide = Campi::nuovo(campi_datum.nodo(&["SPHEROID"])?);
    let enome = campi_ellissoide.testo()?;
    let semiasse = campi_ellissoide.numero()?;
    let seconda_misura = campi_ellissoide.numero()?;
    let eid = authority(&mut campi_ellissoide)?;
    campi_ellissoide.fine()?;
    let id = authority(&mut campi_datum)?;
    campi_datum.fine()?;
    let mut campi_meridiano = Campi::nuovo(campi.nodo(&["PRIMEM"])?);
    let pnome = campi_meridiano.testo()?;
    let longitudine = campi_meridiano.numero()?;
    let pid = authority(&mut campi_meridiano)?;
    campi_meridiano.fine()?;
    let unita_angolare = unita(&mut campi, "angular")?;
    let sistema = sistema(&mut campi, &unita_angolare, "ellipsoidal")?;
    let bid = authority(&mut campi)?;
    campi.fine()?;
    Ok(Base {
        id: bid,
        sistema,
        datum: Datum {
            identita: Identita { id, nome },
            tipo: "GeodeticReferenceFrame".into(),
            membri: Vec::new(),
            accuratezza: None,
            ellissoide: Ellissoide {
                identita: Identita {
                    id: eid,
                    nome: enome,
                },
                semiasse,
                forma: "inverse_flattening".into(),
                seconda_misura,
                unita: implicita("metre", "linear"),
            },
            meridiano: Meridiano {
                identita: Identita {
                    id: pid,
                    nome: pnome,
                },
                longitudine,
                unita: unita_angolare,
            },
        },
    })
}

// Le ambiguita' di latitude_of_origin e false_easting si risolvono dal
// metodo dichiarato, mai dall'ID del CRS o dai valori dei parametri.
fn parametro(metodo: &str, nome: &str) -> Esito<(&'static str, &'static str)> {
    let conica = metodo == "Lambert Conic Conformal (2SP)";
    let obliqua = metodo == "Hotine Oblique Mercator (variant B)";
    Ok(match nome {
        "latitude_of_origin" if !obliqua => (
            if conica {
                "Latitude of false origin"
            } else {
                "Latitude of natural origin"
            },
            "angular",
        ),
        "central_meridian" if !obliqua => (
            if conica {
                "Longitude of false origin"
            } else {
                "Longitude of natural origin"
            },
            "angular",
        ),
        "standard_parallel_1" if conica => ("Latitude of 1st standard parallel", "angular"),
        "standard_parallel_2" if conica => ("Latitude of 2nd standard parallel", "angular"),
        "latitude_of_center" if obliqua => ("Latitude of projection centre", "angular"),
        "longitude_of_center" if obliqua => ("Longitude of projection centre", "angular"),
        "azimuth" if obliqua => ("Azimuth at projection centre", "angular"),
        "rectified_grid_angle" if obliqua => ("Angle from Rectified to Skew Grid", "angular"),
        "scale_factor" if !conica => (
            if obliqua {
                "Scale factor at projection centre"
            } else {
                "Scale factor at natural origin"
            },
            "scale",
        ),
        "false_easting" => (
            if conica {
                "Easting at false origin"
            } else if obliqua {
                "Easting at projection centre"
            } else {
                "False easting"
            },
            "linear",
        ),
        "false_northing" => (
            if conica {
                "Northing at false origin"
            } else if obliqua {
                "Northing at projection centre"
            } else {
                "False northing"
            },
            "linear",
        ),
        _ => return Err(Rifiuto::NonIntegrato),
    })
}

pub(super) fn modello(n: &Nodo) -> Esito<Definizione> {
    if n.nome == "GEOGCS" {
        let base = base(n)?;
        return Ok(Definizione {
            id: base.id.clone(),
            tipo: "GeographicCRS".into(),
            sistema: base.sistema.clone(),
            base,
            conversione: None,
        });
    }
    let mut c = Campi::nuovo(n);
    c.testo()?;
    let base = base(c.nodo(&["GEOGCS"])?)?;
    let mut m = Campi::nuovo(c.nodo(&["PROJECTION"])?);
    let (nome, codice) = match m.testo()?.as_str() {
        "Transverse_Mercator" => ("Transverse Mercator", "9807"),
        "Lambert_Conformal_Conic_2SP" => ("Lambert Conic Conformal (2SP)", "9802"),
        "Hotine_Oblique_Mercator_Azimuth_Center" => ("Hotine Oblique Mercator (variant B)", "9815"),
        "Oblique_Stereographic" => ("Oblique Stereographic", "9809"),
        _ => return Err(Rifiuto::NonIntegrato),
    };
    let metodo = Identita {
        id: authority(&mut m)?,
        nome: nome.into(),
    };
    // L'alias determina la famiglia dei parametri: un ID contraddittorio
    // non puo' renderlo meramente descrittivo come il nome di un datum.
    if metodo
        .id
        .as_ref()
        .is_some_and(|id| id.authority != "EPSG" || id.code != codice)
    {
        return Err(Rifiuto::NonIntegrato);
    }
    m.fine()?;
    let mut letti = Vec::new();
    while let Some(n) = c.opzionale(&["PARAMETER"]) {
        let mut p = Campi::nuovo(n);
        let alias = p.testo()?;
        let (nome, tipo) = parametro(nome, &alias)?;
        let valore = p.numero()?;
        p.fine()?;
        letti.push((nome, tipo, valore));
    }
    let lineare = unita(&mut c, "linear")?;
    let parametri = letti
        .into_iter()
        .map(|(nome, tipo, valore)| Parametro {
            identita: Identita {
                id: None,
                nome: nome.into(),
            },
            valore,
            unita: match tipo {
                "angular" => base.datum.meridiano.unita.clone(),
                "linear" => lineare.clone(),
                _ => implicita("unity", "scale"),
            },
        })
        .collect();
    let sistema = sistema(&mut c, &lineare, "Cartesian")?;
    let id = authority(&mut c)?;
    c.fine()?;
    Ok(Definizione {
        id,
        tipo: "ProjectedCRS".into(),
        base,
        sistema,
        conversione: Some(Conversione {
            id: None,
            metodo,
            parametri,
        }),
    })
}
