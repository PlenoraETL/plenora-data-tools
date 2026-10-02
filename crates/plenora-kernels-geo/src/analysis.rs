//! Kernel binari esatti con limiti di lavoro espliciti: la distanza minima,
//! `geo.nearest`, `geo.within` e `geo.count_points_in_polygons`.
//!
//! Lavorano su colonne di geometrie (`None` per le celle nulle) e rendono
//! provenienza di riga deterministica (posizioni sinistra e destra) e
//! valori scalari. Portare gli attributi e controllare i CRS non e' loro
//! compito: il CRS uguale sui due lati lo verifica l'analisi del contratto.

use std::sync::atomic::{AtomicU64, Ordering};

use crate::margine::{MargineMemoria, TettoRisultati};
use crate::ValidazioneProtetta as _;
use geo::algorithm::line_measures::{Distance, Euclidean};
use geo::{BoundingRect, CoordsIter, Geometry, LineString, Polygon};
use rayon::prelude::*;
use rstar::{PointDistance, RTree, RTreeObject, AABB};
use thiserror::Error;

use crate::spatial_join::{
    spatial_join_nullable, spatial_join_nullable_validated_con_margine, JoinPredicate,
    SpatialJoinError,
};

/// Un abbinamento di [`nearest_matches`]: una riga sinistra e una delle
/// righe destre alla distanza minima.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NearestMatch {
    /// Posizione della riga sinistra.
    pub left: u64,
    /// Posizione della riga destra.
    pub right: u64,
    /// La distanza planare, `Euclidean.distance` di `geo`, nelle unita' del
    /// CRS.
    pub distance: f64,
}

/// Gli errori dei kernel di questo modulo. Nessun messaggio porta valori
/// delle geometrie.
#[derive(Debug, Error)]
pub enum AnalysisError {
    /// Un errore del join spaziale su cui poggiano [`within_indexes`] e
    /// [`count_points_in_polygons`], invariato.
    #[error(transparent)]
    SpatialJoin(#[from] SpatialJoinError),
    /// `max_comparisons` o `max_results` e' zero.
    #[error("limite di lavoro deve essere maggiore di zero")]
    InvalidWorkLimit,
    /// Il numero di confronti (righe sinistre non nulle per righe destre
    /// non nulle e non vuote) supera `max_comparisons` (`limit`) o non
    /// entra in `u64`.
    #[error("numero di confronti oltre il limite di {limit}")]
    WorkLimitExceeded { limit: u64 },
    /// Gli abbinamenti emessi superano `max_results` (`limit`).
    #[error("numero di risultati oltre il limite di {limit}")]
    ResultLimitExceeded { limit: u64 },
    /// Gli abbinamenti emessi (con la loro uscita) supererebbero il
    /// margine di memoria passato al kernel ([`crate::margine`]).
    #[error("vicini: {0}")]
    MargineMemoria(crate::margine::MargineSuperato),
    /// `max_distance` non e' finita o e' negativa.
    #[error("max_distance deve essere finita e non negativa")]
    InvalidMaximumDistance,
    /// Un indice o un conteggio non entra in `u64`.
    #[error("indice non rappresentabile come uint64")]
    IndexOverflow,
    /// Una geometria ha coordinate NaN o infinite o non supera la
    /// validazione OGC; porta il lato (`left`/`right`) e la posizione.
    #[error("geometria {side}[{index}] non valida: {reason}")]
    InvalidGeometry {
        /// `left` o `right`.
        side: &'static str,
        /// La posizione della riga.
        index: usize,
        /// Il motivo, senza valori.
        reason: String,
    },
    /// La validazione OGC non ha concluso: `geo` si e' interrotta.
    ///
    /// **Non** e' una geometria invalida. Nessuno ha dimostrato che l'ingresso
    /// sia sbagliato, e accusarlo manderebbe chi legge a correggere un errore
    /// che non ha commesso. Porta la *forma* del payload, mai il contenuto.
    #[error("validazione OGC non conclusa: {0} (contenuto non pubblicato)")]
    ValidazioneNonConclusa(&'static str),
    /// La distanza di `geo` non ha concluso: e' andata in panico dentro
    /// `calcolo_protetto` (per esempio `nearest_neighbour_distance`
    /// su una linea di un solo punto, o con coordinate che vanno in underflow).
    /// Non accusa l'ingresso; porta la *forma* del payload, mai il contenuto.
    #[error("distanza non conclusa: {0} (contenuto non pubblicato)")]
    CalcoloNonConcluso(&'static str),
}

/// `Euclidean.distance` di `geo` dietro la barriera dei panici: la sola
/// chiamata a `geo` sta nella barriera.
fn distanza_protetta(left: &Geometry<f64>, right: &Geometry<f64>) -> Result<f64, AnalysisError> {
    crate::calcolo_protetto(|| Euclidean.distance(left, right))
        .map_err(AnalysisError::CalcoloNonConcluso)
}

fn validate_geometries(
    geometries: &[Option<Geometry<f64>>],
    side: &'static str,
) -> Result<(), AnalysisError> {
    for (index, geometry) in geometries.iter().enumerate() {
        let Some(geometry) = geometry else {
            continue;
        };
        if geometry
            .coords_iter()
            .any(|coordinate| !coordinate.x.is_finite() || !coordinate.y.is_finite())
        {
            return Err(AnalysisError::InvalidGeometry {
                side,
                index,
                reason: "coordinate NaN o infinite".to_owned(),
            });
        }
        geometry.validazione_protetta().map_err(|esito| {
            esito.separa(
                |ragione| AnalysisError::InvalidGeometry {
                    side,
                    index,
                    reason: ragione.to_string(),
                },
                AnalysisError::ValidazioneNonConclusa,
            )
        })?;
    }
    Ok(())
}

/// La distanza planare minima da ogni riga sinistra alle geometrie destre
/// non nulle e non vuote, una per riga sinistra.
///
/// `None` se la geometria sinistra e' nulla o vuota, o se la destra non ha
/// geometrie utilizzabili.
///
/// # Errors
///
/// - `InvalidWorkLimit`: `max_comparisons` e' zero.
/// - `InvalidGeometry`: una geometria sinistra o destra ha coordinate NaN o
///   infinite o non supera la validazione OGC.
/// - `ValidazioneNonConclusa`: la validazione OGC non ha concluso.
/// - `IndexOverflow`: un numero di righe non entra in `u64`.
/// - `WorkLimitExceeded`: i confronti (righe sinistre non nulle per righe
///   destre utilizzabili) superano `max_comparisons` o non entrano in
///   `u64`.
/// - `CalcoloNonConcluso`: la distanza di `geo` e' andata in panico.
pub fn minimum_distances(
    left: &[Option<Geometry<f64>>],
    right: &[Option<Geometry<f64>>],
    max_comparisons: u64,
) -> Result<Vec<Option<f64>>, AnalysisError> {
    minimum_distances_impl(left, right, max_comparisons, false)
}

/// Variante di [`minimum_distances`] SENZA il gate di ingresso (scansione di
/// finitezza + validazione OGC per geometria).
///
/// # Precondizione (contratto del chiamante)
///
/// Ogni geometria dei due lati deve essere GIA' validata (coordinate finite,
/// validita' OGC), come da [`crate::geometry_from_wkb`] o da un kernel che
/// valida il proprio output. Altrimenti il risultato e' indefinito. Solo per
/// percorsi validati per costruzione; il gate resta in
/// [`minimum_distances`].
///
/// # Errors
///
/// Come [`minimum_distances`], eccetto `InvalidGeometry` (gate omesso).
pub fn minimum_distances_validated(
    left: &[Option<Geometry<f64>>],
    right: &[Option<Geometry<f64>>],
    max_comparisons: u64,
) -> Result<Vec<Option<f64>>, AnalysisError> {
    minimum_distances_impl(left, right, max_comparisons, true)
}

fn minimum_distances_impl(
    left: &[Option<Geometry<f64>>],
    right: &[Option<Geometry<f64>>],
    max_comparisons: u64,
    validated: bool,
) -> Result<Vec<Option<f64>>, AnalysisError> {
    if max_comparisons == 0 {
        return Err(AnalysisError::InvalidWorkLimit);
    }
    if !validated {
        validate_geometries(left, "left")?;
        validate_geometries(right, "right")?;
    }
    let usable_right: Vec<_> = right
        .iter()
        .filter_map(Option::as_ref)
        .filter(|geometry| geometry.coords_count() > 0)
        .collect();
    let comparisons = u64::try_from(left.iter().flatten().count())
        .map_err(|_| AnalysisError::IndexOverflow)?
        .checked_mul(u64::try_from(usable_right.len()).map_err(|_| AnalysisError::IndexOverflow)?)
        .ok_or(AnalysisError::WorkLimitExceeded {
            limit: max_comparisons,
        })?;
    if comparisons > max_comparisons {
        return Err(AnalysisError::WorkLimitExceeded {
            limit: max_comparisons,
        });
    }

    // Primo errore in ordine di riga (collect sequenziale dei `Result`).
    let per_riga: Vec<Result<Option<f64>, AnalysisError>> = left
        .par_iter()
        .map(|geometry| {
            let Some(geometry) = geometry.as_ref() else {
                return Ok(None);
            };
            if geometry.coords_count() == 0 || usable_right.is_empty() {
                return Ok(None);
            }
            // Stesso ordine e stessa riduzione di `reduce(f64::min)`.
            let mut minimo: Option<f64> = None;
            for right in &usable_right {
                let distanza = distanza_protetta(geometry, right)?;
                minimo = Some(minimo.map_or(distanza, |attuale| attuale.min(distanza)));
            }
            Ok(minimo)
        })
        .collect();
    per_riga.into_iter().collect()
}

/// Il vicino piu' prossimo esatto di ogni riga sinistra fra le righe destre:
/// il kernel di `geo.nearest`.
///
/// Per ogni riga sinistra non nulla e non vuota emette le righe destre (non
/// nulle, non vuote) alla distanza minima, tutte in caso di pari, come
/// `sjoin_nearest` di `GeoPandas`. Una riga sinistra nulla o vuota, senza
/// destre utilizzabili o con il minimo oltre `max_distance` (se dato) non
/// emette nulla. Gli abbinamenti sono in ordine `(sinistra, destra)`
/// crescente. La distanza e' `Euclidean.distance` di `geo`, la stessa
/// della forza bruta: un R-tree sceglie i candidati senza cambiare il
/// risultato (docs/limiti.md, «`geo.nearest`: lo scarto dell'R-tree si appoggia
/// alla stima d'errore di `geo`»).
///
/// # Errors
///
/// - `InvalidWorkLimit`: `max_comparisons` o `max_results` e' zero.
/// - `InvalidMaximumDistance`: `max_distance` non e' finita o e' negativa.
/// - `InvalidGeometry`: una geometria sinistra o destra ha coordinate NaN o
///   infinite o non supera la validazione OGC.
/// - `ValidazioneNonConclusa`: la validazione OGC non ha concluso.
/// - `IndexOverflow`: un indice o un conteggio non entra in `u64`.
/// - `WorkLimitExceeded`: i confronti della forza bruta (righe sinistre
///   non nulle per righe destre utilizzabili, `n * m` anche se l'indice ne
///   fa meno) superano `max_comparisons` o non entrano in `u64`.
/// - `ResultLimitExceeded`: gli abbinamenti emessi superano `max_results`
///   (ogni altro errore, il primo in ordine di riga, ha la precedenza).
/// - `CalcoloNonConcluso`: la distanza di `geo` su un candidato e' andata
///   in panico.
pub fn nearest_matches(
    left: &[Option<Geometry<f64>>],
    right: &[Option<Geometry<f64>>],
    max_distance: Option<f64>,
    max_comparisons: u64,
    max_results: u64,
) -> Result<Vec<NearestMatch>, AnalysisError> {
    nearest_matches_impl(
        left,
        right,
        max_distance,
        (max_comparisons, max_results),
        false,
        MargineMemoria::ILLIMITATO,
    )
}

/// Variante di [`nearest_matches`] SENZA il gate di ingresso (scansione di
/// finitezza + validazione OGC per geometria): stessa precondizione e
/// stesso contratto di [`minimum_distances_validated`].
///
/// # Errors
///
/// Come [`nearest_matches`], eccetto `InvalidGeometry` (gate omesso).
pub fn nearest_matches_validated(
    left: &[Option<Geometry<f64>>],
    right: &[Option<Geometry<f64>>],
    max_distance: Option<f64>,
    max_comparisons: u64,
    max_results: u64,
) -> Result<Vec<NearestMatch>, AnalysisError> {
    nearest_matches_impl(
        left,
        right,
        max_distance,
        (max_comparisons, max_results),
        true,
        MargineMemoria::ILLIMITATO,
    )
}

/// Variante di [`nearest_matches_validated`] con il margine di memoria del
/// chiamante ([`crate::margine`]).
///
/// Ogni abbinamento conta
/// [`BYTE_PER_VICINO`] byte nel kernel piu' quelli dell'uscita del
/// margine, e il kernel si ferma con [`AnalysisError::MargineMemoria`]
/// quando gli abbinamenti non ci starebbero, come col limite `max_results`
/// (stesso conteggio, deterministico).
///
/// # Errors
///
/// Come [`nearest_matches_validated`]; in piu'
/// [`AnalysisError::MargineMemoria`] se gli abbinamenti superano il
/// margine prima di `max_results`.
pub fn nearest_matches_validated_con_margine(
    left: &[Option<Geometry<f64>>],
    right: &[Option<Geometry<f64>>],
    max_distance: Option<f64>,
    (max_comparisons, max_results): (u64, u64),
    margine: MargineMemoria,
) -> Result<Vec<NearestMatch>, AnalysisError> {
    nearest_matches_impl(
        left,
        right,
        max_distance,
        (max_comparisons, max_results),
        true,
        margine,
    )
}

/// Un maggiorante dei byte che un abbinamento fa allocare al kernel.
///
/// `NearestMatch` nel gruppo della riga e nel risultato, entrambi a
/// capacita' esatta. I candidati e le loro distanze non si raccolgono.
pub const BYTE_PER_VICINO: u64 = 2 * std::mem::size_of::<NearestMatch>() as u64;

// Una sola sequenza per riga (candidati, predicato, tetto): lunghezza intrinseca.
#[allow(clippy::too_many_lines)]
fn nearest_matches_impl(
    left: &[Option<Geometry<f64>>],
    right: &[Option<Geometry<f64>>],
    max_distance: Option<f64>,
    (max_comparisons, max_results): (u64, u64),
    validated: bool,
    margine: MargineMemoria,
) -> Result<Vec<NearestMatch>, AnalysisError> {
    if max_comparisons == 0 || max_results == 0 {
        return Err(AnalysisError::InvalidWorkLimit);
    }
    // Il tetto dei risultati: `max_results`, o meno se il margine non ne
    // contiene tanti; l'errore dice quale dei due e' scattato.
    let tetto_risultati = TettoRisultati::nuovo(max_results, margine, BYTE_PER_VICINO);
    let tetto = tetto_risultati.tetto();
    let errore_del_tetto = || {
        tetto_risultati.superato_dalla_memoria().map_or(
            AnalysisError::ResultLimitExceeded { limit: max_results },
            AnalysisError::MargineMemoria,
        )
    };
    if max_distance.is_some_and(|value| !value.is_finite() || value < 0.0) {
        return Err(AnalysisError::InvalidMaximumDistance);
    }
    if !validated {
        validate_geometries(left, "left")?;
        validate_geometries(right, "right")?;
    }
    let usable_right: Vec<_> = right
        .iter()
        .enumerate()
        .filter_map(|(index, geometry)| {
            geometry
                .as_ref()
                .filter(|value| value.coords_count() > 0)
                .map(|value| (index, value))
        })
        .collect();
    let comparisons = u64::try_from(left.iter().flatten().count())
        .map_err(|_| AnalysisError::IndexOverflow)?
        .checked_mul(u64::try_from(usable_right.len()).map_err(|_| AnalysisError::IndexOverflow)?)
        .ok_or(AnalysisError::WorkLimitExceeded {
            limit: max_comparisons,
        })?;
    if comparisons > max_comparisons {
        return Err(AnalysisError::WorkLimitExceeded {
            limit: max_comparisons,
        });
    }

    let indice = IndiceVicini::nuovo(&usable_right);
    let result_count = AtomicU64::new(0);
    // Determinismo: i `Result` sono raccolti per riga (ordine preservato) e
    // il primo errore IN ORDINE DI RIGA e' selezionato dal collect
    // sequenziale; il collect parallelo diretto sarebbe non deterministico.
    let groups: Vec<Result<Vec<NearestMatch>, AnalysisError>> = left
        .par_iter()
        .enumerate()
        .map(|(left_index, geometry)| {
            let Some(geometry) = geometry.as_ref().filter(|value| value.coords_count() > 0) else {
                return Ok(Vec::new());
            };
            // Candidati: un sovrainsieme di ogni riga che puo' essere alla
            // distanza minima (vedi `IndiceVicini`), visitati senza
            // raccoglierli. La distanza di ogni candidato e' la stessa
            // chiamata della forza bruta, quindi minimo, pari e valori sono
            // gli stessi bit. Primo giro: il minimo (`f64::min` come
            // `reduce`) e quanti candidati lo eguagliano, senza allocare;
            // il tetto conta i pari prima di raccoglierli; secondo giro (solo
            // con piu' di un pari): i pari, a capacita' esatta.
            let mut minimo: Option<f64> = None;
            let mut pari = 0_usize;
            let mut unico: Option<(usize, f64)> = None;
            indice.visita_candidati(geometry, &usable_right, |posizione| {
                let (right_index, right) = usable_right[posizione];
                let distanza = distanza_protetta(geometry, right)?;
                let nuovo = minimo.map_or(distanza, |attuale| attuale.min(distanza));
                // Uguaglianze esatte: confronti fra gli stessi valori.
                #[allow(clippy::float_cmp)]
                if minimo.is_none_or(|attuale| nuovo < attuale || attuale.is_nan()) {
                    minimo = Some(nuovo);
                    pari = usize::from(distanza == nuovo);
                    unico = (distanza == nuovo).then_some((right_index, distanza));
                } else if distanza == nuovo {
                    pari += 1;
                }
                Ok(())
            })?;
            let Some(minimum) = minimo else {
                return Ok(Vec::new());
            };
            if max_distance.is_some_and(|limit| minimum > limit) {
                return Ok(Vec::new());
            }
            let additional = u64::try_from(pari).map_err(|_| AnalysisError::IndexOverflow)?;
            result_count
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                    current
                        .checked_add(additional)
                        .filter(|next| *next <= tetto)
                })
                .map_err(|_| errore_del_tetto())?;
            let left = u64::try_from(left_index).map_err(|_| AnalysisError::IndexOverflow)?;
            let mut trovati = Vec::new();
            trovati
                .try_reserve_exact(pari)
                .map_err(|_| AnalysisError::CalcoloNonConcluso("allocazione dei vicini"))?;
            let a_match = |right_index: usize, distance: f64| {
                Ok::<_, AnalysisError>(NearestMatch {
                    left,
                    right: u64::try_from(right_index).map_err(|_| AnalysisError::IndexOverflow)?,
                    distance,
                })
            };
            if let (1, Some((right_index, distance))) = (pari, unico) {
                trovati.push(a_match(right_index, distance)?);
            } else {
                indice.visita_candidati(geometry, &usable_right, |posizione| {
                    let (right_index, right) = usable_right[posizione];
                    let distanza = distanza_protetta(geometry, right)?;
                    #[allow(clippy::float_cmp)]
                    if distanza == minimum && trovati.len() < pari {
                        trovati.push(a_match(right_index, distanza)?);
                    }
                    Ok(())
                })?;
                if trovati.len() != pari {
                    return Err(AnalysisError::CalcoloNonConcluso(
                        "i due giri dei vicini non trovano gli stessi pari",
                    ));
                }
                trovati.sort_unstable_by_key(|trovato| trovato.right);
            }
            Ok(trovati)
        })
        .collect();
    // Quale riga supera `max_results` dipende dall'ordine dei thread (il
    // contatore e' condiviso), l'esistenza del superamento no: ogni altro
    // errore vince quindi sul limite dei risultati, il primo in ordine di
    // riga, e l'esito resta deterministico.
    // Capacita' esatta: anche il risultato e' nel conto per abbinamento.
    let totale = groups
        .iter()
        .map(|group| group.as_ref().map_or(0, Vec::len))
        .sum();
    let mut matches = Vec::new();
    matches
        .try_reserve_exact(totale)
        .map_err(|_| AnalysisError::CalcoloNonConcluso("allocazione dei vicini"))?;
    let mut limite_superato = None;
    for group in groups {
        match group {
            Ok(group) => matches.extend(group),
            Err(
                error @ (AnalysisError::ResultLimitExceeded { .. }
                | AnalysisError::MargineMemoria(_)),
            ) => {
                limite_superato.get_or_insert(error);
            }
            Err(error) => return Err(error),
        }
    }
    limite_superato.map_or(Ok(matches), Err)
}

/// Modulo minimo non nullo delle coordinate per cui vale la stima d'errore di
/// [`IndiceVicini`]: `2^-400`. Sotto, prodotti di differenze possono andare
/// in underflow e l'errore di `Euclidean.distance` non e' piu' relativo.
const MODULO_MINIMO: f64 = f64::from_bits((1023 - 400) << 52);
/// Modulo massimo delle coordinate per la stessa stima: `2^400`, lontano
/// dall'overflow dei quadrati.
const MODULO_MASSIMO: f64 = f64::from_bits((1023 + 400) << 52);
/// Margine dello scarto, relativo al modulo massimo delle coordinate: `2^-40`,
/// cioe' 4096 volte l'epsilon di `f64`: la stima d'errore e' sotto
/// `64 * eps * S`, il fattore 64 resta di scorta.
const MARGINE_RELATIVO: f64 = f64::from_bits((1023 - 40) << 52);

/// Rettangolo d'ingombro di una geometria right nell'albero, con la sua
/// posizione in `usable_right`.
struct InvolucroRight {
    envelope: AABB<[f64; 2]>,
    posizione: usize,
}

impl RTreeObject for InvolucroRight {
    type Envelope = AABB<[f64; 2]>;

    fn envelope(&self) -> Self::Envelope {
        self.envelope
    }
}

impl PointDistance for InvolucroRight {
    fn distance_2(&self, point: &[f64; 2]) -> f64 {
        self.envelope.distance_2(point)
    }
}

/// Indice dei candidati di `geo.nearest`: per una geometria left restituisce
/// un sovrainsieme delle righe right che possono stare alla distanza minima
/// della forza bruta. Il risultato non cambia: minimo, pari e valori si
/// calcolano poi con la stessa `Euclidean.distance` sui soli candidati.
///
/// # Perche' lo scarto e' esatto
///
/// Sia `d` la distanza calcolata di un candidato qualunque (il primo vicino
/// del centro del rettangolo left), che entra sempre fra i candidati, e `S` il
/// modulo massimo delle coordinate del left e dei right regolari. Si tengono
/// i right il cui rettangolo interseca il rettangolo left allargato di
/// `e = d + 2 * S * 2^-40`. Un right scartato ha un asse su cui la distanza
/// esatta fra i rettangoli supera `d + S * 2^-40` (l'arrotondamento del bordo
/// della finestra e' sotto `ulp(S)`), quindi anche la distanza vera fra le
/// geometrie. Su geometrie regolari (vedi [`involucro_regolare`])
/// `Euclidean.distance` di `geo` 0.33.1 restituisce o zero da un predicato
/// d'intersezione esatto (i rettangoli allora si toccano e l'elemento non e'
/// scartato), o il minimo di distanze punto-segmento e punto-punto calcolate,
/// ciascuna sotto il valore vero di meno di `64 * eps * S` (differenze di
/// coordinate, `hypot`, scelta del ramo di `line_segment_distance`,
/// tolleranza parametrica di `line_string_contains_point`). La distanza
/// calcolata di uno scartato supera dunque `d`, che non e' sotto il minimo
/// dei candidati: nessuno scartato e' al minimo, ne' lo cambia.
///
/// Le geometrie fuori da quella stima non si scartano mai: i right irregolari
/// sono candidati di ogni riga, un left irregolare prende tutti i right.
/// Irregolare vuol dire una coordinata fuori da `{0} ∪ [2^-400, 2^400]` in
/// modulo (anche NaN o infinita), o una parte vuota o degenere, su cui `geo`
/// risponde zero o con tolleranze assolute (`point_contains_point` in `f32`).
struct IndiceVicini {
    albero: RTree<InvolucroRight>,
    sempre: Vec<usize>,
    modulo_right: f64,
    totale: usize,
}

impl IndiceVicini {
    fn nuovo(usable_right: &[(usize, &Geometry<f64>)]) -> Self {
        let mut involucri = Vec::new();
        let mut sempre = Vec::new();
        let mut modulo_right = 0.0_f64;
        for (posizione, (_, geometria)) in usable_right.iter().enumerate() {
            match involucro_regolare(geometria) {
                Some((envelope, modulo)) => {
                    modulo_right = modulo_right.max(modulo);
                    involucri.push(InvolucroRight {
                        envelope,
                        posizione,
                    });
                }
                None => sempre.push(posizione),
            }
        }
        Self {
            albero: RTree::bulk_load(involucri),
            sempre,
            modulo_right,
            totale: usable_right.len(),
        }
    }

    /// Visita le posizioni di [`Self::candidati`] (lo stesso insieme, ogni
    /// posizione una volta) senza raccoglierle, nell'ordine dell'R-tree
    /// e poi di `sempre`: il primo vicino del centro si visita a parte solo
    /// se la finestra non lo contiene. Gli alberi e `sempre` sono disgiunti
    /// (geometrie regolari nell'albero, le altre in `sempre`).
    ///
    /// # Errors
    ///
    /// `CalcoloNonConcluso` se la distanza del primo candidato va in panico;
    /// il primo errore di `visita`.
    fn visita_candidati(
        &self,
        geometria: &Geometry<f64>,
        usable_right: &[(usize, &Geometry<f64>)],
        mut visita: impl FnMut(usize) -> Result<(), AnalysisError>,
    ) -> Result<(), AnalysisError> {
        let tutti = |visita: &mut dyn FnMut(usize) -> Result<(), AnalysisError>| {
            (0..self.totale).try_for_each(visita)
        };
        let Some((involucro, modulo_left)) = involucro_regolare(geometria) else {
            return tutti(&mut visita);
        };
        let (minimo, massimo) = (involucro.lower(), involucro.upper());
        let centro = [
            f64::midpoint(minimo[0], massimo[0]),
            f64::midpoint(minimo[1], massimo[1]),
        ];
        let Some(primo) = self.albero.nearest_neighbor(&centro) else {
            return tutti(&mut visita);
        };
        let Some((_, destra)) = usable_right.get(primo.posizione) else {
            return tutti(&mut visita);
        };
        let limite = distanza_protetta(geometria, destra)?;
        let margine = modulo_left.max(self.modulo_right) * MARGINE_RELATIVO;
        let espansione = limite + (margine + margine);
        if !espansione.is_finite() {
            return tutti(&mut visita);
        }
        let finestra = AABB::from_corners(
            [minimo[0] - espansione, minimo[1] - espansione],
            [massimo[0] + espansione, massimo[1] + espansione],
        );
        let mut primo_visto = false;
        for candidato in self.albero.locate_in_envelope_intersecting(&finestra) {
            primo_visto |= candidato.posizione == primo.posizione;
            visita(candidato.posizione)?;
        }
        for posizione in &self.sempre {
            visita(*posizione)?;
        }
        if !primo_visto {
            visita(primo.posizione)?;
        }
        Ok(())
    }

    /// Posizioni in `usable_right` da valutare, crescenti e senza ripetizioni.
    ///
    /// # Errors
    ///
    /// `CalcoloNonConcluso` se la distanza del primo candidato va in panico.
    #[cfg(test)]
    fn candidati(
        &self,
        geometria: &Geometry<f64>,
        usable_right: &[(usize, &Geometry<f64>)],
    ) -> Result<Vec<usize>, AnalysisError> {
        let tutti = || Ok((0..self.totale).collect());
        let Some((involucro, modulo_left)) = involucro_regolare(geometria) else {
            return tutti();
        };
        let (minimo, massimo) = (involucro.lower(), involucro.upper());
        // Qualunque punto va bene: il centro serve solo a scegliere `d`.
        let centro = [
            f64::midpoint(minimo[0], massimo[0]),
            f64::midpoint(minimo[1], massimo[1]),
        ];
        let Some(primo) = self.albero.nearest_neighbor(&centro) else {
            return tutti();
        };
        let Some((_, destra)) = usable_right.get(primo.posizione) else {
            return tutti();
        };
        let limite = distanza_protetta(geometria, destra)?;
        let margine = modulo_left.max(self.modulo_right) * MARGINE_RELATIVO;
        let espansione = limite + (margine + margine);
        // NaN o infinito: nessuna stima, nessuno scarto.
        if !espansione.is_finite() {
            return tutti();
        }
        let finestra = AABB::from_corners(
            [minimo[0] - espansione, minimo[1] - espansione],
            [massimo[0] + espansione, massimo[1] + espansione],
        );
        let mut candidati: Vec<usize> = self
            .albero
            .locate_in_envelope_intersecting(&finestra)
            .map(|candidato| candidato.posizione)
            .chain(self.sempre.iter().copied())
            .chain(std::iter::once(primo.posizione))
            .collect();
        candidati.sort_unstable();
        candidati.dedup();
        Ok(candidati)
    }
}

/// Rettangolo d'ingombro e modulo massimo delle coordinate di una geometria
/// su cui vale la stima d'errore di [`IndiceVicini`]; `None` se irregolare.
fn involucro_regolare(geometria: &Geometry<f64>) -> Option<(AABB<[f64; 2]>, f64)> {
    if !struttura_regolare(geometria) {
        return None;
    }
    let mut modulo = 0.0_f64;
    for coordinata in geometria.coords_iter() {
        for valore in [coordinata.x, coordinata.y] {
            let assoluto = valore.abs();
            // NaN non passa: nessuno dei due confronti e' vero.
            let nel_dominio = valore == 0.0 || (MODULO_MINIMO..=MODULO_MASSIMO).contains(&assoluto);
            if !nel_dominio {
                return None;
            }
            modulo = modulo.max(assoluto);
        }
    }
    let rettangolo = geometria.bounding_rect()?;
    Some((
        AABB::from_corners(
            [rettangolo.min().x, rettangolo.min().y],
            [rettangolo.max().x, rettangolo.max().y],
        ),
        modulo,
    ))
}

/// Nessuna parte vuota o degenere: linee con almeno due vertici, anelli
/// chiusi con almeno quattro, collezioni non vuote.
fn struttura_regolare(geometria: &Geometry<f64>) -> bool {
    let linea = |linea: &LineString<f64>| linea.0.len() >= 2;
    let anello = |anello: &LineString<f64>| anello.0.len() >= 4 && anello.is_closed();
    let poligono = |poligono: &Polygon<f64>| {
        anello(poligono.exterior()) && poligono.interiors().iter().all(anello)
    };
    match geometria {
        Geometry::Point(_) | Geometry::Line(_) | Geometry::Rect(_) | Geometry::Triangle(_) => true,
        Geometry::LineString(valore) => linea(valore),
        Geometry::Polygon(valore) => poligono(valore),
        Geometry::MultiPoint(valore) => !valore.0.is_empty(),
        Geometry::MultiLineString(valore) => !valore.0.is_empty() && valore.0.iter().all(linea),
        Geometry::MultiPolygon(valore) => !valore.0.is_empty() && valore.0.iter().all(poligono),
        Geometry::GeometryCollection(valore) => {
            !valore.0.is_empty() && valore.0.iter().all(struttura_regolare)
        }
    }
}

/// Le posizioni delle righe sinistre contenute in almeno una riga destra,
/// crescenti e senza ripetizioni: il kernel di `geo.within`.
///
/// «Contenuta» e' `right.contains(left)` di `geo` (DE-9IM): una geometria
/// sul solo bordo della destra, come un punto sul lato, non lo e'. Le righe
/// nulle o vuote, da entrambi i lati, non sono mai contenute.
///
/// # Errors
///
/// - `SpatialJoin`: ogni errore di [`spatial_join_nullable`]
///   (`max_pairs` zero, o superato dalle coppie confermate, che contano
///   ogni riga destra che contiene una sinistra; geometrie non valide o con
///   coordinate non finite; validazione o predicato non conclusi; indici
///   oltre `u64`).
pub fn within_indexes(
    left: &[Option<Geometry<f64>>],
    right: &[Option<Geometry<f64>>],
    max_pairs: u64,
) -> Result<Vec<u64>, AnalysisError> {
    let pairs = spatial_join_nullable(left, right, JoinPredicate::Within, max_pairs)?;
    let mut indexes: Vec<_> = pairs.into_iter().map(|pair| pair.left).collect();
    indexes.dedup();
    Ok(indexes)
}

/// Variante di [`within_indexes`] SENZA il gate di ingresso (delega a
/// [`spatial_join_nullable_validated`]): stessa precondizione e stesso
/// contratto di [`crate::spatial_join::spatial_join_validated`].
///
/// # Errors
///
/// Come [`within_indexes`], eccetto `InvalidGeometry` (gate omesso).
pub fn within_indexes_validated(
    left: &[Option<Geometry<f64>>],
    right: &[Option<Geometry<f64>>],
    max_pairs: u64,
) -> Result<Vec<u64>, AnalysisError> {
    within_indexes_validated_con_margine(left, right, max_pairs, MargineMemoria::ILLIMITATO)
}

/// Variante di [`within_indexes_validated`] con il margine di memoria del
/// chiamante: le coppie del join contano come in
/// [`crate::spatial_join::spatial_join_nullable_validated_con_margine`].
///
/// # Errors
///
/// Come [`within_indexes_validated`]; in piu' `SpatialJoin(MargineMemoria)`
/// se le coppie confermate superano il margine.
pub fn within_indexes_validated_con_margine(
    left: &[Option<Geometry<f64>>],
    right: &[Option<Geometry<f64>>],
    max_pairs: u64,
    margine: MargineMemoria,
) -> Result<Vec<u64>, AnalysisError> {
    let pairs = spatial_join_nullable_validated_con_margine(
        left,
        right,
        JoinPredicate::Within,
        max_pairs,
        margine,
    )?;
    let mut indexes: Vec<_> = pairs.into_iter().map(|pair| pair.left).collect();
    indexes.dedup();
    Ok(indexes)
}

/// Per ogni riga di `polygons`, quante righe di `points` contiene: il
/// kernel di `geo.count_points_in_polygons`.
///
/// Un conteggio per riga di `polygons`, nella stessa posizione (0 per una
/// riga nulla o vuota). Conta ogni riga di `points` per cui
/// `polygon.contains(point)` di `geo` e' vero, qualunque sia il suo tipo: i
/// punti sul bordo non contano, come il `predicate="within"` di Manipola;
/// un punto dentro piu' poligoni conta in ognuno.
///
/// # Errors
///
/// - `SpatialJoin`: ogni errore di [`spatial_join_nullable`], con
///   `points` a sinistra e `polygons` a destra (`max_pairs` zero, o
///   superato dalle coppie punto-poligono confermate; geometrie non valide
///   o con coordinate non finite; validazione o predicato non conclusi).
/// - `IndexOverflow`: difesa interna sui conteggi (non raggiungibile con
///   ingressi gia' accettati dal join).
pub fn count_points_in_polygons(
    polygons: &[Option<Geometry<f64>>],
    points: &[Option<Geometry<f64>>],
    max_pairs: u64,
) -> Result<Vec<u64>, AnalysisError> {
    let pairs = spatial_join_nullable(points, polygons, JoinPredicate::Within, max_pairs)?;
    let mut counts = vec![0_u64; polygons.len()];
    for pair in pairs {
        let index = usize::try_from(pair.right).map_err(|_| AnalysisError::IndexOverflow)?;
        counts[index] = counts[index]
            .checked_add(1)
            .ok_or(AnalysisError::IndexOverflow)?;
    }
    Ok(counts)
}

/// Variante di [`count_points_in_polygons`] SENZA il gate OGC di
/// ingresso.
///
/// Delega a [`spatial_join_nullable_validated`]: stessa precondizione e
/// stesso contratto di [`crate::spatial_join::spatial_join_validated`].
///
/// # Errors
///
/// Come [`count_points_in_polygons`], eccetto `InvalidGeometry` (gate
/// omesso).
pub fn count_points_in_polygons_validated(
    polygons: &[Option<Geometry<f64>>],
    points: &[Option<Geometry<f64>>],
    max_pairs: u64,
) -> Result<Vec<u64>, AnalysisError> {
    count_points_in_polygons_validated_con_margine(
        polygons,
        points,
        max_pairs,
        MargineMemoria::ILLIMITATO,
    )
}

/// Variante di [`count_points_in_polygons_validated`] con il margine di
/// memoria del chiamante.
///
/// Le coppie punto-poligono del join contano come
/// in [`crate::spatial_join::spatial_join_nullable_validated_con_margine`].
///
/// # Errors
///
/// Come [`count_points_in_polygons_validated`]; in piu'
/// `SpatialJoin(MargineMemoria)` se le coppie confermate superano il
/// margine.
pub fn count_points_in_polygons_validated_con_margine(
    polygons: &[Option<Geometry<f64>>],
    points: &[Option<Geometry<f64>>],
    max_pairs: u64,
    margine: MargineMemoria,
) -> Result<Vec<u64>, AnalysisError> {
    let pairs = spatial_join_nullable_validated_con_margine(
        points,
        polygons,
        JoinPredicate::Within,
        max_pairs,
        margine,
    )?;
    let mut counts = vec![0_u64; polygons.len()];
    for pair in pairs {
        let index = usize::try_from(pair.right).map_err(|_| AnalysisError::IndexOverflow)?;
        counts[index] = counts[index]
            .checked_add(1)
            .ok_or(AnalysisError::IndexOverflow)?;
    }
    Ok(counts)
}

#[cfg(test)]
mod nearest_oracolo;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::some_point as point;
    use crate::test_support::{bowtie, rect};
    use geo::Point;

    // L'Option serve a comporre colonne con null, come in `some_point`.
    #[allow(clippy::unnecessary_wraps)]
    fn square() -> Option<Geometry<f64>> {
        Some(rect(0.0, 0.0, 2.0, 2.0))
    }

    #[test]
    fn distances_preserve_null_rows_and_reject_unbounded_work() {
        let left = vec![point(0.0, 0.0), None, point(3.0, 4.0)];
        let right = vec![point(0.0, 4.0)];
        assert_eq!(
            minimum_distances(&left, &right, 10).unwrap(),
            vec![Some(4.0), None, Some(3.0)]
        );
        assert!(matches!(
            minimum_distances(&left, &right, 1),
            Err(AnalysisError::WorkLimitExceeded { limit: 1 })
        ));
    }

    #[test]
    fn invalid_topology_is_rejected_on_both_sides_before_distance_work() {
        let invalid = Some(bowtie());
        assert!(matches!(
            minimum_distances(std::slice::from_ref(&invalid), &[point(0.0, 0.0)], 1),
            Err(AnalysisError::InvalidGeometry { side: "left", .. })
        ));
        assert!(matches!(
            minimum_distances(&[point(0.0, 0.0)], std::slice::from_ref(&invalid), 1),
            Err(AnalysisError::InvalidGeometry { side: "right", .. })
        ));
        assert!(matches!(
            nearest_matches(
                &[point(0.0, 0.0)],
                std::slice::from_ref(&invalid),
                None,
                1,
                1,
            ),
            Err(AnalysisError::InvalidGeometry { side: "right", .. })
        ));
    }

    #[test]
    fn nearest_emits_stable_ties_and_honours_max_distance() {
        let left = vec![point(0.0, 0.0)];
        let right = vec![point(-1.0, 0.0), None, point(1.0, 0.0)];
        assert_eq!(
            nearest_matches(&left, &right, None, 10, 10).unwrap(),
            vec![
                NearestMatch {
                    left: 0,
                    right: 0,
                    distance: 1.0
                },
                NearestMatch {
                    left: 0,
                    right: 2,
                    distance: 1.0
                },
            ]
        );
        assert!(nearest_matches(&left, &right, Some(0.5), 10, 10)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn within_and_point_counts_use_strict_boundary_semantics() {
        let polygons = vec![square(), None];
        let points = vec![point(1.0, 1.0), point(0.0, 1.0), None];
        assert_eq!(within_indexes(&points, &polygons, 10).unwrap(), vec![0]);
        assert_eq!(
            count_points_in_polygons(&polygons, &points, 10).unwrap(),
            vec![1, 0]
        );
    }

    #[test]
    fn validated_variants_match_the_gated_path_on_valid_inputs() {
        let left = vec![point(0.0, 0.0), None, point(3.0, 4.0), square()];
        let right = vec![point(0.0, 4.0), square(), None];
        assert_eq!(
            minimum_distances(&left, &right, 100).unwrap(),
            minimum_distances_validated(&left, &right, 100).unwrap()
        );
        assert_eq!(
            nearest_matches(&left, &right, None, 100, 100).unwrap(),
            nearest_matches_validated(&left, &right, None, 100, 100).unwrap()
        );
        assert_eq!(
            nearest_matches(&left, &right, Some(5.0), 100, 100).unwrap(),
            nearest_matches_validated(&left, &right, Some(5.0), 100, 100).unwrap()
        );
        assert_eq!(
            within_indexes(&left, &right, 100).unwrap(),
            within_indexes_validated(&left, &right, 100).unwrap()
        );
        assert_eq!(
            count_points_in_polygons(&right, &left, 100).unwrap(),
            count_points_in_polygons_validated(&right, &left, 100).unwrap()
        );
        // I limiti di lavoro restano fail-closed nella variante validated.
        assert!(matches!(
            minimum_distances_validated(&left, &right, 1),
            Err(AnalysisError::WorkLimitExceeded { limit: 1 })
        ));
    }

    #[test]
    fn validated_variants_document_the_caller_precondition() {
        // Test di documentazione del contratto, NON un nuovo modo di
        // accettare geometrie invalide in produzione: il percorso gated
        // rifiuta il bowtie (gate intatto), la variante validated lo prende
        // perche' la precondizione e' del chiamante — qui violata ad arte.
        let bowtie = Some(bowtie());
        assert!(matches!(
            minimum_distances(std::slice::from_ref(&bowtie), &[point(0.0, 0.0)], 10),
            Err(AnalysisError::InvalidGeometry { side: "left", .. })
        ));
        assert!(
            minimum_distances_validated(std::slice::from_ref(&bowtie), &[point(0.0, 0.0)], 10)
                .is_ok()
        );
    }

    #[test]
    fn adversarial_limits_empty_inputs_and_invalid_coordinates() {
        assert!(matches!(
            minimum_distances(&[], &[], 0),
            Err(AnalysisError::InvalidWorkLimit)
        ));
        assert_eq!(minimum_distances(&[None], &[], 1).unwrap(), vec![None]);
        assert!(minimum_distances(
            &[Some(Geometry::Point(Point::new(f64::NAN, 0.0)))],
            &[point(0.0, 0.0)],
            10,
        )
        .is_err());
        assert!(nearest_matches(
            &[point(0.0, 0.0)],
            &[point(0.0, 0.0)],
            Some(f64::NAN),
            10,
            10,
        )
        .is_err());
        assert!(nearest_matches(&[point(0.0, 0.0)], &[point(0.0, 0.0)], None, 0, 10).is_err());
        assert!(matches!(
            nearest_matches(
                &[point(0.0, 0.0)],
                &[point(-1.0, 0.0), point(1.0, 0.0)],
                None,
                10,
                1,
            ),
            Err(AnalysisError::ResultLimitExceeded { .. })
        ));
        assert!(nearest_matches(&[None], &[], None, 1, 1)
            .unwrap()
            .is_empty());
    }
}
