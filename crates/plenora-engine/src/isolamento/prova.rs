//! Far percorrere a un worker **reale** la sequenza intera, e riferirne.
//!
//! # Che cosa prova, e che cosa no
//!
//! Prova il **cablaggio**: che l'immagine indicata riconosca la modalita',
//! erediti i due descrittori, si dichiari, concluda l'accordo, riceva
//! l'incarico, ne rivalidi il piano, esegua, scriva l'artefatto, mandi il
//! progresso, dichiari l'esito, chiuda il canale e sia raccolta. E che
//! l'artefatto ci sia davvero: il referto porta l'esito **riverificato** con i
//! passi da 3 a 8-bis, non la sola affermazione del worker.
//!
//! **Non prova «sotto limite».** Qui non c'e' ne' lo spawner ne' un dominio
//! `cgroup2`: il worker nasce da questo processo e vive con la memoria che il
//! sistema gli concede. Che esegua *sotto* `memory.max` e' un'altra
//! affermazione, e la si fa attraversando spawner e dominio vero sulla VM.
//!
//! # Le due immagini, e che cosa le distingue davvero
//!
//! **Nessuna delle due contiene `internals`**, e va detto perche' ci si potrebbe
//! aspettare il contrario: se l'harness fosse un caso di integrazione, cargo
//! unificherebbe le feature fra dipendenze normali e di sviluppo e compilerebbe
//! con `internals` anche il binario sotto prova. Essendo un binario a se',
//! compilato da un'invocazione sua, quella unificazione non avviene.
//!
//! Cio' che le distingue e' la **provenienza**:
//!
//! - [`Immagine::DiIterazione`] sta nel target condiviso dell'harness, non e'
//!   fissata da un digest, e nulla impedisce a una compilazione successiva di
//!   sostituirla. Serve a lavorare, e **non qualifica**;
//! - [`Immagine::DiProduzione`] e' un percorso **dato da fuori**: compilata a
//!   parte, in un target suo, e fissata da uno SHA-256 che chi la fornisce
//!   verifica prima e dopo. E' l'unica che qualifica.
//!
//! # Il figlio, e il tempo
//!
//! Il processo entra nella **guardia lineare** nella stessa espressione che lo
//! crea, e ne esce da una porta sola: qualunque cammino — compreso quello di un
//! errore a meta' handshake — passa da `chiudi`. Un `Child` nudo non basta: il
//! suo `Drop` non termina niente, e un worker rotto sopravviverebbe all'harness
//! che lo ha avviato.
//!
//! `chiudi` ha **due tempi**, e l'ordine conta: prima l'attesa, che lascia
//! finire da se' chi sta gia' finendo; poi, solo allo scadere, la chiusura, che
//! segnala. Terminare per primo metterebbe `Segnale(9)` nel referto di un
//! percorso riuscito.
//!
//! Ogni attesa ha un tetto. Le letture passano da una sorgente fermabile che un
//! guardiano ferma alla scadenza; la raccolta e' limitata dalla porta stessa. Un
//! worker che non parla fa fallire il percorso, non lo appende.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

use plenora_core::arrow::array::{Int64Array, RecordBatch, StringArray};
use plenora_core::arrow::ipc::writer::FileWriter;
use plenora_core::arrow::schema::{DataType, Field, Schema};
use plenora_core::contract::arrow_schema::contract_from_arrow_schema;
use plenora_core::contract::DataContract;
use plenora_core::error::PlenoraError;
use sha2::{Digest as _, Sha256};

use crate::commit_token::CommitToken;
use crate::esadecimale32::Esadecimale32;
use crate::ipc_boundary::{self, IpcLimits};
use crate::planner;
use crate::protocollo::codifica::codifica;
use crate::protocollo::descrizione;
use crate::protocollo::digest::DigestSha256;
use crate::protocollo::handshake::{AtteseSupervisore, SupervisoreInAttesa};
use crate::protocollo::lettore::leggi_frame;
use crate::protocollo::messaggi::{
    ConteggiDichiarati, Corpo, DescrittoreIngresso, DigestArtefatto, EsitoWorkerSulFilo,
    FormatoIngresso, Frame, Incarico,
};
use crate::verifica::{verifica_artefatto, AtteseVerifica};

use super::figlio::{Chiusura, FiglioVivo, OrologioDiSistema, Uscita};

use super::sorgente::{
    interruttore, rendi_non_bloccante, Freno, Interruttore, SorgenteTerminabile, PASSO_DI_ATTESA,
};
use super::{canale, non_disponibile, Result};

/// Il token del tentativo, fisso: qui non c'e' nessuno che ne assegni uno.
const TOKEN: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";

/// Quanto si concede al worker per dire tutto quello che ha da dire.
///
/// # Perche' un tetto, e perche' questo
///
/// Perche' senza, un worker che non parla non fa fallire il percorso: lo
/// **appende**. In una campagna e' peggio di un rosso — un rosso lo si legge, e
/// una qualificazione ferma la si scopre il giorno dopo.
///
/// Trenta secondi sono due ordini di grandezza sopra il percorso intero su una
/// macchina scarica: qui si esegue un piano di quattro righe. Non e' una misura
/// di prestazione, e' la soglia oltre la quale «lento» e «fermo» non si
/// distinguono piu'.
const TETTO_DELLA_PAROLA: Duration = Duration::from_secs(30);

/// Quanto si aspetta che il figlio finisca **da se'**, prima di segnalargli
/// qualcosa.
///
/// # Perche' esiste
///
/// Perche' sul cammino riuscito il worker sta gia' uscendo: ha chiuso il canale
/// e sta finendo. Fra quel momento e la registrazione della sua uscita c'e' una
/// finestra che non e' un guasto — e' il tempo che ci mette — e terminare li'
/// vorrebbe dire uccidere un processo per aver tardato un millisecondo. Il
/// referto direbbe `Segnale(9)` su un percorso perfettamente riuscito.
///
/// Due secondi sono molto piu' del necessario per un processo che ha gia' chiuso
/// i propri descrittori, e molto meno del tetto della parola: chi tardasse
/// davvero lo si scopre comunque.
const CORTESIA_PRIMA_DEL_SEGNALE: Duration = Duration::from_secs(2);

/// Quanto si concede alla raccolta **dopo** il segnale.
///
/// Separato dagli altri perche' misura un'altra cosa: non quanto ci mette a
/// parlare, ne' quanto ci mette a finire da se', ma quanto ci mette a finire
/// dopo che gli si e' detto di smettere. Un processo che non si raccoglie entro
/// questo tempo e' un difetto di pulizia, e come tale viene riportato.
const TETTO_DELLA_RACCOLTA: Duration = Duration::from_secs(10);

/// Ogni quanto il guardiano riguarda l'orologio.
const PASSO_DEL_GUARDIANO: Duration = Duration::from_millis(20);

/// Ogni quanto si riguarda se il figlio ha finito.
///
/// Lo stesso per la cortesia e per la raccolta dopo il segnale: sono due attese
/// diverse, ma la granularita' con cui si guarda e' la stessa cosa, e due numeri
/// per la stessa cosa sono due numeri che divergono.
const PASSO_DELLA_RACCOLTA: Duration = Duration::from_millis(10);

/// Quale immagine si sta facendo percorrere.
#[derive(Debug, Clone)]
pub enum Immagine {
    /// L'immagine che si usa lavorando: nel target condiviso, non fissata.
    DiIterazione(PathBuf),
    /// L'immagine da qualificare: compilata a parte, in un target suo, e fissata
    /// da un digest che chi la fornisce verifica prima e dopo.
    DiProduzione(PathBuf),
}

impl Immagine {
    /// Il percorso, qualunque delle due sia.
    fn percorso(&self) -> &Path {
        match self {
            Self::DiIterazione(dove) | Self::DiProduzione(dove) => dove,
        }
    }

    /// Come il referto la nomina.
    ///
    /// Due parole stabili, non una descrizione: uno script le confronta.
    const fn etichetta(&self) -> &'static str {
        match self {
            Self::DiIterazione(_) => "iterazione",
            Self::DiProduzione(_) => "produzione",
        }
    }
}

/// Che cosa il worker ha dichiarato di se'.
#[derive(Debug, PartialEq, Eq)]
pub enum EsitoOsservato {
    /// Ha finito, con questo digest e questi conteggi.
    ///
    /// Entrambi si conservano: sono cio' che i passi 5-bis e 8 confrontano con
    /// l'artefatto, e scartarli renderebbe l'esito una parola.
    Successo {
        /// Il digest che il worker dichiara dell'artefatto.
        digest: DigestArtefatto,
        /// Righe e batch che il worker dichiara di aver scritto.
        conteggi: ConteggiDichiarati,
    },
    /// Ha fallito, e ha detto perche'.
    Errore(String),
    /// E' andato in panico, e ne ha dichiarato la sola forma.
    Panico(String),
}

/// Come il processo e' finito, per chi guarda il referto da fuori.
///
/// # Perche' un secondo enum e non quello di `figlio`
///
/// Perche' quello e' `pub(super)`: i tipi dell'isolamento non escono dal crate,
/// e renderli pubblici per un referto allargherebbe per sempre la superficie
/// del motore. Qui ne esce **una copia con tre varianti**, e la conversione e'
/// un `match` esaustivo: una variante nuova di la' non compila finche' non le si
/// dice dove va.
#[derive(Debug, PartialEq, Eq)]
pub enum FineDelProcesso {
    /// Uscito da se', con questo codice.
    Codice(i32),
    /// Fermato da un segnale.
    Segnale(i32),
    /// Il sistema non riporta ne' l'uno ne' l'altro: un'osservazione mancata,
    /// non un codice zero.
    NonRappresentabile,
}

impl FineDelProcesso {
    /// La fine, da come l'ha vista la guardia.
    pub(super) const fn da(uscita: Uscita) -> Self {
        match uscita {
            Uscita::Codice(codice) => Self::Codice(codice),
            Uscita::Segnale(segnale) => Self::Segnale(segnale),
            Uscita::NonRappresentabile => Self::NonRappresentabile,
        }
    }
}

/// Che cosa la riverifica dell'artefatto ha detto.
///
/// # Perche' un esito e non un booleano
///
/// Perche' «non verificato» ha due ragioni diverse — il worker non ha
/// dichiarato un successo, oppure l'ha dichiarato e l'artefatto non regge — e
/// confonderle direbbe che un errore atteso e un artefatto inventato sono la
/// stessa cosa.
#[derive(Debug, PartialEq, Eq)]
pub enum Artefatto {
    /// C'e', e supera i passi da 3 a 8-bis.
    Verificato,
    /// Il worker non ha dichiarato un successo: non c'e' niente da verificare.
    NonDichiarato,
    /// Non regge, e questo e' il motivo.
    Respinto(String),
}

/// Cio' che il percorso ha osservato.
///
/// # Perche' un referto e non delle asserzioni qui dentro
///
/// Perche' l'harness **osserva**, e chi giudica e' chi lo invoca: le due
/// immagini hanno criteri diversi, e mettere il giudizio qui obbligherebbe a
/// passargli anche quale sia il criterio.
#[derive(Debug)]
pub struct Referto {
    /// `"iterazione"` oppure `"produzione"`.
    pub immagine: &'static str,
    /// Lo SHA-256 dell'immagine **effettivamente eseguita**, in esadecimale.
    pub digest_immagine: String,
    /// La `Risposta` e' arrivata e l'accordo si e' concluso.
    pub accordo: bool,
    /// Quanti `Progresso` sono arrivati prima dell'esito.
    pub progressi: usize,
    /// L'esito dichiarato.
    pub esito: EsitoOsservato,
    /// Che cosa dice l'artefatto, riletto.
    pub artefatto: Artefatto,
    /// Dopo l'esito il canale si e' chiuso.
    pub fine_del_canale: bool,
    /// Come il processo e' finito, raccolto.
    pub uscita: Option<FineDelProcesso>,
    /// I difetti di **pulizia**: terminazione o raccolta andate storte.
    ///
    /// Non sostituiscono l'esito del percorso, gli si affiancano: un figlio puo'
    /// aver percorso la sequenza **e** non essersi lasciato raccogliere.
    pub difetti_di_pulizia: Vec<String>,
}

/// Cio' che la sequenza deve produrre, qualunque strada si sia percorsa.
///
/// # Perche' l'oracolo e' uno solo
///
/// Perche' le strade sono due — due pipe nude, oppure spawner e dominio — e
/// **cio' che si pretende non cambia**: se una delle due giudicasse piu'
/// morbidamente, sarebbe proprio quella a dichiarare qualificato cio' che
/// l'altra respinge. Con un solo giudizio, la differenza fra i due percorsi
/// resta dove deve stare: in che cosa attraversano, non in che cosa accettano.
///
/// # Perche' tutto esatto, e niente «almeno»
///
/// Perche' la fixture e' fissa: quattro righe, due sopra la soglia, un batch
/// solo, quindi **un** progresso. Una pretesa morbida lascerebbe passare sia un
/// worker che ne manda cento — cioe' che ignora la quota — sia uno che ne manda
/// uno perche' scrive meno del dovuto.
///
/// E l'artefatto si pretende **verificato**, non dichiarato: un worker che
/// annunciasse un successo senza scrivere niente supererebbe un controllo che si
/// fermi all'esito.
///
/// # Che cosa rende
///
/// L'elenco di cio' che manca, vuoto quando non manca niente. Non un booleano:
/// chi legge un rosso deve sapere **quali** pretese sono cadute, e un booleano
/// lo manderebbe a rileggere il referto per indovinarlo.
#[must_use]
pub fn giudica(referto: &Referto, etichetta: &str, digest_atteso: Option<&str>) -> Vec<String> {
    let mut manca = Vec::new();
    let mut pretendi = |vero: bool, che: String| {
        if !vero {
            manca.push(che);
        }
    };

    pretendi(
        referto.immagine == etichetta,
        format!(
            "l'immagine dichiarata e' «{}» invece di «{etichetta}»",
            referto.immagine
        ),
    );
    if let Some(atteso) = digest_atteso {
        pretendi(
            referto.digest_immagine == atteso,
            "il binario eseguito non e' quello misurato prima del percorso".to_owned(),
        );
    }
    pretendi(referto.accordo, "l'accordo non si e' concluso".to_owned());
    pretendi(
        matches!(referto.esito, EsitoOsservato::Successo { .. }),
        format!("l'esito non e' un successo: {:?}", referto.esito),
    );
    pretendi(
        referto.artefatto == Artefatto::Verificato,
        format!("l'artefatto non regge: {:?}", referto.artefatto),
    );
    pretendi(
        referto.progressi == 1,
        format!(
            "progressi {} invece di 1: la fixture ne determina esattamente uno",
            referto.progressi
        ),
    );
    pretendi(
        referto.fine_del_canale,
        "dopo l'esito il canale non si e' chiuso: il worker non e' uscito".to_owned(),
    );
    pretendi(
        referto.uscita == Some(FineDelProcesso::Codice(0)),
        format!("fine del processo {:?} invece del codice 0", referto.uscita),
    );
    pretendi(
        referto.difetti_di_pulizia.is_empty(),
        format!(
            "il figlio ha lasciato difetti di pulizia: {:?}",
            referto.difetti_di_pulizia
        ),
    );
    manca
}

/// Un referto costruito da chi ha percorso la strada con lo spawner.
///
/// # Perche' esiste
///
/// Perche' il percorso sotto limite osserva le stesse cose di [`percorri`] ma
/// non le ottiene allo stesso modo: il figlio glielo consegna lo spawner, e
/// l'immagine e' quella che gli e' stata indicata. Costruire il referto la'
/// vorrebbe dire conoscere i campi da fuori, e allora l'oracolo giudicherebbe
/// una struttura che qualcun altro ha riempito a mano.
#[must_use]
pub fn referto_del_dominio(
    digest_immagine: String,
    visto: Osservato,
    uscita: Option<FineDelProcesso>,
    difetti_di_pulizia: Vec<String>,
) -> Referto {
    Referto {
        immagine: "dominio",
        digest_immagine,
        accordo: visto.accordo,
        progressi: visto.progressi,
        esito: visto.esito,
        artefatto: visto.artefatto,
        fine_del_canale: visto.fine_del_canale,
        uscita,
        difetti_di_pulizia,
    }
}

/// Fa percorrere a un worker reale la sequenza, e ne riferisce.
///
/// # Errors
///
/// [`PlenoraError::IsolationUnavailable`] se il canale, il processo o la
/// sequenza non reggono; gli errori del planner e dell'Arrow IPC se la fixture
/// non si costruisce.
pub fn percorri(immagine: &Immagine) -> Result<Referto> {
    let digest_immagine = digest_dell_immagine(immagine.percorso())?;
    let stanza = tempfile::tempdir()
        .map_err(|causa| non_disponibile("prova", &format!("nessuna directory: {causa}")))?;
    let ingresso = stanza.path().join("citta.arrow");
    let temporaneo = stanza.path().join("artefatto.arrow");

    let (da_lui, mut verso_lui, estremi) = canale::apri()?;
    let mut comando = std::process::Command::new(immagine.percorso());
    comando.arg(super::VERSIONE_WORKER).env(
        canale::VARIABILE_DEL_CANALE,
        estremi.numeri()?.in_variabile(),
    );
    canale::accerta_monothread()?;
    estremi.rendi_ereditabili()?;
    // Il figlio nasce **dentro la guardia**, nella stessa espressione che lo
    // crea: fra lo `spawn` e un incapsulamento fatto una riga dopo ci sarebbe un
    // tratto in cui il processo esiste e nessuno lo custodisce.
    let guardia = comando
        .spawn()
        .map(FiglioVivo::nuovo)
        .map_err(|causa| non_disponibile("prova", &format!("l'immagine non si avvia: {causa}")))?;
    // Gli estremi del worker si chiudono **subito**: finche' li teniamo aperti,
    // la nostra estremita' impedisce l'EOF che stiamo per pretendere.
    drop(estremi);

    // Da qui in poi ogni cammino passa dalla chiusura della guardia, e la causa
    // che ci ha portato li' si conserva accanto ai difetti di pulizia.
    let osservato = sul_canale(
        &digest_immagine,
        &ingresso,
        &temporaneo,
        da_lui,
        &mut verso_lui,
        None,
    );
    let (uscita, difetti_di_pulizia) = chiudi(guardia, osservato.as_ref().err());

    let visto = con_la_pulizia(osservato, &difetti_di_pulizia)?;
    Ok(Referto {
        immagine: immagine.etichetta(),
        digest_immagine,
        accordo: visto.accordo,
        progressi: visto.progressi,
        esito: visto.esito,
        artefatto: visto.artefatto,
        fine_del_canale: visto.fine_del_canale,
        uscita: uscita.map(FineDelProcesso::da),
        difetti_di_pulizia,
    })
}

/// La causa del dialogo, **con** i difetti che la pulizia ha lasciato.
///
/// # Perche' una funzione e non un `?`
///
/// Perche' un `?` nudo scarterebbe i difetti di pulizia: chi legge il rosso
/// saprebbe perche' il dialogo non ha retto, e non che il figlio ha anche
/// protestato uscendo. Sono due fatti, non un dettaglio dello stesso, ed e'
/// l'unica riga in cui uno dei due puo' sparire senza che nessuno se ne
/// accorga — isolata, un caso la attraversa in entrambi i modi.
///
/// La composizione e' esplicita e non passa da `con_contesto`: quella funzione,
/// per progetto, lascia intatte le varianti che questo percorso produce.
///
/// # Errors
///
/// La causa del dialogo, arricchita dei difetti quando ce ne sono.
fn con_la_pulizia(osservato: Result<Osservato>, difetti: &[String]) -> Result<Osservato> {
    match osservato {
        Ok(visto) => Ok(visto),
        Err(causa) if difetti.is_empty() => Err(causa),
        Err(causa) => Err(non_disponibile(
            "prova",
            &format!("{causa}; e la pulizia del figlio ha lasciato: {difetti:?}"),
        )),
    }
}

/// Il dialogo intero su **un canale gia' aperto**, con la fixture e la
/// riverifica.
///
/// # Perche' esiste, e chi la usa
///
/// Perche' i canali sono due, ma la conversazione e' una. [`percorri`] apre una
/// pipe e avvia il worker da se'; il percorso sotto `cgroup2` riceve invece i
/// due estremi dallo **spawner**, che ha attraversato il dominio e lasciato
/// cadere i privilegi. Cio' che accade fra i due estremi — saluto, accordo,
/// incarico, progresso, esito, riverifica dell'artefatto, EOF — e' identico, e
/// due copie divergerebbero proprio sul percorso che qualifica.
///
/// # `di_chi`
///
/// L'uid e il gid del worker, quando non e' questo processo. Dentro il dominio
/// il worker gira con altre credenziali: la fixture e la directory
/// dell'artefatto devono appartenergli, altrimenti il rifiuto che si osserva e'
/// un permesso mancante e non una proprieta' del protocollo.
///
/// # Errors
///
/// Quelli del dialogo, della fixture o della riverifica.
pub(super) fn sul_canale(
    digest_immagine: &str,
    ingresso: &Path,
    temporaneo: &Path,
    da_lui: std::io::PipeReader,
    verso_lui: &mut std::io::PipeWriter,
    di_chi: Option<(u32, u32)>,
) -> Result<Osservato> {
    // La fixture la scrive chi la descrive: l'ingresso e l'incarico che lo
    // nomina devono nascere insieme, o un percorso che dimentichi di scriverlo
    // fallisce **prima** del saluto e lascia il worker ad aspettarlo.
    scrivi_ingresso(ingresso)?;
    let (incarico, di_uscita) = incarico_per(ingresso, temporaneo)?;
    let token = CommitToken::da_esadecimale(TOKEN)
        .map_err(|forma| non_disponibile("prova", &format!("token non canonico: {forma}")))?;
    let supervisore = supervisore_per(digest_immagine, token)?;
    if let Some((uid, gid)) = di_chi {
        concedi_al_worker(ingresso, temporaneo, uid, gid)?;
    }
    dialoga(
        supervisore,
        da_lui,
        verso_lui,
        incarico,
        &Riferimenti {
            contratto: &di_uscita,
            temporaneo,
            token: &token,
        },
    )
}

/// Da' al worker cio' che gli serve per leggere l'ingresso e scrivere
/// l'artefatto.
///
/// # Perche' il proprietario e non i permessi larghi
///
/// Perche' `0o777` renderebbe la fixture scrivibile da chiunque sulla macchina,
/// e un percorso di qualificazione che allarga i permessi per far passare la
/// prova misura anche la propria concessione. Il proprietario invece dice
/// esattamente **chi**: il worker, e nessun altro.
///
/// # Errors
///
/// [`PlenoraError::IsolationUnavailable`] se il proprietario non si cambia.
fn concedi_al_worker(ingresso: &Path, temporaneo: &Path, uid: u32, gid: u32) -> Result<()> {
    let Some(stanza) = temporaneo.parent() else {
        return Err(non_disponibile(
            "prova",
            "l'artefatto temporaneo non ha una directory che lo contenga",
        ));
    };
    // La directory per prima: senza, il worker non puo' creare l'artefatto
    // nemmeno possedendo l'ingresso.
    for quale in [stanza, ingresso] {
        std::os::unix::fs::chown(quale, Some(uid), Some(gid)).map_err(|causa| {
            non_disponibile(
                "prova",
                &format!("{} non passa al worker: {causa}", quale.display()),
            )
        })?;
    }
    Ok(())
}

/// Cio' contro cui l'artefatto si riverifica.
struct Riferimenti<'a> {
    contratto: &'a DataContract,
    temporaneo: &'a Path,
    token: &'a CommitToken,
}

/// Cio' che il dialogo ha osservato, prima che il figlio venga chiuso.
pub struct Osservato {
    /// La `Risposta` e' arrivata e l'accordo si e' concluso.
    pub accordo: bool,
    /// Quanti `Progresso` sono arrivati prima dell'esito.
    pub progressi: usize,
    /// L'esito dichiarato.
    pub esito: EsitoOsservato,
    /// Che cosa dice l'artefatto, riletto.
    pub artefatto: Artefatto,
    /// Dopo l'esito il canale si e' chiuso.
    pub fine_del_canale: bool,
}

/// Il dialogo intero, dal saluto all'EOF.
///
/// Ogni lettura passa dal guardiano: un worker che tace fa fallire questa
/// funzione entro [`TETTO_DELLA_PAROLA`], e il chiamante chiude comunque il
/// figlio.
fn dialoga(
    supervisore: SupervisoreInAttesa,
    da_lui: std::io::PipeReader,
    verso_lui: &mut std::io::PipeWriter,
    incarico: Incarico,
    riferimenti: &Riferimenti<'_>,
) -> Result<Osservato> {
    rendi_non_bloccante(&da_lui)?;
    let (spia, freno) = interruttore();
    let guardiano = Guardiano::comincia(freno, spia.clone())?;
    let mut sorgente = SorgenteTerminabile::con_interruttore(da_lui, PASSO_DI_ATTESA, spia);

    let visto = (|| -> Result<Osservato> {
        let saluto = Frame::nuovo(Corpo::Saluto(Box::new(supervisore.saluto().clone())));
        scrivi(verso_lui, &codifica(&saluto)?)?;

        let Some(risposta) = leggi_frame(&mut sorgente)? else {
            return Err(non_disponibile(
                "prova",
                "il worker ha chiuso senza rispondere al saluto",
            ));
        };
        // L'accordo si conclude qui: l'`HandshakeAccettato` esiste solo come
        // risultato di una verifica riuscita, quindi **averlo in mano** e' la
        // prova che le due descrizioni concordano.
        let _accordo = supervisore.ricevi(risposta)?;

        scrivi(
            verso_lui,
            &codifica(&Frame::nuovo(Corpo::Incarico(Box::new(incarico))))?,
        )?;

        let (progressi, esito) = fino_all_esito(&mut sorgente)?;
        let artefatto = riverifica(&esito, riferimenti);
        // Dopo l'esito il worker non ha piu' niente da dire, e deve
        // **chiudere**: un canale che resta aperto e' un processo che non e'
        // uscito.
        let fine_del_canale = leggi_frame(&mut sorgente)?.is_none();
        Ok(Osservato {
            accordo: true,
            progressi,
            esito,
            artefatto,
            fine_del_canale,
        })
    })();

    // Il guardiano si ferma su **ogni** cammino: un thread che dorme fino alla
    // scadenza terrebbe in piedi il processo anche quando il dialogo e' finito
    // in un istante.
    secondo_il_guardiano(visto, &guardiano.ferma_e_raccogli())
}

/// Cio' che il dialogo ha visto, letto **alla luce** di com'e' andato il
/// guardiano.
///
/// # Le tre letture
///
/// **`Perduto`**: nessuno puo' dire che le letture fossero limitate, e una
/// qualificazione che non risponde del proprio tempo non risponde di niente. Il
/// percorso fallisce anche su un dialogo riuscito, e la causa — se c'e' —
/// non viene sostituita: le si affianca.
///
/// **`Scaduto`**: la scadenza non sostituisce la causa, la spiega. Senza, il rosso
/// direbbe «lettura fermata su richiesta» e chi legge cercherebbe chi ha
/// chiesto.
///
/// **`NonScaduto`**: non c'e' niente da aggiungere, in nessuno dei due versi.
///
/// I messaggi si compongono qui e non con `con_contesto`: quella funzione, per
/// progetto, lascia intatte le varianti che la lettura produce — `Io` e
/// `IsolationUnavailable` — e cio' che si vuole aggiungere sparirebbe senza che
/// niente lo dica.
///
/// # Errors
///
/// Quella del dialogo, spiegata o affiancata; oppure la perdita del guardiano,
/// anche su un dialogo riuscito.
fn secondo_il_guardiano(visto: Result<Osservato>, stato: &StatoDelGuardiano) -> Result<Osservato> {
    if *stato == StatoDelGuardiano::Perduto {
        return Err(non_disponibile(
            "prova",
            &format!(
                "il guardiano delle letture e' andato perduto: il percorso non puo' rispondere                  del proprio tempo{}",
                visto
                    .as_ref()
                    .err()
                    .map_or_else(String::new, |causa| format!("; e inoltre: {causa}"))
            ),
        ));
    }
    match visto {
        Ok(visto) => Ok(visto),
        Err(causa) if *stato == StatoDelGuardiano::Scaduto => Err(non_disponibile(
            "prova",
            &format!(
                "il worker non ha finito entro {} secondi: {causa}",
                TETTO_DELLA_PAROLA.as_secs()
            ),
        )),
        Err(causa) => Err(causa),
    }
}

/// Chi ferma le letture alla scadenza.
///
/// # Perche' un thread e non una scadenza sulla lettura
///
/// Perche' la sorgente fermabile esiste gia' e guarda un interruttore: riusarla
/// costa un thread che dorme. Una seconda nozione di scadenza — `SO_RCVTIMEO`,
/// o un `poll` con timeout — sarebbe un secondo modo di smettere di ascoltare, e
/// i due potrebbero divergere su che cosa significhi «fermo».
struct Guardiano {
    freno: Freno,
    mano: std::thread::JoinHandle<bool>,
}

/// Che cosa il guardiano ha da dire, quando lo si raccoglie.
///
/// # Perche' tre e non un booleano
///
/// Perche' «non scaduto» e «non lo so» non sono la stessa cosa. Un guardiano
/// andato in panico non ha vigilato: il percorso e' arrivato in fondo senza la
/// garanzia che lo limita, e un `false` lo direbbe tranquillo. Con tre
/// stati chi legge deve nominare anche il terzo.
#[derive(Debug, PartialEq, Eq)]
enum StatoDelGuardiano {
    /// Ha vigilato fino alla fine, e la scadenza non e' scattata.
    NonScaduto,
    /// La scadenza e' scattata: le letture sono state fermate.
    Scaduto,
    /// Il guardiano non c'e' piu': nessuno puo' dire se abbia vigilato.
    Perduto,
}

impl Guardiano {
    /// Comincia a contare.
    ///
    /// # Errors
    ///
    /// [`PlenoraError::IsolationUnavailable`] se il thread non nasce: senza
    /// guardiano le letture non avrebbero tetto, e cominciare comunque darebbe
    /// un percorso che puo' appendersi.
    fn comincia(freno: Freno, spia: Interruttore) -> Result<Self> {
        // La scadenza si calcola **prima** di partire, e in modo controllato: un
        // `Instant + Duration` che tracima va in panico, e un guardiano che
        // muore nascendo lascerebbe le letture senza tetto proprio mentre tutto
        // sembra a posto. Qui l'impossibilita' e' osservata, non presunta.
        let scadenza = std::time::Instant::now()
            .checked_add(TETTO_DELLA_PAROLA)
            .ok_or_else(|| {
                non_disponibile(
                    "prova",
                    "la scadenza del guardiano non e' rappresentabile: senza tetto le letture \
                     potrebbero non finire",
                )
            })?;
        let suo = freno.clone();
        let mano = std::thread::Builder::new()
            .name("plenora-guardiano".to_owned())
            .spawn(move || {
                // Si guarda l'orologio a piccoli passi invece di dormire fino
                // alla scadenza: sul cammino felice il dialogo finisce in un
                // istante, e un guardiano che dorme trenta secondi terrebbe in
                // piedi il processo per nulla.
                while std::time::Instant::now() < scadenza {
                    if spia.fermato() {
                        return false;
                    }
                    std::thread::sleep(PASSO_DEL_GUARDIANO);
                }
                suo.ferma();
                true
            })
            .map_err(|causa| {
                non_disponibile("prova", &format!("il guardiano non nasce: {causa}"))
            })?;
        Ok(Self { freno, mano })
    }

    /// Ferma il guardiano, e dice che cosa ha visto.
    ///
    /// # Perche' il panico non diventa «non scaduto»
    ///
    /// Perche' sarebbero due affermazioni diverse dette con la stessa parola.
    /// Un guardiano perduto non ha vigilato: il percorso e' arrivato dove e'
    /// arrivato **senza** il tetto sulle letture, e un percorso che non puo'
    /// garantire il proprio tempo non qualifica niente. Il payload non si
    /// legge — non ci interessa che cosa dicesse — ma la sua assenza si.
    fn ferma_e_raccogli(self) -> StatoDelGuardiano {
        self.freno.ferma();
        match self.mano.join() {
            Ok(true) => StatoDelGuardiano::Scaduto,
            Ok(false) => StatoDelGuardiano::NonScaduto,
            Err(_) => StatoDelGuardiano::Perduto,
        }
    }
}

/// Chiude la guardia, e separa l'uscita dai difetti di pulizia.
///
/// # Perche' un difetto di pulizia non fa fallire
///
/// Perche' non e' l'esito del percorso: e' un fatto **accanto**. Farlo risalire
/// come errore sostituirebbe la causa vera — quella che ha portato fin qui —
/// con l'ultima cosa andata storta.
///
/// # E perche' pero' ci si arrende, se il figlio resta
///
/// Perche' un figlio che non si raccoglie **esiste ancora**, e sopra questo
/// percorso non c'e' nessuno a cui passarlo: il chiamante e' un processo di
/// qualificazione che sta per finire, e finire lasciandolo vivo direbbe verde
/// mentre un worker resta in giro.
///
/// `arrenditi` prova a ucciderlo e poi ferma tutto **dicendo perche'**: i
/// difetti della pulizia e la causa che ha portato qui. Un rosso rumoroso e'
/// l'unico esito onesto quando la pulizia non riesce, e il referto che si
/// perde e' meno grave del processo che resta.
pub(super) fn chiudi<P: super::figlio::ProcessoFiglio>(
    guardia: FiglioVivo<P>,
    causa: Option<&PlenoraError>,
) -> (Option<Uscita>, Vec<String>) {
    // Prima la **cortesia**: si aspetta che finisca da se', senza segnalargli
    // niente. Un worker che ha chiuso il canale sta uscendo, e il primo sguardo
    // lo trova ancora vivo per una manciata di microsecondi: terminarlo li'
    // metterebbe `Segnale(9)` nel referto di un percorso riuscito.
    let cortesia = OrologioDiSistema::nuovo(PASSO_DELLA_RACCOLTA);
    let (guardia, gia_visti) = match guardia.attendi_la_fine(CORTESIA_PRIMA_DEL_SEGNALE, &cortesia)
    {
        Chiusura::Raccolto { uscita, difetti } => return (uscita, difetti),
        // I difetti dell'attesa — un'interrogazione che non risponde — non si
        // perdono passando al secondo tempo: si sommano ai suoi.
        Chiusura::NonRaccolto { guardia, difetti } => (guardia, difetti),
    };
    chiudi_per_forza(guardia, causa, gia_visti)
}

/// Termina il figlio e lo raccoglie, sommando i difetti gia' visti.
///
/// # Perche' separata
///
/// Perche' e' il **secondo tempo**, e ci si arriva solo quando la cortesia e'
/// finita: nessun cammino manda un segnale prima. Tenerla dentro [`chiudi`]
/// avrebbe reso facile, un domani, spostare la terminazione sopra l'attesa
/// senza accorgersene.
fn chiudi_per_forza<P: super::figlio::ProcessoFiglio>(
    guardia: FiglioVivo<P>,
    causa: Option<&PlenoraError>,
    gia_visti: Vec<String>,
) -> (Option<Uscita>, Vec<String>) {
    let orologio = OrologioDiSistema::nuovo(PASSO_DELLA_RACCOLTA);
    match guardia.termina_e_raccogli(TETTO_DELLA_RACCOLTA, &orologio) {
        Chiusura::Raccolto { uscita, difetti } => (uscita, [gia_visti, difetti].concat()),
        Chiusura::NonRaccolto { guardia, difetti } => {
            let tutti = [gia_visti, difetti].concat();
            guardia.arrenditi(&format!(
                "percorso di qualificazione: il figlio non si e' lasciato raccogliere entro {} \
                 secondi di cortesia piu' {} di terminazione; difetti: {tutti:?}; causa del \
                 percorso: {}",
                CORTESIA_PRIMA_DEL_SEGNALE.as_secs(),
                TETTO_DELLA_RACCOLTA.as_secs(),
                causa.map_or_else(
                    || "nessuna, il dialogo era riuscito".to_owned(),
                    std::string::ToString::to_string
                )
            ))
        }
    }
}

/// Legge i frame finche' non arriva l'esito, contando i progressi.
fn fino_all_esito<R: std::io::Read>(
    sorgente: &mut SorgenteTerminabile<R>,
) -> Result<(usize, EsitoOsservato)> {
    let mut progressi = 0_usize;
    loop {
        let Some(frame) = leggi_frame(sorgente)? else {
            return Err(non_disponibile(
                "prova",
                "il canale e' finito prima dell'esito: il worker non ha dichiarato niente",
            ));
        };
        match frame.in_corpo() {
            Corpo::Progresso(_) => progressi += 1,
            Corpo::Esito(esito) => return Ok((progressi, osservato(*esito))),
            altro => {
                return Err(non_disponibile(
                    "prova",
                    &format!("dal worker e' arrivato un corpo fuori sequenza: {altro:?}"),
                ));
            }
        }
    }
}

/// L'esito del filo, con cio' che serve a riverificarlo.
fn osservato(esito: EsitoWorkerSulFilo) -> EsitoOsservato {
    match esito {
        EsitoWorkerSulFilo::Successo {
            digest_artefatto,
            conteggi,
        } => EsitoOsservato::Successo {
            digest: digest_artefatto,
            conteggi,
        },
        EsitoWorkerSulFilo::Errore { errore } => EsitoOsservato::Errore(errore.messaggio),
        EsitoWorkerSulFilo::Panic { forma } => EsitoOsservato::Panico(format!("{forma:?}")),
    }
}

/// Riapre l'artefatto e gli fa i passi da 3 a 8-bis.
///
/// # Perche' non basta l'esito
///
/// Perche' l'esito e' un'**affermazione del worker**, e un worker che
/// dichiarasse un successo senza scrivere niente passerebbe un controllo che si
/// fermi li'. Il verificatore confronta invece l'artefatto con cio' che e' stato
/// dichiarato: sigillo, framing, digest dell'intero file, contratto per
/// fingerprint, conteggi osservati e commit token del footer.
///
/// E' la **stessa funzione** che il supervisore usa nella sequenza, non una sua
/// imitazione: se qui passasse qualcosa che li' non passa, il percorso direbbe
/// il falso proprio su cio' che deve garantire.
fn riverifica(esito: &EsitoOsservato, riferimenti: &Riferimenti<'_>) -> Artefatto {
    let EsitoOsservato::Successo { digest, conteggi } = esito else {
        return Artefatto::NonDichiarato;
    };
    let attese = AtteseVerifica {
        contratto: riferimenti.contratto,
        digest,
        conteggi: *conteggi,
        commit_token: riferimenti.token,
    };
    match verifica_artefatto(
        riferimenti.temporaneo,
        &attese,
        crate::risolutore::risolvi,
        &IpcLimits::default(),
    ) {
        Ok(()) => Artefatto::Verificato,
        Err(causa) => Artefatto::Respinto(causa.to_string()),
    }
}

/// Scrive tutti i byte e li fa partire.
fn scrivi(dove: &mut std::io::PipeWriter, byte: &[u8]) -> Result<()> {
    dove.write_all(byte)
        .and_then(|()| dove.flush())
        .map_err(|causa| non_disponibile("prova", &format!("il frame non parte: {causa}")))
}

/// Il supervisore che si descrive **come il worker si dichiarera'**.
///
/// Stessa versione, stesso resolver, stesso ambiente, e il digest dell'immagine
/// che sta per eseguire. Se una sola delle quattro cose diverge, l'accordo cade
/// — che e' precisamente cio' che l'handshake esiste per fare.
///
/// # Errors
///
/// Quelli della descrizione locale, e [`PlenoraError::InvalidConfiguration`] se
/// le attese non sono coerenti.
fn supervisore_per(
    digest_immagine: &str,
    commit_token: CommitToken,
) -> Result<SupervisoreInAttesa> {
    let locale = descrizione::di_questa_build(Vec::new())?;
    let mut comune = locale.comune;
    comune.artefatto.digest = DigestSha256::da_esadecimale(digest_immagine)
        .map_err(|forma| non_disponibile("prova", &format!("digest non canonico: {forma}")))?;
    SupervisoreInAttesa::nuovo(AtteseSupervisore {
        comune,
        // Nessuna capability richiesta: cio' che si prova qui e' il cablaggio, e
        // pretendere un backend renderebbe il percorso indisponibile sulle build
        // che non lo hanno.
        capability_richieste: Vec::new(),
        commit_token,
    })
}

/// Lo SHA-256 di un file, in esadecimale minuscolo.
pub(super) fn digest_dell_immagine(percorso: &Path) -> Result<String> {
    use std::io::Read as _;

    let mut file = std::fs::File::open(percorso).map_err(|causa| {
        non_disponibile(
            "prova",
            &format!("l'immagine {} non si apre: {causa}", percorso.display()),
        )
    })?;
    let mut digestore = Sha256::new();
    let mut blocco = vec![0_u8; 64 * 1024];
    loop {
        let quanti = file.read(&mut blocco).map_err(|causa| {
            non_disponibile("prova", &format!("l'immagine non si legge: {causa}"))
        })?;
        if quanti == 0 {
            break;
        }
        digestore.update(&blocco[..quanti]);
    }
    Ok(Esadecimale32::dai_byte(digestore.finalize().into()).in_esadecimale())
}

/// Il piano dell'esempio `e1`, che filtra e ordina.
///
/// Un piano vero e non un grafo vuoto: cio' che si prova e' che il worker
/// **esegua**, e un'esecuzione senza righe non produrrebbe nessun batch, quindi
/// nessun progresso.
fn piano() -> String {
    serde_json::json!({
        "schema_version": 5,
        "inputs": ["citta"],
        "nodes": [
            {
                "id": "grandi",
                "op": "table.filter",
                "in": ["citta"],
                "config": { "column": "abitanti", "operator": ">", "value": 300_000 }
            },
            {
                "id": "ordinate",
                "op": "table.sort",
                "in": ["grandi"],
                "config": { "columns": ["abitanti"], "ascending": false }
            }
        ],
        "output": "ordinate"
    })
    .to_string()
}

/// Scrive l'ingresso in Arrow IPC, formato file.
///
/// Quattro righe, due sopra la soglia del filtro, in un batch solo: i conteggi
/// che il worker dichiarera' sono **determinati**, e chi giudica il referto puo'
/// pretenderli esatti invece di «almeno qualcosa».
fn scrivi_ingresso(dove: &Path) -> Result<()> {
    use std::sync::Arc;

    let schema = Arc::new(Schema::new(vec![
        Field::new("nome", DataType::Utf8, false),
        Field::new("abitanti", DataType::Int64, false),
    ]));
    let batch = RecordBatch::try_new(
        Arc::clone(&schema),
        vec![
            Arc::new(StringArray::from(vec!["alfa", "beta", "gamma", "delta"])),
            Arc::new(Int64Array::from(vec![100_000, 500_000, 250_000, 900_000])),
        ],
    )
    .map_err(|causa| non_disponibile("prova", &format!("il batch non si costruisce: {causa}")))?;

    let file = std::fs::File::create(dove)
        .map_err(|causa| non_disponibile("prova", &format!("l'ingresso non si crea: {causa}")))?;
    let mut scrittore = FileWriter::try_new(file, &schema)
        .map_err(|causa| non_disponibile("prova", &format!("writer IPC: {causa}")))?;
    scrittore
        .write(&batch)
        .and_then(|()| scrittore.finish())
        .map_err(|causa| non_disponibile("prova", &format!("l'ingresso non si scrive: {causa}")))
}

/// L'incarico, e il contratto che l'artefatto dovra' avere.
///
/// Il fingerprint del contratto e il `plan_hash` non si scrivono a mano: si
/// calcolano da `planner`, cioe' da chi li calcolera' anche dall'altra parte.
/// Scriverli qui vorrebbe dire provare che due costanti coincidono.
fn incarico_per(ingresso: &Path, temporaneo: &Path) -> Result<(Incarico, DataContract)> {
    let schema = ipc_boundary::header_schema(ingresso, &IpcLimits::default())?;
    let contratto = contract_from_arrow_schema(schema, crate::risolutore::risolvi)?;
    let impronta = planner::contract_fingerprint(&contratto)?;
    let testo = piano();
    let grafo = planner::validate(&testo, &[("citta".to_owned(), contratto)])?;
    let di_uscita = grafo.output_contract()?.clone();

    let canonico = serde_json::value::RawValue::from_string(testo)
        .map_err(|causa| non_disponibile("prova", &format!("il piano non e' JSON: {causa}")))?;
    let plan_hash_atteso = DigestSha256::da_esadecimale(&grafo.plan_hash().to_hex())
        .map_err(|forma| non_disponibile("prova", &format!("plan hash: {forma}")))?;
    let contract_fingerprint_atteso = DigestSha256::da_esadecimale(&impronta.to_hex())
        .map_err(|forma| non_disponibile("prova", &format!("fingerprint: {forma}")))?;

    Ok((
        Incarico {
            piano_canonico: canonico,
            plan_hash_atteso,
            ingressi: vec![DescrittoreIngresso {
                nome: "citta".to_owned(),
                percorso: percorso_in_testo(ingresso)?,
                formato: FormatoIngresso::File,
                contract_fingerprint_atteso,
            }],
            artefatto_temporaneo: percorso_in_testo(temporaneo)?,
        },
        di_uscita,
    ))
}

/// Un percorso come testo, o il rifiuto di indovinarlo.
///
/// I percorsi non sono testo su nessuna piattaforma, e il protocollo li porta
/// come stringhe: convertirli con perdita darebbe al worker un percorso diverso
/// da quello che gli si intende dare.
fn percorso_in_testo(percorso: &Path) -> Result<String> {
    percorso.to_str().map(str::to_owned).ok_or_else(|| {
        PlenoraError::InvalidConfiguration(format!(
            "il percorso {} non e' UTF-8, e il protocollo porta stringhe",
            percorso.display()
        ))
    })
}

#[cfg(test)]
mod tests;
