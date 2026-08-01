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
use crate::contract::AxisOrder;
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

    /// Ordine degli assi dedotto dalla definizione canonica d'autorita'
    /// (ADR-0009, emendamento 2026-07-31).
    ///
    /// Legge le direzioni dei primi due assi di `coordinate_system` nel
    /// PROJJSON — lo stesso oggetto con cui il kernel opera — e le combina
    /// col `kind`: geographic (north,east) → [`AxisOrder::LatLon`],
    /// geographic (east,north) → [`AxisOrder::LonLat`], projected
    /// (east,north) → [`AxisOrder::EastingNorthing`], projected (north,east)
    /// → [`AxisOrder::NorthingEasting`]. La mappa e' solo direzioni+kind →
    /// variante: NESSUNA tabella di CRS hardcoded — EPSG:4326 → `lat_lon`,
    /// OGC:CRS84 → `lon_lat`, EPSG:32632 → `easting_northing` escono gratis
    /// dalla definizione.
    ///
    /// Deduzione da autorita', mai invenzione: assi assenti o meno di due assi
    /// restituiscono `None`; due direzioni presenti ma fuori dalle quattro
    /// combinazioni canoniche producono [`AxisOrder::Other`].
    #[must_use]
    pub fn authority_axis_order(&self) -> Option<AxisOrder> {
        let axes = self.canonical.get("coordinate_system")?.get("axis")?.as_array()?;
        let first = axes.first()?.get("direction")?.as_str()?;
        let second = axes.get(1)?.get("direction")?.as_str()?;
        match (self.kind, first, second) {
            (CrsKind::Geographic, "north", "east") => Some(AxisOrder::LatLon),
            (CrsKind::Geographic, "east", "north") => Some(AxisOrder::LonLat),
            (CrsKind::Projected, "east", "north") => Some(AxisOrder::EastingNorthing),
            (CrsKind::Projected, "north", "east") => Some(AxisOrder::NorthingEasting),
            _ => Some(AxisOrder::Other),
        }
    }

    /// Ordine delle coordinate usato dalle pipeline PROJ normalizzate per
    /// visualizzazione GIS, distinto dall'ordine nativo dell'autorita'.
    ///
    /// `proj_backend` costruisce le trasformazioni con `Proj::new_known_crs`
    /// e produce sempre x/y normalizzato: longitudine/latitudine per un CRS
    /// geografico, easting/northing per uno proiettato. Questo valore descrive
    /// quindi i byte di output dell'esecuzione; [`Self::authority_axis_order`]
    /// resta disponibile separatamente come metadato della definizione CRS.
    #[must_use]
    pub const fn normalized_gis_axis_order(&self) -> AxisOrder {
        match self.kind {
            CrsKind::Geographic => AxisOrder::LonLat,
            CrsKind::Projected => AxisOrder::EastingNorthing,
        }
    }

    /// SRID dedotto dalla definizione canonica d'autorita' (ADR-0009,
    /// emendamento 2026-07-31).
    ///
    /// `id.code` numerico (in PROJJSON il codice puo' essere numero o
    /// stringa — entrambe le forme sono accettate solo se numeriche) quando
    /// `id.authority` e' una stringa d'autorita'. Codice non numerico (es.
    /// `"CRS84"`), oltre `u32`, o `id` assente → `None`: lo `srid` resta
    /// non emesso (chiave opzionale R5.2), mai indovinato.
    #[must_use]
    pub fn authority_srid(&self) -> Option<u32> {
        self.authority_identifier().map(|(_, code)| code)
    }

    /// Coppia autorita' e codice numerico della definizione canonica.
    ///
    /// Serve ai confronti di coerenza tra rappresentazioni: il solo codice
    /// numerico non identifica un CRS senza la sua autorita'.
    #[must_use]
    pub fn authority_identifier(&self) -> Option<(&str, u32)> {
        let id = self.canonical.get("id")?;
        let authority = id.get("authority")?.as_str()?;
        let code = match id.get("code")? {
            Value::Number(number) => number.as_u64(),
            Value::String(text) => text.parse::<u64>().ok(),
            _ => None,
        }?;
        Some((authority, u32::try_from(code).ok()?))
    }
}

/// SRID da un identificatore testuale `authority:code` (es. `EPSG:4326` →
/// 4326).
///
/// La forma usata dal trasporto legacy, che trasporta la sola definizione
/// senza un [`ResolvedCrs`] (ADR-0009, emendamento 2026-07-31). `None` per
/// ogni altra forma (autorita' o codice vuoti, codice non numerico, oltre
/// `u32`) — mai indovinare: lo `srid` non e' decidibile e l'identificatore
/// resta intero alla risoluzione. Unica fonte di parsing condivisa (era
/// duplicata in `plenora-cli`).
#[must_use]
pub fn authority_code_srid(crs_id: &str) -> Option<u32> {
    let (authority, code) = crs_id.rsplit_once(':')?;
    if authority.is_empty() || code.is_empty() || !code.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    code.parse().ok()
}

/// Coppia `authority:code` semplice, con codice numerico.
///
/// Le forme multi-segmento come gli URN non vengono reinterpretate: il
/// chiamante che dispone di PROJ puo' risolverle semanticamente.
#[must_use]
pub fn authority_code_identifier(crs_id: &str) -> Option<(&str, u32)> {
    let (authority, code) = crs_id.rsplit_once(':')?;
    if authority.is_empty()
        || authority.contains(':')
        || code.is_empty()
        || !code.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    Some((authority, code.parse().ok()?))
}

/// Forma testuale di una definizione CRS (ADR-0009, emendamento 2026-07-31
/// — classe B, emissione).
///
/// Classifica la SOLA stringa, senza backend e senza indovinare: serve
/// all'emissione del blocco canonico R2.2 per scegliere fra
/// `crs_id` (identificatore) e `crs_definition`+`crs_definition_format`
/// (definizione testuale nel formato riconosciuto) — un passthrough
/// idempotente contro la lineage, mai una riscrittura.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DefinitionForm {
    /// Identificatore d'autorita': UNA SOLA coppia `auth:code` senza spazi,
    /// entrambe le parti non vuote e codice qualunque (anche non numerico —
    /// `OGC:CRS84` e' un identificatore valido; la numerita' conta solo per
    /// lo `srid`, [`authority_code_srid`]). Cattura per costruzione anche
    /// gli URN OGC (`urn:ogc:def:crs:...`): la coppia e' valutata
    /// sull'ULTIMO `:` (rsplit), come in [`authority_code_srid`].
    AuthorityCode,
    /// Oggetto JSON (PROJJSON): stesso sniff dell'emissione storica (il
    /// testo si analizza come JSON e produce un oggetto).
    Projjson,
    /// WKT1: inizia (dopo trim) con una parola chiave WKT1 seguita da `[` o
    /// `(` (`PROJCS`, `GEOGCS`, `COMPD_CS`, `GEOCCS`, `VERT_CS`, `LOCAL_CS`,
    /// `FITTED_CS`).
    Wkt,
    /// WKT2: come sopra con parole chiave WKT2 corte e alias long-form
    /// (`PROJCRS`/`PROJECTEDCRS`, `GEODCRS`/`GEODETICCRS`,
    /// `GEOGCRS`/`GEOGRAPHICCRS`, `VERTCRS`/`VERTICALCRS`,
    /// `ENGCRS`/`ENGINEERINGCRS`) e le altre radici esplicitamente elencate
    /// in [`WKT2_KEYWORDS`].
    Wkt2,
    /// Qualunque altra forma (es. proj-string `+proj=...`): in emissione
    /// conserva il comportamento storico (`crs_id`) — limite documentato
    /// preesistente: la tabella §2 non ha un formato proj.
    Other,
}

/// Parole chiave WKT1 riconosciute da [`definition_form`] (seguite da `[` o
/// `(`).
const WKT1_KEYWORDS: [&str; 7] = [
    "PROJCS",
    "GEOGCS",
    "COMPD_CS",
    "GEOCCS",
    "VERT_CS",
    "LOCAL_CS",
    "FITTED_CS",
];

/// Parole chiave CRS top-level WKT2 riconosciute da [`definition_form`]
/// (seguite da `[` o `(`).
const WKT2_KEYWORDS: [&str; 15] = [
    "PROJCRS",
    "PROJECTEDCRS",
    "DERIVEDPROJCRS",
    "GEODCRS",
    "GEODETICCRS",
    "GEOGCRS",
    "GEOGRAPHICCRS",
    "BOUNDCRS",
    "VERTCRS",
    "VERTICALCRS",
    "ENGCRS",
    "ENGINEERINGCRS",
    "PARAMETRICCRS",
    "TIMECRS",
    "COMPOUNDCRS",
];

/// La stringa (gia' trimmata a sinistra) inizia con una parola chiave WKT
/// seguita da `[` o `(` — il delimitatore rende il riconoscimento
/// strutturale, non lessicale (nessun falso positivo su identificatori
/// omonimi).
fn starts_with_wkt_keyword(trimmed: &str, keywords: &[&str]) -> bool {
    let Some(delimiter) = trimmed.find(['[', '(']) else {
        return false;
    };
    let candidate = trimmed[..delimiter].trim_end();
    keywords
        .iter()
        .any(|keyword| candidate.eq_ignore_ascii_case(keyword))
}

/// Classifica una definizione CRS testuale (vedi [`DefinitionForm`]).
///
/// Funzione pura della stringa: nessun backend, mai un errore — le forme
/// non riconosciute cadono in [`DefinitionForm::Other`], che preserva il
/// comportamento storico.
#[must_use]
pub fn definition_form(definition: &str) -> DefinitionForm {
    if let Ok(value) = serde_json::from_str::<Value>(definition) {
        return if value.is_object() {
            DefinitionForm::Projjson
        } else {
            DefinitionForm::Other
        };
    }
    let trimmed = definition.trim_start();
    if starts_with_wkt_keyword(trimmed, &WKT2_KEYWORDS) {
        return DefinitionForm::Wkt2;
    }
    if starts_with_wkt_keyword(trimmed, &WKT1_KEYWORDS) {
        return DefinitionForm::Wkt;
    }
    if !trimmed.contains(char::is_whitespace)
        && trimmed
            .rsplit_once(':')
            .is_some_and(|(authority, code)| !authority.is_empty() && !code.is_empty())
    {
        return DefinitionForm::AuthorityCode;
    }
    DefinitionForm::Other
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

    // --- Deduzione axis_order/srid dalla definizione canonica (ADR-0009,
    // emendamento 2026-07-31) ---------------------------------------------

    #[test]
    fn authority_axis_order_and_srid_from_realistic_epsg_4326() {
        let crs = ResolvedCrs::from_resolved_parts(
            "EPSG:4326".to_owned(),
            serde_json::json!({
                "type": "GeographicCRS",
                "name": "WGS 84",
                "datum": {"type": "GeodeticReferenceFrame", "name": "World Geodetic System 1984"},
                "coordinate_system": {
                    "subtype": "ellipsoidal",
                    "axis": [
                        {"name": "Geodetic latitude", "abbreviation": "Lat",
                         "direction": "north", "unit": "degree"},
                        {"name": "Geodetic longitude", "abbreviation": "Lon",
                         "direction": "east", "unit": "degree"},
                    ],
                },
                "id": {"authority": "EPSG", "code": 4326},
            }),
            CrsKind::Geographic,
            None,
        );
        assert_eq!(crs.authority_axis_order(), Some(AxisOrder::LatLon));
        assert_eq!(crs.authority_srid(), Some(4326));
    }

    #[test]
    fn authority_axis_order_lon_lat_and_no_srid_for_ogc_crs84() {
        let crs = ResolvedCrs::from_resolved_parts(
            "OGC:CRS84".to_owned(),
            serde_json::json!({
                "type": "GeographicCRS",
                "name": "WGS 84 (CRS84)",
                "coordinate_system": {
                    "subtype": "ellipsoidal",
                    "axis": [
                        {"name": "Geodetic longitude", "abbreviation": "Lon",
                         "direction": "east", "unit": "degree"},
                        {"name": "Geodetic latitude", "abbreviation": "Lat",
                         "direction": "north", "unit": "degree"},
                    ],
                },
                "id": {"authority": "OGC", "code": "CRS84"},
            }),
            CrsKind::Geographic,
            None,
        );
        assert_eq!(crs.authority_axis_order(), Some(AxisOrder::LonLat));
        // Codice non numerico: nessuno srid (mai indovinare).
        assert_eq!(crs.authority_srid(), None);
    }

    #[test]
    fn authority_axis_order_and_srid_from_realistic_epsg_32632() {
        let crs = ResolvedCrs::from_resolved_parts(
            "EPSG:32632".to_owned(),
            serde_json::json!({
                "type": "ProjectedCRS",
                "name": "WGS 84 / UTM zone 32N",
                "coordinate_system": {
                    "subtype": "Cartesian",
                    "axis": [
                        {"name": "Easting", "abbreviation": "E",
                         "direction": "east", "unit": "metre"},
                        {"name": "Northing", "abbreviation": "N",
                         "direction": "north", "unit": "metre"},
                    ],
                },
                "id": {"authority": "EPSG", "code": 32632},
            }),
            CrsKind::Projected,
            Some(1.0),
        );
        assert_eq!(crs.authority_axis_order(), Some(AxisOrder::EastingNorthing));
        assert_eq!(crs.authority_srid(), Some(32632));
    }

    #[test]
    fn authority_deduction_without_id_keeps_axes_and_drops_srid() {
        // CRS custom (nessun `id`): gli assi si deducono comunque dalla
        // definizione, lo srid no — e la combinazione projected
        // (north,east) mappa su NorthingEasting.
        let crs = ResolvedCrs::from_resolved_parts(
            "CUSTOM:local-grid".to_owned(),
            serde_json::json!({
                "type": "ProjectedCRS",
                "name": "Local grid",
                "coordinate_system": {
                    "subtype": "Cartesian",
                    "axis": [
                        {"name": "Northing", "abbreviation": "N",
                         "direction": "north", "unit": "metre"},
                        {"name": "Easting", "abbreviation": "E",
                         "direction": "east", "unit": "metre"},
                    ],
                },
            }),
            CrsKind::Projected,
            Some(1.0),
        );
        assert_eq!(crs.authority_axis_order(), Some(AxisOrder::NorthingEasting));
        assert_eq!(crs.authority_srid(), None);
    }

    #[test]
    fn authority_deduction_distinguishes_missing_and_other_axis_order() {
        // Stub senza `coordinate_system` (la forma dei fixture storici):
        // nessuna deduzione — l'emissione resta onesta con `unknown`.
        let crs = ResolvedCrs::from_resolved_parts(
            "EPSG:4326".to_owned(),
            serde_json::json!({"type": "GeographicCRS", "name": "WGS 84"}),
            CrsKind::Geographic,
            None,
        );
        assert_eq!(crs.authority_axis_order(), None);
        assert_eq!(crs.authority_srid(), None);
        // Un solo asse non permette alcuna deduzione.
        let one_axis = ResolvedCrs::from_resolved_parts(
            "EPSG:4326".to_owned(),
            serde_json::json!({
                "type": "GeographicCRS",
                "coordinate_system": {"subtype": "ellipsoidal", "axis": [
                    {"name": "Geodetic latitude", "direction": "north", "unit": "degree"},
                ]},
            }),
            CrsKind::Geographic,
            None,
        );
        assert_eq!(one_axis.authority_axis_order(), None);
        // Due direzioni presenti ma fuori dalle quattro combinazioni
        // canoniche sono un ordine noto non canonico: `other`, non
        // `unknown` (R4.2/R4.3.3).
        let non_canonical = ResolvedCrs::from_resolved_parts(
            "EPSG:32632".to_owned(),
            serde_json::json!({
                "type": "ProjectedCRS",
                "coordinate_system": {"subtype": "Cartesian", "axis": [
                    {"name": "Up", "direction": "up", "unit": "metre"},
                    {"name": "Easting", "direction": "east", "unit": "metre"},
                ]},
            }),
            CrsKind::Projected,
            Some(1.0),
        );
        assert_eq!(non_canonical.authority_axis_order(), Some(AxisOrder::Other));
    }

    #[test]
    fn authority_code_srid_parses_only_numeric_authority_code() {
        assert_eq!(authority_code_srid("EPSG:4326"), Some(4326));
        assert_eq!(authority_code_srid("OGC:CRS84"), None);
        assert_eq!(authority_code_srid(":4326"), None);
        assert_eq!(authority_code_srid("EPSG:"), None);
        assert_eq!(authority_code_srid("EPSG"), None);
        assert_eq!(authority_code_srid("EPSG:4326.0"), None);
        assert_eq!(authority_code_srid("EPSG:99999999999999999999"), None);
        assert_eq!(authority_code_identifier("EPSG:4326"), Some(("EPSG", 4326)));
        assert_eq!(authority_code_identifier("epsg:4326"), Some(("epsg", 4326)));
        assert_eq!(authority_code_identifier("FOO:3003"), Some(("FOO", 3003)));
        assert_eq!(authority_code_identifier("urn:ogc:def:crs:EPSG::4326"), None);
    }

    #[test]
    fn definition_form_recognizes_parenthesized_wkt() {
        // ISO 19162 ammette sia parentesi quadre sia tonde come delimitatori
        // WKT: una definizione valida non deve degradare a `crs_id`.
        assert_eq!(
            definition_form(r#"GEOGCS("WGS 84",DATUM("WGS_1984"))"#),
            DefinitionForm::Wkt
        );
        assert_eq!(
            definition_form(r#"GEODCRS("WGS 84",DATUM("World Geodetic System 1984"))"#),
            DefinitionForm::Wkt2
        );
    }

    #[test]
    fn definition_form_recognizes_all_supported_wkt_root_aliases_and_delimiters() {
        let wkt1 = [
            "PROJCS", "GEOGCS", "COMPD_CS", "GEOCCS", "VERT_CS", "LOCAL_CS", "FITTED_CS",
        ];
        let wkt2 = [
            "PROJCRS",
            "PROJECTEDCRS",
            "DERIVEDPROJCRS",
            "GEODCRS",
            "GEODETICCRS",
            "GEOGCRS",
            "GEOGRAPHICCRS",
            "BOUNDCRS",
            "VERTCRS",
            "VERTICALCRS",
            "ENGCRS",
            "ENGINEERINGCRS",
            "PARAMETRICCRS",
            "TIMECRS",
            "COMPOUNDCRS",
        ];
        for delimiter in ['[', '('] {
            let closing = if delimiter == '[' { ']' } else { ')' };
            for root in wkt1 {
                let definition = format!("{root}{delimiter}\"test\"{closing}");
                assert_eq!(definition_form(&definition), DefinitionForm::Wkt, "{definition}");
            }
            for root in wkt2 {
                let definition = format!("{root}{delimiter}\"test\"{closing}");
                assert_eq!(definition_form(&definition), DefinitionForm::Wkt2, "{definition}");
            }
        }

        for invalid in ["GEODETICCRSISH[\"test\"]", "FITTED_CS_EXTRA(\"test\")"] {
            assert_eq!(definition_form(invalid), DefinitionForm::Other, "{invalid}");
        }
    }

    #[test]
    fn definition_form_classifies_authority_code_projjson_wkt_and_other() {
        // AuthorityCode: codice numerico e non (OGC:CRS84 e' un
        // identificatore valido), URN OGC catturati per costruzione.
        assert_eq!(definition_form("EPSG:4326"), DefinitionForm::AuthorityCode);
        assert_eq!(definition_form("OGC:CRS84"), DefinitionForm::AuthorityCode);
        assert_eq!(
            definition_form("urn:ogc:def:crs:OGC:1.3:CRS84"),
            DefinitionForm::AuthorityCode
        );
        // PROJJSON: oggetto JSON (come lo sniff storico dell'emissione).
        assert_eq!(
            definition_form(r#"{"type":"GeographicCRS","name":"WGS 84"}"#),
            DefinitionForm::Projjson
        );
        // WKT1 (Monte Mario, la forma del caso owner) e WKT2.
        assert_eq!(
            definition_form(r#"PROJCS["Monte Mario / Italy zone 1",GEOGCS["Monte Mario"]]"#),
            DefinitionForm::Wkt
        );
        assert_eq!(
            definition_form(r#"  GEOGCS["Monte Mario",DATUM["Monte_Mario"]]"#),
            DefinitionForm::Wkt,
            "il trim a sinistra non cambia la classifica"
        );
        assert_eq!(
            definition_form(r#"PROJCRS["WGS 84 / UTM zone 32N",BASEGEOGCRS["WGS 84"]]"#),
            DefinitionForm::Wkt2
        );
        assert_eq!(
            definition_form(r#"GEODCRS["WGS 84",DATUM["World Geodetic System 1984"]]"#),
            DefinitionForm::Wkt2
        );
        assert_eq!(definition_form(r#"projcs["lowercase"]"#), DefinitionForm::Wkt);
        assert_eq!(definition_form(r#"GeOdCrS["mixed case"]"#), DefinitionForm::Wkt2);
        for definition in [
            r#"COMPOUNDCRS["compound"]"#,
            r#"PARAMETRICCRS["parametric"]"#,
            r#"TIMECRS["temporal"]"#,
            r#"DERIVEDPROJCRS["derived"]"#,
        ] {
            assert_eq!(definition_form(definition), DefinitionForm::Wkt2, "{definition}");
        }
        // proj-string e forme degeneri: Other (comportamento storico,
        // `crs_id` — la tabella §2 non ha formato proj).
        assert_eq!(
            definition_form("+proj=longlat +datum=WGS84 +no_defs"),
            DefinitionForm::Other
        );
        assert_eq!(definition_form(""), DefinitionForm::Other);
        assert_eq!(definition_form("EPSG:"), DefinitionForm::Other);
        assert_eq!(definition_form(":4326"), DefinitionForm::Other);
        assert_eq!(definition_form("EPSG"), DefinitionForm::Other);
        assert_eq!(definition_form("EPSG: 4326"), DefinitionForm::Other);
        assert_eq!(definition_form("not a crs at all"), DefinitionForm::Other);
        // Un JSON non-oggetto non e' PROJJSON ne' un identificatore; una keyword senza `[` non e' WKT.
        assert_eq!(definition_form(r#""EPSG:4326""#), DefinitionForm::Other);
        assert_eq!(definition_form("PROJCS"), DefinitionForm::Other);
    }
}
