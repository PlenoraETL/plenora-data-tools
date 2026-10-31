//! Profili WKT chiusi; token numerici conservati senza passare da f64.

mod wkt1;

use super::{
    Asse, Base, Conversione, Datum, Definizione, Ellissoide, Id, Identita, Meridiano, Parametro,
    Rifiuto, Sistema, Unita,
};

type Esito<T> = Result<T, Rifiuto>;

#[derive(Debug)]
enum Valore {
    Testo(String),
    Numero(String),
    Parola(String),
    Nodo(Nodo),
}

#[derive(Debug)]
struct Nodo {
    nome: String,
    valori: Vec<Valore>,
}

struct Lexer<'a> {
    resto: &'a str,
    nodi: usize,
}

impl Lexer<'_> {
    fn spazio(&mut self) {
        self.resto = self
            .resto
            .trim_start_matches(|c: char| c.is_ascii_whitespace());
    }

    fn valore(&mut self, profondita: usize) -> Esito<Valore> {
        self.spazio();
        self.nodi += 1;
        if profondita > 32 || self.nodi > 8192 {
            return Err(Rifiuto::NumeroInvalido);
        }
        if let Some(resto) = self.resto.strip_prefix('"') {
            self.resto = resto;
            let mut testo = String::new();
            loop {
                let (parte, resto) = self.resto.split_once('"').ok_or(Rifiuto::NumeroInvalido)?;
                testo.push_str(parte);
                self.resto = resto;
                if let Some(resto) = self.resto.strip_prefix('"') {
                    testo.push('"');
                    self.resto = resto;
                } else {
                    return Ok(Valore::Testo(testo));
                }
            }
        }
        let fine = self
            .resto
            .find(|c: char| c.is_ascii_whitespace() || "[],()".contains(c))
            .unwrap_or(self.resto.len());
        let token = self.resto.get(..fine).ok_or(Rifiuto::NumeroInvalido)?;
        if token.is_empty() {
            return Err(Rifiuto::NumeroInvalido);
        }
        self.resto = self.resto.get(fine..).ok_or(Rifiuto::NumeroInvalido)?;
        if token.starts_with(|c: char| c.is_ascii_digit() || "+-.".contains(c)) {
            // Il profilo comune richiede cifre da entrambi i lati del punto.
            super::numeri::normalizza(token)?;
            return Ok(Valore::Numero(token.into()));
        }
        if !token
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'_')
        {
            return Err(Rifiuto::NumeroInvalido);
        }
        let nome = token.to_ascii_uppercase();
        self.spazio();
        let chiusura = if let Some(resto) = self.resto.strip_prefix('[') {
            self.resto = resto;
            ']'
        } else if let Some(resto) = self.resto.strip_prefix('(') {
            self.resto = resto;
            ')'
        } else {
            return Ok(Valore::Parola(nome));
        };
        let mut valori = Vec::new();
        loop {
            valori.push(self.valore(profondita + 1)?);
            self.spazio();
            if let Some(resto) = self.resto.strip_prefix(chiusura) {
                self.resto = resto;
                break;
            }
            self.resto = self
                .resto
                .strip_prefix(',')
                .ok_or(Rifiuto::NumeroInvalido)?;
        }
        Ok(Valore::Nodo(Nodo { nome, valori }))
    }
}

/// Cursore della grammatica: ordine, cardinalita' e consumo sono espliciti.
struct Campi<'a> {
    resto: &'a [Valore],
}

impl<'a> Campi<'a> {
    fn nuovo(n: &'a Nodo) -> Self {
        Self { resto: &n.valori }
    }

    fn prossimo(&mut self) -> Esito<&'a Valore> {
        let (v, resto) = self.resto.split_first().ok_or(Rifiuto::NonIntegrato)?;
        self.resto = resto;
        Ok(v)
    }

    fn testo(&mut self) -> Esito<String> {
        match self.prossimo()? {
            Valore::Testo(s) => Ok(s.clone()),
            _ => Err(Rifiuto::NonIntegrato),
        }
    }

    fn numero(&mut self) -> Esito<String> {
        match self.prossimo()? {
            Valore::Numero(s) => Ok(s.clone()),
            _ => Err(Rifiuto::NumeroInvalido),
        }
    }

    fn parola(&mut self) -> Esito<String> {
        match self.prossimo()? {
            Valore::Parola(s) => Ok(s.clone()),
            _ => Err(Rifiuto::NonIntegrato),
        }
    }

    fn opzionale(&mut self, nomi: &[&str]) -> Option<&'a Nodo> {
        if let Some((Valore::Nodo(n), resto)) = self.resto.split_first() {
            if nomi.contains(&n.nome.as_str()) {
                self.resto = resto;
                return Some(n);
            }
        }
        None
    }

    fn nodo(&mut self, nomi: &[&str]) -> Esito<&'a Nodo> {
        self.opzionale(nomi).ok_or(Rifiuto::NonIntegrato)
    }

    const fn fine(self) -> Esito<()> {
        if self.resto.is_empty() {
            Ok(())
        } else {
            Err(Rifiuto::NonIntegrato)
        }
    }

    fn id(&mut self) -> Esito<Option<Id>> {
        let Some(n) = self.opzionale(&["ID"]) else {
            return Ok(None);
        };
        let mut c = Self::nuovo(n);
        let authority = c.testo()?;
        let code = match c.prossimo()? {
            Valore::Testo(s) | Valore::Numero(s) => s.clone(),
            _ => return Err(Rifiuto::NonIntegrato),
        };
        c.fine()?;
        Ok(Some(Id { authority, code }))
    }
}

fn identita(n: &Nodo) -> Esito<Identita> {
    let mut c = Campi::nuovo(n);
    let nome = c.testo()?;
    let id = c.id()?;
    c.fine()?;
    Ok(Identita { id, nome })
}

fn unita(n: &Nodo) -> Esito<Unita> {
    let tipo = match n.nome.as_str() {
        "LENGTHUNIT" => "linear",
        "ANGLEUNIT" => "angular",
        "SCALEUNIT" => "scale",
        _ => return Err(Rifiuto::NonIntegrato),
    };
    let mut c = Campi::nuovo(n);
    let nome = c.testo()?;
    let fattore = c.numero()?;
    let id = c.id()?;
    c.fine()?;
    Ok(Unita {
        identita: Identita { id, nome },
        tipo: tipo.into(),
        fattore,
    })
}

fn grado() -> Unita {
    Unita {
        identita: Identita {
            nome: "degree".into(),
            id: None,
        },
        tipo: "angular".into(),
        fattore: "0.0174532925199433".into(),
    }
}

fn sistema(c: &mut Campi<'_>) -> Esito<Sistema> {
    let mut cs = Campi::nuovo(c.nodo(&["CS"])?);
    let tipo = match cs.parola()?.as_str() {
        "ELLIPSOIDAL" => "ellipsoidal",
        "CARTESIAN" => "Cartesian",
        _ => return Err(Rifiuto::NonIntegrato),
    };
    if cs.numero()? != "2" {
        return Err(Rifiuto::NonIntegrato);
    }
    cs.fine()?;
    let mut assi = Vec::new();
    let mut unita_assi = Vec::new();
    let mut ordini = 0;
    for ordine in 1..=2 {
        let mut a = Campi::nuovo(c.nodo(&["AXIS"])?);
        a.testo()?;
        let direzione = a.parola()?.to_ascii_lowercase();
        if let Some(n) = a.opzionale(&["ORDER"]) {
            ordini += 1;
            let mut o = Campi::nuovo(n);
            if o.numero()? != ordine.to_string() {
                return Err(Rifiuto::NonIntegrato);
            }
            o.fine()?;
        }
        let u = a
            .opzionale(&["ANGLEUNIT", "LENGTHUNIT"])
            .map(unita)
            .transpose()?;
        a.fine()?;
        assi.push((ordine, direzione));
        unita_assi.push(u);
    }
    if ordini == 1 {
        return Err(Rifiuto::NonIntegrato);
    }
    let comune = c
        .opzionale(&["ANGLEUNIT", "LENGTHUNIT"])
        .map(unita)
        .transpose()?;
    let assi = assi
        .into_iter()
        .zip(unita_assi)
        .map(|((ordine, direzione), u)| {
            let unita = match (u, &comune) {
                (Some(u), None) => u,
                (None, Some(u)) => u.clone(),
                _ => return Err(Rifiuto::NonIntegrato),
            };
            Ok(Asse {
                direzione,
                ordine,
                unita,
            })
        })
        .collect::<Esito<Vec<_>>>()?;
    Ok(Sistema {
        tipo: tipo.into(),
        assi,
    })
}

fn datum(campi: &mut Campi<'_>) -> Esito<Datum> {
    let nodo_datum = campi.nodo(&["DATUM", "GEODETICDATUM", "ENSEMBLE"])?;
    let ensemble = nodo_datum.nome == "ENSEMBLE";
    let mut campi_datum = Campi::nuovo(nodo_datum);
    let nome = campi_datum.testo()?;
    let mut membri = Vec::new();
    if ensemble {
        while let Some(nodo_datum) = campi_datum.opzionale(&["MEMBER"]) {
            membri.push(identita(nodo_datum)?);
        }
        if membri.len() < 2 {
            return Err(Rifiuto::NonIntegrato);
        }
    }
    let mut campi_ellissoide = Campi::nuovo(campi_datum.nodo(&["ELLIPSOID"])?);
    let enome = campi_ellissoide.testo()?;
    let semiasse = campi_ellissoide.numero()?;
    let seconda_misura = campi_ellissoide.numero()?;
    let eu = unita(campi_ellissoide.nodo(&["LENGTHUNIT"])?)?;
    let eid = campi_ellissoide.id()?;
    campi_ellissoide.fine()?;
    let accuratezza = if ensemble {
        let mut campi_accuratezza = Campi::nuovo(campi_datum.nodo(&["ENSEMBLEACCURACY"])?);
        let valore = campi_accuratezza.numero()?;
        campi_accuratezza.fine()?;
        Some(valore)
    } else {
        None
    };
    let id = campi_datum.id()?;
    campi_datum.fine()?;
    let mut campi_meridiano = Campi::nuovo(campi.nodo(&["PRIMEM", "PRIMEMERIDIAN"])?);
    let pnome = campi_meridiano.testo()?;
    let longitudine = campi_meridiano.numero()?;
    let pu = unita(campi_meridiano.nodo(&["ANGLEUNIT"])?)?;
    let pid = campi_meridiano.id()?;
    campi_meridiano.fine()?;
    Ok(Datum {
        identita: Identita { id, nome },
        tipo: if ensemble {
            "DatumEnsemble"
        } else {
            "GeodeticReferenceFrame"
        }
        .into(),
        membri,
        accuratezza,
        ellissoide: Ellissoide {
            identita: Identita {
                nome: enome,
                id: eid,
            },
            semiasse,
            forma: "inverse_flattening".into(),
            seconda_misura,
            unita: eu,
        },
        meridiano: Meridiano {
            identita: Identita {
                nome: pnome,
                id: pid,
            },
            longitudine,
            unita: pu,
        },
    })
}

fn conversione(n: &Nodo) -> Esito<Conversione> {
    let mut c = Campi::nuovo(n);
    c.testo()?;
    let metodo = identita(c.nodo(&["METHOD"])?)?;
    let mut parametri = Vec::new();
    while let Some(n) = c.opzionale(&["PARAMETER"]) {
        let mut p = Campi::nuovo(n);
        let nome = p.testo()?;
        let valore = p.numero()?;
        let unita = unita(p.nodo(&["ANGLEUNIT", "LENGTHUNIT", "SCALEUNIT"])?)?;
        let id = p.id()?;
        p.fine()?;
        parametri.push(Parametro {
            identita: Identita { id, nome },
            valore,
            unita,
        });
    }
    let id = c.id()?;
    c.fine()?;
    Ok(Conversione {
        id,
        metodo,
        parametri,
    })
}

fn ambito(c: &mut Campi<'_>) -> Esito<()> {
    for nome in ["SCOPE", "AREA"] {
        if let Some(n) = c.opzionale(&[nome]) {
            let mut v = Campi::nuovo(n);
            v.testo()?;
            v.fine()?;
        }
    }
    if let Some(n) = c.opzionale(&["BBOX"]) {
        let mut v = Campi::nuovo(n);
        for _ in 0..4 {
            v.numero()?;
        }
        v.fine()?;
    }
    Ok(())
}

fn modello(n: &Nodo) -> Esito<Definizione> {
    let proiettato = match n.nome.as_str() {
        "GEOGCRS" | "GEOGRAPHICCRS" | "GEODCRS" | "GEODETICCRS" => false,
        "PROJCRS" | "PROJECTEDCRS" => true,
        _ => return Err(Rifiuto::NonIntegrato),
    };
    let mut c = Campi::nuovo(n);
    c.testo()?;
    let (mut base, conversione, sistema) = if proiettato {
        let mut b = Campi::nuovo(c.nodo(&["BASEGEOGCRS", "BASEGEODCRS"])?);
        b.testo()?;
        let datum = datum(&mut b)?;
        let id = b.id()?;
        b.fine()?;
        // WKT2 omette il CS del CRS di base. Il profilo lo rappresenta
        // internamente in lat/lon, gradi; non riguarda le ordinate dei dati.
        let sistema_base = Sistema {
            tipo: "ellipsoidal".into(),
            assi: vec![
                Asse {
                    direzione: "north".into(),
                    ordine: 1,
                    unita: grado(),
                },
                Asse {
                    direzione: "east".into(),
                    ordine: 2,
                    unita: grado(),
                },
            ],
        };
        let base = Base {
            id,
            datum,
            sistema: sistema_base,
        };
        let conversione = conversione(c.nodo(&["CONVERSION"])?)?;
        let sistema = sistema(&mut c)?;
        (base, Some(conversione), sistema)
    } else {
        let datum = datum(&mut c)?;
        let sistema = sistema(&mut c)?;
        (
            Base {
                id: None,
                datum,
                sistema: sistema.clone(),
            },
            None,
            sistema,
        )
    };
    let mut usages = false;
    while let Some(n) = c.opzionale(&["USAGE"]) {
        usages = true;
        let mut u = Campi::nuovo(n);
        // USAGE richiede uno scope e almeno un'estensione; il profilo
        // ammette AREA e BBOX, non estensioni verticali o temporali.
        let mut scope = Campi::nuovo(u.nodo(&["SCOPE"])?);
        scope.testo()?;
        scope.fine()?;
        if !matches!(u.resto.first(), Some(Valore::Nodo(n)) if ["AREA", "BBOX"].contains(&n.nome.as_str()))
        {
            return Err(Rifiuto::NonIntegrato);
        }
        ambito(&mut u)?;
        u.fine()?;
    }
    if !usages {
        ambito(&mut c)?;
    }
    let id = c.id()?;
    if let Some(n) = c.opzionale(&["REMARK"]) {
        let mut r = Campi::nuovo(n);
        r.testo()?;
        r.fine()?;
    }
    c.fine()?;
    if !proiettato {
        base.id.clone_from(&id);
    }
    Ok(Definizione {
        id,
        tipo: if proiettato {
            "ProjectedCRS"
        } else {
            "GeographicCRS"
        }
        .into(),
        base,
        conversione,
        sistema,
    })
}

pub(super) fn leggi(testo: &str) -> Esito<Definizione> {
    if testo.len() > crate::crs::MAX_CRS_DEFINITION_BYTES {
        return Err(Rifiuto::NumeroInvalido);
    }
    let mut lexer = Lexer {
        resto: testo,
        nodi: 0,
    };
    let v = lexer.valore(0)?;
    lexer.spazio();
    if !lexer.resto.is_empty() {
        return Err(Rifiuto::NumeroInvalido);
    }
    match v {
        Valore::Nodo(n) if matches!(n.nome.as_str(), "GEOGCS" | "PROJCS") => wkt1::modello(&n),
        Valore::Nodo(n) => modello(&n),
        _ => Err(Rifiuto::NumeroInvalido),
    }
}
