//! CRS di `GeoParquet`: dal `PROJJSON` all'identificatore integrato e ritorno.
//!
//! In lettura il validatore comune del core confronta tutta la semantica
//! con il candidato individuato dall'ID radice. ID e tipo da soli non
//! bastano: ogni discordanza o costrutto fuori profilo viene rifiutato.
//!
//! In scrittura il documento è il `PROJJSON` completo che PROJ 9.5.1 produce
//! per quel CRS (`data/projjson_integrati.json`, generato da
//! `scripts/genera_projjson_integrati.py`), non il canonical ridotto di
//! `plenora-core`, che PROJ non rileggerebbe.

use std::sync::OnceLock;

use plenora_core::crs::{resolve_crs, CrsError, ResolvedCrs};
use plenora_core::{PlenoraError, Result};
use serde_json::{Map, Value};

/// La tabella generata: identificatore → `PROJJSON`.
const TESTO: &str = include_str!("../data/projjson_integrati.json");

fn tabella() -> Result<&'static Map<String, Value>> {
    static TABELLA: OnceLock<Option<Map<String, Value>>> = OnceLock::new();
    TABELLA
        .get_or_init(|| leggi_tabella(TESTO))
        .as_ref()
        .ok_or_else(|| PlenoraError::Internal("tabella PROJJSON integrata illeggibile".to_owned()))
}

fn leggi_tabella(testo: &str) -> Option<Map<String, Value>> {
    plenora_core::json::ensure_no_duplicate_keys(testo).ok()?;
    plenora_core::json::ensure_numbers_exact(testo).ok()?;
    match serde_json::from_str(testo) {
        Ok(Value::Object(mappa)) => Some(mappa),
        _ => None,
    }
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
/// fuori tabella; `Crs` se il documento differisce da quello integrato;
/// `InvalidPlan` se il valore non e' un oggetto.
pub fn identificativo_da_projjson(documento: &Value) -> Result<String> {
    if !documento.is_object() {
        return Err(PlenoraError::InvalidPlan(
            "metadato `geo`: `crs` deve essere un oggetto PROJJSON o null".into(),
        ));
    }
    let risolto = resolve_crs(&documento.to_string(), "crs")?;
    identificativo_di(&risolto)
}

#[cfg(test)]
mod tests {
    #[test]
    fn catalogo_rifiuta_duplicati_in_superficie_e_annidati() {
        for testo in [
            r#"{"EPSG:4326": {}, "EPSG:4326": {}}"#,
            r#"{"EPSG:4326": {"id": {"code": 4326, "code": 4326}}}"#,
        ] {
            assert!(super::leggi_tabella(testo).is_none());
        }
        assert!(super::leggi_tabella(super::TESTO).is_some());
    }

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
                identificativo_da_projjson(&documento)
                    .unwrap_or_else(|e| panic!("{identificativo}: {e}")),
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
        let mut documento = projjson_di("EPSG:32632").unwrap();
        documento.as_object_mut().unwrap().remove("id");
        documento["ids"] = serde_json::json!([{"authority": "EPSG", "code": "32632"}]);
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
            // Il codice pubblico esce tipizzato su ogni ramo, anche su quello
            // del tipo sbagliato che compone il proprio messaggio.
            assert_eq!(
                errore.public_projection().code(),
                Some("CRS_NOT_BUILTIN"),
                "{documento}"
            );
            // Errori senza dati: né il codice né l'autorità né il tipo letti
            // dal metadato compaiono nel messaggio (pubblico o locale).
            for testo in [
                errore.to_string(),
                errore.public_projection().message().to_owned(),
            ] {
                for valore in [
                    "32632",
                    "4979",
                    "99999",
                    "102100",
                    "ESRI",
                    "GeographicCRS",
                    "BoundCRS",
                ] {
                    assert!(!testo.contains(valore), "{documento}: {testo}");
                }
            }
        }
        assert!(identificativo_da_projjson(&Value::String("EPSG:4326".to_owned())).is_err());
    }
}
