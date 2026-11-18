//! DD, DDM e DMS: notazioni di punti, non celle, senza trasformazioni di datum.
//!
//! Il datum delle coordinate ricevute resta quello del chiamante: il testo non
//! lo codifica. API scalare con asse esplicito; per coppie usare latitudine poi
//! longitudine nel testo, longitudine poi latitudine nelle coordinate XY.
//!
//! Grammatica chiusa (GOP-022, GEOMETRY-OPERATIONS-1.0): `DD = dec H`,
//! `DDM = int SP dec H`, `DMS = int SP int SP dec H`, dove la giustapposizione
//! non introduce spazi, `SP` e' esattamente uno spazio ASCII, `int` contiene
//! 1..3 cifre ASCII (almeno due per minuti/secondi) e `dec = int ["." digit{1,8}]`. `H` e' N/S per latitudine,
//! E/W per longitudine, solo maiuscolo e solo suffisso. Zeri iniziali ammessi.
//! Vietati segni (anche concordanti), emisferi iniziali, simboli ° ' ", tab,
//! spazi esterni, esponenti, virgole, Unicode alternativo e campi mancanti.
//! Minuti e secondi sono minori di 60; magnitudine massima 90/180 gradi,
//! con componenti inferiori nulle al limite. Nessuna normalizzazione in lettura.
//! Massimo 32 byte per componente, verificati prima dell'analisi.
//!
//! Precisione obbligatoria 0..8 decimali dell'ultima componente: passo angolare
//! `10^-p` gradi (DD), `10^-p / 60` (DDM), `10^-p / 3600` (DMS).
//! Arrotondamento al piu' vicino, meta' al pari, sul valore f64 esatto, con
//! riporto intero secondi/minuti/gradi. Dominio verificato prima del riporto:
//! oltre +/-180 si rifiuta, non si avvolge. Zero arrotondato emette N/E.
//! Output ASCII: gradi senza padding, minuti/secondi con almeno due cifre,
//! frazione di esattamente p cifre. Quantizzazione intenzionale massima mezzo
//! passo, distinta dalla precisione geografica di calcolo. Lettura: razionale
//! decimale convertito a f64 con un solo arrotondamento IEEE al piu' vicino;
//! errore aggiuntivo massimo mezzo ULP. Non e' una trasformazione geodetica.
//!
//! Fonte angolare: `NGA.STND.0037_2.0.0_GRIDS` (2014-02-28), cap. 5, 5-1/5-2.
//! Sintassi e scelte numeriche: GEOMETRY-OPERATIONS-1.0, GOP-022/024/025.

/// Asse geografico in gradi nel datum del chiamante.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GeographicAxis {
    /// Latitudine in [-90, 90].
    Latitude,
    /// Longitudine in [-180, 180].
    Longitude,
}

impl GeographicAxis {
    const fn limit(self) -> u32 {
        match self {
            Self::Latitude => 90,
            Self::Longitude => 180,
        }
    }

    const fn hemisphere(self, negative: bool) -> char {
        match (self, negative) {
            (Self::Latitude, false) => 'N',
            (Self::Latitude, true) => 'S',
            (Self::Longitude, false) => 'E',
            (Self::Longitude, true) => 'W',
        }
    }
}

/// Notazione selezionata esplicitamente, mai dedotta dal testo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AngularNotation {
    /// Gradi decimali.
    Dd,
    /// Gradi e minuti decimali.
    Ddm,
    /// Gradi, minuti e secondi decimali.
    Dms,
}

impl AngularNotation {
    const fn units(self) -> u64 {
        match self {
            Self::Dd => 1,
            Self::Ddm => 60,
            Self::Dms => 3600,
        }
    }
}

/// Rifiuti del codec: nessuna variante contiene dati del chiamante.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum CoordinateTextError {
    /// Precisione fuori dall'intervallo 0..8.
    #[error("unsupported angular precision")]
    Precision,
    /// Coordinata non finita o fuori dominio, componenti inferiori >= 60.
    #[error("angular coordinate outside domain")]
    Domain,
    /// Grammatica o emisfero incompatibile con l'asse.
    #[error("invalid angular coordinate syntax")]
    Syntax,
    /// Limite di 32 byte superato.
    #[error("angular coordinate text too long")]
    Length,
}

/// Formatta una componente geografica secondo la grammatica del modulo.
///
/// # Errors
/// `Precision` per decimali > 8, `Domain` per non finiti o fuori dominio.
pub fn format_coordinate(
    degrees: f64,
    axis: GeographicAxis,
    notation: AngularNotation,
    decimals: u8,
) -> Result<String, CoordinateTextError> {
    if decimals > 8 {
        return Err(CoordinateTextError::Precision);
    }
    if !degrees.is_finite() || degrees.abs() > f64::from(axis.limit()) {
        return Err(CoordinateTextError::Domain);
    }
    let scale = 10_u64.pow(u32::from(decimals));
    let ticks = rounded_ticks(degrees.abs(), notation.units() * scale);
    let hemisphere = axis.hemisphere(degrees.is_sign_negative() && ticks != 0);
    let d = ticks / (notation.units() * scale);
    let tail = ticks % (notation.units() * scale);
    let text = match notation {
        AngularNotation::Dd => decimal_text(ticks, scale, decimals, 1),
        AngularNotation::Ddm => format!("{d} {}", decimal_text(tail, scale, decimals, 2)),
        AngularNotation::Dms => format!(
            "{d} {:02} {}",
            tail / (60 * scale),
            decimal_text(tail % (60 * scale), scale, decimals, 2)
        ),
    };
    Ok(format!("{text}{hemisphere}"))
}

// Il prodotto significando (53 bit) per scala (39 bit) entra in u128.
// L'esponente e' negativo nel dominio <= 180. Sotto 2^-128 il prodotto
// scalato e' minore di meta' tick, inclusi i subnormali.
#[allow(clippy::cast_possible_truncation, reason = "tick <= 180 * 3600 * 10^8")]
fn rounded_ticks(value: f64, scale: u64) -> u64 {
    let bits = value.to_bits();
    let exponent = (bits >> 52) & 0x7ff;
    let significand = (bits & ((1_u64 << 52) - 1)) | (u64::from(exponent != 0) << 52);
    let shift = 1075 - exponent.max(1);
    if shift >= 128 {
        return 0;
    }
    let product = u128::from(significand) * u128::from(scale);
    let whole = product >> shift;
    let remainder = product - (whole << shift);
    let half = 1_u128 << (shift - 1);
    (whole + u128::from(remainder > half || (remainder == half && !whole.is_multiple_of(2)))) as u64
}

fn decimal_text(ticks: u64, scale: u64, decimals: u8, width: usize) -> String {
    let integer = ticks / scale;
    if decimals == 0 {
        format!("{integer:0width$}")
    } else {
        let fraction = ticks % scale;
        let precision = usize::from(decimals);
        format!("{integer:0width$}.{fraction:0precision$}")
    }
}

/// Legge una componente geografica nella notazione e nell'asse espliciti.
/// Il risultato mantiene il datum del chiamante; zero restituito positivo.
///
/// # Errors
/// `Length` oltre 32 byte; `Syntax` per ogni violazione della grammatica;
/// `Domain` per minuti/secondi >= 60 o magnitudine oltre il limite dell'asse.
#[allow(
    clippy::cast_precision_loss,
    reason = "numeratore <= 180 * 3600 * 10^8 < 2^53"
)]
pub fn parse_coordinate(
    text: &str,
    axis: GeographicAxis,
    notation: AngularNotation,
) -> Result<f64, CoordinateTextError> {
    if text.len() > 32 {
        return Err(CoordinateTextError::Length);
    }
    let (body, negative) = if let Some(body) = text.strip_suffix(axis.hemisphere(false)) {
        (body, false)
    } else if let Some(body) = text.strip_suffix(axis.hemisphere(true)) {
        (body, true)
    } else {
        return Err(CoordinateTextError::Syntax);
    };
    let mut fields = body.split(' ');
    let first = fields.next().ok_or(CoordinateTextError::Syntax)?;
    let (numerator, denominator) = match notation {
        AngularNotation::Dd => decimal(first)?,
        AngularNotation::Ddm | AngularNotation::Dms => {
            let degrees = integer(first)?;
            let second = fields.next().ok_or(CoordinateTextError::Syntax)?;
            let (lower, scale) = if notation == AngularNotation::Ddm {
                lower_decimal(second)?
            } else {
                let minutes = lower_integer(second)?;
                let (seconds, scale) =
                    lower_decimal(fields.next().ok_or(CoordinateTextError::Syntax)?)?;
                if minutes >= 60 || seconds >= 60 * scale {
                    return Err(CoordinateTextError::Domain);
                }
                (minutes * 60 * scale + seconds, scale)
            };
            if lower >= notation.units() * scale {
                return Err(CoordinateTextError::Domain);
            }
            (
                degrees * notation.units() * scale + lower,
                notation.units() * scale,
            )
        }
    };
    if fields.next().is_some() {
        return Err(CoordinateTextError::Syntax);
    }
    if numerator > u64::from(axis.limit()) * denominator {
        return Err(CoordinateTextError::Domain);
    }
    let magnitude = numerator as f64 / denominator as f64;
    Ok(if negative && numerator != 0 {
        -magnitude
    } else {
        magnitude
    })
}

fn integer(text: &str) -> Result<u64, CoordinateTextError> {
    if text.is_empty() || text.len() > 3 || !text.bytes().all(|b| b.is_ascii_digit()) {
        return Err(CoordinateTextError::Syntax);
    }
    text.parse().map_err(|_| CoordinateTextError::Syntax)
}

fn decimal(text: &str) -> Result<(u64, u64), CoordinateTextError> {
    if let Some((whole, fraction)) = text.split_once('.') {
        let whole = integer(whole)?;
        if fraction.is_empty()
            || fraction.len() > 8
            || !fraction.bytes().all(|b| b.is_ascii_digit())
        {
            return Err(CoordinateTextError::Syntax);
        }
        let scale =
            10_u64.pow(u32::try_from(fraction.len()).map_err(|_| CoordinateTextError::Syntax)?);
        let fraction = fraction
            .parse::<u64>()
            .map_err(|_| CoordinateTextError::Syntax)?;
        Ok((whole * scale + fraction, scale))
    } else {
        Ok((integer(text)?, 1))
    }
}

fn lower_integer(text: &str) -> Result<u64, CoordinateTextError> {
    if text.len() < 2 {
        return Err(CoordinateTextError::Syntax);
    }
    integer(text)
}

fn lower_decimal(text: &str) -> Result<(u64, u64), CoordinateTextError> {
    let whole = text.split('.').next().ok_or(CoordinateTextError::Syntax)?;
    lower_integer(whole)?;
    decimal(text)
}
