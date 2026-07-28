//! Fail-closed coordinate-reference-system contract for public adapters.
//!
//! Coordinates alone never identify a CRS reliably.  The caller must provide
//! a definition and, when the PROJ backend is enabled, this module resolves it
//! against the bundled database before a spatial kernel is allowed to run.
//!
//! Port Fase 1 da `crs.rs` di plenora-geo-tools-arrow. `plenora-core` non ha
//! la feature `proj-backend`: qui vive il contratto CRS backend-indipendente,
//! mentre la risoluzione PROJ (che costruisce [`ResolvedCrs`] dal database
//! bundled) resta in `plenora-kernels-geo` dietro la feature `proj-backend`.
//! Senza backend [`resolve_crs`] fallisce chiuso, come nel sorgente.

use crate::catalog::CrsRequirement;
use crate::error::PlenoraError;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

pub const MAX_CRS_DEFINITION_BYTES: usize = 64 * 1024;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CrsKind {
    Geographic,
    Projected,
}

#[derive(Clone, Debug)]
pub struct ResolvedCrs {
    definition: String,
    canonical: Value,
    kind: CrsKind,
    horizontal_unit_to_metre: Option<f64>,
}

impl ResolvedCrs {
    /// Costruisce un CRS gia' risolto e verificato.
    ///
    /// Riservato ai backend di risoluzione (`plenora-kernels-geo`, feature
    /// `proj-backend`) e ai test: il contratto resta che solo una risoluzione
    /// contro il database PROJ puo' produrre questi valori.
    #[must_use]
    pub const fn from_resolved_parts(
        definition: String,
        canonical: Value,
        kind: CrsKind,
        horizontal_unit_to_metre: Option<f64>,
    ) -> Self {
        Self {
            definition,
            canonical,
            kind,
            horizontal_unit_to_metre,
        }
    }

    #[must_use]
    pub fn definition(&self) -> &str {
        &self.definition
    }

    #[must_use]
    pub const fn kind(&self) -> CrsKind {
        self.kind
    }

    #[must_use]
    pub const fn horizontal_unit_to_metre(&self) -> Option<f64> {
        self.horizontal_unit_to_metre
    }

    #[must_use]
    pub fn semantically_equals(&self, other: &Self) -> bool {
        self.canonical == other.canonical
    }
}

#[derive(Debug, Error)]
pub enum CrsError {
    #[error("CRS_REQUIRED: {name} e' obbligatorio")]
    Required { name: &'static str },
    #[error("CRS_INVALID: {name}: {reason}")]
    InvalidDefinition { name: &'static str, reason: String },
    #[error("CRS_BACKEND_UNAVAILABLE: la validazione CRS richiede il backend PROJ")]
    BackendUnavailable,
    #[error("CRS_TYPE_UNSUPPORTED: tipo PROJJSON {0} non supportato")]
    UnsupportedType(String),
    #[error(
        "LINEAR_UNIT_REQUIRED: il CRS proiettato non dichiara un'unita' lineare orizzontale valida"
    )]
    MissingLinearUnit,
    #[error("PROJECTED_CRS_REQUIRED: ricevuto CRS {actual:?}")]
    ProjectedRequired { actual: CrsKind },
    #[error("GEOGRAPHIC_CRS_REQUIRED: ricevuto CRS {actual:?}")]
    GeographicRequired { actual: CrsKind },
    #[error("CRS_MISMATCH: gli input non usano lo stesso CRS")]
    Mismatch,
    #[error("COORDINATE_OUT_OF_CRS_DOMAIN: ({x}, {y}) non e' longitude/latitude valida")]
    CoordinateOutOfDomain { x: f64, y: f64 },
    #[error("CRS_CONTRACT_INVALID: {0}")]
    InvalidContract(&'static str),
}

impl From<CrsError> for PlenoraError {
    fn from(error: CrsError) -> Self {
        Self::Crs(error.to_string())
    }
}

/// Definizione CRS obbligatoria e testualmente valida.
///
/// # Errors
///
/// Restituisce [`CrsError::Required`] se la definizione manca o e' vuota e
/// [`CrsError::InvalidDefinition`] se supera [`MAX_CRS_DEFINITION_BYTES`] o
/// contiene NUL.
pub fn required_definition<'a>(
    value: Option<&'a str>,
    name: &'static str,
) -> Result<&'a str, CrsError> {
    let value = value.ok_or(CrsError::Required { name })?;
    validate_definition_text(value, name)?;
    Ok(value)
}

fn validate_definition_text(value: &str, name: &'static str) -> Result<(), CrsError> {
    if value.trim().is_empty() {
        return Err(CrsError::Required { name });
    }
    if value.len() > MAX_CRS_DEFINITION_BYTES {
        return Err(CrsError::InvalidDefinition {
            name,
            reason: format!(
                "oltre il limite di {MAX_CRS_DEFINITION_BYTES} byte: {}",
                value.len()
            ),
        });
    }
    if value.contains('\0') {
        return Err(CrsError::InvalidDefinition {
            name,
            reason: "contiene NUL".to_owned(),
        });
    }
    Ok(())
}

/// Risoluzione fail-closed senza backend PROJ.
///
/// La risoluzione reale (PROJJSON canonico, classificazione geographic/
/// projected, unita' lineare) vive in `plenora-kernels-geo` dietro la feature
/// `proj-backend`; senza backend nessuna dichiarazione non verificata viene
/// accettata, esattamente come nel sorgente compilato senza `proj-backend`.
///
/// # Errors
///
/// Restituisce [`CrsError::Required`] o [`CrsError::InvalidDefinition`] per
/// definizioni testualmente invalide; senza backend PROJ restituisce sempre
/// [`CrsError::BackendUnavailable`] dopo la validazione testuale.
pub fn resolve_crs(definition: &str, name: &'static str) -> Result<ResolvedCrs, CrsError> {
    validate_definition_text(definition, name)?;
    Err(CrsError::BackendUnavailable)
}

/// Verifica il requisito CRS del catalogo sugli input risolti.
///
/// # Errors
///
/// Restituisce [`CrsError::InvalidContract`] se il contratto e' violato
/// (nessun input, numero di CRS errato per `Reprojection`),
/// [`CrsError::ProjectedRequired`]/[`CrsError::GeographicRequired`] per il
/// tipo richiesto, [`CrsError::MissingLinearUnit`] se un CRS proiettato non
/// dichiara un'unita' lineare valida e [`CrsError::Mismatch`] se gli input
/// di `SameProjected` non sono semanticamente uguali.
pub fn validate_requirement(
    requirement: CrsRequirement,
    inputs: &[&ResolvedCrs],
) -> Result<(), CrsError> {
    if inputs.is_empty() {
        return Err(CrsError::InvalidContract("nessun CRS di input"));
    }
    match requirement {
        CrsRequirement::Known => Ok(()),
        CrsRequirement::Projected => inputs.iter().try_for_each(|crs| ensure_projected(crs)),
        CrsRequirement::Geographic => inputs.iter().try_for_each(|crs| ensure_geographic(crs)),
        CrsRequirement::SameProjected => {
            inputs.iter().try_for_each(|crs| ensure_projected(crs))?;
            if inputs[1..]
                .iter()
                .any(|crs| !inputs[0].semantically_equals(crs))
            {
                return Err(CrsError::Mismatch);
            }
            Ok(())
        }
        CrsRequirement::Reprojection => {
            if inputs.len() != 2 {
                return Err(CrsError::InvalidContract(
                    "reprojection richiede CRS sorgente e destinazione",
                ));
            }
            Ok(())
        }
    }
}

fn ensure_projected(crs: &ResolvedCrs) -> Result<(), CrsError> {
    if crs.kind != CrsKind::Projected {
        return Err(CrsError::ProjectedRequired { actual: crs.kind });
    }
    if !matches!(crs.horizontal_unit_to_metre, Some(unit) if unit.is_finite() && unit > 0.0) {
        return Err(CrsError::MissingLinearUnit);
    }
    Ok(())
}

fn ensure_geographic(crs: &ResolvedCrs) -> Result<(), CrsError> {
    if crs.kind != CrsKind::Geographic {
        return Err(CrsError::GeographicRequired { actual: crs.kind });
    }
    Ok(())
}

/// Validates normalized GIS axis order (x=longitude, y=latitude).  Call this
/// for every geographic input after WKB decoding and before any kernel work.
///
/// A differenza del sorgente, che itera su `geo::Geometry`, qui il dominio e'
/// verificato su un iteratore di coordinate `(x, y)`: `plenora-core` non
/// dipende da `geo`. Il wrapper tipizzato su `geo::Geometry` e' in
/// `plenora-kernels-geo` (`plenora_kernels_geo::crs::validate_geometry_domain`)
/// e deleghi a questa funzione, senza differenze di comportamento.
///
/// # Errors
///
/// Restituisce [`CrsError::CoordinateOutOfDomain`] alla prima coordinata non
/// finita o fuori dal dominio longitude/latitude di un CRS geografico; per un
/// CRS proiettato non fallisce mai.
pub fn validate_geometry_domain(
    coordinates: impl Iterator<Item = (f64, f64)>,
    crs: &ResolvedCrs,
) -> Result<(), CrsError> {
    if crs.kind != CrsKind::Geographic {
        return Ok(());
    }
    for (x, y) in coordinates {
        if !x.is_finite()
            || !y.is_finite()
            || !(-180.0..=180.0).contains(&x)
            || !(-90.0..=90.0).contains(&y)
        {
            return Err(CrsError::CoordinateOutOfDomain { x, y });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn geographic() -> ResolvedCrs {
        ResolvedCrs::from_resolved_parts(
            "EPSG:4326".to_owned(),
            serde_json::json!({"type": "GeographicCRS", "name": "WGS 84"}),
            CrsKind::Geographic,
            None,
        )
    }

    fn projected_crs(definition: &str, name: &str) -> ResolvedCrs {
        ResolvedCrs::from_resolved_parts(
            definition.to_owned(),
            serde_json::json!({"type": "ProjectedCRS", "name": name}),
            CrsKind::Projected,
            Some(1.0),
        )
    }

    #[test]
    fn missing_empty_nul_and_oversized_definitions_fail_closed() {
        assert!(matches!(
            required_definition(None, "crs"),
            Err(CrsError::Required { .. })
        ));
        assert!(matches!(
            required_definition(Some("  "), "crs"),
            Err(CrsError::Required { .. })
        ));
        let with_nul = ["EPSG:", "\0", "4326"].concat();
        assert!(matches!(
            required_definition(Some(&with_nul), "crs"),
            Err(CrsError::InvalidDefinition { .. })
        ));
        let oversized = "X".repeat(MAX_CRS_DEFINITION_BYTES + 1);
        assert!(matches!(
            required_definition(Some(&oversized), "crs"),
            Err(CrsError::InvalidDefinition { .. })
        ));
    }

    #[test]
    fn missing_backend_never_trusts_an_unverified_declaration() {
        assert!(matches!(
            resolve_crs("EPSG:3857", "crs"),
            Err(CrsError::BackendUnavailable)
        ));
    }

    #[test]
    fn requirements_and_semantic_crs_equality_are_fail_closed() {
        let geographic = geographic();
        let projected = projected_crs("EPSG:3857", "WGS 84 / Pseudo-Mercator");
        let projected_alias = projected_crs("epsg:3857", "WGS 84 / Pseudo-Mercator");
        let different = projected_crs("EPSG:32632", "WGS 84 / UTM zone 32N");

        assert!(matches!(
            validate_requirement(CrsRequirement::Projected, &[&geographic]),
            Err(CrsError::ProjectedRequired { .. })
        ));
        validate_requirement(CrsRequirement::Geographic, &[&geographic]).unwrap();
        validate_requirement(
            CrsRequirement::SameProjected,
            &[&projected, &projected_alias],
        )
        .unwrap();
        assert!(matches!(
            validate_requirement(CrsRequirement::SameProjected, &[&projected, &different]),
            Err(CrsError::Mismatch)
        ));
        assert!(validate_requirement(CrsRequirement::Reprojection, &[&geographic]).is_err());

        assert_eq!(projected.definition(), "EPSG:3857");
        validate_requirement(CrsRequirement::Known, &[&geographic]).unwrap();
        validate_requirement(CrsRequirement::Projected, &[&projected]).unwrap();
        validate_requirement(CrsRequirement::Reprojection, &[&geographic, &projected]).unwrap();
        assert!(matches!(
            validate_requirement(CrsRequirement::Known, &[]),
            Err(CrsError::InvalidContract(_))
        ));
        assert!(matches!(
            validate_requirement(CrsRequirement::Geographic, &[&projected]),
            Err(CrsError::GeographicRequired { .. })
        ));
        validate_geometry_domain(
            [(f64::INFINITY, f64::NEG_INFINITY)].into_iter(),
            &projected,
        )
        .unwrap();

        let missing_unit = ResolvedCrs::from_resolved_parts(
            "EPSG:3857".to_owned(),
            serde_json::json!({"type": "ProjectedCRS"}),
            CrsKind::Projected,
            None,
        );
        assert!(matches!(
            validate_requirement(CrsRequirement::Projected, &[&missing_unit]),
            Err(CrsError::MissingLinearUnit)
        ));
    }

    #[test]
    fn geographic_domain_checks_normalized_longitude_latitude() {
        let geographic = geographic();
        validate_geometry_domain([(-180.0, -90.0), (180.0, 90.0)].into_iter(), &geographic)
            .unwrap();
        assert!(matches!(
            validate_geometry_domain([(181.0, 0.0)].into_iter(), &geographic),
            Err(CrsError::CoordinateOutOfDomain { .. })
        ));
        assert!(matches!(
            validate_geometry_domain([(0.0, 91.0)].into_iter(), &geographic),
            Err(CrsError::CoordinateOutOfDomain { .. })
        ));
        assert!(matches!(
            validate_geometry_domain([(f64::NAN, 0.0)].into_iter(), &geographic),
            Err(CrsError::CoordinateOutOfDomain { .. })
        ));
    }

    #[test]
    fn crs_error_maps_into_plenora_error_crs_variant() {
        let error = PlenoraError::from(CrsError::BackendUnavailable);
        assert!(matches!(error, PlenoraError::Crs(_)));
        assert!(error.to_string().contains("CRS_BACKEND_UNAVAILABLE"));
    }
}
