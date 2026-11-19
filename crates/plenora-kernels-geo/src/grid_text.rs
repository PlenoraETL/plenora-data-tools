//! GARS e `GeoRef` scalari, `NGA.STND.0037_2.0.0_GRIDS` 4-2/4-3 e GOP-023/024/025.
//!
//! Coordinate in gradi nel datum del chiamante, nessuna trasformazione implicita.
//! GARS: DDDLL, DDDLLQ, DDDLLQK; `GeoRef`: LLLL seguito da due campi
//! numerici di uguale lunghezza 2..5. Solo ASCII maiuscolo senza spazi.
//! Risoluzione e rappresentante sono espliciti. Celle sud/ovest inclusive,
//! nord/est esclusive salvo +90/+180, assegnati all'ultima cella; -180 alla prima.
//! Ai poli la longitudine ricevuta seleziona la cella. Troncamento esatto del
//! valore binario f64, anche per subnormali; perdita intenzionale < un passo (<= agli estremi +90/+180).
//! Estensione esatta in tick razionali; centro f64 al piu' vicino (<= mezzo ULP).
//! Sudovest f64 arrotondato verso interno (< un ULP), con angolo esatto conservato
//! nell'estensione. Nessuna promessa centimetrica sul punto originale.
//! Massimo ingresso 256 byte, output 14 byte, memoria e lavoro costanti.

const LETTERS: &[u8; 24] = b"ABCDEFGHJKLMNPQRSTUVWXYZ";

/// Risoluzione GARS esplicita.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GarsResolution {
    /// Cella di 30 minuti.
    Minutes30,
    /// Cella di 15 minuti.
    Minutes15,
    /// Cella di 5 minuti.
    Minutes5,
}

impl GarsResolution {
    const fn scale(self) -> u32 {
        match self {
            Self::Minutes30 => 2,
            Self::Minutes15 => 4,
            Self::Minutes5 => 12,
        }
    }
    const fn length(self) -> usize {
        match self {
            Self::Minutes30 => 5,
            Self::Minutes15 => 6,
            Self::Minutes5 => 7,
        }
    }
}

/// Politica obbligatoria per il punto decodificato.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CellRepresentative {
    /// Punto medio nelle coordinate della griglia sorgente.
    Center,
    /// Angolo minimo nelle coordinate della griglia sorgente.
    Southwest,
}

/// Cella esatta: limiti in gradi = tick / `ticks_per_degree`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GeographicCell {
    /// Denominatore positivo comune.
    pub ticks_per_degree: u32,
    /// Limite occidentale.
    pub west: i32,
    /// Limite meridionale.
    pub south: i32,
    /// Limite orientale.
    pub east: i32,
    /// Limite settentrionale.
    pub north: i32,
}

/// Punto e cella sorgente, senza datum implicito.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DecodedGridCell {
    /// Latitudine in gradi.
    pub latitude: f64,
    /// Longitudine in gradi.
    pub longitude: f64,
    /// Estensione razionale esatta.
    pub extent: GeographicCell,
    /// Politica applicata prima di eventuali trasformazioni del chiamante.
    pub representative: CellRepresentative,
}

/// Errori statici senza dati; categorie GOP-025.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum GridTextError {
    /// Opzione non ammessa.
    #[error("invalid grid configuration")]
    InvalidConfiguration,
    /// Sintassi non canonica o coordinata fuori dominio.
    #[error("invalid grid coordinate")]
    DataMapping,
    /// Ingresso oltre 256 byte.
    #[error("grid text resource limit exceeded")]
    ResourceLimit,
}

// floor(value * scale) senza sommare l'origine in virgola mobile.
// Il prodotto ha al massimo 69 bit; shift >= 128 implica magnitudine < 1.
#[allow(
    clippy::cast_possible_truncation,
    reason = "dominio <= 180 e scala <= 60000"
)]
fn floor_ticks(value: f64, scale: u32) -> i32 {
    let bits = value.abs().to_bits();
    let exponent = (bits >> 52) & 0x7ff;
    let significand = (bits & ((1_u64 << 52) - 1)) | (u64::from(exponent != 0) << 52);
    let shift = 1075 - exponent.max(1);
    let product = u128::from(significand) * u128::from(scale);
    let (whole, remainder) = if shift >= 128 {
        (0, product != 0)
    } else {
        (product >> shift, product != ((product >> shift) << shift))
    };
    let whole = whole as i32;
    if value.is_sign_negative() {
        -whole - i32::from(remainder)
    } else {
        whole
    }
}

fn indices(latitude: f64, longitude: f64, scale: u32) -> Result<(u32, u32), GridTextError> {
    if !latitude.is_finite()
        || !longitude.is_finite()
        || latitude.abs() > 90.0
        || longitude.abs() > 180.0
    {
        return Err(GridTextError::DataMapping);
    }
    let axis = |value, limit| {
        let origin = i32::try_from(limit * scale).map_err(|_| GridTextError::DataMapping)?;
        u32::try_from((floor_ticks(value, scale) + origin).min(2 * origin - 1))
            .map_err(|_| GridTextError::DataMapping)
    };
    Ok((axis(longitude, 180)?, axis(latitude, 90)?))
}

fn letter(index: u32) -> Result<char, GridTextError> {
    LETTERS
        .get(usize::try_from(index).map_err(|_| GridTextError::DataMapping)?)
        .copied()
        .map(char::from)
        .ok_or(GridTextError::DataMapping)
}
fn letter_index(byte: u8, limit: u32) -> Result<u32, GridTextError> {
    let index = LETTERS
        .iter()
        .position(|&b| b == byte)
        .ok_or(GridTextError::DataMapping)?;
    let index = u32::try_from(index).map_err(|_| GridTextError::DataMapping)?;
    if index < limit {
        Ok(index)
    } else {
        Err(GridTextError::DataMapping)
    }
}
fn digits(bytes: &[u8]) -> Result<u32, GridTextError> {
    bytes.iter().try_fold(0, |value, &b| {
        if b.is_ascii_digit() {
            Ok(value * 10 + u32::from(b - b'0'))
        } else {
            Err(GridTextError::DataMapping)
        }
    })
}
const fn input(text: &str, length: usize) -> Result<&[u8], GridTextError> {
    if text.len() > 256 {
        return Err(GridTextError::ResourceLimit);
    }
    if text.len() != length {
        return Err(GridTextError::DataMapping);
    }
    Ok(text.as_bytes())
}
// Sudovest: la divisione nearest puo' essere sotto l'angolo razionale.
// Un solo next_up lo porta nella cella, con errore minore di un ULP.
fn representative_value(tick: i32, scale: u32, offset: i32) -> f64 {
    let value = f64::from(2 * tick + offset) / f64::from(2 * scale);
    if offset == 0 && floor_ticks(value, scale) < tick {
        value.next_up()
    } else {
        value
    }
}

fn decoded(
    x: u32,
    y: u32,
    scale: u32,
    representative: CellRepresentative,
) -> Result<DecodedGridCell, GridTextError> {
    let scale_i = i32::try_from(scale).map_err(|_| GridTextError::DataMapping)?;
    let west = i32::try_from(x).map_err(|_| GridTextError::DataMapping)? - 180 * scale_i;
    let south = i32::try_from(y).map_err(|_| GridTextError::DataMapping)? - 90 * scale_i;
    let offset = i32::from(representative == CellRepresentative::Center);
    Ok(DecodedGridCell {
        latitude: representative_value(south, scale, offset),
        longitude: representative_value(west, scale, offset),
        extent: GeographicCell {
            ticks_per_degree: scale,
            west,
            south,
            east: west + 1,
            north: south + 1,
        },
        representative,
    })
}

/// Codifica GARS con troncamento alla cella contenente il punto.
/// # Errors
/// `DataMapping` per coordinate non finite o fuori dominio.
pub fn format_gars(
    latitude: f64,
    longitude: f64,
    resolution: GarsResolution,
) -> Result<String, GridTextError> {
    let scale = resolution.scale();
    let (x, y) = indices(latitude, longitude, scale)?;
    let subdivisions = scale / 2;
    let band = y / subdivisions;
    let mut text = format!(
        "{:03}{}{}",
        x / subdivisions + 1,
        letter(band / 24)?,
        letter(band % 24)?
    );
    if resolution != GarsResolution::Minutes30 {
        let quadrant_size = scale / 4;
        let qx = x / quadrant_size % 2;
        let qy = y / quadrant_size % 2;
        text.push_str(&(1 + qx + 2 * (1 - qy)).to_string());
        if resolution == GarsResolution::Minutes5 {
            text.push_str(&(1 + x % 3 + 3 * (2 - y % 3)).to_string());
        }
    }
    Ok(text)
}

/// Legge solo GARS canonico della risoluzione richiesta.
/// # Errors
/// `ResourceLimit` oltre 256 byte; `DataMapping` per grammatica non canonica.
pub fn parse_gars(
    text: &str,
    resolution: GarsResolution,
    representative: CellRepresentative,
) -> Result<DecodedGridCell, GridTextError> {
    let bytes = input(text, resolution.length())?;
    let band = digits(&bytes[..3])?;
    if !(1..=720).contains(&band) {
        return Err(GridTextError::DataMapping);
    }
    let mut x = band - 1;
    let mut y = letter_index(bytes[3], 15)? * 24 + letter_index(bytes[4], 24)?;
    if resolution != GarsResolution::Minutes30 {
        let quadrant = digits(&bytes[5..6])?;
        if !(1..=4).contains(&quadrant) {
            return Err(GridTextError::DataMapping);
        }
        x = 2 * x + (quadrant - 1) % 2;
        y = 2 * y + 1 - (quadrant - 1) / 2;
        if resolution == GarsResolution::Minutes5 {
            let key = digits(&bytes[6..7])?;
            if !(1..=9).contains(&key) {
                return Err(GridTextError::DataMapping);
            }
            x = 3 * x + (key - 1) % 3;
            y = 3 * y + 2 - (key - 1) / 3;
        }
    }
    decoded(x, y, resolution.scale(), representative)
}

fn georef_scale(precision: u8) -> Result<u32, GridTextError> {
    if !(2..=5).contains(&precision) {
        return Err(GridTextError::InvalidConfiguration);
    }
    Ok(60 * 10_u32.pow(u32::from(precision - 2)))
}

/// Codifica `GeoRef` con 2..5 cifre per coordinata, longitudine prima.
/// # Errors
/// `InvalidConfiguration` per precisione; `DataMapping` per dominio.
pub fn format_georef(
    latitude: f64,
    longitude: f64,
    precision: u8,
) -> Result<String, GridTextError> {
    let scale = georef_scale(precision)?;
    let (x, y) = indices(latitude, longitude, scale)?;
    let width = usize::from(precision);
    Ok(format!(
        "{}{}{}{}{:0width$}{:0width$}",
        letter(x / scale / 15)?,
        letter(y / scale / 15)?,
        letter(x / scale % 15)?,
        letter(y / scale % 15)?,
        x % scale,
        y % scale
    ))
}

/// Legge solo `GeoRef` canonico con precisione esplicita.
/// # Errors
/// `InvalidConfiguration` per precisione; `ResourceLimit` oltre 256 byte;
/// `DataMapping` per grammatica, lettere o minuti non ammessi.
pub fn parse_georef(
    text: &str,
    precision: u8,
    representative: CellRepresentative,
) -> Result<DecodedGridCell, GridTextError> {
    let scale = georef_scale(precision)?;
    let width = usize::from(precision);
    let bytes = input(text, 4 + 2 * width)?;
    let x = 15 * letter_index(bytes[0], 24)? + letter_index(bytes[2], 15)?;
    let y = 15 * letter_index(bytes[1], 12)? + letter_index(bytes[3], 15)?;
    let mx = digits(&bytes[4..4 + width])?;
    let my = digits(&bytes[4 + width..])?;
    if mx >= scale || my >= scale {
        return Err(GridTextError::DataMapping);
    }
    decoded(x * scale + mx, y * scale + my, scale, representative)
}
