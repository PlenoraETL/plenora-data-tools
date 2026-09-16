//! Cio' che i casi del dispatch condividono.
//!
//! # Perche' un modulo e non una copia per file
//!
//! Perche' le forme che il parser della CLI usa per rifiutare un comando ignoto
//! sono un **fatto solo**, e due copie sono due occasioni di divergere. Quella
//! che diverge e' sempre la seconda: il giorno che il messaggio cambia, un file
//! viene aggiornato e l'altro continua a cercare una parola che nessuno scrive
//! piu' — restando verde, perche' un'asserzione che cerca cio' che non c'e'
//! passa sempre.
//!
//! Sta sotto `tests/comune/` e non in un file di primo livello perche' solo i
//! `.rs` direttamente in `tests/` diventano target: un `mod.rs` in una
//! sottodirectory si include e basta.

/// Se il messaggio viene dal parser della CLI invece che dal dispatch.
///
/// # Perche' tre forme, e perche' proprio queste
///
/// Perche' il parser rifiuta un comando ignoto con **«comando non valido»**, e
/// una verifica che cercasse solo «sconosciuto» non lo vedrebbe: cercherebbe
/// per sempre una parola che quel messaggio non contiene, e il caso resterebbe
/// verde anche col dispatch tolto. E' un difetto misurato, non temuto: col
/// dispatch tolto dal `main`, l'asserzione su «sconosciuto» passa lo stesso.
///
/// Il caso nel suo insieme se ne accorge comunque — altre sue asserzioni
/// cadono — ma **questa** riga, che esiste apposta per distinguere il rifiuto
/// del dispatch da quello del parser, non distingue niente.
///
/// Le altre due restano perche' la formulazione puo' cambiare, e un elenco piu'
/// largo sbaglia solo per eccesso di allarme — che e' il verso giusto in cui
/// sbagliare, per un'asserzione che deve accorgersi di una ricaduta.
pub fn ricaduta_nel_parser(testo: &str) -> bool {
    ["comando non valido", "sconosciuto", "unknown"]
        .iter()
        .any(|forma| testo.contains(forma))
}

/// Il binario di questo workspace, come lo esegue chi lo usa.
pub const fn eseguibile() -> &'static str {
    env!("CARGO_BIN_EXE_plenora-data-tools")
}
