//! Join spaziale deterministico e limitato.
//!
//! Un R-tree dei rettangoli d'ingombro destri sceglie i candidati. E' il
//! kernel di `geo.sjoin`, e la base di `geo.within`,
//! `geo.count_points_in_polygons` e delle coppie candidate di
//! `geo.overlay`.

use std::sync::atomic::{AtomicU64, Ordering};

use crate::ValidazioneProtetta as _;
use geo::{BoundingRect, Contains, CoordsIter, Geometry, Intersects, Relate};
use rayon::prelude::*;
use rstar::{RTree, RTreeObject, AABB};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Il predicato esatto di una coppia (sinistra `L`, destra `R`).
///
/// E' il parametro `predicate` di `geo.sjoin`, in serde `snake_case`. I
/// predicati sono quelli di `geo`, esatti; tutti tranne `Intersects`
/// passano dalla matrice DE-9IM.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum JoinPredicate {
    /// `L` e `R` hanno almeno un punto in comune, bordo compreso
    /// (`intersects`).
    Intersects,
    /// `L` contiene `R`: nessun punto di `R` fuori da `L` e almeno un punto
    /// dell'interno di `R` nell'interno di `L`; `R` sul solo bordo di `L`
    /// non e' contenuta (`contains`).
    Contains,
    /// `L` e' contenuta in `R`, cioe' `R` contiene `L` (`within`).
    Within,
    /// `L` e `R` si attraversano, secondo la DE-9IM (`crosses`).
    Crosses,
    /// `L` e `R` si sovrappongono in parte, secondo la DE-9IM (`overlaps`).
    Overlaps,
    /// `L` e `R` si toccano solo sul bordo, secondo la DE-9IM (`touches`).
    Touches,
}

/// Una coppia confermata dal predicato: le posizioni della riga sinistra e
/// della destra.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct JoinPair {
    /// Posizione della riga sinistra.
    pub left: u64,
    /// Posizione della riga destra.
    pub right: u64,
}

/// Gli errori del join spaziale. Nessun messaggio porta valori delle
/// geometrie.
#[derive(Debug, Error)]
pub enum SpatialJoinError {
    /// Un numero di righe o un indice non entra in `u64`.
    #[error("numero geometrie non rappresentabile nel protocollo uint64")]
    IndexOverflow,
    /// `max_pairs` e' zero.
    #[error("max_pairs deve essere maggiore di zero")]
    InvalidPairLimit,
    /// Le coppie confermate superano `max_pairs` (`limit`).
    #[error("spatial join oltre il limite di {limit} coppie")]
    PairLimitExceeded { limit: u64 },
    /// Una geometria ha coordinate NaN o infinite; porta il lato (`side`,
    /// `left` o `right`) e la posizione della riga (`index`).
    #[error("geometria {side}[{index}] contiene coordinate NaN o infinite")]
    NonFiniteCoordinate { side: &'static str, index: usize },
    /// Una geometria non supera la validazione OGC; porta il lato e la
    /// posizione.
    #[error("geometria {side}[{index}] non valida: {reason}")]
    InvalidGeometry {
        /// `left` o `right`.
        side: &'static str,
        /// La posizione della riga.
        index: usize,
        /// Il motivo, senza valori.
        reason: String,
    },
    /// Invariante interna violata: errore propagato, mai un panico.
    #[error("internal error: {0}")]
    Internal(&'static str),
    /// La validazione OGC non ha concluso: `geo` si e' interrotta.
    ///
    /// **Non** e' una geometria invalida. Nessuno ha dimostrato che l'ingresso
    /// sia sbagliato, e accusarlo manderebbe chi legge a correggere un errore
    /// che non ha commesso. Porta la *forma* del payload, mai il contenuto.
    #[error("validazione OGC non conclusa: {0} (contenuto non pubblicato)")]
    ValidazioneNonConclusa(&'static str),
    /// Il predicato esatto non ha concluso su geometrie **valide**: `relate`
    /// si e' interrotta (`calcolo_protetto`). Non accusa l'ingresso,
    /// e porta la *forma* del payload, mai il contenuto.
    #[error("predicato esatto non concluso: {0} (contenuto non pubblicato)")]
    CalcoloNonConcluso(&'static str),
}

#[derive(Clone, Copy)]
struct IndexedEnvelope {
    index: usize,
    envelope: AABB<[f64; 2]>,
}

impl RTreeObject for IndexedEnvelope {
    type Envelope = AABB<[f64; 2]>;

    fn envelope(&self) -> Self::Envelope {
        self.envelope
    }
}

fn checked_envelope(
    geometry: &Geometry<f64>,
    side: &'static str,
    index: usize,
) -> Result<Option<AABB<[f64; 2]>>, SpatialJoinError> {
    if geometry
        .coords_iter()
        .any(|coordinate| !coordinate.x.is_finite() || !coordinate.y.is_finite())
    {
        return Err(SpatialJoinError::NonFiniteCoordinate { side, index });
    }
    geometry.validazione_protetta().map_err(|esito| {
        esito.separa(
            |ragione| SpatialJoinError::InvalidGeometry {
                side,
                index,
                reason: ragione.to_string(),
            },
            SpatialJoinError::ValidazioneNonConclusa,
        )
    })?;
    envelope_of_validated(geometry, side, index)
}

/// Envelope di una geometria GIA' VALIDATA (coordinate finite + validita'
/// OGC): nessuna camminata di controllo sull'input, la precondizione e' del
/// chiamante (vedi [`spatial_join_validated`]). Il check di finitezza del
/// bounding box resta: costo nullo (il rect e' gia' calcolato) e chiude in
/// fail-closed il caso di precondizione violata con coordinate non finite.
fn envelope_of_validated(
    geometry: &Geometry<f64>,
    side: &'static str,
    index: usize,
) -> Result<Option<AABB<[f64; 2]>>, SpatialJoinError> {
    let Some(rect) = geometry.bounding_rect() else {
        return Ok(None);
    };
    let min = rect.min();
    let max = rect.max();
    if !min.x.is_finite() || !min.y.is_finite() || !max.x.is_finite() || !max.y.is_finite() {
        return Err(SpatialJoinError::NonFiniteCoordinate { side, index });
    }
    Ok(Some(AABB::from_corners([min.x, min.y], [max.x, max.y])))
}

/// Il predicato esatto, dietro [`crate::calcolo_protetto`]: `contains` e i
/// predicati DE-9IM passano da `relate`, che puo' andare in panico anche su
/// geometrie valide.
fn exact_match(
    left: &Geometry<f64>,
    right: &Geometry<f64>,
    predicate: JoinPredicate,
) -> Result<bool, SpatialJoinError> {
    crate::calcolo_protetto(|| match predicate {
        JoinPredicate::Intersects => left.intersects(right),
        JoinPredicate::Contains => left.contains(right),
        JoinPredicate::Within => right.contains(left),
        JoinPredicate::Crosses => left.relate(right).is_crosses(),
        JoinPredicate::Overlaps => left.relate(right).is_overlaps(),
        JoinPredicate::Touches => left.relate(right).is_touches(),
    })
    .map_err(SpatialJoinError::CalcoloNonConcluso)
}

/// Le coppie `(sinistra, destra)` che soddisfano `predicate`, in ordine
/// lessicografico crescente: il kernel di `geo.sjoin`.
///
/// Le geometrie vuote non producono coppie, e una riga sinistra senza
/// coppie non compare. I rettangoli d'ingombro scelgono solo i candidati;
/// ogni coppia e' confermata dal predicato esatto.
///
/// # Errors
///
/// - `SpatialJoinError::InvalidPairLimit`: `max_pairs` e' zero;
/// - `SpatialJoinError::IndexOverflow`: il numero di geometrie non e'
///   rappresentabile in `u64`;
/// - `SpatialJoinError::NonFiniteCoordinate`: una geometria contiene
///   coordinate NaN o infinite (coordinate o bounding box);
/// - `SpatialJoinError::InvalidGeometry`: una geometria non supera la
///   validazione OGC;
/// - `SpatialJoinError::PairLimitExceeded`: le coppie confermate superano
///   `max_pairs`;
/// - `SpatialJoinError::Internal`: invariante interna violata (mai attesa:
///   l'R-tree contiene solo geometrie non nulle).
pub fn spatial_join(
    left: &[Geometry<f64>],
    right: &[Geometry<f64>],
    predicate: JoinPredicate,
    max_pairs: u64,
) -> Result<Vec<JoinPair>, SpatialJoinError> {
    let left_refs: Vec<_> = left.iter().map(Some).collect();
    let right_refs: Vec<_> = right.iter().map(Some).collect();
    spatial_join_refs(&left_refs, &right_refs, predicate, max_pairs, false)
}

/// Variante di [`spatial_join`] SENZA il gate di ingresso (scansione di
/// finitezza + validazione OGC per geometria).
///
/// # Precondizione (contratto del chiamante)
///
/// Ogni geometria dei due lati deve essere GIA' validata (coordinate finite,
/// validita' OGC), come da [`crate::geometry_from_wkb`] o da un kernel che
/// valida il proprio output (es. `checked_result` in [`crate::topology`]).
/// Altrimenti il risultato e' indefinito. Solo per percorsi validati per
/// costruzione; il gate resta in [`spatial_join`].
///
/// # Errors
///
/// Come [`spatial_join`], eccetto `InvalidGeometry` (gate omesso);
/// `NonFiniteCoordinate` resta solo come difesa sul bounding box.
pub fn spatial_join_validated(
    left: &[Geometry<f64>],
    right: &[Geometry<f64>],
    predicate: JoinPredicate,
    max_pairs: u64,
) -> Result<Vec<JoinPair>, SpatialJoinError> {
    let left_refs: Vec<_> = left.iter().map(Some).collect();
    let right_refs: Vec<_> = right.iter().map(Some).collect();
    spatial_join_refs(&left_refs, &right_refs, predicate, max_pairs, true)
}

/// Variante di [`spatial_join`] su colonne con celle nulle: una riga `None`
/// non produce coppie, e ogni coppia porta le posizioni originali delle
/// righe, nulle comprese.
///
/// # Errors
///
/// Come [`spatial_join`]; le righe `None` non producono errori ne' coppie.
pub fn spatial_join_nullable(
    left: &[Option<Geometry<f64>>],
    right: &[Option<Geometry<f64>>],
    predicate: JoinPredicate,
    max_pairs: u64,
) -> Result<Vec<JoinPair>, SpatialJoinError> {
    let left_refs: Vec<_> = left.iter().map(Option::as_ref).collect();
    let right_refs: Vec<_> = right.iter().map(Option::as_ref).collect();
    spatial_join_refs(&left_refs, &right_refs, predicate, max_pairs, false)
}

/// Variante di [`spatial_join_nullable`] SENZA il gate di ingresso: stessa
/// precondizione e stesso contratto di [`spatial_join_validated`].
///
/// # Errors
///
/// Come [`spatial_join_validated`]; le righe `None` non producono errori
/// ne' coppie.
pub fn spatial_join_nullable_validated(
    left: &[Option<Geometry<f64>>],
    right: &[Option<Geometry<f64>>],
    predicate: JoinPredicate,
    max_pairs: u64,
) -> Result<Vec<JoinPair>, SpatialJoinError> {
    let left_refs: Vec<_> = left.iter().map(Option::as_ref).collect();
    let right_refs: Vec<_> = right.iter().map(Option::as_ref).collect();
    spatial_join_refs(&left_refs, &right_refs, predicate, max_pairs, true)
}

fn spatial_join_refs(
    left: &[Option<&Geometry<f64>>],
    right: &[Option<&Geometry<f64>>],
    predicate: JoinPredicate,
    max_pairs: u64,
    validated: bool,
) -> Result<Vec<JoinPair>, SpatialJoinError> {
    if max_pairs == 0 {
        return Err(SpatialJoinError::InvalidPairLimit);
    }
    u64::try_from(left.len()).map_err(|_| SpatialJoinError::IndexOverflow)?;
    u64::try_from(right.len()).map_err(|_| SpatialJoinError::IndexOverflow)?;

    let right_envelopes: Result<Vec<Option<_>>, _> = right
        .iter()
        .enumerate()
        .map(|(index, geometry)| {
            let Some(geometry) = geometry else {
                return Ok(None);
            };
            let envelope = if validated {
                envelope_of_validated(geometry, "right", index)?
            } else {
                checked_envelope(geometry, "right", index)?
            };
            Ok(envelope.map(|envelope| IndexedEnvelope { index, envelope }))
        })
        .collect();
    let right_envelopes: Vec<IndexedEnvelope> = right_envelopes?.into_iter().flatten().collect();
    let tree = crate::calcolo_protetto(|| RTree::bulk_load(right_envelopes))
        .map_err(SpatialJoinError::CalcoloNonConcluso)?;
    let pair_count = AtomicU64::new(0);

    // Determinismo: i `Result` per riga prima (ordine preservato), il primo
    // errore IN ORDINE DI RIGA poi, dal collect sequenziale; mai la
    // selezione non deterministica di rayon.
    let groups: Vec<Result<Vec<JoinPair>, SpatialJoinError>> = left
        .par_iter()
        .enumerate()
        .map(|(left_index, left_geometry)| {
            let Some(left_geometry) = *left_geometry else {
                return Ok(Vec::new());
            };
            let Some(envelope) = (if validated {
                envelope_of_validated(left_geometry, "left", left_index)?
            } else {
                checked_envelope(left_geometry, "left", left_index)?
            }) else {
                return Ok(Vec::new());
            };
            // Il limite delle coppie vale per ogni coppia confermata, prima di
            // accodarla al gruppo: una sola geometria sinistra con milioni di
            // coppie deve fallire senza materializzarle tutte.
            //
            // Superato il limite la riga smette di accumulare ma valuta i
            // candidati restanti: quale riga lo supera dipende dai thread, e
            // fermarsi subito salterebbe predicati che potrebbero non
            // concludere, rendendo l'errore finale dipendente dall'ordine.
            let candidati: Vec<usize> = crate::calcolo_protetto(|| {
                tree.locate_in_envelope_intersecting(&envelope)
                    .map(|candidate| candidate.index)
                    .collect()
            })
            .map_err(SpatialJoinError::CalcoloNonConcluso)?;
            let mut right_indexes: Vec<usize> = Vec::new();
            let mut limite_superato = false;
            for candidate in candidati {
                let right_geometry = right[candidate].ok_or(SpatialJoinError::Internal(
                    "R-tree contains only non-null right geometries",
                ))?;
                if exact_match(left_geometry, right_geometry, predicate)? && !limite_superato {
                    let accettata = pair_count
                        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                            current.checked_add(1).filter(|next| *next <= max_pairs)
                        })
                        .is_ok();
                    if accettata {
                        right_indexes.push(candidate);
                    } else {
                        limite_superato = true;
                        right_indexes = Vec::new();
                    }
                }
            }
            if limite_superato {
                return Err(SpatialJoinError::PairLimitExceeded { limit: max_pairs });
            }
            right_indexes.sort_unstable();

            let left = u64::try_from(left_index).map_err(|_| SpatialJoinError::IndexOverflow)?;
            right_indexes
                .into_iter()
                .map(|right_index| {
                    Ok(JoinPair {
                        left,
                        right: u64::try_from(right_index)
                            .map_err(|_| SpatialJoinError::IndexOverflow)?,
                    })
                })
                .collect()
        })
        .collect();
    // Ogni altro errore vince sul limite delle coppie, il primo in ordine di
    // riga: quale riga supera il limite dipende dai thread, che lo superi
    // no (come in `analysis::nearest_matches`).
    let mut pairs = Vec::new();
    let mut limite_superato = None;
    for group in groups {
        match group {
            Ok(group) => pairs.extend(group),
            Err(error @ SpatialJoinError::PairLimitExceeded { .. }) => {
                limite_superato.get_or_insert(error);
            }
            Err(error) => return Err(error),
        }
    }
    limite_superato.map_or(Ok(pairs), Err)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{bowtie, rect};
    use geo::{line_string, Point};
    use proptest::prelude::*;

    fn brute_force(
        left: &[Geometry<f64>],
        right: &[Geometry<f64>],
        predicate: JoinPredicate,
    ) -> Vec<JoinPair> {
        let mut pairs = Vec::new();
        for (left_index, left_geometry) in left.iter().enumerate() {
            for (right_index, right_geometry) in right.iter().enumerate() {
                if exact_match(left_geometry, right_geometry, predicate).expect("predicato") {
                    pairs.push(JoinPair {
                        left: left_index as u64,
                        right: right_index as u64,
                    });
                }
            }
        }
        pairs
    }

    fn rectangle(spec: (i16, i16, u8, u8)) -> Geometry<f64> {
        let (x, y, width, height) = spec;
        let x = f64::from(x);
        let y = f64::from(y);
        let width = f64::from(width.max(1));
        let height = f64::from(height.max(1));
        rect(x, y, x + width, y + height)
    }

    #[test]
    fn join_is_exact_and_deterministically_ordered() {
        let left = vec![
            Geometry::Point(Point::new(2.0, 2.0)),
            Geometry::Point(Point::new(0.5, 0.5)),
        ];
        let right = vec![
            rect(0.0, 0.0, 1.0, 1.0),
            Geometry::LineString(line_string![(x: 0.0, y: 2.0), (x: 3.0, y: 2.0)]),
        ];
        assert_eq!(
            spatial_join(&left, &right, JoinPredicate::Intersects, 10).unwrap(),
            vec![
                JoinPair { left: 0, right: 1 },
                JoinPair { left: 1, right: 0 },
            ]
        );
    }

    #[test]
    fn contains_and_within_have_explicit_direction() {
        let area = rect(0.0, 0.0, 2.0, 2.0);
        let point = Geometry::Point(Point::new(1.0, 1.0));
        assert_eq!(
            spatial_join(
                std::slice::from_ref(&area),
                std::slice::from_ref(&point),
                JoinPredicate::Contains,
                1,
            )
            .unwrap(),
            vec![JoinPair { left: 0, right: 0 }]
        );
        assert_eq!(
            spatial_join(&[point], &[area], JoinPredicate::Within, 1).unwrap(),
            vec![JoinPair { left: 0, right: 0 }]
        );
    }

    #[test]
    fn de9im_predicates_cover_full_manipola_sjoin_contract() {
        let first = rectangle((0, 0, 2, 2));
        let touching = rectangle((2, 0, 2, 2));
        let overlapping = rectangle((1, 1, 2, 2));
        assert!(exact_match(&first, &touching, JoinPredicate::Touches).expect("predicato"));
        assert!(exact_match(&first, &overlapping, JoinPredicate::Overlaps).expect("predicato"));

        let horizontal = Geometry::LineString(line_string![
            (x: -1.0, y: 0.0), (x: 1.0, y: 0.0)
        ]);
        let vertical = Geometry::LineString(line_string![
            (x: 0.0, y: -1.0), (x: 0.0, y: 1.0)
        ]);
        assert!(exact_match(&horizontal, &vertical, JoinPredicate::Crosses).expect("predicato"));
    }

    #[test]
    fn pair_limit_fails_closed() {
        let points = vec![Geometry::Point(Point::new(1.0, 1.0)); 4];
        assert!(matches!(
            spatial_join(&points, &points, JoinPredicate::Intersects, 3),
            Err(SpatialJoinError::PairLimitExceeded { limit: 3 })
        ));
    }

    #[test]
    fn pair_limit_is_enforced_inside_a_single_left_group() {
        let left = vec![Geometry::Point(Point::new(1.0, 1.0))];
        let right = vec![Geometry::Point(Point::new(1.0, 1.0)); 8];
        assert_eq!(
            spatial_join(&left, &right, JoinPredicate::Intersects, 8)
                .unwrap()
                .len(),
            8
        );
        assert!(matches!(
            spatial_join(&left, &right, JoinPredicate::Intersects, 7),
            Err(SpatialJoinError::PairLimitExceeded { limit: 7 })
        ));
    }

    #[test]
    fn rejects_non_finite_envelopes() {
        let invalid = Geometry::Point(Point::new(f64::NAN, 1.0));
        assert!(matches!(
            spatial_join(&[invalid], &[], JoinPredicate::Intersects, 1),
            Err(SpatialJoinError::NonFiniteCoordinate {
                side: "left",
                index: 0
            })
        ));
    }

    #[test]
    fn invalid_topology_and_empty_geometries_are_handled_on_both_sides() {
        let valid = Geometry::Point(Point::new(0.0, 0.0));
        let invalid = bowtie();
        assert!(matches!(
            spatial_join(
                std::slice::from_ref(&invalid),
                std::slice::from_ref(&valid),
                JoinPredicate::Intersects,
                1,
            ),
            Err(SpatialJoinError::InvalidGeometry { side: "left", .. })
        ));
        assert!(matches!(
            spatial_join(
                std::slice::from_ref(&valid),
                std::slice::from_ref(&invalid),
                JoinPredicate::Intersects,
                1,
            ),
            Err(SpatialJoinError::InvalidGeometry { side: "right", .. })
        ));
        let empty = Geometry::GeometryCollection(Vec::<Geometry<f64>>::new().into());
        assert!(spatial_join(
            std::slice::from_ref(&empty),
            std::slice::from_ref(&valid),
            JoinPredicate::Intersects,
            1,
        )
        .unwrap()
        .is_empty());
        assert!(spatial_join(
            std::slice::from_ref(&valid),
            std::slice::from_ref(&empty),
            JoinPredicate::Intersects,
            1,
        )
        .unwrap()
        .is_empty());
    }

    #[test]
    fn nullable_rows_preserve_original_indexes() {
        let left = vec![None, Some(Geometry::Point(Point::new(1.0, 1.0)))];
        let right = vec![
            None,
            Some(Geometry::Point(Point::new(50.0, 50.0))),
            Some(Geometry::Point(Point::new(1.0, 1.0))),
        ];
        assert_eq!(
            spatial_join_nullable(&left, &right, JoinPredicate::Intersects, 10).unwrap(),
            vec![JoinPair { left: 1, right: 2 }]
        );
    }

    #[test]
    fn validated_variants_match_the_gated_path_on_valid_inputs() {
        let left = vec![
            Geometry::Point(Point::new(2.0, 2.0)),
            Geometry::Point(Point::new(0.5, 0.5)),
            rectangle((5, 5, 3, 3)),
        ];
        let right = vec![rectangle((0, 0, 2, 2)), rectangle((1, 1, 8, 8))];
        for predicate in [
            JoinPredicate::Intersects,
            JoinPredicate::Contains,
            JoinPredicate::Within,
            JoinPredicate::Crosses,
            JoinPredicate::Overlaps,
            JoinPredicate::Touches,
        ] {
            assert_eq!(
                spatial_join(&left, &right, predicate, 1_000).unwrap(),
                spatial_join_validated(&left, &right, predicate, 1_000).unwrap(),
                "{predicate:?}"
            );
            let left_nullable: Vec<_> = left.iter().cloned().map(Some).collect();
            let right_nullable: Vec<_> = right.iter().cloned().map(Some).collect();
            assert_eq!(
                spatial_join_nullable(&left_nullable, &right_nullable, predicate, 1_000).unwrap(),
                spatial_join_nullable_validated(&left_nullable, &right_nullable, predicate, 1_000)
                    .unwrap(),
                "{predicate:?}"
            );
        }
        // Limiti e righe vuote: stesso comportamento.
        let points = vec![Geometry::Point(Point::new(1.0, 1.0)); 4];
        assert!(matches!(
            spatial_join_validated(&points, &points, JoinPredicate::Intersects, 3),
            Err(SpatialJoinError::PairLimitExceeded { limit: 3 })
        ));
        let empty = Geometry::GeometryCollection(Vec::<Geometry<f64>>::new().into());
        assert!(spatial_join_validated(
            std::slice::from_ref(&empty),
            std::slice::from_ref(&points[0]),
            JoinPredicate::Intersects,
            1,
        )
        .unwrap()
        .is_empty());
    }

    #[test]
    fn validated_variants_document_the_caller_precondition() {
        // Test di documentazione del contratto, NON un nuovo modo di
        // accettare geometrie invalide in produzione: il percorso gated
        // rifiuta il bowtie (gate intatto), la variante validated lo prende
        // perche' la precondizione e' del chiamante — qui violata ad arte.
        let bowtie = bowtie();
        let valid = Geometry::Point(Point::new(1.0, 1.0));
        assert!(matches!(
            spatial_join(
                std::slice::from_ref(&bowtie),
                std::slice::from_ref(&valid),
                JoinPredicate::Intersects,
                1,
            ),
            Err(SpatialJoinError::InvalidGeometry { side: "left", .. })
        ));
        assert!(spatial_join_validated(
            std::slice::from_ref(&bowtie),
            std::slice::from_ref(&valid),
            JoinPredicate::Intersects,
            1,
        )
        .is_ok());
    }

    proptest! {
        #[test]
        fn indexed_join_matches_exhaustive_reference(
            left_specs in prop::collection::vec(( -100_i16..100, -100_i16..100, 1_u8..20, 1_u8..20), 0..24),
            right_specs in prop::collection::vec(( -100_i16..100, -100_i16..100, 1_u8..20, 1_u8..20), 0..24),
        ) {
            let left: Vec<_> = left_specs.into_iter().map(rectangle).collect();
            let right: Vec<_> = right_specs.into_iter().map(rectangle).collect();
            for predicate in [
                JoinPredicate::Intersects,
                JoinPredicate::Contains,
                JoinPredicate::Within,
                JoinPredicate::Crosses,
                JoinPredicate::Overlaps,
                JoinPredicate::Touches,
            ] {
                let expected = brute_force(&left, &right, predicate);
                let actual = spatial_join(&left, &right, predicate, 10_000).unwrap();
                prop_assert_eq!(actual, expected);
            }
        }
    }
}
