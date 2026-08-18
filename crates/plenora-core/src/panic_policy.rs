//! Politica di processo per i panici — valida per la CLI **e** per chi ci usa
//! come libreria.
//!
//! ## Il problema
//!
//! L'hook di panico installato da `std` stampa su stderr il payload del
//! panico **prima** che l'unwinding cominci. Il testo di un panico non e'
//! scritto da noi: un `assert_eq!` dentro una dipendenza puo' includere i
//! VALORI confrontati, cioe' dati della riga in lavorazione. Il progetto ha
//! gia' sanitizzato ogni percorso in cui quel testo diventa un *errore*
//! (`ArrowTransportError::ArrowPanic`, il confine IPC, `panic_step_error`),
//! ma la stampa dell'hook precede tutti quei percorsi e non passa da nessuno
//! di essi: la barriera `catch_unwind` di [`crate`] converte il panico in
//! errore, non impedisce all'hook di averlo gia' pubblicato.
//!
//! Finora l'hook veniva silenziato solo nel `main` della CLI. Per un
//! consumatore che ci carica come libreria — in particolare per il futuro
//! binding `PyO3`, dove lo stderr del processo e' quello dell'interprete Python
//! dell'utente — la garanzia non esisteva.
//!
//! ## La politica
//!
//! L'hook e' uno **stato globale del processo** e non e' scambiabile in modo
//! innocuo: sostituirlo di nascosto dall'interno di una libreria romperebbe
//! l'hook di chi ci ospita (un runtime di test, `libfuzzer-sys`, un
//! supervisore che raccoglie i backtrace). Quindi:
//!
//! - l'installazione e' **esplicita**: nessun `ctor`, nessun effetto
//!   collaterale all'import, nessuna installazione implicita dentro i kernel;
//! - l'installazione e' **idempotente** ([`Once`]): la prima chiamata a
//!   [`install`] vince, le successive rispondono `false` e non toccano
//!   l'hook. Due componenti che passano DA QUI non possono sovrascriversi a
//!   vicenda;
//! - chi non chiama nulla resta con l'hook di `std`. E' un residuo
//!   dichiarato, non un difetto nascosto: registrato come **DER-010** in
//!   `docs/deroghe.md`, con regola derogata, hazard e condizione di rientro.
//!
//! ## Che cosa questo modulo NON garantisce
//!
//! [`Once`] governa soltanto le chiamate a [`install`]. `std::panic::set_hook`
//! resta una funzione pubblica di `std`: qualunque componente del processo —
//! una dipendenza, un runtime di test, codice dell'embedder — puo' chiamarla
//! **dopo** di noi e riaprire la pubblicazione del payload. Non esiste modo,
//! in Rust stabile, di rendere un hook non sostituibile.
//!
//! Questo modulo e' quindi un **protocollo cooperativo**, non una garanzia
//! imponibile: vale finche' tutti i componenti del processo passano da qui.
//! Chiamarlo garanzia sarebbe una promessa che il codice non puo' mantenere,
//! e una promessa falsa e' peggio di un residuo dichiarato — chi la legge
//! smette di controllare.
//!
//! Cio' che il modulo garantisce davvero e' piu' modesto e vero:
//!
//! 1. l'hook di `std`, che pubblica il payload, non e' piu' quello attivo
//!    dopo una chiamata a [`install`] andata a buon fine;
//! 2. nessun altro chiamante di [`install`] puo' sostituire quella scelta;
//! 3. le barriere `catch_unwind` del progetto continuano a produrre errori
//!    sanitizzati indipendentemente da quale hook sia installato — quella e'
//!    una proprieta' del codice, non una convenzione.
//!
//! ## Che cosa deve fare il chiamante con l'esito
//!
//! [`install`] restituisce `false` quando qualcun altro era arrivato prima.
//! Le regole del progetto:
//!
//! - **CLI**: e' il processo, ed e' la prima istruzione di `main`. Un `false`
//!   li' significa che qualcosa ha installato un hook prima del nostro
//!   ingresso, cioe' che il contratto «stderr vuoto» non e' piu' garantito.
//!   La CLI lo registra nel proprio stato e lo dichiara nell'envelope di un
//!   eventuale panico, invece di ignorarlo.
//! - **Embedder / binding Python**: non e' il processo. Un `false` e'
//!   normale — l'ospite puo' avere una propria politica, e sovrascriverla
//!   sarebbe il danno che questo modulo evita. L'embedder deve limitarsi a
//!   non ASSUMERE che il payload sia sanitizzato.
//!
//! ## Cosa resta osservabile
//!
//! [`PanicPolicy::Silent`] non stampa nulla — e' il contratto «stderr vuoto»
//! della CLI, dove l'informazione viene recuperata dalla barriera e
//! pubblicata come envelope su stdout con exit code 70.
//!
//! [`PanicPolicy::Sanitized`] stampa **una riga** con la posizione nel
//! sorgente e la FORMA del payload, mai il payload. La posizione e' un
//! percorso di file e un numero di riga del nostro codice o di una
//! dipendenza: e' una proprieta' del programma, non del dato. E' la politica
//! giusta per un embedder, che altrimenti perderebbe ogni traccia di un
//! panico avvenuto dentro di noi.

use std::panic::PanicHookInfo;
use std::sync::Once;

/// Cosa deve fare l'hook quando un panico attraversa il processo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanicPolicy {
    /// Nessun output. Chi la sceglie si impegna a intercettare il panico e a
    /// pubblicarlo su un proprio canale: e' quello che fa la CLI.
    Silent,
    /// Una riga su stderr con posizione e forma del payload, mai il payload.
    Sanitized,
}

static INSTALLAZIONE: Once = Once::new();

/// Installa la politica indicata, **una volta sola** per processo.
///
/// Restituisce `true` se questa chiamata ha installato l'hook, `false` se
/// un'altra chiamata a questa stessa funzione l'aveva gia' fatto: nel secondo
/// caso l'hook esistente resta intatto e la politica richiesta viene
/// ignorata.
///
/// **Il valore di ritorno va guardato.** `false` non significa «l'hook e' il
/// nostro»: significa solo «non l'ho installato io». E anche `true` non
/// impedisce a un `std::panic::set_hook` successivo, fatto da chiunque altro
/// nel processo, di rimpiazzarlo — vedi la sezione «Che cosa questo modulo
/// NON garantisce» in testa al modulo. Chi dichiara ai propri utenti una
/// garanzia sul canale d'errore deve tenerne conto.
///
/// Non e' un'operazione reversibile: non esiste una `uninstall`. Ripristinare
/// l'hook precedente significherebbe riaprire la pubblicazione del payload,
/// che e' esattamente cio' che questa politica chiude.
pub fn install(policy: PanicPolicy) -> bool {
    let mut installato = false;
    INSTALLAZIONE.call_once(|| {
        installato = true;
        match policy {
            PanicPolicy::Silent => std::panic::set_hook(Box::new(|_| {})),
            PanicPolicy::Sanitized => {
                std::panic::set_hook(Box::new(|info| {
                    // `eprintln!` andrebbe in panico se stderr fosse chiuso,
                    // e un panico dentro l'hook aborta il processo: si scrive
                    // ignorando l'esito.
                    use std::io::Write as _;
                    let mut stderr = std::io::stderr();
                    let _ = writeln!(stderr, "{}", riga_sanitizzata(info));
                }));
            }
        }
    });
    installato
}

/// Riga pubblicata da [`PanicPolicy::Sanitized`].
///
/// Separata dall'hook per poter essere verificata da un test senza provocare
/// un panico vero.
#[must_use]
pub fn riga_sanitizzata(info: &PanicHookInfo<'_>) -> String {
    let posizione = info.location().map_or_else(
        || "posizione sconosciuta".to_owned(),
        |location| {
            format!(
                "{}:{}:{}",
                location.file(),
                location.line(),
                location.column()
            )
        },
    );
    format!(
        "plenora: panico interno a {posizione} ({}); \
         nessun contenuto del payload viene pubblicato",
        forma_payload(info.payload())
    )
}

/// Descrizione della FORMA del payload di un panico.
///
/// Distingue i tre casi che `std` puo' produrre senza leggere il contenuto di
/// nessuno di essi. E' la stessa nozione usata dalle barriere del trasporto e
/// della CLI: qui vive la versione condivisa.
#[must_use]
pub fn forma_payload(payload: &(dyn std::any::Any + Send)) -> &'static str {
    if payload.is::<&'static str>() {
        "payload statico (contenuto non pubblicato)"
    } else if payload.is::<String>() {
        "payload dinamico (contenuto non pubblicato)"
    } else {
        "payload non testuale"
    }
}

#[cfg(test)]
mod tests {
    use super::{forma_payload, install, PanicPolicy};

    #[test]
    fn la_forma_del_payload_non_ne_pubblica_il_contenuto() {
        let statico: Box<dyn std::any::Any + Send> = Box::new("segreto-statico");
        let dinamico: Box<dyn std::any::Any + Send> = Box::new("segreto-dinamico".to_owned());
        let altro: Box<dyn std::any::Any + Send> = Box::new(42_u32);

        for payload in [&statico, &dinamico, &altro] {
            let forma = forma_payload(payload.as_ref());
            assert!(
                !forma.contains("segreto") && !forma.contains("42"),
                "la forma non deve contenere il payload: {forma}"
            );
        }
        assert_ne!(
            forma_payload(statico.as_ref()),
            forma_payload(dinamico.as_ref()),
            "le tre forme restano distinguibili"
        );
        assert_eq!(forma_payload(altro.as_ref()), "payload non testuale");
    }

    #[test]
    fn l_installazione_e_idempotente_fra_le_chiamate_a_questa_api() {
        // Il primo che passa DI QUI vince. E' tutto cio' che `Once` puo'
        // fare: `std::panic::set_hook` resta pubblico e chiunque puo'
        // chiamarlo dopo di noi. Il nome del test dice l'ambito reale della
        // proprieta', perche' prima diceva «non rientrante» e si leggeva come
        // una garanzia sull'intero processo.
        let primo = install(PanicPolicy::Silent);
        let secondo = install(PanicPolicy::Sanitized);
        let terzo = install(PanicPolicy::Silent);
        assert!(
            !secondo && !terzo,
            "solo la prima installazione puo' avere effetto (prima: {primo})"
        );
    }
}
