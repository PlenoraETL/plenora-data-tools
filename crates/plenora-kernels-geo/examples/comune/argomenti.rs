//! I parametri dei benchmark geo, da riga di comando e da ambiente.
//!
//! Un parametro assente vale il suo predefinito; uno presente ma non valido
//! (zero, negativo, illeggibile, non UTF-8) ferma il processo con exit 2 e
//! un messaggio su stderr, prima di costruire qualunque fixture: un valore
//! sbagliato non deve diventare in silenzio quello predefinito.
//!
//! Copia di `plenora-kernels-table/examples/comune/argomenti.rs`: i moduli
//! comuni degli esempi non attraversano i confini di crate.

// Ogni benchmark usa un sottoinsieme di queste letture.
#![allow(dead_code)]

use std::ffi::OsString;
use std::time::Duration;

/// Un intero strettamente positivo, o `None` se il testo non lo e'.
pub fn interpreta_positivo(testo: &str) -> Option<usize> {
    testo.parse::<usize>().ok().filter(|valore| *valore > 0)
}

/// L'argomento posizionale `indice` come intero positivo; assente vale
/// `predefinito`.
pub fn intero_positivo_arg(indice: usize, nome: &str, predefinito: usize) -> usize {
    std::env::args_os()
        .nth(indice)
        .map_or(predefinito, |testo| positivo_o_rifiuta(&testo, nome))
}

/// La variabile d'ambiente `var` come intero positivo; assente vale
/// `predefinito`.
pub fn intero_positivo_env(var: &str, predefinito: usize) -> usize {
    std::env::var_os(var).map_or(predefinito, |testo| positivo_o_rifiuta(&testo, var))
}

/// La variabile d'ambiente `var` come numero intero positivo di secondi;
/// assente vale `predefinito` secondi.
pub fn secondi_positivi_env(var: &str, predefinito: u64) -> Duration {
    let secondi = std::env::var_os(var).map_or(predefinito, |testo| {
        testo
            .to_str()
            .and_then(|testo| testo.parse::<u64>().ok())
            .filter(|valore| *valore > 0)
            .unwrap_or_else(|| rifiuta(var, "un intero positivo di secondi", &testo))
    });
    Duration::from_secs(secondi)
}

/// L'argomento posizionale `indice` fra `opzioni`; assente vale
/// `predefinito`.
pub fn scelta_arg(
    indice: usize,
    nome: &str,
    opzioni: &[&'static str],
    predefinito: &'static str,
) -> &'static str {
    std::env::args_os()
        .nth(indice)
        .map_or(predefinito, |testo| {
            opzioni
                .iter()
                .copied()
                .find(|opzione| testo.to_str() == Some(opzione))
                .unwrap_or_else(|| rifiuta(nome, &format!("uno fra {}", opzioni.join("|")), &testo))
        })
}

fn positivo_o_rifiuta(testo: &OsString, nome: &str) -> usize {
    testo
        .to_str()
        .and_then(interpreta_positivo)
        .unwrap_or_else(|| rifiuta(nome, "un intero positivo", testo))
}

fn rifiuta(nome: &str, atteso: &str, testo: &OsString) -> ! {
    eprintln!(
        "errore: {nome} deve essere {atteso}, ricevuto «{}»",
        testo.to_string_lossy()
    );
    std::process::exit(2);
}
