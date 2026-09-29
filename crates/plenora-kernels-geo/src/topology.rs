//! Polygonal boolean kernels. Non-polygon inputs are rejected explicitly:
//! they need a backend with full GEOS-compatible dimensional semantics.
//!
//! Ogni funzione pubblica riceve la precisione dichiarata (1 cm a terra
//! nelle unita' delle coordinate, [`Precision`]) come argomento esplicito,
//! senza valore predefinito: prima di ogni overlay di `i_overlay` il passo
//! della griglia e la spaziatura delle coordinate degli operandi di quella
//! chiamata sono confrontati con la precisione, e dopo l'overlay ogni lato
//! del risultato deve stare entro la precisione dai bordi degli ingressi
//! **originali** dell'operazione (`rust_backend::griglia`). Oltre,
//! [`TopologyError::PrecisionInsufficient`].

use geo::algorithm::bool_ops::unary_union;
use geo::{BooleanOps, Buffer, CoordsIter, Geometry, MultiPolygon};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::rust_backend::griglia::{self, IndiceLinework, PrecisioneInsufficiente};
use crate::rust_backend::precision::Precision;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BooleanOperation {
    Intersection,
    Union,
    Difference,
    SymmetricDifference,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OverlayMode {
    Intersection,
    Union,
    Identity,
    SymmetricDifference,
}

#[derive(Clone, Debug, PartialEq)]
pub struct OverlayPiece {
    pub geometry: Geometry<f64>,
    pub left: Option<u64>,
    pub right: Option<u64>,
}

#[derive(Debug, Error)]
pub enum TopologyError {
    #[error("operazione topologica supportata solo per Polygon/MultiPolygon, ricevuto {0}")]
    UnsupportedGeometry(&'static str),
    #[error("geometria topologica non valida: {0}")]
    InvalidGeometry(String),
    #[error("parametro {name} non valido: {reason}")]
    InvalidParameter {
        name: &'static str,
        reason: &'static str,
    },
    #[error("limite {name} superato: valore={actual}, limite={limit}")]
    ResourceLimit {
        name: &'static str,
        actual: u64,
        limit: u64,
    },
    #[error("indice non rappresentabile come uint64")]
    IndexOverflow,
    /// La validazione OGC non ha concluso: `geo` si e' interrotta.
    ///
    /// **Non** e' una geometria invalida. Nessuno ha dimostrato che l'ingresso
    /// sia sbagliato, e accusarlo manderebbe chi legge a correggere un errore
    /// che non ha commesso. Porta la *forma* del payload, mai il contenuto.
    #[error("validazione OGC non conclusa: {0} (contenuto non pubblicato)")]
    ValidazioneNonConclusa(&'static str),
    /// Il join spaziale che accoppia i candidati non ha concluso: un
    /// predicato esatto interrotto o un'invariante interna. Non accusa
    /// l'ingresso, e porta la *forma* del payload, mai il contenuto.
    #[error("join spaziale non concluso: {0} (contenuto non pubblicato)")]
    CalcoloNonConcluso(&'static str),
    /// Lo spostamento che la griglia di `i_overlay` introdurrebbe, o ha
    /// introdotto, supera la precisione dichiarata: coordinate troppo rade,
    /// passo della griglia troppo grosso per l'estensione degli operandi, o
    /// un lato del risultato oltre la precisione dai bordi degli ingressi.
    /// Nessun dato nel messaggio.
    #[error("geometria troppo estesa per la precisione dichiarata")]
    PrecisionInsufficient,
}

impl From<PrecisioneInsufficiente> for TopologyError {
    fn from(_: PrecisioneInsufficiente) -> Self {
        Self::PrecisionInsufficient
    }
}

/// Il fallimento del join spaziale nella lingua di questo modulo, senza
/// perdere la differenza fra un ingresso sbagliato e un calcolo che non ha
/// concluso.
fn dal_join(error: crate::spatial_join::SpatialJoinError) -> TopologyError {
    use crate::spatial_join::SpatialJoinError as S;
    match error {
        S::ValidazioneNonConclusa(forma) => TopologyError::ValidazioneNonConclusa(forma),
        S::CalcoloNonConcluso(forma) | S::Internal(forma) => {
            TopologyError::CalcoloNonConcluso(forma)
        }
        altro => TopologyError::InvalidGeometry(altro.to_string()),
    }
}

use crate::geometry_type_name as geometry_name;
use crate::ValidazioneProtetta as _;

/// Mappa l'esito della barriera sull'errore proprio di questo modulo.
/// Estratta a parte perche' e' la conversione che una prova sintetica deve
/// esercitare per intero: vedi la motivazione gemella in
/// `predicates::classifica_lato`.
fn classifica_geometria(esito: crate::EsitoValidazione) -> TopologyError {
    esito.separa(
        |ragione| TopologyError::InvalidGeometry(ragione.to_string()),
        TopologyError::ValidazioneNonConclusa,
    )
}

fn as_multi_polygon(geometry: &Geometry<f64>) -> Result<MultiPolygon<f64>, TopologyError> {
    geometry
        .validazione_protetta()
        .map_err(classifica_geometria)?;
    as_multi_polygon_validated(geometry)
}

/// Coercizione a `MultiPolygon` di una geometria GIA' VALIDATA (OGC):
/// nessun controllo di ingresso, la precondizione e' del chiamante (vedi
/// [`boolean_operation_validated`]). Il gate di tipo resta: non e' una
/// validazione, e' il contratto dei kernel poligonali.
fn as_multi_polygon_validated(
    geometry: &Geometry<f64>,
) -> Result<MultiPolygon<f64>, TopologyError> {
    match geometry {
        Geometry::Polygon(polygon) => Ok(MultiPolygon::new(vec![polygon.clone()])),
        Geometry::MultiPolygon(polygons) => Ok(polygons.clone()),
        value => Err(TopologyError::UnsupportedGeometry(geometry_name(value))),
    }
}

fn checked_result(result: MultiPolygon<f64>) -> Result<Geometry<f64>, TopologyError> {
    result
        .validazione_protetta()
        .map_err(classifica_geometria)?;
    Ok(Geometry::MultiPolygon(result))
}

/// Applies a polygonal boolean operation to two inputs.
///
/// `precision` e' la precisione dichiarata nelle unita' delle coordinate
/// (`Precision::from_crs` con un CRS, altrimenti esplicita).
///
/// # Errors
///
/// - `UnsupportedGeometry`: an input is not Polygon/MultiPolygon.
/// - `InvalidGeometry`: an input fails OGC validation, or the result is not
///   valid.
/// - `PrecisionInsufficient`: la griglia dell'overlay sposterebbe, o ha
///   spostato, il risultato oltre la precisione.
pub fn boolean_operation(
    left: &Geometry<f64>,
    right: &Geometry<f64>,
    operation: BooleanOperation,
    precision: Precision,
) -> Result<Geometry<f64>, TopologyError> {
    boolean_operation_impl(left, right, operation, precision, false)
}

/// Variante di [`boolean_operation`] SENZA il gate OGC di ingresso.
///
/// Restano la validazione OGC dell'output ([`checked_result`]), garanzia del
/// produttore per i consumatori a valle (R0.1), il rifiuto dei tipi non
/// poligonali, che e' il contratto del kernel, e i controlli di precisione.
///
/// # Precondizione (contratto del chiamante)
///
/// Entrambi gli input GIA' validati OGC e a coordinate finite, per
/// costruzione: da [`crate::geometry_from_wkb`] o da un kernel che valida il
/// proprio output, mai per inferenza sui chiamanti (R0.1). Altrimenti il
/// risultato e' indefinito e nessun errore dedicato e' garantito.
///
/// # Errors
///
/// Come [`boolean_operation`], ma `InvalidGeometry` resta raggiungibile
/// solo dall'output (gate di ingresso omesso).
pub fn boolean_operation_validated(
    left: &Geometry<f64>,
    right: &Geometry<f64>,
    operation: BooleanOperation,
    precision: Precision,
) -> Result<Geometry<f64>, TopologyError> {
    boolean_operation_impl(left, right, operation, precision, true)
}

fn boolean_operation_impl(
    left_geometry: &Geometry<f64>,
    right_geometry: &Geometry<f64>,
    operation: BooleanOperation,
    precision: Precision,
    validated: bool,
) -> Result<Geometry<f64>, TopologyError> {
    let result = boolean_raw(
        left_geometry,
        right_geometry,
        operation,
        precision,
        validated,
    )?;
    let indice = IndiceLinework::nuovo([(0, left_geometry), (1, right_geometry)]);
    checked_displacement(result, &indice, |_| true, precision)
}

/// Coercizione con o senza il gate OGC di ingresso.
fn coerce(geometry: &Geometry<f64>, validated: bool) -> Result<MultiPolygon<f64>, TopologyError> {
    if validated {
        as_multi_polygon_validated(geometry)
    } else {
        as_multi_polygon(geometry)
    }
}

/// La booleana di due ingressi, con il controllo a priori della griglia e
/// senza alcun controllo del risultato: lo fa il chiamante, contro gli
/// ingressi originali della sua operazione.
fn boolean_raw(
    left: &Geometry<f64>,
    right: &Geometry<f64>,
    operation: BooleanOperation,
    precision: Precision,
    validated: bool,
) -> Result<MultiPolygon<f64>, TopologyError> {
    let left = coerce(left, validated)?;
    let right = coerce(right, validated)?;
    griglia::controlla_overlay(
        griglia::rettangolo_multipoligoni([&left, &right]),
        precision,
    )?;
    Ok(match operation {
        BooleanOperation::Intersection => left.intersection(&right),
        BooleanOperation::Union => left.union(&right),
        BooleanOperation::Difference => left.difference(&right),
        BooleanOperation::SymmetricDifference => left.xor(&right),
    })
}

/// Il controllo a posteriori (ogni lato del risultato entro la precisione
/// dai bordi degli ingressi con etichetta accettata da `filtro`), poi la
/// validazione OGC dell'output.
fn checked_displacement(
    result: MultiPolygon<f64>,
    indice: &IndiceLinework,
    filtro: impl Fn(usize) -> bool,
    precision: Precision,
) -> Result<Geometry<f64>, TopologyError> {
    if !indice.bordo_entro(&result, &[], filtro, precision) {
        return Err(TopologyError::PrecisionInsufficient);
    }
    checked_result(result)
}

/// Efficient polygonal dissolve/unary union. Grouping and attribute
/// aggregation remain a transport/adapter concern.
///
/// # Errors
///
/// - `UnsupportedGeometry`: an input is not Polygon/MultiPolygon.
/// - `InvalidGeometry`: an input fails OGC validation, or the dissolved
///   result is not valid.
/// - `PrecisionInsufficient`: vedi [`boolean_operation`].
pub fn dissolve(
    geometries: &[Geometry<f64>],
    precision: Precision,
) -> Result<Geometry<f64>, TopologyError> {
    dissolve_impl(geometries, false, precision)
}

/// Variante di [`dissolve`] SENZA il gate OGC di ingresso: stessa
/// precondizione e stesso contratto di [`boolean_operation_validated`].
/// La validazione OGC dell'output resta ([`checked_result`]).
///
/// # Errors
///
/// Come [`dissolve`], ma `InvalidGeometry` resta raggiungibile solo
/// dall'output (gate di ingresso omesso).
pub fn dissolve_validated(
    geometries: &[Geometry<f64>],
    precision: Precision,
) -> Result<Geometry<f64>, TopologyError> {
    dissolve_impl(geometries, true, precision)
}

fn dissolve_impl(
    geometries: &[Geometry<f64>],
    validated: bool,
    precision: Precision,
) -> Result<Geometry<f64>, TopologyError> {
    let result = dissolve_raw(geometries, validated, precision)?;
    let indice = IndiceLinework::nuovo(geometries.iter().map(|geometry| (0, geometry)));
    checked_displacement(result, &indice, |_| true, precision)
}

/// L'unione di tutti gli ingressi, con il controllo a priori della griglia
/// e senza controllo del risultato (vedi [`boolean_raw`]).
fn dissolve_raw(
    geometries: &[Geometry<f64>],
    validated: bool,
    precision: Precision,
) -> Result<MultiPolygon<f64>, TopologyError> {
    let polygons: Vec<MultiPolygon<f64>> = geometries
        .iter()
        .map(|geometry| coerce(geometry, validated))
        .collect::<Result<_, _>>()?;
    griglia::controlla_overlay(griglia::rettangolo_multipoligoni(&polygons), precision)?;
    Ok(unary_union(&polygons))
}

fn is_empty(geometry: &Geometry<f64>) -> bool {
    geometry.coords_count() == 0
}

/// Clips each polygonal input row to the dissolved polygonal mask. Empty
/// results become `None`, preserving the input row position for the adapter.
///
/// # Errors
///
/// Propagates the errors of [`dissolve`] (mask) and [`boolean_operation`]
/// (per-row intersection): non-polygonal or invalid inputs, invalid results,
/// `PrecisionInsufficient`.
pub fn clip_to_mask(
    geometries: &[Geometry<f64>],
    masks: &[Geometry<f64>],
    precision: Precision,
) -> Result<Vec<Option<Geometry<f64>>>, TopologyError> {
    clip_to_mask_impl(geometries, masks, false, precision)
}

/// Variante di [`clip_to_mask`] SENZA il gate OGC di ingresso.
///
/// Stessa precondizione e stesso contratto di
/// [`boolean_operation_validated`] (righe e maschere gia' validate).
/// Le validazioni OGC degli output (maschera dissolta e ogni pezzo)
/// restano.
///
/// # Errors
///
/// Come [`clip_to_mask`], ma `InvalidGeometry` resta raggiungibile solo
/// dagli output (gate di ingresso omesso).
pub fn clip_to_mask_validated(
    geometries: &[Geometry<f64>],
    masks: &[Geometry<f64>],
    precision: Precision,
) -> Result<Vec<Option<Geometry<f64>>>, TopologyError> {
    clip_to_mask_impl(geometries, masks, true, precision)
}

/// Il pezzo di ogni riga e' controllato contro i bordi della riga e delle
/// maschere originali (etichetta `0`), non della maschera dissolta: gli
/// spostamenti delle due griglie non si sommano oltre la precisione.
fn clip_to_mask_impl(
    geometries: &[Geometry<f64>],
    masks: &[Geometry<f64>],
    validated: bool,
    precision: Precision,
) -> Result<Vec<Option<Geometry<f64>>>, TopologyError> {
    if masks.is_empty() {
        return Ok(vec![None; geometries.len()]);
    }
    let mask = checked_result(dissolve_raw(masks, validated, precision)?)?;
    let indice = IndiceLinework::nuovo(
        masks.iter().map(|mask| (0, mask)).chain(
            geometries
                .iter()
                .enumerate()
                .map(|(row, geometry)| (row + 1, geometry)),
        ),
    );
    geometries
        .iter()
        .enumerate()
        .map(|(row, geometry)| {
            let clipped = boolean_raw(
                geometry,
                &mask,
                BooleanOperation::Intersection,
                precision,
                validated,
            )?;
            let clipped = checked_displacement(
                clipped,
                &indice,
                |etichetta| etichetta == 0 || etichetta == row + 1,
                precision,
            )?;
            Ok((!is_empty(&clipped)).then_some(clipped))
        })
        .collect()
}

fn push_piece(
    pieces: &mut Vec<OverlayPiece>,
    geometry: Geometry<f64>,
    left: Option<usize>,
    right: Option<usize>,
    max_results: u64,
) -> Result<(), TopologyError> {
    if is_empty(&geometry) {
        return Ok(());
    }
    if u64::try_from(pieces.len()).map_err(|_| TopologyError::IndexOverflow)? >= max_results {
        return Err(TopologyError::ResourceLimit {
            name: "overlay_results",
            actual: u64::try_from(pieces.len())
                .map_err(|_| TopologyError::IndexOverflow)?
                .saturating_add(1),
            limit: max_results,
        });
    }
    pieces.push(OverlayPiece {
        geometry,
        left: left
            .map(u64::try_from)
            .transpose()
            .map_err(|_| TopologyError::IndexOverflow)?,
        right: right
            .map(u64::try_from)
            .transpose()
            .map_err(|_| TopologyError::IndexOverflow)?,
    });
    Ok(())
}

/// Polygonal overlay with explicit attribute lineage.
///
/// Boundary-only line/point intersections are intentionally excluded;
/// enabling `keep_geom_type=false` requires the GEOS backend and must not
/// silently use this kernel.
///
/// # Errors
///
/// - `InvalidParameter`: `max_candidate_pairs` or `max_results` is zero.
/// - `UnsupportedGeometry`: an input is not Polygon/MultiPolygon.
/// - `InvalidGeometry`: an input fails OGC validation, the candidate-pair
///   join fails, or a produced piece is not valid.
/// - `ResourceLimit`: the pieces exceed `max_results`.
/// - `IndexOverflow`: an index is not representable as `u64`/`usize`.
/// - `PrecisionInsufficient`: vedi [`boolean_operation`].
pub fn polygon_overlay(
    left: &[Geometry<f64>],
    right: &[Geometry<f64>],
    mode: OverlayMode,
    max_candidate_pairs: u64,
    max_results: u64,
    precision: Precision,
) -> Result<Vec<OverlayPiece>, TopologyError> {
    polygon_overlay_impl(
        left,
        right,
        mode,
        (max_candidate_pairs, max_results),
        false,
        precision,
    )
}

/// Variante di [`polygon_overlay`] SENZA il gate OGC di ingresso.
///
/// Stessa precondizione e stesso contratto di
/// [`boolean_operation_validated`]; anche il join candidati interno non
/// rivalida. Le validazioni OGC degli output (maschere dissolte e pezzi)
/// restano.
///
/// # Errors
///
/// Come [`polygon_overlay`], ma `InvalidGeometry` resta raggiungibile solo
/// dagli output (gate di ingresso omesso).
pub fn polygon_overlay_validated(
    left: &[Geometry<f64>],
    right: &[Geometry<f64>],
    mode: OverlayMode,
    max_candidate_pairs: u64,
    max_results: u64,
    precision: Precision,
) -> Result<Vec<OverlayPiece>, TopologyError> {
    polygon_overlay_impl(
        left,
        right,
        mode,
        (max_candidate_pairs, max_results),
        true,
        precision,
    )
}

/// Le coppie di righe che si intersecano, dopo il gate di tipo (e OGC, nel
/// percorso gated) di ogni riga.
fn candidate_pairs(
    left: &[Geometry<f64>],
    right: &[Geometry<f64>],
    max_candidate_pairs: u64,
    validated: bool,
) -> Result<Vec<crate::spatial_join::JoinPair>, TopologyError> {
    for geometry in left.iter().chain(right) {
        coerce(geometry, validated)?;
    }
    // Percorso gated: il join candidati rivalida gli input. Percorso
    // validated: gli input sono coperti dalla precondizione, il join non
    // rivalida.
    if validated {
        crate::spatial_join::spatial_join_validated(
            left,
            right,
            crate::spatial_join::JoinPredicate::Intersects,
            max_candidate_pairs,
        )
    } else {
        crate::spatial_join::spatial_join(
            left,
            right,
            crate::spatial_join::JoinPredicate::Intersects,
            max_candidate_pairs,
        )
    }
    .map_err(dal_join)
}

/// Ogni pezzo e' controllato contro i bordi degli ingressi originali da cui
/// viene: etichette `i` per la riga sinistra `i`, `left.len() + j` per la
/// destra `j`; un resto contro tutte le righe dell'altro lato.
fn polygon_overlay_impl(
    left: &[Geometry<f64>],
    right: &[Geometry<f64>],
    mode: OverlayMode,
    (max_candidate_pairs, max_results): (u64, u64),
    validated: bool,
    precision: Precision,
) -> Result<Vec<OverlayPiece>, TopologyError> {
    if max_candidate_pairs == 0 || max_results == 0 {
        return Err(TopologyError::InvalidParameter {
            name: "overlay_limits",
            reason: "devono essere maggiori di zero",
        });
    }
    let pairs = candidate_pairs(left, right, max_candidate_pairs, validated)?;
    let left_count = left.len();
    let indice = IndiceLinework::nuovo(
        left.iter().enumerate().chain(
            right
                .iter()
                .enumerate()
                .map(|(j, geometry)| (left_count + j, geometry)),
        ),
    );
    let boolean =
        |left: &Geometry<f64>, right: &Geometry<f64>, operation, filtro: &dyn Fn(usize) -> bool| {
            let result = boolean_raw(left, right, operation, precision, validated)?;
            checked_displacement(result, &indice, filtro, precision)
        };
    let dissolve_side = |geometries: &[Geometry<f64>]| {
        checked_result(dissolve_raw(geometries, validated, precision)?)
    };
    let mut pieces = Vec::new();

    if matches!(
        mode,
        OverlayMode::Intersection | OverlayMode::Union | OverlayMode::Identity
    ) {
        for pair in pairs {
            let left_index =
                usize::try_from(pair.left).map_err(|_| TopologyError::IndexOverflow)?;
            let right_index =
                usize::try_from(pair.right).map_err(|_| TopologyError::IndexOverflow)?;
            let geometry = boolean(
                &left[left_index],
                &right[right_index],
                BooleanOperation::Intersection,
                &|etichetta| etichetta == left_index || etichetta == left_count + right_index,
            )?;
            push_piece(
                &mut pieces,
                geometry,
                Some(left_index),
                Some(right_index),
                max_results,
            )?;
        }
    }

    if matches!(
        mode,
        OverlayMode::Union | OverlayMode::Identity | OverlayMode::SymmetricDifference
    ) {
        let right_mask = (!right.is_empty())
            .then(|| dissolve_side(right))
            .transpose()?;
        for (index, geometry) in left.iter().enumerate() {
            let remainder = match &right_mask {
                Some(mask) => {
                    boolean(geometry, mask, BooleanOperation::Difference, &|etichetta| {
                        etichetta == index || etichetta >= left_count
                    })?
                }
                None => geometry.clone(),
            };
            push_piece(&mut pieces, remainder, Some(index), None, max_results)?;
        }
    }

    if matches!(mode, OverlayMode::Union | OverlayMode::SymmetricDifference) {
        let left_mask = (!left.is_empty())
            .then(|| dissolve_side(left))
            .transpose()?;
        for (index, geometry) in right.iter().enumerate() {
            let remainder = match &left_mask {
                Some(mask) => {
                    boolean(geometry, mask, BooleanOperation::Difference, &|etichetta| {
                        etichetta == left_count + index || etichetta < left_count
                    })?
                }
                None => geometry.clone(),
            };
            push_piece(&mut pieces, remainder, None, Some(index), max_results)?;
        }
    }
    Ok(pieces)
}

/// Ordered topology cleanup for inputs that are already valid polygons.
///
/// Applies the same gap-closing morphology and first-row-wins overlap
/// policy as Manipola. Invalid inputs are rejected; repair belongs to GEOS
/// make-valid.
///
/// # Errors
///
/// - `InvalidParameter`: `snap_tolerance` is not finite or is negative.
/// - `ResourceLimit`: the input exceeds `max_geometries` or `max_vertices`.
/// - `UnsupportedGeometry`: an input is not Polygon/MultiPolygon.
/// - `InvalidGeometry`: an input fails OGC validation, or a morphology or
///   overlap-removal step produces an invalid geometry.
/// - `IndexOverflow`: a count is not representable as `u64`.
/// - `PrecisionInsufficient`: un buffer della morfologia o una booleana
///   sposterebbe, o ha spostato, il risultato oltre la precisione.
pub fn clean_valid_polygon_topology(
    geometries: &[Geometry<f64>],
    snap_tolerance: f64,
    remove_overlaps: bool,
    fill_gaps: bool,
    max_geometries: u64,
    max_vertices: u64,
    precision: Precision,
) -> Result<Vec<Option<Geometry<f64>>>, TopologyError> {
    clean_valid_polygon_topology_impl(
        geometries,
        snap_tolerance,
        remove_overlaps,
        fill_gaps,
        (max_geometries, max_vertices),
        false,
        precision,
    )
}

/// Variante di [`clean_valid_polygon_topology`] SENZA il gate OGC di
/// ingresso.
///
/// Stessa precondizione e stesso contratto di
/// [`boolean_operation_validated`]. Restano SEMPRE validati: l'output di
/// ogni booleana e la geometria prodotta dalla morfologia `buffer`
/// (output di kernel che NON garantisce la validita' — la catena di
/// fiducia non si applica, regola R0.1).
///
/// # Errors
///
/// Come [`clean_valid_polygon_topology`], ma `InvalidGeometry` resta
/// raggiungibile solo dagli output intermedi/prodotti (gate di ingresso
/// omesso).
pub fn clean_valid_polygon_topology_validated(
    geometries: &[Geometry<f64>],
    snap_tolerance: f64,
    remove_overlaps: bool,
    fill_gaps: bool,
    max_geometries: u64,
    max_vertices: u64,
    precision: Precision,
) -> Result<Vec<Option<Geometry<f64>>>, TopologyError> {
    clean_valid_polygon_topology_impl(
        geometries,
        snap_tolerance,
        remove_overlaps,
        fill_gaps,
        (max_geometries, max_vertices),
        true,
        precision,
    )
}

/// Un buffer della morfologia con i controlli di precisione del buffer
/// (`rust_backend::griglia::controlla_buffer` e `verifica_buffer`).
fn checked_buffer(
    geometry: &Geometry<f64>,
    distance: f64,
    precision: Precision,
) -> Result<MultiPolygon<f64>, TopologyError> {
    griglia::controlla_buffer(geometry, distance, precision)?;
    let result = geometry.buffer(distance);
    griglia::verifica_buffer(
        geometry,
        distance,
        griglia::Estremita::Tonde,
        &result,
        precision,
    )?;
    Ok(result)
}

/// La morfologia (`fill_gaps`) e' un buffer positivo e uno negativo, ognuno
/// con i controlli del buffer. La rimozione delle sovrapposizioni controlla
/// ogni resto contro i bordi delle righe fino alla sua (dopo la
/// morfologia), non contro l'unione accumulata: gli spostamenti delle
/// unioni in catena non si sommano nel risultato oltre la precisione.
fn clean_valid_polygon_topology_impl(
    geometries: &[Geometry<f64>],
    snap_tolerance: f64,
    remove_overlaps: bool,
    fill_gaps: bool,
    (max_geometries, max_vertices): (u64, u64),
    validated: bool,
    precision: Precision,
) -> Result<Vec<Option<Geometry<f64>>>, TopologyError> {
    if !snap_tolerance.is_finite() || snap_tolerance < 0.0 {
        return Err(TopologyError::InvalidParameter {
            name: "snap_tolerance",
            reason: "deve essere finita e non negativa",
        });
    }
    let geometry_count =
        u64::try_from(geometries.len()).map_err(|_| TopologyError::IndexOverflow)?;
    if geometry_count > max_geometries {
        return Err(TopologyError::ResourceLimit {
            name: "geometries",
            actual: geometry_count,
            limit: max_geometries,
        });
    }
    let mut vertices = 0_u64;
    for geometry in geometries {
        if validated {
            as_multi_polygon_validated(geometry)?;
        } else {
            as_multi_polygon(geometry)?;
        }
        vertices = vertices
            .checked_add(
                u64::try_from(geometry.coords_count()).map_err(|_| TopologyError::IndexOverflow)?,
            )
            .ok_or(TopologyError::IndexOverflow)?;
    }
    if vertices > max_vertices {
        return Err(TopologyError::ResourceLimit {
            name: "vertices",
            actual: vertices,
            limit: max_vertices,
        });
    }

    let mut working: Vec<Option<Geometry<f64>>> = geometries.iter().cloned().map(Some).collect();
    if fill_gaps && snap_tolerance > 0.0 {
        for geometry in working.iter_mut().flatten() {
            let expanded =
                Geometry::MultiPolygon(checked_buffer(geometry, snap_tolerance, precision)?);
            let closed =
                Geometry::MultiPolygon(checked_buffer(&expanded, -snap_tolerance, precision)?);
            // Output di `buffer` (kernel che NON garantisce la validita'):
            // gate OGC completo in entrambe le forme — nessuna catena di
            // fiducia su geometrie prodotte senza garanzia.
            *geometry = checked_result(as_multi_polygon(&closed)?)?;
        }
    }
    if remove_overlaps {
        let indice = IndiceLinework::nuovo(
            working
                .iter()
                .enumerate()
                .filter_map(|(row, geometry)| geometry.as_ref().map(|geometry| (row, geometry))),
        );
        let mut accumulated: Option<Geometry<f64>> = None;
        for (row, geometry) in working.iter_mut().enumerate() {
            let Some(current) = geometry.take() else {
                continue;
            };
            let remainder = match &accumulated {
                Some(previous) => {
                    let result = boolean_raw(
                        &current,
                        previous,
                        BooleanOperation::Difference,
                        precision,
                        validated,
                    )?;
                    checked_displacement(result, &indice, |etichetta| etichetta <= row, precision)?
                }
                None => current,
            };
            if is_empty(&remainder) {
                continue;
            }
            accumulated = Some(match accumulated {
                Some(previous) => checked_result(boolean_raw(
                    &previous,
                    &remainder,
                    BooleanOperation::Union,
                    precision,
                    validated,
                )?)?,
                None => remainder.clone(),
            });
            *geometry = Some(remainder);
        }
    }
    Ok(working)
}

#[cfg(test)]
// Confronti float esatti intenzionali: le fixture sono costruite per
// produrre valori esatti (coordinate note, round-trip bit-esatti); il
// confronto per bit e' il contratto verificato, non un'approssimazione.
#[allow(clippy::float_cmp)]
mod tests {
    use super::*;
    use crate::test_support::{bowtie, rect};
    use geo::{
        line_string, Area, GeometryCollection, Line, MultiLineString, MultiPoint, Point, Rect,
        Triangle,
    };
    use proptest::prelude::*;

    /// Il join che non conclude resta un calcolo non concluso, non una
    /// geometria invalida.
    #[test]
    fn il_join_non_concluso_non_diventa_una_geometria_invalida() {
        use crate::spatial_join::SpatialJoinError as S;

        assert!(matches!(
            dal_join(S::ValidazioneNonConclusa("forma")),
            TopologyError::ValidazioneNonConclusa("forma")
        ));
        for errore in [S::CalcoloNonConcluso("forma"), S::Internal("forma")] {
            assert!(matches!(
                dal_join(errore),
                TopologyError::CalcoloNonConcluso("forma")
            ));
        }
        assert!(matches!(
            dal_join(S::InvalidPairLimit),
            TopologyError::InvalidGeometry(_)
        ));
    }

    /// Sintetico attraverso la conversione reale (come
    /// `predicates::classifica_lato_non_appiattisce_l_interruzione`).
    ///
    /// Nessun reperto reale interrompe `geo` col candidato esatto, quindi
    /// l'innesco e' un `EsitoValidazione::NonConclusa` costruito a mano, ma
    /// la funzione chiamata e' quella vera di `dissolve`.
    #[test]
    fn classifica_geometria_non_appiattisce_l_interruzione() {
        let esito = crate::EsitoValidazione::NonConclusa("forma di prova");
        let errore = classifica_geometria(esito);
        assert!(
            matches!(
                errore,
                TopologyError::ValidazioneNonConclusa("forma di prova")
            ),
            "atteso ValidazioneNonConclusa, ottenuto: {errore:?}"
        );
        assert_eq!(
            errore.to_string(),
            "validazione OGC non conclusa: forma di prova (contenuto non pubblicato)"
        );
    }

    /// Controprova: un esito concluso produce l'altra variante — mai la
    /// stessa.
    #[test]
    fn classifica_geometria_su_esito_concluso_resta_invalidgeometry() {
        let esito = crate::EsitoValidazione::NonValida(crate::RagioneNonValida::AutoIntersezione);
        let errore = classifica_geometria(esito);
        assert!(
            matches!(errore, TopologyError::InvalidGeometry(_)),
            "atteso InvalidGeometry, ottenuto: {errore:?}"
        );
        assert_eq!(
            errore.to_string(),
            "geometria topologica non valida: anello con auto-intersezione"
        );
    }

    /// Precisione dei test: coordinate astratte fino a qualche centinaio di
    /// unita', un milionesimo di unita' (la griglia degli overlay resta sotto).
    fn precisione() -> Precision {
        Precision::new(1e-6).unwrap()
    }

    fn square(x: f64, y: f64, size: f64) -> Geometry<f64> {
        rect(x, y, x + size, y + size)
    }

    #[test]
    fn boolean_areas_match_known_overlapping_squares() {
        let left = square(0.0, 0.0, 2.0);
        let right = square(1.0, 0.0, 2.0);
        let cases = [
            (BooleanOperation::Intersection, 2.0),
            (BooleanOperation::Union, 6.0),
            (BooleanOperation::Difference, 2.0),
            (BooleanOperation::SymmetricDifference, 4.0),
        ];
        for (operation, expected_area) in cases {
            let result = boolean_operation(&left, &right, operation, precisione()).unwrap();
            assert_eq!(result.unsigned_area(), expected_area);
        }
    }

    #[test]
    fn dissolve_merges_overlaps_and_rejects_non_polygons() {
        let result = dissolve(
            &[square(0.0, 0.0, 2.0), square(1.0, 0.0, 2.0)],
            precisione(),
        )
        .unwrap();
        assert_eq!(result.unsigned_area(), 6.0);
        assert!(matches!(
            dissolve(&[Geometry::Point(Point::new(0.0, 0.0))], precisione()),
            Err(TopologyError::UnsupportedGeometry("Point"))
        ));
    }

    #[test]
    fn clip_preserves_rows_and_marks_empty_results() {
        let inputs = vec![square(0.0, 0.0, 2.0), square(10.0, 10.0, 1.0)];
        let result = clip_to_mask(&inputs, &[square(1.0, 0.0, 2.0)], precisione()).unwrap();
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].as_ref().unwrap().unsigned_area(), 2.0);
        assert!(result[1].is_none());
    }

    #[test]
    fn overlay_lineage_and_areas_are_complete_and_deterministic() {
        let left = vec![square(0.0, 0.0, 2.0)];
        let right = vec![square(1.0, 0.0, 2.0)];
        let pieces =
            polygon_overlay(&left, &right, OverlayMode::Union, 10, 10, precisione()).unwrap();
        assert_eq!(pieces.len(), 3);
        assert_eq!(pieces[0].left, Some(0));
        assert_eq!(pieces[0].right, Some(0));
        assert_eq!(pieces[1].left, Some(0));
        assert_eq!(pieces[1].right, None);
        assert_eq!(pieces[2].left, None);
        assert_eq!(pieces[2].right, Some(0));
        assert_eq!(
            pieces
                .iter()
                .map(|piece| piece.geometry.unsigned_area())
                .sum::<f64>(),
            6.0
        );

        assert!(matches!(
            polygon_overlay(&left, &right, OverlayMode::Union, 10, 2, precisione()),
            Err(TopologyError::ResourceLimit {
                name: "overlay_results",
                ..
            })
        ));
    }

    #[test]
    fn clean_topology_removes_overlap_with_first_row_wins() {
        let cleaned = clean_valid_polygon_topology(
            &[square(0.0, 0.0, 2.0), square(1.0, 0.0, 2.0)],
            0.0,
            true,
            false,
            10,
            100,
            precisione(),
        )
        .unwrap();
        assert_eq!(cleaned[0].as_ref().unwrap().unsigned_area(), 4.0);
        assert_eq!(cleaned[1].as_ref().unwrap().unsigned_area(), 2.0);
        assert_eq!(
            boolean_operation(
                cleaned[0].as_ref().unwrap(),
                cleaned[1].as_ref().unwrap(),
                BooleanOperation::Intersection,
                precisione(),
            )
            .unwrap()
            .unsigned_area(),
            0.0
        );
        assert!(matches!(
            clean_valid_polygon_topology(
                &[square(0.0, 0.0, 2.0)],
                0.0,
                true,
                false,
                0,
                100,
                precisione()
            ),
            Err(TopologyError::ResourceLimit {
                name: "geometries",
                ..
            })
        ));
    }

    #[test]
    fn every_unsupported_geometry_family_is_rejected_explicitly() {
        let values = [
            Geometry::Point(Point::new(0.0, 0.0)),
            Geometry::Line(Line::new((0.0, 0.0), (1.0, 1.0))),
            Geometry::LineString(line_string![(x: 0.0, y: 0.0), (x: 1.0, y: 1.0)]),
            Geometry::MultiPoint(MultiPoint::new(vec![Point::new(0.0, 0.0)])),
            Geometry::MultiLineString(MultiLineString::new(vec![line_string![
                (x: 0.0, y: 0.0), (x: 1.0, y: 1.0)
            ]])),
            Geometry::GeometryCollection(GeometryCollection(vec![])),
            Geometry::Rect(Rect::new((0.0, 0.0), (1.0, 1.0))),
            Geometry::Triangle(Triangle::new(
                (0.0, 0.0).into(),
                (1.0, 0.0).into(),
                (0.0, 1.0).into(),
            )),
        ];
        let names = [
            "Point",
            "Line",
            "LineString",
            "MultiPoint",
            "MultiLineString",
            "GeometryCollection",
            "Rect",
            "Triangle",
        ];
        for (value, expected) in values.iter().zip(names) {
            assert!(matches!(
                dissolve(std::slice::from_ref(value), precisione()),
                Err(TopologyError::UnsupportedGeometry(actual)) if actual == expected
            ));
        }
    }

    #[test]
    fn clip_overlay_and_cleanup_cover_empty_and_limit_boundaries() {
        let inputs = vec![square(0.0, 0.0, 1.0), square(3.0, 3.0, 1.0)];
        assert_eq!(
            clip_to_mask(&inputs, &[], precisione()).unwrap(),
            vec![None, None]
        );

        for mode in [
            OverlayMode::Intersection,
            OverlayMode::Identity,
            OverlayMode::SymmetricDifference,
        ] {
            let pieces =
                polygon_overlay(&inputs[..1], &inputs[1..], mode, 10, 10, precisione()).unwrap();
            match mode {
                OverlayMode::Intersection => assert!(pieces.is_empty()),
                OverlayMode::Identity => assert_eq!(pieces.len(), 1),
                OverlayMode::SymmetricDifference => assert_eq!(pieces.len(), 2),
                OverlayMode::Union => unreachable!(),
            }
        }
        assert_eq!(
            polygon_overlay(&inputs[..1], &[], OverlayMode::Union, 10, 10, precisione())
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            polygon_overlay(&[], &inputs[..1], OverlayMode::Union, 10, 10, precisione())
                .unwrap()
                .len(),
            1
        );
        assert!(matches!(
            polygon_overlay(&inputs, &inputs, OverlayMode::Union, 0, 10, precisione()),
            Err(TopologyError::InvalidParameter { .. })
        ));
        assert!(matches!(
            polygon_overlay(&inputs, &inputs, OverlayMode::Union, 10, 0, precisione()),
            Err(TopologyError::InvalidParameter { .. })
        ));

        assert!(matches!(
            clean_valid_polygon_topology(&inputs, f64::NAN, false, false, 10, 100, precisione()),
            Err(TopologyError::InvalidParameter { .. })
        ));
        assert!(matches!(
            clean_valid_polygon_topology(&inputs, -1.0, false, false, 10, 100, precisione()),
            Err(TopologyError::InvalidParameter { .. })
        ));
        assert!(matches!(
            clean_valid_polygon_topology(&inputs, 0.0, false, false, 10, 5, precisione()),
            Err(TopologyError::ResourceLimit {
                name: "vertices",
                ..
            })
        ));
        let closed = clean_valid_polygon_topology(
            &[square(0.0, 0.0, 1.0), square(1.05, 0.0, 1.0)],
            0.1,
            true,
            true,
            10,
            100,
            precisione(),
        )
        .unwrap();
        assert_eq!(closed.len(), 2);
        assert!(closed.iter().all(Option::is_some));

        let swallowed = clean_valid_polygon_topology(
            &[square(0.0, 0.0, 4.0), square(1.0, 1.0, 1.0)],
            0.0,
            true,
            false,
            10,
            100,
            precisione(),
        )
        .unwrap();
        assert!(swallowed[0].is_some());
        assert!(swallowed[1].is_none());
    }

    #[test]
    fn validated_variants_match_the_gated_path_on_valid_inputs() {
        let left = vec![square(0.0, 0.0, 2.0), square(10.0, 10.0, 1.0)];
        let right = vec![square(1.0, 0.0, 2.0), square(20.0, 20.0, 1.0)];
        for operation in [
            BooleanOperation::Intersection,
            BooleanOperation::Union,
            BooleanOperation::Difference,
            BooleanOperation::SymmetricDifference,
        ] {
            assert_eq!(
                boolean_operation(&left[0], &right[0], operation, precisione()).unwrap(),
                boolean_operation_validated(&left[0], &right[0], operation, precisione()).unwrap(),
                "{operation:?}"
            );
        }
        assert_eq!(
            dissolve(&left, precisione()).unwrap(),
            dissolve_validated(&left, precisione()).unwrap()
        );
        assert_eq!(
            clip_to_mask(&left, &right[..1], precisione()).unwrap(),
            clip_to_mask_validated(&left, &right[..1], precisione()).unwrap()
        );
        for mode in [
            OverlayMode::Intersection,
            OverlayMode::Union,
            OverlayMode::Identity,
            OverlayMode::SymmetricDifference,
        ] {
            assert_eq!(
                polygon_overlay(&left, &right, mode, 100, 100, precisione()).unwrap(),
                polygon_overlay_validated(&left, &right, mode, 100, 100, precisione()).unwrap(),
                "{mode:?}"
            );
        }
        assert_eq!(
            clean_valid_polygon_topology(&left, 0.1, true, true, 10, 1_000, precisione()).unwrap(),
            clean_valid_polygon_topology_validated(&left, 0.1, true, true, 10, 1_000, precisione())
                .unwrap()
        );
        // Il gate di TIPO resta nella variante validated (contratto dei
        // kernel poligonali, non una validazione).
        let point = Geometry::Point(Point::new(0.0, 0.0));
        assert!(matches!(
            boolean_operation_validated(&point, &left[0], BooleanOperation::Union, precisione()),
            Err(TopologyError::UnsupportedGeometry("Point"))
        ));
        assert!(matches!(
            dissolve_validated(std::slice::from_ref(&point), precisione()),
            Err(TopologyError::UnsupportedGeometry("Point"))
        ));
        assert!(matches!(
            polygon_overlay_validated(&left, &right, OverlayMode::Union, 0, 10, precisione()),
            Err(TopologyError::InvalidParameter { .. })
        ));
        assert!(matches!(
            clean_valid_polygon_topology_validated(
                &left,
                f64::NAN,
                true,
                true,
                10,
                1_000,
                precisione()
            ),
            Err(TopologyError::InvalidParameter { .. })
        ));
    }

    #[test]
    fn validated_variants_document_the_caller_precondition() {
        // Precondizione violata ad arte: il percorso gated rifiuta il bowtie
        // in ingresso, la variante validated no. Gli errori ammessi sono solo
        // quelli dei gate di output (`InvalidGeometry`) e di tipo: nessun
        // panic.
        let bowtie = bowtie();
        let valid = square(0.0, 0.0, 2.0);
        assert!(matches!(
            boolean_operation(
                &bowtie,
                &valid,
                BooleanOperation::Intersection,
                precisione()
            ),
            Err(TopologyError::InvalidGeometry(_))
        ));
        let outcome = boolean_operation_validated(
            &bowtie,
            &valid,
            BooleanOperation::Intersection,
            precisione(),
        );
        assert!(
            outcome.is_ok() || matches!(outcome, Err(TopologyError::InvalidGeometry(_))),
            "solo il gate di output puo' fallire: {outcome:?}"
        );
        assert!(matches!(
            dissolve(std::slice::from_ref(&bowtie), precisione()),
            Err(TopologyError::InvalidGeometry(_))
        ));
        let outcome = dissolve_validated(std::slice::from_ref(&bowtie), precisione());
        assert!(
            outcome.is_ok() || matches!(outcome, Err(TopologyError::InvalidGeometry(_))),
            "solo il gate di output puo' fallire: {outcome:?}"
        );
    }

    proptest! {
        #[test]
        fn boolean_area_identities_hold_for_generated_rectangles(
            ax in -100_i16..100,
            ay in -100_i16..100,
            aw in 1_u8..30,
            ah in 1_u8..30,
            bx in -100_i16..100,
            by in -100_i16..100,
            bw in 1_u8..30,
            bh in 1_u8..30,
        ) {
            let left = square(f64::from(ax), f64::from(ay), f64::from(aw.min(ah)));
            let right = square(f64::from(bx), f64::from(by), f64::from(bw.min(bh)));
            let left_area = left.unsigned_area();
            let right_area = right.unsigned_area();
            let intersection = boolean_operation(
                &left, &right, BooleanOperation::Intersection
            , precisione()).unwrap().unsigned_area();
            let union = boolean_operation(
                &left, &right, BooleanOperation::Union
            , precisione()).unwrap().unsigned_area();
            let difference = boolean_operation(
                &left, &right, BooleanOperation::Difference
            , precisione()).unwrap().unsigned_area();
            let xor = boolean_operation(
                &left, &right, BooleanOperation::SymmetricDifference
            , precisione()).unwrap().unsigned_area();
            prop_assert!((left_area + right_area - union - intersection).abs() < 1e-9);
            prop_assert!((left_area - difference - intersection).abs() < 1e-9);
            prop_assert!((xor - (union - intersection)).abs() < 1e-9);
        }
    }
}
