//! Validazione lessicale e normalizzazione esatta condivisa con `json`.

use super::Rifiuto;

/// Profilo decimale comune a WKT e JSON. Il parser JSON restringera' il
/// segno iniziale e gli zeri secondo la propria grammatica.
pub(super) fn normalizza(token: &str) -> Result<(bool, String, i64), Rifiuto> {
    if token.is_empty() || token.len() > crate::crs::MAX_CRS_DEFINITION_BYTES {
        return Err(Rifiuto::NumeroInvalido);
    }
    if token.starts_with("+-") {
        return Err(Rifiuto::NumeroInvalido);
    }
    let token = token.strip_prefix('+').unwrap_or(token);
    let unsigned = token.strip_prefix('-').unwrap_or(token);
    let (mantissa, esponente) = unsigned.split_once(['e', 'E']).unwrap_or((unsigned, "0"));
    let esponente = esponente.strip_prefix(['+', '-']).unwrap_or(esponente);
    let (intera, frazione) = mantissa.split_once('.').unwrap_or((mantissa, "0"));
    if [intera, frazione, esponente]
        .iter()
        .any(|parte| parte.is_empty() || !parte.bytes().all(|b| b.is_ascii_digit()))
    {
        return Err(Rifiuto::NumeroInvalido);
    }
    crate::json::decimale_canonico(token).ok_or(Rifiuto::NumeroInvalido)
}

pub(super) fn uguali(a: &str, b: &str) -> Result<bool, Rifiuto> {
    Ok(normalizza(a)? == normalizza(b)?)
}
