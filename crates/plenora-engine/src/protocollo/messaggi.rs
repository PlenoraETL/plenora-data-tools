//! I messaggi del protocollo, come **forma serializzata**.
//!
//! `deny_unknown_fields` a ogni livello, nessun `serde_json::Value`, nessuna
//! mappa aperta: cio' che non e' riconosciuto e' un errore **visibile**, non
//! un silenzio.
//!
//! [`Incarico::piano_canonico`] e' l'unico campo non tipizzato, perche' il
//! piano ha validatore, versione e hash propri. E' [`RawValue`], JSON
//! **grezzo**: una sola serializzazione, misurata e spedita, senza riscrivere
//! i numeri su cui e' calcolato il `plan_hash` ne' collassare chiavi
//! duplicate. Che sia un piano con l'hash [`Incarico::plan_hash_atteso`] lo
//! verifica il worker.
//!
//! La **semantica** (digest, resolver, capability) appartiene al supervisore.
//! Sul filo viaggia la proiezione di [`EsitoWorker`], non l'esito
//! classificato, che nasce nel supervisore con l'evidenza del sistema
//! operativo.

use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;

use super::digest::DigestSha256;
use crate::commit_token::CommitToken;

/// Versione del protocollo.
///
/// Sta **solo** nell'involucro di ogni frame: `Saluto` e `Risposta` non ne
/// portano una seconda copia che possa divergere.
pub const VERSIONE_PROTOCOLLO: u16 = 1;

/// Genera **insieme** l'enum, il suo nome sul filo e l'insieme di tutte le sue
/// varianti.
///
/// Una tabella di prova scritta a mano accanto all'enum enumera se stessa: una
/// variante nuova ne resterebbe fuori a test verde. Qui la lista e' una sola,
/// e ogni variante ha un nome sul filo e compare in [`TUTTE`](Self::TUTTE).
/// Che i nomi siano distinti e rileggano la propria variante lo provano i
/// test.
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
            /// Generata dalla stessa lista che genera le varianti.
            ///
            /// Solo `cfg(test)`: la enumerano i casi che attraversano ogni
            /// variante; la produzione ne converte una per volta.
            #[cfg(test)]
            pub const TUTTE: &'static [(Self, &'static str)] = &[
                $( (Self::$variante, $filo), )+
            ];
        }
    };
}

/// Come [`enum_sul_filo!`], ma per gli enum **con tag interno**, le cui
/// varianti possono avere campi.
///
/// **Con** `= valore` genera anche `TUTTE`, un campione per variante
/// costruito dal compilatore; **senza** (campioni non costanti, come un
/// `DigestArtefatto` con `String`) genera i soli `NOMI`, che bastano perche'
/// il test pretenda un caso per ciascuna variante.
///
/// Le graffe sono obbligatorie anche senza campi (`Never {}`): in un enum con
/// tag interno `deny_unknown_fields` **non copre le varianti unitarie**, e la
/// macro rende quella forma non scrivibile.
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
            ///
            /// Solo `cfg(test)`: la enumerano i casi che attraversano ogni
            /// variante; la produzione ne converte una per volta.
            #[cfg(test)]
            pub const NOMI: &'static [&'static str] = &[ $( $filo ),+ ];

            /// Un campione per variante, col proprio nome sul filo.
            ///
            /// Solo `cfg(test)`: la enumerano i casi che attraversano ogni
            /// variante; la produzione ne converte una per volta.
            #[cfg(test)]
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
            ///
            /// Solo `cfg(test)`: la enumerano i casi che attraversano ogni
            /// variante; la produzione ne converte una per volta.
            #[cfg(test)]
            pub const NOMI: &'static [&'static str] = &[ $( $filo ),+ ];
        }
    };
}

enum_sul_filo! {
    /// Il tipo di un messaggio, enumerazione **chiusa**.
    TipoMessaggio {
        Saluto => "saluto",
        Incarico => "incarico",
        IncaricoVerifica => "incarico_verifica",
        Annulla => "annulla",
        Risposta => "risposta",
        Progresso => "progresso",
        Esito => "esito",
        EsitoVerifica => "esito_verifica",
    }
}

impl TipoMessaggio {
    /// La direzione ammessa per questo tipo.
    #[must_use]
    pub const fn direzione(self) -> Direzione {
        match self {
            Self::Saluto | Self::Incarico | Self::IncaricoVerifica | Self::Annulla => {
                Direzione::VersoWorker
            }
            Self::Risposta | Self::Progresso | Self::Esito | Self::EsitoVerifica => {
                Direzione::VersoSupervisore
            }
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
/// fingerprint, invece di fidarsi di una descrizione altrui.
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

/// L'incarico del **verificatore**: che cosa l'artefatto deve risultare, e
/// quanto gli e' concesso di trattenere per accertarlo.
///
/// # Perche' un messaggio distinto
///
/// Il verificatore non esegue un piano: non riceve `piano_canonico` ne'
/// `ingressi` (`GA-5`), e non deve poter dedurre da dove l'artefatto viene ne'
/// quale piano lo ha prodotto. Un `Incarico` con campi opzionali renderebbe
/// rappresentabile un `Incarico` senza piano.
///
/// # Che cosa porta
///
/// Gli elementi di
/// `isolamento.md#2-ter-la-verifica-non-può-stare-fuori-dal-limite`, gli
/// stessi di [`AtteseVerifica`](crate::verifica::AtteseVerifica) in processo.
/// Il `commit_token` **non** c'e': e' gia' nel `Saluto` (`§4.3`). Il tetto di
/// memoria viaggia come valore singolo, da cui il verificatore deriva
/// `IpcLimits` con la stessa funzione pura
/// (`ipc_boundary::limits_from_memory_budget`) del coordinatore.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IncaricoVerifica {
    /// Verifica come [`DescrittoreIngresso::contract_fingerprint_atteso`]:
    /// l'uscita testuale di uno SHA-256 a 32 byte, nella stessa forma
    /// canonica.
    pub contract_fingerprint_atteso: DigestSha256,
    /// Il digest che l'artefatto deve rendere (passo 5-bis).
    pub digest_atteso: DigestArtefatto,
    /// Righe e batch che l'artefatto deve contenere (passo 8).
    pub conteggi_attesi: ConteggiDichiarati,
    /// Il budget di memoria governata da cui derivare i tetti del confine
    /// ostile Arrow IPC — la stessa quantita' che
    /// `ipc_boundary::limits_from_memory_budget` gia' riceve nel percorso in
    /// processo.
    pub budget_memoria_governata_bytes: u64,
}

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
///
/// # I contatori sono **totali**, non incrementi
///
/// Ogni `Progresso` dichiara quanto si e' fatto **fin li'**: i tre assi sono
/// **non decrescenti**, e un valore piu' piccolo del precedente e' una
/// violazione del protocollo. Chi riceve conserva l'ultimo e non somma:
/// nessun traboccamento su `u64` da saturare o da cui andare in panico, e un
/// rapporto perso o ripetuto non falsa il conto. Il contratto e' scritto
/// anche in `isolamento.md`.
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

enum_con_tag_sul_filo! {
    /// Asse «ritentativo».
    ///
    /// `delay_ms` e' ammesso **esclusivamente** con [`Self::After`]: un ritardo su
    /// una disposizione che non lo prevede direbbe al chiamante di riprovare piu'
    /// tardi senza che nulla glielo abbia concesso.
    ///
    /// # Perche' `Never {}`
    ///
    /// In un enum con tag interno `deny_unknown_fields` **non ha effetto sulle
    /// varianti unitarie**: `Never` accetterebbe `{"kind":"never","delay_ms":10}`
    /// scartando `delay_ms`. `Never {}` e' identica sul filo ma passa per un
    /// visitor di struttura, dove `deny_unknown_fields` vale.
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
/// # Perche' sono nel `Successo`
///
/// Sono il termine di paragone del passo 8 della verifica (§7 di
/// `isolamento.md`). Il digest non li sostituisce: un worker fermo a meta' che
/// finalizzasse comunque produrrebbe un artefatto integro e **incompleto**.
///
/// Non c'e' `nodi_completati`, a differenza di `Progresso`: rileggendo un file
/// Arrow IPC si osservano righe e batch, non nodi, e il verificatore non
/// potrebbe confrontarlo.
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

enum_con_tag_sul_filo! {
    /// L'esito che il worker dichiara **di se'**.
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

enum_con_tag_sul_filo! {
    /// L'esito che il **verificatore** dichiara di se'.
    ///
    /// Tipo distinto da `EsitoWorkerSulFilo` pur con la stessa forma: questo
    /// dice «ho riconfermato che l'artefatto e' questo», non «ho eseguito il
    /// piano» (§4.3, come `Incarico` e `IncaricoVerifica`). La macchina del
    /// supervisore (`isolamento::macchina`) resta una sola e riconosce il
    /// corpo che chiude il dialogo secondo `macchina::Ruolo`.
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    EsitoVerificaSulFilo, tag = "esito" {
        /// Il verificatore ha riconfermato l'artefatto: digest e conteggi
        /// **osservati**, non solo quelli attesi ripetuti — se il passo 5-bis
        /// o il passo 8 fallissero questa variante non si costruirebbe.
        Successo {
            digest_artefatto: DigestArtefatto,
            conteggi: ConteggiDichiarati,
        } => "successo",
        /// Stesso motivo del `Box` in `EsitoWorkerSulFilo::Errore`.
        Errore { errore: Box<ErroreSulFilo>, } => "errore",
        Panic { forma: FormaPanicSulFilo, } => "panic"
    }
}

/// Il corpo di un frame, scelto dal tipo.
///
/// # Serializza, non deserializza
///
/// `untagged` vale **solo in scrittura**: emette il corpo nudo, perche' il
/// tipo lo dichiara gia' [`Frame::tipo`]. In lettura `untagged` proverebbe le
/// varianti a turno; il decoder invece legge `tipo` **prima** e deserializza
/// il corpo in quel tipo, cosi' l'incoerenza e' un errore di forma.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum Corpo {
    Saluto(Box<Saluto>),
    Incarico(Box<Incarico>),
    IncaricoVerifica(Box<IncaricoVerifica>),
    Annulla(Annulla),
    Risposta(Box<Risposta>),
    Progresso(Progresso),
    Esito(Box<EsitoWorkerSulFilo>),
    EsitoVerifica(Box<EsitoVerificaSulFilo>),
}

/// L'involucro di ogni frame.
///
/// # Una sola autorita'
///
/// Il frame porta **solo il corpo**: la versione e' fissata e il tipo e'
/// [derivato](Self::tipo) dal corpo, cosi' il codificatore non puo' emettere
/// una versione `2` o un `tipo: "saluto"` con dentro un `Annulla`.
///
/// # Solo `Serialize`
///
/// Si legge con `codifica::decodifica`: la direzione filo → struttura e' una
/// funzione che controlla, non una conversione che riesce.
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
            Corpo::IncaricoVerifica(_) => TipoMessaggio::IncaricoVerifica,
            Corpo::Annulla(_) => TipoMessaggio::Annulla,
            Corpo::Risposta(_) => TipoMessaggio::Risposta,
            Corpo::Progresso(_) => TipoMessaggio::Progresso,
            Corpo::Esito(_) => TipoMessaggio::Esito,
            Corpo::EsitoVerifica(_) => TipoMessaggio::EsitoVerifica,
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
