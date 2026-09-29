//! CRS di `GeoParquet`: dal `PROJJSON` all'identificatore integrato e ritorno.
//!
//! In lettura il `PROJJSON` di una colonna si riconduce all'identificatore
//! della tabella dei CRS integrati **solo tramite il suo `id`**:
//! `{"authority": "EPSG", "code": <n>}` → `EPSG:<n>`,
//! `{"authority": "OGC", "code": "CRS84"}` → `OGC:CRS84`. Un documento senza
//! `id`, con un'altra autorità o con un codice fuori tabella fallisce con
//! `CRS_NOT_BUILTIN`: il documento non si interpreta e il CRS non si
//! indovina. Il tipo del documento (`GeographicCRS`/`ProjectedCRS`) deve
//! essere quello del CRS integrato; il resto del documento non si confronta
//! (limite dichiarato nel README).
//!
//! In scrittura il documento è il `PROJJSON` completo che PROJ 9.5.1 produce
//! per quel CRS (`data/projjson_integrati.json`, generato da
//! `scripts/genera_projjson_integrati.py`), non il canonical ridotto di
//! `plenora-core`, che PROJ non rileggerebbe.

use std::sync::OnceLock;

use plenora_core::crs::{resolve_crs, CrsError, CrsKind, ResolvedCrs};
use plenora_core::{PlenoraError, Result};
use serde_json::{Map, Value};

/// La tabella generata: identificatore → `PROJJSON`.
const TESTO: &str = include_str!("../data/projjson_integrati.json");

fn tabella() -> Result<&'static Map<String, Value>> {
    static TABELLA: OnceLock<Option<Map<String, Value>>> = OnceLock::new();
    TABELLA
        .get_or_init(|| match serde_json::from_str(TESTO) {
            Ok(Value::Object(mappa)) => Some(mappa),
            _ => None,
        })
        .as_ref()
        .ok_or_else(|| PlenoraError::Internal("tabella PROJJSON integrata illeggibile".to_owned()))
}

fn non_integrato() -> PlenoraError {
    PlenoraError::from(CrsError::NotBuiltin)
}

/// Gli identificatori della tabella `PROJJSON`, nel suo ordine.
///
/// # Errors
///
/// `Internal` se la tabella incorporata non si legge.
pub fn identificatori() -> Result<Vec<String>> {
    Ok(tabella()?.keys().cloned().collect())
}

/// Il `PROJJSON` completo di un identificatore integrato.
///
/// # Errors
///
/// `Crs` (`CRS_NOT_BUILTIN`) se l'identificatore non è in tabella.
pub fn projjson_di(identificativo: &str) -> Result<Value> {
    tabella()?
        .get(identificativo)
        .cloned()
        .ok_or_else(non_integrato)
}

/// L'identificatore canonico (`EPSG:<n>` o `OGC:CRS84`) di un CRS risolto,
/// verificato risolvendolo di nuovo e confrontando i canonical.
///
/// # Errors
///
/// `Crs` (`CRS_NOT_BUILTIN`) se il CRS non è uno della tabella integrata.
pub fn identificativo_di(crs: &ResolvedCrs) -> Result<String> {
    let candidato = match crs.authority_identifier() {
        Some((autorita, codice)) if autorita.eq_ignore_ascii_case("EPSG") => {
            format!("EPSG:{codice}")
        }
        Some(_) => return Err(non_integrato()),
        None => "OGC:CRS84".to_owned(),
    };
    let risolto = resolve_crs(&candidato, "crs")?;
    if !risolto.semantically_equals(crs) {
        return Err(non_integrato());
    }
    Ok(candidato)
}

/// L'identificatore integrato di un `PROJJSON` letto da un file `GeoParquet`.
///
/// # Errors
///
/// `Crs` (`CRS_NOT_BUILTIN`) per un documento senza `id` riconoscibile o
/// fuori tabella; `Crs` se il tipo del documento non è quello del CRS
/// integrato; `InvalidPlan` se il valore non è un oggetto.
pub fn identificativo_da_projjson(documento: &Value) -> Result<String> {
    let Value::Object(oggetto) = documento else {
        return Err(PlenoraError::InvalidPlan(
            "metadato `geo`: `crs` deve essere un oggetto PROJJSON o null".to_owned(),
        ));
    };
    let id = match (oggetto.get("id"), oggetto.get("ids")) {
        (Some(id), None) => id,
        (None, Some(Value::Array(ids))) if ids.len() == 1 => &ids[0],
        _ => return Err(non_integrato()),
    };
    let autorita = id
        .get("authority")
        .and_then(Value::as_str)
        .ok_or_else(non_integrato)?;
    let codice = match id.get("code") {
        Some(Value::Number(numero)) => numero.as_u64().map(|n| n.to_string()),
        Some(Value::String(testo)) => Some(testo.clone()),
        _ => None,
    }
    .ok_or_else(non_integrato)?;
    let identificativo = match autorita {
        "EPSG" if !codice.is_empty() && codice.bytes().all(|b| b.is_ascii_digit()) => {
            format!("EPSG:{codice}")
        }
        "OGC" if codice == "CRS84" => "OGC:CRS84".to_owned(),
        _ => return Err(non_integrato()),
    };
    let risolto = resolve_crs(&identificativo, "crs")?;
    let tipo_atteso = match risolto.kind() {
        CrsKind::Geographic => "GeographicCRS",
        CrsKind::Projected => "ProjectedCRS",
    };
    if oggetto.get("type").and_then(Value::as_str) != Some(tipo_atteso) {
        return Err(PlenoraError::Crs(format!(
            "CRS_NOT_BUILTIN: il PROJJSON di `{identificativo}` non e' di tipo {tipo_atteso}"
        )));
    }
    Ok(identificativo)
}

#[cfg(test)]
mod tests {
    use super::*;
    use plenora_core::crs::builtin_crs_identifiers;

    #[test]
    fn la_tabella_copre_esattamente_i_crs_integrati() {
        let mut attesi: Vec<String> = builtin_crs_identifiers().collect();
        let mut trovati = identificatori().expect("tabella");
        attesi.sort();
        trovati.sort();
        assert_eq!(trovati, attesi);
    }

    #[test]
    fn ogni_documento_torna_al_suo_identificatore() {
        for identificativo in identificatori().expect("tabella") {
            let documento = projjson_di(&identificativo).expect("documento");
            assert_eq!(
                identificativo_da_projjson(&documento).expect("rilettura"),
                identificativo
            );
            let risolto = resolve_crs(&identificativo, "crs").expect("risolto");
            assert_eq!(identificativo_di(&risolto).expect("id"), identificativo);
        }
    }

    #[test]
    fn forme_non_canoniche_riportano_all_identificatore() {
        let urn = resolve_crs("urn:ogc:def:crs:EPSG::32632", "crs").expect("urn");
        assert_eq!(identificativo_di(&urn).expect("id"), "EPSG:32632");
        let documento = serde_json::json!({
            "type": "ProjectedCRS", "ids": [{"authority": "EPSG", "code": "32632"}]
        });
        assert_eq!(
            identificativo_da_projjson(&documento).expect("ids"),
            "EPSG:32632"
        );
    }

    #[test]
    fn rifiuti() {
        for documento in [
            serde_json::json!({"type": "ProjectedCRS"}),
            serde_json::json!({"type": "ProjectedCRS", "id": {"authority": "ESRI", "code": 102_100}}),
            serde_json::json!({"type": "ProjectedCRS", "id": {"authority": "EPSG", "code": 99_999}}),
            serde_json::json!({"type": "GeographicCRS", "id": {"authority": "EPSG", "code": 4979}}),
            serde_json::json!({"type": "GeographicCRS", "id": {"authority": "EPSG", "code": 32632}}),
            serde_json::json!({"type": "ProjectedCRS", "id": {"authority": "EPSG", "code": -1}}),
            serde_json::json!({"type": "BoundCRS", "id": {"authority": "EPSG", "code": 4326}}),
            serde_json::json!({"ids": [{"authority": "EPSG", "code": 4326}, {"authority": "EPSG", "code": 4326}]}),
        ] {
            let errore = identificativo_da_projjson(&documento).expect_err("rifiutato");
            assert!(
                errore.to_string().contains("CRS_NOT_BUILTIN"),
                "{documento}: {errore}"
            );
        }
        assert!(identificativo_da_projjson(&Value::String("EPSG:4326".to_owned())).is_err());
    }
}
