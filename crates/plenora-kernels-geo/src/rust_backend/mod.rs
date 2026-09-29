//! Backend topologico in Rust puro per `geo.make_valid`, `geo.polygonize` e
//! `geo.split` poligonale.
//!
//! Sostituisce `geos_backend` di `plenora-data-tools@190c493` con le stesse
//! firme e gli stessi nomi d'errore, senza dipendenze native.
//!
//! # Provenienza
//!
//! I tre kernel ([`polygonize`], [`split`], [`make_valid`]) vengono da
//! `plenora-memory-lab/operations/geo_rust`, dove sono stati qualificati
//! contro GEOS (28.672 confronti differenziali su 12.288 configurazioni, 861
//! casi curati e di matrice, 1.097 controlli indipendenti da GEOS). Gli
//! algoritmi sono quelli, riga per riga, salvo le decisioni numeriche
//! corrette sotto. Le sole modifiche:
//!
//! - **meccaniche, per i lint del workspace**: `const fn` dove clippy lo
//!   chiede, `Eq` derivato su `PolygonizeError`, catene `if` di confronto
//!   riscritte come `match` su `Ord::cmp`, `Option::unwrap_or`/`map_or_else`
//!   al posto di `match` equivalenti, chiusure ridondanti sostituite dal
//!   metodo, due bracci identici fusi, `use` riordinati da `rustfmt`, gli
//!   import del crate `plenora-polygonize-rust-candidate` diventati
//!   `super::polygonize`. Nessuna cambia il valore calcolato;
//! - **lint lasciati spenti, con motivo, in testa a ogni file**:
//!   `suboptimal_flops` e `manual_midpoint` (un `mul_add` o `f64::midpoint`
//!   cambierebbe l'arrotondamento, e il double-double di Dekker si regge
//!   proprio su prodotti e somme separati), `float_cmp` (spareggi esatti
//!   voluti), `bool_to_int_with_if` (`usize::from` invertirebbe il ramo su
//!   un NaN), `similar_names`, `too_many_lines`, `needless_pass_by_value`;
//! - **non meccaniche**:
//!   - `split::SplitError::AreaMismatch` non porta piu' le due aree (dato
//!     derivato dalla cella, regola «errori senza dati»); il controllo di
//!     conservazione dell'area e' invariato;
//!   - **segni e ordini d'area esatti** ([`exact`]): il laboratorio decideva
//!     da somme di Gauss in `f64` senza compensazione, che la cancellazione
//!     azzera o scambia (il quadrato unitario in `(2^30, 2^30)` spariva fra
//!     gli anelli invalidi, senza errore). Ora sono esatti: l'orientamento
//!     delle facce in `polygonize::extract_faces`; i confronti d'area che
//!     scelgono genitori e figli in `assemble_face_holes` e
//!     `atomize_contained_faces` (e la loro scorciatoia «tutte uguali»); il
//!     lato del punto nel test pari-dispari di `split` (con `orient2d`
//!     esatto al posto dell'ascissa d'incrocio in `f64`) e il verso del
//!     campione interno; l'«area positiva» del passthrough di `make_valid`.
//!     Fuori dal dominio esatto, quando il filtro non decide, l'errore
//!     `PolygonizeError::NumericRange` sostituisce il segno indovinato. Un
//!     test del laboratorio cambia attesa per questo: un triangolo di area
//!     circa 3,45e-31 che GEOS scarta come anello invalido e' ora un
//!     poligono (vedi `README.md`). Nessun chiamante di [`exact`] traduce
//!     un esito non decidibile in una decisione: `polygonize` e
//!     `make_valid` restituiscono `NumericRange`, e `split::face_sample`
//!     rinuncia al verso e passa a `interior_point`;
//!   - **precondizione degli overlay** in `make_valid`
//!     (`overlay_precondition`): la griglia intera di `i_overlay` (passo
//!     `2^-30` dell'estensione normalizzata) poteva collassare una feature
//!     sottile in un risultato valido ma senza quella parte. Prima di ogni
//!     overlay si verifica che lati, distanze vertice-lato e incroci degli
//!     operandi siano almeno 8 passi di griglia (derivazione nel codice):
//!     altrimenti `MakeValidError::PrecisionInsufficient` e nessun overlay.
//!     Nessun controllo a posteriori certifica il risultato;
//!
//! Decisioni numeriche **non** rese esatte, valutate e dichiarate:
//!
//! - `polygonize`: il punto medio in `f64` dei lati in `union_boundary_rings`
//!   (poi `contains` esatto) e il campione `interior_point` di `geo` per
//!   l'annidamento; lo spareggio per distanza `hypot` fra vicini collineari
//!   nello stesso verso, possibili solo senza noding (archi sovrapposti);
//! - `split`: le tolleranze relative `1e-9` dei controlli a posteriori di
//!   area e copertura (`checked_output`, `point_on_segment`) sono verifiche,
//!   non la scelta delle facce: una scheggia sotto `1e-9` dell'area puo'
//!   passarle; in senso opposto rifiutano (fail-closed) casi che GEOS
//!   risolve esattamente, circa 600 nella campagna traslata di `2^30`;
//! - `make_valid`: lo snap per asse di `restore_multi_snapped`, che riporta
//!   i vertici dell'overlay sulle coordinate sorgente entro
//!   `span * 4 / i32::MAX`: la precondizione degli overlay garantisce che non
//!   cambi la topologia, non che ogni coordinata d'incrocio sia quella
//!   esatta.
//!
//! Classificazioni con tolleranza di `make_valid` rese esatte o fail-closed
//! (la campagna differenziale traslata di `2^30` ha trovato 130 casi in cui
//! `LINEWORK` perdeva tutte le linee dei buchi):
//!
//! - «segmento sul bordo dell'area» e «coordinata rappresentata» in
//!   `linework`: la tolleranza `64 * EPSILON * |coordinata|` (circa `1.5e-5`
//!   a `2^30`) e' sostituita da `orient2d` esatto piu' contenimento esatto;
//!   un punto non esattamente sopra ma entro la banda dello snap dell'overlay
//!   (`span * 2^-26` per asse) e' `PrecisionInsufficient`;
//! - «il buco esce dalla shell» in `linework`: l'area della differenza e'
//!   positiva in modo esatto, non oltre `1e-12 * max(area, 1)`;
//! - `normalized_intersects`, che sceglie fra buco da sottrarre e da
//!   promuovere in `STRUCTURE`: deciso sulle coordinate originali con i
//!   predicati esatti di `geo`, non su quelle normalizzate.
//!
//! Il laboratorio ha girato su `geo` 0.33.1 **non patchato**; qui `geo`
//! risolve alla copia vendorizzata con `orient2d` esatto (filtro veloce piu'
//! ricaduta esatta, `vendor/geo-0.33.1-exact-filtered`). Dove il vecchio
//! predicato sbagliava il segno i due percorsi possono divergere: le prove
//! del laboratorio sono rieseguite qui (test del modulo e
//! `tests/geo_rust_assurance.rs`), la campagna differenziale contro GEOS no.
//!
//! # Contratto comune
//!
//! - **Limiti sempre finiti**: l'adapter chiama solo gli ingressi
//!   `*_bounded` dei kernel. Un limite a [`u64::MAX`] e' rifiutato con
//!   [`RustBackendError::UnboundedLimits`] prima di toccare i dati (GEOS lo
//!   accettava). Un limite superato e' un errore, mai un output troncato.
//! - **Panici delle dipendenze**: il kernel gira dietro
//!   [`calcolo_protetto`](crate::calcolo_protetto); un panico di `geo` o
//!   `i_overlay` diventa [`RustBackendError::CalcoloNonConcluso`], interno,
//!   con la sola forma del payload.
//! - **Errori senza dati**: nessun messaggio porta coordinate, aree o testo
//!   dei kernel; i conteggi dei limiti sono gli stessi che GEOS riportava.
//! - **Determinismo**: stesso input, stesso output, ordine compreso. I
//!   kernel usano solo `BTreeMap`/`BTreeSet` e ordinamenti con spareggio
//!   sull'indice; nessun thread, hash o tempo.
//!
//! Le differenze operazione per operazione sono nella documentazione di
//! [`make_valid_wkb`], [`polygonize_linework`] e
//! [`split_polygon_by_linework`], e in `README.md` («Differenze da GEOS»).

pub mod arrow;
pub mod exact;
pub mod make_valid;
pub mod polygonize;
pub mod split;
mod wkb;

use geo::{CoordsIter, Geometry, LineString, Polygon};
use geozero::{CoordDimensions, ToWkb};
use plenora_core::contract::arrow_metadata::MAX_CELL_COORDINATES;
use plenora_core::PlenoraError;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{geometry_from_wkb, geometry_type_name as geometry_type};

use self::make_valid::{MakeValidError, MakeValidLimits};
use self::polygonize::{PolygonizeError, PolygonizeLimits, PolygonizeOptions};
use self::split::{SplitError, SplitLimits};

/// Lavoro massimo di noding per `polygonize` e `split` poligonale nel
/// percorso Arrow.
///
/// Lo stesso valore del trasporto di `plenora-data-tools@190c493`, con il
/// significato del kernel Rust (coppie di segmenti esaminate, vedi
/// [`polygonize_linework`]).
pub const MAX_NODING_WORK: u64 = 100_000_000;

/// Test di intersezione massimi per lo split lineare (`split_line`), come nel
/// trasporto di `plenora-data-tools@190c493`.
pub const MAX_SPLIT_WORK: u64 = 100_000_000;

/// Coordinate massime di input e di output di `polygonize` nel percorso
/// Arrow, come nel trasporto di `plenora-data-tools@190c493`.
pub const MAX_CLEAN_VERTICES: u64 = 100_000_000;

/// Profilo di [`make_valid_wkb`], che GEOS non limitava.
///
/// Input e output entro il limite per cella ([`MAX_CELL_COORDINATES`]),
/// geometrie di output entro il tetto di componenti del contratto WKB
/// ([`crate::MAX_WKB_COMPONENTS`]), lavoro di noding entro
/// [`MAX_NODING_WORK`]. Il preflight del kernel stima il lavoro come
/// quadrato dei segmenti: sopra 10.000 segmenti un input **invalido** e'
/// rifiutato con [`RustBackendError::WorkLimit`]. Un input gia' valido non
/// arriva al kernel e passa invariato a ogni dimensione.
pub const MAKE_VALID_LIMITS: MakeValidLimits = MakeValidLimits {
    max_input_coordinates: MAX_CELL_COORDINATES,
    max_noding_work: MAX_NODING_WORK,
    max_output_geometries: crate::MAX_WKB_COMPONENTS,
    max_output_coordinates: MAX_CELL_COORDINATES,
};

/// Metodo di riparazione, come in `geos_backend::RepairMethod`.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RepairMethod {
    /// Noda tutto il bordo, estrae le facce e conserva i residui lineari e
    /// puntuali (`MakeValid LINEWORK` di GEOS).
    Linework,
    /// Ripara shell e buchi separatamente, poi li combina con l'overlay
    /// (`GeometryFixer` di GEOS).
    Structure,
}

impl RepairMethod {
    const fn kernel(self) -> make_valid::RepairMethod {
        match self {
            Self::Linework => make_valid::RepairMethod::Linework,
            Self::Structure => make_valid::RepairMethod::Structure,
        }
    }
}

/// Errori del backend Rust: le varianti di `geos_backend::GeosBackendError`
/// meno `Geos`, piu' quelle che solo il kernel Rust puo' dare.
#[derive(Debug, Error)]
pub enum RustBackendError {
    #[error(transparent)]
    InputContract(#[from] PlenoraError),
    #[error("make-valid ha prodotto una geometria ancora non valida")]
    InvalidRepair,
    #[error("tipo geometria non supportato da {operation}: {actual}")]
    UnsupportedGeometry {
        operation: &'static str,
        actual: &'static str,
    },
    #[error("coordinate oltre il limite di {limit}: {actual}")]
    CoordinateLimit { actual: u64, limit: u64 },
    #[error("output oltre il limite di {limit}: {actual}")]
    OutputLimit { actual: u64, limit: u64 },
    #[error("lavoro di noding oltre il limite di {limit}: {actual}")]
    WorkLimit { actual: u64, limit: u64 },
    #[error(
        "polygonize incompleto: cuts={cuts}, dangles={dangles}, invalid_rings={invalid_rings}"
    )]
    IncompletePolygonize {
        cuts: usize,
        dangles: usize,
        invalid_rings: usize,
    },
    /// L'output del kernel non supera la sua stessa validazione. Il payload
    /// e' il passo, mai il testo del validatore.
    #[error("output del backend Rust non valido: {0}")]
    InvalidOutput(&'static str),
    #[error("lo split poligonale non conserva l'area dell'input")]
    AreaMismatch,
    #[error("lo split poligonale non ricopre esattamente l'input")]
    CoverageMismatch,
    /// Un limite lasciato a [`u64::MAX`]: gli ingressi `*_bounded` del
    /// kernel non ammettono profili aperti.
    #[error("profilo limiti incompleto: input, noding e output devono avere tetti espliciti")]
    UnboundedLimits,
    /// Il noding iterativo non ha raggiunto un grafo interamente nodato entro
    /// le iterazioni previste: l'input e' fuori dal dominio che il kernel
    /// sa trattare, non necessariamente sbagliato.
    #[error("noding non convergente: input fuori dal dominio del kernel Rust")]
    NodingDidNotConverge,
    /// Un segno o un confronto d'area non decidibile in `f64` su coordinate
    /// fuori dal dominio dell'aritmetica esatta (vedi [`exact`]): un errore
    /// al posto di un segno indovinato.
    #[error("coordinate fuori dal dominio dell'aritmetica esatta delle aree")]
    NumericRange,
    /// Un overlay di `make_valid` avrebbe lavorato su feature sotto la
    /// risoluzione della griglia intera di `i_overlay`: non viene eseguito
    /// (precondizione `make_valid::overlay_precondition`).
    #[error("feature dell'overlay sotto la risoluzione della griglia intera")]
    PrecisionInsufficient,
    /// Una prenotazione di memoria fallita.
    #[error("prenotazione di memoria fallita per {0}")]
    AllocationFailed(&'static str),
    /// Il kernel e' andato in panico dentro la barriera: non conclude, e non
    /// accusa l'ingresso. Porta la forma del payload, mai il contenuto.
    #[error("calcolo non concluso: {0} (contenuto non pubblicato)")]
    CalcoloNonConcluso(&'static str),
    /// Invariante interna del kernel violata.
    #[error("errore interno del backend Rust: {0}")]
    Internal(&'static str),
}

impl From<RustBackendError> for PlenoraError {
    /// La stessa attribuzione che il passo dell'executor faceva sugli errori
    /// GEOS: interno cio' che e' interno, `InvalidPlan` il resto (limiti
    /// compresi). In piu' la memoria esaurita e' `ResourceLimit` e il noding
    /// non convergente e' `Unsupported`, casi che GEOS non aveva.
    fn from(error: RustBackendError) -> Self {
        match error {
            RustBackendError::InputContract(error) => error,
            RustBackendError::CalcoloNonConcluso(_) | RustBackendError::Internal(_) => {
                Self::Internal(error.to_string())
            }
            RustBackendError::AllocationFailed(_) => Self::ResourceLimit(error.to_string()),
            RustBackendError::NodingDidNotConverge
            | RustBackendError::NumericRange
            | RustBackendError::PrecisionInsufficient => Self::Unsupported(error.to_string()),
            other => Self::InvalidPlan(other.to_string()),
        }
    }
}

impl RustBackendError {
    /// Traduce l'errore del polygonize, nominando l'operazione pubblica che
    /// lo ha chiamato.
    fn from_polygonize(error: &PolygonizeError, operation: &'static str) -> Self {
        match *error {
            PolygonizeError::UnsupportedGeometry(actual) => {
                Self::UnsupportedGeometry { operation, actual }
            }
            PolygonizeError::InvalidInput(_) => Self::InputContract(PlenoraError::InvalidPlan(
                format!("{operation}: linework rifiutato dalla validazione del kernel"),
            )),
            PolygonizeError::UnboundedLimitConfiguration => Self::UnboundedLimits,
            PolygonizeError::CoordinateLimit { actual, limit } => {
                Self::CoordinateLimit { actual, limit }
            }
            PolygonizeError::WorkLimit { actual, limit } => Self::WorkLimit { actual, limit },
            PolygonizeError::OutputLimit { actual, limit } => Self::OutputLimit { actual, limit },
            PolygonizeError::NodingDidNotConverge { .. } => Self::NodingDidNotConverge,
            PolygonizeError::NumericRange => Self::NumericRange,
            // L'adapter chiama sempre con `require_complete = false` e
            // controlla i residui da se', per riportarli per classe.
            PolygonizeError::Incomplete { .. } => {
                Self::Internal("residui del polygonize richiesti al kernel")
            }
            PolygonizeError::InvalidOutput(_) => Self::InvalidOutput("faccia poligonale"),
            PolygonizeError::IndexOverflow => Self::Internal("indice non rappresentabile"),
            PolygonizeError::InternalInvariant(_) => {
                Self::Internal("invariante del polygonize violata")
            }
            PolygonizeError::AllocationFailed(context) => Self::AllocationFailed(context),
        }
    }

    fn from_split(error: SplitError) -> Self {
        match error {
            SplitError::UnsupportedSource(actual) | SplitError::UnsupportedSplitter(actual) => {
                Self::UnsupportedGeometry {
                    operation: "split",
                    actual,
                }
            }
            SplitError::InvalidInput(_) => Self::InputContract(PlenoraError::InvalidPlan(
                "split: input rifiutato dalla validazione del kernel".to_owned(),
            )),
            SplitError::UnboundedLimitConfiguration => Self::UnboundedLimits,
            SplitError::CoordinateLimit { actual, limit } => {
                Self::CoordinateLimit { actual, limit }
            }
            SplitError::WorkLimit { actual, limit } => Self::WorkLimit { actual, limit },
            SplitError::OutputLimit { actual, limit } => Self::OutputLimit { actual, limit },
            SplitError::Polygonize(error) => Self::from_polygonize(&error, "split"),
            SplitError::AreaMismatch => Self::AreaMismatch,
            SplitError::CoverageMismatch => Self::CoverageMismatch,
            SplitError::IndexOverflow => Self::Internal("indice non rappresentabile"),
            SplitError::InternalInvariant(_) => Self::Internal("invariante dello split violata"),
            SplitError::AllocationFailed(context) => Self::AllocationFailed(context),
        }
    }

    fn from_make_valid(error: MakeValidError) -> Self {
        match error {
            MakeValidError::NonFiniteCoordinate => {
                Self::InputContract(crate::non_finite_coordinate())
            }
            MakeValidError::InvalidStructure(reason) => {
                Self::InputContract(crate::invalid_wkb_structure(reason))
            }
            MakeValidError::UnsupportedGeometry(actual) => Self::UnsupportedGeometry {
                operation: "make_valid",
                actual,
            },
            MakeValidError::UnboundedLimitConfiguration => Self::UnboundedLimits,
            MakeValidError::CoordinateLimit { actual, limit } => {
                Self::CoordinateLimit { actual, limit }
            }
            MakeValidError::WorkLimit { actual, limit } => Self::WorkLimit { actual, limit },
            MakeValidError::OutputLimit { actual, limit } => Self::OutputLimit { actual, limit },
            MakeValidError::IndexOverflow => Self::Internal("indice non rappresentabile"),
            MakeValidError::Polygonize(error) => Self::from_polygonize(&error, "make_valid"),
            MakeValidError::InvalidOutput(_) => Self::InvalidRepair,
            MakeValidError::InternalInvariant(_) => {
                Self::Internal("invariante del make_valid violata")
            }
            MakeValidError::AllocationFailed(context) => Self::AllocationFailed(context),
            MakeValidError::NumericRange => Self::NumericRange,
            MakeValidError::PrecisionInsufficient => Self::PrecisionInsufficient,
        }
    }
}

/// Esegue un kernel dietro la barriera dei panici delle dipendenze.
fn protetto<T, E>(
    calcolo: impl FnOnce() -> Result<T, E>,
    traduci: impl FnOnce(E) -> RustBackendError,
) -> Result<T, RustBackendError> {
    match crate::calcolo_protetto(calcolo) {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(error)) => Err(traduci(error)),
        Err(forma) => Err(RustBackendError::CalcoloNonConcluso(forma)),
    }
}

fn ensure_linework(
    geometry: &Geometry<f64>,
    operation: &'static str,
) -> Result<(), RustBackendError> {
    match geometry {
        Geometry::LineString(_) | Geometry::MultiLineString(_) => Ok(()),
        Geometry::GeometryCollection(collection) => {
            for child in &collection.0 {
                ensure_linework(child, operation)?;
            }
            Ok(())
        }
        _ => Err(RustBackendError::UnsupportedGeometry {
            operation,
            actual: geometry_type(geometry),
        }),
    }
}

/// Il gate d'ingresso che il backend GEOS applicava prima della libreria
/// nativa: limite di coordinate, poi contratto WKB strutturale e validazione
/// OGC del workspace sulla forma canonica XY. I kernel rifanno la propria
/// validazione con `geo`; questa resta la definizione di «valido» del
/// workspace, che rifiuta anche gli anelli con punta.
fn checked_input(geometry: &Geometry<f64>, max_coordinates: u64) -> Result<(), RustBackendError> {
    let actual =
        u64::try_from(geometry.coords_count()).map_err(|_| RustBackendError::CoordinateLimit {
            actual: u64::MAX,
            limit: max_coordinates,
        })?;
    if actual > max_coordinates {
        return Err(RustBackendError::CoordinateLimit {
            actual,
            limit: max_coordinates,
        });
    }
    let payload = geometry
        .to_wkb(CoordDimensions::xy())
        .map_err(|_| RustBackendError::Internal("codifica WKB intermedia"))?;
    geometry_from_wkb(&payload)?;
    Ok(())
}

/// Ripara un WKB 2D strutturalmente valido.
///
/// Come in GEOS l'input puo' essere OGC-invalido, perche' e' cio' che
/// l'operazione ripara; dimensioni Z/M, coordinate non finite e WKB
/// malformato restano rifiutati prima del kernel. Un input gia' valido
/// (validazione OGC del workspace) torna **byte per byte**, senza passare dal
/// kernel.
///
/// Differenze dal backend GEOS:
///
/// - l'algoritmo e' quello del laboratorio (`LINEWORK`: noding del bordo,
///   facce e residui; `STRUCTURE`: shell e buchi riparati a parte, poi
///   overlay di `geo`), qualificato contro GEOS per equivalenza semantica,
///   non byte per byte: ordine dei componenti, punto iniziale e verso degli
///   anelli, e la forma `Polygon`/`MultiPolygon`/`GeometryCollection` di un
///   risultato possono differire da GEOS a parita' di geometria;
/// - nelle `GeometryCollection` `keep_collapsed` non si propaga ai figli di
///   `STRUCTURE` (come `GeometryFixer` di GEOS), mentre `LINEWORK` li ripara
///   sempre con `keep_collapsed = true`;
/// - l'input invalido e' soggetto a [`MAKE_VALID_LIMITS`], che GEOS non
///   aveva: oltre 10.000 segmenti la riparazione fallisce chiusa con
///   [`RustBackendError::WorkLimit`];
/// - «valido» e' la validazione OGC del workspace (quella di `geo` piu' il
///   controllo degli anelli con punta), non `IsValid` di GEOS: dove le due
///   divergono, passthrough e riparazione possono scambiarsi.
///
/// # Errors
///
/// [`RustBackendError::InputContract`] se il payload viola il contratto WKB;
/// i limiti, [`RustBackendError::InvalidRepair`] se l'output resta invalido
/// per il kernel o per la validazione del workspace, e gli altri errori del
/// kernel tradotti in [`RustBackendError`].
pub fn make_valid_wkb(
    payload: &[u8],
    method: RepairMethod,
    keep_collapsed: bool,
) -> Result<Vec<u8>, RustBackendError> {
    make_valid_wkb_with_limits(payload, method, keep_collapsed, MAKE_VALID_LIMITS)
}

/// [`make_valid_wkb`] con un profilo di limiti esplicito.
///
/// # Errors
///
/// Come [`make_valid_wkb`]; [`RustBackendError::UnboundedLimits`] se un
/// limite e' [`u64::MAX`], anche su input gia' valido.
pub fn make_valid_wkb_with_limits(
    payload: &[u8],
    method: RepairMethod,
    keep_collapsed: bool,
    limits: MakeValidLimits,
) -> Result<Vec<u8>, RustBackendError> {
    if !limits.is_fully_bounded() {
        return Err(RustBackendError::UnboundedLimits);
    }
    // Il decoder validante e' il gate strutturale (`validate_wkb_contract`,
    // stessa parita' di esiti) e costruisce la geometria nella stessa
    // passata; la validazione OGC viene dopo, perche' l'input invalido e'
    // ammesso.
    let geometry = crate::wkb_decoder::decode_validated(payload)?;
    match crate::valida_ogc(&geometry) {
        Ok(()) => return Ok(payload.to_vec()),
        Err(PlenoraError::InvalidPlan(_)) => {}
        // Una validazione che non conclude non dimostra che l'input vada
        // riparato: si ferma, interna.
        Err(other) => return Err(other.into()),
    }
    let repaired = protetto(
        || {
            make_valid::make_valid_geometry_rust_bounded(
                &geometry,
                method.kernel(),
                keep_collapsed,
                limits,
            )
        },
        RustBackendError::from_make_valid,
    )?;
    // Encoder proprio: il poligono vuoto di un anello collassato esce a zero
    // anelli, come in GEOS (vedi `wkb`).
    let output = wkb::wkb_xy(&repaired)?;
    // Rivalidazione dell'output con il contratto del workspace (nessuna
    // fiducia nel produttore): struttura, poi OGC.
    let decoded = crate::wkb_decoder::decode_validated(&output)?;
    match crate::valida_ogc(&decoded) {
        Ok(()) => Ok(output),
        Err(PlenoraError::InvalidPlan(_)) => Err(RustBackendError::InvalidRepair),
        Err(other) => Err(other.into()),
    }
}

/// Variante di [`make_valid_wkb`] su geometria gia' decodificata: passa per
/// la stessa forma canonica XY, quindi stessi risultati e stessi errori.
///
/// # Errors
///
/// Come [`make_valid_wkb`]; [`RustBackendError::Internal`] se la codifica
/// WKB intermedia fallisce.
pub fn make_valid_geometry(
    geometry: &Geometry<f64>,
    method: RepairMethod,
    keep_collapsed: bool,
) -> Result<Geometry<f64>, RustBackendError> {
    let payload = wkb::wkb_xy(geometry)?;
    let repaired = make_valid_wkb(&payload, method, keep_collapsed)?;
    geometry_from_wkb(&repaired).map_err(RustBackendError::from)
}

/// Esito di [`polygonize_linework`], con la forma di
/// `geos_backend::PolygonizeResult` a 190c493 (in piu' `PartialEq`).
#[derive(Clone, Debug, PartialEq)]
pub struct PolygonizeResult {
    pub polygons: Vec<Polygon<f64>>,
    pub cut_edges: Vec<LineString<f64>>,
    pub dangles: Vec<LineString<f64>>,
    pub invalid_ring_lines: Vec<LineString<f64>>,
}

impl PolygonizeResult {
    /// Residui di tutte le classi.
    ///
    /// Infallibile come a 190c493: tre `Vec` di `LineString` (24 byte per
    /// elemento) non superano insieme `3 * isize::MAX / 24 < usize::MAX`
    /// elementi, quindi la somma non satura mai; `saturating_add` tiene
    /// comunque fuori l'overflow dal percorso.
    #[must_use]
    pub const fn residual_count(&self) -> usize {
        self.cut_edges
            .len()
            .saturating_add(self.dangles.len())
            .saturating_add(self.invalid_ring_lines.len())
    }
}

impl From<polygonize::PolygonizeResult> for PolygonizeResult {
    fn from(result: polygonize::PolygonizeResult) -> Self {
        Self {
            polygons: result.polygons,
            cut_edges: result.cut_edges,
            dangles: result.dangles,
            invalid_ring_lines: result.invalid_ring_lines,
        }
    }
}

/// Polygonizza linework 2D valido e conserva ogni categoria di residuo.
/// `node_input` inserisce esplicitamente i nodi d'incrocio prima della
/// costruzione del grafo.
///
/// Differenze dal backend GEOS:
///
/// - **ordine**: poligoni nell'ordine di estrazione delle facce (chiavi di
///   coordinata ordinate), anelli esterni antiorari; residui divisi in
///   `cut_edges`, `dangles`, `invalid_ring_lines` come in GEOS, ma ognuno
///   nell'ordine del grafo, non in quello di GEOS. Il contenuto e'
///   equivalente a GEOS sui casi qualificati, l'ordine no;
/// - **forma canonica**: su input permutato o invertito l'output e' lo
///   stesso quando i punti d'intersezione sono gli stessi bit (sempre su
///   coordinate e intersezioni esattamente rappresentabili). In generale no:
///   il segmento superstite fra duplicati e il segno dello zero di una
///   coordinata seguono l'ordine d'ingresso, e senza noding le linee
///   duplicate escono come `cut_edges` nell'ordine d'ingresso;
/// - `max_noding_work` conta le coppie di segmenti **esaminate** dal noding
///   e dalla sua validazione, iterazioni comprese, addebitate mentre
///   accadono; GEOS stimava prima il quadrato dei segmenti. Il limite scatta
///   durante il noding, su input diversi da quelli che GEOS rifiutava;
/// - `max_output_geometries` e `max_output_coordinates` valgono anche sulle
///   facce intermedie, prima dell'assemblaggio dei buchi;
/// - un noding che non converge e' [`RustBackendError::NodingDidNotConverge`];
/// - un limite a [`u64::MAX`] e' [`RustBackendError::UnboundedLimits`].
///
/// # Errors
///
/// [`RustBackendError::UnsupportedGeometry`] se l'input non e' linework;
/// limiti di coordinate, noding e output; il contratto WKB e la validazione
/// OGC dell'input; [`RustBackendError::IncompletePolygonize`] se
/// `require_complete` e' attivo e restano residui; gli altri errori del
/// kernel tradotti.
pub fn polygonize_linework(
    linework: &Geometry<f64>,
    node_input: bool,
    require_complete: bool,
    max_input_coordinates: u64,
    max_noding_work: u64,
    max_output_geometries: u64,
    max_output_coordinates: u64,
) -> Result<PolygonizeResult, RustBackendError> {
    let limits = PolygonizeLimits {
        max_input_coordinates,
        max_noding_work,
        max_output_geometries,
        max_output_coordinates,
    };
    if !limits.is_fully_bounded() {
        return Err(RustBackendError::UnboundedLimits);
    }
    ensure_linework(linework, "polygonize")?;
    checked_input(linework, max_input_coordinates)?;
    let result = protetto(
        || {
            polygonize::polygonize_linework_rust_bounded(
                linework,
                PolygonizeOptions {
                    node_input,
                    require_complete: false,
                    limits,
                },
            )
        },
        |error| RustBackendError::from_polygonize(&error, "polygonize"),
    )?;
    let result = PolygonizeResult::from(result);
    if require_complete && result.residual_count() != 0 {
        return Err(RustBackendError::IncompletePolygonize {
            cuts: result.cut_edges.len(),
            dangles: result.dangles.len(),
            invalid_rings: result.invalid_ring_lines.len(),
        });
    }
    Ok(result)
}

/// Divide un Polygon/MultiPolygon con geometria lineare.
///
/// Bordo della sorgente e splitter sono nodati insieme dal polygonize Rust;
/// le facce il cui punto campione cade nella sorgente sono le parti, e il
/// risultato e' verificato in modo indipendente per conservazione dell'area
/// e copertura del bordo (tolleranza relativa `1e-9`).
///
/// Differenze dal backend GEOS:
///
/// - la scelta delle facce usa il campione interno del laboratorio e un test
///   pari-dispari sugli anelli, non `point_on_surface` + `covers`; la
///   copertura e' verificata sul bordo (ogni segmento della sorgente coperto
///   una volta, ogni segmento interno due), non con la differenza
///   simmetrica di GEOS. Un'incoerenza resta un errore esplicito
///   ([`RustBackendError::AreaMismatch`] o
///   [`RustBackendError::CoverageMismatch`]), mai parti in piu' o in meno;
/// - **ordine**: le parti escono nell'ordine delle facce del polygonize, non
///   in quello di GEOS; stessa regola di forma canonica di
///   [`polygonize_linework`];
/// - `max_input_coordinates` vale per ciascun input (come in GEOS) **e** per
///   la loro somma (kernel); `max_noding_work` ha il significato di
///   [`polygonize_linework`]; `max_output_parts` e `max_output_coordinates`
///   valgono anche sul polygonize intermedio: facce fuori dalla sorgente e
///   residui scartati (dangle e cut edge delle lame che sporgono) contano
///   come parti, mentre GEOS contava solo le parti tenute.
///
/// # Errors
///
/// [`RustBackendError::UnsupportedGeometry`] se la sorgente non e'
/// poligonale o lo splitter non e' linework; limiti; contratto WKB e
/// validazione OGC degli input; area o copertura non conservate; gli altri
/// errori del kernel tradotti.
pub fn split_polygon_by_linework(
    source: &Geometry<f64>,
    splitter: &Geometry<f64>,
    max_input_coordinates: u64,
    max_noding_work: u64,
    max_output_parts: u64,
    max_output_coordinates: u64,
) -> Result<Vec<Polygon<f64>>, RustBackendError> {
    let limits = SplitLimits {
        max_input_coordinates,
        max_noding_work,
        max_output_parts,
        max_output_coordinates,
    };
    if !limits.is_fully_bounded() {
        return Err(RustBackendError::UnboundedLimits);
    }
    if !matches!(source, Geometry::Polygon(_) | Geometry::MultiPolygon(_)) {
        return Err(RustBackendError::UnsupportedGeometry {
            operation: "split",
            actual: geometry_type(source),
        });
    }
    ensure_linework(splitter, "split")?;
    checked_input(source, max_input_coordinates)?;
    checked_input(splitter, max_input_coordinates)?;
    protetto(
        || split::split_polygon_by_linework_rust_bounded(source, splitter, limits),
        RustBackendError::from_split,
    )
}

/// Le linee residue di un [`PolygonizeResult`] per classe, nell'ordine del
/// contratto (`cut_edge`, `dangle`, `invalid_ring`).
#[must_use]
pub fn residual_classes(result: &PolygonizeResult) -> [(&'static str, &[LineString<f64>]); 3] {
    [
        ("cut_edge", &result.cut_edges),
        ("dangle", &result.dangles),
        ("invalid_ring", &result.invalid_ring_lines),
    ]
}

// I test di `geos_backend` di `plenora-data-tools@190c493` e la parte GEOS di
// `plenora-cli/tests/geo_adversarial.rs`, sullo stesso contratto: le attese
// (conteggi, aree, varianti d'errore) sono quelle che GEOS soddisfaceva.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{bowtie, rect, rect_polygon};
    use crate::ValidazioneProtetta as _;
    use geo::{line_string, polygon, Area, GeometryCollection, Point};

    fn bow_tie_wkb() -> Vec<u8> {
        bowtie().to_wkb(CoordDimensions::xy()).unwrap()
    }

    #[test]
    fn repairs_self_intersection_with_both_methods() {
        let input = bow_tie_wkb();
        assert!(geometry_from_wkb(&input).is_err());
        for method in [RepairMethod::Linework, RepairMethod::Structure] {
            let output = make_valid_wkb(&input, method, false).unwrap();
            let repaired = geometry_from_wkb(&output).unwrap();
            assert!((repaired.unsigned_area() - 2.0).abs() < 1e-12);
        }
    }

    #[test]
    fn valid_input_is_returned_byte_for_byte() {
        let input = rect(0.0, 0.0, 2.0, 2.0)
            .to_wkb(CoordDimensions::xy())
            .unwrap();
        assert_eq!(
            make_valid_wkb(&input, RepairMethod::Structure, false).unwrap(),
            input
        );
    }

    #[test]
    fn malformed_or_non_finite_wkb_never_reaches_the_kernel() {
        assert!(make_valid_wkb(&[1, 2, 3], RepairMethod::Structure, false).is_err());
        let mut nan_point = vec![1_u8, 1, 0, 0, 0];
        nan_point.extend_from_slice(&f64::NAN.to_le_bytes());
        nan_point.extend_from_slice(&1.0_f64.to_le_bytes());
        assert!(matches!(
            make_valid_wkb(&nan_point, RepairMethod::Structure, false),
            Err(RustBackendError::InputContract(_))
        ));
    }

    #[test]
    fn polygonize_preserves_residual_linework_and_can_fail_closed() {
        let linework = Geometry::MultiLineString(geo::MultiLineString(vec![
            line_string![(x: 0.0, y: 0.0), (x: 2.0, y: 0.0)],
            line_string![(x: 2.0, y: 0.0), (x: 2.0, y: 2.0)],
            line_string![(x: 2.0, y: 2.0), (x: 0.0, y: 2.0)],
            line_string![(x: 0.0, y: 2.0), (x: 0.0, y: 0.0)],
            line_string![(x: 2.0, y: 2.0), (x: 3.0, y: 2.0)],
        ]));
        let result = polygonize_linework(&linework, true, false, 100, 1_000, 100, 100).unwrap();
        assert_eq!(result.polygons.len(), 1);
        assert_eq!(result.residual_count(), 1);
        assert!(matches!(
            polygonize_linework(&linework, true, true, 100, 1_000, 100, 100),
            Err(RustBackendError::IncompletePolygonize {
                cuts: 0,
                dangles: 1,
                invalid_rings: 0
            })
        ));
        assert!(matches!(
            polygonize_linework(&linework, true, false, 4, 1_000, 100, 100),
            Err(RustBackendError::CoordinateLimit { .. })
        ));
        assert!(matches!(
            polygonize_linework(&linework, true, false, 100, 1_000, 1, 100),
            Err(RustBackendError::OutputLimit { .. })
        ));
        assert!(matches!(
            polygonize_linework(&linework, true, false, 100, 1, 100, 100),
            Err(RustBackendError::WorkLimit { .. })
        ));
        assert!(matches!(
            polygonize_linework(&linework, true, false, 100, 1_000, 100, 6),
            Err(RustBackendError::OutputLimit { .. })
        ));
    }

    #[test]
    fn polygon_split_conserves_area_and_ignores_outside_faces() {
        let source = rect(0.0, 0.0, 10.0, 10.0);
        let splitter = Geometry::MultiLineString(geo::MultiLineString(vec![
            line_string![(x: 5.0, y: -1.0), (x: 5.0, y: 11.0)],
            line_string![(x: 20.0, y: 20.0), (x: 22.0, y: 20.0),
                         (x: 22.0, y: 22.0), (x: 20.0, y: 22.0),
                         (x: 20.0, y: 20.0)],
        ]));
        let pieces = split_polygon_by_linework(&source, &splitter, 100, 10_000, 10, 100).unwrap();
        assert_eq!(pieces.len(), 2);
        let total_area = pieces.iter().map(Area::unsigned_area).sum::<f64>();
        assert!((total_area - 100.0).abs() < f64::EPSILON);
        assert!(matches!(
            split_polygon_by_linework(&source, &splitter, 100, 10_000, 1, 100),
            Err(RustBackendError::OutputLimit { .. })
        ));
        assert!(matches!(
            split_polygon_by_linework(&source, &splitter, 100, 1, 10, 100),
            Err(RustBackendError::WorkLimit { .. })
        ));
        assert!(matches!(
            split_polygon_by_linework(&source, &splitter, 100, 10_000, 10, 9),
            Err(RustBackendError::OutputLimit { .. })
        ));
    }

    #[test]
    fn polygon_split_handles_holes_multipolygons_and_boundary_coincidence() {
        let holed = Geometry::Polygon(Polygon::new(
            line_string![
                (x: 0.0, y: 0.0), (x: 10.0, y: 0.0),
                (x: 10.0, y: 10.0), (x: 0.0, y: 10.0),
                (x: 0.0, y: 0.0)
            ],
            vec![line_string![
                (x: 4.0, y: 4.0), (x: 6.0, y: 4.0),
                (x: 6.0, y: 6.0), (x: 4.0, y: 6.0),
                (x: 4.0, y: 4.0)
            ]],
        ));
        let through_hole = Geometry::LineString(line_string![(x: 5.0, y: -1.0), (x: 5.0, y: 11.0)]);
        let pieces =
            split_polygon_by_linework(&holed, &through_hole, 100, 10_000, 10, 100).unwrap();
        assert_eq!(pieces.len(), 2);
        assert!((pieces.iter().map(Area::unsigned_area).sum::<f64>() - 96.0).abs() < 1e-12);

        let source = Geometry::MultiPolygon(geo::MultiPolygon(vec![
            rect_polygon(0.0, 0.0, 2.0, 2.0),
            rect_polygon(4.0, 0.0, 6.0, 2.0),
        ]));
        let cutter = Geometry::LineString(line_string![(x: -1.0, y: 1.0), (x: 7.0, y: 1.0)]);
        let pieces = split_polygon_by_linework(&source, &cutter, 100, 10_000, 10, 100).unwrap();
        assert_eq!(pieces.len(), 4);
        assert!((pieces.iter().map(Area::unsigned_area).sum::<f64>() - 8.0).abs() < 1e-12);

        let boundary = Geometry::LineString(line_string![(x: 0.0, y: 0.0), (x: 2.0, y: 0.0)]);
        let pieces = split_polygon_by_linework(&source, &boundary, 100, 10_000, 10, 100).unwrap();
        assert_eq!(pieces.len(), 2);
        assert!((pieces.iter().map(Area::unsigned_area).sum::<f64>() - 8.0).abs() < 1e-12);
    }

    #[test]
    fn adversarial_backend_types_empty_linework_and_non_noded_modes() {
        let invalid_types = vec![
            Geometry::Point(geo::Point::new(0.0, 0.0)),
            Geometry::MultiPoint(vec![geo::Point::new(0.0, 0.0)].into()),
            rect(0.0, 0.0, 1.0, 1.0),
            Geometry::Rect(geo::Rect::new((0.0, 0.0), (1.0, 1.0))),
            Geometry::Triangle(geo::Triangle::new(
                geo::Coord { x: 0.0, y: 0.0 },
                geo::Coord { x: 1.0, y: 0.0 },
                geo::Coord { x: 0.0, y: 1.0 },
            )),
        ];
        for geometry in invalid_types {
            assert!(matches!(
                polygonize_linework(&geometry, true, false, 100, 1_000, 100, 100),
                Err(RustBackendError::UnsupportedGeometry { .. })
            ));
        }

        let empty = Geometry::MultiLineString(geo::MultiLineString(Vec::new()));
        // GEOS accettava `max_noding_work = 0` senza noding; il kernel Rust
        // chiede ogni limite finito, e zero lo e'.
        let result = polygonize_linework(&empty, false, true, 100, 0, 100, 100).unwrap();
        assert!(result.polygons.is_empty());
        assert_eq!(result.residual_count(), 0);

        let square_lines = Geometry::GeometryCollection(
            vec![
                Geometry::LineString(line_string![(x: 0.0, y: 0.0), (x: 2.0, y: 0.0)]),
                Geometry::MultiLineString(geo::MultiLineString(vec![
                    line_string![(x: 2.0, y: 0.0), (x: 2.0, y: 2.0)],
                    line_string![(x: 2.0, y: 2.0), (x: 0.0, y: 2.0)],
                    line_string![(x: 0.0, y: 2.0), (x: 0.0, y: 0.0)],
                ])),
            ]
            .into(),
        );
        assert_eq!(
            polygonize_linework(&square_lines, false, true, 100, 0, 100, 100)
                .unwrap()
                .polygons
                .len(),
            1
        );

        let source = rect(0.0, 0.0, 2.0, 2.0);
        assert_eq!(
            split_polygon_by_linework(&source, &empty, 100, 1_000, 10, 100)
                .unwrap()
                .len(),
            1
        );
        assert!(matches!(
            split_polygon_by_linework(&source, &square_lines, 1, 1_000, 10, 100),
            Err(RustBackendError::CoordinateLimit { .. })
        ));
    }

    #[test]
    fn make_valid_geometry_matches_the_wkb_path() {
        // architettura.md#geometrie D12.1: la variante su forma decodificata
        // deve produrre la STESSA geometria del percorso WKB, sull'input
        // OGC-invalido che l'operazione esiste per riparare.
        let input = bow_tie_wkb();
        let decoded = crate::wkb_decoder::decode_validated(&input).expect("gate solo strutturale");
        let via_geometry = make_valid_geometry(&decoded, RepairMethod::Linework, true)
            .expect("riparazione su forma decodificata");
        let via_wkb = make_valid_wkb(&input, RepairMethod::Linework, true).expect("wkb");
        assert_eq!(
            via_geometry,
            geometry_from_wkb(&via_wkb).expect("output wkb valido"),
            "risultato diverso tra forma decodificata e WKB"
        );
        // Input gia' valido: passthrough (stessa geometria in uscita).
        let valid = rect(0.0, 0.0, 2.0, 2.0);
        assert_eq!(
            make_valid_geometry(&valid, RepairMethod::Linework, true).expect("passthrough"),
            valid
        );
    }

    /// `Line`, `Rect` e `Triangle` entrano come a 190c493: geozero li
    /// codificava come `LineString` e `Polygon` (`to_polygon` di `geo`),
    /// anche dentro una collezione; gia' validi, tornano in quella forma.
    #[test]
    fn make_valid_geometry_normalizes_line_rect_and_triangle() {
        let rect = geo::Rect::new((0.0, 0.0), (2.0, 1.0));
        let triangle = geo::Triangle::new(
            geo::Coord { x: 0.0, y: 0.0 },
            geo::Coord { x: 3.0, y: 0.0 },
            geo::Coord { x: 0.0, y: 3.0 },
        );
        let line = geo::Line::new(geo::Coord { x: 0.0, y: 0.0 }, geo::Coord { x: 1.0, y: 1.0 });
        let cases = [
            (Geometry::Rect(rect), Geometry::Polygon(rect.to_polygon())),
            (
                Geometry::Triangle(triangle),
                Geometry::Polygon(triangle.to_polygon()),
            ),
            (
                Geometry::Line(line),
                Geometry::LineString(LineString::new(vec![line.start, line.end])),
            ),
            (
                Geometry::GeometryCollection(GeometryCollection::new_from(vec![
                    Geometry::Rect(rect),
                    Geometry::Line(line),
                ])),
                Geometry::GeometryCollection(GeometryCollection::new_from(vec![
                    Geometry::Polygon(rect.to_polygon()),
                    Geometry::LineString(LineString::new(vec![line.start, line.end])),
                ])),
            ),
        ];
        for (input, expected) in cases {
            for method in [RepairMethod::Linework, RepairMethod::Structure] {
                let output = make_valid_geometry(&input, method, true).expect("passthrough");
                assert_eq!(output, expected);
                // Stessi byte che geozero dava a 190c493.
                assert_eq!(
                    wkb::wkb_xy(&input).unwrap(),
                    input.to_wkb(CoordDimensions::xy()).unwrap()
                );
            }
        }
    }

    #[test]
    fn make_valid_keep_collapsed_handles_degenerate_invalid_polygon() {
        let degenerate = Geometry::Polygon(polygon![
            (x: 0.0, y: 0.0), (x: 1.0, y: 0.0),
            (x: 2.0, y: 0.0), (x: 0.0, y: 0.0)
        ])
        .to_wkb(CoordDimensions::xy())
        .unwrap();
        for keep_collapsed in [false, true] {
            let output =
                make_valid_wkb(&degenerate, RepairMethod::Structure, keep_collapsed).unwrap();
            let repaired = geometry_from_wkb(&output).unwrap();
            assert!(repaired.validazione_protetta().is_ok());
        }
    }

    /// `geos_and_proj_reject_hostile_inputs_and_handle_complex_topology` di
    /// `plenora-cli/tests/geo_adversarial.rs`, parte GEOS.
    #[test]
    fn rejects_hostile_inputs_and_handles_complex_topology() {
        let square = |x: f64, y: f64, size: f64| rect(x, y, x + size, y + size);
        assert!(make_valid_wkb(&[1, 2, 3], RepairMethod::Linework, false).is_err());
        let valid = square(0.0, 0.0, 4.0).to_wkb(CoordDimensions::xy()).unwrap();
        assert_eq!(
            make_valid_wkb(&valid, RepairMethod::Structure, false).unwrap(),
            valid
        );

        let wrong = Geometry::Point(Point::new(0.0, 0.0));
        assert!(matches!(
            polygonize_linework(&wrong, true, false, 10, 10, 10, 10),
            Err(RustBackendError::UnsupportedGeometry { .. })
        ));
        let network = Geometry::GeometryCollection(GeometryCollection::new_from(vec![
            Geometry::LineString(line_string![(x: 0.0, y: 0.0), (x: 4.0, y: 0.0)]),
            Geometry::LineString(line_string![(x: 4.0, y: 0.0), (x: 4.0, y: 4.0)]),
            Geometry::LineString(line_string![(x: 4.0, y: 4.0), (x: 0.0, y: 4.0)]),
            Geometry::LineString(line_string![(x: 0.0, y: 4.0), (x: 0.0, y: 0.0)]),
            Geometry::LineString(line_string![(x: 0.0, y: 0.0), (x: 4.0, y: 4.0)]),
        ]));
        assert_eq!(
            polygonize_linework(&network, true, true, 100, 10_000, 100, 1_000)
                .unwrap()
                .polygons
                .len(),
            2
        );
        assert!(split_polygon_by_linework(&wrong, &network, 100, 10_000, 100, 1_000).is_err());
        assert!(
            split_polygon_by_linework(&square(0.0, 0.0, 4.0), &wrong, 100, 10_000, 100, 1_000)
                .is_err()
        );
    }

    /// La firma di `geos_backend::PolygonizeResult::residual_count` a
    /// 190c493: infallibile, `usize`, `const`.
    #[test]
    fn residual_count_keeps_the_190c493_signature() {
        const fn conta(result: &PolygonizeResult) -> usize {
            result.residual_count()
        }
        let signature: fn(&PolygonizeResult) -> usize = PolygonizeResult::residual_count;
        let result = PolygonizeResult {
            polygons: Vec::new(),
            cut_edges: vec![LineString::new(Vec::new())],
            dangles: vec![LineString::new(Vec::new()); 2],
            invalid_ring_lines: Vec::new(),
        };
        assert_eq!(signature(&result), 3);
        assert_eq!(conta(&result), 3);
    }

    #[test]
    fn unbounded_limits_are_refused_before_the_data() {
        let square = rect(0.0, 0.0, 2.0, 2.0);
        let lines = Geometry::LineString(line_string![(x: 0.0, y: 0.0), (x: 1.0, y: 0.0)]);
        assert!(matches!(
            polygonize_linework(&lines, true, false, u64::MAX, 10, 10, 10),
            Err(RustBackendError::UnboundedLimits)
        ));
        assert!(matches!(
            split_polygon_by_linework(&square, &lines, 10, 10, 10, u64::MAX),
            Err(RustBackendError::UnboundedLimits)
        ));
        let payload = square.to_wkb(CoordDimensions::xy()).unwrap();
        assert!(matches!(
            make_valid_wkb_with_limits(
                &payload,
                RepairMethod::Linework,
                true,
                MakeValidLimits {
                    max_noding_work: u64::MAX,
                    ..MAKE_VALID_LIMITS
                },
            ),
            Err(RustBackendError::UnboundedLimits)
        ));
    }

    /// Nessun messaggio porta coordinate: i valori delle celle qui sono
    /// numeri riconoscibili, e non devono comparire in nessun errore.
    #[test]
    fn errors_carry_no_cell_values() {
        let marker = 7_654_321.125_f64;
        let source = rect(marker, marker, marker + 10.0, marker + 10.0);
        let splitter = Geometry::LineString(line_string![
            (x: marker + 5.0, y: marker - 1.0), (x: marker + 5.0, y: marker + 11.0)
        ]);
        let errors: Vec<PlenoraError> = vec![
            split_polygon_by_linework(&source, &splitter, 100, 1, 10, 100)
                .unwrap_err()
                .into(),
            split_polygon_by_linework(&source, &splitter, 100, 10_000, 1, 100)
                .unwrap_err()
                .into(),
            polygonize_linework(&splitter, true, true, 100, 10_000, 100, 100)
                .unwrap_err()
                .into(),
            polygonize_linework(&source, true, true, 100, 10_000, 100, 100)
                .unwrap_err()
                .into(),
        ];
        for error in errors {
            let message = error.to_string();
            for forbidden in ["7654321", "7654", "7.654"] {
                assert!(
                    !message.contains(forbidden),
                    "dato nel messaggio: {message}"
                );
            }
        }
        assert_eq!(
            PlenoraError::from(RustBackendError::AreaMismatch).to_string(),
            "contract violation: lo split poligonale non conserva l'area dell'input"
        );
    }

    #[test]
    fn errors_keep_the_step_attribution_of_the_geos_path() {
        assert!(matches!(
            PlenoraError::from(RustBackendError::WorkLimit {
                actual: 2,
                limit: 1
            }),
            PlenoraError::InvalidPlan(_)
        ));
        assert!(matches!(
            PlenoraError::from(RustBackendError::CalcoloNonConcluso("stringa")),
            PlenoraError::Internal(_)
        ));
        assert!(matches!(
            PlenoraError::from(RustBackendError::Internal("invariante")),
            PlenoraError::Internal(_)
        ));
        assert!(matches!(
            PlenoraError::from(RustBackendError::AllocationFailed("segmenti")),
            PlenoraError::ResourceLimit(_)
        ));
        assert!(matches!(
            PlenoraError::from(RustBackendError::NodingDidNotConverge),
            PlenoraError::Unsupported(_)
        ));
        assert!(matches!(
            PlenoraError::from(RustBackendError::InputContract(PlenoraError::Internal(
                "x".to_owned()
            ))),
            PlenoraError::Internal(_)
        ));
    }
}
