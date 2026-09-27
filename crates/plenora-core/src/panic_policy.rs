//! Politica di processo per i panici, valida per la CLI **e** per chi ci usa
//! come libreria (errori-e-limiti.md#panic-policy).
//!
//! L'hook di `std` stampa su stderr il payload del panico prima dell'unwinding,
//! e quel testo puo' contenere valori di riga (un `assert_eq!` in una
//! dipendenza); le barriere `catch_unwind` arrivano dopo. La politica sta qui
//! perche' la raggiungano sia la CLI sia un embedder (per esempio `PyO3`).
//!
//! L'installazione e' **esplicita** e **idempotente** ([`Once`]): l'hook e'
//! stato globale del processo. E' un **protocollo cooperativo**, non una
//! garanzia: `std::panic::set_hook` resta pubblico e chiunque puo' sostituire
//! l'hook dopo di noi; si garantisce solo che dopo un [`install`] riuscito
//! l'hook di `std` non e' attivo e nessun altro [`install`] lo cambia. Chi
//! non chiama nulla resta con l'hook di `std`, residuo dichiarato. Con esito
//! `false` la CLI (prima istruzione di `main`)
//! dichiara nell'envelope che «stderr vuoto» non e' piu' garantito; un
//! embedder non assume che il payload sia sanitizzato.
//!
//! [`PanicPolicy::Silent`] non stampa nulla (la CLI pubblica l'envelope su
//! stdout, exit code 70); [`PanicPolicy::Sanitized`] stampa posizione nel
//! sorgente e forma del payload, mai il payload.

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
/// **Il valore di ritorno va guardato**: `false` significa solo «non l'ho
/// installato io», e anche dopo `true` un `std::panic::set_hook` altrui puo'
/// rimpiazzare l'hook (vedi il doc del modulo).
///
/// Non esiste `uninstall`: ripristinare l'hook precedente riaprirebbe la
/// pubblicazione del payload.
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

/// La FORMA del payload di un panico, senza il contenuto.
///
/// Distingue i tre casi che `std` puo' produrre senza leggere il contenuto di
/// nessuno di essi. E' la nozione unica usata dalle barriere del trasporto,
/// della CLI e dell'isolamento: chi la traduce (sul filo, nel dominio) fa un
/// `match` esaustivo su questo enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormaPayload {
    /// `panic!("letterale")`: un `&'static str`.
    Statico,
    /// `panic!("{}", valore)`: una `String`.
    Dinamico,
    /// `std::panic::panic_any` con un altro tipo.
    NonTestuale,
}

impl FormaPayload {
    /// La forma di un payload, letta dal tipo e mai dal contenuto.
    #[must_use]
    pub fn di(payload: &(dyn std::any::Any + Send)) -> Self {
        if payload.is::<&'static str>() {
            Self::Statico
        } else if payload.is::<String>() {
            Self::Dinamico
        } else {
            Self::NonTestuale
        }
    }

    /// Il testo pubblico della forma.
    #[must_use]
    pub const fn descrizione(self) -> &'static str {
        match self {
            Self::Statico => "payload statico (contenuto non pubblicato)",
            Self::Dinamico => "payload dinamico (contenuto non pubblicato)",
            Self::NonTestuale => "payload non testuale",
        }
    }
}

/// Descrizione della FORMA del payload di un panico ([`FormaPayload`]).
#[must_use]
pub fn forma_payload(payload: &(dyn std::any::Any + Send)) -> &'static str {
    FormaPayload::di(payload).descrizione()
}

std::thread_local! {
    /// Quante barriere di dipendenza sono aperte sullo stack di questo thread.
    static BARRIERE_APERTE: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

/// Chiude una barriera anche quando il lavoro risale in panico.
struct BarrieraAperta;

impl BarrieraAperta {
    fn apri() -> Self {
        BARRIERE_APERTE.with(|aperte| aperte.set(aperte.get().saturating_add(1)));
        Self
    }
}

impl Drop for BarrieraAperta {
    fn drop(&mut self) {
        BARRIERE_APERTE.with(|aperte| aperte.set(aperte.get().saturating_sub(1)));
    }
}

/// Esegue `lavoro` dentro una **barriera di dipendenza**.
///
/// E' un `catch_unwind` per una dipendenza nominata che va in panico su
/// ingressi ordinari (`relate` di `geo`, `fb_to_schema` di `arrow-ipc`).
///
/// A differenza di una rete di sicurezza attorno a codice nostro, qui il
/// panico e' **atteso**: chi sorveglia i panici (l'hook dei target di fuzz)
/// lo distingue con [`dentro_una_barriera_di_dipendenza`]
/// (errori-e-limiti.md#il-fuzzing-tollera-solo-i-panici-attesi-delle-dipendenze).
///
/// Il lavoro contiene la sola chiamata alla dipendenza: un panico di codice
/// nostro dentro la barriera sparirebbe dal fuzz.
///
/// # Errors
///
/// Il payload del panico, come `catch_unwind`.
pub fn barriera_di_dipendenza<T>(
    lavoro: impl FnOnce() -> T + std::panic::UnwindSafe,
) -> std::thread::Result<T> {
    let _aperta = BarrieraAperta::apri();
    std::panic::catch_unwind(lavoro)
}

/// Se il thread corrente sta eseguendo dentro una [`barriera_di_dipendenza`].
///
/// Vale anche dentro l'hook di panico, che gira **prima** dell'unwinding:
/// la barriera si chiude solo quando `catch_unwind` ha reso.
#[must_use]
pub fn dentro_una_barriera_di_dipendenza() -> bool {
    BARRIERE_APERTE.with(|aperte| aperte.get() > 0)
}

#[cfg(test)]
mod tests {
    use super::{
        barriera_di_dipendenza, dentro_una_barriera_di_dipendenza, forma_payload, install,
        PanicPolicy,
    };

    #[test]
    fn la_barriera_e_visibile_durante_il_lavoro_e_si_chiude_dopo() {
        assert!(!dentro_una_barriera_di_dipendenza());
        let dentro =
            barriera_di_dipendenza(dentro_una_barriera_di_dipendenza).expect("nessun panico");
        assert!(dentro, "dentro la barriera la domanda deve rispondere si'");
        assert!(!dentro_una_barriera_di_dipendenza());

        let annidata = barriera_di_dipendenza(|| {
            barriera_di_dipendenza(|| ()).expect("nessun panico");
            dentro_una_barriera_di_dipendenza()
        })
        .expect("nessun panico");
        assert!(
            annidata,
            "chiudere la barriera interna non chiude l'esterna"
        );
        assert!(!dentro_una_barriera_di_dipendenza());
    }

    #[test]
    fn la_barriera_si_chiude_anche_dopo_un_panico() {
        let esito = barriera_di_dipendenza(|| -> () { std::panic::panic_any("prova") });
        assert!(esito.is_err());
        assert!(
            !dentro_una_barriera_di_dipendenza(),
            "un panico non deve lasciare la barriera aperta"
        );
    }

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
        // chiamarlo dopo di noi. Il nome del test dice quell'ambito e non di
        // piu': «non rientrante» si leggerebbe come una garanzia sull'intero
        // processo, che questo test non puo' dare.
        let primo = install(PanicPolicy::Silent);
        let secondo = install(PanicPolicy::Sanitized);
        let terzo = install(PanicPolicy::Silent);
        assert!(
            !secondo && !terzo,
            "solo la prima installazione puo' avere effetto (prima: {primo})"
        );
    }
}
