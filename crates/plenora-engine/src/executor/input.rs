//! Gli ingressi di un'esecuzione: da dove arrivano i batch.
//!
//! [`Input`] e' una sorgente (batch in memoria, file o stream Arrow IPC,
//! iteratore del chiamante); [`Inputs`] e' l'insieme atteso dal piano,
//! indicizzato per nome.
//!
//! [`Inputs::strict`] esige un [`DataContract`] per ogni ingresso e confronta
//! il **fingerprint completo**; il profilo permissivo confronta solo lo schema
//! Arrow e resta il default perche' cambiarlo romperebbe i chiamanti esistenti.

use std::collections::BTreeMap;
use std::path::Path;

use plenora_core::arrow::array::RecordBatch;
use plenora_core::arrow::schema::SchemaRef;
use plenora_core::contract::DataContract;
use plenora_core::{PlenoraError, Result};

use crate::governor::GovernedBatch;

/// Uno stream di batch governati: il tipo che attraversa tutto l'executor.
pub(super) type BatchStream = Box<dyn Iterator<Item = Result<GovernedBatch>>>;
use crate::ipc_boundary::{self, IpcFormat, IpcLimits};

/// Un input del piano: lettore Arrow IPC o iteratore di `RecordBatch`.
///
/// L'enum e' **opaco**: solo i costruttori impongono l'invariante
/// "`Batches` non e' mai vuoto", da cui dipende [`Input::schema`] che legge
/// `batches[0]`.
pub enum Input {
    /// Batch gia' in memoria. Invariante: mai vuoto (vedi
    /// [`Input::from_batches`]).
    ///
    /// `#[non_exhaustive]` la rende non costruibile fuori dal crate; e' una
    /// variante di struttura perche' su una di tupla bloccherebbe anche il
    /// pattern matching dall'esterno.
    #[non_exhaustive]
    Batches {
        /// I batch, nell'ordine di ingresso.
        batches: Vec<RecordBatch>,
    },
    /// Sorgente lazy (lettore IPC o qualunque iteratore di batch).
    Stream {
        /// Schema dichiarato della sorgente (verificato contro il contratto).
        schema: SchemaRef,
        /// Iteratore di batch.
        iter: BatchStream,
    },
}

impl Input {
    /// Input da batch in memoria (schema dal primo batch).
    ///
    /// # Errors
    ///
    /// `PlenoraError::InvalidPlan` se il vettore e' vuoto (per input vuoti usare
    /// [`Input::empty`] con lo schema esplicito).
    pub fn from_batches(batches: Vec<RecordBatch>) -> Result<Self> {
        if batches.is_empty() {
            return Err(PlenoraError::InvalidPlan(
                "input da batch: vettore vuoto, usare Input::empty con lo schema".into(),
            ));
        }
        Ok(Self::Batches { batches })
    }

    /// Input vuoto con schema esplicito.
    #[must_use]
    pub fn empty(schema: SchemaRef) -> Self {
        Self::Stream {
            schema,
            iter: Box::new(std::iter::empty()),
        }
    }

    /// Input da un iteratore di batch con schema dichiarato.
    ///
    /// I batch entrano nel perimetro nudi (API pubblica su `RecordBatch`) e
    /// sono avvolti in [`GovernedBatch`] senza lease: quota e sequenza sono
    /// assegnate all'ingresso dell'arco di input (architettura.md#memoria e #determinismo).
    #[must_use]
    pub fn from_iter<I>(schema: SchemaRef, iter: I) -> Self
    where
        I: Iterator<Item = Result<RecordBatch>> + 'static,
    {
        Self::Stream {
            schema,
            iter: Box::new(
                iter.map(|item| item.map(|batch| GovernedBatch::new(batch, None, None))),
            ),
        }
    }

    /// Lettore Arrow IPC **file format** (lazy: i batch sono letti dal disco
    /// man mano che lo stream di output viene tirato).
    ///
    /// # Errors
    ///
    /// `PlenoraError::Io`/`PlenoraError::DataMapping` se il file non si apre o
    /// l'header IPC non e' valido — taggati [`ErrorPhase::Read`] (BLOCK-03:
    /// nascono leggendo la sorgente).
    pub fn read_ipc_file(path: &Path) -> Result<Self> {
        Self::from_boundary(path, IpcFormat::File, &IpcLimits::default())
    }

    /// Lettore Arrow IPC **stream format** (lazy).
    ///
    /// # Errors
    ///
    /// Come [`Input::read_ipc_file`].
    pub fn read_ipc_stream(path: &Path) -> Result<Self> {
        Self::from_boundary(path, IpcFormat::Stream, &IpcLimits::default())
    }

    /// Lettore Arrow IPC che riconosce il formato dal magic (`ARROW1` →
    /// file format, altrimenti stream format), con i tetti di confine di
    /// default.
    ///
    /// # Errors
    ///
    /// Come [`Input::read_ipc_file`].
    pub fn read_ipc(path: &Path) -> Result<Self> {
        Self::read_ipc_with_limits(path, &IpcLimits::default())
    }

    /// Come [`Input::read_ipc`], ma con i tetti di confine derivati dai
    /// limiti effettivi del piano
    /// ([`crate::ipc_boundary::limits_from_plan`]).
    ///
    /// Con il piano gia' validato i tetti su body e numero di messaggi si
    /// applicano alle lunghezze DICHIARATE, prima che arrow allochi.
    ///
    /// # Errors
    ///
    /// Come [`Input::read_ipc_file`].
    pub fn read_ipc_with_limits(path: &Path, limits: &IpcLimits) -> Result<Self> {
        let format = ipc_boundary::sniff_format(path)?;
        Self::from_boundary(path, format, limits)
    }

    /// Apre la sorgente attraverso il lettore di confine condiviso
    /// ([`crate::ipc_boundary`]): framing e limiti pre-validati prima che
    /// arrow allochi, panico di `fb_to_schema` convertito in errore. Nessun
    /// ingresso non fidato apre `FileReader`/`StreamReader` per conto proprio.
    fn from_boundary(path: &Path, format: IpcFormat, limits: &IpcLimits) -> Result<Self> {
        let (schema, batches) = ipc_boundary::open_with_format(path, format, limits)?;
        Ok(Self::Stream {
            schema,
            iter: Box::new(
                batches.map(|batch| batch.map(|batch| GovernedBatch::new(batch, None, None))),
            ),
        })
    }

    /// Schema dichiarato della sorgente.
    ///
    /// Fallibile invece di indicizzare: l'invariante «`Batches` non e' mai
    /// vuoto» e' imposta da [`Input::from_batches`] e dalla variante
    /// `#[non_exhaustive]`, quindi un vettore vuoto qui e' un difetto nostro,
    /// non un input malformato — e va segnalato, non fatto abortire.
    pub(super) fn schema(&self) -> Result<SchemaRef> {
        match self {
            Self::Batches { batches } => {
                batches.first().map(RecordBatch::schema).ok_or_else(|| {
                    PlenoraError::Internal(
                        "input da batch vuoto: invariante di Input::from_batches violata"
                            .to_owned(),
                    )
                })
            }
            Self::Stream { schema, .. } => Ok(schema.clone()),
        }
    }
}

/// Gli input di un'esecuzione, per nome come dichiarati nel piano.
#[derive(Default)]
pub struct Inputs {
    pub(super) readers: BTreeMap<String, Input>,
    /// Contratti dichiarati dal chiamante per gli input che li portano
    /// ([`Inputs::add_with_contract`]).
    pub(super) contracts: BTreeMap<String, DataContract>,
    /// Profilo stretto: ogni input DEVE portare il proprio contratto
    /// ([`Inputs::strict`]).
    pub(super) strict: bool,
}

impl Inputs {
    /// Insieme vuoto, nel profilo permissivo: gli input possono entrare
    /// senza contratto e il confine si chiude allora sul solo schema Arrow
    /// (vedi [`Inputs::add`]).
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Insieme vuoto nel profilo **stretto**.
    ///
    /// Ogni input deve portare il proprio [`DataContract`] e [`Inputs::add`]
    /// fallisce; il fingerprint completo del contratto e' confrontato con
    /// quello del grafo validato, la stessa garanzia di
    /// [`crate::planner::check_input_compatibility`]. Non e' il default perche'
    /// cambierebbe il comportamento dei chiamanti esistenti (rottura semver).
    #[must_use]
    pub fn strict() -> Self {
        Self {
            strict: true,
            ..Self::default()
        }
    }

    /// `true` se questo insieme esige il contratto per ogni input.
    #[must_use]
    pub const fn is_strict(&self) -> bool {
        self.strict
    }

    /// Aggiunge un input per nome, SENZA dichiararne il contratto.
    ///
    /// Il confine si chiude allora sullo schema Arrow completo (campi e
    /// metadati di campo): ferma uno schema diverso, ma NON distingue due
    /// contratti che condividono lo schema e differiscono nella geometria (CRS
    /// risolto contro mancante, tipi dichiarati diversi).
    ///
    /// # Deprecazione
    ///
    /// Il percorso senza contratto resta solo per non rompere i chiamanti
    /// esistenti. Migrare a [`Inputs::add_with_contract`], possibilmente su un
    /// insieme [`Inputs::strict`], che rende l'omissione un errore.
    ///
    /// # Errors
    ///
    /// `PlenoraError::InvalidPlan` se il nome e' gia' presente, oppure se
    /// l'insieme e' nel profilo stretto ([`Inputs::strict`]), dove un input
    /// senza contratto non e' ammesso.
    #[deprecated(
        note = "il confine si chiude solo sullo schema Arrow: usare `add_with_contract` (con `Inputs::strict`)"
    )]
    pub fn add(&mut self, name: impl Into<String>, input: Input) -> Result<()> {
        let name = name.into();
        if self.strict {
            return Err(PlenoraError::InvalidPlan(format!(
                "input `{name}` senza contratto: l'insieme e' nel profilo stretto,                  usa `Inputs::add_with_contract`"
            )));
        }
        match self.readers.entry(name) {
            std::collections::btree_map::Entry::Occupied(occupied) => Err(
                PlenoraError::InvalidPlan(format!("input duplicato `{}`", occupied.key())),
            ),
            std::collections::btree_map::Entry::Vacant(vacant) => {
                vacant.insert(input);
                Ok(())
            }
        }
    }

    /// Aggiunge un input DICHIARANDONE il contratto.
    ///
    /// L'esecuzione verifica il **fingerprint completo** del contratto contro
    /// quello del grafo validato, la stessa garanzia di
    /// [`crate::planner::check_input_compatibility`] al momento in cui i dati
    /// entrano. Senza contratto il confronto e' solo sullo schema (vedi
    /// [`Inputs::add`]).
    ///
    /// # Errors
    ///
    /// Come [`Inputs::add`].
    pub fn add_with_contract(
        &mut self,
        name: impl Into<String>,
        input: Input,
        contract: DataContract,
    ) -> Result<()> {
        let name = name.into();
        // Il duplicato si rileva PRIMA di scrivere: con `insert` il secondo
        // inserimento renderebbe `Err` avendo gia' sostituito il reader, e
        // lascerebbe il vecchio contratto appaiato al nuovo — cioe' proprio
        // la coppia incoerente che il contratto serve a impedire. Un errore
        // deve lasciare `Inputs` invariato.
        if self.readers.contains_key(&name) {
            return Err(PlenoraError::InvalidPlan(format!(
                "input duplicato `{name}`"
            )));
        }
        self.readers.insert(name.clone(), input);
        self.contracts.insert(name, contract);
        Ok(())
    }

    /// Builder: aggiunge un input e restituisce `self`.
    ///
    /// # Deprecazione
    ///
    /// Come [`Inputs::add`], di cui e' la forma builder: usare
    /// [`Inputs::with_contract`].
    ///
    /// # Errors
    ///
    /// Come [`Inputs::add`].
    #[deprecated(
        note = "il confine si chiude solo sullo schema Arrow: usare `with_contract` (con `Inputs::strict`)"
    )]
    pub fn with(mut self, name: impl Into<String>, input: Input) -> Result<Self> {
        // La deprecazione riguarda i CHIAMANTI: qui la delega e' voluta.
        #[allow(deprecated)]
        self.add(name, input)?;
        Ok(self)
    }

    /// Builder: aggiunge un input col suo contratto e restituisce `self`.
    ///
    /// # Errors
    ///
    /// Come [`Inputs::add`].
    pub fn with_contract(
        mut self,
        name: impl Into<String>,
        input: Input,
        contract: DataContract,
    ) -> Result<Self> {
        self.add_with_contract(name, input, contract)?;
        Ok(self)
    }
}
