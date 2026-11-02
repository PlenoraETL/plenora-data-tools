//! Oracolo condiviso tra libFuzzer e le regressioni deterministiche del core.

use crate::esiti;
use plenora_core::crs::{resolve_crs, CrsError, MAX_CRS_DEFINITION_BYTES};
use plenora_core::{ErrorCategory, PlenoraError};

pub fn verifica(testo: &str) {
    let primo = resolve_crs(testo, "crs");
    let secondo = resolve_crs(testo, "crs");
    match (primo, secondo) {
        (Ok(a), Ok(b)) => {
            assert!(testo.len() <= MAX_CRS_DEFINITION_BYTES);
            assert!(!testo.contains('\0'));
            assert_eq!(a.definition(), testo);
            assert_eq!(format!("{a:?}"), format!("{b:?}"));
            let identificatore = a.authority_identifier().map_or_else(
                || "OGC:CRS84".to_owned(),
                |(autorita, codice)| format!("{autorita}:{codice}"),
            );
            let integrato = resolve_crs(&identificatore, "crs").unwrap();
            assert!(a.semantically_equals(&integrato));
            assert_eq!(a.area_of_use(), integrato.area_of_use());
            assert_eq!(a.validity_domain(), integrato.validity_domain());
            assert_eq!(a.ellipsoid(), integrato.ellipsoid());
            assert_eq!(a.authority_axis_order(), integrato.authority_axis_order());
        }
        (Err(a), Err(b)) => {
            assert_eq!(a.code(), b.code());
            // Una whitelist completa intercetta anche dati diversi dalla sentinella.
            match &a {
                CrsError::Required { name } => assert_eq!(*name, "crs"),
                CrsError::InvalidDefinition { name, reason } => {
                    assert_eq!(*name, "crs");
                    assert!(reason == "contiene NUL"
                        || reason == "definizione invalida, ambigua, numero non esatto o limiti superati"
                        || reason == &format!("oltre il limite di {MAX_CRS_DEFINITION_BYTES} byte: {}", testo.len()));
                }
                CrsError::NotBuiltin | CrsError::BackendUnavailable => {}
                _ => panic!("errore estraneo alla risoluzione"),
            }
            let a: PlenoraError = a.into();
            let b: PlenoraError = b.into();
            assert_ne!(a.category(), ErrorCategory::Internal);
            esiti::stesso_errore("CRS", &a, &b);
            esiti::senza_sentinella("CRS", &a);
        }
        _ => panic!("risoluzione non deterministica"),
    }
    if !testo.trim().is_empty() && (testo.len() > MAX_CRS_DEFINITION_BYTES || testo.contains('\0'))
    {
        assert_eq!(
            resolve_crs(testo, "crs").unwrap_err().code().as_str(),
            "CRS_INVALID"
        );
    }
}
