//! Aritmetica dei clock ai confini esterni, senza conversioni calendariali.
//! Una deadline passata scade al primo controllo; una futura non
//! rappresentabile si rifiuta. Nessun arrotondamento fra unita'.

use std::time::{Duration, Instant, SystemTime};

use crate::{PlenoraError, Result};

/// GOP-028: controllo comune del workspace, scadenza prima dell'annullamento.
/// Gli errori sono costruiti solo quando il controllo interrompe il lavoro.
///
/// # Errors
/// L'errore di scadenza se raggiunta, altrimenti quello di annullamento.
pub fn verifica_interruzione<E>(
    scadenza: Option<Instant>,
    annullamento: Option<&std::sync::atomic::AtomicBool>,
    errore_scadenza: impl FnOnce() -> E,
    errore_annullamento: impl FnOnce() -> E,
) -> std::result::Result<(), E> {
    if scadenza.is_some_and(|fine| Instant::now() >= fine) {
        return Err(errore_scadenza());
    }
    if annullamento.is_some_and(|segnale| segnale.load(std::sync::atomic::Ordering::Acquire)) {
        return Err(errore_annullamento());
    }
    Ok(())
}

fn fuori_dominio() -> PlenoraError {
    PlenoraError::InvalidConfiguration("tempo fuori dal dominio rappresentabile".into())
}

/// Timeout relativo, incluso zero.
///
/// # Errors
/// `InvalidConfiguration` se la somma supera il dominio del clock.
pub fn timeout(adesso: Instant, durata: Duration) -> Result<Instant> {
    let scadenza = adesso.checked_add(durata).ok_or_else(fuori_dominio)?;
    if scadenza.duration_since(adesso) != durata {
        return Err(fuori_dominio());
    }
    Ok(scadenza)
}

fn durata_secondi_non_negativi(secondi: f64) -> Result<Duration> {
    Duration::try_from_secs_f64(secondi).map_err(|_| fuori_dominio())
}

/// Timeout Python in secondi IEEE754, arrotondato al nanosecondo.
///
/// # Errors
/// `InvalidConfiguration` per durata negativa, non finita o fuori dominio.
pub fn timeout_secondi_f64(adesso: Instant, secondi: f64) -> Result<Instant> {
    timeout(adesso, durata_secondi_non_negativi(secondi)?)
}

fn nanosecondi_epoca(istante: SystemTime) -> i128 {
    match istante.duration_since(SystemTime::UNIX_EPOCH) {
        Ok(d) => i128::from(d.as_secs()) * 1_000_000_000 + i128::from(d.subsec_nanos()),
        Err(e) => {
            let d = e.duration();
            -(i128::from(d.as_secs()) * 1_000_000_000 + i128::from(d.subsec_nanos()))
        }
    }
}

/// Deadline Unix: secondi floor e frazione in nanosecondi.
///
/// I leap second
/// non sono rappresentabili nei clock standard e vengono rifiutati.
/// Il confronto in i128 copre anche `SystemTime` fuori dal calendario chrono.
///
/// # Errors
/// `InvalidConfiguration` per frazione non canonica o clock fuori dominio.
pub fn deadline_unix(
    secondi: i64,
    nanos: u32,
    civile: SystemTime,
    monotono: Instant,
) -> Result<Instant> {
    if nanos >= 1_000_000_000 {
        return Err(fuori_dominio());
    }
    let epoca = i128::from(secondi) * 1_000_000_000 + i128::from(nanos);
    deadline_nanosecondi(epoca, civile, monotono)
}

fn deadline_nanosecondi(epoca: i128, civile: SystemTime, monotono: Instant) -> Result<Instant> {
    let restante = epoca - nanosecondi_epoca(civile);
    if restante <= 0 {
        return Ok(monotono);
    }
    let secondi = u64::try_from(restante / 1_000_000_000).map_err(|_| fuori_dominio())?;
    let nanos = u32::try_from(restante % 1_000_000_000).map_err(|_| fuori_dominio())?;
    timeout(monotono, Duration::new(secondi, nanos))
}

/// Deadline gia' nel dominio `SystemTime`; passato = primo controllo.
///
/// # Errors
/// `InvalidConfiguration` se il clock monotono non rappresenta la scadenza.
pub fn deadline_systemtime(
    istante: SystemTime,
    civile: SystemTime,
    monotono: Instant,
) -> Result<Instant> {
    istante
        .duration_since(civile)
        .map_or(Ok(monotono), |restante| timeout(monotono, restante))
}

/// Deadline Python in secondi IEEE754.
///
/// La durata si arrotonda al nanosecondo
/// come `Duration::try_from_secs_f64`; la precisione del float e' gia' quella
/// dichiarata dall'SDK. Nessun passaggio attraverso il calendario del clock.
///
/// # Errors
/// `InvalidConfiguration` per valori non finiti o non rappresentabili.
pub fn deadline_secondi_f64(epoca: f64, civile: SystemTime, monotono: Instant) -> Result<Instant> {
    if !epoca.is_finite() {
        return Err(fuori_dominio());
    }
    let durata = durata_secondi_non_negativi(epoca.abs())?;
    let nanos = i128::from(durata.as_secs()) * 1_000_000_000 + i128::from(durata.subsec_nanos());
    deadline_nanosecondi(if epoca < 0.0 { -nanos } else { nanos }, civile, monotono)
}

/// Rifiuta una frazione RFC3339 che chrono troncherebbe sotto il nanosecondo.
/// La grammatica completa resta responsabilita' del parser del chiamante.
///
/// # Errors
/// `InvalidConfiguration` se dopo nove cifre esiste una cifra non nulla.
pub fn frazione_rfc3339_esatta(testo: &str) -> Result<()> {
    if let Some((_, frazione)) = testo.split_once('.') {
        if frazione
            .bytes()
            .take_while(u8::is_ascii_digit)
            .skip(9)
            .any(|b| b != b'0')
        {
            return Err(fuori_dominio());
        }
    }
    Ok(())
}
