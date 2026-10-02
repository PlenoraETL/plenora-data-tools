//! Un panico di un altro thread non lascia passare un successo, anche se
//! avviene prima che l'invocazione cominci (il thread del gestore di
//! Ctrl-C parte prima): conta la base presa subito dopo l'hook, come in
//! `main.rs`.
//!
//! Hook e conto sono del processo: questo binario di test ha **un solo
//! test**, così i delta sono esatti e nessun altro test li sporca.

use plenora_cli::{esegui_invocazione_dal, Uscita};
use plenora_core::panic_policy::{
    barriera_di_dipendenza, install, panici_fuori_dalle_barriere, PanicPolicy,
};
use serde_json::Value;

fn documento(uscita: &Uscita) -> Value {
    serde_json::from_str(&uscita.stdout).expect("JSON")
}

#[test]
fn panico_dopo_la_base_rende_internal_barriera_no() {
    assert!(
        install(PanicPolicy::Silent),
        "prima installazione del processo"
    );
    let catalogo = ["catalog".to_owned()];
    let run = ["run".to_owned()];

    // Base presa, poi un panico in un thread staccato *prima*
    // dell'invocazione: come un panico immediato del gestore di Ctrl-C.
    let base = panici_fuori_dalle_barriere();
    let figlio = std::thread::spawn(|| std::panic::panic_any(String::from("PAYLOAD")));
    assert!(figlio.join().is_err());
    assert_eq!(panici_fuori_dalle_barriere(), base + 1);
    let uscita = esegui_invocazione_dal(&catalogo, &None, base);
    assert_eq!(uscita.codice, 70, "{}", uscita.stdout);
    assert_eq!(documento(&uscita)["error"]["category"], "internal");
    assert!(!uscita.stdout.contains("PAYLOAD"));
    // `run` (qui un errore d'uso) resta l'errore tipizzato: il conto cambia
    // solo un successo.
    assert_eq!(esegui_invocazione_dal(&run, &None, base).codice, 2);

    // Un panico dentro una barriera non conta: con la base attuale il
    // catalogo riesce.
    let base = panici_fuori_dalle_barriere();
    assert!(barriera_di_dipendenza(|| -> () { std::panic::panic_any("atteso") }).is_err());
    assert_eq!(panici_fuori_dalle_barriere(), base);
    assert_eq!(esegui_invocazione_dal(&catalogo, &None, base).codice, 0);
}
