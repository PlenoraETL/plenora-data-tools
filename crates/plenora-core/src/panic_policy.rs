//! Politica di processo per i panici, per chi usa il workspace come
//! libreria (un eseguibile, un embedder come `PyO3`).
//!
//! L'hook di `std` stampa su stderr il payload del panico prima dell'unwinding,
//! e quel testo può contenere valori di riga (un `assert_eq!` in una
//! dipendenza); le barriere `catch_unwind` arrivano dopo. La politica sta qui
//! perché la raggiunga chiunque chiami i kernel.
//!
//! L'installazione è **esplicita** e **idempotente** ([`Once`]): l'hook è
//! stato globale del processo. È un **protocollo cooperativo**, non una
//! garanzia: `std::panic::set_hook` resta pubblico e chiunque può sostituire
//! l'hook dopo di noi; si garantisce solo che dopo un [`install`] riuscito
//! l'hook di `std` non è attivo e nessun altro [`install`] lo cambia. Chi
//! non chiama nulla resta con l'hook di `std`, residuo dichiarato. Con esito
//! `false` il chiamante non può assumere che il payload sia sanitizzato.
//!
//! [`PanicPolicy::Silent`] non stampa nulla (chi la sceglie pubblica il
//! panico su un canale proprio); [`PanicPolicy::Sanitized`] stampa
//! posizione nel sorgente e forma del payload, mai il payload.

use std::panic::PanicHookInfo;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Once;

/// Cosa deve fare l'hook quando un panico attraversa il processo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanicPolicy {
    /// Nessun output. Chi la sceglie si impegna a intercettare il panico e a
    /// pubblicarlo su un proprio canale (lo faceva la CLI del progetto
    /// d'origine).
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
            PanicPolicy::Silent => std::panic::set_hook(Box::new(|_| {
                registra_panico(dentro_una_barriera_di_dipendenza());
            })),
            PanicPolicy::Sanitized => {
                std::panic::set_hook(Box::new(|info| {
                    use std::io::Write as _;
                    registra_panico(dentro_una_barriera_di_dipendenza());
                    // `eprintln!` andrebbe in panico se stderr fosse chiuso,
                    // e un panico dentro l'hook aborta il processo: si scrive
                    // ignorando l'esito.
                    let mut stderr = std::io::stderr();
                    let _ = writeln!(stderr, "{}", riga_sanitizzata(info));
                }));
            }
        }
    });
    installato
}

/// Panici fuori da una [`barriera_di_dipendenza`] osservati dall'hook di
/// [`install`], da qualunque thread.
static PANICI_FUORI_DALLE_BARRIERE: AtomicU64 = AtomicU64::new(0);

/// Conta un panico, se non è dentro una barriera (lì è atteso e diventa un
/// errore della barriera). La provano, con delta esatti e in un processo
/// tutto suo, `tests/conto_dei_panici.rs` e il test omologo della CLI.
fn registra_panico(dentro_una_barriera: bool) {
    if !dentro_una_barriera {
        PANICI_FUORI_DALLE_BARRIERE.fetch_add(1, Ordering::AcqRel);
    }
}

/// Quanti panici fuori dalle barriere di dipendenza l'hook installato da
/// [`install`] ha visto finora, in qualunque thread (senza payload).
///
/// Serve a chi non può intercettare un panico con `catch_unwind`: un thread
/// staccato che muore in silenzio (con [`PanicPolicy::Silent`] nessuno lo
/// vede) lascia comunque il conto cresciuto. La CLI `plenora-data` lo legge
/// prima di dichiarare un successo: un conto cambiato durante
/// l'invocazione la fa finire con un errore `internal`. Senza un
/// [`install`] riuscito il conto resta zero.
#[must_use]
pub fn panici_fuori_dalle_barriere() -> u64 {
    PANICI_FUORI_DALLE_BARRIERE.load(Ordering::Acquire)
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
/// Distingue i tre casi che `std` può produrre senza leggere il contenuto di
/// nessuno di essi. È la nozione unica usata dalle barriere attorno alle
/// dipendenze (qui quelle dei kernel geo): chi la traduce fa un `match`
/// esaustivo su questo enum.
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
/// ingressi ordinari (`relate` di `geo`, `arrow-ipc` e `parquet` su file
/// malformati).
///
/// A differenza di una rete di sicurezza attorno a codice nostro, qui il
/// panico è **atteso**: chi sorveglia i panici (per esempio l'hook di un
/// target di fuzz, che deve tollerare solo i panici attesi delle
/// dipendenze) lo distingue con [`dentro_una_barriera_di_dipendenza`].
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
        // I testi sono pubblici: finiscono negli errori. Si fissano tutti e
        // tre.
        assert_eq!(
            forma_payload(statico.as_ref()),
            "payload statico (contenuto non pubblicato)"
        );
        assert_eq!(
            forma_payload(dinamico.as_ref()),
            "payload dinamico (contenuto non pubblicato)"
        );
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
