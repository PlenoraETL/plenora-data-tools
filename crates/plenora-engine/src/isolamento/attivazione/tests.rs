use super::{autorizza_profilo_isolato, politica_da, verifica_piattaforma, RifiutoPolitica};
use plenora_core::error::PlenoraError;

// --- verifica_piattaforma ---------------------------------------------------
//
// Presa dei fatti e giudizio separati apposta: questi test provano il
// giudizio su OGNI piattaforma, non solo quella su cui girano davvero.

#[test]
fn nessuna_richiesta_passa_su_qualunque_sistema() {
    for sistema in ["linux", "windows", "macos", "freebsd"] {
        assert!(
            verifica_piattaforma(false, sistema).is_ok(),
            "senza richiesta di isolamento il sistema {sistema} non deve mai rifiutare"
        );
    }
}

#[test]
fn una_richiesta_su_linux_passa() {
    assert!(verifica_piattaforma(true, "linux").is_ok());
}

#[test]
fn una_richiesta_fuori_da_linux_e_unsupported_e_nomina_il_sistema() {
    for sistema in ["windows", "macos", "freebsd"] {
        let errore = verifica_piattaforma(true, sistema)
            .expect_err("una richiesta di isolamento fuori da linux deve essere rifiutata");
        assert!(
            matches!(&errore, PlenoraError::Unsupported(_)),
            "atteso Unsupported, ottenuto {errore:?}"
        );
        let testo = errore.to_string();
        assert!(testo.contains(sistema), "{sistema}: {testo}");
    }
}

// --- politica_da (giudizio sulla lettura dell'ambiente) ---------------------

#[test]
fn politica_assente_e_rifiutata() {
    assert_eq!(politica_da(None), Err(RifiutoPolitica::Assente));
}

#[test]
fn politica_non_numerica_e_rifiutata() {
    assert_eq!(
        politica_da(Some("non-un-numero")),
        Err(RifiutoPolitica::NonNumerica)
    );
    assert_eq!(politica_da(Some("")), Err(RifiutoPolitica::NonNumerica));
    assert_eq!(
        politica_da(Some("1024x")),
        Err(RifiutoPolitica::NonNumerica)
    );
    assert_eq!(politica_da(Some("-1")), Err(RifiutoPolitica::NonNumerica));
}

#[test]
fn politica_zero_e_rifiutata_distintamente() {
    assert_eq!(politica_da(Some("0")), Err(RifiutoPolitica::Zero));
}

#[test]
fn politica_valida_e_accettata_con_spazi_tollerati() {
    assert_eq!(politica_da(Some("1073741824")), Ok(1_073_741_824));
    assert_eq!(politica_da(Some("  1073741824  ")), Ok(1_073_741_824));
}

#[test]
fn ogni_rifiuto_di_politica_e_isolation_unavailable_e_nomina_la_variabile() {
    for rifiuto in [
        RifiutoPolitica::Assente,
        RifiutoPolitica::NonNumerica,
        RifiutoPolitica::Zero,
    ] {
        let errore = rifiuto.detto();
        assert!(
            matches!(&errore, PlenoraError::IsolationUnavailable(_)),
            "atteso IsolationUnavailable, ottenuto {errore:?}"
        );
        assert!(errore.to_string().contains(super::VARIABILE_POLITICA_HOST));
    }
}

// --- autorizza_profilo_isolato (ritaglio min() e coerenza col governato) ---

const GOVERNATO: u64 = 512 * 1024 * 1024;

#[test]
fn richiesta_entro_il_limite_host_e_concessa_per_intero() {
    let concessione =
        autorizza_profilo_isolato(2 * GOVERNATO, GOVERNATO, 4 * GOVERNATO).expect("concessa");
    assert_eq!(concessione.richiesto_byte, 2 * GOVERNATO);
    assert_eq!(concessione.concesso_byte, 2 * GOVERNATO);
}

#[test]
fn richiesta_oltre_il_limite_host_e_ritagliata_al_limite() {
    // Il limite dell'host resta sopra il governato: il ritaglio e' legittimo,
    // non un'incoerenza.
    let limite_host = 3 * GOVERNATO;
    let concessione = autorizza_profilo_isolato(10 * GOVERNATO, GOVERNATO, limite_host)
        .expect("concessa, ritagliata");
    assert_eq!(concessione.richiesto_byte, 10 * GOVERNATO);
    assert_eq!(
        concessione.concesso_byte, limite_host,
        "min(richiesta, host)"
    );
}

#[test]
fn richiesta_uguale_al_governato_e_concessa_al_limite() {
    let concessione = autorizza_profilo_isolato(GOVERNATO, GOVERNATO, GOVERNATO)
        .expect("il pavimento e' incluso");
    assert_eq!(concessione.concesso_byte, GOVERNATO);
}

#[test]
fn ritaglio_sotto_il_governato_e_rifiutato_prima_di_qualunque_spawn() {
    // Il piano ha chiesto un tetto valido (sopra il governato, altrimenti
    // PLAN-011 lo avrebbe gia' respinto in validazione); e' la politica
    // dell'host, applicata DOPO, a renderlo incoerente col governato.
    let richiesto = 2 * GOVERNATO;
    let limite_host = GOVERNATO / 2; // sotto il pavimento
    let errore = autorizza_profilo_isolato(richiesto, GOVERNATO, limite_host)
        .expect_err("un tetto concesso sotto il governato deve essere rifiutato");
    assert!(
        matches!(&errore, PlenoraError::IsolationUnavailable(_)),
        "atteso IsolationUnavailable, ottenuto {errore:?}"
    );
    let testo = errore.to_string();
    assert!(testo.contains(&limite_host.to_string()), "{testo}");
    assert!(testo.contains(&GOVERNATO.to_string()), "{testo}");
    // Non modifica implicitamente il tetto ne' lo silenzia: nessuna
    // `ConcessioneDominio` esiste da questo ramo per costruzione — la
    // funzione rende `Result`, e senza `Ok` non c'e' valore da passare a
    // uno spawner. E' la stessa prova strutturale usata per
    // `verifica_piattaforma`: un chiamante non puo' ottenere un tetto da
    // usare senza prima ottenere `Ok` da questa funzione.
}

#[test]
fn nessun_default_permissivo_su_limite_host_a_zero() {
    // `politica_da` rifiuta zero a monte; qui si prova che la funzione di
    // autorizzazione non lo tratterebbe comunque come "nessun limite" se
    // qualcuno le passasse zero direttamente.
    let errore = autorizza_profilo_isolato(GOVERNATO, GOVERNATO, 0)
        .expect_err("un limite host a zero non concede mai il governato");
    assert!(matches!(&errore, PlenoraError::IsolationUnavailable(_)));
}

// --- lettura reale dell'ambiente (Linux soltanto) --------------------------
//
// L'unico test di questo file che tocca la variabile d'ambiente per davvero:
// nessun altro test in questo processo la legge o la scrive, quindi non c'e'
// una corsa da evitare fra thread di test paralleli — la stessa ragione per
// cui `politica_da` sopra e' provata pura, senza toccare l'ambiente affatto.
#[cfg(target_os = "linux")]
#[test]
fn lettura_reale_della_politica_dell_host() {
    use super::{
        leggi_politica_dell_host, prepara_e_autorizza, richiesta_isolamento_non_ancora_servibile,
        VARIABILE_POLITICA_HOST,
    };

    let originale = std::env::var(VARIABILE_POLITICA_HOST).ok();

    std::env::remove_var(VARIABILE_POLITICA_HOST);
    assert!(
        matches!(
            leggi_politica_dell_host(),
            Err(PlenoraError::IsolationUnavailable(_))
        ),
        "assente: deve rifiutare, mai un default permissivo"
    );

    std::env::set_var(VARIABILE_POLITICA_HOST, "non-un-numero");
    assert!(matches!(
        leggi_politica_dell_host(),
        Err(PlenoraError::IsolationUnavailable(_))
    ));

    std::env::set_var(VARIABILE_POLITICA_HOST, "0");
    assert!(matches!(
        leggi_politica_dell_host(),
        Err(PlenoraError::IsolationUnavailable(_))
    ));

    std::env::set_var(VARIABILE_POLITICA_HOST, "1073741824");
    let letta = leggi_politica_dell_host().expect("la politica valida deve leggersi");
    assert_eq!(letta, 1_073_741_824);

    // La composizione lettura+giudizio: stessa politica, richiesta sopra il
    // limite dell'host, ritagliata di conseguenza.
    let concessione = prepara_e_autorizza(4 * GOVERNATO, GOVERNATO).expect("concessa e ritagliata");
    assert_eq!(concessione.richiesto_byte, 4 * GOVERNATO);
    assert_eq!(
        concessione.concesso_byte, 1_073_741_824,
        "min(richiesta, host)"
    );

    // Anche quando l'autorizzazione riuscirebbe, `execute` chiamato
    // direttamente (bypassando `esecuzione_isolata::esegui_isolato`) resta
    // un rifiuto, mai un'esecuzione.
    let errore = richiesta_isolamento_non_ancora_servibile(GOVERNATO, GOVERNATO);
    assert!(
        matches!(&errore, PlenoraError::IsolationUnavailable(_)),
        "{errore:?}"
    );
    assert!(errore.to_string().contains("esegui_isolato"), "{errore}");

    match originale {
        Some(valore) => std::env::set_var(VARIABILE_POLITICA_HOST, valore),
        None => std::env::remove_var(VARIABILE_POLITICA_HOST),
    }
}

// --- richiesta_isolamento_non_ancora_servibile ------------------------------
//
// Il ramo Linux (autorizzazione riuscita ma nessun chiamante) e' provato
// dentro `lettura_reale_della_politica_dell_host`, sopra: e' l'unico test che
// tocca la variabile d'ambiente, e restarci evita una seconda corsa.

#[cfg(not(target_os = "linux"))]
#[test]
fn fuori_da_linux_rifiuta_come_verifica_piattaforma() {
    use super::richiesta_isolamento_non_ancora_servibile;

    let errore = richiesta_isolamento_non_ancora_servibile(GOVERNATO, GOVERNATO);
    assert!(
        matches!(&errore, PlenoraError::Unsupported(_)),
        "{errore:?}"
    );
}
