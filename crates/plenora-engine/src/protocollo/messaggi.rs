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
//! La **semantica**. Questo modulo sa dire «questo non e' un `Saluto` ben
//! formato»; non sa dire «questo `Saluto` viene dal binario sbagliato». Che
//! il digest sia quello giusto, che il resolver sia compatibile, che le
//! capability bastino, che il `commit_token` sia quello che finira' nel
//! footer — appartengono al supervisore.
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

use super::digest::DigestSha256;
use crate::commit_token::CommitToken;

/// Versione del protocollo.
///
/// Sta **solo** nell'involucro di ogni frame. `Saluto` e `Risposta` non ne
/// portano una seconda copia: due rappresentazioni della stessa cosa sono due
/// cose che possono divergere, e la domanda «quale delle due vale» non ha una
/// risposta buona.
pub const VERSIONE_PROTOCOLLO: u16 = 1;

/// Genera **insieme** l'enum, il suo nome sul filo e l'insieme di tutte le sue
/// varianti.
///
/// Serve a togliere di mezzo una classe di difetto, non a scrivere meno: una
/// tabella di prova scritta a mano accanto all'enum **enumera se stessa**.
/// Aggiungere una variante la lascia invariata, e il test resta verde
/// affermando che tutte le varianti hanno il nome giusto — su un insieme che
/// non le contiene tutte.
///
/// Qui la lista e' una sola. Una variante non puo' esistere senza un nome sul
/// filo (la macro lo pretende) e senza comparire in [`TUTTE`](Self::TUTTE).
///
/// Cio' che la macro **non** garantisce, e che i test devono ancora provare:
/// che i nomi siano distinti, e che ciascuno rilegga la propria variante e non
/// quella di un altro.
macro_rules! enum_sul_filo {
    (
        $(#[$attributo:meta])*
        $nome:ident {
            $( $variante:ident => $filo:literal ),+ $(,)?
        }
    ) => {
        $(#[$attributo])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
        pub enum $nome {
            $(
                #[serde(rename = $filo)]
                $variante,
            )+
        }

        impl $nome {
            /// Ogni variante col proprio nome sul filo.
            ///
            /// Generata dalla stessa lista che genera le varianti: le due non
            /// possono divergere.
            pub const TUTTE: &'static [(Self, &'static str)] = &[
                $( (Self::$variante, $filo), )+
            ];
        }
    };
}

/// Come [`enum_sul_filo!`], ma per gli enum **con tag interno**, le cui
/// varianti possono avere campi.
///
/// Due regole, scelte dal fatto che i campi portino o no un valore
/// rappresentativo:
///
/// - **con** `= valore` genera anche `TUTTE`, cioe' un campione per variante
///   col proprio nome sul filo;
/// - **senza**, genera i soli `NOMI`.
///
/// La differenza esiste perche' non tutti i campioni sono costruibili in
/// contesto costante: un `DigestArtefatto` porta `String`. Dove il campione
/// c'e' la prova e' piu' forte — il valore lo costruisce il compilatore, e
/// deve combaciare con la forma della variante; dove non c'e', `NOMI` basta
/// comunque a rendere **impossibile** che una variante resti fuori dalle
/// prove, perche' il test itera i nomi generati e pretende un caso per
/// ciascuno.
///
/// Le graffe sono obbligatorie anche per le varianti senza campi
/// (`Never {}`), e non e' un vezzo di sintassi: in un enum con tag interno
/// `deny_unknown_fields` **non copre le varianti unitarie**. La macro rende
/// quella forma non scrivibile.
macro_rules! enum_con_tag_sul_filo {
    (
        $(#[$attributo:meta])*
        $nome:ident, tag = $tag:literal {
            $(
                $(#[$vattributo:meta])*
                $variante:ident { $( $campo:ident : $tipo:ty = $campione:expr, )* } => $filo:literal
            ),+ $(,)?
        }
    ) => {
        $(#[$attributo])*
        #[serde(tag = $tag, deny_unknown_fields)]
        pub enum $nome {
            $(
                $(#[$vattributo])*
                #[serde(rename = $filo)]
                $variante { $( $campo : $tipo ),* },
            )+
        }

        impl $nome {
            /// I nomi sul filo, generati con le varianti.
            pub const NOMI: &'static [&'static str] = &[ $( $filo ),+ ];

            /// Un campione per variante, col proprio nome sul filo.
            pub const TUTTE: &'static [(Self, &'static str)] = &[
                $( (Self::$variante { $( $campo : $campione ),* }, $filo), )+
            ];
        }
    };
    (
        $(#[$attributo:meta])*
        $nome:ident, tag = $tag:literal {
            $(
                $(#[$vattributo:meta])*
                $variante:ident { $( $campo:ident : $tipo:ty, )* } => $filo:literal
            ),+ $(,)?
        }
    ) => {
        $(#[$attributo])*
        #[serde(tag = $tag, deny_unknown_fields)]
        pub enum $nome {
            $(
                $(#[$vattributo])*
                #[serde(rename = $filo)]
                $variante { $( $campo : $tipo ),* },
            )+
        }

        impl $nome {
            /// I nomi sul filo, generati con le varianti.
            pub const NOMI: &'static [&'static str] = &[ $( $filo ),+ ];
        }
    };
}

enum_sul_filo! {
    /// Il tipo di un messaggio, enumerazione **chiusa**.
    TipoMessaggio {
        Saluto => "saluto",
        Incarico => "incarico",
        Annulla => "annulla",
        Risposta => "risposta",
        Progresso => "progresso",
        Esito => "esito",
    }
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
    /// Digest dell'eseguibile. E' un tipo e non una `String`: la forma
    /// canonica non e' un controllo da ricordare in `codifica`.
    pub digest: DigestSha256,
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
    pub digest_insieme: DigestSha256,
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
    ///
    /// E' un [`CommitToken`] e non una `String`: la forma canonica e' garantita
    /// dal tipo, quindi non c'e' un tetto da applicare qui ne' un controllo che
    /// il decoder possa dimenticare.
    pub commit_token: CommitToken,
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

enum_sul_filo! {
    /// Formato del contenitore Arrow IPC di un ingresso.
    FormatoIngresso {
        File => "file",
        Stream => "stream",
    }
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
    pub contract_fingerprint_atteso: DigestSha256,
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
    pub plan_hash_atteso: DigestSha256,
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

enum_sul_filo! {
    /// Asse «categoria» dell'errore sul filo.
    ///
    /// Enumerazione chiusa: il nome canonico non viaggia come stringa libera,
    /// altrimenti un valore sconosciuto passerebbe come testo e si fermerebbe
    /// piu' tardi, o mai.
    CategoriaSulFilo {
        InvalidPlan => "invalid_plan",
        InvalidConfiguration => "invalid_configuration",
        Schema => "schema",
        DataMapping => "data_mapping",
        Crs => "crs",
        Unsupported => "unsupported",
        NotFound => "not_found",
        Conflict => "conflict",
        Authentication => "authentication",
        Authorization => "authorization",
        Timeout => "timeout",
        Cancelled => "cancelled",
        ResourceLimit => "resource_limit",
        Io => "io",
        Protocol => "protocol",
        Transient => "transient",
        Execution => "execution",
        IsolationUnavailable => "isolation_unavailable",
        UnattributedMemoryPressure => "unattributed_memory_pressure",
        Internal => "internal",
    }
}

enum_sul_filo! {
    /// Asse «fase».
    FaseSulFilo {
        Validate => "validate",
        Connect => "connect",
        Probe => "probe",
        Prepare => "prepare",
        Read => "read",
        Write => "write",
        Finalize => "finalize",
        Commit => "commit",
        Rollback => "rollback",
        Cleanup => "cleanup",
    }
}

enum_sul_filo! {
    /// Asse «effetto remoto».
    EffettoSulFilo {
        None => "none",
        RolledBack => "rolled_back",
        Partial => "partial",
        Committed => "committed",
        Unknown => "unknown",
    }
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
/// dell'oggetto. Scritto `Never`, questo tipo accetterebbe
/// `{"kind":"never","delay_ms":10}` e butterebbe via `delay_ms` in silenzio —
/// cioe' esattamente la cosa che `deny_unknown_fields` esiste per impedire.
///
/// La forma `Never {}` e' una variante di struttura con zero campi: sul filo
/// e' identica (`{"kind":"never"}`), ma la deserializzazione passa per un
/// visitor di struttura, e li' `deny_unknown_fields` vale davvero.
enum_con_tag_sul_filo! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
    RetrySulFilo, tag = "kind" {
        Never {} => "never",
        Safe {} => "safe",
        RequiresIdempotencyKey {} => "requires_idempotency_key",
        RequiresRecovery {} => "requires_recovery",
        After { delay_ms: u64 = 1, } => "after"
    }
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

enum_sul_filo! {
    /// La forma del payload di un panico, enumerazione **chiusa**.
    ///
    /// Non una stringa: una stringa accetta qualunque stringa, e il contenuto del
    /// panico finirebbe dove il progetto dichiara che non finisce mai. I tre
    /// valori sono quelli che `std` puo' produrre, e li distingue
    /// `plenora_core::panic_policy` senza leggerne nessuno.
    FormaPanicSulFilo {
        Statico => "statico",
        Dinamico => "dinamico",
        NonTestuale => "non_testuale",
    }
}

/// I conteggi che il worker dichiara sull'artefatto prodotto.
///
/// # Perche' sono nel `Successo` e non altrove
///
/// Il passo 8 della verifica (§7 di `isolamento.md`) confronta «i conteggi
/// dichiarati nell'`Esito`» con quelli osservati rileggendo l'artefatto. Senza
/// questi campi quel passo non avrebbe un termine di paragone: un `Successo`
/// che portasse il solo digest lascerebbe la sequenza normativa a citare un
/// dato che non viaggia.
///
/// Un digest uguale non li sostituisce. Dice che il file e' quel file, non che
/// contenga cio' che il worker crede di aver scritto: un worker che si
/// fermasse a meta' e finalizzasse comunque produrrebbe un artefatto integro
/// e **incompleto**, e il digest non avrebbe nulla da obiettare.
///
/// # Perche' due e non tre
///
/// `Progresso` porta anche `nodi_completati`, e qui non c'e'. Non e' una
/// dimenticanza: rileggendo un file Arrow IPC si osservano righe e batch, non
/// quanti nodi del piano li hanno prodotti. Dichiarare un numero che il
/// verificatore non puo' confrontare significherebbe chiedergli di crederci —
/// ed e' esattamente cio' che questo passo esiste per non fare.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConteggiDichiarati {
    /// Righe scritte nell'artefatto.
    pub righe: u64,
    /// Record batch scritti nell'artefatto.
    pub batch: u64,
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
enum_con_tag_sul_filo! {
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    EsitoWorkerSulFilo, tag = "esito" {
        /// Il worker ha finito. **Non** e' il successo finale: la verifica e
        /// il publish non sono affermazioni del worker.
        ///
        /// I conteggi sono **obbligatori**: sono il termine di paragone del
        /// passo 8, e renderli facoltativi avrebbe reso facoltativo il passo.
        Successo {
            digest_artefatto: DigestArtefatto,
            conteggi: ConteggiDichiarati,
        } => "successo",
        /// L'errore viaggia in un `Box`: e' molto piu' grande delle altre due
        /// varianti, e senza il `Box` ogni `Esito` — compresi i successi —
        /// occuperebbe la sua taglia. Sul filo non cambia nulla.
        Errore { errore: Box<ErroreSulFilo>, } => "errore",
        Panic { forma: FormaPanicSulFilo, } => "panic"
    }
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
/// # Una sola autorita', e non per disciplina
///
/// Il frame porta **solo il corpo**. La versione e' fissata internamente e il
/// tipo e' [derivato](Self::tipo) dal corpo: nessuno dei due e' un campo che
/// si possa impostare.
///
/// Tenendoli come campi pubblici indipendenti, il codificatore potrebbe
/// emettere un frame che il decoder rifiuta — una versione `2`, o un
/// `tipo: "saluto"` con dentro un `Annulla`. Nessuna verifica in `codifica`
/// li *eliminerebbe*: li intercetterebbe soltanto, cioe' li sposterebbe da
/// «impossibile» a «controllato».
///
/// # Solo `Serialize`
///
/// Si legge con `codifica::decodifica`, non con un derive: la direzione
/// filo → struttura e' una funzione che controlla, non una conversione che
/// riesce.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    corpo: Corpo,
}

impl Frame {
    /// L'unico modo di costruire un frame.
    #[must_use]
    pub const fn nuovo(corpo: Corpo) -> Self {
        Self { corpo }
    }

    /// Il corpo.
    #[must_use]
    pub const fn corpo(&self) -> &Corpo {
        &self.corpo
    }

    /// Il corpo, **consumando** il frame.
    ///
    /// Esiste perche' chi riceve un frame lo esaurisce: leggerlo per
    /// riferimento e poi clonarne il contenuto sarebbe una copia in piu' e,
    /// peggio, lascerebbe in giro un frame gia' consumato.
    #[must_use]
    pub fn in_corpo(self) -> Corpo {
        self.corpo
    }

    /// Il corpo, modificabile: **solo per i test**.
    ///
    /// Non riapre il difetto che i campi privati chiudono: il tipo resta una
    /// funzione del corpo, quindi cambiare il corpo cambia anche il tipo. Cio'
    /// che non esiste piu' e' la possibilita' di cambiarne *uno solo*.
    #[cfg(test)]
    pub(super) const fn corpo_mutabile(&mut self) -> &mut Corpo {
        &mut self.corpo
    }

    /// Il tipo, **dedotto** dal corpo.
    ///
    /// Non e' un campo da tenere allineato: e' una funzione del corpo, quindi
    /// non esiste uno stato in cui i due si contraddicono.
    #[must_use]
    pub const fn tipo(&self) -> TipoMessaggio {
        match &self.corpo {
            Corpo::Saluto(_) => TipoMessaggio::Saluto,
            Corpo::Incarico(_) => TipoMessaggio::Incarico,
            Corpo::Annulla(_) => TipoMessaggio::Annulla,
            Corpo::Risposta(_) => TipoMessaggio::Risposta,
            Corpo::Progresso(_) => TipoMessaggio::Progresso,
            Corpo::Esito(_) => TipoMessaggio::Esito,
        }
    }
}

/// Emette i tre campi dell'involucro: la versione dalla costante, il tipo dal
/// corpo, e il corpo nudo.
///
/// Scritta a mano e non derivata perche' due dei tre campi **non sono campi**:
/// se lo fossero, tornerebbe la possibilita' di impostarli male.
impl Serialize for Frame {
    fn serialize<S: serde::Serializer>(&self, serializzatore: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;

        let mut involucro = serializzatore.serialize_struct("Frame", 3)?;
        involucro.serialize_field("protocol_version", &VERSIONE_PROTOCOLLO)?;
        involucro.serialize_field("tipo", &self.tipo())?;
        involucro.serialize_field("corpo", &self.corpo)?;
        involucro.end()
    }
}
