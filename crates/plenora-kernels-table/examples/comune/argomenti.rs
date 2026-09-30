//! I parametri numerici dei benchmark, da riga di comando.
//!
//! Un argomento assente vale il suo predefinito; uno presente ma non valido
//! (zero, negativo, illeggibile, non UTF-8) ferma il processo con exit 2 e
//! un messaggio su stderr, prima di costruire la fixture: un valore sbagliato
//! non deve diventare in silenzio quello predefinito.

use std::ffi::OsString;

/// Un intero strettamente positivo, o `None` se il testo non lo e'.
pub fn interpreta_positivo(testo: &str) -> Option<usize> {
    testo.parse::<usize>().ok().filter(|valore| *valore > 0)
}

/// L'argomento posizionale `indice` come intero positivo; assente vale
/// `predefinito`.
pub fn intero_positivo_arg(indice: usize, nome: &str, predefinito: usize) -> usize {
    std::env::args_os()
        .nth(indice)
        .map_or(predefinito, |testo| {
            testo
                .to_str()
                .and_then(interpreta_positivo)
                .unwrap_or_else(|| rifiuta(nome, &testo))
        })
}

fn rifiuta(nome: &str, testo: &OsString) -> ! {
    eprintln!(
        "errore: {nome} deve essere un intero positivo, ricevuto «{}»",
        testo.to_string_lossy()
    );
    std::process::exit(2);
}
