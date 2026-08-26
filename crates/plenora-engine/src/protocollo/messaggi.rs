//! I sei messaggi del protocollo, come **forma serializzata**.
//!
//! `deny_unknown_fields` a ogni livello, nessun `serde_json::Value`, nessuna
//! mappa aperta, nessun campo d'estensione. Cio' che non e' riconosciuto e'
//! un errore: e' la regola che trasforma un'estensione futura in un guasto
//! **presente e visibile**, invece che in un silenzio.
//!
//! # L'unico campo non tipizzato, e perche' non e' un'eccezione alla regola
//!
//! [`Incarico::piano_canonico`] non ha una forma dichiarata qui. La regola che
//! vieta `serde_json::Value` vieta i **punti d'estensione aperti**: un campo
//! in cui struttura sconosciuta entra in un *messaggio* e ci resta senza che
//! nessuno la guardi. Il piano non e' struttura del protocollo — ha un
//! validatore proprio, una versione propria e un hash proprio, e il protocollo
//! non e' l'autorita' che lo interpreta.
//!
//! Il campo e' quindi [`RawValue`]: JSON **grezzo, non parsato**. La differenza
//! con `Value` non e' stilistica.
//!
//! - `Value` avrebbe fatto due serializzazioni — una misurata contro il tetto,
//!   un'altra spedita — e nulla avrebbe garantito che fossero gli stessi byte.
//!   `RawValue` ne ha una sola: quella che si misura e' quella che parte.
//! - `Value` avrebbe riparsato ogni numero in `f64`/`i64` e riemesso la propria
//!   forma, cioe' avrebbe potuto **riscrivere** il testo su cui il `plan_hash`
//!   e' stato calcolato.
//! - `Value` avrebbe collassato in silenzio le chiavi duplicate interne al
//!   piano («vince l'ultima»).
//!
//! Cio' che `RawValue` **non** fa e' validare: garantisce solo che il testo sia
//! JSON sintatticamente valido. Che sia un piano, che sia della versione
//! giusta, che il suo hash sia [`Incarico::plan_hash_atteso`] — sono verifiche
//! del worker, ed e' li' che devono stare.
//!
//! # Che cosa NON e' qui
//!
//! La **semantica**. `PR-4` sa dire «questo non e' un `Saluto` ben formato»;
//! non sa dire «questo `Saluto` viene dal binario sbagliato». Che il digest
//! sia quello giusto, che il resolver sia compatibile, che le capability
//! bastino, che il `commit_token` sia quello che finira' nel footer —
//! appartengono a `PR-5`.
//!
//! # Il worker non conclude per il supervisore
//!
//! Sul filo viaggia la proiezione di [`EsitoWorker`], non l'esito
//! classificato: quello nasce nel supervisore combinando esito del worker,
//! timeout, cancellazione ed evidenza del sistema operativo. Un worker che
//! dichiarasse `ResourceLimit` starebbe affermando un'evidenza cgroup che non
//! ha letto.

use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;

/// Versione del protocollo.
///
/// Sta **solo** nell'involucro di ogni frame. `Saluto` e `Risposta` non ne
/// portano una seconda copia: due rappresentazioni della stessa cosa sono due
/// cose che possono divergere, e la domanda «quale delle due vale» non ha una
/// risposta buona.
pub const VERSIONE_PROTOCOLLO: u16 = 1;

/// Il tipo di un messaggio, enumerazione **chiusa**.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TipoMessaggio {
    Saluto,
    Incarico,
    Annulla,
    Risposta,
    Progresso,
    Esito,
}

impl TipoMessaggio {
    /// La direzione ammessa per questo tipo.
    #[must_use]
    pub const fn direzione(self) -> Direzione {
        match self {
            Self::Saluto | Self::Incarico | Self::Annulla => Direzione::VersoWorker,
            Self::Risposta | Self::Progresso | Self::Esito => Direzione::VersoSupervisore,
        }
    }
}

/// Il verso del canale.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direzione {
    VersoWorker,
    VersoSupervisore,
}

/// Identita' di un artefatto: digest e versione dichiarata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IdentitaArtefatto {
    pub digest: String,
    pub versione: String,
}

/// Identita' del resolver CRS.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IdentitaResolver {
    pub identita: String,
    pub versione: String,
}

/// Una risorsa che il caricatore ha **effettivamente** aperto.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RisorsaRisolta {
    pub nome: String,
    pub versione: String,
    pub percorso: String,
}

/// Un backend collegato dinamicamente.
///
/// Ha un'identita' propria perche' il digest dell'eseguibile **non lo copre**:
/// non dice nulla della libreria che il caricatore risolvera'.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackendDinamico {
    pub nome: String,
    pub versione: String,
    pub percorso: String,
}

/// L'ambiente su cui i due lati si accordano.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Ambiente {
    /// Digest dell'insieme immutabile e content-addressed delle risorse
    /// **disponibili**, non di quelle usate.
    pub digest_insieme: String,
    /// Deve essere `false`. E' dichiarato invece che assunto perche' un
    /// backend che scarica una griglia a meta' esecuzione renderebbe il
    /// digest una fotografia scaduta.
    pub acquisizione_dinamica: bool,
    pub risorse: Vec<RisorsaRisolta>,
    pub backend_dinamici: Vec<BackendDinamico>,
}

/// Primo messaggio del supervisore.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Saluto {
    pub artefatto: IdentitaArtefatto,
    pub resolver: IdentitaResolver,
    pub ambiente: Ambiente,
    /// Trasmesso e accettato qui, e **solo** qui: legarlo all'handshake gli
    /// da' una sola autorita' invece di due copie che possono divergere.
    pub commit_token: String,
    pub limiti: LimitiDichiarati,
}

/// I tetti che il supervisore dichiara, **nominati uno per uno**.
///
/// Non sono negoziabili: il worker li verifica contro le proprie costanti e
/// si ferma se non coincidono. Viaggiano perche' un disaccordo va scoperto
/// nell'handshake, non alla prima violazione.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
// Il prefisso comune non e' ridondanza da togliere: questi nomi sono
// **quelli sul filo**, e devono restare uguali alle costanti che
// dichiarano. Accorciarli renderebbe il messaggio piu' difficile da
// confrontare con cio' che afferma.
#[allow(clippy::struct_field_names)]
pub struct LimitiDichiarati {
    pub max_frame_bytes: u64,
    pub max_piano_canonico_bytes: u64,
    pub max_messaggi_verso_worker: u64,
    pub max_messaggi_verso_supervisore: u64,
}

/// Formato del contenitore Arrow IPC di un ingresso.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FormatoIngresso {
    File,
    Stream,
}

/// Un ingresso, **descritto** e non trasportato.
///
/// Il contratto completo non viaggia: il worker rilegge lo schema dal file,
/// ricostruisce il contratto con l'autorita' condivisa e confronta il
/// fingerprint. Trasportarlo avrebbe richiesto un codec reversibile del
/// `DataContract` che oggi non esiste, e avrebbe fatto fidare il worker di
/// una descrizione altrui invece che del file che sta per leggere.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DescrittoreIngresso {
    pub nome: String,
    pub percorso: String,
    pub formato: FormatoIngresso,
    /// Verifica **schema e contratto**, non l'identita' dei dati: due file
    /// con righe diverse e lo stesso schema hanno lo stesso fingerprint.
    pub contract_fingerprint_atteso: String,
}

/// L'incarico: il piano e da dove leggere.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Incarico {
    /// La forma **canonica** del piano, come JSON grezzo.
    ///
    /// Non una stringa: una stringa subirebbe l'espansione degli escape e
    /// costringerebbe a un secondo parse. E non il testo originale, che il
    /// modello validato non conserva.
    ///
    /// Il worker la riparsa, rivalida e **ricontrolla il `plan_hash`** contro
    /// [`Self::plan_hash_atteso`].
    pub piano_canonico: Box<RawValue>,
    /// Senza questo il worker puo' ricalcolare un hash e non ha nulla con cui
    /// confrontarlo.
    pub plan_hash_atteso: String,
    pub ingressi: Vec<DescrittoreIngresso>,
    /// Un solo percorso, dentro una directory che il supervisore ha creato.
    /// Il worker non ne sceglie ne' il nome ne' la posizione.
    pub artefatto_temporaneo: String,
}

/// Uguaglianza **sui byte**, perche' [`RawValue`] non ne ha una derivabile.
///
/// Due piani semanticamente identici scritti in modo diverso sono qui
/// diversi, ed e' la risposta giusta per un tipo di filo: la domanda che
/// questo tipo sa porre e' «e' arrivato lo stesso testo», non «e' lo stesso
/// piano». La seconda ha una risposta sola, ed e' il `plan_hash`.
impl PartialEq for Incarico {
    fn eq(&self, altro: &Self) -> bool {
        self.piano_canonico.get() == altro.piano_canonico.get()
            && self.plan_hash_atteso == altro.plan_hash_atteso
            && self.ingressi == altro.ingressi
            && self.artefatto_temporaneo == altro.artefatto_temporaneo
    }
}

impl Eq for Incarico {}

/// Cancellazione richiesta.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Annulla {
    pub motivo: String,
}

/// Primo messaggio del worker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Risposta {
    pub artefatto: IdentitaArtefatto,
    pub resolver: IdentitaResolver,
    pub ambiente: Ambiente,
    pub capability: Vec<String>,
}

/// Avanzamento: contatori deterministici, **mai** dati.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Progresso {
    pub righe: u64,
    pub batch: u64,
    pub nodi_completati: u64,
}

/// Asse «categoria» dell'errore sul filo.
///
/// Enumerazione chiusa: il nome canonico non viaggia come stringa libera,
/// altrimenti un valore sconosciuto passerebbe come testo e si fermerebbe
/// piu' tardi, o mai.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CategoriaSulFilo {
    InvalidPlan,
    InvalidConfiguration,
    Schema,
    DataMapping,
    Crs,
    Unsupported,
    NotFound,
    Conflict,
    Authentication,
    Authorization,
    Timeout,
    Cancelled,
    ResourceLimit,
    Io,
    Protocol,
    Transient,
    Execution,
    IsolationUnavailable,
    UnattributedMemoryPressure,
    Internal,
}

/// Asse «fase».
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FaseSulFilo {
    Validate,
    Connect,
    Probe,
    Prepare,
    Read,
    Write,
    Finalize,
    Commit,
    Rollback,
    Cleanup,
}

/// Asse «effetto remoto».
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EffettoSulFilo {
    None,
    RolledBack,
    Partial,
    Committed,
    Unknown,
}

/// Asse «ritentativo».
///
/// `delay_ms` e' ammesso **esclusivamente** con [`Self::After`]: un ritardo su
/// una disposizione che non lo prevede direbbe al chiamante di riprovare piu'
/// tardi senza che nulla glielo abbia concesso.
///
/// # Perche' le varianti senza campi sono scritte `Never {}`
///
/// In un enum con tag interno, `deny_unknown_fields` **non ha effetto sulle
/// varianti unitarie**: `serde` le riconosce dal tag e ignora il resto
/// dell'oggetto. Scritto `Never`, questo tipo accettava
/// `{"kind":"never","delay_ms":10}` e buttava via `delay_ms` in silenzio —
/// cioe' esattamente la cosa che il campo esiste per impedire.
///
/// La forma `Never {}` e' una variante di struttura con zero campi: sul filo
/// e' identica (`{"kind":"never"}`), ma la deserializzazione passa per un
/// visitor di struttura, e li' `deny_unknown_fields` vale davvero.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RetrySulFilo {
    Never {},
    Safe {},
    RequiresIdempotencyKey {},
    RequiresRecovery {},
    After { delay_ms: u64 },
}

/// Un esempio della diagnostica di riga.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EsempioDiagnostica {
    pub indice: u64,
    pub codice: String,
}

/// Diagnostica di riga, **struttura chiusa**.
///
/// Non un JSON arbitrario: conteggi e limiti per elemento, cosi' un payload
/// grande non e' un payload profondo, e ogni voce ha un tetto proprio.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiagnosticaSulFilo {
    pub contract: String,
    pub scope: String,
    pub completeness: String,
    pub observed_total: u64,
    pub conteggi: Vec<(String, u64)>,
    pub esempi: Vec<EsempioDiagnostica>,
    pub esempi_troncati: bool,
}

/// Un errore tipizzato sul filo.
///
/// Conserva i quattro assi **e** cio' che li accompagna: messaggio
/// sanitizzato, contesto strutturale, diagnostica ammessa. I soli quattro
/// assi scarterebbero il resto prima che serva.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ErroreSulFilo {
    pub categoria: CategoriaSulFilo,
    pub fase: FaseSulFilo,
    pub effetto: EffettoSulFilo,
    pub retry: RetrySulFilo,
    /// Sanitizzato: mai valori di cella, mai payload, mai frammenti di riga.
    pub messaggio: String,
    pub nodo: Option<String>,
    pub operazione: Option<String>,
    pub execution_id: Option<String>,
    pub diagnostica: Option<DiagnosticaSulFilo>,
}

/// La forma del payload di un panico, enumerazione **chiusa**.
///
/// Non una stringa: una stringa accetta qualunque stringa, e il contenuto del
/// panico finirebbe dove il progetto dichiara che non finisce mai. I tre
/// valori sono quelli che `std` puo' produrre, e li distingue
/// `plenora_core::panic_policy` senza leggerne nessuno.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FormaPanicSulFilo {
    Statico,
    Dinamico,
    NonTestuale,
}

/// Il digest dell'artefatto finalizzato.
///
/// **Non** il marcatore del footer: quello e' un sigillo durevole scritto da
/// `FileWriter::finish` e verificato col framing. Questo e' calcolato
/// sull'**intero file finalizzato, footer compreso**, e viaggia solo qui —
/// scriverlo dentro il file che copre sarebbe autoreferenziale.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DigestArtefatto {
    pub algoritmo: String,
    pub valore: String,
}

/// L'esito che il worker dichiara **di se'**.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "esito", rename_all = "snake_case", deny_unknown_fields)]
pub enum EsitoWorkerSulFilo {
    /// Il worker ha finito. **Non** e' il successo finale: la verifica e il
    /// publish non sono affermazioni del worker.
    Successo {
        digest_artefatto: DigestArtefatto,
    },
    /// L'errore viaggia in un `Box`: e' molto piu' grande delle altre due
    /// varianti, e senza il `Box` ogni `Esito` — compresi i successi —
    /// occuperebbe la sua taglia. Sul filo non cambia nulla.
    Errore {
        errore: Box<ErroreSulFilo>,
    },
    Panic {
        forma: FormaPanicSulFilo,
    },
}

/// Il corpo di un frame, scelto dal tipo.
///
/// # Serializza, non deserializza
///
/// `untagged` qui vale **solo in scrittura**, dove significa «emetti il corpo
/// nudo, senza una seconda etichetta»: il tipo lo dichiara gia'
/// [`Frame::tipo`], e ripeterlo darebbe due autorita' sulla stessa domanda.
///
/// In lettura questo tipo non ha `Deserialize`, ed e' deliberato. `untagged`
/// deserializza **provando le varianti a turno**: il tipo dichiarato non
/// verrebbe usato per scegliere, ma solo confrontato dopo, e un corpo
/// etichettato male passerebbe il parser per essere respinto altrove. Il
/// decoder invece legge `tipo` **prima** e deserializza il corpo in quel tipo
/// e basta — cosi' l'incoerenza e' un errore di forma, non un controllo che
/// qualcuno puo' dimenticare di scrivere.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum Corpo {
    Saluto(Box<Saluto>),
    Incarico(Box<Incarico>),
    Annulla(Annulla),
    Risposta(Box<Risposta>),
    Progresso(Progresso),
    Esito(Box<EsitoWorkerSulFilo>),
}

/// L'involucro di ogni frame.
///
/// Solo `Serialize`: si legge con `codifica::decodifica`, non con un derive.
/// La direzione filo → struttura e' una funzione che controlla, non una
/// conversione che riesce.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Frame {
    pub protocol_version: u16,
    pub tipo: TipoMessaggio,
    pub corpo: Corpo,
}
