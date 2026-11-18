//! USNG `standard_v1`, GOP-021/023/024/025; FGDC-STD-011-2001, 3-5, A/B/E.
//!
//! XY in gradi sul datum esplicito, senza trasformazioni. Dominio matematico
//! UTM -80..=84, longitudine -180..=180 (schema globale FGDC figura 1);
//! nessuna UPS. Non dichiara accuratezza geodetica del datum fuori area d'uso.
//! Zona senza padding, banda e quadrato completi, spazi ASCII singoli,
//! 0..5 cifre uguali per asse. NAD27 richiede ` (NAD 27)`, NAD83 nessun suffisso.
//! Massimo 27 byte. Troncamento alla cella; errore numerico <= 0.01 m,
//! distinto dalla quantizzazione (< lato per asse). Centro e sudovest sono
//! selezionati nel rettangolo proiettato intero prima dell'inversa.

use crate::{
    coordinate_mgrs::{format_on, parse_on, MgrsCell, MgrsError},
    grid_text::CellRepresentative,
};
use plenora_core::crs::riproiezione::utm::GridEllipsoid;

/// Datum obbligatorio; le coordinate appartengono gia' al datum scelto.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsngDatum {
    /// North American Datum 1983, ellissoide GRS80.
    Nad83,
    /// North American Datum 1927, ellissoide Clarke 1866.
    Nad27,
}

impl UsngDatum {
    const fn ellipsoid(self) -> GridEllipsoid {
        match self {
            Self::Nad83 => GridEllipsoid::Grs80,
            Self::Nad27 => GridEllipsoid::Clarke1866,
        }
    }

    const fn suffix(self) -> &'static str {
        match self {
            Self::Nad83 => "",
            Self::Nad27 => " (NAD 27)",
        }
    }
}

/// Errori GOP-025, messaggi statici senza coordinate o testo del chiamante.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum UsngError {
    /// Grammatica, coordinate, designatori o suffisso incoerenti.
    #[error("invalid USNG reference")]
    DataMapping,
    /// Precisione non compresa in 0..5.
    #[error("invalid USNG precision")]
    InvalidConfiguration,
    /// Testo oltre 27 byte.
    #[error("USNG reference length limit")]
    ResourceLimit,
    /// Regione polare non supportata da USNG.
    #[error("unsupported USNG domain")]
    Unsupported,
    /// Calcolo non certificabile.
    #[error("USNG projection failed")]
    Execution,
}

impl From<MgrsError> for UsngError {
    fn from(error: MgrsError) -> Self {
        match error {
            MgrsError::DataMapping => Self::DataMapping,
            MgrsError::InvalidConfiguration => Self::InvalidConfiguration,
            MgrsError::ResourceLimit => Self::ResourceLimit,
            MgrsError::Execution => Self::Execution,
        }
    }
}

/// Cella sorgente e rappresentante sul datum richiesto, senza spostamenti.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DecodedUsng {
    /// Rettangolo UTM in metri interi sul datum indicato, zona mai zero.
    pub cell: MgrsCell,
    /// Datum delle coordinate proiettate e geografiche.
    pub datum: UsngDatum,
    /// Politica esplicita, applicata prima dell'inversa.
    pub representative: CellRepresentative,
    /// Longitudine in gradi sul datum indicato.
    pub longitude: f64,
    /// Latitudine in gradi sul datum indicato.
    pub latitude: f64,
}

/// Formatta la cella contenente un punto nel datum esplicito.
///
/// # Errors
/// `InvalidConfiguration` per cifre, `DataMapping` per coordinate invalide,
/// `Unsupported` fuori UTM, `Execution` per calcolo non certificabile.
pub fn format_usng(
    longitude: f64,
    latitude: f64,
    digits: u8,
    datum: UsngDatum,
) -> Result<String, UsngError> {
    if digits > 5 {
        return Err(UsngError::InvalidConfiguration);
    }
    if !longitude.is_finite()
        || !latitude.is_finite()
        || longitude.abs() > 180.0
        || latitude.abs() > 90.0
    {
        return Err(UsngError::DataMapping);
    }
    if !(-80.0..=84.0).contains(&latitude) {
        return Err(UsngError::Unsupported);
    }
    let compact = format_on(longitude, latitude, digits, datum.ellipsoid())?;
    let zone_start = usize::from(compact.starts_with('0'));
    let mut result = format!("{} {}", &compact[zone_start..3], &compact[3..5]);
    if digits != 0 {
        let middle = 5 + usize::from(digits);
        result.push(' ');
        result.push_str(&compact[5..middle]);
        result.push(' ');
        result.push_str(&compact[middle..]);
    }
    result.push_str(datum.suffix());
    Ok(result)
}

/// Legge solo riferimenti canonici completi, senza completamenti geografici.
/// Il rettangolo e il rappresentante possono attraversare il bordo del fuso.
///
/// # Errors
/// `ResourceLimit` oltre 27 byte, `DataMapping` per sintassi o datum discordi,
/// `Unsupported` per designazioni polari complete, `Execution` per calcolo.
pub fn parse_usng(
    text: &str,
    datum: UsngDatum,
    representative: CellRepresentative,
) -> Result<DecodedUsng, UsngError> {
    if text.len() > 27 {
        return Err(UsngError::ResourceLimit);
    }
    if !text.is_ascii() {
        return Err(UsngError::DataMapping);
    }
    let body = match datum {
        UsngDatum::Nad27 => text
            .strip_suffix(datum.suffix())
            .ok_or(UsngError::DataMapping)?,
        UsngDatum::Nad83 => text,
    };
    let mut fields = body.split(' ');
    let designation = fields.next().ok_or(UsngError::DataMapping)?;
    let square = fields.next().ok_or(UsngError::DataMapping)?;
    if square.len() != 2 || !square.bytes().all(|b| b.is_ascii_uppercase()) {
        return Err(UsngError::DataMapping);
    }
    let mut compact = String::with_capacity(15);
    let polar = matches!(designation, "A" | "B" | "Y" | "Z");
    if !polar {
        if !(2..=3).contains(&designation.len()) || designation.starts_with('0') {
            return Err(UsngError::DataMapping);
        }
        let (zone, band) = designation.split_at(designation.len() - 1);
        if !zone.bytes().all(|b| b.is_ascii_digit())
            || !band.bytes().all(|b| b.is_ascii_uppercase())
        {
            return Err(UsngError::DataMapping);
        }
        if zone.len() == 1 {
            compact.push('0');
        }
    }
    compact.push_str(designation);
    compact.push_str(square);
    if let Some(easting) = fields.next() {
        let northing = fields.next().ok_or(UsngError::DataMapping)?;
        if !(1..=5).contains(&easting.len())
            || easting.len() != northing.len()
            || !easting
                .bytes()
                .chain(northing.bytes())
                .all(|b| b.is_ascii_digit())
        {
            return Err(UsngError::DataMapping);
        }
        compact.push_str(easting);
        compact.push_str(northing);
    }
    if fields.next().is_some() {
        return Err(UsngError::DataMapping);
    }
    if polar {
        // Valida il riferimento WGS84 solo per distinguere sintassi e dominio.
        crate::coordinate_mgrs::parse_mgrs(&compact, representative)?;
        return Err(UsngError::Unsupported);
    }
    let decoded = parse_on(&compact, representative, datum.ellipsoid())?;
    Ok(DecodedUsng {
        cell: decoded.cell,
        datum,
        representative,
        longitude: decoded.longitude,
        latitude: decoded.latitude,
    })
}
