//! Prove della classificazione §10.
//!
//! Due strati che non si coprono a vicenda:
//!
//! - un **caso nominato per ogni riga normativa**, che dice a parole quale
//!   situazione sta descrivendo;
//! - un **oracolo indipendente** sul prodotto esaustivo dei quattro segnali,
//!   scritto dalla tabella e non dal `match` di produzione.
//!
//! Il secondo non sostituisce il primo. Un oracolo dice che le due
//! formulazioni concordano, non che concordino su qualcosa di giusto: se
//! entrambe sbagliassero la stessa riga, resterebbero d'accordo. I casi
//! nominati sono l'ancoraggio alla matrice.

use plenora_core::{
    DiagnosticaSupplementare, ErrorCategory, EvidenzaDiLimite, PlenoraError, PressioneDegliAntenati,
};

use super::{
    classifica, classifica_evidenza, ClasseEvidenzaMemoria, EsitoClassificato, EsitoWorker,
    FattiDopoLaQuiescenza, FormaDelPayload, Segnale,
};

// ---------------------------------------------------------------------------
// Costruzione delle evidenze
// ---------------------------------------------------------------------------

/// Evidenza con i quattro segnali dati e diagnostica neutra.
fn evidenza(ol: Option<u64>, kl: Option<u64>, kh: Option<u64>, g: Option<u64>) -> EvidenzaDiLimite {
    EvidenzaDiLimite {
        oom_locali: ol,
        uccisi_nel_dominio: kl,
        uccisi_nella_gerarchia: kh,
        group_kill_locale: g,
        oom_degli_antenati: PressioneDegliAntenati::alla_radice(),
        diagnostica: DiagnosticaSupplementare {
            tetto_byte: 64 * 1024 * 1024,
            picco_byte: Some(1024),
            respinte_al_tetto: Some(0),
        },
    }
}

/// I fatti col solo campo che interessa, tutto il resto inerte.
fn fatti_con_evidenza(prova: EvidenzaDiLimite) -> FattiDopoLaQuiescenza {
    FattiDopoLaQuiescenza::dopo_la_quiescenza(false, Some(prova), false, false, None)
}

// ---------------------------------------------------------------------------
// L'oracolo indipendente: cinque predicati MUTUAMENTE ESCLUSIVI
// ---------------------------------------------------------------------------

/// La classe attesa, derivata dalla tabella della §10.0-bis.
///
/// # Perche' i predicati sono mutuamente esclusivi
///
/// Se fossero una catena ordinata — «prima incoerente, poi attribuita, ...» —
/// l'oracolo replicherebbe l'ordine del `match` di produzione, e una
/// sovrapposizione sbagliata li ingannerebbe **entrambi** allo stesso modo.
///
/// Qui ogni predicato porta le proprie esclusioni: `attribuita` richiede
/// esplicitamente che non ci sia incoerenza, e cosi' via. L'oracolo verifica
/// poi che esattamente **uno** sia vero, il che e' una proprieta' della
/// tabella e non dell'ordine in cui la si legge.
fn classe_attesa(
    ol: Option<u64>,
    kl: Option<u64>,
    kh: Option<u64>,
    g: Option<u64>,
) -> ClasseEvidenzaMemoria {
    let osservato = |v: Option<u64>| v.is_some();
    let positivo = |v: Option<u64>| matches!(v, Some(n) if n > 0);
    let zero = |v: Option<u64>| v == Some(0);

    // Incoerenza, sui VALORI: `Kl` e' un sottoinsieme di `Kh`, e un group
    // kill locale non puo' scattare se il tetto locale non e' mai stato
    // invocato.
    let kl_oltre_kh = matches!((kl, kh), (Some(a), Some(b)) if a > b);
    let g_senza_ol = positivo(g) && zero(ol);
    // Un group kill che non ha ucciso nulla: `G` positivo con `Kh` a zero.
    let g_senza_uccisi = positivo(g) && zero(kh);
    let incoerente = kl_oltre_kh || g_senza_ol || g_senza_uccisi;

    let tutti_osservati = osservato(ol) && osservato(kl) && osservato(kh) && osservato(g);
    let tutti_positivi = positivo(ol) && positivo(kl) && positivo(kh) && positivo(g);
    let qualche_positivo = positivo(ol) || positivo(kl) || positivo(kh) || positivo(g);

    let attribuita = !incoerente && tutti_osservati && tutti_positivi;
    let assente = !incoerente && tutti_osservati && !qualche_positivo;
    let non_attribuita = !incoerente && !attribuita && qualche_positivo;
    let indeterminata = !incoerente && !tutti_osservati && !qualche_positivo;

    let vere = [
        incoerente,
        attribuita,
        assente,
        non_attribuita,
        indeterminata,
    ]
    .into_iter()
    .filter(|v| *v)
    .count();
    assert_eq!(
        vere, 1,
        "i predicati dell'oracolo devono essere mutuamente esclusivi ed \
         esaustivi: {ol:?} {kl:?} {kh:?} {g:?} ne soddisfa {vere}"
    );

    if incoerente {
        ClasseEvidenzaMemoria::Incoerente
    } else if attribuita {
        ClasseEvidenzaMemoria::Attribuita
    } else if assente {
        ClasseEvidenzaMemoria::Assente
    } else if non_attribuita {
        ClasseEvidenzaMemoria::NonAttribuita
    } else {
        ClasseEvidenzaMemoria::Indeterminata
    }
}

/// I quattro livelli che il prodotto esaustivo attraversa.
const LIVELLI: [Option<u64>; 4] = [None, Some(0), Some(1), Some(2)];

#[test]
fn il_prodotto_esaustivo_dei_quattro_segnali_concorda_con_l_oracolo() {
    // 4^4 = 256 combinazioni. Generate, non elencate: un elenco scritto a
    // mano avrebbe la stessa lacuna del codice che deve giudicare.
    let mut viste = 0_usize;
    for ol in LIVELLI {
        for kl in LIVELLI {
            for kh in LIVELLI {
                for g in LIVELLI {
                    let prova = evidenza(ol, kl, kh, g);
                    assert_eq!(
                        classifica_evidenza(&prova),
                        classe_attesa(ol, kl, kh, g),
                        "combinazione Ol={ol:?} Kl={kl:?} Kh={kh:?} G={g:?}"
                    );
                    viste += 1;
                }
            }
        }
    }
    assert_eq!(
        viste, 256,
        "nessuna combinazione deve restare non esercitata"
    );
}

#[test]
fn i_valori_estremi_non_cambiano_la_classificazione() {
    // `u64::MAX` e' un positivo come un altro: la matrice ragiona su livelli,
    // e un contatore enorme non e' piu' probante di un contatore a uno.
    let massimi = evidenza(
        Some(u64::MAX),
        Some(u64::MAX),
        Some(u64::MAX),
        Some(u64::MAX),
    );
    assert_eq!(
        classifica_evidenza(&massimi),
        ClasseEvidenzaMemoria::Attribuita
    );
    // E `Kl > Kh` resta incoerente anche al limite del tipo.
    let estremo_incoerente = evidenza(Some(1), Some(u64::MAX), Some(u64::MAX - 1), Some(1));
    assert_eq!(
        classifica_evidenza(&estremo_incoerente),
        ClasseEvidenzaMemoria::Incoerente
    );
}

#[test]
fn kl_oltre_kh_e_incoerente_in_piu_forme() {
    // Il controllo e' sui VALORI: dopo la riduzione a livelli `2` e `1` sono
    // entrambi «positivo» e la disuguaglianza sparirebbe.
    for (kl, kh) in [(1_u64, 0_u64), (2, 1), (u64::MAX, 0), (5, 4)] {
        assert_eq!(
            classifica_evidenza(&evidenza(Some(1), Some(kl), Some(kh), Some(0))),
            ClasseEvidenzaMemoria::Incoerente,
            "Kl={kl} Kh={kh}"
        );
    }
    // Con uno dei due non osservato la contraddizione non e' constatabile.
    assert_ne!(
        classifica_evidenza(&evidenza(Some(1), Some(5), None, Some(0))),
        ClasseEvidenzaMemoria::Incoerente,
        "non aver letto `Kh` non e' un conflitto fra segnali"
    );
}

#[test]
fn la_diagnostica_non_cambia_la_classificazione() {
    // `Oa`, picco, tetto e `max` sono diagnostici: viaggiano nell'evidenza e
    // non toccano la categoria. Se pesassero, l'attribuzione dipenderebbe da
    // misure che il progetto ha gia' dichiarato non causali.
    let base = evidenza(Some(1), Some(1), Some(1), Some(1));
    let atteso = classifica_evidenza(&base);

    let variata = EvidenzaDiLimite {
        oom_degli_antenati: PressioneDegliAntenati::nuova(&[Some(9)], Some(1), 0)
            .expect("forma canonica"),
        diagnostica: DiagnosticaSupplementare {
            tetto_byte: u64::MAX,
            picco_byte: Some(u64::MAX),
            respinte_al_tetto: Some(u64::MAX),
        },
        ..base
    };
    assert_eq!(
        classifica_evidenza(&variata),
        atteso,
        "la diagnostica non decide la classe"
    );

    // E anche nel verso opposto: azzerata del tutto.
    let spoglia = EvidenzaDiLimite {
        oom_degli_antenati: PressioneDegliAntenati::profondita_non_stabilita(),
        diagnostica: DiagnosticaSupplementare {
            tetto_byte: 0,
            picco_byte: None,
            respinte_al_tetto: None,
        },
        ..base
    };
    assert_eq!(classifica_evidenza(&spoglia), atteso);
}

// ---------------------------------------------------------------------------
// Un caso nominato per ogni riga della matrice §10.0-bis
// ---------------------------------------------------------------------------

#[test]
fn riga_attribuzione_forte() {
    // L'unica riga che attribuisce: la riga INTERA, non il solo `G`.
    assert_eq!(
        classifica_evidenza(&evidenza(Some(1), Some(1), Some(1), Some(1))),
        ClasseEvidenzaMemoria::Attribuita
    );
}

#[test]
fn riga_pressione_senza_group_kill_locale() {
    // Qualcuno e' sopravvissuto, e i due delta possono essere eventi
    // distinti.
    assert_eq!(
        classifica_evidenza(&evidenza(Some(1), Some(1), Some(1), Some(0))),
        ClasseEvidenzaMemoria::NonAttribuita
    );
}

#[test]
fn riga_uccisione_in_un_discendente_col_sigillo_fallito() {
    assert_eq!(
        classifica_evidenza(&evidenza(Some(1), Some(0), Some(1), Some(0))),
        ClasseEvidenzaMemoria::NonAttribuita
    );
}

#[test]
fn riga_group_kill_locale_senza_uccisi_nel_dominio() {
    // `G` positivo con `Kl = 0`: nessun processo DEL DOMINIO e' stato ucciso,
    // quindi non e' la riga dell'attribuzione.
    assert_eq!(
        classifica_evidenza(&evidenza(Some(1), Some(0), Some(1), Some(1))),
        ClasseEvidenzaMemoria::NonAttribuita
    );
}

#[test]
fn riga_dominio_non_uccidibile() {
    // `oom_score_adj` non normalizzato: limite raggiunto e nessun task
    // uccidibile. NON e' `ResourceLimit` — senza group kill locale manca
    // l'operazione che lega il limite di questo dominio alla sua morte.
    assert_eq!(
        classifica_evidenza(&evidenza(Some(1), Some(0), Some(0), Some(0))),
        ClasseEvidenzaMemoria::NonAttribuita
    );
}

#[test]
fn riga_group_kill_che_non_ha_ucciso_nulla() {
    assert_eq!(
        classifica_evidenza(&evidenza(Some(1), Some(0), Some(0), Some(1))),
        ClasseEvidenzaMemoria::Incoerente
    );
}

#[test]
fn riga_oom_che_viene_da_fuori() {
    // Il worker e' morto senza che il SUO tetto sia stato raggiunto.
    assert_eq!(
        classifica_evidenza(&evidenza(Some(0), Some(1), Some(1), Some(0))),
        ClasseEvidenzaMemoria::NonAttribuita
    );
}

#[test]
fn riga_group_kill_locale_col_tetto_mai_invocato() {
    assert_eq!(
        classifica_evidenza(&evidenza(Some(0), Some(1), Some(1), Some(1))),
        ClasseEvidenzaMemoria::Incoerente
    );
}

#[test]
fn riga_uccisione_in_un_discendente_senza_pressione_locale() {
    assert_eq!(
        classifica_evidenza(&evidenza(Some(0), Some(0), Some(1), Some(0))),
        ClasseEvidenzaMemoria::NonAttribuita
    );
    // Con `G` positivo la stessa riga diventa incoerente.
    assert_eq!(
        classifica_evidenza(&evidenza(Some(0), Some(0), Some(1), Some(1))),
        ClasseEvidenzaMemoria::Incoerente
    );
}

#[test]
fn riga_nessuna_evidenza() {
    // Tutti e quattro OSSERVATI e a zero. Senza quella qualifica un `None` si
    // leggerebbe come uno zero, e «non ho letto» diventerebbe «non e'
    // successo».
    assert_eq!(
        classifica_evidenza(&evidenza(Some(0), Some(0), Some(0), Some(0))),
        ClasseEvidenzaMemoria::Assente
    );
    assert_eq!(
        classifica_evidenza(&evidenza(Some(0), Some(0), Some(0), Some(1))),
        ClasseEvidenzaMemoria::Incoerente
    );
}

#[test]
fn riga_indeterminata_non_e_pressione() {
    // La riga che ha imposto la quinta classe: `Ol` non letto e gli altri tre
    // a zero. Non abbiamo osservato pressione — abbiamo un'osservazione
    // incompleta — e dichiarare `NonAttribuita` affermerebbe una pressione
    // mai vista.
    assert_eq!(
        classifica_evidenza(&evidenza(None, Some(0), Some(0), Some(0))),
        ClasseEvidenzaMemoria::Indeterminata
    );
    // Nessuna lettura affatto: stessa cosa.
    assert_eq!(
        classifica_evidenza(&evidenza(None, None, None, None)),
        ClasseEvidenzaMemoria::Indeterminata
    );
}

#[test]
fn riga_lettura_incompleta_con_un_positivo() {
    // Qualcosa si e' visto, e non basta: non attribuita, non indeterminata.
    assert_eq!(
        classifica_evidenza(&evidenza(None, Some(1), Some(1), Some(1))),
        ClasseEvidenzaMemoria::NonAttribuita
    );
}

#[test]
fn un_none_non_autorizza_mai_l_attribuzione() {
    // In particolare su `G`: confondere «non letto» con «e' scattato»
    // attribuirebbe su un contatore mai visto.
    for mancante in 0..4 {
        let mut segnali = [Some(1_u64); 4];
        segnali[mancante] = None;
        assert_ne!(
            classifica_evidenza(&evidenza(segnali[0], segnali[1], segnali[2], segnali[3])),
            ClasseEvidenzaMemoria::Attribuita,
            "con il segnale {mancante} non letto non si attribuisce"
        );
    }
}

#[test]
fn i_livelli_del_segnale_distinguono_il_non_letto_dallo_zero() {
    assert_eq!(Segnale::di(None), Segnale::NonOsservato);
    assert_eq!(Segnale::di(Some(0)), Segnale::Zero);
    assert_eq!(Segnale::di(Some(1)), Segnale::Positivo);
    assert_eq!(Segnale::di(Some(u64::MAX)), Segnale::Positivo);
}

#[test]
fn ogni_classe_ha_la_propria_categoria() {
    assert_eq!(
        ClasseEvidenzaMemoria::Attribuita.categoria(),
        ErrorCategory::ResourceLimit
    );
    assert_eq!(
        ClasseEvidenzaMemoria::NonAttribuita.categoria(),
        ErrorCategory::UnattributedMemoryPressure
    );
    for classe in [
        ClasseEvidenzaMemoria::Assente,
        ClasseEvidenzaMemoria::Indeterminata,
        ClasseEvidenzaMemoria::Incoerente,
    ] {
        assert_eq!(
            classe.categoria(),
            ErrorCategory::Internal,
            "{classe:?} non afferma nulla sul budget del chiamante"
        );
    }
}

// ---------------------------------------------------------------------------
// La precedenza §10.3
// ---------------------------------------------------------------------------

/// Un evento concorrente, per la generazione esaustiva.
#[derive(Debug, Clone, Copy)]
struct Combinazione {
    publish: bool,
    evidenza: Option<ClasseEvidenzaMemoria>,
    timeout: bool,
    cancellazione: bool,
    worker: Option<u8>,
}

/// Un'evidenza che si classifica nella classe voluta.
fn evidenza_di_classe(classe: ClasseEvidenzaMemoria) -> EvidenzaDiLimite {
    let prova = match classe {
        ClasseEvidenzaMemoria::Attribuita => evidenza(Some(1), Some(1), Some(1), Some(1)),
        ClasseEvidenzaMemoria::NonAttribuita => evidenza(Some(1), Some(1), Some(1), Some(0)),
        ClasseEvidenzaMemoria::Assente => evidenza(Some(0), Some(0), Some(0), Some(0)),
        ClasseEvidenzaMemoria::Indeterminata => evidenza(None, None, None, None),
        ClasseEvidenzaMemoria::Incoerente => evidenza(Some(1), Some(2), Some(1), Some(0)),
    };
    assert_eq!(
        classifica_evidenza(&prova),
        classe,
        "la fixture deve davvero avere la classe che dichiara"
    );
    prova
}

fn esito_worker(codice: u8) -> EsitoWorker {
    match codice {
        0 => EsitoWorker::Successo,
        1 => EsitoWorker::Errore(PlenoraError::Io(std::io::Error::other("io"))),
        _ => EsitoWorker::Panic {
            forma: FormaDelPayload::di(&"x"),
        },
    }
}

/// Il livello di precedenza atteso, dalla tabella della §10.3.
fn livello_atteso(caso: Combinazione) -> u8 {
    if caso.publish {
        1
    } else if caso.evidenza == Some(ClasseEvidenzaMemoria::Attribuita) {
        2
    } else if caso.timeout {
        3
    } else if caso.cancellazione {
        4
    } else if caso.evidenza == Some(ClasseEvidenzaMemoria::NonAttribuita) {
        5
    } else if matches!(
        caso.evidenza,
        Some(ClasseEvidenzaMemoria::Incoerente | ClasseEvidenzaMemoria::Indeterminata)
    ) {
        6
    } else if caso.worker.is_some() {
        7
    } else {
        8
    }
}

fn livello_dell_esito(esito: &EsitoClassificato) -> u8 {
    match esito {
        EsitoClassificato::Pubblicato { .. } => 1,
        EsitoClassificato::LimiteAttribuito(_) => 2,
        EsitoClassificato::Timeout { .. } => 3,
        EsitoClassificato::Cancellato { .. } => 4,
        EsitoClassificato::PressioneNonAttribuita(_) => 5,
        EsitoClassificato::EvidenzaNonUtilizzabile { .. } => 6,
        EsitoClassificato::ErroreDelWorker { .. }
        | EsitoClassificato::PanicDelWorker { .. }
        | EsitoClassificato::DaVerificare { .. } => 7,
        EsitoClassificato::TerminazioneAmbigua { .. } => 8,
    }
}

fn tutte_le_combinazioni() -> Vec<Combinazione> {
    let classi = [
        None,
        Some(ClasseEvidenzaMemoria::Attribuita),
        Some(ClasseEvidenzaMemoria::NonAttribuita),
        Some(ClasseEvidenzaMemoria::Assente),
        Some(ClasseEvidenzaMemoria::Indeterminata),
        Some(ClasseEvidenzaMemoria::Incoerente),
    ];
    let mut tutte = Vec::new();
    for publish in [false, true] {
        for evidenza in classi {
            for timeout in [false, true] {
                for cancellazione in [false, true] {
                    for worker in [None, Some(0), Some(1), Some(2)] {
                        tutte.push(Combinazione {
                            publish,
                            evidenza,
                            timeout,
                            cancellazione,
                            worker,
                        });
                    }
                }
            }
        }
    }
    tutte
}

fn classifica_caso(caso: Combinazione) -> EsitoClassificato {
    classifica(FattiDopoLaQuiescenza::dopo_la_quiescenza(
        caso.publish,
        caso.evidenza.map(evidenza_di_classe),
        caso.timeout,
        caso.cancellazione,
        caso.worker.map(esito_worker),
    ))
}

#[test]
fn tutte_le_combinazioni_degli_eventi_rispettano_la_precedenza() {
    // 2 x 6 x 2 x 2 x 4 = 192 combinazioni, GENERATE. Elencarle a mano
    // avrebbe lasciato fuori proprio quelle a cui nessuno pensa.
    let combinazioni = tutte_le_combinazioni();
    assert_eq!(combinazioni.len(), 192);
    for caso in combinazioni {
        let esito = classifica_caso(caso);
        assert_eq!(
            livello_dell_esito(&esito),
            livello_atteso(caso),
            "combinazione {caso:?} -> {esito:?}"
        );
    }
}

#[test]
fn l_evidenza_sopravvive_a_ogni_classificazione() {
    // Anche quando la classe non decide l'esito: assente, indeterminata e
    // incoerente non producono un livello di precedenza, e cio' che il
    // sistema ha detto non va buttato via per questo.
    for caso in tutte_le_combinazioni() {
        if caso.evidenza.is_none() {
            continue;
        }
        let esito = classifica_caso(caso);
        assert!(
            esito.evidenza().is_some(),
            "l'evidenza si perde in {caso:?} -> {esito:?}"
        );
    }
}

#[test]
fn evidenza_incoerente_o_indeterminata_non_diventa_da_verificare() {
    // Il blocker: `Incoerente` e `Indeterminata` ricadevano sull'esito del
    // worker, e con un worker che dichiara successo producevano
    // `DaVerificare` — cioe' «prosegui» mentre una lettura del dominio e'
    // rotta o mancante. La §10.0-bis dice che NESSUNA delle cinque classi
    // autorizza la pubblicazione, e proseguire alla verifica e' il primo
    // passo verso di essa.
    for classe in [
        ClasseEvidenzaMemoria::Incoerente,
        ClasseEvidenzaMemoria::Indeterminata,
    ] {
        for worker in [None, Some(0), Some(1), Some(2)] {
            let esito = classifica(FattiDopoLaQuiescenza::dopo_la_quiescenza(
                false,
                Some(evidenza_di_classe(classe)),
                false,
                false,
                worker.map(esito_worker),
            ));
            assert!(
                !matches!(esito, EsitoClassificato::DaVerificare { .. }),
                "{classe:?} con worker {worker:?} non deve proseguire: {esito:?}"
            );
            assert_eq!(
                esito.categoria(),
                Some(ErrorCategory::Internal),
                "{classe:?} con worker {worker:?}"
            );
            assert!(!esito.pubblica());
            assert!(
                esito.evidenza().is_some(),
                "la prova e' obbligatoria: e' cio' che va guardato"
            );
        }
    }
}

#[test]
fn evidenza_assente_non_sovrascrive_l_esito_del_worker() {
    // La classe `Assente` NON e' un livello di precedenza: e' una lettura
    // riuscita in cui non c'era nulla, quindi non contraddice il worker.
    // Farne un livello direbbe «difetto interno» ogni volta che il dominio e'
    // stato letto e stava bene.
    let esito = classifica(FattiDopoLaQuiescenza::dopo_la_quiescenza(
        false,
        Some(evidenza_di_classe(ClasseEvidenzaMemoria::Assente)),
        false,
        false,
        Some(EsitoWorker::Successo),
    ));
    assert!(
        matches!(esito, EsitoClassificato::DaVerificare { .. }),
        "{esito:?}"
    );
    assert!(
        esito.evidenza().is_some(),
        "l'evidenza si conserva comunque"
    );
}

#[test]
fn la_forma_del_payload_non_puo_portare_il_contenuto() {
    // Il campo e' privato e l'unico costruttore delega a `panic_policy`:
    // scrivere un letterale non compila. Qui si verifica che cio' che ESCE
    // non porti il contenuto, e che venga dall'autorita' e non da una copia.
    let segreto = "contenuto-che-non-deve-uscire".to_owned();
    let forma = FormaDelPayload::di(&segreto);
    let reso = forma.to_string();
    assert!(!reso.contains("contenuto-che-non-deve-uscire"), "{reso}");
    assert_eq!(
        reso,
        plenora_core::panic_policy::forma_payload(&segreto),
        "la forma deve venire dall'autorita', non da una copia locale"
    );
}

#[test]
fn solo_il_publish_pubblica() {
    for caso in tutte_le_combinazioni() {
        let esito = classifica_caso(caso);
        assert_eq!(
            esito.pubblica(),
            caso.publish,
            "un esito terminale ambiguo non pubblica mai (GA-1): {caso:?}"
        );
    }
}

#[test]
fn la_classificazione_e_ripetibile() {
    // NOME PRECEDENTE: «l'ordine di arrivo e' irrilevante». Non lo provava:
    // esegue due volte gli STESSI fatti, quindi verifica la ripetibilita'.
    //
    // Che l'ordine sia irrilevante e' una proprieta' del TIPO, non di questo
    // test: `FattiDopoLaQuiescenza` non porta timestamp ne' sequenza, e non
    // c'e' modo di esprimere due ordini diversi degli stessi fatti da
    // confrontare. Attribuire a un test una prova che la struttura fornisce
    // gia' avrebbe fatto sembrare verificato cio' che e' costruito.
    for caso in tutte_le_combinazioni() {
        let primo = livello_dell_esito(&classifica_caso(caso));
        let secondo = livello_dell_esito(&classifica_caso(caso));
        assert_eq!(
            primo, secondo,
            "classificazione non deterministica: {caso:?}"
        );
    }
}

#[test]
fn la_categoria_di_ogni_esito_e_quella_della_matrice() {
    for caso in tutte_le_combinazioni() {
        let esito = classifica_caso(caso);
        let atteso = match livello_dell_esito(&esito) {
            1 => None,
            2 => Some(ErrorCategory::ResourceLimit),
            3 => Some(ErrorCategory::Timeout),
            4 => Some(ErrorCategory::Cancelled),
            5 => Some(ErrorCategory::UnattributedMemoryPressure),
            // Il 6 e l'8 condividono la categoria e NON il significato:
            // «lettura non utilizzabile» e «morto senza esito». La tabella li
            // tiene distinti; la proiezione su ErrorCategory no, perche'
            // Internal e' l'unica che non afferma nulla.
            6 | 8 => Some(ErrorCategory::Internal),
            7 => match caso.worker {
                Some(0) => None,
                Some(1) => Some(ErrorCategory::Io),
                _ => Some(ErrorCategory::Internal),
            },
            altro => panic!("livello di precedenza inatteso: {altro}"),
        };
        assert_eq!(esito.categoria(), atteso, "{caso:?} -> {esito:?}");
    }
}

// ---------------------------------------------------------------------------
// I casi nominati della matrice §10
// ---------------------------------------------------------------------------

#[test]
fn riga_1_publish_completato_vince_su_tutto() {
    // Anche con evidenza attribuita, timeout e cancellazione insieme: se
    // l'output e' visibile, l'operazione e' riuscita.
    let esito = classifica(FattiDopoLaQuiescenza::dopo_la_quiescenza(
        true,
        Some(evidenza_di_classe(ClasseEvidenzaMemoria::Attribuita)),
        true,
        true,
        None,
    ));
    assert!(esito.pubblica());
    assert_eq!(esito.categoria(), None);
    assert!(
        esito.evidenza().is_some(),
        "l'evidenza si conserva comunque"
    );
}

#[test]
fn riga_2_errore_del_worker_conserva_i_quattro_assi() {
    // L'errore viaggia INTERO: i quattro assi si leggono da li', e con essi
    // messaggio sanitizzato e contesto.
    let errore = PlenoraError::Timeout("attesa del worker".to_owned());
    let categoria = errore.category();
    let esito = classifica(FattiDopoLaQuiescenza::dopo_la_quiescenza(
        false,
        None,
        false,
        false,
        Some(EsitoWorker::Errore(errore)),
    ));
    assert_eq!(esito.categoria(), Some(categoria));
    match esito {
        EsitoClassificato::ErroreDelWorker { errore, .. } => {
            assert!(
                errore.to_string().contains("attesa del worker"),
                "il messaggio non deve perdersi: {errore}"
            );
            assert_eq!(errore.phase(), plenora_core::ErrorPhase::Write);
        }
        altro => panic!("atteso un errore del worker: {altro:?}"),
    }
}

#[test]
fn riga_3_panic_porta_la_sola_forma_del_payload() {
    let segreto = "contenuto-che-non-deve-uscire";
    let forma = FormaDelPayload::di(&segreto);
    let esito = classifica(FattiDopoLaQuiescenza::dopo_la_quiescenza(
        false,
        None,
        false,
        false,
        Some(EsitoWorker::Panic { forma }),
    ));
    assert_eq!(esito.categoria(), Some(ErrorCategory::Internal));
    match esito {
        EsitoClassificato::PanicDelWorker { forma, .. } => {
            assert!(
                !forma.to_string().contains(segreto),
                "la forma non deve portare il contenuto: {forma}"
            );
        }
        altro => panic!("atteso un panic del worker: {altro:?}"),
    }
}

#[test]
fn riga_4_morte_senza_esito_e_terminazione_ambigua() {
    // `esito_worker: None` significa «morto senza esito», non «non ancora
    // finito»: la quiescenza e' gia' avvenuta.
    let esito = classifica(FattiDopoLaQuiescenza::dopo_la_quiescenza(
        false, None, false, false, None,
    ));
    assert_eq!(esito.categoria(), Some(ErrorCategory::Internal));
    assert!(matches!(
        esito,
        EsitoClassificato::TerminazioneAmbigua { .. }
    ));
}

#[test]
fn riga_5_l_evidenza_del_dominio_batte_l_esito_dichiarato() {
    // Il worker riporta un errore suo e il dominio ha la prova: vince la
    // prova. L'errore che un processo riesce a riportare in quelle condizioni
    // e' quasi sempre la conseguenza, non la causa.
    let esito = classifica(FattiDopoLaQuiescenza::dopo_la_quiescenza(
        false,
        Some(evidenza_di_classe(ClasseEvidenzaMemoria::Attribuita)),
        false,
        false,
        Some(EsitoWorker::Errore(PlenoraError::Io(
            std::io::Error::other("allocazione fallita"),
        ))),
    ));
    assert_eq!(esito.categoria(), Some(ErrorCategory::ResourceLimit));
    assert!(
        matches!(esito, EsitoClassificato::LimiteAttribuito(_)),
        "la prova e' obbligatoria per tipo"
    );
}

#[test]
fn riga_5_bis_dominio_non_uccidibile_non_e_resource_limit() {
    let esito = classifica(fatti_con_evidenza(evidenza(
        Some(3),
        Some(0),
        Some(0),
        Some(0),
    )));
    assert_eq!(
        esito.categoria(),
        Some(ErrorCategory::UnattributedMemoryPressure),
        "senza group kill locale manca l'operazione che lega il limite alla morte"
    );
}

#[test]
fn riga_6a_terminazione_senza_evidenza_e_interna() {
    let esito = classifica(fatti_con_evidenza(evidenza(
        Some(0),
        Some(0),
        Some(0),
        Some(0),
    )));
    assert_eq!(esito.categoria(), Some(ErrorCategory::Internal));
    assert!(esito.evidenza().is_some(), "l'evidenza letta si conserva");
}

#[test]
fn riga_6b_pressione_non_attribuibile_non_e_interna() {
    let esito = classifica(fatti_con_evidenza(evidenza(
        Some(1),
        Some(1),
        Some(1),
        Some(0),
    )));
    assert_eq!(
        esito.categoria(),
        Some(ErrorCategory::UnattributedMemoryPressure),
        "`Internal` direbbe «difetto nostro» dove il difetto puo' essere dell'ambiente"
    );
}

#[test]
fn riga_7_il_timeout_batte_la_cancellazione_e_l_esito() {
    let esito = classifica(FattiDopoLaQuiescenza::dopo_la_quiescenza(
        false,
        None,
        true,
        true,
        Some(EsitoWorker::Successo),
    ));
    assert_eq!(esito.categoria(), Some(ErrorCategory::Timeout));
}

#[test]
fn riga_8_la_cancellazione_batte_il_successo_dichiarato() {
    // Un successo che non e' diventato visibile non e' un successo per chi ha
    // chiesto di annullare.
    let esito = classifica(FattiDopoLaQuiescenza::dopo_la_quiescenza(
        false,
        None,
        false,
        true,
        Some(EsitoWorker::Successo),
    ));
    assert_eq!(esito.categoria(), Some(ErrorCategory::Cancelled));
}

#[test]
fn il_successo_del_worker_non_e_il_successo_finale() {
    // Significa «prosegui alla verifica»: non pubblica e non ha categoria.
    let esito = classifica(FattiDopoLaQuiescenza::dopo_la_quiescenza(
        false,
        None,
        false,
        false,
        Some(EsitoWorker::Successo),
    ));
    assert!(matches!(esito, EsitoClassificato::DaVerificare { .. }));
    assert!(!esito.pubblica(), "la verifica non e' ancora avvenuta");
    assert_eq!(
        esito.categoria(),
        None,
        "non e' un fallimento: e' un esito non ancora finale"
    );
}

#[test]
fn la_pressione_non_attribuita_sta_sotto_timeout_e_cancellazione() {
    let prova = || Some(evidenza_di_classe(ClasseEvidenzaMemoria::NonAttribuita));
    let col_timeout = classifica(FattiDopoLaQuiescenza::dopo_la_quiescenza(
        false,
        prova(),
        true,
        false,
        None,
    ));
    assert_eq!(col_timeout.categoria(), Some(ErrorCategory::Timeout));
    let con_cancellazione = classifica(FattiDopoLaQuiescenza::dopo_la_quiescenza(
        false,
        prova(),
        false,
        true,
        None,
    ));
    assert_eq!(
        con_cancellazione.categoria(),
        Some(ErrorCategory::Cancelled)
    );
}

#[test]
fn la_pressione_non_attribuita_sta_sopra_l_esito_del_worker() {
    let esito = classifica(FattiDopoLaQuiescenza::dopo_la_quiescenza(
        false,
        Some(evidenza_di_classe(ClasseEvidenzaMemoria::NonAttribuita)),
        false,
        false,
        Some(EsitoWorker::Errore(PlenoraError::Io(
            std::io::Error::other("allocazione fallita"),
        ))),
    ));
    assert_eq!(
        esito.categoria(),
        Some(ErrorCategory::UnattributedMemoryPressure),
        "l'errore riportato sotto pressione e' quasi sempre la conseguenza"
    );
}
