//! L'errore unico del workspace, [`PlenoraError`], e i suoi assi.
//!
//! Regola: nessun dato negli errori. Il testo porta il contesto (passo,
//! operazione, colonna per nome, motivo), mai valori di righe o colonne,
//! nemmeno in diagnostica; gli indici e i conteggi delle righe rifiutate
//! viaggiano nel payload strutturato di [`PlenoraError::RowDiagnostics`].
//!
//! Ogni errore espone quattro assi indipendenti, con valori da enumerazioni
//! chiuse e nomi stabili: categoria ([`ErrorCategory`],
//! [`PlenoraError::category`]), fase ([`ErrorPhase`]), effetto restato sul
//! supporto ([`RemoteEffect`]) e disposizione al ritentativo
//! ([`RetryDisposition`]). Un booleano `retryable()` non basterebbe: un
//! timeout in lettura è ritentabile, lo stesso timeout dopo l'invio di un
//! commit no.
//!
//! La fase derivata dalla variante si raffina con [`PlenoraError::Tagged`]
//! dove chi produce l'errore conosce il momento esatto; gli altri assi
//! restano delegati alla sorgente. Oggi nessun codice del workspace tagga:
//! i rifiuti per riga dei kernel (conversioni, divisioni per zero,
//! asserzioni) nascono eseguendo un passo, non leggendo un supporto, e
//! prendono la fase derivata, [`ErrorPhase::Write`].
//!
//! Alcune varianti vengono dall'engine, dalla CLI e dal trasporto fra
//! processi di `plenora-data-tools` e qui nessun codice le produce
//! (`Execution`, `Protocol`): lo dice il rustdoc di ciascuna. `Cancelled` e
//! `Timeout` le produce il runner (`plenora-pipeline`), al controllo
//! cooperativo fra i passi.
//!
//! La forma pubblica, `plenora-error-v1`, è [`PlenoraError::public_projection`]
//! ([`PublicError`]): i quattro assi, un `code` stabile dove l'errore ne ha
//! uno, il messaggio limitato e la diagnostica per riga in
//! `details.row_diagnostics`.

use std::fmt;
use std::time::Duration;

use thiserror::Error;

use crate::diagnostics::RowDiagnostics;

mod pubblico;

pub use pubblico::{
    PublicError, CODE_CANCELLED, CODE_DEADLINE_EXCEEDED, CODE_DETAILS_NOT_PUBLISHABLE,
    MAX_DETAILS_BYTES, MAX_DETAILS_DEPTH, MAX_DETAILS_FANOUT, MAX_DETAILS_NODES,
    MAX_DETAILS_STRING_BYTES, MAX_ERROR_BYTES, MAX_MESSAGE_CHARS, MAX_RETRY_DELAY_MS,
};

/// Errore unico del workspace.
///
/// I nomi delle varianti seguono le categorie canoniche ([`ErrorCategory`]).
/// I testi `Display` ("contract violation", "step failed at node", "arrow
/// error", ...) non seguono i nomi delle varianti e sono stabili per chi
/// legge il testo.
///
/// Approssimazione dichiarata: `DataMapping` fonde errori JSON e Arrow e
/// perde la sorgente tipizzata (resta nel testo) e la distinzione di fase a
/// livello di variante, recuperata dove serve con [`PlenoraError::Tagged`].
#[derive(Debug, Error)]
pub enum PlenoraError {
    /// Piano o configurazione di un nodo malformati o incoerenti.
    #[error("contract violation: {0}")]
    InvalidPlan(String),

    /// Operazione o caso non supportato: id sconosciuto, operazione che il
    /// runner non esegue, parametro o tipo che il kernel non tratta,
    /// geometria oltre la precisione dichiarata dove l'errore attraversa il
    /// confine con `PlenoraError`.
    #[error("unsupported operation: {0}")]
    Unsupported(String),

    /// Violazione di schema Arrow o di `DataContract`.
    #[error("schema violation: {0}")]
    Schema(String),

    /// Un valore non è rappresentabile nella destinazione: errore Arrow
    /// (solo il codice della variante, [`arrow_error_code`]), lettura JSON
    /// di piano o config, valore di cella rifiutato da un kernel.
    #[error("{0}")]
    DataMapping(String),

    /// Fallimento di un nodo durante l'esecuzione di un DAG.
    ///
    /// Nessun codice di questo repository la produce: la costruiva l'executor
    /// di `plenora-data-tools`, che riempiva `execution_id` al confine del
    /// nodo. Con `execution_id` vuoto il `Display` lo omette.
    #[error(
        "step failed at node `{node}` (operation `{operation}`{}): {reason}",
        execution_suffix(execution_id)
    )]
    Execution {
        node: String,
        operation: String,
        execution_id: String,
        reason: String,
    },

    /// Errore CRS: identificatore non risolvibile (anche un codice fuori
    /// dalla tabella integrata o una definizione WKT/PROJJSON), requisito
    /// dell'operazione non soddisfatto, coordinate fuori dominio, percorso
    /// di riproiezione rifiutato.
    #[error("CRS error: {0}")]
    Crs(String),

    /// Errore CRS con il codice stabile di [`crate::crs::CrsError`]
    /// (`code`, il `code` di `plenora-error-v1`): lo produce la conversione
    /// da `CrsError`. Stesso testo, categoria e assi di
    /// [`PlenoraError::Crs`], che resta per i messaggi senza codice.
    #[error("CRS error: {message}")]
    CrsCoded {
        /// [`crate::crs::CrsError::code`]: valido per costruzione.
        code: crate::crs::CodiceCrs,
        /// Il messaggio, con il codice in testa e gli eventuali contesti.
        message: String,
    },

    /// Esecuzione annullata dal chiamante: il segnale di annullamento del
    /// runner (`plenora_pipeline::Interruzione`) era alzato a un controllo
    /// fra i passi. Il testo dice dove (passo e operazione, o la consegna
    /// degli output), mai dati. Codice pubblico `EXECUTION_CANCELLED`.
    #[error("cancelled: {0}")]
    Cancelled(String),

    /// Limite di risorsa superato: righe, byte in memoria, byte temporanei,
    /// fattore di espansione, lavoro massimo di un kernel.
    ///
    /// Distinta da [`PlenoraError::InvalidPlan`]: qui il piano è corretto e
    /// sono i dati a non entrare nel budget, quindi chi orchestra rilancia con
    /// più budget o meno dati invece di correggere il piano. È l'unica
    /// variante con categoria `resource_limit`.
    #[error("resource limit: {0}")]
    ResourceLimit(String),

    /// Errore di I/O.
    ///
    /// Categoria e ritentativo dipendono dall'`ErrorKind` ([`assi_io`]). Il
    /// testo del sistema operativo può contenere percorsi: resta nel
    /// `Display` locale, mai nel messaggio pubblico
    /// ([`PlenoraError::public_projection`]), che porta un testo fisso per
    /// `ErrorKind` e il solo contesto nostro ([`PlenoraError::io_con_contesto`]).
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    /// Scambio malformato o fuori sequenza su un canale tra processi.
    ///
    /// Nessun codice di questo repository la produce: il canale era quello
    /// dell'esecuzione isolata di `plenora-data-tools`. Riguarda la
    /// conversazione, non il supporto: un canale che si chiude a metà è
    /// [`PlenoraError::Io`]. Testo strutturale (quale confine, quale attesa),
    /// mai il contenuto del messaggio.
    #[error("protocol error: {0}")]
    Protocol(String),

    /// La scadenza dell'esecuzione è passata: il runner l'ha trovata
    /// superata a un controllo fra i passi
    /// (`plenora_pipeline::Interruzione`), prima di eseguire il passo
    /// successivo o di consegnare gli output. Il testo dice dove. Codice
    /// pubblico `EXECUTION_DEADLINE_EXCEEDED`.
    #[error("timeout: {0}")]
    Timeout(String),

    /// La destinazione di una scrittura è occupata.
    ///
    /// Il caso concreto è la scrittura atomica di `plenora-io` senza
    /// sovrascrittura: la destinazione esiste al controllo anticipato, o è
    /// comparsa fra il controllo e la rinomina, o è il file di un altro
    /// output appena scritto dallo stesso piano. Distinta da
    /// [`PlenoraError::Io`] perché non c'è nulla di rotto (c'è qualcun
    /// altro), e da `InvalidPlan` perché lo stesso piano, su una destinazione
    /// libera, è corretto.
    #[error("conflict: {0}")]
    Conflict(String),

    /// La configurazione dell'ambiente, non il piano, è incoerente.
    ///
    /// Il piano descrive **che cosa** calcolare ed è portabile, la
    /// configurazione **dove**: si correggono in posti diversi, e la
    /// categoria lo dice. Oggi la produce `geo.reproject` per una config non
    /// leggibile o un percorso di griglia `NTv2` non valido.
    #[error("invalid configuration: {0}")]
    InvalidConfiguration(String),

    /// Invariante interna violata: uno stato che per costruzione non
    /// dovrebbe esistere. Sta al posto delle primitive di panico
    /// (`unreachable!`, `expect`) nei punti in cui il compilatore non può
    /// dimostrare l'esaustività: il caso "impossibile" diventa un errore
    /// esplicito, mai un panico. Il testo porta il contesto strutturale, mai
    /// valori di righe o colonne.
    #[error("internal error: {0}")]
    Internal(String),

    /// Errore con diagnostica per riga (`plenora-row-diagnostics-v1`):
    /// righe rifiutate, contate per codice, con esempi limitati (indice di
    /// riga e codice, mai il valore). Si costruisce con
    /// [`PlenoraError::with_row_diagnostics`].
    #[error("{source}")]
    RowDiagnostics {
        /// Causa primaria; testo e assi restano invariati.
        source: Box<Self>,
        /// Payload strutturato, di dimensione limitata.
        diagnostics: Box<RowDiagnostics>,
    },

    /// Errore con effetto sul supporto esplicito, assegnato da chi sa che
    /// una parte dell'effetto è già visibile (oggi solo `esegui_da_file` di
    /// `plenora-io`, dopo il primo output scritto: `Partial`).
    ///
    /// Wrapper trasparente come [`PlenoraError::Tagged`]: testo, categoria e
    /// fase delegati; [`PlenoraError::remote_effect`] e
    /// [`PlenoraError::retry_disposition`] ne tengono conto. Si costruisce con
    /// [`PlenoraError::with_remote_effect`].
    #[error("{source}")]
    WithRemoteEffect {
        /// Effetto dichiarato dal confine.
        remote_effect: RemoteEffect,
        /// Errore originale.
        source: Box<Self>,
    },

    /// Errore con fase esplicita, assegnata da chi lo ha prodotto.
    ///
    /// Wrapper trasparente: `Display`, categoria, effetto e disposizione sono
    /// delegati alla sorgente; solo [`PlenoraError::phase`] e' raffinato. Si
    /// costruisce con [`PlenoraError::with_phase`]: il primo tag vince e non si
    /// annida.
    #[error("{source}")]
    Tagged {
        /// Fase dichiarata dal confine (sovrascrive la derivazione per
        /// variante).
        phase: ErrorPhase,
        /// Errore originale: testo e assi diversi dalla fase invariati.
        source: Box<Self>,
    },
}

/// Codice stabile della variante di un [`arrow_schema::ArrowError`].
///
/// I messaggi di arrow-rs citano spesso il valore che ha causato il difetto
/// (`Cannot cast string '<valore>' to Int64`): farli passare violerebbe la
/// regola «errori senza dati», e la privacy dipenderebbe da una libreria
/// esterna invece che dalla nostra costruzione. Si conserva quindi la sola
/// **variante**, che dice il genere di difetto senza dire su quale dato.
///
/// Il `match` e' esaustivo e `arrow-schema` e' pinnato a una versione esatta:
/// una variante nuova non compila, invece di cadere su un ramo generico.
#[must_use]
pub const fn arrow_error_code(error: &arrow_schema::ArrowError) -> &'static str {
    use arrow_schema::ArrowError as E;
    match error {
        E::NotYetImplemented(_) => "not_yet_implemented",
        E::ExternalError(_) => "external",
        E::CastError(_) => "cast",
        E::MemoryError(_) => "memory",
        E::ParseError(_) => "parse",
        E::SchemaError(_) => "schema",
        E::ComputeError(_) => "compute",
        E::DivideByZero => "divide_by_zero",
        E::ArithmeticOverflow(_) => "arithmetic_overflow",
        E::CsvError(_) => "csv",
        E::JsonError(_) => "json",
        E::AvroError(_) => "avro",
        E::IoError(_, _) => "io",
        E::IpcError(_) => "ipc",
        E::InvalidArgumentError(_) => "invalid_argument",
        E::ParquetError(_) => "parquet",
        E::CDataInterface(_) => "c_data_interface",
        E::DictionaryKeyOverflowError => "dictionary_key_overflow",
        E::RunEndIndexOverflowError => "run_end_index_overflow",
        E::OffsetOverflowError(_) => "offset_overflow",
    }
}

impl From<arrow_schema::ArrowError> for PlenoraError {
    fn from(error: arrow_schema::ArrowError) -> Self {
        // Prefisso `arrow error: ` seguito da un codice scritto da noi, non
        // dal testo della dipendenza. Vedi [`arrow_error_code`].
        Self::DataMapping(format!("arrow error: {}", arrow_error_code(&error)))
    }
}

impl From<serde_json::Error> for PlenoraError {
    fn from(error: serde_json::Error) -> Self {
        // Il testo è quello di serde_json: posizione e genere del difetto e,
        // per un tipo sbagliato, il valore letto. In forma tipizzata si
        // leggono solo piani e config, i cui valori sono parametri, non dati
        // di riga.
        Self::DataMapping(format!("json error: {error}"))
    }
}

/// Genera insieme l'enum delle categorie, l'elenco completo, l'indice e il
/// nome stabile: **una sola dichiarazione**, quattro derivati.
///
/// Un `match` esaustivo obbliga ad aggiungere un braccio per ogni variante
/// nuova, ma nessuno obbliga ad aggiungerla a un elenco come `ALL`, e i test
/// che iterano `ALL` resterebbero verdi. Nati dalla stessa lista, elenco e
/// indice non possono divergere.
macro_rules! categorie_errore {
    (
        $(
            $(#[$attributo:meta])*
            $variante:ident => $nome:literal
        ),+ $(,)?
    ) => {
        /// Categoria stabile di un [`PlenoraError`].
        ///
        /// Le categorie canoniche del contratto d'errore di
        /// `plenora-data-tools`, senza valori propri. Le due estensioni
        /// locali dell'esecuzione isolata di quel progetto non ci sono:
        /// l'isolamento non è in questo workspace. `NotFound`,
        /// `Authentication`, `Authorization` e `Transient` non hanno una
        /// variante di [`PlenoraError`] che le produca: restano perché
        /// l'enumerazione è quella canonica.
        ///
        /// Pensata per telemetria e report machine-readable, non per il
        /// controllo di flusso (per quello ci sono le varianti). Enum,
        /// [`ErrorCategory::ALL`], [`ErrorCategory::index`] e
        /// [`ErrorCategory::as_str`] nascono dalla macro `categorie_errore`.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum ErrorCategory {
            $(
                $(#[$attributo])*
                $variante,
            )+
        }

        impl ErrorCategory {
            /// Elenco completo delle categorie **supportate**, in ordine
            /// di dichiarazione.
            ///
            /// Nasce dalla stessa lista dell'enum.
            pub const ALL: &'static [Self] = &[$(Self::$variante),+];

            /// Nome stabile **pubblico** della categoria (telemetria,
            /// report JSON), in `snake_case`.
            ///
            /// È il nome canonico della categoria. Cambiarlo rompe chi
            /// legge i report.
            #[must_use]
            pub const fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variante => $nome,)+
                }
            }

            /// Categoria dal nome stabile, l'inverso di [`Self::as_str`].
            ///
            /// `None` per una stringa che non è una categoria (un report di
            /// un'altra versione, o corrotto).
            #[must_use]
            pub fn from_stable_name(nome: &str) -> Option<Self> {
                match nome {
                    $($nome => Some(Self::$variante),)+
                    _ => None,
                }
            }

            /// Posizione della categoria in [`Self::ALL`].
            #[must_use]
            pub const fn index(self) -> usize {
                let mut posizione = 0;
                $(
                    if matches!(self, Self::$variante) {
                        return posizione;
                    }
                    posizione += 1;
                )+
                posizione
            }
        }
    };
}

categorie_errore! {
    /// Piano o configurazione malformati o incoerenti.
    InvalidPlan => "invalid_plan",
    /// Configurazione del componente invalida.
    InvalidConfiguration => "invalid_configuration",
    /// Schema Arrow o contratto dati incoerente.
    Schema => "schema",
    /// Un valore non e' rappresentabile nella destinazione.
    DataMapping => "data_mapping",
    /// CRS assente, irrisolto o incoerente.
    Crs => "crs",
    /// Capability non offerta dal componente.
    Unsupported => "unsupported",
    /// Risorsa, layer o tabella inesistente.
    NotFound => "not_found",
    /// Destinazione gia' esistente o conflitto di scrittura.
    Conflict => "conflict",
    /// La risorsa è cambiata fra lettura e scrittura. Nessuna variante di
    /// [`PlenoraError`] la produce: c'è perché l'enumerazione di
    /// `plenora-error-v1` la contiene.
    ConcurrentModification => "concurrent_modification",
    /// Credenziali assenti o rifiutate.
    Authentication => "authentication",
    /// Permessi insufficienti.
    Authorization => "authorization",
    /// Scadenza superata.
    Timeout => "timeout",
    /// Annullato dal chiamante.
    Cancelled => "cancelled",
    /// Limite di byte, righe, profondita' o quota superato.
    ResourceLimit => "resource_limit",
    /// Errore del filesystem o del dispositivo.
    Io => "io",
    /// Violazione del protocollo di trasporto o di rete.
    Protocol => "protocol",
    /// Condizione temporanea, ritentabile per natura.
    Transient => "transient",
    /// Fallimento di un nodo durante la trasformazione.
    Execution => "execution",
    /// Invariante interna violata.
    Internal => "internal",
}

/// Segnaposto per il carico di una variante nei `match` generati: il nome
/// stabile non dipende dal payload, quindi il pattern lo ignora.
macro_rules! carico_ignorato {
    ($carico:ty) => {
        _
    };
}

/// Genera insieme l'enum di un asse canonico dell'errore, il nome stabile di
/// ogni valore e l'elenco completo delle varianti: **una sola
/// dichiarazione**, tre derivati.
///
/// Il compilatore obbliga il `match` di `as_str` a coprire ogni variante, ma
/// non un elenco scritto a mano: una variante mancante li' sparirebbe dagli
/// oracoli che lo iterano, senza che nulla fallisca.
macro_rules! asse_canonico {
    (
        $(#[$meta_enum:meta])*
        $nome_enum:ident => $elenco:ident, nota_nome_stabile = $nota:literal {
            $(
                $(#[$attributo:meta])*
                $variante:ident $(($carico:ty) = $rappresentante:expr)? => $nome:literal
            ),+ $(,)?
        }
    ) => {
        $(#[$meta_enum])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum $nome_enum {
            $(
                $(#[$attributo])*
                $variante $(($carico))?,
            )+
        }

        impl $nome_enum {
            /// Nome stabile del valore (telemetria, report JSON), in
            /// `snake_case`.
            ///
            #[doc = $nota]
            ///
            /// Generato dalla stessa lista dell'enum: un nome nuovo non puo'
            /// mancare ne' divergere dalle varianti.
            #[must_use]
            pub const fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variante $((carico_ignorato!($carico)))? => $nome,)+
                }
            }

            /// Elenco completo delle varianti dichiarate, in ordine di
            /// dichiarazione; le varianti con un carico compaiono con un
            /// rappresentante.
            ///
            /// Privato e sotto `cfg(test)`: serve agli oracoli per pretendere
            /// che le proprie tabelle nominino ogni variante.
            #[cfg(test)]
            const $elenco: &'static [Self] = &[
                $(asse_canonico!(@valore Self::$variante $(, $rappresentante)?)),+
            ];
        }

        impl fmt::Display for $nome_enum {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.as_str())
            }
        }
    };
    (@valore $variante:expr) => {
        $variante
    };
    (@valore $variante:expr, $rappresentante:expr) => {
        $rappresentante
    };
}

asse_canonico! {
    /// Fase del ciclo dell'operazione in cui l'errore è nato: l'asse «fase».
    ///
    /// Enumerazione canonica: il componente ne usa un sottoinsieme e non ne
    /// definisce di proprie. Non esiste una fase «Execute»: l'esecuzione dei
    /// kernel ricade in [`ErrorPhase::Write`] (vedi [`PlenoraError::phase`]).
    /// Sui bordi del filesystem: `Connect` = acquisizione dell'handle,
    /// `Probe` = ispezione preliminare del formato, `Commit` = rinomina della
    /// scrittura atomica.
    ///
    /// Enum, [`ErrorPhase::as_str`] ed elenco delle varianti nascono dalla
    /// macro `asse_canonico`.
    ErrorPhase => FASI_DICHIARATE, nota_nome_stabile = "`snake_case` canonico." {
        /// Validazione: lettura del piano (JSON), contratti, schema, CRS,
        /// limiti.
        Validate => "validate",
        /// Acquisizione dell'handle sulla risorsa (bordo del filesystem).
        Connect => "connect",
        /// Ispezione preliminare del formato o della risorsa di destinazione.
        Probe => "probe",
        /// Preparazione di kernel e risorse prima dell'esecuzione.
        Prepare => "prepare",
        /// Lettura dei dati di input dal supporto.
        Read => "read",
        /// Produzione dell'uscita: esecuzione dei kernel e scrittura del file
        /// temporaneo.
        Write => "write",
        /// Finalizzazione dello stream d'uscita (chiusura dello scrittore).
        Finalize => "finalize",
        /// Commit dell'effetto: rinomina della scrittura atomica.
        Commit => "commit",
        /// Annullamento dell'effetto, con conferma.
        Rollback => "rollback",
        /// Pulizia di risorse e residui.
        Cleanup => "cleanup",
    }
}

asse_canonico! {
    /// Effetto restato sul sistema remoto o sul supporto quando l'operazione
    /// riporta l'esito: l'asse «effetto», enumerazione canonica.
    ///
    /// L'esito ignoto non è una categoria d'errore ma
    /// [`RemoteEffect::Unknown`]. Un [`PlenoraError`] ha sempre effetto
    /// [`RemoteEffect::None`] (vedi [`PlenoraError::remote_effect`]).
    ///
    /// Enum, [`RemoteEffect::as_str`] ed elenco delle varianti nascono dalla
    /// macro `asse_canonico`.
    RemoteEffect => EFFETTI_DICHIARATI, nota_nome_stabile = "`snake_case` canonico." {
        /// L'operazione non ha prodotto alcun effetto osservabile.
        None => "none",
        /// L'effetto e' stato annullato, con conferma.
        RolledBack => "rolled_back",
        /// Una parte dell'effetto e' visibile e una no.
        Partial => "partial",
        /// L'effetto e' definitivo, benche' l'operazione riporti un errore.
        Committed => "committed",
        /// L'effetto non e' determinabile con i mezzi disponibili.
        Unknown => "unknown",
    }
}

asse_canonico! {
    /// Disposizione al ritentativo di un'operazione fallita: l'asse
    /// «ritentativo», enumerazione canonica.
    ///
    /// Calcolata da fase, effetto e idempotenza, mai dalla sola categoria, in
    /// [`PlenoraError::retry_disposition`].
    ///
    /// Enum, [`RetryDisposition::as_str`] ed elenco delle varianti nascono
    /// dalla macro `asse_canonico`.
    RetryDisposition => DISPOSIZIONI_DICHIARATE,
    nota_nome_stabile = "Per [`RetryDisposition::After`] e' il solo nome del \
                         valore (`after`); la durata e' esposta da \
                         [`RetryDisposition::delay`]." {
        /// Ritentare e' sempre errato (causa deterministica o volontaria).
        Never => "never",
        /// L'effetto e' ignoto e la risorsa va messa da parte (non riusata)
        /// finche' lo stato non si accerta. Nessun errore di questo
        /// workspace la produce: c'e' perche' l'enumerazione di
        /// `plenora-error-v1` la contiene.
        Quarantine => "quarantine",
        /// L'operazione e' idempotente o priva di effetti: si puo' ritentare.
        Safe => "safe",
        /// Ritentabile solo con una chiave che deduplichi l'effetto.
        RequiresIdempotencyKey => "requires_idempotency_key",
        /// Prima di ritentare occorre accertare lo stato reale.
        RequiresRecovery => "requires_recovery",
        /// Ritentabile non prima della durata indicata.
        ///
        /// Nell'elenco delle varianti dichiarate compare con una durata
        /// qualunque: e' il valore a essere enumerato, non il ritardo.
        After(Duration) = Self::After(Duration::from_millis(0)) => "after",
    }
}

impl RetryDisposition {
    /// Durata minima prima del retry: presente solo per
    /// [`RetryDisposition::After`], `None` per gli altri valori.
    #[must_use]
    pub const fn delay(self) -> Option<Duration> {
        match self {
            Self::After(duration) => Some(duration),
            Self::Never
            | Self::Quarantine
            | Self::Safe
            | Self::RequiresIdempotencyKey
            | Self::RequiresRecovery => None,
        }
    }
}

impl PlenoraError {
    /// Antepone un contesto (`"{contesto}: {messaggio}"`) al messaggio di
    /// `InvalidPlan`, `Unsupported`, `Schema`, `Crs` e `CrsCoded`, i cui
    /// testi sono nostri (anche sotto `WithRemoteEffect`); la variante resta
    /// la stessa.
    ///
    /// Lo usano il runner (nome dell'input) e `plenora-io` (nome dell'input
    /// o dell'output, colonna geometrica). Il `match` non ha ramo di
    /// default: una variante nuova obbliga a decidere se il contesto le si
    /// applica, invece di perderlo in silenzio.
    ///
    /// Tornano invariate tutte le altre: `Io` (errore del sistema
    /// operativo); `Execution`, `RowDiagnostics` e `Tagged`, che portano già
    /// un'attribuzione strutturata; `Internal`, dove il contesto utile è il
    /// punto del codice; `DataMapping`, `Cancelled`, `ResourceLimit`,
    /// `Protocol`, `Timeout`, `Conflict` e `InvalidConfiguration`, che chi
    /// vuole un contesto compone da sé.
    #[must_use]
    pub fn con_contesto(self, contesto: &str) -> Self {
        let anteponi = |messaggio: String| format!("{contesto}: {messaggio}");
        match self {
            Self::InvalidPlan(messaggio) => Self::InvalidPlan(anteponi(messaggio)),
            Self::Unsupported(messaggio) => Self::Unsupported(anteponi(messaggio)),
            Self::Schema(messaggio) => Self::Schema(anteponi(messaggio)),
            Self::Crs(messaggio) => Self::Crs(anteponi(messaggio)),
            Self::CrsCoded { code, message } => Self::CrsCoded {
                code,
                message: anteponi(message),
            },
            Self::WithRemoteEffect {
                remote_effect,
                source,
            } => Self::WithRemoteEffect {
                remote_effect,
                source: Box::new(source.con_contesto(contesto)),
            },
            altro @ (Self::DataMapping(_)
            | Self::Execution { .. }
            | Self::Cancelled(_)
            | Self::ResourceLimit(_)
            | Self::Io(_)
            | Self::Protocol(_)
            | Self::Timeout(_)
            | Self::Conflict(_)
            | Self::InvalidConfiguration(_)
            | Self::Internal(_)
            | Self::RowDiagnostics { .. }
            | Self::Tagged { .. }) => altro,
        }
    }

    /// Categoria dell'errore, dichiarata per variante (per `Io`
    /// dall'`ErrorKind`, [`assi_io`]). Per i wrapper è delegata alla
    /// sorgente.
    #[must_use]
    pub fn category(&self) -> ErrorCategory {
        match self {
            Self::InvalidPlan(_) => ErrorCategory::InvalidPlan,
            Self::Unsupported(_) => ErrorCategory::Unsupported,
            Self::Schema(_) => ErrorCategory::Schema,
            Self::DataMapping(_) => ErrorCategory::DataMapping,
            Self::Execution { .. } => ErrorCategory::Execution,
            Self::Crs(_) | Self::CrsCoded { .. } => ErrorCategory::Crs,
            Self::Cancelled(_) => ErrorCategory::Cancelled,
            Self::ResourceLimit(_) => ErrorCategory::ResourceLimit,
            Self::Io(errore) => assi_io(errore.kind()).0,
            Self::Protocol(_) => ErrorCategory::Protocol,
            Self::Timeout(_) => ErrorCategory::Timeout,
            Self::Conflict(_) => ErrorCategory::Conflict,
            Self::InvalidConfiguration(_) => ErrorCategory::InvalidConfiguration,
            Self::Internal(_) => ErrorCategory::Internal,
            Self::Tagged { source, .. }
            | Self::RowDiagnostics { source, .. }
            | Self::WithRemoteEffect { source, .. } => source.category(),
        }
    }

    /// Disposizione al ritentativo (l'asse «ritentativo»), calcolata da
    /// fase, effetto e idempotenza, mai dalla sola categoria.
    ///
    /// - L'effetto è sempre [`RemoteEffect::None`] (la scrittura atomica non
    ///   lascia mai un file parziale alla destinazione) e la riesecuzione è
    ///   deterministica: nessun errore richiede
    ///   [`RetryDisposition::RequiresIdempotencyKey`] o
    ///   [`RetryDisposition::RequiresRecovery`], che restano per i componenti
    ///   con stato remoto.
    /// - Per gli errori di I/O dall'`ErrorKind` ([`assi_io`]): `Safe` solo
    ///   per le cause transitorie, `Never` per le deterministiche e per
    ///   quelle non classificate.
    /// - [`RetryDisposition::Safe`] per la scadenza e l'annullamento del
    ///   runner (`Timeout`, `Cancelled`): scattano prima di un passo o della
    ///   consegna degli output, nessun output è stato reso e la riesecuzione
    ///   è deterministica (come nei vettori `data-run-timeout-error` e
    ///   `io-read-cancelled-error` del contratto). Backoff, tentativi e la
    ///   scelta di ritentare dopo un annullamento voluto spettano al
    ///   chiamante.
    /// - [`RetryDisposition::Never`] per le cause deterministiche e
    ///   `Internal`.
    /// - [`RetryDisposition::RequiresRecovery`] al posto di un ritentativo
    ///   automatico quando [`PlenoraError::WithRemoteEffect`] dichiara un
    ///   effetto gia' visibile.
    /// - [`RetryDisposition::After`] e [`RetryDisposition::Quarantine`] non
    ///   sono mai prodotti.
    ///
    /// Il tag di fase ([`PlenoraError::Tagged`]) non cambia la disposizione.
    #[must_use]
    pub fn retry_disposition(&self) -> RetryDisposition {
        match self {
            Self::Io(errore) => assi_io(errore.kind()).1,
            Self::Timeout(_) | Self::Cancelled(_) => RetryDisposition::Safe,
            Self::InvalidPlan(_)
            | Self::Unsupported(_)
            | Self::Schema(_)
            | Self::DataMapping(_)
            | Self::Execution { .. }
            | Self::Crs(_)
            | Self::CrsCoded { .. }
            | Self::ResourceLimit(_)
            | Self::Protocol(_)
            | Self::Conflict(_)
            | Self::InvalidConfiguration(_)
            | Self::Internal(_) => RetryDisposition::Never,
            Self::Tagged { source, .. } | Self::RowDiagnostics { source, .. } => {
                source.retry_disposition()
            }
            // Un effetto gia' visibile (parziale, definitivo o ignoto) esclude
            // il ritentativo automatico: prima va accertato lo stato
            // (ERR-006 per `unknown`, la stessa regola per gli altri). Una
            // causa che non si ritenta mai resta `Never`.
            Self::WithRemoteEffect {
                remote_effect,
                source,
            } => match (remote_effect, source.retry_disposition()) {
                (RemoteEffect::None | RemoteEffect::RolledBack, retry)
                | (_, retry @ (RetryDisposition::Never | RetryDisposition::Quarantine)) => retry,
                (_, _) => RetryDisposition::RequiresRecovery,
            },
        }
    }

    /// Fase del ciclo in cui l'errore è nato (l'asse «fase»).
    ///
    /// Un errore taggato ([`PlenoraError::Tagged`]) riporta la fase che gli
    /// ha dato chi lo ha prodotto; uno non taggato quella derivata dalla
    /// variante. Oggi nessun codice del workspace tagga: kernel e runner
    /// lasciano la derivazione (un rifiuto per riga di un kernel è un
    /// `DataMapping`, quindi [`ErrorPhase::Write`], la fase dell'esecuzione),
    /// e `plenora-io` non tagga.
    ///
    /// Derivazione per variante, con le approssimazioni dichiarate:
    ///
    /// - `InvalidPlan`, `Unsupported`, `Schema`, `Crs` ->
    ///   [`ErrorPhase::Validate`], anche quando il controllo scatta durante
    ///   l'esecuzione;
    /// - `Execution`, `Cancelled`, `Timeout` -> [`ErrorPhase::Write`]: il
    ///   canone non ha «Execute», e questi errori nascono mentre il piano
    ///   produce le uscite;
    /// - `DataMapping`, `Io`, `Internal` non taggati -> [`ErrorPhase::Write`],
    ///   il lato con possibile effetto sul supporto (scelta conservativa; la
    ///   disposizione di retry non dipende comunque dalla fase);
    /// - `InvalidConfiguration` -> [`ErrorPhase::Prepare`], `Conflict` ->
    ///   [`ErrorPhase::Commit`] (anche quando `plenora-io` lo rileva al
    ///   controllo anticipato, prima di scrivere).
    #[must_use]
    pub const fn phase(&self) -> ErrorPhase {
        match self {
            // Bracci fusi per fase (stessa decisione documentata sopra per
            // ogni variante): l'esaustivita' e' preservata perche' tutte
            // le varianti restano nominate esplicitamente.
            Self::InvalidPlan(_)
            | Self::Unsupported(_)
            | Self::Schema(_)
            | Self::Crs(_)
            | Self::CrsCoded { .. } => ErrorPhase::Validate,
            Self::Execution { .. }
            | Self::Cancelled(_)
            | Self::DataMapping(_)
            | Self::Io(_)
            // `ResourceLimit` deriva `Write` come le altre varianti di
            // runtime, ma la fase VERA dipende da dove il limite scatta: chi
            // lo produce leggendo un input lo tagga `Read` con `with_phase`,
            // e il tag del confine vince sulla derivazione (vedi sotto).
            | Self::ResourceLimit(_)
            // `Protocol` deriva `Write` come `Io`, il lato con possibile
            // effetto. Chi conosce il confine raffina con `with_phase`.
            | Self::Protocol(_)
            | Self::Timeout(_)
            | Self::Internal(_) => ErrorPhase::Write,
            // La configurazione si decide PRIMA che qualunque dato si muova:
            // se fallisce, non e' stato letto ne' scritto nulla.
            Self::InvalidConfiguration(_) => ErrorPhase::Prepare,
            // Il conflitto e' per definizione al commit: e' li' che lo stato
            // osservato smentisce quello su cui si e' deciso.
            Self::Conflict(_) => ErrorPhase::Commit,
            // Il tag del confine vince sulla derivazione per variante.
            Self::Tagged { phase, .. } => *phase,
            Self::RowDiagnostics { source, .. } | Self::WithRemoteEffect { source, .. } => {
                source.phase()
            }
        }
    }

    /// Assegna la fase in cui l'errore è nato.
    ///
    /// Avvolge l'errore in [`PlenoraError::Tagged`]; `Display`, categoria,
    /// effetto e disposizione restano delegati. Se l'errore e' gia' taggato
    /// vince il tag esistente, il piu' vicino all'origine, e non si annida.
    ///
    /// Il tag va **sotto** i wrapper trasparenti: un errore gia' taggato dentro
    /// [`PlenoraError::RowDiagnostics`] (per esempio un confine di lettura
    /// che chiami `with_phase(Read)` su errori con diagnostica di riga) verrebbe
    /// altrimenti taggato di nuovo. La funzione applica il tag alla sorgente,
    /// e la forma canonica e' sempre
    ///
    /// ```text
    ///     RowDiagnostics -> Tagged(fase) -> causa
    /// ```
    ///
    /// Tutti gli assi e il payload sono invarianti; la struttura pubblica
    /// (`match`, `Debug`, catena di [`std::error::Error::source`]) è invece
    /// osservabile. `RowDiagnostics` e `Tagged` sono le sole varianti con un
    /// `Box<Self>`: un terzo wrapper richiede di estendere questa funzione.
    #[must_use]
    pub fn with_phase(self, phase: ErrorPhase) -> Self {
        match self {
            // Il primo tag vince: il confine piu' vicino all'origine e' il
            // piu' preciso.
            Self::Tagged { .. } => self,
            // Wrapper trasparente: il tag scende, il payload resta sopra. Se
            // la sorgente e' gia' taggata, la ricorsione la restituisce
            // invariata — e vince comunque il tag originario.
            Self::RowDiagnostics {
                source,
                diagnostics,
            } => Self::RowDiagnostics {
                source: Box::new(source.with_phase(phase)),
                diagnostics,
            },
            Self::WithRemoteEffect {
                remote_effect,
                source,
            } => Self::WithRemoteEffect {
                remote_effect,
                source: Box::new(source.with_phase(phase)),
            },
            _ => Self::Tagged {
                phase,
                source: Box::new(self),
            },
        }
    }

    /// Dichiara l'effetto restato sul supporto
    /// ([`PlenoraError::WithRemoteEffect`]). Il primo effetto dichiarato
    /// vince, come il tag di fase: il confine più vicino all'origine sa di
    /// più. `RemoteEffect::None` lascia l'errore com'è.
    #[must_use]
    pub fn with_remote_effect(self, remote_effect: RemoteEffect) -> Self {
        if remote_effect == RemoteEffect::None || self.ha_effetto_dichiarato() {
            return self;
        }
        Self::WithRemoteEffect {
            remote_effect,
            source: Box::new(self),
        }
    }

    /// Un [`PlenoraError::WithRemoteEffect`] in un punto qualunque della
    /// catena dei wrapper trasparenti.
    fn ha_effetto_dichiarato(&self) -> bool {
        match self {
            Self::WithRemoteEffect { .. } => true,
            Self::Tagged { source, .. } | Self::RowDiagnostics { source, .. } => {
                source.ha_effetto_dichiarato()
            }
            _ => false,
        }
    }

    /// Un errore di I/O con un contesto nostro (nomi del piano, mai
    /// percorsi né dati) davanti: `Display` `"{contesto}: {causa}"`, stesso
    /// `ErrorKind`. Il contesto entra anche nel messaggio pubblico, il testo
    /// della causa no.
    #[must_use]
    pub fn io_con_contesto(contesto: &str, causa: std::io::Error) -> Self {
        Self::Io(std::io::Error::new(
            causa.kind(),
            ContestoIo {
                contesto: contesto.to_owned(),
                causa,
            },
        ))
    }

    /// Associa un payload di diagnostica per riga senza alterare testo o
    /// assi dell'errore.
    ///
    /// Un payload che non supera [`RowDiagnostics::validate_for_emission`]
    /// non si allega: l'errore diventa `Internal`, perché un payload
    /// incoerente è un difetto del kernel che lo ha costruito.
    #[must_use]
    pub fn with_row_diagnostics(self, diagnostics: RowDiagnostics) -> Self {
        if diagnostics.validate_for_emission().is_err() {
            return Self::Internal("row diagnostics interne non valide".to_owned());
        }
        Self::RowDiagnostics {
            source: Box::new(self),
            diagnostics: Box::new(diagnostics),
        }
    }

    /// Restituisce il payload di diagnostica per riga, anche attraverso un
    /// wrapper di fase.
    #[must_use]
    pub const fn row_diagnostics(&self) -> Option<&RowDiagnostics> {
        match self {
            Self::RowDiagnostics { diagnostics, .. } => Some(diagnostics),
            Self::Tagged { source, .. } | Self::WithRemoteEffect { source, .. } => {
                source.row_diagnostics()
            }
            _ => None,
        }
    }

    /// Restituisce `true` anche quando la cancellazione è avvolta da tag o
    /// diagnostica row-scoped.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.category() == ErrorCategory::Cancelled
    }

    /// Nodo, operazione ed `execution_id` di un `Execution` con
    /// `execution_id` assegnato, attraversando i wrapper trasparenti.
    #[must_use]
    pub fn execution_context(&self) -> Option<(&str, &str, &str)> {
        let (node, operation, execution_id) = self.execution_location()?;
        execution_id.map(|execution_id| (node, operation, execution_id))
    }

    /// Nodo, operazione ed `execution_id` (se assegnato) di un `Execution`,
    /// attraversando i wrapper trasparenti.
    #[must_use]
    pub fn execution_location(&self) -> Option<(&str, &str, Option<&str>)> {
        match self {
            Self::Execution {
                node,
                operation,
                execution_id,
                ..
            } => Some((
                node,
                operation,
                (!execution_id.is_empty()).then_some(execution_id.as_str()),
            )),
            Self::Tagged { source, .. }
            | Self::RowDiagnostics { source, .. }
            | Self::WithRemoteEffect { source, .. } => source.execution_location(),
            _ => None,
        }
    }

    /// Motivo di un `Execution`, attraverso i wrapper.
    #[must_use]
    pub fn execution_reason(&self) -> Option<&str> {
        match self {
            Self::Execution { reason, .. } => Some(reason),
            Self::Tagged { source, .. }
            | Self::RowDiagnostics { source, .. }
            | Self::WithRemoteEffect { source, .. } => source.execution_reason(),
            _ => None,
        }
    }

    /// Assegna l'`execution_id` a un `Execution` (anche sotto i wrapper)
    /// senza sovrascriverne uno già assegnato; le altre varianti tornano
    /// invariate.
    #[must_use]
    pub fn with_execution_id(self, execution_id: &str) -> Self {
        match self {
            Self::Execution {
                node,
                operation,
                execution_id: current,
                reason,
            } => Self::Execution {
                node,
                operation,
                execution_id: if current.is_empty() {
                    execution_id.to_owned()
                } else {
                    current
                },
                reason,
            },
            Self::Tagged { source, phase } => Self::Tagged {
                source: Box::new(source.with_execution_id(execution_id)),
                phase,
            },
            Self::RowDiagnostics {
                source,
                diagnostics,
            } => Self::RowDiagnostics {
                source: Box::new(source.with_execution_id(execution_id)),
                diagnostics,
            },
            Self::WithRemoteEffect {
                remote_effect,
                source,
            } => Self::WithRemoteEffect {
                remote_effect,
                source: Box::new(source.with_execution_id(execution_id)),
            },
            other => other,
        }
    }

    /// Il tag di fase esplicito, se il confine lo ha assegnato; `None` per
    /// un errore la cui fase e' derivata dalla variante.
    #[must_use]
    pub const fn phase_tag(&self) -> Option<ErrorPhase> {
        match self {
            Self::Tagged { phase, .. } => Some(*phase),
            Self::RowDiagnostics { source, .. } | Self::WithRemoteEffect { source, .. } => {
                source.phase_tag()
            }
            _ => None,
        }
    }

    /// Rimuove il tag di fase (ricorsivamente, se costruito annidato a mano)
    /// e restituisce l'errore originale: per i consumatori che fanno match
    /// sulle varianti canoniche e non portano la nozione di fase.
    #[must_use]
    pub fn untag(self) -> Self {
        match self {
            Self::Tagged { source, .. } => source.untag(),
            Self::RowDiagnostics {
                source,
                diagnostics,
            } => Self::RowDiagnostics {
                source: Box::new(source.untag()),
                diagnostics,
            },
            Self::WithRemoteEffect {
                remote_effect,
                source,
            } => Self::WithRemoteEffect {
                remote_effect,
                source: Box::new(source.untag()),
            },
            _ => self,
        }
    }

    /// Effetto restato sul supporto quando l'errore è riportato (l'asse
    /// «effetto»).
    ///
    /// [`RemoteEffect::None`] per costruzione: la scrittura atomica di
    /// `plenora-io` non rende mai visibile un file d'uscita parziale, e un
    /// temporaneo rimasto dopo un crash non sta alla destinazione. L'effetto
    /// dichiarato da [`PlenoraError::WithRemoteEffect`] vince: `esegui_da_file`
    /// scrive gli output uno alla volta e marca `Partial` un errore dopo il
    /// primo output scritto.
    #[must_use]
    pub const fn remote_effect(&self) -> RemoteEffect {
        match self {
            // Tutte le varianti nominate esplicitamente (esaustivita'
            // preservata): `None` per costruzione, vedi la doc sopra.
            Self::InvalidPlan(_)
            | Self::Unsupported(_)
            | Self::Schema(_)
            | Self::DataMapping(_)
            | Self::Execution { .. }
            | Self::Crs(_)
            | Self::CrsCoded { .. }
            | Self::Cancelled(_)
            | Self::Io(_)
            | Self::ResourceLimit(_)
            // Nessuna eccezione tra le nuove: `Conflict` e' l'unica che
            // tocca una destinazione, e la tocca per RIFIUTARSI di
            // sovrascriverla. Il rename atomico non e' avvenuto, quindi non
            // c'e' effetto da riparare.
            | Self::Protocol(_)
            | Self::Timeout(_)
            | Self::Conflict(_)
            | Self::InvalidConfiguration(_)
            | Self::Internal(_) => RemoteEffect::None,
            Self::WithRemoteEffect { remote_effect, .. } => *remote_effect,
            // Delegato alla sorgente: il tag raffina solo la fase.
            Self::Tagged { source, .. } | Self::RowDiagnostics { source, .. } => {
                source.remote_effect()
            }
        }
    }
}

/// Suffisso del `Display` con l'`execution_id`: omesso quando l'id è vuoto,
/// così il messaggio resta quello senza esecuzione.
fn execution_suffix(execution_id: &str) -> String {
    if execution_id.is_empty() {
        String::new()
    } else {
        format!(", execution `{execution_id}`")
    }
}

/// Contesto nostro di un errore di I/O ([`PlenoraError::io_con_contesto`]).
#[derive(Debug, Error)]
#[error("{contesto}: {causa}")]
struct ContestoIo {
    contesto: String,
    #[source]
    causa: std::io::Error,
}

/// I contesti nostri di un errore di I/O, dal più esterno; vuoto se non ne
/// ha.
pub(crate) fn contesti_io(errore: &std::io::Error) -> Vec<&str> {
    let mut contesti = Vec::new();
    let mut corrente = errore;
    while let Some(contesto) = corrente
        .get_ref()
        .and_then(|interno| interno.downcast_ref::<ContestoIo>())
    {
        contesti.push(contesto.contesto.as_str());
        corrente = &contesto.causa;
    }
    contesti
}

/// Categoria e ritentativo di un errore di I/O dal suo `ErrorKind`, con le
/// categorie più precise di `plenora-error-v1` dove ci sono.
///
/// - `Safe` solo per le cause transitorie: `TimedOut` (`timeout`),
///   `Interrupted`, `WouldBlock`, `ResourceBusy` (`transient`);
/// - `Never` per le deterministiche: `NotFound` (`not_found`),
///   `PermissionDenied` (`authorization`), `AlreadyExists` (`conflict`),
///   spazio o quota esauriti, file troppo grande, memoria (`resource_limit`),
///   `Unsupported` (`unsupported`), e il resto del filesystem (`io`);
/// - un `ErrorKind` non classificato (anche uno nuovo della libreria
///   standard): `io` e `Never`, la scelta prudente.
#[must_use]
pub const fn assi_io(kind: std::io::ErrorKind) -> (ErrorCategory, RetryDisposition) {
    use std::io::ErrorKind as K;
    match kind {
        K::TimedOut => (ErrorCategory::Timeout, RetryDisposition::Safe),
        K::Interrupted | K::WouldBlock | K::ResourceBusy => {
            (ErrorCategory::Transient, RetryDisposition::Safe)
        }
        K::NotFound => (ErrorCategory::NotFound, RetryDisposition::Never),
        K::PermissionDenied => (ErrorCategory::Authorization, RetryDisposition::Never),
        K::AlreadyExists => (ErrorCategory::Conflict, RetryDisposition::Never),
        K::StorageFull | K::QuotaExceeded | K::FileTooLarge | K::OutOfMemory => {
            (ErrorCategory::ResourceLimit, RetryDisposition::Never)
        }
        K::Unsupported => (ErrorCategory::Unsupported, RetryDisposition::Never),
        _ => (ErrorCategory::Io, RetryDisposition::Never),
    }
}

pub type Result<T> = std::result::Result<T, PlenoraError>;

#[cfg(test)]
mod tests {
    use super::*;
    // Il confronto fra elenco dichiarato e tabella attesa e' per
    // DISCRIMINANTE: `RetryDisposition::After` porta una durata, e la tabella
    // non deve dover indovinare quella del rappresentante.
    use core::mem::discriminant;

    fn step(execution_id: &str) -> PlenoraError {
        PlenoraError::Execution {
            node: "n".to_owned(),
            operation: "table.filter".to_owned(),
            execution_id: execution_id.to_owned(),
            reason: "boom".to_owned(),
        }
    }

    fn cancelled() -> PlenoraError {
        PlenoraError::Cancelled("prima del passo `n` (table.filter)".to_owned())
    }

    /// Una istanza per variante costruibile direttamente, con la categoria e
    /// la fase attese; le varianti `#[from]` sono coperte a parte.
    /// `testo_internal` e' il messaggio di `Internal`, proprio di ciascun uso.
    fn campioni(testo_internal: &str) -> Vec<(PlenoraError, ErrorCategory, ErrorPhase)> {
        use ErrorCategory as C;
        use ErrorPhase as P;
        vec![
            (
                PlenoraError::InvalidPlan("c".into()),
                C::InvalidPlan,
                P::Validate,
            ),
            (
                PlenoraError::Unsupported("u".into()),
                C::Unsupported,
                P::Validate,
            ),
            (PlenoraError::Schema("s".into()), C::Schema, P::Validate),
            (
                PlenoraError::DataMapping("d".into()),
                C::DataMapping,
                P::Write,
            ),
            (step("exec-1"), C::Execution, P::Write),
            (PlenoraError::Crs("crs".into()), C::Crs, P::Validate),
            (cancelled(), C::Cancelled, P::Write),
            (
                PlenoraError::Timeout("prima del passo `n` (table.filter)".into()),
                C::Timeout,
                P::Write,
            ),
            (
                PlenoraError::Io(std::io::Error::other("io")),
                C::Io,
                P::Write,
            ),
            (
                PlenoraError::Internal(testo_internal.into()),
                C::Internal,
                P::Write,
            ),
            // Wrapper di fase: la categoria e' quella della
            // sorgente (DataMapping non e' Io, quindi il test di retry qui
            // sotto attende `Never` senza vedere attraverso il wrapper); la
            // fase e' il tag del confine, non la derivazione della sorgente
            // (DataMapping → Write).
            (
                PlenoraError::DataMapping("d".into()).with_phase(P::Read),
                C::DataMapping,
                P::Read,
            ),
        ]
    }

    /// [`campioni`] con la categoria attesa.
    fn samples() -> Vec<(PlenoraError, ErrorCategory)> {
        campioni("invariante violata")
            .into_iter()
            .map(|(errore, categoria, _)| (errore, categoria))
            .collect()
    }

    #[test]
    fn category_mapping_is_declared_per_variant() {
        for (error, expected) in samples() {
            assert_eq!(error.category(), expected, "{error}");
            assert!(!error.category().as_str().is_empty());
        }
        // Conversioni `From` esterne (fuse in `DataMapping`).
        let arrow: PlenoraError = arrow_schema::ArrowError::SchemaError("boom".into()).into();
        assert_eq!(arrow.category(), ErrorCategory::DataMapping);
        assert!(arrow.to_string().starts_with("arrow error: "));
        let json: PlenoraError = serde_json::from_str::<u32>("\"non-un-numero\"")
            .expect_err("json invalido")
            .into();
        assert_eq!(json.category(), ErrorCategory::DataMapping);
        assert!(json.to_string().starts_with("json error: "));
    }

    /// Sentinella di privacy: il testo di arrow-rs cita i valori che hanno
    /// causato il difetto, e non deve attraversare il confine (errori senza
    /// dati). Resta il codice della variante, che e' strutturale.
    #[test]
    fn il_testo_di_arrow_non_attraversa_il_confine() {
        const SENTINELLA: &str = "mario.rossi@example.com";
        let casi = [
            arrow_schema::ArrowError::CastError(format!("Cannot cast '{SENTINELLA}' to Int64")),
            arrow_schema::ArrowError::ParseError(SENTINELLA.to_owned()),
            arrow_schema::ArrowError::ComputeError(SENTINELLA.to_owned()),
            arrow_schema::ArrowError::SchemaError(SENTINELLA.to_owned()),
            arrow_schema::ArrowError::InvalidArgumentError(SENTINELLA.to_owned()),
        ];
        for grezzo in casi {
            let atteso = arrow_error_code(&grezzo);
            let convertito: PlenoraError = grezzo.into();
            let testo = convertito.to_string();
            assert!(
                !testo.contains(SENTINELLA),
                "il valore ha attraversato il confine: {testo}"
            );
            assert_eq!(testo, format!("arrow error: {atteso}"));
        }
        // Il codice distingue le varianti: non e' una stringa unica.
        assert_eq!(
            arrow_error_code(&arrow_schema::ArrowError::DivideByZero),
            "divide_by_zero"
        );
        assert_ne!(
            arrow_error_code(&arrow_schema::ArrowError::CastError(String::new())),
            arrow_error_code(&arrow_schema::ArrowError::SchemaError(String::new()))
        );
    }

    #[test]
    fn internal_display_and_axes() {
        // La variante Internal raccoglie le violazioni di invariante che
        // altrimenti sarebbero panic; gli assi sono quelli dichiarati.
        let error = PlenoraError::Internal("stato impossibile".into());
        assert_eq!(error.to_string(), "internal error: stato impossibile");
        assert_eq!(error.category(), ErrorCategory::Internal);
        assert_eq!(error.phase(), ErrorPhase::Write);
        assert_eq!(error.remote_effect(), RemoteEffect::None);
        assert_eq!(error.retry_disposition(), RetryDisposition::Never);
    }

    #[test]
    fn retry_disposition_is_safe_only_for_interruptions_among_the_samples() {
        // La disposizione sostituisce il booleano: fra i campioni `Safe` solo
        // per scadenza e annullamento del runner, a effetto assente; `Never`
        // per le cause deterministiche e per l'`Io` non classificato del
        // campione (`Other`). Gli `ErrorKind` hanno un test a parte.
        for (error, _) in samples() {
            let expected = if matches!(error, PlenoraError::Cancelled(_) | PlenoraError::Timeout(_))
            {
                RetryDisposition::Safe
            } else {
                RetryDisposition::Never
            };
            assert_eq!(error.retry_disposition(), expected, "{error}");
        }
    }

    /// Una tabella di nomi stabili scritta a mano contro l'elenco dichiarato
    /// di un asse: ogni valore dichiarato vi compare (per discriminante), la
    /// tabella non ne nomina altri, e ogni nome e' sia `as_str` sia `Display`.
    fn pretendi_nomi_stabili<T: Copy + core::fmt::Debug + core::fmt::Display>(
        attesi: &[(T, &str)],
        dichiarati: &[T],
        as_str: impl Fn(T) -> &'static str,
    ) {
        for dichiarato in dichiarati {
            assert!(
                attesi
                    .iter()
                    .any(|(atteso, _)| discriminant(atteso) == discriminant(dichiarato)),
                "{dichiarato:?} e' dichiarato ma la tabella attesa non lo nomina"
            );
        }
        assert_eq!(
            attesi.len(),
            dichiarati.len(),
            "la tabella attesa nomina valori che la dichiarazione non contiene"
        );
        for &(valore, nome) in attesi {
            assert_eq!(as_str(valore), nome);
            assert_eq!(valore.to_string(), nome, "Display = as_str canonico");
        }
    }

    #[test]
    fn ogni_disposizione_dichiarata_ha_il_nome_stabile_atteso() {
        // Solo i valori canonici, snake_case, nessun valore proprio.
        //
        // La tabella e' scritta a mano, seconda opinione su `as_str`; si
        // itera `DISPOSIZIONI_DICHIARATE` e si pretende che la tabella nomini
        // ogni valore, cosi' una variante nuova fa fallire il test.
        pretendi_nomi_stabili(
            &[
                (RetryDisposition::Never, "never"),
                (RetryDisposition::Quarantine, "quarantine"),
                (RetryDisposition::Safe, "safe"),
                (
                    RetryDisposition::RequiresIdempotencyKey,
                    "requires_idempotency_key",
                ),
                (RetryDisposition::RequiresRecovery, "requires_recovery"),
                (RetryDisposition::After(Duration::from_millis(250)), "after"),
            ],
            RetryDisposition::DISPOSIZIONI_DICHIARATE,
            RetryDisposition::as_str,
        );
        // `after(durata)` trasporta la durata minima prima del retry.
        assert_eq!(
            RetryDisposition::After(Duration::from_millis(250)).delay(),
            Some(Duration::from_millis(250))
        );
        assert_eq!(RetryDisposition::Safe.delay(), None);
    }

    /// [`campioni`] con la fase attesa.
    fn phase_samples() -> Vec<(PlenoraError, ErrorPhase)> {
        campioni("i")
            .into_iter()
            .map(|(errore, _, fase)| (errore, fase))
            .collect()
    }

    #[test]
    fn phase_mapping_is_declared_per_variant() {
        for (error, expected) in phase_samples() {
            assert_eq!(error.phase(), expected, "{error}");
        }
        // Conversioni `From` esterne: entrambe in `DataMapping` (Write: la
        // fusione cancella la distinzione parse/I-O A LIVELLO DI VARIANTE;
        // chi la conosce la recupera col tag, vedi i test del wrapper).
        let arrow: PlenoraError = arrow_schema::ArrowError::SchemaError("boom".into()).into();
        assert_eq!(arrow.phase(), ErrorPhase::Write);
        let json: PlenoraError = serde_json::from_str::<u32>("\"non-un-numero\"")
            .expect_err("json invalido")
            .into();
        assert_eq!(json.phase(), ErrorPhase::Write);
    }

    #[test]
    fn tagged_phase_overrides_derivation_and_display_is_identical() {
        // Il tag raffina SOLO la fase. Per ogni fase
        // canonica: `phase()` riporta il tag, il `Display` e' byte-identico
        // alla sorgente (nessun consumatore testuale si rompe).
        let source = || PlenoraError::Io(std::io::Error::other("caduta di rete"));
        let expected_text = source().to_string();
        for phase in [
            ErrorPhase::Validate,
            ErrorPhase::Connect,
            ErrorPhase::Probe,
            ErrorPhase::Prepare,
            ErrorPhase::Read,
            ErrorPhase::Write,
            ErrorPhase::Finalize,
            ErrorPhase::Commit,
            ErrorPhase::Rollback,
            ErrorPhase::Cleanup,
        ] {
            let tagged = source().with_phase(phase);
            assert_eq!(tagged.phase(), phase);
            assert_eq!(tagged.phase_tag(), Some(phase));
            assert_eq!(tagged.to_string(), expected_text, "Display delegato");
        }
        assert_eq!(source().phase_tag(), None, "non taggato: fase derivata");
    }

    #[test]
    fn l_elenco_completo_e_coerente_con_gli_indici_e_i_nomi() {
        // Che `ALL` contenga tutte le varianti lo garantisce la macro
        // `categorie_errore`. Qui si verifica cio' che la macro non
        // garantisce: che gli indici siano le posizioni reali. Che i nomi
        // stabili siano distinti lo provano il giro di `from_stable_name`
        // qui sotto e la tabella scritta a mano di
        // `i_nomi_stabili_sono_quelli_dichiarati_e_la_tabella_li_copre_tutti`.
        for (posizione, categoria) in ErrorCategory::ALL.iter().enumerate() {
            assert_eq!(
                categoria.index(),
                posizione,
                "{categoria:?} e' in posizione {posizione} ma dichiara indice {}",
                categoria.index()
            );
        }
        // Il conteggio e' un'informazione, non un presidio: le 19 categorie
        // di `plenora-error-v1` (le due estensioni locali dell'isolamento
        // del progetto d'origine non ci sono).
        assert_eq!(ErrorCategory::ALL.len(), 19, "categorie supportate");
        // `from_stable_name` e' l'inverso di `as_str`: se il giro non
        // chiudesse, un report legittimo verrebbe letto come categoria
        // sconosciuta.
        for &categoria in ErrorCategory::ALL {
            assert_eq!(
                ErrorCategory::from_stable_name(categoria.as_str()),
                Some(categoria),
                "{categoria:?} non si rilegge dal proprio nome stabile"
            );
        }
        assert_eq!(
            ErrorCategory::from_stable_name("categoria_di_un_altro_binario"),
            None,
            "una stringa che non e' una categoria deve restare non riconosciuta"
        );
    }

    #[test]
    fn tagged_axes_are_delegated_to_the_source() {
        // Gli assi diversi dalla fase attraversano il wrapper invariati:
        // `Io` transitorio taggato resta `transient`, effetto None, retry
        // `Safe`: la disposizione NON cambia col raffinamento di fase.
        let tagged = PlenoraError::Io(std::io::Error::new(std::io::ErrorKind::Interrupted, "io"))
            .with_phase(ErrorPhase::Read);
        assert_eq!(tagged.category(), ErrorCategory::Transient);
        assert_eq!(tagged.remote_effect(), RemoteEffect::None);
        assert_eq!(tagged.retry_disposition(), RetryDisposition::Safe);
        // Anche una causa deterministica taggata resta `Never`.
        let tagged_plan = PlenoraError::InvalidPlan("c".into()).with_phase(ErrorPhase::Commit);
        assert_eq!(tagged_plan.category(), ErrorCategory::InvalidPlan);
        assert_eq!(tagged_plan.remote_effect(), RemoteEffect::None);
        assert_eq!(tagged_plan.retry_disposition(), RetryDisposition::Never);
        // `Error::source()` espone l'errore originale (catena standard).
        let source = std::error::Error::source(&tagged).expect("sorgente");
        assert_eq!(source.to_string(), "io error: io");
    }

    #[test]
    fn le_varianti_nuove_dichiarano_i_quattro_assi() {
        // La tabella e' scritta a mano: e' il contratto approvato, e serve
        // che sia leggibile accanto a cio' che verifica.
        // `Timeout` sta in `campioni`: e' ritentabile (`Safe`).
        let casi: [(PlenoraError, ErrorCategory, ErrorPhase); 3] = [
            (
                PlenoraError::Protocol("attesa hello, ricevuto ready".into()),
                ErrorCategory::Protocol,
                ErrorPhase::Write,
            ),
            (
                PlenoraError::Conflict("destinazione comparsa prima del rename".into()),
                ErrorCategory::Conflict,
                ErrorPhase::Commit,
            ),
            (
                PlenoraError::InvalidConfiguration("radice temporanea non scrivibile".into()),
                ErrorCategory::InvalidConfiguration,
                ErrorPhase::Prepare,
            ),
        ];
        for (errore, categoria, fase) in casi {
            assert_eq!(errore.category(), categoria, "categoria di {errore:?}");
            assert_eq!(errore.phase(), fase, "fase di {errore:?}");
            // Gli altri due assi sono uniformi per tutte e quattro, e vale la
            // pena che il test lo dica invece di ripeterlo nella tabella.
            assert_eq!(
                errore.remote_effect(),
                RemoteEffect::None,
                "nessuna delle quattro lascia effetto: {errore:?}"
            );
            assert_eq!(
                errore.retry_disposition(),
                RetryDisposition::Never,
                "nessuna delle quattro e' ritentabile in cieco: {errore:?}"
            );
        }
    }

    #[test]
    fn il_tag_di_fase_raffina_anche_le_varianti_nuove() {
        // `Protocol` deriva `Write` ma nasce davvero dove il confine dice.
        // Se il tag non scendesse, la fase resterebbe un'approssimazione
        // permanente per tutta la famiglia nuova.
        let al_confine =
            PlenoraError::Protocol("cornice troncata".into()).with_phase(ErrorPhase::Read);
        assert_eq!(al_confine.phase(), ErrorPhase::Read);
        // E il raffinamento NON tocca gli altri tre assi.
        assert_eq!(al_confine.category(), ErrorCategory::Protocol);
        assert_eq!(al_confine.remote_effect(), RemoteEffect::None);
        assert_eq!(al_confine.retry_disposition(), RetryDisposition::Never);
    }

    #[test]
    fn with_phase_keeps_the_first_tag_and_untag_strips_it() {
        // Il confine piu' vicino all'origine e' il piu' preciso: un secondo
        // tag non sovrascrive e non annida.
        let tagged = PlenoraError::DataMapping("d".into())
            .with_phase(ErrorPhase::Read)
            .with_phase(ErrorPhase::Write);
        assert_eq!(tagged.phase(), ErrorPhase::Read);
        // `untag` restituisce l'errore originale, invariato nel testo.
        let untagged = tagged.untag();
        assert!(matches!(untagged, PlenoraError::DataMapping(_)));
        assert_eq!(untagged.to_string(), "d");
        assert_eq!(untagged.phase(), ErrorPhase::Write, "derivata, non taggata");
        // Anche un wrapper annidato a mano e' rimosso ricorsivamente.
        let nested = PlenoraError::Tagged {
            phase: ErrorPhase::Commit,
            source: Box::new(PlenoraError::Schema("s".into()).with_phase(ErrorPhase::Probe)),
        };
        assert_eq!(nested.phase(), ErrorPhase::Commit, "il tag esterno vince");
        assert!(matches!(nested.untag(), PlenoraError::Schema(_)));
    }

    #[test]
    fn remote_effect_is_none_for_every_variant_by_construction() {
        // La scrittura atomica non rende mai visibile un'uscita parziale: un
        // `PlenoraError` non accompagna mai un effetto osservabile.
        for (error, _) in samples() {
            assert_eq!(error.remote_effect(), RemoteEffect::None, "{error}");
        }
        let arrow: PlenoraError = arrow_schema::ArrowError::SchemaError("boom".into()).into();
        assert_eq!(arrow.remote_effect(), RemoteEffect::None);
        let json: PlenoraError = serde_json::from_str::<u32>("\"non-un-numero\"")
            .expect_err("json invalido")
            .into();
        assert_eq!(json.remote_effect(), RemoteEffect::None);
    }

    #[test]
    fn ogni_fase_dichiarata_ha_il_nome_stabile_atteso() {
        // Solo i dieci valori canonici, snake_case; nessun valore proprio.
        //
        // Tabella scritta a mano — seconda opinione su `as_str` — e giro
        // sull'elenco dichiarato: e' `FASI_DICHIARATE` a decidere che cosa
        // dev'essere nominato, non questa tabella a decidere che cosa
        // guardare.
        pretendi_nomi_stabili(
            &[
                (ErrorPhase::Validate, "validate"),
                (ErrorPhase::Connect, "connect"),
                (ErrorPhase::Probe, "probe"),
                (ErrorPhase::Prepare, "prepare"),
                (ErrorPhase::Read, "read"),
                (ErrorPhase::Write, "write"),
                (ErrorPhase::Finalize, "finalize"),
                (ErrorPhase::Commit, "commit"),
                (ErrorPhase::Rollback, "rollback"),
                (ErrorPhase::Cleanup, "cleanup"),
            ],
            ErrorPhase::FASI_DICHIARATE,
            ErrorPhase::as_str,
        );
    }

    #[test]
    fn ogni_effetto_dichiarato_ha_il_nome_stabile_atteso() {
        // Solo i cinque valori canonici; l'esito ignoto e' un effetto
        // (`unknown`), non una categoria.
        //
        // Stesso impianto delle fasi: tabella indipendente, giro
        // sull'elenco dichiarato.
        pretendi_nomi_stabili(
            &[
                (RemoteEffect::None, "none"),
                (RemoteEffect::RolledBack, "rolled_back"),
                (RemoteEffect::Partial, "partial"),
                (RemoteEffect::Committed, "committed"),
                (RemoteEffect::Unknown, "unknown"),
            ],
            RemoteEffect::EFFETTI_DICHIARATI,
            RemoteEffect::as_str,
        );
    }

    #[test]
    fn step_display_omits_the_execution_id_when_empty() {
        // Senza execution_id il messaggio non ha il suffisso.
        assert_eq!(
            step("").to_string(),
            "step failed at node `n` (operation `table.filter`): boom"
        );
        assert_eq!(
            step("exec-42").to_string(),
            "step failed at node `n` (operation `table.filter`, execution `exec-42`): boom"
        );
    }

    #[test]
    fn cancelled_display_carries_context_without_values() {
        let text = cancelled().to_string();
        assert_eq!(text, "cancelled: prima del passo `n` (table.filter)");
        assert_eq!(cancelled().category(), ErrorCategory::Cancelled);
        assert!(cancelled().is_cancelled());
        assert_eq!(
            cancelled().retry_disposition(),
            RetryDisposition::Safe,
            "nessun output reso: rieseguire e' sicuro, deciderlo spetta al chiamante"
        );
    }

    #[test]
    fn i_nomi_stabili_sono_quelli_dichiarati_e_la_tabella_li_copre_tutti() {
        // I nomi stabili pubblici delle categorie: la tabella verifica la
        // stabilita' del nome, non la sua autorita'.
        //
        // Scritta a mano, seconda opinione su `as_str`; si itera
        // `ErrorCategory::ALL` e si chiede alla tabella di nominare ogni
        // categoria, cosi' una categoria nuova non passa inosservata.
        let all = [
            (ErrorCategory::InvalidPlan, "invalid_plan"),
            (ErrorCategory::InvalidConfiguration, "invalid_configuration"),
            (ErrorCategory::Schema, "schema"),
            (ErrorCategory::DataMapping, "data_mapping"),
            (ErrorCategory::Crs, "crs"),
            (ErrorCategory::Unsupported, "unsupported"),
            (ErrorCategory::NotFound, "not_found"),
            (ErrorCategory::Conflict, "conflict"),
            (
                ErrorCategory::ConcurrentModification,
                "concurrent_modification",
            ),
            (ErrorCategory::Authentication, "authentication"),
            (ErrorCategory::Authorization, "authorization"),
            (ErrorCategory::Timeout, "timeout"),
            (ErrorCategory::Cancelled, "cancelled"),
            (ErrorCategory::ResourceLimit, "resource_limit"),
            (ErrorCategory::Io, "io"),
            (ErrorCategory::Protocol, "protocol"),
            (ErrorCategory::Transient, "transient"),
            (ErrorCategory::Execution, "execution"),
            (ErrorCategory::Internal, "internal"),
        ];
        for categoria in ErrorCategory::ALL {
            let atteso = all
                .iter()
                .find_map(|(dichiarata, nome)| (dichiarata == categoria).then_some(*nome))
                .unwrap_or_else(|| {
                    panic!("{categoria:?} non compare nella tabella dei nomi stabili")
                });
            assert_eq!(categoria.as_str(), atteso, "nome stabile pubblico");
        }
        // E nessuna riga della tabella nomina una categoria che non esiste
        // piu': senza questo, una rimozione lascerebbe una riga morta.
        assert_eq!(
            all.len(),
            ErrorCategory::ALL.len(),
            "la tabella e l'elenco delle categorie hanno lunghezze diverse"
        );
    }

    // -----------------------------------------------------------------------
    // `with_phase` attraverso i wrapper trasparenti.
    // -----------------------------------------------------------------------

    /// Diagnostica di riga valida: `validate_for_emission` ha regole strette
    /// che si tengono a vicenda, e un payload inventato viene rifiutato
    /// degradando l'errore a `Internal` — cioe' i test verificherebbero
    /// un'altra cosa.
    fn diagnostica() -> crate::diagnostics::RowDiagnostics {
        use crate::diagnostics::tests::{example, report};
        report(1, vec![example(0)])
    }

    /// Quanti `Tagged` ci sono nell'intera catena.
    ///
    /// E' la proprieta' che il contratto dichiara e che nessun accessore puo'
    /// verificare: categoria, fase e payload sono identici con uno o due tag.
    fn tag_nella_catena(errore: &PlenoraError) -> usize {
        match errore {
            PlenoraError::Tagged { source, .. } => 1 + tag_nella_catena(source),
            PlenoraError::RowDiagnostics { source, .. } => tag_nella_catena(source),
            _ => 0,
        }
    }

    #[test]
    fn il_tag_scende_sotto_la_diagnostica_di_riga() {
        let errore = PlenoraError::DataMapping("causa".to_owned())
            .with_row_diagnostics(diagnostica())
            .with_phase(ErrorPhase::Read);

        // Forma canonica: RowDiagnostics -> Tagged -> causa, mai il contrario.
        let PlenoraError::RowDiagnostics { source, .. } = &errore else {
            panic!("il wrapper trasparente deve restare esterno, trovato {errore:?}");
        };
        assert!(
            matches!(source.as_ref(), PlenoraError::Tagged { phase, .. } if *phase == ErrorPhase::Read),
            "il tag non e' sceso sotto la diagnostica"
        );
        assert_eq!(tag_nella_catena(&errore), 1);
    }

    #[test]
    fn sotto_la_diagnostica_vince_il_tag_originario() {
        // Il confine piu' vicino all'origine e' il piu' preciso: una seconda
        // fase, applicata piu' in alto, non deve sovrascrivere la prima.
        let errore = PlenoraError::DataMapping("causa".to_owned())
            .with_phase(ErrorPhase::Read)
            .with_row_diagnostics(diagnostica())
            .with_phase(ErrorPhase::Write);

        assert_eq!(
            errore.phase(),
            ErrorPhase::Read,
            "la seconda fase ha sovrascritto la prima"
        );
        assert_eq!(
            tag_nella_catena(&errore),
            1,
            "si e' formato un tag annidato"
        );
    }

    #[test]
    fn la_diagnostica_sopravvive_al_tag() {
        let errore = PlenoraError::ResourceLimit("tetto".to_owned())
            .with_row_diagnostics(diagnostica())
            .with_phase(ErrorPhase::Read);

        assert!(
            errore.row_diagnostics().is_some(),
            "il payload e' andato perduto applicando la fase"
        );
        assert_eq!(errore.category(), ErrorCategory::ResourceLimit);
        assert_eq!(errore.phase(), ErrorPhase::Read);
    }

    #[test]
    fn la_diagnostica_si_legge_anche_sotto_un_tag_esterno() {
        // `with_phase` mette sempre il tag sotto la diagnostica, ma la forma
        // opposta resta costruibile a mano (come il tag annidato): gli
        // accessori attraversano il wrapper di fase in entrambi i versi.
        let errore = PlenoraError::Tagged {
            phase: ErrorPhase::Commit,
            source: Box::new(
                PlenoraError::ResourceLimit("tetto".to_owned()).with_row_diagnostics(diagnostica()),
            ),
        };

        assert_eq!(
            errore.row_diagnostics(),
            Some(&diagnostica()),
            "il payload sotto il tag esterno non e' raggiungibile"
        );
        assert_eq!(errore.category(), ErrorCategory::ResourceLimit);
        assert_eq!(errore.phase(), ErrorPhase::Commit);
        assert_eq!(errore.phase_tag(), Some(ErrorPhase::Commit));
        assert_eq!(
            errore.to_string(),
            PlenoraError::ResourceLimit("tetto".to_owned()).to_string()
        );
    }

    #[test]
    fn un_solo_tag_anche_applicando_la_fase_piu_volte() {
        let mut errore =
            PlenoraError::DataMapping("causa".to_owned()).with_row_diagnostics(diagnostica());
        for _ in 0..5 {
            errore = errore.with_phase(ErrorPhase::Read);
        }
        assert_eq!(tag_nella_catena(&errore), 1);
        assert!(errore.row_diagnostics().is_some());
    }

    /// `con_contesto` antepone il contesto alle varianti con un messaggio
    /// nostro e preserva la variante; le altre passano inalterate.
    ///
    /// Il contesto di prova ha la forma di quello che compone chi legge un
    /// input da file: nome e percorso.
    #[test]
    fn con_contesto_antepone_il_contesto_e_preserva_la_variante() {
        let contesto = "input `main` (dati.arrow)";
        let prefixed = PlenoraError::InvalidPlan("boom".into()).con_contesto(contesto);
        match prefixed {
            PlenoraError::InvalidPlan(message) => {
                assert_eq!(message, "input `main` (dati.arrow): boom");
            }
            other => panic!("variante non preservata: {other:?}"),
        }
        for make in [
            PlenoraError::Unsupported as fn(String) -> PlenoraError,
            PlenoraError::Schema,
            PlenoraError::Crs,
        ] {
            let prefixed = make("boom".to_owned()).con_contesto(contesto);
            let message = match &prefixed {
                PlenoraError::Unsupported(message)
                | PlenoraError::Schema(message)
                | PlenoraError::Crs(message) => message,
                other => panic!("variante non preservata: {other:?}"),
            };
            assert_eq!(message, "input `main` (dati.arrow): boom");
        }
        // Le altre varianti passano inalterate (testo e tipo).
        let io = PlenoraError::Io(std::io::Error::other("disco")).con_contesto(contesto);
        assert!(matches!(io, PlenoraError::Io(_)));
        assert_eq!(io.to_string(), "io error: disco");
    }

    #[test]
    fn l_effetto_dichiarato_vince_e_toglie_il_ritentativo_automatico() {
        let io = || {
            PlenoraError::Io(std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                "disco",
            ))
        };
        let parziale = io().with_remote_effect(RemoteEffect::Partial);
        assert_eq!(parziale.remote_effect(), RemoteEffect::Partial);
        assert_eq!(
            parziale.retry_disposition(),
            RetryDisposition::RequiresRecovery
        );
        assert_eq!(parziale.category(), ErrorCategory::Transient);
        assert_eq!(parziale.to_string(), io().to_string(), "Display delegato");
        // Il primo effetto vince; `None` non avvolge.
        let due = io()
            .with_remote_effect(RemoteEffect::Partial)
            .with_remote_effect(RemoteEffect::Committed);
        assert_eq!(due.remote_effect(), RemoteEffect::Partial);
        assert!(matches!(
            io().with_remote_effect(RemoteEffect::None),
            PlenoraError::Io(_)
        ));
        // Una causa che non si ritenta mai resta `Never`.
        let mai =
            PlenoraError::ResourceLimit("budget".into()).with_remote_effect(RemoteEffect::Partial);
        assert_eq!(mai.retry_disposition(), RetryDisposition::Never);
        // Fase e diagnostica attraversano il wrapper.
        let taggato = io()
            .with_remote_effect(RemoteEffect::Partial)
            .with_phase(ErrorPhase::Commit);
        assert_eq!(taggato.phase(), ErrorPhase::Commit);
        assert_eq!(taggato.remote_effect(), RemoteEffect::Partial);
        let con_diagnostica = PlenoraError::DataMapping("d".into())
            .with_row_diagnostics(diagnostica())
            .with_remote_effect(RemoteEffect::Partial);
        assert!(con_diagnostica.row_diagnostics().is_some());
        assert_eq!(
            con_diagnostica.untag().remote_effect(),
            RemoteEffect::Partial
        );
    }

    #[test]
    fn il_primo_effetto_vince_sotto_ogni_wrapper() {
        let parziale = || {
            PlenoraError::Io(std::io::Error::from(std::io::ErrorKind::Interrupted))
                .with_remote_effect(RemoteEffect::Partial)
        };
        let ordini = [
            (
                "tag sopra l'effetto",
                parziale().with_phase(ErrorPhase::Commit),
            ),
            (
                "diagnostica sopra l'effetto",
                parziale().with_row_diagnostics(diagnostica()),
            ),
            (
                "diagnostica e tag sopra l'effetto",
                parziale()
                    .with_phase(ErrorPhase::Read)
                    .with_row_diagnostics(diagnostica()),
            ),
            (
                "tag costruito a mano sopra l'effetto",
                PlenoraError::Tagged {
                    phase: ErrorPhase::Write,
                    source: Box::new(parziale()),
                },
            ),
        ];
        for (ordine, errore) in ordini {
            let ridichiarato = errore.with_remote_effect(RemoteEffect::RolledBack);
            assert_eq!(
                ridichiarato.remote_effect(),
                RemoteEffect::Partial,
                "{ordine}"
            );
            assert_eq!(
                ridichiarato.public_projection().remote_effect(),
                RemoteEffect::Partial,
                "{ordine}"
            );
        }
    }

    #[test]
    fn gli_errori_di_io_si_classificano_per_tipo() {
        use std::io::ErrorKind as K;
        let casi = [
            (
                K::NotFound,
                ErrorCategory::NotFound,
                RetryDisposition::Never,
            ),
            (
                K::PermissionDenied,
                ErrorCategory::Authorization,
                RetryDisposition::Never,
            ),
            (
                K::AlreadyExists,
                ErrorCategory::Conflict,
                RetryDisposition::Never,
            ),
            (K::IsADirectory, ErrorCategory::Io, RetryDisposition::Never),
            (K::NotADirectory, ErrorCategory::Io, RetryDisposition::Never),
            (K::InvalidInput, ErrorCategory::Io, RetryDisposition::Never),
            (
                K::InvalidFilename,
                ErrorCategory::Io,
                RetryDisposition::Never,
            ),
            (
                K::ReadOnlyFilesystem,
                ErrorCategory::Io,
                RetryDisposition::Never,
            ),
            (K::InvalidData, ErrorCategory::Io, RetryDisposition::Never),
            (
                K::StorageFull,
                ErrorCategory::ResourceLimit,
                RetryDisposition::Never,
            ),
            (
                K::Unsupported,
                ErrorCategory::Unsupported,
                RetryDisposition::Never,
            ),
            (K::TimedOut, ErrorCategory::Timeout, RetryDisposition::Safe),
            (
                K::Interrupted,
                ErrorCategory::Transient,
                RetryDisposition::Safe,
            ),
            (
                K::WouldBlock,
                ErrorCategory::Transient,
                RetryDisposition::Safe,
            ),
            (
                K::ResourceBusy,
                ErrorCategory::Transient,
                RetryDisposition::Safe,
            ),
            // Non classificato: la scelta prudente.
            (K::Other, ErrorCategory::Io, RetryDisposition::Never),
        ];
        for (tipo, categoria, ritentativo) in casi {
            let errore = PlenoraError::Io(std::io::Error::from(tipo));
            assert_eq!(errore.category(), categoria, "{tipo:?}");
            assert_eq!(errore.retry_disposition(), ritentativo, "{tipo:?}");
            assert_eq!(errore.public_projection().category(), categoria, "{tipo:?}");
        }
    }

    #[test]
    fn il_messaggio_pubblico_di_io_non_porta_il_testo_del_sistema() {
        const PERCORSO: &str = "C:/Utenti/mario.rossi/segreti/dati.parquet";
        let causa = std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("No such file or directory at path {PERCORSO}"),
        );
        let errore = PlenoraError::io_con_contesto(
            "output `b`",
            std::io::Error::new(causa.kind(), causa.to_string()),
        );
        // Il testo locale tiene la causa, per chi diagnostica sul posto.
        assert!(errore.to_string().contains(PERCORSO));
        let pubblico = errore.public_projection();
        assert!(
            !pubblico.message().contains("segreti"),
            "{}",
            pubblico.message()
        );
        assert_eq!(pubblico.message(), "io error: output `b`: entity not found");
        // Anche sotto i wrapper e con contesti annidati.
        let annidato = PlenoraError::io_con_contesto(
            "output `b`",
            match PlenoraError::io_con_contesto("scrittura", causa) {
                PlenoraError::Io(interno) => interno,
                altro => panic!("{altro:?}"),
            },
        )
        .with_phase(ErrorPhase::Commit)
        .with_remote_effect(RemoteEffect::Partial);
        let messaggio = annidato.public_projection().message().to_owned();
        assert_eq!(
            messaggio,
            "io error: output `b`: scrittura: entity not found"
        );
    }
}
