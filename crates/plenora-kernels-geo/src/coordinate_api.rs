//! API comune `standard_v1`, GOP-021..025, sopra i codec scalari.
//!
//! XY e' longitudine/latitudine in gradi sul datum esplicito; il testo angolare
//! e' latitudine poi longitudine. Nessuna trasformazione o selezione automatica
//! del formato. La tabella [`FORMAT_RULES`] dichiara tutte le opzioni: nessun
//! default, precisione/risoluzione obbligatoria in scrittura, rappresentante
//! obbligatorio solo in lettura delle celle. In lettura precisione e risoluzione
//! sono contenute nel testo canonico, senza opzioni di ricampionamento.
//!
//! Massimo 256 byte UTF-8 per testo e `65_536` elementi per batch (limite locale,
//! non numero imposto da GOP-025). Successo ordinato, errore senza prefissi
//! pubblicati; indice zero-based solo per errori dell'elemento. Configurazione
//! verificata anche sui batch vuoti. Testi entro 256 byte che violano i limiti
//! grammaticali dei codec sono `data_mapping`.
//! Ordine di validazione: profilo/cardinalita' opzioni, presenza/duplicati,
//! poi valori in ordine Precision, Resolution, Representative, Datum.
//! Permutare le opzioni non cambia la categoria. GOP-022 ammette zeri iniziali
//! anche nei minuti/secondi: tre cifre intere sono ammesse solo entro [0,60).
//! La scrittura resta canonica a due cifre per minuti/secondi.
//!
//! Quantizzazione: mezzo passo per DD/DDM/DMS e UTM; troncamento inferiore a un
//! lato per celle (uguaglianza al bordo globale +90/+180). Le estensioni restano
//! nella griglia sorgente e nel datum selezionato. Nessuna promessa centimetrica
//! sul punto originale; valgono i limiti numerici documentati dai singoli codec.
//! Le varianti `with_limits` supportano GOP-028. Non qualificano runtime o contabilita'
//! della memoria di GOP-026..031 e non introduce operazioni Arrow/CLI.

use crate::{
    coordinate_mgrs::{format_mgrs, parse_mgrs, MgrsCell, MgrsError},
    coordinate_text::{
        format_coordinate, parse_coordinate, AngularNotation, CoordinateTextError, GeographicAxis,
    },
    coordinate_usng::{format_usng, parse_usng, UsngDatum, UsngError},
    coordinate_utm::{format_utm, parse_utm, UtmError},
    grid_text::{
        format_gars, format_georef, parse_gars, parse_georef, CellRepresentative, GarsResolution,
        GeographicCell, GridTextError,
    },
};

/// Profilo richiesto in entrambe le direzioni.
pub const STANDARD_V1: &str = "standard_v1";
/// Limite GOP-025, in byte UTF-8, non caratteri.
pub const MAX_TEXT_BYTES: usize = 256;
/// Limite locale inclusivo, comune a entrambe le direzioni.
pub const MAX_BATCH_ITEMS: usize = 65_536;

/// Categorie pubbliche, senza valori del chiamante.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum CoordinateErrorKind {
    /// Profilo, opzione, duplicato o parametro obbligatorio mancante.
    #[error("invalid_configuration")]
    InvalidConfiguration,
    /// Datum sconosciuto o non ammesso; nessuna trasformazione implicita.
    #[error("crs")]
    Crs,
    /// Testo o coordinate non validi.
    #[error("data_mapping")]
    DataMapping,
    /// Limite di byte, elementi o riserva del vettore.
    #[error("resource_limit")]
    ResourceLimit,
    /// Dominio geografico escluso dal formato.
    #[error("unsupported")]
    Unsupported,
    /// Proiezione non certificabile.
    #[error("execution")]
    Execution,
    /// Annullamento cooperativo.
    #[error("cancelled")]
    Cancelled,
    /// Scadenza monotona raggiunta.
    #[error("timeout")]
    DeadlineExceeded,
}

/// Errore atomico: nessun output parziale, testo statico e indice opzionale.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("{kind}")]
pub struct CoordinateApiError {
    /// Categoria stabile.
    pub kind: CoordinateErrorKind,
    /// Indice zero-based; assente per errori scalari o dell'intero batch.
    pub index: Option<usize>,
}

impl From<CoordinateErrorKind> for CoordinateApiError {
    fn from(kind: CoordinateErrorKind) -> Self {
        Self { kind, index: None }
    }
}

/// Datum delle coordinate, obbligatorio, senza trasformazioni.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoordinateDatum {
    Wgs84,
    Nad83,
    Nad27,
}

impl std::str::FromStr for CoordinateDatum {
    type Err = CoordinateApiError;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "WGS84" => Ok(Self::Wgs84),
            "NAD83" => Ok(Self::Nad83),
            "NAD27" => Ok(Self::Nad27),
            _ => Err(CoordinateErrorKind::Crs.into()),
        }
    }
}

/// Risoluzione con unita' esplicita; unita' estranee al formato sono rifiutate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoordinateResolution {
    Metres(u8),
    Gars(GarsResolution),
}

macro_rules! option_types {
    ($($name:ident($value:ty) = $label:literal),+ $(,)?) => {
        /// Opzioni nominate; duplicati e opzioni non applicabili sono errori.
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum CoordinateOption { $($name($value)),+ }
        /// Insieme esaustivo delle chiavi della tabella.
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum CoordinateOptionKind { $($name),+ }
        impl CoordinateOptionKind {
            /// Tutte le opzioni, nello stesso ordine delle colonne della tabella.
            pub const ALL: &'static [Self] = &[$(Self::$name),+];
        }
        impl std::str::FromStr for CoordinateOptionKind {
            type Err = CoordinateApiError;
            fn from_str(value: &str) -> Result<Self, Self::Err> {
                match value { $($label => Ok(Self::$name),)+ _ => Err(CoordinateErrorKind::InvalidConfiguration.into()) }
            }
        }
        impl CoordinateOption {
            /// Chiave dell'opzione.
            #[must_use]
            pub const fn kind(self) -> CoordinateOptionKind {
                match self { $(Self::$name(_) => CoordinateOptionKind::$name),+ }
            }
        }
    };
}
option_types! {
    Precision(u8) = "precision",
    Resolution(CoordinateResolution) = "resolution",
    Representative(CellRepresentative) = "representative",
    Datum(CoordinateDatum) = "datum",
}

/// Direzione della conversione.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoordinateDirection {
    Encode,
    Decode,
}

/// Presenza ammessa: non esistono default impliciti.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OptionRequirement {
    Required,
    Forbidden,
}

/// Riga dichiarativa: colonne in ordine [`CoordinateOptionKind::ALL`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CoordinateFormatRule {
    pub format: CoordinateFormat,
    pub encode: [OptionRequirement; 4],
    pub decode: [OptionRequirement; 4],
    /// Nessun valore viene inserito per un'opzione omessa.
    pub defaults: [Option<CoordinateOption>; 4],
    pub datums: &'static [CoordinateDatum],
    /// Intervallo inclusivo delle cifre in scrittura, se pertinente.
    pub precision: Option<(u8, u8)>,
    /// Valori e unita' ammessi in scrittura.
    pub resolutions: &'static [CoordinateResolution],
}

use OptionRequirement::{Forbidden as F, Required as R};
const WGS: &[CoordinateDatum] = &[CoordinateDatum::Wgs84];
const NAD: &[CoordinateDatum] = &[CoordinateDatum::Nad83, CoordinateDatum::Nad27];

macro_rules! format_rules {
    ($($name:ident = $label:literal, $encode:expr, $decode:expr, $datums:expr, $precision:expr, $resolutions:expr;)+) => {
        /// Formato esplicito, senza riconoscimento dal testo.
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum CoordinateFormat { $($name),+ }
        impl CoordinateFormat {
            /// Enumerazione generata dalle righe dichiarative.
            pub const ALL: &'static [Self] = &[$(Self::$name),+];
            /// Regole del formato, condivise da validazione e descrizione.
            #[must_use]
            pub const fn rule(self) -> CoordinateFormatRule {
                match self { $(Self::$name => CoordinateFormatRule {
                    format: Self::$name, encode: $encode, decode: $decode,
                    defaults: [None; 4], datums: $datums, precision: $precision, resolutions: $resolutions,
                }),+ }
            }
        }
        impl std::str::FromStr for CoordinateFormat {
            type Err = CoordinateApiError;
            fn from_str(value: &str) -> Result<Self, Self::Err> {
                match value { $($label => Ok(Self::$name),)+ _ => Err(CoordinateErrorKind::InvalidConfiguration.into()) }
            }
        }
        /// Tabella normativa locale, senza default di servizio.
        pub const FORMAT_RULES: &[CoordinateFormatRule] = &[$(CoordinateFormat::$name.rule()),+];
    };
}
format_rules! {
    Dd = "DD", [R,F,F,R], [F,F,F,R], WGS, Some((0,8)), &[];
    Dms = "DMS", [R,F,F,R], [F,F,F,R], WGS, Some((0,8)), &[];
    Ddm = "DDM", [R,F,F,R], [F,F,F,R], WGS, Some((0,8)), &[];
    Utm = "UTM", [F,R,F,R], [F,F,F,R], WGS, None, &[CoordinateResolution::Metres(1)];
    Mgrs = "MGRS", [R,F,F,R], [F,F,R,R], WGS, Some((0,5)), &[];
    Usng = "USNG", [R,F,F,R], [F,F,R,R], NAD, Some((0,5)), &[];
    Gars = "GARS", [F,R,F,R], [F,F,R,R], WGS, None, &[CoordinateResolution::Gars(GarsResolution::Minutes30), CoordinateResolution::Gars(GarsResolution::Minutes15), CoordinateResolution::Gars(GarsResolution::Minutes5)];
    GeoRef = "GeoRef", [R,F,F,R], [F,F,R,R], WGS, Some((2,5)), &[];
}

/// Richiesta comune scalare e batch. Anche un batch vuoto valida le opzioni.
#[derive(Debug, Clone, Copy)]
pub struct CoordinateRequest<'a> {
    pub profile: &'a str,
    pub format: CoordinateFormat,
    pub options: &'a [CoordinateOption],
}

/// Coordinate XY in gradi nel datum della richiesta.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CoordinatePoint {
    pub longitude: f64,
    pub latitude: f64,
}

/// Estensione nella griglia sorgente, senza trasformazioni o bounding box derivati.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoordinateExtent {
    Geographic(GeographicCell),
    Projected(MgrsCell),
}

/// Cella e politica sempre insieme; assente per notazioni di punti.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CoordinateCell {
    pub extent: CoordinateExtent,
    pub representative: CellRepresentative,
}

/// Punto e, per formati di cella, estensione e rappresentante scelto.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DecodedCoordinate {
    pub point: CoordinatePoint,
    pub datum: CoordinateDatum,
    pub cell: Option<CoordinateCell>,
}

struct Validated {
    format: CoordinateFormat,
    datum: CoordinateDatum,
    precision: Option<u8>,
    resolution: Option<CoordinateResolution>,
    representative: Option<CellRepresentative>,
}

fn validate(
    request: CoordinateRequest<'_>,
    direction: CoordinateDirection,
) -> Result<Validated, CoordinateErrorKind> {
    use CoordinateErrorKind::InvalidConfiguration;
    if request.profile != STANDARD_V1 || request.options.len() > CoordinateOptionKind::ALL.len() {
        return Err(InvalidConfiguration);
    }
    let rule = request.format.rule();
    let requirements = match direction {
        CoordinateDirection::Encode => rule.encode,
        CoordinateDirection::Decode => rule.decode,
    };
    for (key, requirement) in CoordinateOptionKind::ALL.iter().zip(requirements) {
        let count = request
            .options
            .iter()
            .filter(|option| option.kind() == *key)
            .count();
        if count != usize::from(requirement == R) {
            return Err(InvalidConfiguration);
        }
    }
    let mut result = Validated {
        format: request.format,
        datum: CoordinateDatum::Wgs84,
        precision: None,
        resolution: None,
        representative: None,
    };
    // Profilo e presenze precedono i valori; valori nell'ordine pubblico ALL.
    for option in CoordinateOptionKind::ALL
        .iter()
        .filter_map(|key| request.options.iter().find(|option| option.kind() == *key))
    {
        match *option {
            CoordinateOption::Datum(datum) => {
                if !rule.datums.contains(&datum) {
                    return Err(CoordinateErrorKind::Crs);
                }
                result.datum = datum;
            }
            CoordinateOption::Precision(digits) => {
                let (min, max) = rule.precision.ok_or(InvalidConfiguration)?;
                if !(min..=max).contains(&digits) {
                    return Err(InvalidConfiguration);
                }
                result.precision = Some(digits);
            }
            CoordinateOption::Resolution(resolution) => {
                if !rule.resolutions.contains(&resolution) {
                    return Err(InvalidConfiguration);
                }
                result.resolution = Some(resolution);
            }
            CoordinateOption::Representative(representative) => {
                result.representative = Some(representative);
            }
        }
    }
    Ok(result)
}

const fn angular(format: CoordinateFormat) -> Option<AngularNotation> {
    match format {
        CoordinateFormat::Dd => Some(AngularNotation::Dd),
        CoordinateFormat::Ddm => Some(AngularNotation::Ddm),
        CoordinateFormat::Dms => Some(AngularNotation::Dms),
        _ => None,
    }
}

impl Validated {
    fn digits(&self) -> Result<u8, CoordinateErrorKind> {
        self.precision
            .ok_or(CoordinateErrorKind::InvalidConfiguration)
    }
    fn representative(&self) -> Result<CellRepresentative, CoordinateErrorKind> {
        self.representative
            .ok_or(CoordinateErrorKind::InvalidConfiguration)
    }
    const fn usng_datum(&self) -> Result<UsngDatum, CoordinateErrorKind> {
        match self.datum {
            CoordinateDatum::Nad83 => Ok(UsngDatum::Nad83),
            CoordinateDatum::Nad27 => Ok(UsngDatum::Nad27),
            CoordinateDatum::Wgs84 => Err(CoordinateErrorKind::Crs),
        }
    }
    fn encode(&self, point: CoordinatePoint) -> Result<String, CoordinateErrorKind> {
        use CoordinateFormat::{Dd, Ddm, Dms, Gars, GeoRef, Mgrs, Usng, Utm};
        let CoordinatePoint {
            longitude,
            latitude,
        } = point;
        if !longitude.is_finite()
            || !latitude.is_finite()
            || longitude.abs() > 180.0
            || latitude.abs() > 90.0
        {
            return Err(CoordinateErrorKind::DataMapping);
        }
        match self.format {
            Dd | Ddm | Dms => {
                let notation =
                    angular(self.format).ok_or(CoordinateErrorKind::InvalidConfiguration)?;
                let lat = format_coordinate(
                    latitude,
                    GeographicAxis::Latitude,
                    notation,
                    self.digits()?,
                )?;
                let lon = format_coordinate(
                    longitude,
                    GeographicAxis::Longitude,
                    notation,
                    self.digits()?,
                )?;
                Ok(format!("{lat} {lon}"))
            }
            Utm => {
                if !(-80.0..=84.0).contains(&latitude) {
                    return Err(CoordinateErrorKind::Unsupported);
                }
                Ok(format_utm(longitude, latitude, 1)?)
            }
            Mgrs => Ok(format_mgrs(longitude, latitude, self.digits()?)?),
            Usng => Ok(format_usng(
                longitude,
                latitude,
                self.digits()?,
                self.usng_datum()?,
            )?),
            Gars => match self.resolution {
                Some(CoordinateResolution::Gars(resolution)) => {
                    Ok(format_gars(latitude, longitude, resolution)?)
                }
                _ => Err(CoordinateErrorKind::InvalidConfiguration),
            },
            GeoRef => Ok(format_georef(latitude, longitude, self.digits()?)?),
        }
    }

    fn decode(&self, text: &str) -> Result<DecodedCoordinate, CoordinateErrorKind> {
        use CoordinateErrorKind::DataMapping;
        use CoordinateFormat::{Dd, Ddm, Dms, Gars, GeoRef, Mgrs, Usng, Utm};
        if text.len() > MAX_TEXT_BYTES {
            return Err(CoordinateErrorKind::ResourceLimit);
        }
        let mut cell = None;
        let (longitude, latitude) = match self.format {
            Dd | Ddm | Dms => {
                let notation =
                    angular(self.format).ok_or(CoordinateErrorKind::InvalidConfiguration)?;
                let end = text.find(['N', 'S']).ok_or(DataMapping)? + 1;
                let (lat, rest) = text.split_at(end);
                let lon = rest.strip_prefix(' ').ok_or(DataMapping)?;
                (
                    parse_coordinate(lon, GeographicAxis::Longitude, notation)?,
                    parse_coordinate(lat, GeographicAxis::Latitude, notation)?,
                )
            }
            Utm => parse_utm(text)?,
            Mgrs => {
                let decoded = parse_mgrs(text, self.representative()?)?;
                cell = Some(CoordinateCell {
                    extent: CoordinateExtent::Projected(decoded.cell),
                    representative: decoded.representative,
                });
                (decoded.longitude, decoded.latitude)
            }
            Usng => {
                let decoded = parse_usng(text, self.usng_datum()?, self.representative()?)?;
                cell = Some(CoordinateCell {
                    extent: CoordinateExtent::Projected(decoded.cell),
                    representative: decoded.representative,
                });
                (decoded.longitude, decoded.latitude)
            }
            Gars | GeoRef => {
                let decoded = if self.format == Gars {
                    let resolution = match text.len() {
                        5 => GarsResolution::Minutes30,
                        6 => GarsResolution::Minutes15,
                        7 => GarsResolution::Minutes5,
                        _ => return Err(DataMapping),
                    };
                    parse_gars(text, resolution, self.representative()?)?
                } else {
                    let digits = match text.len() {
                        8 => 2,
                        10 => 3,
                        12 => 4,
                        14 => 5,
                        _ => return Err(DataMapping),
                    };
                    parse_georef(text, digits, self.representative()?)?
                };
                cell = Some(CoordinateCell {
                    extent: CoordinateExtent::Geographic(decoded.extent),
                    representative: decoded.representative,
                });
                (decoded.longitude, decoded.latitude)
            }
        };
        Ok(DecodedCoordinate {
            point: CoordinatePoint {
                longitude,
                latitude,
            },
            datum: self.datum,
            cell,
        })
    }
}

/// Codifica un punto nel formato, profilo e datum espliciti.
/// # Errors
/// Configurazione, datum, coordinate, dominio o proiezione invalidi.
pub fn encode_coordinate(
    request: CoordinateRequest<'_>,
    point: CoordinatePoint,
) -> Result<String, CoordinateApiError> {
    Ok(validate(request, CoordinateDirection::Encode)?.encode(point)?)
}

/// Decodifica testo canonico; per celle restituisce estensione e rappresentante.
/// # Errors
/// Configurazione, datum, testo o proiezione invalidi; limite di 256 byte.
pub fn decode_coordinate(
    request: CoordinateRequest<'_>,
    text: &str,
) -> Result<DecodedCoordinate, CoordinateApiError> {
    Ok(validate(request, CoordinateDirection::Decode)?.decode(text)?)
}

/// Limiti del batch; il massimo locale resta `MAX_BATCH_ITEMS`.
#[derive(Clone, Copy, Debug)]
pub struct CoordinateLimits {
    pub max_items: usize,
    pub deadline: Option<std::time::Instant>,
}

impl Default for CoordinateLimits {
    fn default() -> Self {
        Self {
            max_items: MAX_BATCH_ITEMS,
            deadline: None,
        }
    }
}

fn checkpoint(work: &mut crate::noding::Work<'_>) -> Result<(), CoordinateApiError> {
    work.step().map_err(|error| {
        CoordinateApiError::from(match error {
            crate::predicates::PredicateError::DeadlineExceeded => {
                CoordinateErrorKind::DeadlineExceeded
            }
            crate::predicates::PredicateError::Cancelled => CoordinateErrorKind::Cancelled,
            _ => CoordinateErrorKind::ResourceLimit,
        })
    })
}

fn control(
    limits: CoordinateLimits,
    cancelled: Option<&std::sync::atomic::AtomicBool>,
) -> crate::noding::Work<'_> {
    crate::noding::Work::new(
        crate::predicates::RelateLimits {
            max_steps: (MAX_BATCH_ITEMS as u64) + 4,
            deadline: limits.deadline,
            ..crate::predicates::RelateLimits::default()
        },
        cancelled,
    )
}

fn batch<T, U>(
    input: &[T],
    limits: CoordinateLimits,
    work: &mut crate::noding::Work<'_>,
    mut convert: impl FnMut(&T) -> Result<U, CoordinateErrorKind>,
) -> Result<Vec<U>, CoordinateApiError> {
    if input.len() > MAX_BATCH_ITEMS.min(limits.max_items) {
        return Err(CoordinateErrorKind::ResourceLimit.into());
    }
    let mut output = Vec::new();
    output
        .try_reserve_exact(input.len())
        .map_err(|_| CoordinateErrorKind::ResourceLimit)?;
    for (index, value) in input.iter().enumerate() {
        #[cfg(test)]
        crate::noding::gop028::unit();
        checkpoint(work)?;
        output.push(convert(value).map_err(|kind| CoordinateApiError {
            kind,
            index: Some(index),
        })?);
    }
    checkpoint(work)?;
    Ok(output)
}

/// Codifica atomica, ordinata, massimo [`MAX_BATCH_ITEMS`] elementi.
/// # Errors
/// Come [`encode_coordinate`], con indice dell'elemento; limite del batch senza indice.
pub fn encode_coordinates(
    request: CoordinateRequest<'_>,
    points: &[CoordinatePoint],
) -> Result<Vec<String>, CoordinateApiError> {
    encode_coordinates_with_limits(request, points, CoordinateLimits::default(), None)
}

/// Decodifica atomica; nessun prefisso viene restituito in caso di errore.
/// # Errors
/// Come [`decode_coordinate`], con indice dell'elemento; limite del batch senza indice.
pub fn decode_coordinates(
    request: CoordinateRequest<'_>,
    texts: &[&str],
) -> Result<Vec<DecodedCoordinate>, CoordinateApiError> {
    decode_coordinates_with_limits(request, texts, CoordinateLimits::default(), None)
}

/// Codifica cooperativa: checkpoint all'ingresso, per elemento e prima del ritorno.
/// L'intervallo massimo e' un codec scalare, con testo limitato a 256 byte.
/// # Errors
/// Errori atomici della famiglia; la deadline prevale sulla cancellazione.
pub fn encode_coordinates_with_limits(
    request: CoordinateRequest<'_>,
    points: &[CoordinatePoint],
    limits: CoordinateLimits,
    cancelled: Option<&std::sync::atomic::AtomicBool>,
) -> Result<Vec<String>, CoordinateApiError> {
    let mut work = control(limits, cancelled);
    checkpoint(&mut work)?;
    let options = validate(request, CoordinateDirection::Encode)?;
    batch(points, limits, &mut work, |point| options.encode(*point))
}

/// Decodifica cooperativa con gli stessi checkpoint della codifica.
/// # Errors
/// Errori atomici della famiglia; la deadline prevale sulla cancellazione.
pub fn decode_coordinates_with_limits(
    request: CoordinateRequest<'_>,
    texts: &[&str],
    limits: CoordinateLimits,
    cancelled: Option<&std::sync::atomic::AtomicBool>,
) -> Result<Vec<DecodedCoordinate>, CoordinateApiError> {
    let mut work = control(limits, cancelled);
    checkpoint(&mut work)?;
    let options = validate(request, CoordinateDirection::Decode)?;
    batch(texts, limits, &mut work, |text| options.decode(text))
}

impl From<CoordinateTextError> for CoordinateErrorKind {
    fn from(value: CoordinateTextError) -> Self {
        match value {
            CoordinateTextError::Precision => Self::InvalidConfiguration,
            CoordinateTextError::Domain
            | CoordinateTextError::Syntax
            | CoordinateTextError::Length => Self::DataMapping,
        }
    }
}
impl From<UtmError> for CoordinateErrorKind {
    fn from(value: UtmError) -> Self {
        match value {
            UtmError::Precision => Self::InvalidConfiguration,
            UtmError::Projection => Self::Execution,
            UtmError::Domain
            | UtmError::Syntax
            | UtmError::Zone
            | UtmError::Hemisphere
            | UtmError::Length => Self::DataMapping,
        }
    }
}
impl From<MgrsError> for CoordinateErrorKind {
    fn from(value: MgrsError) -> Self {
        match value {
            MgrsError::InvalidConfiguration => Self::InvalidConfiguration,
            MgrsError::Execution => Self::Execution,
            MgrsError::DataMapping | MgrsError::ResourceLimit => Self::DataMapping,
        }
    }
}
impl From<UsngError> for CoordinateErrorKind {
    fn from(value: UsngError) -> Self {
        match value {
            UsngError::InvalidConfiguration => Self::InvalidConfiguration,
            UsngError::Execution => Self::Execution,
            UsngError::Unsupported => Self::Unsupported,
            UsngError::DataMapping | UsngError::ResourceLimit => Self::DataMapping,
        }
    }
}
impl From<GridTextError> for CoordinateErrorKind {
    fn from(value: GridTextError) -> Self {
        match value {
            GridTextError::InvalidConfiguration => Self::InvalidConfiguration,
            GridTextError::DataMapping => Self::DataMapping,
            GridTextError::ResourceLimit => Self::ResourceLimit,
        }
    }
}

#[cfg(test)]
#[deny(clippy::disallowed_methods)]
mod tests {
    use super::*;
    #[test]
    #[deny(clippy::disallowed_methods)]
    fn interruption_at_each_element_and_publication_is_atomic() {
        use std::sync::atomic::AtomicBool;
        for deadline in [false, true] {
            for at in 0..=3 {
                let flag = AtomicBool::new(false);
                let limits = CoordinateLimits::default();
                let mut work = control(limits, Some(&flag));
                work.interrupt_at = Some((at, deadline));
                let result = batch(&[1, 2, 3], limits, &mut work, |value| Ok(*value));
                let kind = if deadline {
                    CoordinateErrorKind::DeadlineExceeded
                } else {
                    CoordinateErrorKind::Cancelled
                };
                assert_eq!(result, Err(kind.into()));
            }
        }
    }
}
