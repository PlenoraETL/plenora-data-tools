//! Fa percorrere a un worker **reale** la sequenza intera, e giudica il referto.
//!
//! # Perche' un binario e non un caso
//!
//! Perche' l'harness fa esattamente cio' che fa il supervisore: toglie
//! `CLOEXEC` a due descrittori e subito dopo avvia un processo. In quella
//! finestra i due estremi sono ereditabili da **qualunque** altro `spawn` del
//! processo, e per questo `accerta_monothread` pretende un thread solo.
//!
//! Un binario di test non lo puo' garantire: libtest esegue ogni caso in un
//! thread proprio, e il conteggio dei task e' due prima ancora che il caso
//! cominci. Rinunciare al controllo per far girare il caso avrebbe fatto
//! divergere l'harness dal codice che dice di provare — e proprio sulla riga
//! piu' delicata.
//!
//! # La grammatica, e perche' e' esatta
//!
//! Due sole righe di comando sono ammesse:
//!
//! ```text
//!   --immagine <percorso> --etichetta iterazione
//!   --immagine <percorso> --etichetta produzione --digest <64 esadecimali>
//! ```
//!
//! Ogni opzione **una volta sola**, nessuna opzione sconosciuta, nessun
//! argomento sciolto, nessun valore per difetto. Una grammatica permissiva
//! qualifica cio' che chi lancia non ha chiesto: un `--digest` ripetuto
//! sceglierebbe in silenzio una delle due copie, un'opzione con un refuso
//! verrebbe ignorata, e `produzione` senza digest direbbe di aver qualificato un
//! binario senza poterlo nominare.
//!
//! # I due ingressi, e perche' non comunicano
//!
//! `iterazione`: l'immagine sta nel target condiviso dell'harness e nessun
//! digest la fissa. Serve a lavorare, e **non qualifica** — per questo un digest
//! non glielo si puo' nemmeno passare.
//!
//! `produzione`: l'immagine e' compilata a parte, in un target suo, e il digest
//! dice quale ci si aspetta di aver eseguito. Qualifica.
//!
//! # Che cosa prova, e che cosa no
//!
//! Prova immagine reale, eredita' dei descrittori, handshake, incarico,
//! progresso, esito, **artefatto riverificato**, EOF e raccolta del processo.
//!
//! **Non prova «sotto limite».** Qui non c'e' ne' lo spawner ne' un dominio
//! `cgroup2`: il worker nasce da questo processo e vive con la memoria che il
//! sistema gli concede. Quel verde arriva soltanto sulla VM, attraversando
//! spawner e dominio vero con `memory.max`.

/// Che cosa la riga di comando chiede.
#[cfg(all(target_os = "linux", feature = "internals"))]
struct Richiesta {
    percorso: String,
    etichetta: &'static str,
    digest: Option<String>,
}

/// Legge la riga di comando, o dice esattamente che cosa non va.
///
/// # Errors
///
/// Quando la riga non e' una delle due ammesse: opzione sconosciuta, ripetuta,
/// senza valore, mancante, oppure una coppia etichetta/digest che non sta in
/// piedi.
#[cfg(all(target_os = "linux", feature = "internals"))]
fn richiesta_da(argomenti: &[String]) -> Result<Richiesta, String> {
    let mut percorso: Option<String> = None;
    let mut etichetta: Option<String> = None;
    let mut digest: Option<String> = None;

    let mut resta = argomenti.iter();
    while let Some(opzione) = resta.next() {
        let posto = match opzione.as_str() {
            "--immagine" => &mut percorso,
            "--etichetta" => &mut etichetta,
            "--digest" => &mut digest,
            altro => return Err(format!("opzione sconosciuta: «{altro}»")),
        };
        if posto.is_some() {
            return Err(format!(
                "«{opzione}» compare piu' di una volta: quale valga non lo dichiara nessuno"
            ));
        }
        let Some(valore) = resta.next() else {
            return Err(format!("«{opzione}» non ha un valore"));
        };
        *posto = Some(valore.clone());
    }

    let Some(percorso) = percorso else {
        return Err(
            "manca «--immagine <percorso>»: senza, non c'e' niente da percorrere".to_owned(),
        );
    };
    let Some(etichetta) = etichetta else {
        return Err(
            "manca «--etichetta»: «iterazione» per lavorare, «produzione» per qualificare. Non \
             c'e' un valore per difetto, perche' sceglierlo per conto di chi lancia direbbe quale \
             binario si sta qualificando."
                .to_owned(),
        );
    };

    match (etichetta.as_str(), digest) {
        ("iterazione", None) => Ok(Richiesta {
            percorso,
            etichetta: "iterazione",
            digest: None,
        }),
        ("iterazione", Some(_)) => Err(
            "«--digest» con «--etichetta iterazione»: quell'immagine non e' fissata da nessun \
             digest, e accettarne uno direbbe che lo e'"
                .to_owned(),
        ),
        ("produzione", Some(digest)) if canonico(&digest) => Ok(Richiesta {
            percorso,
            etichetta: "produzione",
            digest: Some(digest),
        }),
        ("produzione", Some(storto)) => Err(format!(
            "«--digest {storto}» non e' uno SHA-256 in forma canonica: 64 esadecimali minuscoli"
        )),
        ("produzione", None) => Err(
            "«--etichetta produzione» senza «--digest»: qualificare un'immagine senza poterla \
             nominare direbbe di aver provato qualcosa su un file qualunque"
                .to_owned(),
        ),
        (altro, _) => Err(format!(
            "«--etichetta {altro}» non e' una delle due: «iterazione» oppure «produzione»"
        )),
    }
}

/// La forma che un digest deve avere per essere confrontabile.
///
/// Si controlla qui e non dopo: un digest in maiuscolo non combacerebbe mai con
/// quello calcolato, e il rosso parlerebbe di un binario sostituito invece che
/// di una riga di comando storta.
#[cfg(all(target_os = "linux", feature = "internals"))]
fn canonico(digest: &str) -> bool {
    digest.len() == 64
        && digest
            .bytes()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
}

/// Il giudizio, che **non e' scritto qui**.
///
/// # Perche' sta nella libreria
///
/// Perche' le strade che portano un worker fino all'esito sono due — due pipe
/// nude, oppure spawner e dominio — e cio' che si pretende non cambia. Un
/// oracolo per strada sarebbe due oracoli, e a divergere sarebbe proprio quello
/// che qualifica: la strada piu' severa direbbe rosso su cio' che l'altra
/// accetta, e nessuno saprebbe quale delle due ha ragione.
///
/// Qui resta solo il modo di **riportarlo**: le pretese cadute si stampano una
/// per riga, perche' chi legge un rosso non debba rileggere il referto per
/// indovinare quali fossero.
#[cfg(all(target_os = "linux", feature = "internals"))]
fn riporta(manca: &[String]) -> bool {
    for difetto in manca {
        eprintln!("PERSO: {difetto}");
    }
    manca.is_empty()
}

#[cfg(all(target_os = "linux", feature = "internals"))]
fn main() -> std::process::ExitCode {
    use plenora_engine::prova::{percorri, Immagine};

    let argomenti: Vec<String> = std::env::args().skip(1).collect();
    let richiesta = match richiesta_da(&argomenti) {
        Ok(richiesta) => richiesta,
        Err(motivo) => {
            eprintln!("{motivo}");
            return std::process::ExitCode::from(2);
        }
    };
    let immagine = if richiesta.digest.is_some() {
        Immagine::DiProduzione(richiesta.percorso.clone().into())
    } else {
        Immagine::DiIterazione(richiesta.percorso.clone().into())
    };

    match percorri(&immagine) {
        Err(causa) => {
            eprintln!("PERSO: la sequenza non si e' percorsa: {causa}");
            std::process::ExitCode::FAILURE
        }
        Ok(referto) => {
            // Il referto si stampa **sempre**, anche quando il giudizio e'
            // negativo: chi legge un rosso deve poter vedere fin dove si e'
            // arrivati senza rieseguire niente.
            println!("immagine={}", referto.immagine);
            println!("digest={}", referto.digest_immagine);
            println!("accordo={}", referto.accordo);
            println!("progressi={}", referto.progressi);
            println!("esito={:?}", referto.esito);
            println!("artefatto={:?}", referto.artefatto);
            println!("fine_del_canale={}", referto.fine_del_canale);
            println!("uscita={:?}", referto.uscita);
            println!("difetti_di_pulizia={:?}", referto.difetti_di_pulizia);
            if riporta(&plenora_engine::prova::giudica(
                &referto,
                richiesta.etichetta,
                richiesta.digest.as_deref(),
            )) {
                println!("VINTO: il worker reale ha percorso la sequenza");
                std::process::ExitCode::SUCCESS
            } else {
                std::process::ExitCode::FAILURE
            }
        }
    }
}

/// Fuori da Linux il percorso non esiste, e dirlo e' meglio che non compilare.
#[cfg(not(all(target_os = "linux", feature = "internals")))]
fn main() -> std::process::ExitCode {
    eprintln!(
        "questo percorso esiste solo su Linux e con la feature «internals»: l'esecuzione isolata \
         non e' supportata altrove"
    );
    std::process::ExitCode::from(2)
}

// I due attributi si sommano, ed e' la stessa condizione di `all(...)`: sono
// separati perche' il gate `scripts/verifica_assenza_assert.py` riconosce
// soltanto la forma letterale `#[cfg(test)]`, di proposito — una forma
// riconosciuta di troppo lascerebbe passare codice di produzione con un
// `assert!`, che e' il verso sbagliato in cui sbagliare. Scritta cosi', la
// condizione resta intera e il gate legge cio' che deve leggere.
#[cfg(test)]
#[cfg(all(target_os = "linux", feature = "internals"))]
mod tests {
    use super::richiesta_da;

    /// Gli argomenti, come `main` li riceve.
    fn riga(pezzi: &[&str]) -> Vec<String> {
        pezzi.iter().map(|uno| (*uno).to_owned()).collect()
    }

    const DIGEST: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";

    /// Le due righe ammesse passano, e nessun'altra.
    #[test]
    fn le_due_righe_ammesse_passano() {
        let iterazione = richiesta_da(&riga(&["--immagine", "/x", "--etichetta", "iterazione"]))
            .expect("l'iterazione non vuole un digest");
        assert_eq!(iterazione.etichetta, "iterazione");
        assert!(iterazione.digest.is_none());

        let produzione = richiesta_da(&riga(&[
            "--immagine",
            "/x",
            "--etichetta",
            "produzione",
            "--digest",
            DIGEST,
        ]))
        .expect("la produzione vuole un digest, e ce l'ha");
        assert_eq!(produzione.etichetta, "produzione");
        assert_eq!(produzione.digest.as_deref(), Some(DIGEST));
    }

    /// **Ogni forma storta e' un rifiuto, e ognuna dice quale.**
    ///
    /// # Che cosa esclude
    ///
    /// Che una riga di comando che nessuno ha chiesto qualifichi comunque
    /// qualcosa. I casi sono elencati uno per uno perche' falliscono in modi
    /// diversi: un duplicato sceglierebbe in silenzio una delle due copie,
    /// un'opzione con un refuso verrebbe ignorata, e `produzione` senza digest
    /// direbbe di aver qualificato un binario senza poterlo nominare.
    #[test]
    fn ogni_riga_storta_ha_il_suo_rifiuto() {
        let storte: [(&[&str], &str); 8] = [
            (&["--immagine", "/x"], "etichetta"),
            (
                &["--etichetta", "produzione", "--digest", DIGEST],
                "immagine",
            ),
            (&["--immagine", "/x", "--etichetta", "produzione"], "digest"),
            (
                &[
                    "--immagine",
                    "/x",
                    "--etichetta",
                    "iterazione",
                    "--digest",
                    DIGEST,
                ],
                "digest",
            ),
            (
                &[
                    "--immagine",
                    "/x",
                    "--immagine",
                    "/y",
                    "--etichetta",
                    "iterazione",
                ],
                "piu' di una volta",
            ),
            (
                &["--immagine", "/x", "--etichetta", "altro"],
                "una delle due",
            ),
            (&["--immagine"], "non ha un valore"),
            (
                &["--immagine", "/x", "--etichetta", "iterazione", "sciolto"],
                "sconosciuta",
            ),
        ];

        for (pezzi, atteso) in storte {
            let Err(motivo) = richiesta_da(&riga(pezzi)) else {
                panic!("«{pezzi:?}» non e' una riga ammessa, e deve essere rifiutata");
            };
            assert!(
                motivo.contains(atteso),
                "«{pezzi:?}» deve essere rifiutata nominando «{atteso}»: {motivo}"
            );
        }
    }

    /// Un digest che non e' in forma canonica si rifiuta **prima** del percorso.
    ///
    /// Confrontarlo dopo lo farebbe fallire comunque, ma il rosso parlerebbe di
    /// un binario sostituito invece che di una riga di comando storta.
    #[test]
    fn un_digest_non_canonico_si_rifiuta_subito() {
        for storto in [
            "abc",
            "00112233445566778899AABBCCDDEEFF00112233445566778899aabbccddeeff",
            "zz112233445566778899aabbccddeeff00112233445566778899aabbccddeeff",
        ] {
            let esito = richiesta_da(&riga(&[
                "--immagine",
                "/x",
                "--etichetta",
                "produzione",
                "--digest",
                storto,
            ]));
            let Err(motivo) = esito else {
                panic!("«{storto}» non e' uno SHA-256 canonico");
            };
            assert!(motivo.contains("forma canonica"), "{motivo}");
        }
    }
}
