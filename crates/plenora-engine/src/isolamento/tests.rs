//! I casi deterministici del preflight: ordine, riletture, parsing, possesso.
//!
//! Girano **ovunque** e senza privilegi, perche' la superficie e' controllata.
//! Non provano che le scritture arrivino al kernel — quello lo prova solo una
//! gerarchia vera — ma provano che la procedura sia giusta, e sono due
//! affermazioni diverse che non si sostituiscono.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use plenora_core::error::{ErrorCategory, ErrorPhase};

use super::{
    bersagli_del_possesso, prepara_dominio, riconosci_modalita, rivalida, spawner_ammissibile,
    Controllo, DifettoSuperficie, IdentitaWorker, Montaggio, ProprietaFile, RichiestaSpawner,
    Riconoscimento, SuperficieDominio, PREFISSO_RISERVATO, PREFISSO_WORKER, VERSIONE_RICHIESTA,
    VERSIONE_WORKER,
};

/// Il riconoscimento **dello spawner**, per i casi che parlano di lui.
///
/// La regola e' una sola e prende il namespace come argomento; questi casi
/// riguardano lo spawner, e ripetere le due costanti a ogni riga renderebbe
/// illeggibile cio' che stanno provando.
fn riconosci_dello_spawner(primo: Option<&std::ffi::OsString>) -> super::Riconoscimento {
    riconosci_modalita(primo, PREFISSO_RISERVATO, VERSIONE_RICHIESTA)
}

/// L'identita' con cui il worker gira in questi casi.
const WORKER: IdentitaWorker = IdentitaWorker {
    uid: 61000,
    gid: 61000,
};

/// Permessi che il worker non puo' scrivere: proprietario diverso, nessun bit
/// per gruppo o altri.
const INTOCCABILE: ProprietaFile = ProprietaFile {
    uid: 0,
    gid: 0,
    mode: 0o644,
};

/// Il tetto dei casi del confine, in byte.
const TETTO: u64 = 1_048_576;

/// I due numeri del canale, per i casi che non hanno pipe vere.
///
/// Valgono `3` e `4` perche' sono i primi ammissibili: i casi qui provano la
/// procedura, non l'ambiente, e due numeri qualunque servirebbero a nascondere
/// che la procedura li tratta come opachi.
const CANALE_DEI_CASI: super::NumeriDelCanale = super::NumeriDelCanale {
    legge: 3,
    scrive: 4,
};

const RADICE: &str = "/sys/fs/cgroup/plenora";
const DOMINIO: &str = "/sys/fs/cgroup/plenora/dominio-7";

type Esito<T> = std::result::Result<T, DifettoSuperficie>;

/// Una superficie in memoria che **registra ogni operazione nell'ordine in cui
/// arriva**.
///
/// Il registro comprende letture e scritture: senza le letture non
/// distinguerebbe «scrivi e rileggi, uno per volta» da «scrivi tutto, poi
/// rileggi tutto», che e' esattamente la differenza che il preflight dichiara
/// di rispettare.
struct Superficie {
    risposte: RefCell<BTreeMap<&'static str, String>>,
    scrittura_rotta: Option<&'static str>,
    lettura_rotta: Option<&'static str>,
    eventi: Option<String>,
    montaggio: Option<Montaggio>,
    /// Proprietario e permessi per percorso; assente significa
    /// [`INTOCCABILE`].
    proprieta: BTreeMap<PathBuf, ProprietaFile>,
    namespace: Vec<(String, String)>,
    registro: RefCell<Vec<String>>,
}

impl Superficie {
    /// Una superficie che si comporta bene: rilegge cio' che le viene scritto,
    /// dominio vuoto, gerarchia che il worker non possiede.
    fn onesta() -> Self {
        Self {
            risposte: RefCell::new(BTreeMap::new()),
            scrittura_rotta: None,
            lettura_rotta: None,
            eventi: Some("populated 0\nfrozen 0\n".to_owned()),
            montaggio: Some(Montaggio {
                punto: PathBuf::from("/sys/fs/cgroup"),
                radice: PathBuf::from("/"),
                opzioni_mount: "rw,nosuid,nodev,noexec,relatime".to_owned(),
                opzioni_superblocco: "rw,nsdelegate".to_owned(),
                dispositivo: "0:27".to_owned(),
            }),
            proprieta: BTreeMap::new(),
            namespace: vec![("user".to_owned(), "user:[4026531837]".to_owned())],
            registro: RefCell::new(Vec::new()),
        }
    }

    fn che_diverge_su(controllo: Controllo, valore: &str) -> Self {
        let superficie = Self::onesta();
        superficie
            .risposte
            .borrow_mut()
            .insert(controllo.file(), valore.to_owned());
        superficie
    }

    fn annota(&self, cosa: String) {
        self.registro.borrow_mut().push(cosa);
    }
}

impl SuperficieDominio for Superficie {
    fn dominio(&self) -> Esito<PathBuf> {
        self.annota("risolvi dominio".to_owned());
        Ok(PathBuf::from(DOMINIO))
    }

    fn radice_control_plane(&self) -> Esito<PathBuf> {
        self.annota("risolvi control plane".to_owned());
        Ok(PathBuf::from(RADICE))
    }

    fn montaggio(&self, _dominio: &Path) -> Esito<Montaggio> {
        self.annota("montaggio".to_owned());
        self.montaggio
            .clone()
            .ok_or_else(|| DifettoSuperficie::Forma("nessun cgroup2".to_owned()))
    }

    fn proprieta(&self, percorso: &Path) -> Esito<ProprietaFile> {
        self.annota(format!("proprieta {}", percorso.display()));
        Ok(self.proprieta.get(percorso).copied().unwrap_or(INTOCCABILE))
    }

    fn namespace(&self) -> Esito<Vec<(String, String)>> {
        self.annota("namespace".to_owned());
        Ok(self.namespace.clone())
    }

    fn scrivi(&mut self, controllo: Controllo, valore: &str) -> Esito<()> {
        self.annota(format!("scrivi {} = {valore}", controllo.file()));
        if self.scrittura_rotta == Some(controllo.file()) {
            return Err(DifettoSuperficie::scrittura(
                "controllo",
                std::io::Error::from(std::io::ErrorKind::PermissionDenied),
            ));
        }
        self.risposte
            .borrow_mut()
            .entry(controllo.file())
            .or_insert_with(|| valore.to_owned());
        Ok(())
    }

    fn rileggi(&self, controllo: Controllo) -> Esito<String> {
        self.annota(format!("rileggi {}", controllo.file()));
        if self.lettura_rotta == Some(controllo.file()) {
            return Err(DifettoSuperficie::lettura(
                "controllo",
                std::io::Error::from(std::io::ErrorKind::PermissionDenied),
            ));
        }
        self.risposte
            .borrow()
            .get(controllo.file())
            .cloned()
            .ok_or_else(|| {
                DifettoSuperficie::lettura(
                    "controllo",
                    std::io::Error::from(std::io::ErrorKind::NotFound),
                )
            })
    }

    fn eventi(&self) -> Esito<String> {
        self.annota("cgroup.events".to_owned());
        self.eventi.clone().ok_or_else(|| {
            DifettoSuperficie::lettura(
                "cgroup.events",
                std::io::Error::from(std::io::ErrorKind::NotFound),
            )
        })
    }
}

/// La sequenza **completa** e' quella dichiarata, operazione per operazione.
///
/// Il registro comprende risoluzione del percorso, montaggio, possesso,
/// namespace, scritture, riletture e quiescenza: e' l'unica forma in cui il
/// caso distingue «scrivi e rileggi, uno per volta» da «scrivi tutto, poi
/// rileggi tutto», e in cui si vede che **niente si scrive prima** di sapere
/// dove si sta scrivendo e chi potrebbe disfarlo.
///
/// Si pretende l'uguaglianza **esatta**, non l'inclusione: e' il solo modo in
/// cui un caso sull'ordine fallisce quando l'ordine cambia.
#[test]
fn la_sequenza_completa_e_quella_dichiarata() {
    let mut superficie = Superficie::onesta();
    prepara_dominio(&mut superficie, 1_048_576, WORKER).expect("preflight");

    let mut atteso = vec![
        "risolvi dominio".to_owned(),
        "risolvi control plane".to_owned(),
        "montaggio".to_owned(),
    ];
    for bersaglio in bersagli_del_possesso(Path::new(DOMINIO), Path::new(RADICE)) {
        atteso.push(format!("proprieta {}", bersaglio.display()));
    }
    atteso.push("namespace".to_owned());
    atteso.extend([
        "scrivi memory.max = 1048576".to_owned(),
        "rileggi memory.max".to_owned(),
        "scrivi memory.swap.max = 0".to_owned(),
        "rileggi memory.swap.max".to_owned(),
        "scrivi memory.oom.group = 1".to_owned(),
        "rileggi memory.oom.group".to_owned(),
        "scrivi cgroup.max.depth = 0".to_owned(),
        "rileggi cgroup.max.depth".to_owned(),
        "cgroup.events".to_owned(),
    ]);

    assert_eq!(
        superficie.registro.into_inner(),
        atteso,
        "prima dove, poi chi, poi che cosa: nessuna scrittura precede la \
         verifica del percorso e del possesso"
    );
}

/// I bersagli del possesso comprendono **ogni antenato** fino alla radice, con
/// la sua directory e il suo `cgroup.procs`.
///
/// # Perche' il `cgroup.procs` del dominio non basta
///
/// Perche' non e' quello con cui si evade: scrivere il proprio pid nel
/// `cgroup.procs` del dominio corrente non porta da nessuna parte, ci si e'
/// gia'. Per uscire si scrive quello di un altro cgroup — il padre — e quella
/// scrittura non tocca nessun file del dominio.
#[test]
fn i_bersagli_comprendono_gli_antenati_fino_alla_radice() {
    let bersagli = bersagli_del_possesso(
        Path::new("/sys/fs/cgroup/plenora/a/b"),
        Path::new("/sys/fs/cgroup/plenora"),
    );

    for atteso in [
        "/sys/fs/cgroup/plenora/a/b",
        "/sys/fs/cgroup/plenora/a/b/cgroup.procs",
        "/sys/fs/cgroup/plenora/a",
        "/sys/fs/cgroup/plenora/a/cgroup.procs",
        "/sys/fs/cgroup/plenora",
        "/sys/fs/cgroup/plenora/cgroup.procs",
    ] {
        assert!(
            bersagli.contains(&PathBuf::from(atteso)),
            "manca {atteso} fra {bersagli:?}"
        );
    }
    assert!(
        !bersagli.contains(&PathBuf::from("/sys/fs/cgroup")),
        "sopra la radice del control plane c'e' l'amministrazione della \
         macchina, che non e' cosa nostra da giudicare"
    );
}

/// Ogni rilettura divergente ferma il preflight, e ciascuna col **proprio**
/// nome nel messaggio.
#[test]
fn una_rilettura_divergente_ferma_il_preflight_qualunque_sia() {
    for (controllo, bugia) in [
        (Controllo::Tetto, "999"),
        (Controllo::Swap, "max"),
        (Controllo::GroupKill, "0"),
        (Controllo::Sigillo, "10"),
    ] {
        let mut superficie = Superficie::che_diverge_su(controllo, bugia);
        let errore = prepara_dominio(&mut superficie, 1_048_576, WORKER)
            .expect_err("una rilettura divergente non puo' passare");

        assert_eq!(
            errore.category(),
            ErrorCategory::IsolationUnavailable,
            "non e' un piano invalido: lo stesso piano su un'altra macchina gira"
        );
        assert_eq!(errore.phase(), ErrorPhase::Prepare);
        let testo = errore.to_string();
        assert!(
            testo.contains(controllo.file()),
            "il messaggio deve dire QUALE controllo ha ceduto: {testo}"
        );
        assert!(testo.contains(bugia), "e che cosa ha riletto: {testo}");
    }
}

/// Scrittura e lettura impossibili sono difetti diversi dalla divergenza — li'
/// il file risponde, qui non risponde — e vanno distinti nel messaggio.
#[test]
fn una_scrittura_o_una_lettura_impossibile_ferma_il_preflight() {
    for rotta in [Controllo::Tetto, Controllo::Sigillo] {
        let mut superficie = Superficie::onesta();
        superficie.scrittura_rotta = Some(rotta.file());
        assert!(prepara_dominio(&mut superficie, 4096, WORKER)
            .expect_err("scrittura rotta")
            .to_string()
            .contains("scrittura fallita"));

        let mut superficie = Superficie::onesta();
        superficie.lettura_rotta = Some(rotta.file());
        assert!(prepara_dominio(&mut superficie, 4096, WORKER)
            .expect_err("lettura rotta")
            .to_string()
            .contains("lettura fallita"));
    }
}

/// Un dominio gia' popolato non e' il nostro, e un `cgroup.events` ambiguo non
/// e' un segnale.
#[test]
fn la_quiescenza_e_un_prerequisito_e_ogni_forma_ambigua_e_un_rifiuto() {
    let casi: [(Option<&str>, &str); 5] = [
        (Some("populated 1\nfrozen 0\n"), "populated e' 1"),
        (Some("frozen 0\n"), "manca il campo populated"),
        (
            Some("populated 0\npopulated 1\n"),
            "compare piu' di una volta",
        ),
        (Some("populated si\n"), "non e' 0 ne' 1"),
        (None, "cgroup.events"),
    ];
    for (eventi, atteso) in casi {
        let mut superficie = Superficie::onesta();
        superficie.eventi = eventi.map(ToOwned::to_owned);
        let errore = prepara_dominio(&mut superficie, 4096, WORKER)
            .expect_err("una quiescenza non accertata non puo' passare");
        assert!(
            errore.to_string().contains(atteso),
            "atteso «{atteso}», ottenuto: {errore}"
        );
    }
}

/// Il possesso si verifica **preventivamente**, su ogni bersaglio, e per
/// ciascuna delle vie di scrittura.
///
/// # Il bit di gruppo si rifiuta sempre, GID a parte
///
/// Su un filesystem con ACL i bit di gruppo sono la **mask della classe**, non
/// «il gruppo proprietario»: un file con `g+w` e un GID estraneo puo' portare
/// una ACL nominativa per l'UID del worker, e quella vale. Rifiutare la mask
/// costa qualche rifiuto in piu' su gerarchie che non useremmo comunque.
#[test]
fn il_possesso_della_gerarchia_e_un_prerequisito() {
    let vie = [
        (
            "proprietario",
            ProprietaFile {
                uid: WORKER.uid,
                gid: 0,
                mode: 0o644,
            },
        ),
        (
            "gruppo, con un GID che col worker non c'entra",
            ProprietaFile {
                uid: 0,
                gid: 999,
                mode: 0o664,
            },
        ),
        (
            "tutti",
            ProprietaFile {
                uid: 0,
                gid: 0,
                mode: 0o646,
            },
        ),
    ];

    for (via, proprieta) in vie {
        for bersaglio in bersagli_del_possesso(Path::new(DOMINIO), Path::new(RADICE)) {
            let mut superficie = Superficie::onesta();
            superficie.proprieta.insert(bersaglio.clone(), proprieta);
            let errore = prepara_dominio(&mut superficie, 4096, WORKER)
                .expect_err("un bersaglio scrivibile dal worker non puo' passare");
            assert_eq!(errore.category(), ErrorCategory::IsolationUnavailable);
            assert!(
                errore
                    .to_string()
                    .contains(&bersaglio.display().to_string()),
                "il messaggio deve dire quale bersaglio, via «{via}»: {errore}"
            );
        }
    }
}

/// Le meta' che passano: il proprietario diverso col suo bit, e il worker
/// proprietario **senza** bit di scrittura.
///
/// Senza queste, il caso sopra reggerebbe anche a giudizio troppo severo — uno
/// che rifiutasse qualunque bit di scrittura renderebbe la gerarchia
/// inservibile anche a chi la prepara.
#[test]
fn il_possesso_non_e_troppo_severo() {
    let ammessi = [
        ProprietaFile {
            uid: 0,
            gid: 0,
            mode: 0o644,
        },
        ProprietaFile {
            uid: WORKER.uid,
            gid: 0,
            mode: 0o444,
        },
    ];
    for proprieta in ammessi {
        let mut superficie = Superficie::onesta();
        for bersaglio in bersagli_del_possesso(Path::new(DOMINIO), Path::new(RADICE)) {
            superficie.proprieta.insert(bersaglio, proprieta);
        }
        assert!(
            prepara_dominio(&mut superficie, 4096, WORKER).is_ok(),
            "{proprieta:?} non da' autorita' al worker e non va rifiutato"
        );
    }
}

/// Un dominio che non sta sotto la radice del control plane si rifiuta prima
/// di scrivere qualunque cosa.
#[test]
fn un_dominio_fuori_dal_control_plane_si_rifiuta_prima_di_scrivere() {
    struct Altrove(Superficie);
    impl SuperficieDominio for Altrove {
        fn dominio(&self) -> Esito<PathBuf> {
            Ok(PathBuf::from("/sys/fs/cgroup/altro/dominio"))
        }
        fn radice_control_plane(&self) -> Esito<PathBuf> {
            self.0.radice_control_plane()
        }
        fn montaggio(&self, d: &Path) -> Esito<Montaggio> {
            self.0.montaggio(d)
        }
        fn proprieta(&self, p: &Path) -> Esito<ProprietaFile> {
            self.0.proprieta(p)
        }
        fn namespace(&self) -> Esito<Vec<(String, String)>> {
            self.0.namespace()
        }
        fn scrivi(&mut self, c: Controllo, v: &str) -> Esito<()> {
            self.0.scrivi(c, v)
        }
        fn rileggi(&self, c: Controllo) -> Esito<String> {
            self.0.rileggi(c)
        }
        fn eventi(&self) -> Esito<String> {
            self.0.eventi()
        }
    }

    let mut superficie = Altrove(Superficie::onesta());
    let errore =
        prepara_dominio(&mut superficie, 4096, WORKER).expect_err("fuori dal control plane");
    assert!(errore.to_string().contains("non sta sotto"), "{errore}");
    assert!(
        !superficie
            .0
            .registro
            .borrow()
            .iter()
            .any(|voce| voce.starts_with("scrivi")),
        "niente si scrive prima di sapere dove"
    );
}

/// `memory_localevents` si **registra**, non si rifiuta, e si cerca fra le
/// opzioni del **superblocco**.
#[test]
fn memory_localevents_si_registra_e_non_respinge() {
    let mut con = Superficie::onesta();
    con.montaggio = Some(Montaggio {
        opzioni_superblocco: "rw,nsdelegate,memory_localevents".to_owned(),
        ..con.montaggio.clone().expect("montaggio")
    });
    let esito = prepara_dominio(&mut con, 4096, WORKER).expect("l'opzione non e' un rifiuto");
    assert!(esito.eventi_locali);

    let mut senza = Superficie::onesta();
    assert!(
        !prepara_dominio(&mut senza, 4096, WORKER)
            .expect("preflight")
            .eventi_locali
    );
}

/// Il confronto sulle opzioni e' per **elemento**, non per sottostringa.
#[test]
fn una_opzione_che_contiene_il_nome_non_e_quella_opzione() {
    let mut superficie = Superficie::onesta();
    superficie.montaggio = Some(Montaggio {
        opzioni_superblocco: "rw,no_memory_localevents".to_owned(),
        ..superficie.montaggio.clone().expect("montaggio")
    });
    assert!(
        !prepara_dominio(&mut superficie, 4096, WORKER)
            .expect("preflight")
            .eventi_locali
    );
}

/// Il token porta tutto cio' che lo spawner usera', e lo porta **insieme**:
/// dominio canonico, radice, identita' giudicata, montaggio e namespace.
///
/// E' la forma che impedisce di verificare una combinazione ed eseguirne
/// un'altra.
#[test]
fn l_esito_porta_cio_che_lo_spawner_dovra_verificare() {
    let mut superficie = Superficie::onesta();
    let esito = prepara_dominio(&mut superficie, 268_435_456, WORKER).expect("preflight");
    assert_eq!(esito.tetto_byte, 268_435_456);
    assert_eq!(esito.dominio, PathBuf::from(DOMINIO));
    assert_eq!(esito.radice, PathBuf::from(RADICE));
    assert_eq!(esito.worker, WORKER);
    assert_eq!(esito.montaggio.punto, PathBuf::from("/sys/fs/cgroup"));
    assert_eq!(esito.montaggio.dispositivo, "0:27");
    assert_eq!(
        esito.namespace_attesi,
        vec![("user".to_owned(), "user:[4026531837]".to_owned())]
    );
}

/// Se il dominio **e'** la radice del control plane, non ci sono antenati da
/// giudicare.
///
/// Salirne uno significherebbe uscire dal perimetro dichiarato — sopra la
/// radice c'e' l'amministrazione della macchina — e senza il ramo esplicito il
/// ciclo non incontrerebbe mai la condizione di arresto, salendo fino a `/`.
#[test]
fn un_dominio_che_coincide_con_la_radice_non_ha_antenati() {
    let radice = Path::new("/sys/fs/cgroup/plenora");
    let bersagli = bersagli_del_possesso(radice, radice);

    let mut atteso = vec![radice.to_path_buf()];
    for file in [
        "cgroup.procs",
        "memory.max",
        "memory.swap.max",
        "memory.oom.group",
        "cgroup.max.depth",
    ] {
        atteso.push(radice.join(file));
    }
    assert_eq!(
        bersagli, atteso,
        "solo il dominio e i suoi file: nessun antenato, e in particolare non /sys/fs/cgroup"
    );
}

/// Senza un montaggio che contenga il dominio non si parte.
#[test]
fn senza_montaggio_non_si_parte() {
    let mut superficie = Superficie::onesta();
    superficie.montaggio = None;
    assert!(prepara_dominio(&mut superficie, 4096, WORKER)
        .expect_err("nessun montaggio")
        .to_string()
        .contains("montaggio"));
}

// --- il confine fra supervisore e spawner ---------------------------------

/// Il preflight prepara e rende un token; il token rende la richiesta.
fn richiesta_di_un_preflight_riuscito() -> RichiestaSpawner {
    let mut superficie = Superficie::onesta();
    prepara_dominio(&mut superficie, TETTO, WORKER)
        .expect("preflight")
        .consuma(CANALE_DEI_CASI)
        .0
}

/// La richiesta sopravvive al giro sugli argomenti **identica**.
///
/// E' la sola proprieta' che rende il confine attraversabile: se ando' e
/// ritorno cambiassero anche un campo, lo spawner rivaliderebbe un dominio
/// diverso da quello preparato, e lo farebbe senza accorgersene.
#[test]
fn la_richiesta_sopravvive_al_giro_sugli_argomenti() {
    let originale = richiesta_di_un_preflight_riuscito();
    let riletta = RichiestaSpawner::da_argomenti(&originale.in_argomenti()).expect("rilettura");
    assert_eq!(riletta, originale);
}

/// La richiesta porta la versione, ed e' il primo argomento.
///
/// Non e' un dettaglio di forma: e' cio' che impedisce a un supervisore e a uno
/// spawner di versioni diverse di interpretare gli argomenti l'uno dell'altro.
#[test]
fn una_versione_diversa_non_si_interpreta() {
    let mut pezzi = richiesta_di_un_preflight_riuscito().in_argomenti();
    pezzi[0] = std::ffi::OsString::from("plenora-spawner-0");
    assert!(RichiestaSpawner::da_argomenti(&pezzi)
        .expect_err("versione")
        .contains(VERSIONE_RICHIESTA));
}

/// Ogni forma malformata e' un rifiuto, e nessuna ha una lettura di ripiego.
#[test]
fn ogni_richiesta_malformata_e_un_rifiuto() {
    let buona = richiesta_di_un_preflight_riuscito().in_argomenti();

    // Un argomento in meno, e uno in piu': in entrambi i casi chi scrive e chi
    // legge non sono d'accordo su che cosa sia una richiesta.
    assert!(RichiestaSpawner::da_argomenti(&buona[..5]).is_err());
    let mut troppi = buona.clone();
    troppi.push(std::ffi::OsString::from("extra"));
    assert!(RichiestaSpawner::da_argomenti(&troppi).is_err());

    // Un percorso relativo non si confronta col percorso canonico che la
    // rivalidazione risolve.
    for indice in [1, 2] {
        let mut pezzi = buona.clone();
        pezzi[indice] = std::ffi::OsString::from("relativo/dominio");
        assert!(RichiestaSpawner::da_argomenti(&pezzi).is_err());
    }

    // Un numero che non e' un numero: uid, gid, tetto.
    for indice in [3, 4, 5] {
        let mut pezzi = buona.clone();
        pezzi[indice] = std::ffi::OsString::from("non-un-numero");
        assert!(RichiestaSpawner::da_argomenti(&pezzi).is_err());
    }

    // Un uid che non entra in u32 e' un uid che non esiste.
    let mut fuori = buona;
    fuori[3] = std::ffi::OsString::from("4294967296");
    assert!(RichiestaSpawner::da_argomenti(&fuori).is_err());
}

/// La rivalidazione **rilegge** i quattro controlli e non ne riscrive nessuno.
///
/// E' la differenza fra uno spawner che verifica e uno che rifa' il preflight:
/// il tetto deve essere gia' in vigore quando lo spawner nasce, e riscriverlo
/// qui vorrebbe dire che fra la nascita e il limite c'e' una finestra.
#[test]
fn la_rivalidazione_rilegge_e_non_riscrive() {
    let mut superficie = Superficie::onesta();
    let richiesta = prepara_dominio(&mut superficie, TETTO, WORKER)
        .expect("preflight")
        .consuma(CANALE_DEI_CASI)
        .0;
    superficie.registro.borrow_mut().clear();

    let padre = vec![("user".to_owned(), "user:[4026531837]".to_owned())];
    let rivalidato = rivalida(&superficie, &richiesta, padre.clone()).expect("rivalidazione");
    assert_eq!(rivalidato.namespace_del_padre, padre);
    assert_eq!(rivalidato.worker, WORKER);

    let registro = superficie.registro.borrow().clone();
    assert!(
        !registro.iter().any(|passo| passo.starts_with("scrivi")),
        "la rivalidazione ha scritto: {registro:?}"
    );

    let mut atteso = vec![
        "risolvi dominio".to_owned(),
        "risolvi control plane".to_owned(),
        "montaggio".to_owned(),
    ];
    for bersaglio in bersagli_del_possesso(Path::new(DOMINIO), Path::new(RADICE)) {
        atteso.push(format!("proprieta {}", bersaglio.display()));
    }
    atteso.extend([
        "rileggi memory.max".to_owned(),
        "rileggi memory.swap.max".to_owned(),
        "rileggi memory.oom.group".to_owned(),
        "rileggi cgroup.max.depth".to_owned(),
        "cgroup.events".to_owned(),
    ]);

    // Uguaglianza esatta, come per il preflight: e' il solo modo in cui un
    // caso sull'ordine fallisce quando l'ordine cambia. E il possesso si
    // rigiudica **tutto**, antenati compresi, perche' un permesso allentato
    // dopo il preflight vale quanto uno allentato prima.
    assert_eq!(registro, atteso);
}

/// Un controllo cambiato fra il preflight e lo spawner ferma lo spawner.
///
/// E' il motivo per cui la rivalidazione esiste: fra le due fasi passa uno
/// `spawn`, e in quel tempo qualcuno puo' aver riscritto il tetto. Uno spawner
/// che eseguisse sulla parola del proprio chiamante non se ne accorgerebbe.
#[test]
fn un_controllo_riscritto_nel_frattempo_ferma_lo_spawner() {
    for controllo in Controllo::ORDINE {
        let mut superficie = Superficie::onesta();
        let richiesta = prepara_dominio(&mut superficie, TETTO, WORKER)
            .expect("preflight")
            .consuma(CANALE_DEI_CASI)
            .0;
        superficie
            .risposte
            .borrow_mut()
            .insert(controllo.file(), "999".to_owned());

        let errore = rivalida(&superficie, &richiesta, Vec::new())
            .expect_err("un controllo divergente ferma lo spawner");
        assert!(
            errore.to_string().contains(controllo.file()),
            "l'esito non nomina {}: {errore}",
            controllo.file()
        );
    }
}

/// Un dominio popolato fra il preflight e lo spawner ferma lo spawner.
#[test]
fn un_dominio_popolato_nel_frattempo_ferma_lo_spawner() {
    let mut superficie = Superficie::onesta();
    let richiesta = prepara_dominio(&mut superficie, TETTO, WORKER)
        .expect("preflight")
        .consuma(CANALE_DEI_CASI)
        .0;
    superficie.eventi = Some("populated 1\nfrozen 0\n".to_owned());

    assert!(rivalida(&superficie, &richiesta, Vec::new())
        .expect_err("dominio popolato")
        .to_string()
        .contains("populated"));
}

/// Una richiesta che nomina un dominio diverso da quello risolto e' un rifiuto.
///
/// E' il caso in cui il confine viene attraversato da qualcosa che il
/// supervisore non ha scritto: la rivalidazione risolve i percorsi da se', e
/// pretende che coincidano con quelli nominati.
#[test]
fn una_richiesta_che_nomina_un_altro_dominio_e_un_rifiuto() {
    let superficie = Superficie::onesta();
    for (dominio, radice, atteso) in [
        ("/sys/fs/cgroup/plenora/dominio-8", RADICE, "dominio"),
        (DOMINIO, "/sys/fs/cgroup/altro", "control plane"),
    ] {
        let richiesta = RichiestaSpawner {
            dominio: PathBuf::from(dominio),
            radice: PathBuf::from(radice),
            uid: WORKER.uid,
            gid: WORKER.gid,
            tetto_byte: TETTO,
            worker_legge: 3,
            worker_scrive: 4,
        };
        let errore = rivalida(&superficie, &richiesta, Vec::new()).expect_err(atteso);
        assert!(
            errore.to_string().contains(atteso),
            "l'esito non nomina {atteso}: {errore}"
        );
    }
}

/// Una richiesta che nomina un'identita' che possiede la gerarchia e' un
/// rifiuto, e lo e' **nello spawner**.
///
/// Il possesso si giudica di nuovo contro l'identita' della richiesta, non
/// contro quella che il preflight aveva usato: altrimenti il confine
/// porterebbe un UID e la verifica ne guarderebbe un altro.
#[test]
fn lo_spawner_giudica_il_possesso_contro_l_identita_della_richiesta() {
    let mut superficie = Superficie::onesta();
    let richiesta = prepara_dominio(&mut superficie, TETTO, WORKER)
        .expect("preflight")
        .consuma(CANALE_DEI_CASI)
        .0;
    let padrone = RichiestaSpawner {
        uid: INTOCCABILE.uid,
        gid: INTOCCABILE.gid,
        ..richiesta
    };
    assert!(rivalida(&superficie, &padrone, Vec::new())
        .expect_err("il proprietario della gerarchia non e' un worker")
        .to_string()
        .contains("potrebbe scriverlo"));
}

// --- l'evidenza, e il binario che il chiamante non sceglie -----------------

/// Una superficie onesta il cui superblocco porta `memory_localevents`.
///
/// Serve a due casi che devono osservare **la stessa cosa**: quello del ramo
/// riuscito e quello del ramo fallito. Costruirla due volte a mano
/// permetterebbe che divergano, ed e' proprio l'uguaglianza fra le due
/// evidenze che il secondo caso confronta.
fn superficie_con_localevents() -> Superficie {
    let mut superficie = Superficie::onesta();
    if let Some(montaggio) = superficie.montaggio.as_mut() {
        montaggio.opzioni_superblocco = "rw,nsdelegate,memory_localevents".to_owned();
    }
    superficie
}

/// L'evidenza sopravvive alla transizione riuscita, `memory_localevents`
/// compreso.
///
/// E' il caso che vale quando conta: consumare il token e' cio' che succede
/// **nel ramo riuscito**, ed e' li' che un'evidenza attaccata al token andrebbe
/// persa. Il contratto promette `memory_localevents` registrato insieme al
/// sigillo, e una promessa mantenuta solo quando la transizione fallisce non e'
/// quella promessa.
#[test]
fn l_evidenza_sopravvive_al_consumo_del_token() {
    let mut con = superficie_con_localevents();
    let (richiesta, evidenza) = prepara_dominio(&mut con, TETTO, WORKER)
        .expect("preflight")
        .consuma(CANALE_DEI_CASI);

    assert!(
        evidenza.eventi_locali,
        "l'opzione osservata dal preflight non arriva dall'altra parte"
    );
    assert_eq!(evidenza.dominio, PathBuf::from(DOMINIO));
    assert_eq!(evidenza.radice, PathBuf::from(RADICE));
    assert_eq!(evidenza.worker, WORKER);
    assert_eq!(evidenza.tetto_byte, TETTO);
    assert_eq!(
        evidenza.montaggio,
        con.montaggio.clone().expect("montaggio")
    );
    assert_eq!(evidenza.namespace_attesi, con.namespace);

    // La richiesta porta invece **meno**: montaggio, namespace e opzioni non
    // attraversano il confine, perche' lo spawner li ritrova da se'.
    assert_eq!(richiesta.dominio, evidenza.dominio);
    assert_eq!(richiesta.tetto_byte, evidenza.tetto_byte);
}

/// Un'immagine rimossa sotto il processo non si riesegue.
#[test]
fn un_immagine_cancellata_non_si_riesegue() {
    let errore = spawner_ammissibile(
        Path::new("/opt/plenora/supervisore (deleted)"),
        true,
        INTOCCABILE,
        WORKER,
    )
    .expect_err("immagine cancellata");
    assert!(errore.to_string().contains("rimossa o sostituita"));
}

/// Cio' che non e' un file regolare non e' un eseguibile.
#[test]
fn cio_che_non_e_un_file_regolare_non_si_riesegue() {
    assert!(
        spawner_ammissibile(Path::new("/opt/plenora"), false, INTOCCABILE, WORKER)
            .expect_err("directory")
            .to_string()
            .contains("file regolare")
    );
}

/// Un binario che il worker puo' riscrivere non separa nessun privilegio.
///
/// E' la prova discriminante del vincolo: sono i tre modi in cui il permesso
/// cede — bit per altri, bit per gruppo (che con le ACL e' la mask della
/// classe), bit del proprietario quando il proprietario **e'** il worker — e
/// ciascuno da solo basta. Togliendo una delle tre condizioni, uno di questi
/// casi passa.
#[test]
fn un_binario_riscrivibile_dal_worker_non_e_uno_spawner() {
    for proprieta in [
        ProprietaFile {
            uid: 0,
            gid: 0,
            mode: 0o646,
        },
        ProprietaFile {
            uid: 0,
            gid: 0,
            mode: 0o664,
        },
        ProprietaFile {
            uid: WORKER.uid,
            gid: WORKER.gid,
            mode: 0o744,
        },
    ] {
        let errore = spawner_ammissibile(
            Path::new("/opt/plenora/supervisore"),
            true,
            proprieta,
            WORKER,
        )
        .expect_err("un binario riscrivibile dal worker");
        assert!(
            errore.to_string().contains("potrebbe riscriverlo"),
            "mode {:o}: {errore}",
            proprieta.mode
        );
    }
}

/// Un binario regolare, esistente e fuori dalla portata del worker passa.
///
/// Serve perche' i casi qui sopra non passino per il motivo sbagliato, cioe'
/// perche' la regola rifiuti tutto.
#[test]
fn un_binario_del_control_plane_e_uno_spawner_ammissibile() {
    assert!(spawner_ammissibile(
        Path::new("/opt/plenora/supervisore"),
        true,
        INTOCCABILE,
        WORKER
    )
    .is_ok());
}

/// Un percorso che non e' UTF-8 non e' un percorso rifiutato.
///
/// Un nome di cgroup e' una sequenza di byte, e un dominio il cui nome non si
/// decodifica resta un dominio valido: rifiutarlo sarebbe una restrizione che
/// nessuno ha dichiarato, e che il parser di `mountinfo` non applica.
#[test]
#[cfg(unix)]
fn un_percorso_non_utf8_attraversa_il_confine() {
    use std::os::unix::ffi::OsStringExt as _;

    // `0xFF` e `0xFE` non compaiono in nessuna sequenza UTF-8 valida: un nome
    // che li contiene e' un nome che `to_str()` rifiuterebbe.
    let mut byte = b"/sys/fs/cgroup/plenora/dominio-".to_vec();
    byte.extend([0xFF, 0xFE]);
    let grezzo = std::ffi::OsString::from_vec(byte);
    let richiesta = RichiestaSpawner {
        dominio: PathBuf::from(&grezzo),
        radice: PathBuf::from(RADICE),
        uid: WORKER.uid,
        gid: WORKER.gid,
        tetto_byte: TETTO,
        worker_legge: 3,
        worker_scrive: 4,
    };
    let riletta =
        RichiestaSpawner::da_argomenti(&richiesta.in_argomenti()).expect("un nome di byte");
    assert_eq!(riletta, richiesta);
}

/// L'evidenza sopravvive **intera** anche quando l'avvio fallisce.
///
/// # Perche' si prova su `esito` e non su `avvia`
///
/// Perche' l'affermazione da provare e' che i due rami portino la stessa
/// evidenza, e quella e' una proprieta' di `esito`: `avvia` ci aggiunge
/// l'accertamento dell'immagine e lo `spawn`, cioe' esattamente le due cose che
/// dipendono dalla macchina.
///
/// Provarla su `avvia` vorrebbe dire far fallire davvero un avvio, e il modo
/// naturale — far coincidere il worker col proprietario del binario dei casi,
/// cosi' che lo spawner risulti riscrivibile — dipende dal bit di scrittura del
/// proprietario, che e' probabile e non garantito: su un filesystem in sola
/// lettura, o con una build riproducibile, non c'e', e il caso comincerebbe ad
/// avviare processi invece di fallire.
///
/// Che l'immagine vera sia giudicata bene e' un'altra affermazione, e la prova
/// il gate su Linux. Qui non c'e' nessuna giuntura che permetta di saltare
/// quel giudizio: `esito` non lo esegue e non lo salta, non e' sulla sua
/// strada.
///
/// # Perche' il confronto e' integrale
///
/// Perche' la promessa e' che sopravviva `EvidenzaPreflight`, non alcuni dei
/// suoi campi: montaggio, namespace e `memory_localevents` sono osservazioni
/// quanto il dominio, e un confronto che ne guardasse solo qualcuna
/// lascerebbe passare la perdita delle altre.
#[test]
fn l_evidenza_sopravvive_intera_all_avvio_fallito() {
    let mut superficie = superficie_con_localevents();
    let evidenza = prepara_dominio(&mut superficie, TETTO, WORKER)
        .expect("preflight")
        .consuma(CANALE_DEI_CASI)
        .1;
    let atteso = evidenza.clone();

    // Il parametro dice `()` perche' `esito` non guarda cio' che il tentativo
    // rende: la sua regola e' come si compone un fallimento, e vale uguale per
    // un figlio e per niente. Fissarlo qui a un tipo senza contenuto e' anche il
    // modo di dirlo — se un domani `esito` cominciasse a toccare l'esito
    // riuscito, questo caso smetterebbe di compilare.
    let fallita = super::esito::<()>(
        Err(super::non_disponibile("immagine", "rifiuto iniettato").into()),
        evidenza,
    )
    .expect_err("un tentativo fallito rende la transizione fallita");

    assert!(fallita.causa.to_string().contains("rifiuto iniettato"));
    assert_eq!(fallita.evidenza, atteso);
    assert!(
        fallita.evidenza.eventi_locali,
        "un'osservazione che si perde solo nel ramo fallito si perde comunque"
    );
}

// --- il contratto del dispatch --------------------------------------------

/// Un `argv[1]` come lo vede il programma.
fn primo(testo: &str) -> std::ffi::OsString {
    std::ffi::OsString::from(testo)
}

/// La versione supportata entra nello spawner.
#[test]
fn la_versione_supportata_entra_nello_spawner() {
    assert_eq!(
        riconosci_dello_spawner(Some(&primo(VERSIONE_RICHIESTA))),
        Riconoscimento::Supportata
    );
}

/// Un comando ordinario prosegue per la sua strada, e cosi' l'assenza di
/// argomenti.
///
/// E' la meta' che si dimentica: un dispatch che catturasse troppo
/// trasformerebbe ogni invocazione della CLI in un tentativo di spawner.
#[test]
fn un_comando_ordinario_non_e_uno_spawner() {
    for testo in [
        "run", "catalog", "validate", "--help", "", "plenora", "spawner",
    ] {
        assert_eq!(
            riconosci_dello_spawner(Some(&primo(testo))),
            Riconoscimento::AltroComando,
            "«{testo}» non appartiene al namespace riservato"
        );
    }
    assert_eq!(riconosci_dello_spawner(None), Riconoscimento::AltroComando);
}

/// Ogni versione del namespace riservato che non sia quella supportata e' un
/// **rifiuto esplicito**, non una ricaduta nel parser della CLI.
///
/// # Perche' e' la parte che conta
///
/// Perche' e' la differenza fra due diagnosi: «versione non supportata», che
/// dice a un supervisore di un'altra generazione che cosa e' successo, e
/// «comando sconosciuto», che manda a cercare un errore di digitazione. La
/// seconda arriverebbe da sola se il riconoscimento guardasse la sola versione
/// esatta.
#[test]
fn ogni_altra_versione_del_namespace_e_un_rifiuto() {
    for testo in [
        "plenora-spawner-1",
        "plenora-spawner-3",
        "plenora-spawner-99",
        "plenora-spawner-",
        "plenora-spawner-2-bis",
        "plenora-spawner-due",
    ] {
        assert_eq!(
            riconosci_dello_spawner(Some(&primo(testo))),
            Riconoscimento::VersioneNonSupportata,
            "«{testo}» avrebbe dovuto essere un rifiuto"
        );
    }
}

/// Una versione piu' vecchia del protocollo si riconosce come tale, non come
/// comando ignoto.
///
/// E' cio' che incontra per primo un supervisore che non e' stato aggiornato, e
/// la risposta che riceve decide se sa che cosa aggiornare.
#[test]
fn una_versione_piu_vecchia_si_riconosce() {
    let precedente = "plenora-spawner-1";
    assert!(precedente.starts_with(PREFISSO_RISERVATO));
    assert_ne!(precedente, VERSIONE_RICHIESTA);
    assert_eq!(
        riconosci_dello_spawner(Some(&primo(precedente))),
        Riconoscimento::VersioneNonSupportata
    );
}

/// Il riconoscimento **non porta via niente** dell'argomento.
///
/// La variante e' senza campi, quindi non c'e' una stringa da copiare: e' la
/// forma in cui «non amplificare l'ingresso» non dipende da chi scrive il
/// messaggio dopo. Un `argv` che contenesse ritorni a capo o byte non UTF-8
/// non ha nessuna strada per arrivare in un log.
///
/// Il caso guarda il **comportamento**: due argomenti diversi, entrambi del
/// namespace riservato, danno lo stesso identico esito. Misurare invece la
/// dimensione dell'enum direbbe che cosa ha scelto il compilatore per il
/// layout, che e' un'altra cosa e potrebbe coincidere per caso.
#[test]
fn il_rifiuto_non_trattiene_l_argomento() {
    let velenoso = "plenora-spawner-\nriga finta: qualcosa di inventato";
    let innocuo = "plenora-spawner-7";
    assert_eq!(
        riconosci_dello_spawner(Some(&primo(velenoso))),
        Riconoscimento::VersioneNonSupportata
    );
    assert_eq!(
        riconosci_dello_spawner(Some(&primo(velenoso))),
        riconosci_dello_spawner(Some(&primo(innocuo))),
        "due argomenti diversi devono dare lo stesso esito: cio' che li distingue non sopravvive"
    );
}

/// Un `argv[1]` che non e' UTF-8 sta nel namespace se i suoi **byte** ci
/// stanno.
///
/// Passare per `to_str()` farebbe cadere nel parser della CLI una riga che nel
/// namespace riservato ci sta eccome, e la diagnosi tornerebbe a essere quella
/// sbagliata.
#[test]
#[cfg(unix)]
fn il_namespace_si_riconosce_sui_byte() {
    use std::os::unix::ffi::OsStringExt as _;

    let mut byte = PREFISSO_RISERVATO.as_bytes().to_vec();
    byte.extend([0xFF, 0xFE]);
    match riconosci_dello_spawner(Some(&std::ffi::OsString::from_vec(byte))) {
        Riconoscimento::VersioneNonSupportata => {}
        altro => panic!("un nome di byte nel namespace riservato e' {altro:?}"),
    }
}

/// La versione supportata sta nel namespace riservato.
///
/// Sembra ovvio, e non lo e': se qualcuno cambiasse la versione senza il
/// prefisso, il riconoscimento continuerebbe a funzionare per la versione
/// esatta e smetterebbe di funzionare per tutte le altre — cioe' tornerebbe il
/// difetto, invisibile.
#[test]
fn la_versione_supportata_appartiene_al_namespace() {
    assert!(
        VERSIONE_RICHIESTA.starts_with(PREFISSO_RISERVATO),
        "«{VERSIONE_RICHIESTA}» non comincia con «{PREFISSO_RISERVATO}»"
    );
}

// --- la forma canonica dei descrittori ------------------------------------

/// La forma canonica e' quella che il supervisore produce, e nient'altro.
///
/// Le forme rifiutate qui sono tutte forme che `str::parse` accetterebbe:
/// e' la differenza fra «leggere un numero» e «leggere cio' che il
/// produttore dichiarato emette».
#[test]
fn solo_la_forma_canonica_di_un_descrittore() {
    // Cio' che `i32::to_string()` produce.
    for (testo, atteso) in [
        ("0", 0),
        ("3", 3),
        ("42", 42),
        ("-1", -1),
        ("2147483647", i32::MAX),
    ] {
        assert_eq!(
            super::descrittore_canonico(testo),
            Ok(atteso),
            "«{testo}» e' forma canonica"
        );
    }

    // Cio' che `parse` accetterebbe e il supervisore non scrive mai.
    for testo in ["+3", "03", "-0", "-01", "007", "+0"] {
        assert!(
            super::descrittore_canonico(testo).is_err(),
            "«{testo}» non e' forma canonica"
        );
    }

    // E cio' che non e' proprio un numero.
    for testo in ["", "-", "tre", "3.0", "3a", " 3", "3 ", "0x3"] {
        assert!(
            super::descrittore_canonico(testo).is_err(),
            "«{testo}» non e' un numero"
        );
    }
}

/// `-1` ha forma canonica: il rifiuto arriva **dopo**, sul valore.
///
/// Le due domande sono diverse — «e' scritto come lo scriverebbe il
/// supervisore?» e «e' un descrittore che si puo' usare?» — e i due messaggi
/// mandano in due posti diversi. Il giudizio sul valore vive in
/// `canale::numero_ammissibile`, che e' di Linux; qui si fissa che la forma
/// **non** lo anticipa.
#[test]
fn un_descrittore_negativo_passa_la_forma() {
    assert_eq!(super::descrittore_canonico("-1"), Ok(-1));
    assert_eq!(super::descrittore_canonico("-2147483648"), Ok(i32::MIN));
}

// --- il riconoscimento della modalita' worker --------------------------------

/// Il riconoscimento **del worker**, per i casi che parlano di lui.
fn riconosci_del_worker(primo: Option<&std::ffi::OsString>) -> super::Riconoscimento {
    riconosci_modalita(primo, PREFISSO_WORKER, VERSIONE_WORKER)
}

/// La versione del worker sta nel proprio namespace.
///
/// Se non ci stesse, una versione diversa non verrebbe riconosciuta come tale e
/// cadrebbe nel parser della CLI: il rifiuto direbbe «comando sconosciuto» a chi
/// ha invece sbagliato versione.
#[test]
fn la_versione_del_worker_sta_nel_suo_namespace() {
    assert!(
        VERSIONE_WORKER.starts_with(PREFISSO_WORKER),
        "«{VERSIONE_WORKER}» non comincia con «{PREFISSO_WORKER}»"
    );
}

/// I due namespace sono **disgiunti**.
///
/// # Perche' non e' ovvio, e perche' va fissato
///
/// Perche' il riconoscimento e' una regola sola applicata due volte, e le due
/// applicazioni sono in fila. Se un prefisso fosse prefisso dell'altro, una
/// riga destinata a una modalita' verrebbe rivendicata dalla prima interrogata
/// — con un rifiuto che nomina la versione sbagliata, che e' peggio di nessun
/// rifiuto.
#[test]
fn i_due_namespace_non_si_sovrappongono() {
    assert!(!PREFISSO_WORKER.starts_with(PREFISSO_RISERVATO));
    assert!(!PREFISSO_RISERVATO.starts_with(PREFISSO_WORKER));
    assert_ne!(VERSIONE_WORKER, VERSIONE_RICHIESTA);
}

/// La versione esatta si riconosce; il namespace con un'altra versione si
/// **rifiuta**; tutto il resto passa.
#[test]
fn il_worker_si_riconosce_e_rifiuta_come_lo_spawner() {
    assert_eq!(
        riconosci_del_worker(Some(&primo(VERSIONE_WORKER))),
        super::Riconoscimento::Supportata
    );
    for testo in [
        "plenora-worker-0",
        "plenora-worker-2",
        "plenora-worker-",
        "plenora-worker-1-e-poi",
    ] {
        assert_eq!(
            riconosci_del_worker(Some(&primo(testo))),
            super::Riconoscimento::VersioneNonSupportata,
            "«{testo}» e' del namespace del worker, e va rifiutato per versione"
        );
    }
    for testo in ["run", "verify", "plenora-spawner-2", "--help", ""] {
        assert_eq!(
            riconosci_del_worker(Some(&primo(testo))),
            super::Riconoscimento::AltroComando,
            "«{testo}» non e' del namespace del worker"
        );
    }
    assert_eq!(
        riconosci_del_worker(None),
        super::Riconoscimento::AltroComando
    );
}

/// **Una modalita' non rivendica l'altra.**
///
/// La riga dello spawner non e' un worker con la versione sbagliata, e
/// viceversa: sono due namespace, e la distinzione deve reggere in entrambi i
/// versi. Senza, la prima modalita' interrogata rifiuterebbe le righe della
/// seconda nominando la propria versione attesa.
#[test]
fn nessuna_modalita_rivendica_le_righe_dell_altra() {
    assert_eq!(
        riconosci_del_worker(Some(&primo(VERSIONE_RICHIESTA))),
        super::Riconoscimento::AltroComando,
        "la riga dello spawner non appartiene al worker"
    );
    assert_eq!(
        riconosci_dello_spawner(Some(&primo(VERSIONE_WORKER))),
        super::Riconoscimento::AltroComando,
        "la riga del worker non appartiene allo spawner"
    );
}
