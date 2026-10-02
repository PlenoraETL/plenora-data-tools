//! Le operazioni geo su due tabelle (left, right).
//!
//! Semantica delle righe come a `190c493` (`execute_geo_binary` di
//! `executor/geo.rs` per i join, `pair_arrow` di `geo_transport/pair.rs`
//! per ritaglio, overlay e booleane), sulle forme del contratto dell'analisi
//! (docs/runner.md, «Operazioni geo»):
//!
//! - `sjoin`, `nearest`: una riga per coppia trovata, le colonne di left
//!   della sua riga e `__right_index` (con `distance` per `nearest`);
//! - `within`, `count_points_in_polygons`: allineate a left, una colonna in
//!   coda (null per una geometria left null);
//! - `clip`: allineata a left, ogni geometria ritagliata dall'unione di
//!   tutte le geometrie di right (la maschera); null dove il ritaglio e'
//!   vuoto;
//! - `overlay`: una riga per pezzo, geometria e indici di riga dei due lati;
//! - `intersection`, `union`, `difference`, `symmetric_difference`: riga
//!   `i` di left con riga `i` di right, stesse righe richieste; null dove
//!   uno dei due e' null o il risultato e' vuoto.
//!
//! Le geometrie dei due lati si decodificano e si validano (OGC) una volta,
//! poi i kernel `*_validated`.

// `_sx`/`_dx` e singolare/plurale sono coppie volute.
#![allow(clippy::similar_names)]

use std::collections::HashSet;
use std::sync::Arc;

use geo::{CoordsIter, Geometry};
use plenora_core::arrow::array::cast::AsArray;
use plenora_core::arrow::array::Array;
use plenora_core::arrow::array::{
    ArrayRef, BinaryArray, BooleanArray, Float64Array, RecordBatch, UInt64Array,
};
use plenora_core::arrow::select::take::take;
use plenora_core::arrow::DataType;
use plenora_core::limits::Limits;
use plenora_core::{PlenoraError, Result};
use plenora_kernels_geo::analyze::config::{NearestConfig, OverlayConfig, SJoinConfig};
use plenora_kernels_geo::arrow_adapter::encode_geometry;
use plenora_kernels_geo::decoded_size::decoded_size_xy;
use plenora_kernels_geo::margine::{byte_heap_geometria, MargineMemoria};
use plenora_kernels_geo::rust_backend::precision::Precision;
use plenora_kernels_geo::spatial_join::JoinPredicate;
use plenora_kernels_geo::topology::{BooleanOperation, OverlayMode};
use plenora_kernels_geo::{analysis, spatial_join, topology};
use serde_json::Value;

use super::collettivi::decodifica;
use super::errori::del_kernel;
use super::{binaria, config, sostituisci, Lato};

/// Il kernel di un'operazione binaria, con la config letta.
#[derive(Debug)]
pub(super) enum KernelBinario {
    Unione(JoinPredicate),
    PiuVicino {
        distanza: Option<f64>,
        confronti: u64,
    },
    Dentro,
    Conta,
    Ritaglia(Precision),
    Sovrapponi(OverlayMode, Precision),
    Booleana(BooleanOperation, Precision),
}

impl KernelBinario {
    /// Il kernel di `op`, se e' un'operazione binaria geo.
    pub(super) fn prepara(
        op: &str,
        valore: &Value,
        lati: &[Option<Lato>],
        limiti: &Limits,
    ) -> Result<Option<Self>> {
        let [Some(sinistra), Some(_)] = lati else {
            return Ok(None);
        };
        let booleana =
            |operazione| -> Result<Self> { Ok(Self::Booleana(operazione, sinistra.precisione()?)) };
        Ok(Some(match op {
            "geo.sjoin" => {
                let letta: SJoinConfig = config(op, valore)?;
                Self::Unione(letta.predicate)
            }
            "geo.nearest" => {
                let letta: NearestConfig = config(op, valore)?;
                // Come l'engine di `190c493`: il quadrato del massimo fra
                // `max_input_rows` e `max_rows_per_edge`.
                let tetto = limiti
                    .rows
                    .max_input_rows
                    .max(limiti.rows.max_rows_per_edge);
                Self::PiuVicino {
                    distanza: letta.max_distance,
                    confronti: tetto.saturating_mul(tetto),
                }
            }
            // `output_column` e' gia' nel contratto.
            "geo.within" => Self::Dentro,
            "geo.count_points_in_polygons" => Self::Conta,
            "geo.clip" => Self::Ritaglia(sinistra.precisione()?),
            "geo.overlay" => {
                let letta: OverlayConfig = config(op, valore)?;
                Self::Sovrapponi(letta.mode, sinistra.precisione()?)
            }
            "geo.intersection" => booleana(BooleanOperation::Intersection)?,
            "geo.union" => booleana(BooleanOperation::Union)?,
            "geo.difference" => booleana(BooleanOperation::Difference)?,
            "geo.symmetric_difference" => booleana(BooleanOperation::SymmetricDifference)?,
            _ => return Ok(None),
        }))
    }

    /// Le colonne dell'uscita, nell'ordine del contratto, e le righe.
    #[allow(clippy::too_many_lines)] // Un braccio per operazione.
    pub(super) fn esegui(
        &self,
        op: &str,
        lati: (&Lato, &Lato),
        tabelle: (&RecordBatch, &RecordBatch),
        righe_massime: u64,
        margine: MargineMemoria,
    ) -> Result<(Vec<ArrayRef>, usize)> {
        let (lato_sx, lato_dx) = lati;
        let (sinistra, destra) = tabelle;
        // Le uscite a una riga per coppia ripetono una riga di left: conta
        // nel margine la riga piu' larga (un maggiorante di ogni riga
        // ripetuta), con gli indici.
        let riga_sinistra = byte_riga_massima(sinistra);
        // Prima di decodificare: le geometrie dei due lati devono stare nel
        // margine, a giudicare dalle intestazioni del WKB.
        let (celle_sx, celle_dx) = (lato_sx.celle(sinistra)?, lato_dx.celle(destra)?);
        margine
            .verifica(
                byte_decodificate_previste(celle_sx)
                    .saturating_add(byte_decodificate_previste(celle_dx)),
            )
            .map_err(|superato| PlenoraError::ResourceLimit(format!("{op}: {superato}")))?;
        let geometrie_sx = decodifica(celle_sx)?;
        let geometrie_dx = decodifica(celle_dx)?;
        // Le geometrie decodificate restano vive per tutto il kernel: il
        // margine dei kernel e' quello che resta.
        let decodificate =
            [&geometrie_sx, &geometrie_dx]
                .into_iter()
                .fold(0_u64, |totale, geometrie| {
                    geometrie.iter().fold(
                        totale.saturating_add(
                            u64::try_from(
                                geometrie
                                    .capacity()
                                    .saturating_mul(std::mem::size_of::<Option<Geometry<f64>>>()),
                            )
                            .unwrap_or(u64::MAX),
                        ),
                        |totale, geometria| {
                            totale.saturating_add(geometria.as_ref().map_or(0, byte_heap_geometria))
                        },
                    )
                });
        margine
            .verifica(decodificate)
            .map_err(|superato| PlenoraError::ResourceLimit(format!("{op}: {superato}")))?;
        let margine = margine.con_byte(margine.byte_disponibili() - decodificate);
        let righe_sx = sinistra.num_rows();
        match self {
            Self::Unione(predicato) => {
                let coppie = spatial_join::spatial_join_nullable_validated_con_margine(
                    &geometrie_sx,
                    &geometrie_dx,
                    *predicato,
                    righe_massime,
                    margine.con_uscita_per_risultato(riga_sinistra.saturating_add(16)),
                )
                .map_err(|e| del_kernel(op, &e))?;
                let indici = UInt64Array::from_iter_values(coppie.iter().map(|c| c.left));
                let mut colonne = righe_di(sinistra, &indici)?;
                colonne.push(Arc::new(UInt64Array::from_iter_values(
                    coppie.iter().map(|c| c.right),
                )));
                Ok((colonne, indici.len()))
            }
            Self::PiuVicino {
                distanza,
                confronti,
            } => {
                let trovati = analysis::nearest_matches_validated_con_margine(
                    &geometrie_sx,
                    &geometrie_dx,
                    *distanza,
                    (*confronti, righe_massime),
                    margine.con_uscita_per_risultato(riga_sinistra.saturating_add(24)),
                )
                .map_err(|e| del_kernel(op, &e))?;
                let indici = UInt64Array::from_iter_values(trovati.iter().map(|m| m.left));
                let mut colonne = righe_di(sinistra, &indici)?;
                colonne.push(Arc::new(UInt64Array::from_iter_values(
                    trovati.iter().map(|m| m.right),
                )));
                colonne.push(Arc::new(Float64Array::from_iter_values(
                    trovati.iter().map(|m| m.distance),
                )));
                Ok((colonne, indici.len()))
            }
            Self::Dentro => {
                let dentro: HashSet<u64> = analysis::within_indexes_validated_con_margine(
                    &geometrie_sx,
                    &geometrie_dx,
                    righe_massime,
                    margine,
                )
                .map_err(|e| del_kernel(op, &e))?
                .into_iter()
                .collect();
                let mut indici = 0_u64..;
                let valori: BooleanArray = geometrie_sx
                    .iter()
                    .map(|g| {
                        let riga = indici.next().unwrap_or(u64::MAX);
                        g.as_ref().map(|_| dentro.contains(&riga))
                    })
                    .collect();
                let mut colonne = sinistra.columns().to_vec();
                colonne.push(Arc::new(valori));
                Ok((colonne, righe_sx))
            }
            Self::Conta => {
                let conteggi = analysis::count_points_in_polygons_validated_con_margine(
                    &geometrie_sx,
                    &geometrie_dx,
                    righe_massime,
                    margine,
                )
                .map_err(|e| del_kernel(op, &e))?;
                // Un conteggio per riga di left, verificato prima di
                // accoppiarli (come a 190c493).
                if conteggi.len() != geometrie_sx.len() {
                    return Err(PlenoraError::Internal(format!(
                        "{op}: il kernel non ha reso un conteggio per ogni riga di left"
                    )));
                }
                let valori: UInt64Array = geometrie_sx
                    .iter()
                    .zip(&conteggi)
                    .map(|(g, conteggio)| g.as_ref().map(|_| *conteggio))
                    .collect();
                let mut colonne = sinistra.columns().to_vec();
                colonne.push(Arc::new(valori));
                Ok((colonne, righe_sx))
            }
            Self::Ritaglia(precisione) => {
                let maschere: Vec<Geometry<f64>> = geometrie_dx.into_iter().flatten().collect();
                let (posizioni, presenti) = presenti(geometrie_sx);
                let ritagliate =
                    topology::clip_to_mask_validated(&presenti, &maschere, *precisione)
                        .map_err(|e| del_kernel(op, &e))?;
                if ritagliate.len() != posizioni.len() {
                    return Err(PlenoraError::Internal(format!(
                        "{op}: il kernel non ha reso un risultato per ogni riga non nulla"
                    )));
                }
                let mut celle: Vec<Option<Vec<u8>>> = vec![None; righe_sx];
                for (riga, ritagliata) in posizioni.into_iter().zip(ritagliate) {
                    let cella = celle.get_mut(riga).ok_or_else(|| {
                        PlenoraError::Internal(format!("{op}: posizione oltre le righe"))
                    })?;
                    *cella = ritagliata.as_ref().map(encode_geometry).transpose()?;
                }
                let mut colonne = sinistra.columns().to_vec();
                sostituisci(op, &mut colonne, lato_sx.indice, vec![binaria(&celle)])?;
                Ok((colonne, righe_sx))
            }
            Self::Sovrapponi(modo, precisione) => {
                let (posizioni_sx, presenti_sx) = presenti(geometrie_sx);
                let (posizioni_dx, presenti_dx) = presenti(geometrie_dx);
                // Per pezzo, oltre alla geometria che il kernel conta: i due
                // indici con le validita'.
                let pezzi = topology::polygon_overlay_validated_con_margine(
                    &presenti_sx,
                    &presenti_dx,
                    *modo,
                    (righe_massime, righe_massime),
                    *precisione,
                    margine.con_uscita_per_risultato(24),
                )
                .map_err(|e| del_kernel(op, &e))?;
                let riga_di = |posizioni: &[usize], indice: Option<u64>| -> Result<Option<u64>> {
                    indice
                        .map(|indice| {
                            usize::try_from(indice)
                                .ok()
                                .and_then(|indice| posizioni.get(indice))
                                .and_then(|riga| u64::try_from(*riga).ok())
                                .ok_or_else(|| {
                                    PlenoraError::Internal(format!(
                                        "{op}: indice di un pezzo oltre le righe"
                                    ))
                                })
                        })
                        .transpose()
                };
                let mut celle = Vec::with_capacity(pezzi.len());
                let mut indici_sx = Vec::with_capacity(pezzi.len());
                let mut indici_dx = Vec::with_capacity(pezzi.len());
                for pezzo in &pezzi {
                    celle.push(Some(encode_geometry(&pezzo.geometry)?));
                    indici_sx.push(riga_di(&posizioni_sx, pezzo.left)?);
                    indici_dx.push(riga_di(&posizioni_dx, pezzo.right)?);
                }
                Ok((
                    vec![
                        binaria(&celle),
                        Arc::new(UInt64Array::from(indici_sx)),
                        Arc::new(UInt64Array::from(indici_dx)),
                    ],
                    pezzi.len(),
                ))
            }
            Self::Booleana(operazione, precisione) => {
                if righe_sx != destra.num_rows() {
                    return Err(PlenoraError::InvalidPlan(format!(
                        "{op}: le due tabelle devono avere le stesse righe (riga i con riga i)"
                    )));
                }
                let risultati = geometrie_sx
                    .iter()
                    .zip(&geometrie_dx)
                    .map(|coppia| match coppia {
                        (Some(a), Some(b)) => {
                            let risultato = topology::boolean_operation_validated(
                                a,
                                b,
                                *operazione,
                                *precisione,
                            )
                            .map_err(|e| del_kernel(op, &e))?;
                            // Vuoto -> null, come `clip`.
                            if risultato.coords_count() == 0 {
                                Ok(None)
                            } else {
                                encode_geometry(&risultato).map(Some)
                            }
                        }
                        _ => Ok(None),
                    })
                    .collect::<Result<Vec<_>>>()?;
                let mut colonne = sinistra.columns().to_vec();
                sostituisci(op, &mut colonne, lato_sx.indice, vec![binaria(&risultati)])?;
                Ok((colonne, righe_sx))
            }
        }
    }
}

/// Un maggiorante dei byte che `take` alloca per **una qualunque** riga di
/// `tabella`, colonna per colonna: per i binari e i testi il valore piu'
/// lungo piu' l'offset, per i tipi a larghezza fissa la larghezza, piu' un
/// byte di validita' (per eccesso sul bit); per ogni altro tipo (liste,
/// struct, dizionari) i byte dell'intera colonna, che nessuna riga supera.
/// Mai la media: una riga enorme ripetuta migliaia di volte costerebbe ben
/// oltre.
fn byte_riga_massima(tabella: &RecordBatch) -> u64 {
    tabella.columns().iter().fold(0_u64, |totale, colonna| {
        totale.saturating_add(byte_riga_massima_colonna(colonna.as_ref()))
    })
}

fn byte_riga_massima_colonna(colonna: &dyn Array) -> u64 {
    let piu_lungo = |lunghezze: &mut dyn Iterator<Item = usize>| {
        u64::try_from(lunghezze.max().unwrap_or(0)).unwrap_or(u64::MAX)
    };
    let valore = match colonna.data_type() {
        DataType::Binary => piu_lungo(&mut colonna.as_binary::<i32>().offsets().lengths()) + 4,
        DataType::LargeBinary => piu_lungo(&mut colonna.as_binary::<i64>().offsets().lengths()) + 8,
        DataType::Utf8 => piu_lungo(&mut colonna.as_string::<i32>().offsets().lengths()) + 4,
        DataType::LargeUtf8 => piu_lungo(&mut colonna.as_string::<i64>().offsets().lengths()) + 8,
        DataType::Boolean => 1,
        tipo => tipo.primitive_width().map_or_else(
            || u64::try_from(colonna.get_array_memory_size()).unwrap_or(u64::MAX),
            |larghezza| u64::try_from(larghezza).unwrap_or(u64::MAX),
        ),
    };
    valore.saturating_add(1)
}

/// I byte delle geometrie decodificate di una colonna, dalle intestazioni
/// del WKB e senza decodificare: il posto di ogni riga piu' l'heap che il
/// decoder costruirebbe ([`decoded_size_xy`], esatto per il nostro
/// decoder). Una cella che la camminata rifiuta conta solo il suo posto: il
/// decoder la rifiutera'.
fn byte_decodificate_previste(celle: &BinaryArray) -> u64 {
    let posto = std::mem::size_of::<Option<Geometry<f64>>>() as u64;
    (0..celle.len()).fold(0_u64, |totale, riga| {
        let heap = if celle.is_null(riga) {
            0
        } else {
            decoded_size_xy(celle.value(riga)).unwrap_or(0)
        };
        totale.saturating_add(posto).saturating_add(heap)
    })
}

/// Le colonne di left, riga per indice.
fn righe_di(sinistra: &RecordBatch, indici: &UInt64Array) -> Result<Vec<ArrayRef>> {
    sinistra
        .columns()
        .iter()
        .map(|colonna| take(colonna.as_ref(), indici, None).map_err(PlenoraError::from))
        .collect()
}

/// Le geometrie non-null e la loro riga.
fn presenti(geometrie: Vec<Option<Geometry<f64>>>) -> (Vec<usize>, Vec<Geometry<f64>>) {
    geometrie
        .into_iter()
        .enumerate()
        .filter_map(|(riga, geometria)| geometria.map(|geometria| (riga, geometria)))
        .unzip()
}
