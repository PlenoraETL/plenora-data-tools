//! I casi del percorso: cio' che non deve poter sfuggire.

use crate::isolamento::figlio::{FiglioVivo, ProcessoFiglio, Uscita};

use super::chiudi;

/// Un figlio che protesta in **tutti e due** i tempi della chiusura.
///
/// Il primo sguardo non risponde — e' la cortesia che incontra un difetto — il
/// secondo lo trova vivo, cosi' la terminazione viene tentata davvero e
/// protesta, e il terzo lo raccoglie. E' l'unico modo di attraversare entrambe
/// le fasi in un caso solo, e serve proprio a questo: i difetti della prima non
/// devono sparire passando alla seconda.
///
/// Il caso in cui la raccolta non riesce affatto non si prova qui: li' la
/// sentinella ferma il processo, ed e' proprio cio' che deve fare — un caso che
/// lo esercitasse fermerebbe il runner.
struct CheProtesta {
    sguardi: std::cell::Cell<u8>,
}

impl ProcessoFiglio for &CheProtesta {
    fn pid(&self) -> u32 {
        4242
    }

    fn prova_a_raccogliere(&mut self) -> std::io::Result<Option<Uscita>> {
        let quanti = self.sguardi.get();
        self.sguardi.set(quanti + 1);
        match quanti {
            0 => Err(std::io::Error::other("questo figlio non si interroga")),
            1 => Ok(None),
            _ => Ok(Some(Uscita::Segnale(9))),
        }
    }

    fn termina(&mut self) -> std::io::Result<()> {
        Err(std::io::Error::other("questo figlio non si termina"))
    }
}

/// Un figlio gia' uscito, che si raccoglie al primo colpo.
struct CheEUscito;

impl ProcessoFiglio for CheEUscito {
    fn pid(&self) -> u32 {
        4243
    }

    fn prova_a_raccogliere(&mut self) -> std::io::Result<Option<Uscita>> {
        Ok(Some(Uscita::Codice(0)))
    }

    fn termina(&mut self) -> std::io::Result<()> {
        panic!("non si termina un figlio gia' raccolto")
    }
}

/// **I difetti dei due tempi si sommano, e nessuno sparisce.**
///
/// # Che cosa esclude
///
/// Due perdite diverse. Che una terminazione andata storta passi inosservata
/// perche' la raccolta poi e' riuscita; e che il difetto della **cortesia** —
/// il primo tempo — venga buttato via passando al secondo. Il secondo caso e'
/// il piu' facile da introdurre: basta cominciare la terminazione con una lista
/// vuota invece che con quella gia' raccolta.
///
/// `chiudi` non fallisce — un difetto di pulizia non e' l'esito del percorso —
/// quindi l'unica cosa che li rende visibili e' che vengano **riportati**.
#[test]
fn i_difetti_dei_due_tempi_si_sommano() {
    let finto = CheProtesta {
        sguardi: std::cell::Cell::new(0),
    };
    let (uscita, difetti) = chiudi(FiglioVivo::nuovo(&finto), None);

    assert_eq!(
        uscita,
        Some(Uscita::Segnale(9)),
        "il figlio e' stato raccolto"
    );
    assert!(
        difetti
            .iter()
            .any(|detto| detto.contains("non si interroga")),
        "il difetto della cortesia deve sopravvivere al secondo tempo: {difetti:?}"
    );
    assert!(
        difetti.iter().any(|detto| detto.contains("non si termina")),
        "e anche quello della terminazione: {difetti:?}"
    );
}

/// Un figlio gia' uscito si raccoglie, e non lascia difetti.
///
/// E' il cammino normale, e serve a fissare che il caso precedente misuri il
/// difetto e non `chiudi` in generale.
#[test]
fn un_figlio_gia_uscito_non_lascia_difetti() {
    let (uscita, difetti) = chiudi(FiglioVivo::nuovo(CheEUscito), None);

    assert_eq!(uscita, Some(Uscita::Codice(0)));
    assert!(difetti.is_empty(), "{difetti:?}");
}

/// Un figlio che ci mette un attimo a finire, e non va segnalato.
///
/// Il `Cell` conta gli sguardi: i primi due lo trovano vivo — e' la finestra fra
/// «ha chiuso il canale» e «il kernel ha registrato l'uscita» — poi esce con
/// zero. Se qualcuno lo terminasse, `termina` lo direbbe.
struct CheStaFinendo {
    sguardi: std::cell::Cell<u8>,
    segnalato: std::cell::Cell<bool>,
}

impl ProcessoFiglio for &CheStaFinendo {
    fn pid(&self) -> u32 {
        4244
    }

    fn prova_a_raccogliere(&mut self) -> std::io::Result<Option<Uscita>> {
        let quanti = self.sguardi.get();
        self.sguardi.set(quanti + 1);
        if quanti < 2 {
            Ok(None)
        } else {
            Ok(Some(Uscita::Codice(0)))
        }
    }

    fn termina(&mut self) -> std::io::Result<()> {
        self.segnalato.set(true);
        Ok(())
    }
}

/// **Un worker che sta uscendo non viene ucciso per una corsa.**
///
/// # Che cosa esclude
///
/// Il difetto piu' silenzioso di tutti: un percorso **riuscito** che riporta
/// `Segnale(9)` perche' il primo sguardo ha trovato il figlio ancora vivo. Il
/// tetto della raccolta misura l'attesa **dopo** il segnale, quindi non
/// protegge da niente qui: senza una cortesia prima, ogni worker piu' lento di
/// uno sguardo verrebbe terminato — e il referto direbbe che il worker e' stato
/// fermato, quando invece stava finendo.
///
/// Il caso non guarda i tempi: guarda che **nessun segnale sia partito**.
#[test]
fn un_figlio_che_sta_finendo_non_viene_segnalato() {
    let finto = CheStaFinendo {
        sguardi: std::cell::Cell::new(0),
        segnalato: std::cell::Cell::new(false),
    };
    let (uscita, difetti) = chiudi(FiglioVivo::nuovo(&finto), None);

    assert_eq!(uscita, Some(Uscita::Codice(0)), "e' uscito da se'");
    assert!(
        !finto.segnalato.get(),
        "a un figlio che sta finendo non si manda niente"
    );
    assert!(difetti.is_empty(), "{difetti:?}");
}

/// Un `Osservato` qualunque: qui non conta che cosa dica, ma che sopravviva.
fn riuscito() -> super::Osservato {
    super::Osservato {
        accordo: true,
        progressi: 1,
        esito: super::EsitoOsservato::Errore("non conta".to_owned()),
        artefatto: super::Artefatto::NonDichiarato,
        fine_del_canale: true,
    }
}

/// Una causa riconoscibile nel messaggio composto.
fn causa() -> plenora_core::PlenoraError {
    // `IsolationUnavailable` di proposito: e' una delle varianti che
    // `con_contesto` **non tocca**, quindi un caso che passasse di li' non si
    // accorgerebbe di niente.
    plenora_core::PlenoraError::IsolationUnavailable("il dialogo non ha retto".to_owned())
}

/// **Un dialogo fallito non nasconde i difetti di pulizia.**
///
/// # Che cosa esclude
///
/// Che il rosso dica soltanto perche' il dialogo non ha retto, tacendo che il
/// figlio ha anche protestato uscendo. Sono due fatti: senza il secondo, chi
/// legge chiude il caso sul primo e non sa che c'e' un processo che si e'
/// comportato male.
#[test]
fn un_dialogo_fallito_porta_con_se_i_difetti_di_pulizia() {
    let difetti = vec!["non si riesce a terminare il figlio: no".to_owned()];
    let Err(composta) = super::con_la_pulizia(Err(causa()), &difetti) else {
        panic!("un dialogo fallito non diventa un successo");
    };
    let detto = composta.to_string();
    assert!(
        detto.contains("il dialogo non ha retto") && detto.contains("non si riesce a terminare"),
        "nessuno dei due fatti deve sparire: {detto}"
    );
}

/// Senza difetti la causa passa **intatta**, variante compresa.
///
/// Serve a fissare che il caso precedente misuri la composizione e non
/// `con_la_pulizia` in generale: se la funzione riscrivesse sempre l'errore,
/// perderebbe la categoria di chi la attraversa senza difetti.
#[test]
fn senza_difetti_la_causa_non_viene_riscritta() {
    let Err(intatta) = super::con_la_pulizia(Err(causa()), &[]) else {
        panic!("un dialogo fallito non diventa un successo");
    };
    assert_eq!(intatta.to_string(), causa().to_string());
    assert!(super::con_la_pulizia(Ok(riuscito()), &[]).is_ok());
}

/// **Un guardiano perduto non lascia verde un percorso, in nessuno dei due
/// versi.**
///
/// # Che cosa esclude
///
/// Che un percorso arrivi in fondo senza la garanzia che lo limita e lo dica
/// riuscito. Il caso attraversa **entrambi** i versi perche' falliscono in modi
/// diversi: su un dialogo riuscito la perdita e' l'unico fatto, e un ramo che la
/// ignorasse darebbe un verde pulito; su un dialogo fallito la perdita si
/// affianca alla causa, e un ramo che la sostituisse perderebbe il motivo vero.
#[test]
fn un_guardiano_perduto_non_lascia_verde() {
    use super::StatoDelGuardiano::{NonScaduto, Perduto, Scaduto};

    let Err(su_riuscito) = super::secondo_il_guardiano(Ok(riuscito()), &Perduto) else {
        panic!("senza garanzia temporale il percorso non qualifica");
    };
    let detto = su_riuscito.to_string();
    assert!(detto.contains("guardiano"), "{detto}");
    assert!(
        !detto.contains("e inoltre"),
        "su un dialogo riuscito non c'e' una seconda causa da nominare: {detto}"
    );

    let Err(su_fallito) = super::secondo_il_guardiano(Err(causa()), &Perduto) else {
        panic!("un dialogo fallito non diventa un successo");
    };
    let detto = su_fallito.to_string();
    assert!(
        detto.contains("guardiano") && detto.contains("il dialogo non ha retto"),
        "la perdita si affianca alla causa, non la sostituisce: {detto}"
    );

    // E gli altri due stati non inventano niente.
    assert!(super::secondo_il_guardiano(Ok(riuscito()), &NonScaduto).is_ok());
    assert!(super::secondo_il_guardiano(Ok(riuscito()), &Scaduto).is_ok());
    let Err(scaduto) = super::secondo_il_guardiano(Err(causa()), &Scaduto) else {
        panic!("un dialogo fallito non diventa un successo");
    };
    assert!(
        scaduto.to_string().contains("non ha finito entro")
            && scaduto.to_string().contains("il dialogo non ha retto"),
        "la scadenza spiega la causa, non la sostituisce: {scaduto}"
    );
}

/// Un referto che supera l'oracolo, da cui partire per romperlo.
fn referto_buono() -> super::Referto {
    super::Referto {
        immagine: "dominio",
        digest_immagine: "a".repeat(64),
        accordo: true,
        progressi: 1,
        esito: super::EsitoOsservato::Successo {
            digest: crate::protocollo::messaggi::DigestArtefatto {
                algoritmo: "sha256".to_owned(),
                valore: "b".repeat(64),
            },
            conteggi: crate::protocollo::messaggi::ConteggiDichiarati { righe: 2, batch: 1 },
        },
        artefatto: super::Artefatto::Verificato,
        fine_del_canale: true,
        uscita: Some(super::FineDelProcesso::Codice(0)),
        difetti_di_pulizia: Vec::new(),
    }
}

/// Una rottura del referto: come si chiama, e la mano che la compie.
type Rottura = (&'static str, fn(&mut super::Referto));

/// **Ogni pretesa dell'oracolo cade da sola.**
///
/// # Che cosa esclude
///
/// Che una pretesa sia scritta e non guardata. Un oracolo con otto righe e un
/// caso che ne prova una sola direbbe di giudicare otto cose e ne
/// giudicherebbe una: le altre sette resterebbero verdi qualunque cosa
/// arrivasse, ed e' esattamente il modo in cui un qualificatore certifica
/// un'esecuzione diversa da quella dichiarata.
///
/// Il referto di partenza passa; ogni riga qui sotto ne rompe **una** cosa e
/// pretende che l'oracolo se ne accorga.
#[test]
fn ogni_pretesa_dell_oracolo_cade_da_sola() {
    let atteso = "a".repeat(64);
    assert!(
        super::giudica(&referto_buono(), "dominio", Some(&atteso)).is_empty(),
        "il referto di partenza deve passare, o il caso non prova niente"
    );

    let rotture: [Rottura; 8] = [
        ("immagine", |r| r.immagine = "iterazione"),
        ("binario eseguito", |r| r.digest_immagine = "c".repeat(64)),
        ("accordo", |r| r.accordo = false),
        ("esito", |r| {
            r.esito = super::EsitoOsservato::Errore("non e' andata".to_owned());
        }),
        ("artefatto", |r| {
            r.artefatto = super::Artefatto::Respinto("il footer non regge".to_owned());
        }),
        ("progressi", |r| r.progressi = 3),
        ("canale", |r| r.fine_del_canale = false),
        ("fine del processo", |r| {
            r.uscita = Some(super::FineDelProcesso::Codice(1));
        }),
    ];

    for (che, rompi) in rotture {
        let mut referto = referto_buono();
        rompi(&mut referto);
        let manca = super::giudica(&referto, "dominio", Some(&atteso));
        assert_eq!(
            manca.len(),
            1,
            "rompendo «{che}» deve cadere **una** pretesa, e sono cadute {manca:?}"
        );
    }
}

/// **Un difetto di pulizia fa cadere il giudizio.**
///
/// Sta a parte perche' e' l'unica pretesa che non riguarda cio' che il worker ha
/// detto, ma cio' che ha lasciato: un figlio che non si e' chiuso bene non
/// invalida l'esito, e invalida la **qualificazione** — un percorso che lascia
/// processi in giro non ha provato di saperli chiudere.
#[test]
fn un_difetto_di_pulizia_fa_cadere_il_giudizio() {
    let mut referto = referto_buono();
    referto
        .difetti_di_pulizia
        .push("il figlio non si e' terminato".to_owned());

    let manca = super::giudica(&referto, "dominio", None);
    assert_eq!(manca.len(), 1, "{manca:?}");
    assert!(manca[0].contains("difetti di pulizia"), "{manca:?}");
}

/// Senza digest atteso l'oracolo non inventa un confronto.
///
/// E' la differenza fra i due percorsi: quello di iterazione non e' fissato da
/// nessun digest, e pretenderne uno lo renderebbe rosso per una cosa che nessuno
/// gli ha chiesto.
#[test]
fn senza_digest_atteso_non_si_confronta_niente() {
    let mut referto = referto_buono();
    referto.digest_immagine = "f".repeat(64);
    assert!(super::giudica(&referto, "dominio", None).is_empty());
}
