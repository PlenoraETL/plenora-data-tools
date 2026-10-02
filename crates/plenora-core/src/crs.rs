//! Contratto CRS fail-closed.
//!
//! Le sole coordinate non identificano un CRS: il chiamante fornisce una
//! definizione, che va risolta prima che un kernel spaziale giri. In questo
//! workspace (Rust puro) non c'è risoluzione PROJ: [`resolve_crs`] risolve
//! solo gli identificatori d'autorità della tabella dei CRS integrati
//! (`epsg_integrati`, generata dal registro EPSG); ogni altra definizione
//! fallisce chiusa e un [`ResolvedCrs`] diverso entra solo già risolto dal
//! chiamante (README, «CRS integrati»). La riproiezione fra CRS integrati
//! sta in [`riproiezione`].
//!
//! Per ogni CRS integrato ci sono due insiemi di limiti:
//! - l'**area d'uso EPSG** ([`AreaOfUse`]), stretta: e' un metadato, nessun
//!   controllo la usa per rifiutare dati;
//! - il **dominio di validita'**, largo: [`validate_geometry_domain`]
//!   rifiuta le coordinate che ne escono. Per i geografici e' il mondo
//!   (longitudine `-180..=180`, latitudine `-90..=90`); per i proiettati e'
//!   un rettangolo in easting/northing ([`ResolvedCrs::validity_domain`])
//!   con una regola fissa per famiglia di proiezione, descritta nel
//!   generatore `scripts/genera_crs_integrati.py`.

mod epsg_integrati;
mod integrati;
pub mod riproiezione;

use std::fmt;

use crate::catalog::CrsRequirement;
use crate::contract::AxisOrder;
use crate::error::PlenoraError;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

/// Byte massimi di una definizione CRS testuale (64 KiB).
pub const MAX_CRS_DEFINITION_BYTES: usize = 64 * 1024;

/// Versione del registro EPSG da cui e' generata la tabella dei CRS
/// integrati.
pub const BUILTIN_EPSG_VERSION: &str = epsg_integrati::VERSIONE_EPSG;
/// Data di pubblicazione di [`BUILTIN_EPSG_VERSION`].
pub const BUILTIN_EPSG_DATE: &str = epsg_integrati::DATA_EPSG;
/// Versione di PROJ che distribuiva il registro letto dal generatore.
pub const BUILTIN_PROJ_VERSION: &str = epsg_integrati::VERSIONE_PROJ;

/// Precisione a terra delle geometrie, in metri: un centimetro (README,
/// «Precisione delle operazioni geografiche: 1 cm a terra»).
pub const GROUND_PRECISION_METRES: f64 = 0.01;

/// Riquadro longitudine/latitudine in gradi, come il registro EPSG pubblica
/// l'area d'uso.
///
/// `west_longitude > east_longitude` indica un riquadro che attraversa
/// l'antimeridiano (per esempio NAD83, da 167.65 a -40.73).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GeographicBounds {
    /// Longitudine ovest, in gradi.
    pub west_longitude: f64,
    /// Latitudine sud, in gradi.
    pub south_latitude: f64,
    /// Longitudine est, in gradi.
    pub east_longitude: f64,
    /// Latitudine nord, in gradi.
    pub north_latitude: f64,
}

impl GeographicBounds {
    /// Riquadro dai quattro lati, senza verifiche.
    #[must_use]
    pub const fn new(
        west_longitude: f64,
        south_latitude: f64,
        east_longitude: f64,
        north_latitude: f64,
    ) -> Self {
        Self {
            west_longitude,
            south_latitude,
            east_longitude,
            north_latitude,
        }
    }

    /// Il riquadro attraversa l'antimeridiano.
    #[must_use]
    pub fn crosses_antimeridian(&self) -> bool {
        self.west_longitude > self.east_longitude
    }

    /// Il punto `(longitudine, latitudine)` sta nel riquadro, bordi
    /// compresi. Un valore non finito non ci sta mai.
    #[must_use]
    pub fn contains(&self, longitude: f64, latitude: f64) -> bool {
        if !(self.south_latitude..=self.north_latitude).contains(&latitude) {
            return false;
        }
        if self.crosses_antimeridian() {
            (self.west_longitude..=180.0).contains(&longitude)
                || (-180.0..=self.east_longitude).contains(&longitude)
        } else {
            (self.west_longitude..=self.east_longitude).contains(&longitude)
        }
    }
}

/// Rettangolo in coordinate proiettate, per nome d'asse.
///
/// Easting e northing si nominano, non si contano per posizione: vale anche
/// per i CRS con northing come primo asse d'autorita'. Unita': quella
/// lineare del CRS (il metro per ogni CRS integrato).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ProjectedBounds {
    /// Easting minimo.
    pub min_easting: f64,
    /// Northing minimo.
    pub min_northing: f64,
    /// Easting massimo.
    pub max_easting: f64,
    /// Northing massimo.
    pub max_northing: f64,
}

impl ProjectedBounds {
    /// Rettangolo dai quattro estremi, senza verifiche.
    #[must_use]
    pub const fn new(
        min_easting: f64,
        min_northing: f64,
        max_easting: f64,
        max_northing: f64,
    ) -> Self {
        Self {
            min_easting,
            min_northing,
            max_easting,
            max_northing,
        }
    }

    /// Il punto `(easting, northing)` sta nel rettangolo, bordi compresi.
    /// Un valore non finito non ci sta mai.
    #[must_use]
    pub fn contains(&self, easting: f64, northing: f64) -> bool {
        (self.min_easting..=self.max_easting).contains(&easting)
            && (self.min_northing..=self.max_northing).contains(&northing)
    }
}

/// Area d'uso EPSG di un CRS: metadato, non un controllo.
///
/// `geographic` e' il riquadro del registro; `projected`, per i CRS
/// proiettati, e' il suo inviluppo nelle coordinate del CRS, arrotondato al
/// millimetro verso l'esterno.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AreaOfUse {
    /// Riquadro lon/lat del registro.
    pub geographic: GeographicBounds,
    /// Inviluppo nelle coordinate del CRS (solo proiettati).
    pub projected: Option<ProjectedBounds>,
}

/// Ellissoide del datum di un CRS.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ellipsoid {
    /// Semiasse maggiore, in metri.
    pub semi_major_axis_metre: f64,
    /// Inverso dello schiacciamento.
    pub inverse_flattening: f64,
}

/// Tipo di CRS: i soli due che il contratto ammette.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CrsKind {
    /// Geografico: coordinate in gradi (longitudine, latitudine).
    Geographic,
    /// Proiettato: coordinate in un'unità lineare (easting, northing).
    Projected,
}

/// Un CRS risolto: definizione originale, canonical PROJJSON, tipo, unità
/// e, per i CRS della tabella integrata, area d'uso, dominio di validità ed
/// ellissoide.
///
/// Nasce solo da una risoluzione verificata ([`resolve_crs`]) o, per un
/// risolutore del chiamante, da [`ResolvedCrs::from_resolved_parts`].
#[derive(Clone, Debug)]
pub struct ResolvedCrs {
    definition: String,
    canonical: Value,
    kind: CrsKind,
    horizontal_unit_to_metre: Option<f64>,
    /// Metadati della tabella integrata: assenti per un CRS risolto dal
    /// chiamante con [`Self::from_resolved_parts`].
    area_of_use: Option<AreaOfUse>,
    validity_domain: Option<ProjectedBounds>,
    ellipsoid: Option<Ellipsoid>,
    /// La riga della tabella integrata da cui viene: la usa la
    /// riproiezione ([`riproiezione`]). `None` per un CRS risolto dal
    /// chiamante, che non si riproietta.
    integrato: Option<integrati::Identificativo>,
}

impl ResolvedCrs {
    /// Costruisce un CRS gia' risolto e verificato.
    ///
    /// Riservato ai risolutori del chiamante e ai test: il contratto resta
    /// che solo una risoluzione verificata può produrre questi valori.
    ///
    /// Il valore non porta area d'uso, dominio di validita' ne' ellissoide:
    /// per un CRS proiettato [`validate_geometry_domain`] controlla solo che
    /// le coordinate siano finite.
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
            area_of_use: None,
            validity_domain: None,
            ellipsoid: None,
            integrato: None,
        }
    }

    /// Area d'uso EPSG (metadato), presente per i CRS della tabella
    /// integrata.
    #[must_use]
    pub const fn area_of_use(&self) -> Option<AreaOfUse> {
        self.area_of_use
    }

    /// Dominio di validita' in easting/northing di un CRS proiettato della
    /// tabella integrata: e' il limite che [`validate_geometry_domain`] fa
    /// rispettare. `None` per i geografici (il loro dominio e' il mondo) e
    /// per i CRS risolti dal chiamante.
    #[must_use]
    pub const fn validity_domain(&self) -> Option<ProjectedBounds> {
        self.validity_domain
    }

    /// Ellissoide del datum, presente per i CRS della tabella integrata.
    #[must_use]
    pub const fn ellipsoid(&self) -> Option<Ellipsoid> {
        self.ellipsoid
    }

    /// Precisione a terra ([`GROUND_PRECISION_METRES`], un centimetro)
    /// espressa nelle unita' delle coordinate del CRS.
    ///
    /// Proiettato: `0.01 / horizontal_unit_to_metre`. Geografico: 1 cm in
    /// gradi sul raggio di curvatura **massimo** dell'ellissoide del datum,
    /// `a / (1 - f)` (ai poli, lungo il meridiano e lungo il parallelo):
    /// un grado di latitudine o di longitudine non e' mai piu' lungo di
    /// `a / (1 - f) * pi / 180` metri, quindi il passo in gradi vale al piu'
    /// 1 cm a terra ovunque, in entrambe le direzioni (per WGS 84 circa
    /// `8.953e-8` gradi). Il quoziente si arrotonda verso il basso di un
    /// margine relativo di `1e-12`. Senza ellissoide (un geografico risolto
    /// dal chiamante) `None`: nessun raggio prudente copre con certezza ogni
    /// ellissoide (Clarke 1880 IGN ha `a / (1 - f)` = 6 400 057,7 m), e la
    /// precisione non si indovina.
    /// `None` per un proiettato senza un'unita' lineare finita e positiva, o
    /// quando il quoziente non e' un `f64` normale e positivo (unita' fuori
    /// scala, per esempio `f64::from_bits(1)` o `f64::MAX`): la precisione
    /// non si indovina.
    #[must_use]
    pub fn precisione_coordinate(&self) -> Option<f64> {
        match self.kind {
            CrsKind::Geographic => {
                let raggio = self.ellipsoid.and_then(|ellissoide| {
                    let a = ellissoide.semi_major_axis_metre;
                    let inverso = ellissoide.inverse_flattening;
                    (a.is_finite() && a > 0.0 && inverso.is_finite() && inverso > 1.0)
                        .then(|| a / (1.0 - 1.0 / inverso))
                })?;
                let metri_per_grado = raggio.to_radians();
                Some(GROUND_PRECISION_METRES / metri_per_grado * (1.0 - 1e-12))
                    .filter(|precisione| precisione.is_normal() && *precisione > 0.0)
            }
            // Il quoziente si verifica, non solo l'unita': un'unita' finita e
            // positiva ma minuscola (subnormale) darebbe infinito, una enorme
            // un subnormale che ha perso cifre. Solo un normale positivo passa.
            CrsKind::Projected => match self.horizontal_unit_to_metre {
                Some(unit) if unit.is_finite() && unit > 0.0 => {
                    Some(GROUND_PRECISION_METRES / unit)
                        .filter(|precisione| precisione.is_normal() && *precisione > 0.0)
                }
                _ => None,
            },
        }
    }

    /// La definizione originale, nella forma in cui è stata data.
    #[must_use]
    pub fn definition(&self) -> &str {
        &self.definition
    }

    /// Geografico o proiettato.
    #[must_use]
    pub const fn kind(&self) -> CrsKind {
        self.kind
    }

    /// Metri per unità orizzontale: `Some(1.0)` per i proiettati della
    /// tabella integrata, `None` per i geografici.
    #[must_use]
    pub const fn horizontal_unit_to_metre(&self) -> Option<f64> {
        self.horizontal_unit_to_metre
    }

    /// Stesso CRS: canonical uguali, qualunque sia la forma della
    /// definizione (`EPSG:4326` e la sua forma URN sono uguali).
    #[must_use]
    pub fn semantically_equals(&self, other: &Self) -> bool {
        self.canonical == other.canonical
    }

    /// Nodo CRS da cui dedurre identita', assi e autorita'. Un `BoundCRS`
    /// conserva l'operazione nel canonical completo, ma queste proprieta'
    /// appartengono al suo `source_crs`.
    fn identity_canonical(&self) -> Option<&Value> {
        if self.canonical.get("type").and_then(Value::as_str) == Some("BoundCRS") {
            self.canonical.get("source_crs")
        } else {
            Some(&self.canonical)
        }
    }

    /// Ordine degli assi dedotto dalla definizione canonica d'autorità.
    ///
    /// Combina le direzioni dei primi due assi di `coordinate_system` nel
    /// PROJJSON con il `kind`, senza tabelle di CRS: per esempio geographic
    /// (north,east) da' [`AxisOrder::LatLon`], projected (east,north)
    /// [`AxisOrder::EastingNorthing`]. Meno di due assi da' `None`; due
    /// direzioni fuori dalle quattro combinazioni canoniche danno
    /// [`AxisOrder::Other`].
    #[must_use]
    pub fn authority_axis_order(&self) -> Option<AxisOrder> {
        let axes = self
            .identity_canonical()?
            .get("coordinate_system")?
            .get("axis")?
            .as_array()?;
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

    /// Ordine delle coordinate normalizzato per l'uso GIS, distinto
    /// dall'ordine nativo dell'autorità: sempre x/y (lon/lat o
    /// easting/northing).
    ///
    /// È l'ordine dei byte delle geometrie in tutto il workspace, compresa
    /// l'uscita di `geo.reproject`; [`Self::authority_axis_order`] resta il
    /// metadato della definizione.
    #[must_use]
    pub const fn normalized_gis_axis_order(&self) -> AxisOrder {
        match self.kind {
            CrsKind::Geographic => AxisOrder::LonLat,
            CrsKind::Projected => AxisOrder::EastingNorthing,
        }
    }

    /// SRID dedotto dalla definizione canonica d'autorità.
    ///
    /// `id.code` numerico (numero o stringa numerica) quando `id.authority` è
    /// una stringa. Codice non numerico, oltre `u32` o `id` assente danno
    /// `None`: lo `srid` resta non emesso (la chiave è opzionale), mai
    /// indovinato.
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
        let id = self.identity_canonical()?.get("id")?;
        let authority = id.get("authority")?.as_str()?;
        let code = match id.get("code")? {
            Value::Number(number) => number.as_u64(),
            Value::String(text) => text.parse::<u64>().ok(),
            _ => None,
        }?;
        Some((authority, u32::try_from(code).ok()?))
    }
}

/// SRID da un identificatore testuale `authority:code` (es. `EPSG:4326`).
///
/// Serve dove c'è la sola definizione senza un [`ResolvedCrs`] (il blocco
/// canonico da una definizione, il confronto fra `crs_id` e `srid`). Ogni
/// altra forma (parti vuote, codice non numerico, oltre `u32`) dà `None`,
/// mai un valore indovinato. È l'unica fonte di questo parsing.
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
/// Le forme multi-segmento come gli URN non vengono reinterpretate: si
/// confrontano risolvendole ([`resolve_crs`] o il risolutore del chiamante).
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

/// Forma testuale di una definizione CRS.
///
/// Classifica la sola stringa, senza risolverla: sceglie per il blocco
/// canonico `plenora.geometry.*` fra `crs_id` e
/// `crs_definition`+`crs_definition_format`, come passaggio idempotente
/// della lineage, mai una riscrittura.
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
    /// nell'elenco delle parole chiave WKT2 del modulo.
    Wkt2,
    /// Qualunque altra forma (es. proj-string `+proj=...`): in emissione
    /// finisce in `crs_id`. Limite dichiarato: le chiavi canoniche non hanno
    /// un formato proj-string.
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
/// Funzione pura della stringa: nessuna risoluzione, mai un errore. Le
/// forme non riconosciute cadono in [`DefinitionForm::Other`].
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

/// Motivo strutturale di una violazione del dominio geografico.
///
/// Nomina l'asse e la natura del difetto, **mai il valore**: la coordinata e'
/// un dato di cella e non può comparire in un messaggio d'errore (errori
/// senza dati). Chi diagnostica ha comunque
/// l'informazione che serve — quale asse, e se il difetto e' un valore non
/// finito o un valore fuori intervallo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoordinateDomainViolation {
    /// Almeno una delle due componenti non e' finita (NaN o infinito).
    NonFinite,
    /// Longitudine fuori da `-180..=180`.
    LongitudeOutOfRange,
    /// Latitudine fuori da `-90..=90`.
    LatitudeOutOfRange,
    /// Easting fuori dal dominio di validita' del CRS proiettato.
    EastingOutOfValidityDomain,
    /// Northing fuori dal dominio di validita' del CRS proiettato.
    NorthingOutOfValidityDomain,
    /// Longitudine o latitudine fuori dalla regione lon/lat da cui nasce il
    /// dominio di validita' della proiezione (Transverse Mercator e
    /// Mercator), o fuori dal dominio matematico del metodo: lo vede la
    /// riproiezione, che riporta il punto in lon/lat.
    OutsideProjectionRegion,
}

impl fmt::Display for CoordinateDomainViolation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let testo = match self {
            Self::NonFinite => "coordinata non finita",
            Self::LongitudeOutOfRange => "longitudine fuori da -180..=180",
            Self::LatitudeOutOfRange => "latitudine fuori da -90..=90",
            Self::EastingOutOfValidityDomain => {
                "easting fuori dal dominio di validita' del CRS proiettato"
            }
            Self::NorthingOutOfValidityDomain => {
                "northing fuori dal dominio di validita' del CRS proiettato"
            }
            Self::OutsideProjectionRegion => {
                "coordinata fuori dalla regione lon/lat del dominio della proiezione"
            }
        };
        formatter.write_str(testo)
    }
}

/// Errore CRS, con un codice stabile in testa al messaggio; diventa
/// [`PlenoraError::Crs`].
#[derive(Debug, Error)]
pub enum CrsError {
    /// Definizione assente o vuota.
    #[error("CRS_REQUIRED: {name} e' obbligatorio")]
    Required { name: &'static str },
    /// Definizione oltre [`MAX_CRS_DEFINITION_BYTES`] o con NUL.
    #[error("CRS_INVALID: {name}: {reason}")]
    InvalidDefinition { name: &'static str, reason: String },
    /// Definizione non d'autorità (WKT, WKT2, PROJJSON, proj-string), che
    /// qui non si risolve.
    #[error("CRS_BACKEND_UNAVAILABLE: la validazione CRS richiede il backend PROJ")]
    BackendUnavailable,
    /// Identificatore d'autorità fuori dalla tabella dei CRS integrati.
    #[error(
        "CRS_NOT_BUILTIN: l'identificatore d'autorita' non e' nella tabella dei CRS integrati \
         (senza backend PROJ non si risolve altro)"
    )]
    NotBuiltin,
    /// Tipo PROJJSON diverso da geografico o proiettato. Nessun codice di
    /// questo repository lo produce: lo produceva il risolutore PROJ del
    /// progetto d'origine.
    #[error("CRS_TYPE_UNSUPPORTED: tipo PROJJSON {0} non supportato")]
    UnsupportedType(String),
    /// CRS proiettato senza un'unità lineare orizzontale finita e positiva.
    #[error(
        "LINEAR_UNIT_REQUIRED: il CRS proiettato non dichiara un'unita' lineare orizzontale valida"
    )]
    MissingLinearUnit,
    /// L'operazione richiede un CRS proiettato.
    #[error("PROJECTED_CRS_REQUIRED: ricevuto CRS {actual:?}")]
    ProjectedRequired { actual: CrsKind },
    /// L'operazione richiede un CRS geografico.
    #[error("GEOGRAPHIC_CRS_REQUIRED: ricevuto CRS {actual:?}")]
    GeographicRequired { actual: CrsKind },
    /// Le misure geodetiche richiedono l'ellissoide del datum del CRS, che
    /// solo un CRS della tabella integrata porta: nessun ripiego su WGS 84.
    #[error(
        "ELLIPSOID_REQUIRED: le misure geodetiche richiedono l'ellissoide del datum del CRS \
         (un CRS della tabella integrata)"
    )]
    EllipsoidRequired,
    /// Gli ingressi di un'operazione `SameProjected` hanno CRS diversi.
    #[error("CRS_MISMATCH: gli input non usano lo stesso CRS")]
    Mismatch,
    /// Coordinata non finita o fuori dal dominio del CRS.
    #[error("COORDINATE_OUT_OF_CRS_DOMAIN: {violation}")]
    CoordinateOutOfDomain {
        violation: CoordinateDomainViolation,
    },
    /// Contratto del requisito violato (nessun ingresso, numero di CRS
    /// sbagliato).
    #[error("CRS_CONTRACT_INVALID: {0}")]
    InvalidContract(&'static str),
    /// Riproiezione: nessun percorso di trasformazioni EPSG fra i due datum.
    #[error(
        "REPROJECTION_PATH_UNAVAILABLE: nessun percorso di trasformazioni EPSG collega i due datum (senza griglie, o con le sole griglie NTv2 fornite)"
    )]
    ReprojectionPathUnavailable,
    /// Riproiezione: il percorso migliore è meno accurato di 1 cm e la
    /// config non dichiara `accuratezza_accettata_m` sufficiente.
    #[error(
        "REPROJECTION_ACCURACY_NOT_ACCEPTED: il percorso migliore fra i due datum ha accuratezza EPSG di {accuracy_m} m, oltre la precisione di 0.01 m: dichiarare `accuratezza_accettata_m` almeno pari, e il risultato vale solo entro quella accuratezza"
    )]
    ReprojectionAccuracyNotAccepted { accuracy_m: f64 },
    /// Riproiezione: config non valida.
    #[error("REPROJECTION_CONFIG_INVALID: {0}")]
    ReprojectionConfig(&'static str),
    /// Riproiezione: la geometria esce dall'area d'uso di ogni percorso
    /// ammesso.
    #[error(
        "REPROJECTION_OUTSIDE_TRANSFORMATION_AREA: la geometria esce dall'area d'uso di ogni percorso ammesso fra i due datum (o dalle griglie fornite)"
    )]
    ReprojectionOutsideTransformationArea,
    /// Riproiezione: vertici della stessa geometria in aree d'uso che
    /// preferiscono percorsi diversi.
    #[error(
        "REPROJECTION_MIXED_TRANSFORMATION_AREAS: i vertici della geometria preferiscono percorsi diversi fra i due datum (aree d'uso diverse): una sola trasformazione per tutti darebbe ad alcuni vertici parametri di un'altra area, oltre l'accuratezza dichiarata; dividere la geometria o fissare `trasformazioni`"
    )]
    ReprojectionMixedTransformationAreas,
    /// Riproiezione: un'inversa iterativa non converge.
    #[error("REPROJECTION_NOT_CONVERGED: un'inversa iterativa non ha raggiunto la precisione")]
    ReprojectionNotConverged,
    /// Riproiezione: un lato non si approssima entro la precisione con la
    /// densificazione ammessa.
    #[error(
        "REPROJECTION_EDGE_NOT_CONVERGED: un lato non si approssima entro la precisione con la densificazione ammessa (discontinuita' della proiezione, per esempio l'antimeridiano, o limite di vertici)"
    )]
    ReprojectionEdgeNotConverged,
    /// Griglia `NTv2`: il file non si legge.
    #[error("NTV2_GRID_UNREADABLE: il file della griglia non si legge")]
    GridUnreadable,
    /// Griglia `NTv2`: il contenuto non è una griglia valida.
    #[error("NTV2_GRID_INVALID: {reason}")]
    GridInvalid { reason: &'static str },
}

/// Codice stabile di un [`CrsError`], il `code` di `plenora-error-v1`.
///
/// Pattern `^[A-Z][A-Z0-9_]{1,63}$`. Si ottiene solo da [`CrsError::code`]:
/// il campo è privato, quindi un codice fuori dal pattern non si costruisce
/// (il test di `error::pubblico` verifica il pattern su ogni variante).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct CodiceCrs(&'static str);

impl CodiceCrs {
    /// Il codice come testo.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        self.0
    }
}

impl fmt::Display for CodiceCrs {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.0)
    }
}

impl CrsError {
    /// Codice stabile dell'errore, lo stesso in testa al messaggio. Il
    /// `match` non ha ramo di default: una variante nuova non compila senza
    /// il suo codice.
    #[must_use]
    pub const fn code(&self) -> CodiceCrs {
        CodiceCrs(match self {
            Self::Required { .. } => "CRS_REQUIRED",
            Self::InvalidDefinition { .. } => "CRS_INVALID",
            Self::BackendUnavailable => "CRS_BACKEND_UNAVAILABLE",
            Self::NotBuiltin => "CRS_NOT_BUILTIN",
            Self::UnsupportedType(_) => "CRS_TYPE_UNSUPPORTED",
            Self::MissingLinearUnit => "LINEAR_UNIT_REQUIRED",
            Self::ProjectedRequired { .. } => "PROJECTED_CRS_REQUIRED",
            Self::GeographicRequired { .. } => "GEOGRAPHIC_CRS_REQUIRED",
            Self::EllipsoidRequired => "ELLIPSOID_REQUIRED",
            Self::Mismatch => "CRS_MISMATCH",
            Self::CoordinateOutOfDomain { .. } => "COORDINATE_OUT_OF_CRS_DOMAIN",
            Self::InvalidContract(_) => "CRS_CONTRACT_INVALID",
            Self::ReprojectionPathUnavailable => "REPROJECTION_PATH_UNAVAILABLE",
            Self::ReprojectionAccuracyNotAccepted { .. } => "REPROJECTION_ACCURACY_NOT_ACCEPTED",
            Self::ReprojectionConfig(_) => "REPROJECTION_CONFIG_INVALID",
            Self::ReprojectionOutsideTransformationArea => {
                "REPROJECTION_OUTSIDE_TRANSFORMATION_AREA"
            }
            Self::ReprojectionMixedTransformationAreas => "REPROJECTION_MIXED_TRANSFORMATION_AREAS",
            Self::ReprojectionNotConverged => "REPROJECTION_NOT_CONVERGED",
            Self::ReprojectionEdgeNotConverged => "REPROJECTION_EDGE_NOT_CONVERGED",
            Self::GridUnreadable => "NTV2_GRID_UNREADABLE",
            Self::GridInvalid { .. } => "NTV2_GRID_INVALID",
        })
    }
}

/// [`PlenoraError::CrsCoded`]: il codice viaggia tipizzato accanto al testo.
impl From<CrsError> for PlenoraError {
    fn from(error: CrsError) -> Self {
        Self::CrsCoded {
            code: error.code(),
            message: error.to_string(),
        }
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

/// Risoluzione contro la tabella dei CRS integrati, fail-closed per tutto il
/// resto.
///
/// Riconosce gli identificatori d'autorita' dei CRS della tabella:
/// `EPSG:<codice>` (autorita' senza distinzione di maiuscole, codice
/// decimale senza zeri iniziali), le forme URN
/// `urn:ogc:def:crs:EPSG:<versione>:<codice>` (versione vuota o fatta di
/// cifre e punti) e `OGC:CRS84` / `urn:ogc:def:crs:OGC:<versione>:CRS84`. Il
/// canonical dipende solo dal CRS, non dalla forma: due forme dello stesso
/// codice sono [`ResolvedCrs::semantically_equals`], e la definizione
/// originale resta in [`ResolvedCrs::definition`]. Nessuna riproiezione: il
/// valore descrive il CRS (tipo, unita', assi, area d'uso, dominio), non
/// trasforma coordinate.
///
/// Il canonical e' un sottoinsieme del PROJJSON che PROJ produrrebbe (tipo,
/// nome, sistema di coordinate, `id`), non il documento completo: e' stabile
/// dentro questo workspace, ma non e' confrontabile con un canonical del
/// risolutore PROJ di plenora-data-tools.
///
/// # Errors
///
/// - [`CrsError::Required`] o [`CrsError::InvalidDefinition`] per
///   definizioni testualmente invalide;
/// - [`CrsError::NotBuiltin`] per un identificatore d'autorita' che non e'
///   nella tabella (codice sconosciuto, altra autorita', forma non
///   riconosciuta);
/// - [`CrsError::BackendUnavailable`] per ogni definizione non d'autorita'
///   (WKT, WKT2, PROJJSON, proj-string): verificarla richiederebbe PROJ,
///   che qui non c'è.
pub fn resolve_crs(definition: &str, name: &'static str) -> Result<ResolvedCrs, CrsError> {
    validate_definition_text(definition, name)?;
    if definition_form(definition) != DefinitionForm::AuthorityCode {
        return Err(CrsError::BackendUnavailable);
    }
    integrati::risolvi(definition).ok_or(CrsError::NotBuiltin)
}

/// Gli identificatori canonici (`OGC:CRS84`, `EPSG:<codice>`) di tutti i CRS
/// della tabella integrata: `OGC:CRS84` per primo, poi i codici EPSG in
/// ordine crescente.
pub fn builtin_crs_identifiers() -> impl Iterator<Item = String> {
    integrati::identificatori()
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

/// Verifica il dominio di validita' delle coordinate di un input.
///
/// Presuppone l'ordine GIS normalizzato (x=longitudine, y=latitudine per i
/// geografici; x=easting, y=northing per i proiettati, anche quando l'asse
/// d'autorita' e' northing-first) e va chiamata su ogni input dopo la
/// decodifica WKB, prima dei kernel.
///
/// - Geografico: longitudine `-180..=180`, latitudine `-90..=90`, bordi
///   compresi. L'area d'uso EPSG non e' un limite.
/// - Proiettato della tabella integrata: easting e northing entro
///   [`ResolvedCrs::validity_domain`], bordi compresi.
/// - Proiettato risolto dal chiamante senza dominio: solo coordinate finite.
///
/// Il dominio e' un controllo di plausibilita' contro un CRS sbagliato o
/// coordinate prive di senso, non una garanzia di precisione.
///
/// Lavora su coordinate `(x, y)` perche' `plenora-core` non dipende da `geo`;
/// `plenora_kernels_geo::crs::validate_geometry_domain` e' il wrapper su
/// `geo::Geometry` e delega qui.
///
/// # Errors
///
/// Restituisce [`CrsError::CoordinateOutOfDomain`] alla prima coordinata non
/// finita o fuori dal dominio.
pub fn validate_geometry_domain(
    coordinates: impl Iterator<Item = (f64, f64)>,
    crs: &ResolvedCrs,
) -> Result<(), CrsError> {
    for (x, y) in coordinates {
        // Il motivo e' strutturale, non numerico: nomina l'asse e la natura
        // del difetto senza riportare la coordinata, che e' un dato di cella.
        let violation = if !x.is_finite() || !y.is_finite() {
            Some(CoordinateDomainViolation::NonFinite)
        } else {
            match (crs.kind, crs.validity_domain) {
                (CrsKind::Geographic, _) => {
                    if !(-180.0..=180.0).contains(&x) {
                        Some(CoordinateDomainViolation::LongitudeOutOfRange)
                    } else if !(-90.0..=90.0).contains(&y) {
                        Some(CoordinateDomainViolation::LatitudeOutOfRange)
                    } else {
                        None
                    }
                }
                (CrsKind::Projected, Some(domain)) => {
                    if !(domain.min_easting..=domain.max_easting).contains(&x) {
                        Some(CoordinateDomainViolation::EastingOutOfValidityDomain)
                    } else if !(domain.min_northing..=domain.max_northing).contains(&y) {
                        Some(CoordinateDomainViolation::NorthingOutOfValidityDomain)
                    } else {
                        None
                    }
                }
                (CrsKind::Projected, None) => None,
            }
        };
        if let Some(violation) = violation {
            return Err(CrsError::CoordinateOutOfDomain { violation });
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
        // Le definizioni non d'autorita' richiedono PROJ, anche quando
        // descrivono un CRS della tabella.
        for definizione in [
            r#"PROJCS["WGS 84 / Pseudo-Mercator",GEOGCS["WGS 84"]]"#,
            r#"PROJCRS["WGS 84 / UTM zone 32N",BASEGEOGCRS["WGS 84"]]"#,
            r#"{"type":"GeographicCRS","name":"WGS 84","id":{"authority":"EPSG","code":4326}}"#,
            "+proj=longlat +datum=WGS84 +no_defs",
        ] {
            assert!(
                matches!(
                    resolve_crs(definizione, "crs"),
                    Err(CrsError::BackendUnavailable)
                ),
                "{definizione}"
            );
        }
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
        // Proiettato senza dominio (risolto dal chiamante): passa ogni
        // coordinata finita, mai una non finita.
        validate_geometry_domain([(-1.0e12, 1.0e12)].into_iter(), &projected).unwrap();
        assert!(matches!(
            validate_geometry_domain([(f64::INFINITY, f64::NEG_INFINITY)].into_iter(), &projected),
            Err(CrsError::CoordinateOutOfDomain {
                violation: CoordinateDomainViolation::NonFinite
            })
        ));

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

    /// Sentinella di privacy: la coordinata che ha violato il dominio e' un
    /// dato di cella e non deve comparire in nessuna forma del messaggio,
    /// ne' nella variante CRS ne' nell'errore unificato che la avvolge
    /// (errori senza dati).
    #[test]
    fn il_messaggio_di_dominio_non_riporta_la_coordinata() {
        let geographic = geographic();
        // Valori riconoscibili: se sopravvivono, si vedono.
        let sentinelle = [[181.5, 0.25], [0.5, 91.75], [1234.5, 5678.25]];
        for coppia in sentinelle {
            let errore =
                validate_geometry_domain([(coppia[0], coppia[1])].into_iter(), &geographic)
                    .expect_err("coordinata fuori dominio");
            let testo = errore.to_string();
            let unificato = PlenoraError::from(errore).to_string();
            for numero in coppia {
                // Le cifre della coordinata, in qualunque forma stampata.
                for forma in [format!("{numero}"), format!("{numero:?}")] {
                    assert!(!testo.contains(&forma), "coordinata nel testo: {testo}");
                    assert!(
                        !unificato.contains(&forma),
                        "coordinata nell'errore unificato: {unificato}"
                    );
                }
            }
            assert!(testo.contains("COORDINATE_OUT_OF_CRS_DOMAIN"), "{testo}");
        }
        // Il motivo strutturale resta, e distingue i tre casi.
        assert!(matches!(
            validate_geometry_domain([(f64::NAN, 0.0)].into_iter(), &geographic),
            Err(CrsError::CoordinateOutOfDomain {
                violation: CoordinateDomainViolation::NonFinite
            })
        ));
        assert!(matches!(
            validate_geometry_domain([(181.0, 0.0)].into_iter(), &geographic),
            Err(CrsError::CoordinateOutOfDomain {
                violation: CoordinateDomainViolation::LongitudeOutOfRange
            })
        ));
        assert!(matches!(
            validate_geometry_domain([(0.0, 91.0)].into_iter(), &geographic),
            Err(CrsError::CoordinateOutOfDomain {
                violation: CoordinateDomainViolation::LatitudeOutOfRange
            })
        ));
    }

    #[test]
    fn crs_error_maps_into_plenora_error_crs_coded_variant() {
        let error = PlenoraError::from(CrsError::BackendUnavailable);
        assert!(matches!(
            &error,
            PlenoraError::CrsCoded { code, .. } if code.as_str() == "CRS_BACKEND_UNAVAILABLE"
        ));
        assert!(error.to_string().contains("CRS_BACKEND_UNAVAILABLE"));
    }

    // --- Deduzione axis_order/srid dalla definizione canonica ------------

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
        // `unknown`.
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
        assert_eq!(
            authority_code_identifier("urn:ogc:def:crs:EPSG::4326"),
            None
        );
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
            "PROJCS",
            "GEOGCS",
            "COMPD_CS",
            "GEOCCS",
            "VERT_CS",
            "LOCAL_CS",
            "FITTED_CS",
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
                assert_eq!(
                    definition_form(&definition),
                    DefinitionForm::Wkt,
                    "{definition}"
                );
            }
            for root in wkt2 {
                let definition = format!("{root}{delimiter}\"test\"{closing}");
                assert_eq!(
                    definition_form(&definition),
                    DefinitionForm::Wkt2,
                    "{definition}"
                );
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
        assert_eq!(
            definition_form(r#"projcs["lowercase"]"#),
            DefinitionForm::Wkt
        );
        assert_eq!(
            definition_form(r#"GeOdCrS["mixed case"]"#),
            DefinitionForm::Wkt2
        );
        for definition in [
            r#"COMPOUNDCRS["compound"]"#,
            r#"PARAMETRICCRS["parametric"]"#,
            r#"TIMECRS["temporal"]"#,
            r#"DERIVEDPROJCRS["derived"]"#,
        ] {
            assert_eq!(
                definition_form(definition),
                DefinitionForm::Wkt2,
                "{definition}"
            );
        }
        // proj-string e forme degeneri: Other (in emissione `crs_id`: le
        // chiavi canoniche non hanno un formato proj-string).
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
