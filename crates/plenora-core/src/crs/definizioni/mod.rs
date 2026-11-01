//! Semantica interna, indipendente dal formato testuale e dalle ordinate.
//!
//! I numeri sono token decimali: nessuna conversione a double nel confronto.
//! Gli alias WKT1 sono finiti; gli altri formati usano i nomi del registro.
//! I parser devono rifiutare costrutti non rappresentati da questo modello.

mod confronto;
mod numeri;
mod projjson;
#[cfg(test)]
mod tests;
mod wkt;
#[cfg(test)]
mod wkt1_tests;
#[cfg(test)]
mod wkt_tests;

use serde::{Deserialize, Serialize};

use super::integrati::Identificativo;

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Id {
    pub authority: String,
    pub code: String,
}

/// Il nome identifica solo in assenza di ID; con ID e' descrittivo.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Identita {
    pub id: Option<Id>,
    pub nome: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Unita {
    pub identita: Identita,
    pub tipo: String,
    /// Metri, radianti o unita' adimensionale. Forma decimale ammessa,
    /// non un'approssimazione con tolleranza del fattore angolare.
    pub fattore: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Asse {
    pub direzione: String,
    pub ordine: u32,
    pub unita: Unita,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Sistema {
    pub tipo: String,
    pub assi: Vec<Asse>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Ellissoide {
    pub identita: Identita,
    pub semiasse: String,
    pub forma: String,
    pub seconda_misura: String,
    pub unita: Unita,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Meridiano {
    pub identita: Identita,
    pub longitudine: String,
    pub unita: Unita,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Datum {
    pub identita: Identita,
    pub tipo: String,
    pub membri: Vec<Identita>,
    pub accuratezza: Option<String>,
    pub ellissoide: Ellissoide,
    pub meridiano: Meridiano,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Base {
    pub id: Option<Id>,
    pub datum: Datum,
    pub sistema: Sistema,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Parametro {
    pub identita: Identita,
    pub valore: String,
    pub unita: Unita,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Conversione {
    pub id: Option<Id>,
    pub metodo: Identita,
    pub parametri: Vec<Parametro>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Definizione {
    pub id: Option<Id>,
    pub tipo: String,
    pub base: Base,
    pub conversione: Option<Conversione>,
    pub sistema: Sistema,
}

/// Motivi chiusi, senza valori provenienti dal documento.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Rifiuto {
    NonIntegrato,
    NumeroInvalido,
    TabellaInvalida,
}

impl Rifiuto {
    #[cfg(test)]
    pub const fn codice(self) -> &'static str {
        match self {
            Self::NonIntegrato => "CRS_NOT_BUILTIN",
            Self::NumeroInvalido => "CRS_INVALID",
            Self::TabellaInvalida => "CRS_CONTRACT_INVALID",
        }
    }
}

/// Il JSON incorporato e' una codifica del modello Rust, non PROJJSON.
/// Un errore nei dati generati non diventa un mancato riconoscimento.
fn tabella() -> Result<Vec<Definizione>, Rifiuto> {
    serde_json::from_str(include_str!("semantica.json")).map_err(|_| Rifiuto::TabellaInvalida)
}

pub(super) fn riconosci(definizione: &Definizione) -> Result<Identificativo, Rifiuto> {
    let id = definizione.id.as_ref().ok_or(Rifiuto::NonIntegrato)?;
    let identificativo = match (id.authority.as_str(), id.code.as_str()) {
        ("EPSG", codice) => {
            let numero = codice.parse::<u32>().map_err(|_| Rifiuto::NonIntegrato)?;
            if numero.to_string() != codice {
                return Err(Rifiuto::NonIntegrato);
            }
            Identificativo::Epsg(numero)
        }
        ("OGC", "CRS84") => Identificativo::OgcCrs84,
        _ => return Err(Rifiuto::NonIntegrato),
    };
    super::integrati::cerca(identificativo).ok_or(Rifiuto::NonIntegrato)?;
    let tabella = tabella()?;
    let attesa = tabella
        .iter()
        .find(|voce| voce.id.as_ref() == Some(id))
        .ok_or(Rifiuto::TabellaInvalida)?;
    confronto::confronta(definizione, attesa)?;
    Ok(identificativo)
}

/// Risolve i profili testuali conservando il documento originale.
pub(super) fn risolvi(
    testo: &str,
    name: &'static str,
) -> Result<super::ResolvedCrs, super::CrsError> {
    let modello = if testo.trim_start().starts_with('{') {
        projjson::leggi(testo)
    } else {
        wkt::leggi(testo)
    };
    let esito = modello.and_then(|d| riconosci(&d));
    let id = esito.map_err(|e| match e {
        Rifiuto::NonIntegrato => super::CrsError::NotBuiltin,
        Rifiuto::NumeroInvalido => super::CrsError::InvalidDefinition {
            name,
            reason: "definizione invalida, ambigua, numero non esatto o limiti superati".into(),
        },
        Rifiuto::TabellaInvalida => {
            super::CrsError::InvalidContract("tabella semantica integrata illeggibile")
        }
    })?;
    super::integrati::risolvi_verificato(id, testo).ok_or(super::CrsError::InvalidContract(
        "tabella integrata incoerente",
    ))
}
