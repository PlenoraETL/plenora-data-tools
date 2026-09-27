//! Il chiamante di produzione (`PR-12`): **due domini** in sequenza — worker
//! e verificatore — spawner, protocollo, conduzione a stati e pubblicazione,
//! per una richiesta reale — non una fixture.
//!
//! Non reimplementa nessun passo: sequenzia `prepara_dominio`,
//! `spawner::avvia`, l'handshake di `prova` (una volta per dominio) e
//! `macchina::conduci_isolato`, la **stessa** macchina per entrambi i dialoghi
//! (`isolamento.md#2-quater-topologia-chi-osserva-chi`). La verifica avviene
//! nel dominio del **verificatore** ([`verificatore`](super::verificatore)),
//! su un handle distinto, dopo che il dominio del worker e' stato distrutto.
//! Vedi [`esegui_isolato`] per la sequenza e [`verifica_poi_pubblica`] per la
//! barriera fra verifica e pubblicazione.
//!
//! Vale anche qui la sentinella di [`super::figlio::FiglioVivo`]: un worker
//! che non si lascia raccogliere abortisce il processo, anche sotto
//! `plenora-cli run`. Un figlio fuori controllo con privilegi ceduti e' peggio
//! di un'interruzione dichiarata.
//!
//! Radice del cgroup2 delegato e identita' del worker sono configurazione del
//! **dispiegamento**, come la politica di memoria dell'host
//! ([`super::attivazione`]): variabili d'ambiente, mai il piano.

use std::path::{Path, PathBuf};
use std::time::Duration;

use plenora_core::contract::DataContract;
use plenora_core::error::{PlenoraError, Result};
use sha2::{Digest as _, Sha256};

use crate::cancellation::CancellationToken;
use crate::commit_token::CommitToken;
use crate::geo_transport::publish::EsitoDellaPubblicazione;
use crate::geo_transport::publish::PublishProfile;
use crate::planner::ValidatedGraph;
use crate::protocollo::codifica::codifica;
use crate::protocollo::lettore::leggi_frame;
use crate::protocollo::messaggi::{
    ConteggiDichiarati, Corpo, DescrittoreIngresso, DigestArtefatto, FormatoIngresso, Frame,
    Incarico, IncaricoVerifica,
};
use crate::pubblicazione;

use super::attivazione::ConcessioneDominio;
use super::dominio::Gerarchia;
use super::figlio::FiglioVivo;
use super::macchina;
use super::prova::{self, Guardiano, StatoDelGuardiano, TETTO_DELLA_PAROLA};
use super::sorgente::{interruttore, rendi_non_bloccante, SorgenteTerminabile, PASSO_DI_ATTESA};
use super::spawner::{avvia, DaEseguire};
use super::{non_disponibile, prepara_dominio, IdentitaWorker};

/// Variabile d'ambiente della directory che fa da radice a un cgroup2
/// delegato, sotto cui questo processo puo' creare sottocgroup propri.
pub const VARIABILE_RADICE_CGROUP: &str = "PLENORA_ISOLATION_CGROUP_ROOT";

/// Variabile d'ambiente dell'identita' a cui il worker confinato cede i
/// privilegi, nella forma `uid:gid`.
pub const VARIABILE_IDENTITA_WORKER: &str = "PLENORA_ISOLATION_WORKER_UIDGID";

/// Variabile d'ambiente del tempo massimo concesso all'esecuzione isolata.
///
/// Conta dall'accordo concluso alla dichiarazione dell'esito; l'handshake ha
/// il proprio tetto fisso ([`TETTO_DELLA_PAROLA`]). Del dispiegamento, mai del
/// piano, e senza default: nessun numero varrebbe per ogni piano.
pub const VARIABILE_TEMPO_DI_ESECUZIONE: &str = "PLENORA_ISOLATION_EXECUTION_TIMEOUT_SECONDS";

/// Perche' la configurazione del dispiegamento non basta a costruire un
/// dominio.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RifiutoConfigurazione {
    RadiceCgroupAssente,
    IdentitaAssente,
    IdentitaMalformata,
    TempoAssente,
    TempoNonNumerico,
    TempoZero,
}

impl RifiutoConfigurazione {
    fn detto(self) -> PlenoraError {
        let motivo = match self {
            Self::RadiceCgroupAssente => format!(
                "la variabile «{VARIABILE_RADICE_CGROUP}» non c'e': nessuna radice cgroup2 \
                 delegata, il profilo isolato non e' disponibile"
            ),
            Self::IdentitaAssente => format!(
                "la variabile «{VARIABILE_IDENTITA_WORKER}» non c'e': nessuna identita' a cui il \
                 worker possa cedere i privilegi"
            ),
            Self::IdentitaMalformata => {
                format!("la variabile «{VARIABILE_IDENTITA_WORKER}» non e' nella forma «uid:gid»")
            }
            Self::TempoAssente => format!(
                "la variabile «{VARIABILE_TEMPO_DI_ESECUZIONE}» non c'e': nessun tetto per \
                 l'esecuzione isolata"
            ),
            Self::TempoNonNumerico => format!(
                "la variabile «{VARIABILE_TEMPO_DI_ESECUZIONE}» non e' un numero di secondi"
            ),
            Self::TempoZero => format!(
                "la variabile «{VARIABILE_TEMPO_DI_ESECUZIONE}» e' zero: un tetto a zero non e' \
                 un tetto, e' un rifiuto travestito"
            ),
        };
        PlenoraError::IsolationUnavailable(motivo)
    }
}

/// Il giudizio sull'identita' letta, separato dalla lettura per lo stesso
/// principio di [`super::attivazione::politica_da`].
fn identita_da(grezzo: Option<&str>) -> std::result::Result<(u32, u32), RifiutoConfigurazione> {
    let grezzo = grezzo.ok_or(RifiutoConfigurazione::IdentitaAssente)?;
    let (uid, gid) = grezzo
        .split_once(':')
        .ok_or(RifiutoConfigurazione::IdentitaMalformata)?;
    let uid: u32 = uid
        .trim()
        .parse()
        .map_err(|_| RifiutoConfigurazione::IdentitaMalformata)?;
    let gid: u32 = gid
        .trim()
        .parse()
        .map_err(|_| RifiutoConfigurazione::IdentitaMalformata)?;
    Ok((uid, gid))
}

#[cfg(target_os = "linux")]
fn radice_cgroup() -> Result<PathBuf> {
    std::env::var(VARIABILE_RADICE_CGROUP)
        .ok()
        .filter(|testo| !testo.is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| RifiutoConfigurazione::RadiceCgroupAssente.detto())
}

#[cfg(target_os = "linux")]
fn identita_worker() -> Result<IdentitaWorker> {
    let (uid, gid) = identita_da(std::env::var(VARIABILE_IDENTITA_WORKER).ok().as_deref())
        .map_err(RifiutoConfigurazione::detto)?;
    Ok(IdentitaWorker { uid, gid })
}

/// Il giudizio sul tempo letto, separato dalla lettura per lo stesso
/// principio di [`identita_da`] e di [`super::attivazione::politica_da`].
fn tempo_da(grezzo: Option<&str>) -> std::result::Result<Duration, RifiutoConfigurazione> {
    let grezzo = grezzo.ok_or(RifiutoConfigurazione::TempoAssente)?;
    let secondi: u64 = grezzo
        .trim()
        .parse()
        .map_err(|_| RifiutoConfigurazione::TempoNonNumerico)?;
    if secondi == 0 {
        return Err(RifiutoConfigurazione::TempoZero);
    }
    Ok(Duration::from_secs(secondi))
}

#[cfg(target_os = "linux")]
fn tempo_di_esecuzione() -> Result<Duration> {
    tempo_da(std::env::var(VARIABILE_TEMPO_DI_ESECUZIONE).ok().as_deref())
        .map_err(RifiutoConfigurazione::detto)
}

/// Un identificativo di tentativo praticamente unico, non crittografico: la
/// sua sola funzione e' separare due esecuzioni nel nome della directory di
/// dominio e nel `commit_token`, non custodire un segreto. Nessuna
/// dipendenza di generazione casuale esiste nel workspace: si compone
/// processo, istante e un contatore di processo con lo `sha2` gia' in uso
/// per i digest, invece di aggiungerne una.
fn identificativo_del_tentativo() -> [u8; 32] {
    use std::sync::atomic::{AtomicU64, Ordering};
    static CONTATORE: AtomicU64 = AtomicU64::new(0);

    let mut digestore = Sha256::new();
    digestore.update(std::process::id().to_le_bytes());
    digestore.update(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
            .to_le_bytes(),
    );
    digestore.update(CONTATORE.fetch_add(1, Ordering::Relaxed).to_le_bytes());
    digestore.finalize().into()
}

fn token_del_tentativo() -> Result<CommitToken> {
    let byte = identificativo_del_tentativo();
    let esadecimale = byte.iter().fold(String::with_capacity(64), |mut testo, b| {
        use std::fmt::Write as _;
        let _ = write!(testo, "{b:02x}");
        testo
    });
    CommitToken::da_esadecimale(&esadecimale).map_err(|forma| {
        PlenoraError::Internal(format!(
            "commit_token generato in forma non canonica: {forma}"
        ))
    })
}

/// Il nome di una sottodirectory di dominio, univoco per tentativo.
fn nome_del_dominio(identificativo: &[u8; 32]) -> String {
    let corto = identificativo
        .iter()
        .take(8)
        .fold(String::with_capacity(16), |mut testo, b| {
            use std::fmt::Write as _;
            let _ = write!(testo, "{b:02x}");
            testo
        });
    format!("plenora-isolato-{corto}")
}

/// L'incarico e il contratto d'uscita, costruiti dal grafo REALE gia'
/// validato e dai percorsi REALI degli ingressi — non dalla fixture di
/// qualificazione.
///
/// Il grafo arriva gia' validato (`planner::validate`, nel percorso CLI):
/// rivalidare produrrebbe un secondo `plan_hash`, che potrebbe divergere da
/// quello su cui il chiamante ha deciso.
fn incarico_per(
    graph: &ValidatedGraph,
    ingressi: &[(String, PathBuf)],
    temporaneo: &Path,
) -> Result<(Incarico, DataContract)> {
    let piano_canonico =
        serde_json::value::RawValue::from_string(graph.plan().canonical_json().to_string())
            .map_err(|causa| {
                PlenoraError::Internal(format!("piano canonico non e' JSON: {causa}"))
            })?;
    let plan_hash_atteso =
        crate::protocollo::digest::DigestSha256::da_esadecimale(&graph.plan_hash().to_hex())
            .map_err(|forma| {
                PlenoraError::Internal(format!("plan_hash in forma non canonica: {forma}"))
            })?;

    let nomi_dichiarati = graph.plan().inputs();
    let impronte = graph.input_contract_fingerprints();
    if nomi_dichiarati.len() != impronte.len() {
        return Err(PlenoraError::Internal(
            "il numero di impronte di contratto non combacia con gli input dichiarati dal piano"
                .to_owned(),
        ));
    }

    let mut descrittori = Vec::with_capacity(ingressi.len());
    for (nome, percorso) in ingressi {
        let posizione = nomi_dichiarati
            .iter()
            .position(|dichiarato| dichiarato == nome)
            .ok_or_else(|| {
                PlenoraError::InvalidPlan(format!("input `{nome}` non dichiarato dal piano"))
            })?;
        let contract_fingerprint_atteso =
            crate::protocollo::digest::DigestSha256::da_esadecimale(&impronte[posizione].to_hex())
                .map_err(|forma| {
                    PlenoraError::Internal(format!(
                        "fingerprint di contratto in forma non canonica: {forma}"
                    ))
                })?;
        descrittori.push(DescrittoreIngresso {
            nome: nome.clone(),
            percorso: percorso_in_testo(percorso)?,
            formato: FormatoIngresso::File,
            contract_fingerprint_atteso,
        });
    }

    Ok((
        Incarico {
            piano_canonico,
            plan_hash_atteso,
            ingressi: descrittori,
            artefatto_temporaneo: percorso_in_testo(temporaneo)?,
        },
        graph.output_contract()?.clone(),
    ))
}

fn percorso_in_testo(percorso: &Path) -> Result<String> {
    percorso.to_str().map(str::to_owned).ok_or_else(|| {
        PlenoraError::InvalidConfiguration(format!(
            "il percorso {} non e' UTF-8, e il protocollo porta stringhe",
            percorso.display()
        ))
    })
}

/// Esegue un piano che richiede il profilo isolato, per davvero: **due**
/// domini in sequenza — quello del worker e quello del verificatore — con la
/// politica dell'host gia' autorizzata dal chiamante.
///
/// Il dominio del worker e' **distrutto**, non solo svuotato, prima che nasca
/// quello del verificatore (`isolamento.md#2-quater-topologia-chi-osserva-chi`):
/// i picchi dei due non si sommano solo se sono davvero sequenziali.
/// [`esegui_il_worker`] rimuove il proprio dominio prima di rendere, qualunque
/// sia l'esito, e solo dopo si crea il secondo.
///
/// # Errors
///
/// [`PlenoraError::IsolationUnavailable`] se il dispiegamento non e'
/// configurato per l'isolamento (radice cgroup, identita' del worker) o se
/// una qualunque fase del preflight cede; altrimenti l'errore classificato
/// del dialogo del worker, del dialogo di verifica, o della pubblicazione.
#[cfg(target_os = "linux")]
pub fn esegui_isolato(
    graph: &ValidatedGraph,
    ingressi: &[(String, PathBuf)],
    destinazione: &Path,
    concessione: ConcessioneDominio,
    annullamento_esterno: CancellationToken,
) -> Result<EsitoDellaPubblicazione> {
    let radice = radice_cgroup()?;
    let worker = identita_worker()?;
    let tempo_di_esecuzione = tempo_di_esecuzione()?;
    let immagine =
        std::env::current_exe().map_err(|causa| non_disponibile("immagine", &causa.to_string()))?;
    let digest_immagine = prova::digest_dell_immagine(&immagine)?;

    // --- il dominio del worker: dalla nascita alla distruzione --------------
    let identificativo = identificativo_del_tentativo();
    let dominio_worker = radice.join(nome_del_dominio(&identificativo));
    std::fs::create_dir(&dominio_worker).map_err(|causa| {
        non_disponibile(
            "dominio",
            &format!("mkdir {}: {causa}", dominio_worker.display()),
        )
    })?;
    let esito_worker = esegui_il_worker(
        &dominio_worker,
        &radice,
        worker,
        &immagine,
        &digest_immagine,
        graph,
        ingressi,
        concessione.concesso_byte,
        tempo_di_esecuzione,
        annullamento_esterno.clone(),
    );
    // Il dominio del worker si distrugge **qui**, prima di guardare l'esito e
    // prima che il dominio del verificatore possa esistere: e' l'invariante
    // stessa, non una conseguenza del controllo di errore che segue. Un
    // dominio non ripulito e' un residuo da segnalare, non un secondo motivo
    // di rifiuto quando il tentativo e' gia' riuscito o gia' fallito per
    // un'altra ragione.
    if let Err(difetto) = std::fs::remove_dir(&dominio_worker) {
        eprintln!(
            "plenora: il dominio {} (worker) non si e' lasciato rimuovere dopo il tentativo: \
             {difetto}",
            dominio_worker.display()
        );
    }
    let (_cartella_lavoro, temporaneo, contratto_di_uscita, token, digest, conteggi) =
        esito_worker?;

    // --- il dominio del verificatore, e la pubblicazione --------------------
    //
    // Il dominio del worker e' gia' rimosso. `_cartella_lavoro` vive fino alla
    // fine: il suo `Drop` cancella la directory di `temporaneo`, che il
    // coordinatore deve ancora aprire.
    //
    // Fra i due domini nessun `conduci_isolato` sorveglia
    // `annullamento_esterno`: questo controllo evita di far nascere un secondo
    // dominio per un tentativo gia' cancellato. Il secondo controllo, appena
    // prima dello spawn, e' in `dialoga_con_verificatore`.
    if annullamento_esterno.is_cancelled() {
        // `Cancelled`, non `Internal`: stessa categoria e stesso exit code
        // (130) di ogni altra cancellazione isolata (vedi
        // `macchina::interpreta_classificato`) — non e' un difetto interno,
        // e' la cancellazione cooperativa osservata prima ancora che il
        // dominio del verificatore nascesse.
        return Err(PlenoraError::Cancelled {
            node: "verificatore".to_owned(),
            operation: "creazione dominio".to_owned(),
            execution_id: String::new(),
            reason: "l'esecuzione isolata e' stata cancellata prima che il dominio del \
                     verificatore nascesse"
                .to_owned(),
        });
    }
    let identificativo_verifica = identificativo_del_tentativo();
    let dominio_verificatore = radice.join(nome_del_dominio_verifica(&identificativo_verifica));
    std::fs::create_dir(&dominio_verificatore).map_err(|causa| {
        non_disponibile(
            "dominio",
            &format!("mkdir {}: {causa}", dominio_verificatore.display()),
        )
    })?;
    let risultato = esegui_il_verificatore_e_pubblica(
        &dominio_verificatore,
        &radice,
        worker,
        &immagine,
        &digest_immagine,
        &temporaneo,
        &contratto_di_uscita,
        &token,
        &digest,
        conteggi,
        graph.effective_limits().max_governed_memory_bytes,
        concessione.concesso_byte,
        tempo_di_esecuzione,
        destinazione,
        annullamento_esterno,
    );
    if let Err(difetto) = std::fs::remove_dir(&dominio_verificatore) {
        eprintln!(
            "plenora: il dominio {} (verificatore) non si e' lasciato rimuovere dopo il \
             tentativo: {difetto}",
            dominio_verificatore.display()
        );
    }
    risultato
}

/// Il nome della sottodirectory di dominio del **verificatore**.
///
/// Un identificativo nuovo, non quello del worker con un suffisso: i due
/// domini non coesistono, ma i nomi devono distinguersi a colpo d'occhio fra
/// le directory residue.
fn nome_del_dominio_verifica(identificativo: &[u8; 32]) -> String {
    let corto = identificativo
        .iter()
        .take(8)
        .fold(String::with_capacity(16), |mut testo, b| {
            use std::fmt::Write as _;
            let _ = write!(testo, "{b:02x}");
            testo
        });
    format!("plenora-verifica-{corto}")
}

/// Conclude l'handshake col worker, col proprio guardiano e il proprio
/// tetto fisso ([`TETTO_DELLA_PAROLA`]).
///
/// L'handshake ha taglia fissa, indipendente dal piano; il resto del dialogo
/// ha il tetto `tempo_di_esecuzione`. Un interruttore condiviso confonderebbe
/// a chi appartiene la scadenza, per questo la sorgente grezza torna al
/// chiamante e non resta in un `SorgenteTerminabile`.
///
/// `soggetto` nomina chi sta dall'altro capo (`"worker"` o `"verificatore"`),
/// solo per i messaggi, come in [`macchina::conduci_isolato`]: l'handshake
/// concorda le stesse quattro cose per entrambi i ruoli.
///
/// # Errors
///
/// L'errore dell'handshake, o un timeout se il proprio guardiano scade
/// prima, con la sua provenienza ([`FallimentoDellHandshake`]). Il figlio e'
/// gia' chiuso e i suoi difetti di pulizia gia' riportati: il chiamante non
/// deve chiudere una seconda volta.
#[cfg(target_os = "linux")]
fn concludi_handshake(
    soggetto: &str,
    supervisore: crate::protocollo::handshake::SupervisoreInAttesa,
    lettore_grezzo: std::io::PipeReader,
    mut scrittore: std::io::PipeWriter,
    guardia: FiglioVivo<std::process::Child>,
) -> std::result::Result<
    (
        std::io::PipeReader,
        std::io::PipeWriter,
        crate::protocollo::handshake::HandshakeAccettato,
        FiglioVivo<std::process::Child>,
    ),
    FallimentoDellHandshake,
> {
    // Nessun `?` finche' `guardia` e' viva: un errore la lascerebbe cadere
    // col figlio dentro, e la sentinella di `FiglioVivo` interromperebbe il
    // processo invece di rendere l'errore.
    let rinuncia = |guardia: FiglioVivo<std::process::Child>, causa: PlenoraError| {
        let (_uscita, difetti_di_pulizia) = super::prova::chiudi(guardia, Some(&causa));
        for difetto in &difetti_di_pulizia {
            eprintln!("plenora: pulizia del {soggetto} isolato: {difetto}");
        }
        causa
    };
    if let Err(causa) = rendi_non_bloccante(&lettore_grezzo) {
        return Err(FallimentoDellHandshake::DelCanale(rinuncia(guardia, causa)));
    }
    let (spia, freno) = interruttore();
    let guardiano = match Guardiano::comincia(freno, spia.clone(), TETTO_DELLA_PAROLA) {
        Ok(guardiano) => guardiano,
        Err(causa) => return Err(FallimentoDellHandshake::DelCanale(rinuncia(guardia, causa))),
    };
    let mut sorgente = SorgenteTerminabile::con_interruttore(lettore_grezzo, PASSO_DI_ATTESA, spia);

    // Solo il giudizio su una `Risposta` arrivata per intero e' un fatto
    // accertato; scrivere il saluto e leggere la risposta sono il canale.
    let esito = (|| -> std::result::Result<_, FallimentoDellHandshake> {
        let saluto = Frame::nuovo(Corpo::Saluto(Box::new(supervisore.saluto().clone())));
        codifica(&saluto)
            .and_then(|byte| prova::scrivi(&mut scrittore, &byte))
            .map_err(FallimentoDellHandshake::DelCanale)?;
        let Some(frame) = leggi_frame(&mut sorgente).map_err(FallimentoDellHandshake::DelCanale)?
        else {
            return Err(FallimentoDellHandshake::DelCanale(non_disponibile(
                "prova",
                &format!("il {soggetto} ha chiuso senza rispondere al saluto"),
            )));
        };
        supervisore
            .ricevi(frame)
            .map_err(FallimentoDellHandshake::DelGiudizio)
    })();
    let stato_guardiano = guardiano.ferma_e_raccogli();
    let lettore = sorgente.dentro();

    match esito {
        Ok(accordo) => Ok((lettore, scrittore, accordo, guardia)),
        Err(fallimento) => {
            let (_uscita, difetti_di_pulizia) =
                super::prova::chiudi(guardia, Some(fallimento.causa()));
            for difetto in &difetti_di_pulizia {
                eprintln!("plenora: pulizia del {soggetto} isolato: {difetto}");
            }
            drop(scrittore);
            // Un guardiano scaduto dice che la parola non e' arrivata in
            // tempo: qualunque cosa sia seguita, non e' un giudizio.
            Err(if stato_guardiano == StatoDelGuardiano::Scaduto {
                FallimentoDellHandshake::DelCanale(non_disponibile(
                    "prova",
                    &format!(
                        "l'handshake col {soggetto} isolato non si e' concluso entro {} secondi: \
                         {}",
                        TETTO_DELLA_PAROLA.as_secs(),
                        fallimento.causa()
                    ),
                ))
            } else {
                fallimento
            })
        }
    }
}

/// Da dove viene il fallimento di un handshake, perche' la rilettura
/// dell'evidenza lo pesa diversamente.
#[cfg(target_os = "linux")]
#[derive(Debug)]
enum FallimentoDellHandshake {
    /// Il canale: il saluto non si scrive, la risposta non arriva o arriva
    /// troncata, il guardiano scade. Un dominio che poi va terminato con
    /// `cgroup.kill` rende l'esito ambiguo.
    DelCanale(PlenoraError),
    /// Il giudizio del supervisore su una `Risposta` arrivata per intero:
    /// protocollo, artefatto, resolver, ambiente o capability incompatibili
    /// (righe 9 e 10 della matrice). E' un fatto accertato, e una quiescenza
    /// tardiva non lo rende ambiguo.
    DelGiudizio(PlenoraError),
}

#[cfg(target_os = "linux")]
impl FallimentoDellHandshake {
    const fn causa(&self) -> &PlenoraError {
        match self {
            Self::DelCanale(causa) | Self::DelGiudizio(causa) => causa,
        }
    }

    /// Rilegge la causa alla luce dell'evidenza del dominio.
    fn rileggi(
        self,
        evidenza: macchina::EvidenzaDaPrimaDelloSpawn,
        soggetto: &str,
    ) -> PlenoraError {
        match self {
            Self::DelCanale(causa) => evidenza.rileggi_il_fallimento(soggetto, causa),
            Self::DelGiudizio(causa) => evidenza.rileggi_il_giudizio(soggetto, causa),
        }
    }
}

/// Il dominio del worker: esecuzione, artefatto scritto sul temporaneo, e
/// **nient'altro**.
///
/// Non verifica e non pubblica: sono passi del dominio del verificatore. Rende
/// la `TempDir` (viva finche' il verificatore non ha letto `temporaneo`), il
/// percorso del temporaneo, il contratto d'uscita, il `commit_token` e il
/// digest e i conteggi **dichiarati** dal worker.
///
/// # Errors
///
/// L'errore classificato del dialogo col worker, o un errore di preparazione
/// del dominio/temporaneo.
#[cfg(target_os = "linux")]
#[allow(clippy::too_many_arguments)]
fn esegui_il_worker(
    dominio: &Path,
    radice: &Path,
    worker: IdentitaWorker,
    immagine: &Path,
    digest_immagine: &str,
    graph: &ValidatedGraph,
    ingressi: &[(String, PathBuf)],
    concesso_byte: u64,
    tempo_di_esecuzione: Duration,
    annullamento_esterno: CancellationToken,
) -> Result<(
    tempfile::TempDir,
    PathBuf,
    DataContract,
    CommitToken,
    DigestArtefatto,
    ConteggiDichiarati,
)> {
    let cartella_lavoro = tempfile::tempdir()
        .map_err(|causa| non_disponibile("temporaneo", &format!("directory di lavoro: {causa}")))?;
    // Il supervisore la crea con la propria identita'; il worker, che ne ha
    // una distinta, deve potervi CREARE il proprio file in esclusiva — non
    // solo aprirlo, quindi il solo permesso non basta: serve il possesso. Il
    // supervisore lo cede perche' e' l'unico dei due ad avere il privilegio
    // di farlo.
    std::os::unix::fs::chown(cartella_lavoro.path(), Some(worker.uid), Some(worker.gid)).map_err(
        |causa| {
            non_disponibile(
                "temporaneo",
                &format!("possesso della directory di lavoro: {causa}"),
            )
        },
    )?;
    let temporaneo = cartella_lavoro.path().join("artefatto.arrow");

    let mut gerarchia = Gerarchia::nuova(dominio, radice).map_err(|difetto| {
        non_disponibile(dominio.to_string_lossy().as_ref(), &difetto.to_string())
    })?;
    // Il tetto del dominio e' quello AUTORIZZATO dalla politica dell'host
    // (`attivazione::prepara_e_autorizza`, gia' eseguita dal chiamante),
    // non quello grezzo richiesto dal piano: altrimenti il taglio della
    // politica dell'host non avrebbe alcun effetto sul dominio reale.
    let preparato = prepara_dominio(&mut gerarchia, concesso_byte, worker)?;
    // Il «prima» dell'evidenza, adesso: `avvia` consuma `preparato`, e dopo
    // non si potrebbe piu' prendere (`macchina::EvidenzaDaPrimaDelloSpawn`).
    let evidenza = macchina::EvidenzaDaPrimaDelloSpawn::prendi(&preparato);

    let (incarico, contratto_di_uscita) = incarico_per(graph, ingressi, &temporaneo)?;
    let token = token_del_tentativo()?;

    let argomento_worker: std::ffi::OsString = super::VERSIONE_WORKER.into();
    let da_eseguire = DaEseguire {
        eseguibile: immagine,
        argomenti: std::slice::from_ref(&argomento_worker),
    };
    // Nessun terzo descrittore: il worker non riceve mai l'artefatto in
    // lettura — non ne ha ancora scritto uno — ed e' comunque il
    // verificatore, non il worker, a doverlo rileggere.
    let riuscita = avvia(preparato, &da_eseguire, None).map_err(|fallita| {
        let super::TransizioneFallita {
            causa,
            evidenza,
            difetto_di_pulizia,
        } = *fallita;
        eprintln!("plenora: preflight del dominio isolato al momento del fallimento: {evidenza:?}");
        if let Some(residuo) = &difetto_di_pulizia {
            eprintln!("plenora: pulizia dopo lo spawn fallito: {residuo}");
        }
        causa.con_contesto("lo spawner del profilo isolato non e' partito")
    })?;
    let guardia = FiglioVivo::nuovo(riuscita.figlio);
    eprintln!(
        "plenora: preflight del dominio isolato riuscito, worker pid {:?}: {:?}",
        guardia.pid(),
        riuscita.evidenza
    );
    // Ogni fallimento fra qui e la conduzione ha gia' chiuso il figlio, e
    // passa dall'evidenza del dominio prima di uscire
    // (`EvidenzaDaPrimaDelloSpawn::rileggi_il_fallimento`).
    let (supervisore, guardia) =
        match supervisore_o_raccogli("worker isolato", digest_immagine, token, guardia) {
            Ok(coppia) => coppia,
            Err(causa) => return Err(evidenza.rileggi_il_fallimento("worker", causa)),
        };
    let (lettore, mut scrittore, accordo, guardia) = match concludi_handshake(
        "worker",
        supervisore,
        riuscita.supervisore_legge,
        riuscita.supervisore_scrive,
        guardia,
    ) {
        Ok(quattro) => quattro,
        Err(fallimento) => return Err(fallimento.rileggi(evidenza, "worker")),
    };

    // --- l'incarico, sullo stesso canale ------------------------------------
    let incarico_frame = Frame::nuovo(Corpo::Incarico(Box::new(incarico)));
    if let Err(causa) =
        codifica(&incarico_frame).and_then(|byte| prova::scrivi(&mut scrittore, &byte))
    {
        let (_uscita, difetti_di_pulizia) = super::prova::chiudi(guardia, Some(&causa));
        for difetto in &difetti_di_pulizia {
            eprintln!("plenora: pulizia del worker isolato: {difetto}");
        }
        drop(scrittore);
        return Err(evidenza.rileggi_il_fallimento("worker", causa));
    }

    // --- il resto del dialogo, condotto dalla macchina a stati --------------
    //
    // Da qui in poi non e' piu' `isolamento::prova` a guidare: e'
    // `macchina::conduci_isolato`, con gli adattatori reali del dominio.
    // `scrittore` passa per intero: la conduzione lo usa per l'`Annulla`,
    // che sorveglia `annullamento_esterno` — lo stesso token che l'handler
    // Ctrl-C della CLI cancella, e che il percorso in-process osserva.
    let esito = macchina::conduci_isolato(
        macchina::Ruolo::Worker,
        lettore,
        accordo,
        scrittore,
        tempo_di_esecuzione,
        guardia,
        dominio.to_path_buf(),
        evidenza,
        annullamento_esterno,
    );
    let (digest, conteggi) = esito?;

    Ok((
        cartella_lavoro,
        temporaneo,
        contratto_di_uscita,
        token,
        digest,
        conteggi,
    ))
}

/// Consegna `supervisore_per`, o raccoglie la guardia prima di restituire
/// l'errore.
///
/// Fra `FiglioVivo::nuovo` e `concludi_handshake`, `supervisore_per` puo'
/// rifiutare legittimamente (per esempio ambiente PROJ non inventariabile):
/// un `?` nudo lascerebbe la guardia alla sentinella, che abortirebbe invece
/// di restituire l'errore. Condivisa da worker e verificatore.
///
/// # Errors
///
/// La causa di `supervisore_per`, sola se la raccolta del figlio non ha
/// lasciato difetti; altrimenti un errore che porta **entrambi** i fatti, come
/// `prova::con_la_pulizia`.
#[cfg(target_os = "linux")]
fn supervisore_o_raccogli<P: super::figlio::ProcessoFiglio>(
    cosa: &str,
    digest_immagine: &str,
    token: CommitToken,
    guardia: FiglioVivo<P>,
) -> Result<(
    crate::protocollo::handshake::SupervisoreInAttesa,
    FiglioVivo<P>,
)> {
    match prova::supervisore_per(digest_immagine, token) {
        Ok(supervisore) => Ok((supervisore, guardia)),
        Err(causa) => {
            let (_uscita, difetti_di_pulizia) = super::prova::chiudi(guardia, Some(&causa));
            Err(if difetti_di_pulizia.is_empty() {
                causa
            } else {
                non_disponibile(
                    cosa,
                    &format!(
                        "{causa}; e la pulizia del figlio ha lasciato: {difetti_di_pulizia:?}"
                    ),
                )
            })
        }
    }
}

/// L'`IncaricoVerifica`: contratto atteso (per fingerprint), digest atteso,
/// conteggi attesi e budget di memoria governata, nella forma sul filo.
///
/// Il `commit_token` non c'e': viaggia nel `Saluto`
/// (`isolamento.md#43-messaggi`), e il verificatore lo riceve da
/// `WorkerAccordato::commit_token`.
///
/// # Errors
///
/// [`PlenoraError::Internal`] se il contratto d'uscita non si riduce a un
/// fingerprint — impossibile per costruzione, ma non dimostrabile al
/// compilatore (la stessa non-garanzia di
/// [`crate::planner::contract_fingerprint`]).
fn incarico_verifica_per(
    contratto_di_uscita: &DataContract,
    digest: &DigestArtefatto,
    conteggi: ConteggiDichiarati,
    budget_memoria_governata_bytes: u64,
) -> Result<IncaricoVerifica> {
    let impronta = crate::planner::contract_fingerprint(contratto_di_uscita)?;
    let contract_fingerprint_atteso = crate::protocollo::digest::DigestSha256::da_esadecimale(
        &impronta.to_hex(),
    )
    .map_err(|forma| {
        PlenoraError::Internal(format!(
            "fingerprint del contratto d'uscita in forma non canonica: {forma}"
        ))
    })?;
    Ok(IncaricoVerifica {
        contract_fingerprint_atteso,
        digest_atteso: digest.clone(),
        conteggi_attesi: conteggi,
        budget_memoria_governata_bytes,
    })
}

/// Il dominio del verificatore, il dialogo vero: prepara il **secondo**
/// dominio, avvia lo spawner in modalita' verificatore col terzo
/// descrittore ceduto, conclude l'accordo, manda l'`IncaricoVerifica`, e
/// conduce il resto con la stessa macchina a stati del worker.
///
/// Stesso `concesso_byte` del worker: i due domini non coesistono, quindi il
/// tetto di ciascuno e' quello del totale
/// (`isolamento.md#2-quater-topologia-chi-osserva-chi`).
///
/// Riusa `tempo_di_esecuzione` invece di un tetto dedicato: la verifica fa
/// meno lavoro e un tetto piu' stretto sarebbe probabilmente piu' corretto,
/// ma non c'e' una misura per sceglierlo, e una variabile di dispiegamento
/// non misurata e' cio' che
/// `isolamento.md#2-quinquies-il-piano-chiede-la-politica-dellhost-concede`
/// evita. Il tetto resta **imposto**, solo piu' largo.
///
/// # Errors
///
/// L'errore classificato del dialogo di verifica, o un errore di
/// preparazione del secondo dominio.
#[cfg(target_os = "linux")]
#[allow(clippy::too_many_arguments)]
fn dialoga_con_verificatore(
    dominio: &Path,
    radice: &Path,
    worker: IdentitaWorker,
    immagine: &Path,
    digest_immagine: &str,
    artefatto: &std::fs::File,
    incarico_verifica: IncaricoVerifica,
    token: CommitToken,
    tempo_di_verifica: Duration,
    concesso_byte: u64,
    annullamento_esterno: CancellationToken,
) -> Result<(DigestArtefatto, ConteggiDichiarati)> {
    let mut gerarchia = Gerarchia::nuova(dominio, radice).map_err(|difetto| {
        non_disponibile(dominio.to_string_lossy().as_ref(), &difetto.to_string())
    })?;
    let preparato = prepara_dominio(&mut gerarchia, concesso_byte, worker)?;
    // Stessa regola del worker: il «prima» dell'evidenza si prende dal
    // preparato, prima che `avvia` lo consumi.
    let evidenza = macchina::EvidenzaDaPrimaDelloSpawn::prendi(&preparato);

    let argomento_verificatore: std::ffi::OsString = super::VERSIONE_VERIFICATORE.into();
    let da_eseguire = DaEseguire {
        eseguibile: immagine,
        argomenti: std::slice::from_ref(&argomento_verificatore),
    };
    // Secondo controllo sincrono di transizione, appena prima dello spawn
    // vero: chiude la parte della finestra che precede `comando.spawn()`
    // (dentro `spawner::tenta`, attraverso `avvia` qui sotto). Non chiude
    // (non puo': nessun controllo sincrono lo puo') la parte che segue —
    // fra questa riga e la `spawn()` vera non c'e' un confine cooperativo,
    // solo la costruzione del comando e `accerta_monothread`. Quella parte
    // si chiude appena dopo, quando `avvia` e' gia' tornata.
    if annullamento_esterno.is_cancelled() {
        return Err(PlenoraError::Cancelled {
            node: "verificatore".to_owned(),
            operation: "spawn".to_owned(),
            execution_id: String::new(),
            reason: "l'esecuzione isolata e' stata cancellata prima dello spawn del verificatore"
                .to_owned(),
        });
    }
    // Il terzo descrittore: l'unico handle che il verificatore riceve
    // sull'artefatto, in sola lettura. Nessun percorso viaggia — ne' qui ne'
    // nell'incarico — e nessuna capability di pubblicazione: e' lo stesso
    // principio di `GA-5` per il worker, applicato a un processo che non ha
    // nemmeno il piano da cui la destinazione potrebbe dedursi.
    let riuscita = avvia(preparato, &da_eseguire, Some(artefatto)).map_err(|fallita| {
        let super::TransizioneFallita {
            causa,
            evidenza,
            difetto_di_pulizia,
        } = *fallita;
        eprintln!(
            "plenora: preflight del dominio del verificatore al momento del fallimento: \
             {evidenza:?}"
        );
        if let Some(residuo) = &difetto_di_pulizia {
            eprintln!("plenora: pulizia dopo lo spawn fallito (verificatore): {residuo}");
        }
        causa.con_contesto("lo spawner del verificatore non e' partito")
    })?;
    let guardia = FiglioVivo::nuovo(riuscita.figlio);
    // Terzo controllo: la cancellazione puo' arrivare fra il secondo e la
    // `spawn()`. Il verificatore esiste gia', quindi si termina e si raccoglie
    // **subito**, prima dell'handshake, con lo stesso `prova::chiudi` degli
    // altri fallimenti precoci: `conduci_isolato` non sorveglia ancora.
    if annullamento_esterno.is_cancelled() {
        let causa = PlenoraError::Cancelled {
            node: "verificatore".to_owned(),
            operation: "spawn".to_owned(),
            execution_id: String::new(),
            reason: "l'esecuzione isolata e' stata cancellata appena dopo lo spawn del \
                     verificatore"
                .to_owned(),
        };
        eprintln!(
            "plenora: verificatore pid {:?} cancellato appena nato, termino e raccolgo",
            guardia.pid()
        );
        let (_uscita, difetti_di_pulizia) = super::prova::chiudi(guardia, Some(&causa));
        for difetto in &difetti_di_pulizia {
            eprintln!(
                "plenora: pulizia del verificatore isolato (cancellato appena nato): {difetto}"
            );
        }
        // Anche qui il dominio e' nato: un OOM attribuito concorrente precede
        // la cancellazione (§10.3), il resto no.
        return Err(evidenza.rileggi_la_cancellazione("verificatore", causa));
    }
    eprintln!(
        "plenora: preflight del dominio del verificatore riuscito, pid {:?}: {:?}",
        guardia.pid(),
        riuscita.evidenza
    );

    // Stessa identita' del worker: il `commit_token` del tentativo, non uno
    // nuovo, perche' il verificatore accerta lo **stesso** tentativo.
    // `supervisore_o_raccogli` copre la stessa finestra del worker, e ogni
    // fallimento fino alla conduzione passa dall'evidenza del dominio.
    let (supervisore, guardia) =
        match supervisore_o_raccogli("verificatore isolato", digest_immagine, token, guardia) {
            Ok(coppia) => coppia,
            Err(causa) => return Err(evidenza.rileggi_il_fallimento("verificatore", causa)),
        };
    let (lettore, mut scrittore, accordo, guardia) = match concludi_handshake(
        "verificatore",
        supervisore,
        riuscita.supervisore_legge,
        riuscita.supervisore_scrive,
        guardia,
    ) {
        Ok(quattro) => quattro,
        Err(fallimento) => return Err(fallimento.rileggi(evidenza, "verificatore")),
    };

    // --- l'incarico di verifica, sullo stesso canale ------------------------
    let incarico_frame = Frame::nuovo(Corpo::IncaricoVerifica(Box::new(incarico_verifica)));
    if let Err(causa) =
        codifica(&incarico_frame).and_then(|byte| prova::scrivi(&mut scrittore, &byte))
    {
        let (_uscita, difetti_di_pulizia) = super::prova::chiudi(guardia, Some(&causa));
        for difetto in &difetti_di_pulizia {
            eprintln!("plenora: pulizia del verificatore isolato: {difetto}");
        }
        drop(scrittore);
        return Err(evidenza.rileggi_il_fallimento("verificatore", causa));
    }

    // --- il resto del dialogo, condotto dalla **stessa** macchina a stati --
    //
    // `macchina::conduci_isolato` e' generica sull'esito atteso dal ruolo
    // (`Corpo::Esito` per il worker, `Corpo::EsitoVerifica` per il
    // verificatore): `macchina::Ruolo::Verificatore` le dice quale corpo
    // accettare. Cosi' la classificazione della §10 non si duplica.
    macchina::conduci_isolato(
        macchina::Ruolo::Verificatore,
        lettore,
        accordo,
        scrittore,
        tempo_di_verifica,
        guardia,
        dominio.to_path_buf(),
        evidenza,
        annullamento_esterno,
    )
}

/// Il nucleo **testabile** della sequenza a due domini.
///
/// Apre l'artefatto in sola lettura (passo 1), conduce il dialogo di verifica
/// con `esegui_verificatore` (passi 2-4: in produzione
/// [`dialoga_con_verificatore`], nei casi una chiusura che simula ogni esito)
/// e, solo se il dialogo conferma digest e conteggi, pubblica (passo 9) con
/// l'handle **del coordinatore**. Separata da
/// [`esegui_il_verificatore_e_pubblica`] perche' non tocca cgroup ne'
/// processi: la barriera si prova senza privilegi.
///
/// L'handle e' quello del passo 1 perche' quello del verificatore muore con
/// lui. `ipc_boundary::artefatto_gia_accertato` non rifa' il framing, ma
/// `pubblicazione::copia_accertando` rimisura il file e ricalcola il digest sui
/// byte **effettivamente copiati** prima del commit point: un artefatto
/// mutato dopo la verifica resta rilevato li'
/// (`pubblicazione::tests::un_artefatto_accorciato_dopo_la_verifica_non_si_pubblica`,
/// `..._alterato_a_pari_lunghezza_non_si_pubblica`).
///
/// La risposta si confronta con cio' che l'`IncaricoVerifica` ha **chiesto**:
/// la barriera del dialogo accerta un esito pulito, non che sia quello di
/// *questo* tentativo. I due controlli sono indipendenti: uno scopre una
/// risposta incoerente con l'incarico, l'altro un file mutato.
///
/// # Errors
///
/// L'errore che `esegui_verificatore` rende — **mai** pubblicato — un
/// disaccordo fra la risposta e l'incarico, o l'errore della pubblicazione
/// stessa.
fn verifica_poi_pubblica(
    temporaneo: &Path,
    digest_atteso_dall_incarico: &DigestArtefatto,
    conteggi_attesi_dall_incarico: ConteggiDichiarati,
    esegui_verificatore: impl FnOnce(
        &std::fs::File,
        u64,
    ) -> Result<(DigestArtefatto, ConteggiDichiarati)>,
    destinazione: &Path,
    profilo: PublishProfile,
) -> Result<EsitoDellaPubblicazione> {
    // Passo 1: l'handle del coordinatore. Esplicitamente in sola lettura per
    // costruzione — `File::open` non chiede altro — perche' il coordinatore
    // non deve mai poter scrivere l'artefatto che sta per pubblicare:
    // leggere e scrivere sono due autorita' diverse, e qui ne serve una sola.
    let handle = std::fs::File::open(temporaneo).map_err(|causa| {
        PlenoraError::Internal(format!(
            "l'artefatto temporaneo non si \
             riapre per la pubblicazione: {causa}"
        ))
    })?;
    let byte_totali = handle
        .metadata()
        .map_err(|causa| {
            PlenoraError::Internal(format!(
                "l'artefatto temporaneo non si lascia interrogare: {causa}"
            ))
        })?
        .len();

    // Passi 2-4: il dialogo di verifica, nel proprio dominio. Un errore qui —
    // rifiuto, timeout, OOM, cancellazione, uscita anomala, chiusura non
    // pulita — e' propagato **senza pubblicare**: il `?` e' la barriera, e
    // non ce n'e' una parallela da tenere sincronizzata.
    let (digest_confermato, conteggi_confermati) = esegui_verificatore(&handle, byte_totali)?;

    // La risposta deve essere quella di **questo** incarico. Ne' il digest ne'
    // i conteggi entrano nel messaggio: sono funzioni del contenuto, e questo
    // progetto non li mette nei log (stessa regola di `verifica::verifica_digest`).
    if digest_confermato != *digest_atteso_dall_incarico
        || conteggi_confermati != conteggi_attesi_dall_incarico
    {
        return Err(PlenoraError::DataMapping(
            "il verificatore ha confermato un digest o dei conteggi diversi da quelli \
             richiesti nell'incarico di verifica: la risposta non corrisponde a questo \
             tentativo"
                .to_owned(),
        ));
    }

    let digest_atteso = crate::esadecimale32::Esadecimale32::da_esadecimale(
        &digest_confermato.valore,
    )
    .map_err(|forma| {
        // `FormaNonValida` non implementa `Display` — solo `Debug` —
        // ed e' comunque sicuro: le sue varianti portano lunghezze e
        // posizioni, mai il testo che ha fallito la conversione
        // (vedi il commento sul tipo, in `esadecimale32.rs`).
        PlenoraError::Internal(format!(
            "il verificatore ha confermato un digest non canonico: {forma:?}"
        ))
    })?;

    // Passo 9: la stessa autorita' di publish gia' qualificata, mai
    // reimplementata.
    let verificato = pubblicazione::ArtefattoVerificato::accertato(
        crate::ipc_boundary::artefatto_gia_accertato(handle, byte_totali),
        byte_totali,
        digest_atteso,
    );
    pubblicazione::pubblica(verificato, destinazione, profilo)
}

/// Il dominio del verificatore in produzione: prepara il dominio, conduce il
/// dialogo, e — solo attraverso [`verifica_poi_pubblica`] — pubblica.
///
/// # Errors
///
/// L'errore classificato del dialogo di verifica, o quello della
/// pubblicazione.
#[cfg(target_os = "linux")]
#[allow(clippy::too_many_arguments)]
fn esegui_il_verificatore_e_pubblica(
    dominio: &Path,
    radice: &Path,
    worker: IdentitaWorker,
    immagine: &Path,
    digest_immagine: &str,
    temporaneo: &Path,
    contratto_di_uscita: &DataContract,
    token: &CommitToken,
    digest: &DigestArtefatto,
    conteggi: ConteggiDichiarati,
    budget_memoria_governata_bytes: u64,
    concesso_byte: u64,
    tempo_di_verifica: Duration,
    destinazione: &Path,
    annullamento_esterno: CancellationToken,
) -> Result<EsitoDellaPubblicazione> {
    let incarico_verifica = incarico_verifica_per(
        contratto_di_uscita,
        digest,
        conteggi,
        budget_memoria_governata_bytes,
    )?;
    let token_del_verificatore = *token;
    verifica_poi_pubblica(
        temporaneo,
        digest,
        conteggi,
        move |artefatto, _byte_totali| {
            dialoga_con_verificatore(
                dominio,
                radice,
                worker,
                immagine,
                digest_immagine,
                artefatto,
                incarico_verifica,
                token_del_verificatore,
                tempo_di_verifica,
                concesso_byte,
                annullamento_esterno,
            )
        },
        destinazione,
        PublishProfile::Atomic,
    )
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use plenora_core::arrow::array::{Int64Array, RecordBatch, StringArray};
    use plenora_core::arrow::ipc::writer::FileWriter;
    use plenora_core::arrow::schema::{DataType, Field, Schema};
    use plenora_core::contract::arrow_schema::contract_from_arrow_schema;

    use crate::planner;

    use super::*;

    // --- identita_da: giudizio puro, senza toccare l'ambiente ---------------

    #[test]
    fn identita_da_none_e_assente() {
        assert_eq!(
            identita_da(None),
            Err(RifiutoConfigurazione::IdentitaAssente)
        );
    }

    #[test]
    fn identita_da_malformata_e_rifiutata() {
        for grezzo in ["", "1000", "1000:", ":1000", "uid:gid", "1000:1000:extra"] {
            assert_eq!(
                identita_da(Some(grezzo)),
                Err(RifiutoConfigurazione::IdentitaMalformata),
                "«{grezzo}»"
            );
        }
    }

    #[test]
    fn identita_da_valida_con_spazi_tollerati() {
        assert_eq!(identita_da(Some("1000:1000")), Ok((1000, 1000)));
        assert_eq!(identita_da(Some(" 1000 : 1000 ")), Ok((1000, 1000)));
    }

    // --- nome_del_dominio / identificativo_del_tentativo ---------------------

    #[test]
    fn il_nome_del_dominio_ha_il_prefisso_atteso_ed_e_deterministico() {
        let identificativo = [7_u8; 32];
        let nome = nome_del_dominio(&identificativo);
        assert!(nome.starts_with("plenora-isolato-"), "{nome}");
        assert_eq!(
            nome,
            nome_del_dominio(&identificativo),
            "stesso identificativo, stesso nome"
        );
    }

    #[test]
    fn due_tentativi_non_collidono() {
        let a = identificativo_del_tentativo();
        let b = identificativo_del_tentativo();
        assert_ne!(
            a, b,
            "due tentativi devono avere directory di dominio distinte"
        );
    }

    /// Il nome del dominio del verificatore ha il proprio prefisso — distinto
    /// da quello del worker — ed e' deterministico sullo stesso
    /// identificativo, sullo stesso principio di
    /// [`il_nome_del_dominio_ha_il_prefisso_atteso_ed_e_deterministico`], che
    /// prova la proprieta' solo per `nome_del_dominio` (worker): questo test
    /// la prova direttamente anche per il verificatore.
    #[test]
    fn il_nome_del_dominio_verifica_ha_il_proprio_prefisso_ed_e_deterministico() {
        let identificativo = [7_u8; 32];
        let nome = nome_del_dominio_verifica(&identificativo);
        assert!(nome.starts_with("plenora-verifica-"), "{nome}");
        assert_ne!(
            nome,
            nome_del_dominio(&identificativo),
            "worker e verificatore non condividono mai lo stesso nome, anche a parita' di \
             identificativo"
        );
        assert_eq!(
            nome,
            nome_del_dominio_verifica(&identificativo),
            "stesso identificativo, stesso nome"
        );
    }

    /// Un `commit_token` generato e' sempre nella propria forma canonica —
    /// altrimenti `token_del_tentativo` stesso lo rifiuterebbe — e due
    /// tentativi non condividono lo stesso token, sullo stesso principio di
    /// [`due_tentativi_non_collidono`].
    #[test]
    fn token_del_tentativo_riesce_ed_e_distinto_fra_due_chiamate() {
        let a = token_del_tentativo().expect("un token deve generarsi");
        let b = token_del_tentativo().expect("un secondo token deve generarsi");
        assert_ne!(a, b, "due tentativi non condividono lo stesso commit_token");
    }

    // --- supervisore_o_raccogli: la guardia non sfugge a un rifiuto --------
    //
    // `supervisore_per` puo' rifiutare fra la nascita del figlio e
    // `concludi_handshake`; un `?` nudo farebbe abortire il processo. Qui il
    // rifiuto si forza con un `digest_immagine` non canonico, su un **vero**
    // processo figlio, per ciascuno dei due punti (worker e verificatore):
    //  1. l'errore e' quello leggibile di `supervisore_per`, e il processo di
    //     prova non abortisce;
    //  2. nessuna pubblicazione: il rifiuto precede `concludi_handshake`;
    //  3. il figlio e' raccolto davvero, verificato dall'esterno con `kill -0`.
    #[cfg(target_os = "linux")]
    fn figlio_di_prova_reale() -> (std::process::Child, u32) {
        let figlio = std::process::Command::new("sleep")
            .arg("5")
            .spawn()
            .expect("spawn di un vero processo per la prova");
        let pid = figlio.id();
        (figlio, pid)
    }

    #[cfg(target_os = "linux")]
    fn pid_esiste_ancora(pid: u32) -> bool {
        // `kill -0` non manda nessun segnale: chiede solo al kernel se il pid
        // esiste ancora, come processo vivo o come zombie non raccolto —
        // esattamente cio' che questa prova deve escludere. Nessuna nuova
        // dipendenza: e' l'utility di sistema, non una crate.
        std::process::Command::new("kill")
            .args(["-0", &pid.to_string()])
            .status()
            .expect("kill -0 deve potersi eseguire")
            .success()
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn supervisore_o_raccogli_rifiuta_con_errore_leggibile_e_raccoglie_il_worker() {
        let (figlio, pid) = figlio_di_prova_reale();
        let guardia = FiglioVivo::nuovo(figlio);
        let token = token_del_tentativo().expect("token del tentativo");

        let esito =
            supervisore_o_raccogli("worker isolato", "non-e-un-digest-canonico", token, guardia);

        let causa = esito.expect_err("un digest non canonico deve far rifiutare supervisore_per");
        let testo = causa.to_string();
        // `supervisore_per` valida l'ambiente PRIMA del digest: con
        // `proj-backend` compilato rifiuta sempre sul resolver PROJ
        // (`protocollo/descrizione.rs::ambiente`, `InvalidConfiguration`),
        // senza la causa e' il digest non canonico (`IsolationUnavailable`).
        // Nessuna delle due varianti porta una causa strutturata, quindi si
        // confronta il testo esatto.
        if cfg!(feature = "proj-backend") {
            assert_eq!(
                causa.category(),
                plenora_core::ErrorCategory::InvalidConfiguration
            );
            assert!(
                testo.contains("resolver") && testo.contains("proj"),
                "con proj-backend l'errore deve restituire la causa leggibile del rifiuto \
                 dell'ambiente PROJ, non un abort ne' un errore generico: {testo}"
            );
        } else {
            assert_eq!(
                causa.category(),
                plenora_core::ErrorCategory::IsolationUnavailable
            );
            assert!(
                testo.contains("digest") && testo.contains("canonico"),
                "senza proj-backend l'errore deve restituire la causa leggibile del digest \
                 non canonico, non un abort ne' un errore generico: {testo}"
            );
        }

        // Raccolto per davvero: il pid non esiste piu', ne' vivo ne' zombie.
        // Nessun ciclo d'attesa qui — `chiudi` dentro `supervisore_o_raccogli`
        // e' gia' tornato quando `esito` e' pronto, quindi la raccolta e' gia'
        // conclusa a questo punto, non in corso.
        assert!(
            !pid_esiste_ancora(pid),
            "il worker deve essere stato raccolto (non residuo, non zombie): pid {pid}"
        );
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn supervisore_o_raccogli_rifiuta_con_errore_leggibile_e_raccoglie_il_verificatore() {
        let (figlio, pid) = figlio_di_prova_reale();
        let guardia = FiglioVivo::nuovo(figlio);
        let token = token_del_tentativo().expect("token del tentativo");

        let esito = supervisore_o_raccogli(
            "verificatore isolato",
            "non-e-un-digest-canonico",
            token,
            guardia,
        );

        let causa = esito.expect_err("un digest non canonico deve far rifiutare supervisore_per");
        let testo = causa.to_string();
        // Stessa distinzione del test simmetrico sopra: senza causa
        // strutturata separata dal testo per queste varianti, l'asserzione sul
        // testo esatto e' la verifica di conservazione della causa.
        if cfg!(feature = "proj-backend") {
            assert_eq!(
                causa.category(),
                plenora_core::ErrorCategory::InvalidConfiguration
            );
            assert!(
                testo.contains("resolver") && testo.contains("proj"),
                "con proj-backend l'errore deve restituire la causa leggibile del rifiuto \
                 dell'ambiente PROJ, non un abort ne' un errore generico: {testo}"
            );
        } else {
            assert_eq!(
                causa.category(),
                plenora_core::ErrorCategory::IsolationUnavailable
            );
            assert!(
                testo.contains("digest") && testo.contains("canonico"),
                "senza proj-backend l'errore deve restituire la causa leggibile del digest \
                 non canonico, non un abort ne' un errore generico: {testo}"
            );
        }
        assert!(
            !pid_esiste_ancora(pid),
            "il verificatore deve essere stato raccolto (non residuo, non zombie): pid {pid}"
        );
    }

    // --- percorso_in_testo: giudizio puro sulla forma del percorso ----------

    #[test]
    fn percorso_in_testo_accetta_un_percorso_utf8() {
        let percorso = Path::new("/tmp/artefatto.arrow");
        assert_eq!(
            percorso_in_testo(percorso).expect("un percorso UTF-8 deve convertirsi"),
            "/tmp/artefatto.arrow"
        );
    }

    /// Il protocollo porta stringhe: un percorso che non e' UTF-8 — possibile
    /// su Unix, dove i percorsi sono byte arbitrari — si rifiuta invece di
    /// perdere o storpiare silenziosamente i byte non rappresentabili.
    #[test]
    #[cfg(target_os = "linux")]
    fn percorso_in_testo_rifiuta_un_percorso_non_utf8() {
        use std::os::unix::ffi::OsStrExt as _;
        let byte_non_utf8 = [0x66_u8, 0x6f, 0xff, 0x6f]; // "fo\xFFo": 0xFF non e' UTF-8 valido
        let percorso = Path::new(std::ffi::OsStr::from_bytes(&byte_non_utf8));
        let motivo =
            percorso_in_testo(percorso).expect_err("un percorso non UTF-8 non e' rappresentabile");
        assert!(
            motivo.to_string().contains("non e' UTF-8"),
            "il rifiuto deve nominare la ragione: {motivo}"
        );
    }

    // --- il piano isolato di prova, e l'ingresso che gli corrisponde --------

    fn piano_isolato() -> String {
        serde_json::json!({
            "schema_version": 6,
            "inputs": ["citta"],
            "limits": { "max_domain_memory_bytes": 1_073_741_824_u64 },
            "nodes": [
                {
                    "id": "grandi",
                    "op": "table.filter",
                    "in": ["citta"],
                    "config": { "column": "abitanti", "operator": ">", "value": 300_000 }
                }
            ],
            "output": "grandi"
        })
        .to_string()
    }

    fn scrivi_ingresso(dove: &Path) {
        let schema = Arc::new(Schema::new(vec![
            Field::new("nome", DataType::Utf8, false),
            Field::new("abitanti", DataType::Int64, false),
        ]));
        let batch = RecordBatch::try_new(
            Arc::clone(&schema),
            vec![
                Arc::new(StringArray::from(vec!["alfa", "beta"])),
                Arc::new(Int64Array::from(vec![100_000, 500_000])),
            ],
        )
        .expect("il batch di prova deve costruirsi");
        let file = std::fs::File::create(dove).expect("l'ingresso di prova deve crearsi");
        let mut scrittore = FileWriter::try_new(file, &schema).expect("writer IPC di prova");
        scrittore.write(&batch).expect("il batch deve scriversi");
        scrittore.finish().expect("il writer deve chiudersi");
    }

    fn grafo_isolato_di_prova(ingresso: &Path) -> ValidatedGraph {
        let schema = crate::ipc_boundary::header_schema(ingresso, &crate::IpcLimits::default())
            .expect("lo schema dell'ingresso di prova deve leggersi");
        let contratto = contract_from_arrow_schema(schema, crate::risolutore::risolvi)
            .expect("il contratto dell'ingresso di prova deve derivarsi");
        planner::validate(&piano_isolato(), &[("citta".to_owned(), contratto)])
            .expect("il piano isolato di prova deve validare")
    }

    // --- incarico_per: giudizio puro sul grafo REALE gia' validato ----------
    //
    // Nessun dominio, nessuno spawn: solo il grafo e i percorsi, esattamente
    // cio' che `incarico_per` riceve davvero da `esegui_il_worker` prima di
    // toccare un cgroup.

    #[test]
    fn incarico_per_riesce_con_un_ingresso_dichiarato() {
        let ingresso_dir = tempfile::tempdir().expect("cartella dell'ingresso");
        let ingresso = ingresso_dir.path().join("citta.arrow");
        scrivi_ingresso(&ingresso);
        let graph = grafo_isolato_di_prova(&ingresso);
        let temporaneo = std::path::PathBuf::from("/tmp/plenora-artefatto-di-prova.arrow");

        let (incarico, contratto_di_uscita) = incarico_per(
            &graph,
            &[("citta".to_owned(), ingresso.clone())],
            &temporaneo,
        )
        .expect("un ingresso dichiarato dal piano deve produrre un incarico");

        assert_eq!(incarico.ingressi.len(), 1);
        assert_eq!(incarico.ingressi[0].nome, "citta");
        assert_eq!(
            incarico.ingressi[0].percorso,
            ingresso.to_str().expect("il percorso di prova e' UTF-8")
        );
        assert_eq!(
            incarico.artefatto_temporaneo,
            temporaneo.to_str().expect("il percorso di prova e' UTF-8")
        );
        // `DataContract` non implementa `PartialEq` (e' di `plenora-core`):
        // il confronto che conta davvero, a valle, e' sul fingerprint — lo
        // stesso che il verificatore ricontrolla — non sull'identita' di
        // struct per struct.
        let atteso = crate::planner::contract_fingerprint(
            graph
                .output_contract()
                .expect("il grafo di prova ha un contratto d'uscita"),
        )
        .expect("il fingerprint del contratto atteso deve calcolarsi");
        let ottenuto = crate::planner::contract_fingerprint(&contratto_di_uscita)
            .expect("il fingerprint del contratto reso deve calcolarsi");
        assert_eq!(
            ottenuto.to_hex(),
            atteso.to_hex(),
            "il contratto reso deve essere quello d'uscita del grafo, non uno ricostruito"
        );
    }

    /// Un ingresso che il piano non dichiara e' un piano invalido rispetto a
    /// **questi** ingressi — non un difetto interno: il chiamante ha passato
    /// una coppia nome/percorso che il grafo non conosce.
    #[test]
    fn incarico_per_rifiuta_un_ingresso_non_dichiarato() {
        let ingresso_dir = tempfile::tempdir().expect("cartella dell'ingresso");
        let ingresso = ingresso_dir.path().join("citta.arrow");
        scrivi_ingresso(&ingresso);
        let graph = grafo_isolato_di_prova(&ingresso);
        let temporaneo = std::path::PathBuf::from("/tmp/plenora-artefatto-di-prova.arrow");

        let errore = incarico_per(&graph, &[("sconosciuto".to_owned(), ingresso)], &temporaneo)
            .expect_err("un ingresso non dichiarato dal piano deve rifiutarsi");
        assert_eq!(
            errore.category(),
            plenora_core::error::ErrorCategory::InvalidPlan
        );
        assert!(
            errore.to_string().contains("non dichiarato dal piano"),
            "{errore}"
        );
    }

    // --- incarico_verifica_per: giudizio puro sul contratto d'uscita --------

    #[test]
    fn incarico_verifica_per_riesce_e_porta_i_valori_passati() {
        let ingresso_dir = tempfile::tempdir().expect("cartella dell'ingresso");
        let ingresso = ingresso_dir.path().join("citta.arrow");
        scrivi_ingresso(&ingresso);
        let graph = grafo_isolato_di_prova(&ingresso);
        let contratto_di_uscita = graph
            .output_contract()
            .expect("il grafo di prova ha un contratto d'uscita")
            .clone();
        let (_dir, _temporaneo, byte) = artefatto_di_prova();
        let digest = digest_reale(&byte);
        let conteggi = conteggi_di_prova();

        let incarico_verifica =
            incarico_verifica_per(&contratto_di_uscita, &digest, conteggi, 536_870_912)
                .expect("un contratto d'uscita valido deve produrre un incarico di verifica");

        assert_eq!(incarico_verifica.digest_atteso, digest);
        assert_eq!(incarico_verifica.conteggi_attesi, conteggi);
        assert_eq!(
            incarico_verifica.budget_memoria_governata_bytes,
            536_870_912
        );
        let impronta_vera = crate::planner::contract_fingerprint(&contratto_di_uscita)
            .expect("il fingerprint del contratto di prova deve calcolarsi");
        assert_eq!(
            incarico_verifica
                .contract_fingerprint_atteso
                .in_esadecimale(),
            impronta_vera.to_hex(),
            "il fingerprint trasportato deve essere quello vero del contratto, non uno segnaposto"
        );
    }

    // --- esegui_isolato, senza privilegi ne' una VM vera ---------------------
    //
    // Un tempdir qualunque non sta sotto un montaggio `cgroup2`: il preflight
    // (`prepara_dominio` -> `accerta_perimetro` -> `montaggio`) fallisce prima
    // dello spawner, come per un dispiegamento mal configurato.
    //
    // E' l'unico caso che tocca le tre variabili d'ambiente di configurazione,
    // quindi non c'e' corsa fra thread di test paralleli.
    #[cfg(target_os = "linux")]
    #[test]
    fn esegui_isolato_senza_cgroup2_reale_fallisce_ripulisce_e_non_scrive_output() {
        let radice = tempfile::tempdir().expect("radice fittizia");
        let ingresso_dir = tempfile::tempdir().expect("cartella dell'ingresso");
        let ingresso = ingresso_dir.path().join("citta.arrow");
        scrivi_ingresso(&ingresso);
        let graph = grafo_isolato_di_prova(&ingresso);

        let destinazione_dir = tempfile::tempdir().expect("cartella di destinazione");
        let destinazione = destinazione_dir.path().join("output.arrow");

        let concessione = ConcessioneDominio {
            richiesto_byte: 1_073_741_824,
            concesso_byte: 1_073_741_824,
        };

        std::env::set_var(VARIABILE_RADICE_CGROUP, radice.path());
        std::env::set_var(VARIABILE_IDENTITA_WORKER, "0:0");
        std::env::set_var(VARIABILE_TEMPO_DI_ESECUZIONE, "30");

        let esito = esegui_isolato(
            &graph,
            &[("citta".to_owned(), ingresso)],
            &destinazione,
            concessione,
            CancellationToken::new(),
        );

        std::env::remove_var(VARIABILE_RADICE_CGROUP);
        std::env::remove_var(VARIABILE_IDENTITA_WORKER);
        std::env::remove_var(VARIABILE_TEMPO_DI_ESECUZIONE);

        let errore = esito.expect_err(
            "senza un cgroup2 reale sotto la radice il preflight deve fallire — mai un'esecuzione",
        );
        // `non_disponibile` tagga esplicitamente `ErrorPhase::Prepare`
        // (isolamento.rs), quindi l'errore arriva qui avvolto in `Tagged`:
        // la categoria, delegata alla sorgente, e' il modo giusto per
        // giudicarlo invece di un match sulla variante grezza.
        assert_eq!(
            errore.category(),
            plenora_core::error::ErrorCategory::IsolationUnavailable,
            "atteso isolation_unavailable, ottenuto {errore:?}"
        );

        let residui: Vec<_> = std::fs::read_dir(radice.path())
            .expect("la radice deve restare leggibile dopo il tentativo")
            .collect();
        assert!(
            residui.is_empty(),
            "la directory di dominio non si e' ripulita dopo il fallimento: {residui:?}"
        );
        assert!(
            !destinazione.try_exists().expect("stat"),
            "nessun output deve comparire quando il preflight isolato fallisce"
        );
    }

    // --- verifica_poi_pubblica: la barriera, senza cgroup ne' processi ------
    //
    // La chiusura non parla con nessun processo: simula ciascun esito della
    // matrice, e la barriera fra verifica e pubblicazione si prova senza
    // privilegi.

    fn digest_reale(byte: &[u8]) -> DigestArtefatto {
        let mut hasher = Sha256::new();
        hasher.update(byte);
        let esito: [u8; 32] = hasher.finalize().into();
        let mut valore = String::with_capacity(64);
        for grezzo in esito {
            use std::fmt::Write as _;
            let _ = write!(valore, "{grezzo:02x}");
        }
        DigestArtefatto {
            algoritmo: crate::protocollo::digest::ALGORITMO_DIGEST.to_owned(),
            valore,
        }
    }

    fn conteggi_di_prova() -> ConteggiDichiarati {
        ConteggiDichiarati { righe: 3, batch: 1 }
    }

    /// Scrive un artefatto di prova qualunque — non serve che sia Arrow IPC
    /// valido: `verifica_poi_pubblica` non lo parsa mai, ed e' precisamente
    /// il punto (la §2-ter lo vuole fuori dal coordinatore).
    fn artefatto_di_prova() -> (tempfile::TempDir, std::path::PathBuf, Vec<u8>) {
        let dir = tempfile::tempdir().expect("tempdir");
        let percorso = dir.path().join("artefatto.arrow");
        let byte = b"contenuto di prova, non e' Arrow IPC e non deve esserlo".to_vec();
        std::fs::write(&percorso, &byte).expect("scrittura dell'artefatto di prova");
        (dir, percorso, byte)
    }

    /// **Successo genuino**: la chiusura conferma il digest vero, e la
    /// pubblicazione avviene con lo stesso contenuto.
    #[test]
    fn una_verifica_confermata_pubblica_lo_stesso_contenuto() {
        let (_dir, temporaneo, byte) = artefatto_di_prova();
        let digest_vero = digest_reale(&byte);
        let destinazione_dir = tempfile::tempdir().expect("destinazione");
        let destinazione = destinazione_dir.path().join("output.arrow");

        let atteso = digest_vero.clone();
        let esito = verifica_poi_pubblica(
            &temporaneo,
            &atteso,
            conteggi_di_prova(),
            move |_artefatto, _byte_totali| Ok((digest_vero, conteggi_di_prova())),
            &destinazione,
            PublishProfile::Atomic,
        );

        assert!(
            esito.is_ok(),
            "la verifica confermata deve pubblicare: {esito:?}"
        );
        assert!(
            destinazione.try_exists().expect("stat"),
            "la destinazione deve comparire"
        );
        assert_eq!(
            std::fs::read(&destinazione).expect("lettura della destinazione"),
            byte,
            "il contenuto pubblicato deve essere quello verificato"
        );
    }

    /// **Verifica riuscita ma processo uscito male dopo**: e' un rifiuto come
    /// gli altri.
    ///
    /// `macchina::conduci_isolato` non concede `DaVerificare` finche'
    /// quiescenza, EOF e uscita pulita non sono tutti osservati
    /// (`isolamento.md#31-supervisore`): l'uscita anomala produce un `Err`, e
    /// niente si pubblica.
    #[test]
    fn un_verificatore_uscito_male_dopo_il_successo_non_pubblica() {
        let (_dir, temporaneo, byte) = artefatto_di_prova();
        let atteso = digest_reale(&byte);
        let destinazione_dir = tempfile::tempdir().expect("destinazione");
        let destinazione = destinazione_dir.path().join("output.arrow");

        let esito = verifica_poi_pubblica(
            &temporaneo,
            &atteso,
            conteggi_di_prova(),
            |_artefatto, _byte_totali| {
                Err(PlenoraError::Internal(
                    "il verificatore isolato e' terminato in modo ambiguo".to_owned(),
                ))
            },
            &destinazione,
            PublishProfile::Atomic,
        );

        assert!(
            esito.is_err(),
            "una terminazione anomala non deve pubblicare"
        );
        assert!(
            !destinazione.try_exists().expect("stat"),
            "nessun output deve comparire"
        );
    }

    /// **Timeout del verificatore**: nessuna pubblicazione.
    #[test]
    fn un_timeout_del_verificatore_non_pubblica() {
        let (_dir, temporaneo, byte) = artefatto_di_prova();
        let atteso = digest_reale(&byte);
        let destinazione_dir = tempfile::tempdir().expect("destinazione");
        let destinazione = destinazione_dir.path().join("output.arrow");

        let esito = verifica_poi_pubblica(
            &temporaneo,
            &atteso,
            conteggi_di_prova(),
            |_artefatto, _byte_totali| {
                Err(PlenoraError::Timeout(
                    "il verificatore isolato non ha concluso entro 30 secondi".to_owned(),
                ))
            },
            &destinazione,
            PublishProfile::Atomic,
        );

        let errore = esito.expect_err("un timeout non deve pubblicare");
        assert_eq!(
            errore.category(),
            plenora_core::error::ErrorCategory::Timeout
        );
        assert!(
            !destinazione.try_exists().expect("stat"),
            "nessun output deve comparire"
        );
    }

    /// **Cancellazione durante la verifica**: nessuna pubblicazione.
    #[test]
    fn una_cancellazione_durante_la_verifica_non_pubblica() {
        let (_dir, temporaneo, byte) = artefatto_di_prova();
        let atteso = digest_reale(&byte);
        let destinazione_dir = tempfile::tempdir().expect("destinazione");
        let destinazione = destinazione_dir.path().join("output.arrow");

        let esito = verifica_poi_pubblica(
            &temporaneo,
            &atteso,
            conteggi_di_prova(),
            |_artefatto, _byte_totali| {
                Err(PlenoraError::Cancelled {
                    node: "verificatore".to_owned(),
                    operation: "dominio isolato".to_owned(),
                    execution_id: String::new(),
                    reason: "l'esecuzione isolata del verificatore e' stata cancellata".to_owned(),
                })
            },
            &destinazione,
            PublishProfile::Atomic,
        );

        let errore = esito.expect_err("una cancellazione non deve pubblicare");
        assert_eq!(
            errore.category(),
            plenora_core::error::ErrorCategory::Cancelled
        );
        assert!(
            !destinazione.try_exists().expect("stat"),
            "nessun output deve comparire"
        );
    }

    /// **Risposta di verifica incoerente**: il digest confermato non e' quello
    /// vero dell'artefatto.
    ///
    /// `pubblicazione::copia_accertando` lo scopre ricalcolando il digest sui
    /// byte copiati, prima del commit point: lo stesso presidio di
    /// `pubblicazione::tests::un_artefatto_alterato_a_pari_lunghezza_non_si_pubblica`,
    /// qui attraverso la sequenza a due domini.
    #[test]
    fn una_risposta_di_verifica_incoerente_non_pubblica() {
        let (_dir, temporaneo, byte) = artefatto_di_prova();
        // Un digest canonico ma **sbagliato**. L'atteso passato a
        // `verifica_poi_pubblica` e' lo stesso digest, cosi' il caso isola il
        // presidio di `copia_accertando`; il confronto con l'incarico lo prova
        // `una_risposta_verificatore_diversa_dall_incarico_non_pubblica`.
        let digest_sbagliato = digest_reale(b"tutt'altro contenuto");
        assert_ne!(digest_sbagliato.valore, digest_reale(&byte).valore);
        let atteso = digest_sbagliato.clone();
        let destinazione_dir = tempfile::tempdir().expect("destinazione");
        let destinazione = destinazione_dir.path().join("output.arrow");

        let esito = verifica_poi_pubblica(
            &temporaneo,
            &atteso,
            conteggi_di_prova(),
            move |_artefatto, _byte_totali| Ok((digest_sbagliato, conteggi_di_prova())),
            &destinazione,
            PublishProfile::Atomic,
        );

        assert!(
            esito.is_err(),
            "un digest confermato ma sbagliato non deve pubblicare"
        );
        assert!(
            !destinazione.try_exists().expect("stat"),
            "nessun output deve comparire"
        );
    }

    /// **Risposta positiva ma diversa da cio' che l'incarico aveva
    /// chiesto**.
    ///
    /// Digest e conteggi sono coerenti con l'artefatto reale, e
    /// `copia_accertando` da solo li accetterebbe, ma i conteggi non sono
    /// quelli richiesti: la barriera del dialogo non lo vede, il confronto con
    /// l'incarico si'.
    #[test]
    fn una_risposta_verificatore_diversa_dall_incarico_non_pubblica() {
        let (_dir, temporaneo, byte) = artefatto_di_prova();
        let digest_vero = digest_reale(&byte);
        let digest_per_la_chiusura = digest_vero.clone();
        let destinazione_dir = tempfile::tempdir().expect("destinazione");
        let destinazione = destinazione_dir.path().join("output.arrow");

        // L'incarico chiede conteggi diversi da quelli che la chiusura
        // confermera'.
        let conteggi_richiesti_dall_incarico = ConteggiDichiarati {
            righe: conteggi_di_prova().righe + 1,
            batch: conteggi_di_prova().batch,
        };

        let esito = verifica_poi_pubblica(
            &temporaneo,
            &digest_vero,
            conteggi_richiesti_dall_incarico,
            move |_artefatto, _byte_totali| Ok((digest_per_la_chiusura, conteggi_di_prova())),
            &destinazione,
            PublishProfile::Atomic,
        );

        let errore = esito
            .expect_err("una risposta positiva ma incoerente con l'incarico non deve pubblicare");
        assert_eq!(
            errore.category(),
            plenora_core::error::ErrorCategory::DataMapping,
            "il rifiuto deve venire dal confronto con l'incarico"
        );
        assert!(
            !destinazione.try_exists().expect("stat"),
            "nessun output deve comparire"
        );
    }

    /// **L'artefatto cambia fra la fine della verifica e la copia**: la
    /// chiusura simula esattamente quella finestra, mutando il file dopo
    /// aver "confermato" il digest dell'originale. La stessa barriera di
    /// `copia_accertando` — che rimisura la lunghezza **e** ricalcola il
    /// digest sui byte copiati — deve fermare la pubblicazione anche qui,
    /// attraverso la sequenza a due domini e non solo nel passo 9 isolato.
    #[test]
    fn un_artefatto_modificato_fra_verifica_e_copia_non_pubblica() {
        let (_dir, temporaneo, byte_originali) = artefatto_di_prova();
        let digest_dell_originale = digest_reale(&byte_originali);
        let atteso = digest_dell_originale.clone();
        let destinazione_dir = tempfile::tempdir().expect("destinazione");
        let destinazione = destinazione_dir.path().join("output.arrow");
        let percorso_per_la_chiusura = temporaneo.clone();

        let esito = verifica_poi_pubblica(
            &temporaneo,
            &atteso,
            conteggi_di_prova(),
            move |_artefatto, _byte_totali| {
                // La "verifica" e' avvenuta sul contenuto originale — il
                // digest confermato e' quello vero, all'apertura. Poi,
                // dentro la stessa finestra che la sequenza reale lascia
                // aperta, qualcuno riscrive il file.
                std::fs::write(
                    &percorso_per_la_chiusura,
                    b"contenuto cambiato dopo la verifica",
                )
                .expect("riscrittura dell'artefatto");
                Ok((digest_dell_originale, conteggi_di_prova()))
            },
            &destinazione,
            PublishProfile::Atomic,
        );

        assert!(
            esito.is_err(),
            "un artefatto modificato dopo la verifica non deve pubblicare"
        );
        assert!(
            !destinazione.try_exists().expect("stat"),
            "nessun output deve comparire"
        );
    }
}
