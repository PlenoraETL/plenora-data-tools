//! Risolutore dei CRS integrati.
//!
//! Riconosce gli identificatori d'autorita' dei CRS di
//! [`super::epsg_integrati`] e ne costruisce il [`ResolvedCrs`]: canonical
//! PROJJSON ridotto (tipo, nome, sistema di coordinate, `id`), tipo, unita'
//! lineare, area d'uso EPSG, dominio di validita', ellissoide. Nessuna
//! trasformazione di coordinate.

use serde_json::{json, Value};

use super::epsg_integrati::{CRS84, EPSG};
use super::{AreaOfUse, CrsKind, Ellipsoid, GeographicBounds, ProjectedBounds, ResolvedCrs};

/// Identita' di un CRS della tabella.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Identificativo {
    Epsg(u32),
    OgcCrs84,
}

/// Direzione di un asse d'autorita'. La tabella ha solo assi nord/est: il
/// generatore rifiuta ogni altra direzione.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Direzione {
    North,
    East,
}

impl Direzione {
    const fn projjson(self) -> &'static str {
        match self {
            Self::North => "north",
            Self::East => "east",
        }
    }
}

/// Un asse del sistema di coordinate, come lo nomina il registro.
#[derive(Clone, Copy, Debug)]
pub(super) struct Asse {
    pub(super) nome: &'static str,
    pub(super) abbreviazione: &'static str,
    pub(super) direzione: Direzione,
}

/// Una riga della tabella.
#[derive(Clone, Copy, Debug)]
pub(super) struct CrsIntegrato {
    pub(super) identificativo: Identificativo,
    pub(super) nome: &'static str,
    pub(super) kind: CrsKind,
    /// Assi nell'ordine dell'autorita'.
    pub(super) assi: [Asse; 2],
    pub(super) ellissoide: Ellipsoid,
    pub(super) area: AreaOfUse,
    /// Dominio di validita' dei proiettati; `None` per i geografici.
    pub(super) dominio: Option<ProjectedBounds>,
}

/// Riga di un CRS geografico (unita' degli assi: grado).
pub(super) const fn geografico(
    identificativo: Identificativo,
    nome: &'static str,
    assi: [Asse; 2],
    ellissoide: Ellipsoid,
    area: GeographicBounds,
) -> CrsIntegrato {
    CrsIntegrato {
        identificativo,
        nome,
        kind: CrsKind::Geographic,
        assi,
        ellissoide,
        area: AreaOfUse {
            geographic: area,
            projected: None,
        },
        dominio: None,
    }
}

/// Riga di un CRS proiettato (unita' degli assi: metro).
pub(super) const fn proiettato(
    identificativo: Identificativo,
    nome: &'static str,
    assi: [Asse; 2],
    ellissoide: Ellipsoid,
    area: GeographicBounds,
    area_proiettata: ProjectedBounds,
    dominio: ProjectedBounds,
) -> CrsIntegrato {
    CrsIntegrato {
        identificativo,
        nome,
        kind: CrsKind::Projected,
        assi,
        ellissoide,
        area: AreaOfUse {
            geographic: area,
            projected: Some(area_proiettata),
        },
        dominio: Some(dominio),
    }
}

/// Riconosce un identificatore d'autorita' della tabella.
///
/// Forme: `AUT:CODICE` e `urn:ogc:def:crs:AUT:VERSIONE:CODICE` (versione
/// vuota o numerica, [`versione_urn`]), con `AUT`
/// `EPSG` (codice decimale senza zeri iniziali) oppure `OGC` (codice
/// `CRS84`), senza distinzione di maiuscole su prefissi e autorita'. Ogni
/// altra forma da' `None`.
pub(super) fn identificativo(definizione: &str) -> Option<Identificativo> {
    let parti: Vec<&str> = definizione.split(':').collect();
    match parti.as_slice() {
        [autorita, codice] => da_autorita(autorita, codice),
        [urn, ogc, def, crs, autorita, versione, codice]
            if urn.eq_ignore_ascii_case("urn")
                && ogc.eq_ignore_ascii_case("ogc")
                && def.eq_ignore_ascii_case("def")
                && crs.eq_ignore_ascii_case("crs")
                && versione_urn(versione) =>
        {
            da_autorita(autorita, codice)
        }
        _ => None,
    }
}

/// Versione di un URN OGC: vuota, oppure numeri separati da un solo punto
/// (`9.9.1`), senza punti iniziali, finali o doppi.
fn versione_urn(versione: &str) -> bool {
    versione.is_empty()
        || versione
            .split('.')
            .all(|parte| !parte.is_empty() && parte.bytes().all(|byte| byte.is_ascii_digit()))
}

fn da_autorita(autorita: &str, codice: &str) -> Option<Identificativo> {
    if autorita.eq_ignore_ascii_case("EPSG") {
        codice_decimale(codice).map(Identificativo::Epsg)
    } else if autorita.eq_ignore_ascii_case("OGC") && codice.eq_ignore_ascii_case("CRS84") {
        Some(Identificativo::OgcCrs84)
    } else {
        None
    }
}

/// Codice decimale canonico: solo cifre, niente zeri iniziali, entro `u32`.
fn codice_decimale(codice: &str) -> Option<u32> {
    if codice.is_empty()
        || !codice.bytes().all(|byte| byte.is_ascii_digit())
        || (codice.len() > 1 && codice.starts_with('0'))
    {
        return None;
    }
    codice.parse().ok()
}

/// La riga della tabella per un identificativo.
pub(super) fn cerca(identificativo: Identificativo) -> Option<&'static CrsIntegrato> {
    match identificativo {
        Identificativo::OgcCrs84 => Some(&CRS84),
        Identificativo::Epsg(codice) => {
            let indice = EPSG
                .binary_search_by_key(&codice, |voce| match voce.identificativo {
                    Identificativo::Epsg(codice) => codice,
                    // Le righe EPSG hanno solo identificativi EPSG (test
                    // `la_tabella_e_ordinata_e_senza_doppioni`).
                    Identificativo::OgcCrs84 => 0,
                })
                .ok()?;
            EPSG.get(indice)
        }
    }
}

/// Risolve una definizione d'autorita' della tabella.
pub(super) fn risolvi(definizione: &str) -> Option<ResolvedCrs> {
    let voce = cerca(identificativo(definizione)?)?;
    Some(ResolvedCrs {
        definition: definizione.to_owned(),
        canonical: canonico(voce),
        kind: voce.kind,
        horizontal_unit_to_metre: match voce.kind {
            CrsKind::Geographic => None,
            CrsKind::Projected => Some(1.0),
        },
        area_of_use: Some(voce.area),
        validity_domain: voce.dominio,
        ellipsoid: Some(voce.ellissoide),
        integrato: Some(voce.identificativo),
    })
}

/// Identificatori canonici di tutta la tabella.
pub(super) fn identificatori() -> impl Iterator<Item = String> {
    std::iter::once(&CRS84)
        .chain(EPSG.iter())
        .map(|voce| testo_identificativo(voce.identificativo))
}

pub(super) fn testo_identificativo(identificativo: Identificativo) -> String {
    match identificativo {
        Identificativo::Epsg(codice) => format!("EPSG:{codice}"),
        Identificativo::OgcCrs84 => "OGC:CRS84".to_owned(),
    }
}

/// Canonical PROJJSON ridotto: le stesse chiavi e forme che PROJ emette per
/// tipo, nome, sistema di coordinate e `id`, cosi' `authority_axis_order` e
/// `authority_identifier` lo leggono come un PROJJSON completo.
fn canonico(voce: &CrsIntegrato) -> Value {
    let (tipo, sottotipo, unita) = match voce.kind {
        CrsKind::Geographic => ("GeographicCRS", "ellipsoidal", "degree"),
        CrsKind::Projected => ("ProjectedCRS", "Cartesian", "metre"),
    };
    let assi: Vec<Value> = voce
        .assi
        .iter()
        .map(|asse| {
            json!({
                "name": asse.nome,
                "abbreviation": asse.abbreviazione,
                "direction": asse.direzione.projjson(),
                "unit": unita,
            })
        })
        .collect();
    let id = match voce.identificativo {
        Identificativo::Epsg(codice) => json!({"authority": "EPSG", "code": codice}),
        Identificativo::OgcCrs84 => json!({"authority": "OGC", "code": "CRS84"}),
    };
    json!({
        "type": tipo,
        "name": voce.nome,
        "coordinate_system": {"subtype": sottotipo, "axis": assi},
        "id": id,
    })
}

#[cfg(test)]
mod tests;
