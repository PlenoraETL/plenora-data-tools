//! Le operazioni geo bloccanti: espansioni 1:N, aggregazioni a sole
//! geometrie, operazioni collettive allineate alle righe, coperture e la
//! griglia generativa.
//!
//! Porting delle funzioni `explode_batches`, `collect_batches`,
//! `voronoi_batches`, `clean_topology_batches`, `line_merge_batches` di
//! `plenora-engine/src/geo_transport/unary.rs` e `geo_*_batch` di
//! `executor/geo.rs` a `190c493`, sulle forme del contratto dell'analisi;
//! `make_valid`, `polygonize` e `split` con gli adapter Arrow dei kernel
//! (`rust_backend::arrow`). Una tabella intera per passo: i kernel collettivi
//! vedono tutte le righe insieme, come l'executor d'origine dopo il
//! drenaggio del ramo.

// Singolare e plurale (`cella`/`celle`, `madre`/`madri`) sono i nomi
// giusti, non refusi.
#![allow(clippy::similar_names)]

use std::sync::Arc;

use geo::{Geometry, GeometryCollection};
use plenora_core::arrow::array::{
    Array, ArrayRef, BinaryArray, Float64Array, RecordBatch, StringArray, UInt64Array,
};
use plenora_core::arrow::schema::{DataType, Field, Schema};
use plenora_core::arrow::select::take::take;
use plenora_core::contract::arrow_metadata::MAX_CELL_COORDINATES;
use plenora_core::contract::DataContract;
use plenora_core::{PlenoraError, Result};
use plenora_kernels_geo::analyze::config::{
    CleanTopologyConfig, ClusterDbscanConfig, CollectConfig, CoverageValidateConfig,
    GenerateGridConfig, PolygonizeConfig, SharedPathsConfig, SplitConfig, SubdivideConfig,
    VoronoiConfig,
};
use plenora_kernels_geo::arrow_adapter::{decode_geometry_cell, encode_geometry, map_nullable};
use plenora_kernels_geo::extensions2::{GridExtent, GridShape};
use plenora_kernels_geo::rust_backend::arrow::{
    make_valid_batches, polygonize_batches, split_batches, PolygonizeParams,
};
use plenora_kernels_geo::rust_backend::precision::Precision;
use plenora_kernels_geo::rust_backend::MAX_CLEAN_VERTICES;
use plenora_kernels_geo::{
    advanced, cluster, construction, extended_algorithms, extensions, extensions2, extensions3,
    operations, topology,
};
use serde_json::Value;

use plenora_kernels_geo::margine::MargineMemoria;

use super::errori::del_kernel;

/// I byte di una riga d'uscita di `geo.coverage_validate` oltre la zona
/// (che il kernel conta): tipo (offset e testo), due indici, area, con le
/// validita'.
const BYTE_RIGA_PROBLEMA: u64 = 64;
use super::{binaria, config, sostituisci, Lato};

/// Punti massimi di `voronoi` senza `max_points` in config: il default del
/// trasporto a `190c493` (`DEFAULT_MAX_POINTS`).
const VORONOI_PUNTI_PREDEFINITI: u64 = 100_000;

/// Geometrie massime di `clean_topology`: `MAX_ROWS` del trasporto a
/// `190c493`.
const PULIZIA_GEOMETRIE_MASSIME: u64 = 100_000_000;

/// Espansione 1:N di una riga.
#[derive(Debug)]
pub(super) enum Espansione {
    Esplodi,
    Triangola,
    Suddividi {
        vertici: usize,
        precisione: Precision,
    },
}

/// Il kernel di un'operazione bloccante, con la config letta.
#[derive(Debug)]
pub(super) enum KernelCollettivo {
    /// Righe per parte, attributi della riga madre, `__parent_index`.
    Espandi(Espansione),
    /// `split`: l'adapter dei kernel, con la lama della config su ogni riga.
    Dividi {
        lama: Vec<u8>,
        tolleranza: Option<f64>,
        precisione: Precision,
    },
    /// `make_valid`: l'adapter dei kernel, in place.
    Ripara(Precision),
    /// Una riga con l'aggregato di tutte le geometrie.
    Dissolvi(Precision),
    Linea,
    Poligono,
    /// Una riga per linea fusa.
    FondiLinee,
    /// L'adapter dei kernel: geometria e `__class`.
    Poligonizza(PolygonizeParams, Precision),
    /// Una riga per gruppo: geometria raccolta e colonne chiave.
    Raccogli(Vec<usize>),
    /// Collettive allineate alle righe non-null.
    Voronoi {
        punti: usize,
        precisione: Precision,
    },
    Pulisci {
        aggancio: f64,
        sovrapposizioni: bool,
        buchi: bool,
        precisione: Precision,
    },
    Raggruppa {
        eps: f64,
        punti: usize,
    },
    /// Coperture: schema nuovo, una riga per problema o tratto.
    Copertura {
        tolleranza: f64,
        problemi: usize,
        precisione: Precision,
    },
    TrattiComuni {
        tolleranza: f64,
        lunghezza: f64,
    },
    /// La griglia: l'ingresso fa solo da innesco.
    Griglia {
        estensione: GridExtent,
        lato: f64,
        forma: GridShape,
        centroidi: bool,
    },
}

fn in_usize(op: &str, nome: &str, valore: u64) -> Result<usize> {
    usize::try_from(valore).map_err(|_| {
        PlenoraError::ResourceLimit(format!(
            "{op}: `{nome}` oltre quanto questa piattaforma sa rappresentare"
        ))
    })
}

impl KernelCollettivo {
    /// Il kernel di `op`, se e' un'operazione bloccante unaria.
    #[allow(clippy::too_many_lines)] // Un braccio per operazione.
    pub(super) fn prepara(
        op: &str,
        valore: &Value,
        lati: &[Option<Lato>],
        ingressi: &[DataContract],
    ) -> Result<Option<Self>> {
        let [contratto] = ingressi else {
            return Ok(None);
        };
        if op == "geo.generate_grid" {
            let letta: GenerateGridConfig = config(op, valore)?;
            let estensione = GridExtent::new(
                letta.extent.xmin,
                letta.extent.ymin,
                letta.extent.xmax,
                letta.extent.ymax,
            )
            .map_err(|e| del_kernel(op, &e))?;
            return Ok(Some(Self::Griglia {
                estensione,
                lato: letta.cell_size,
                forma: letta.shape.unwrap_or(GridShape::Square),
                centroidi: letta.include_centroid.unwrap_or(false),
            }));
        }
        let Some(Some(geometria)) = lati.first() else {
            return Ok(None);
        };
        Ok(Some(match op {
            "geo.explode" => Self::Espandi(Espansione::Esplodi),
            "geo.delaunay" => Self::Espandi(Espansione::Triangola),
            "geo.subdivide" => {
                let letta: SubdivideConfig = config(op, valore)?;
                Self::Espandi(Espansione::Suddividi {
                    vertici: letta.max_vertices,
                    precisione: geometria.precisione()?,
                })
            }
            "geo.split" => {
                let letta: SplitConfig = config(op, valore)?;
                Self::Dividi {
                    lama: plenora_kernels_geo::wkb_hex_to_bytes(&letta.other_wkb).ok_or_else(
                        || {
                            PlenoraError::InvalidPlan(format!(
                                "{op}: parametro `other_wkb` non valido"
                            ))
                        },
                    )?,
                    tolleranza: letta.tolerance,
                    precisione: geometria.precisione()?,
                }
            }
            "geo.make_valid" => Self::Ripara(geometria.precisione()?),
            "geo.dissolve" => Self::Dissolvi(geometria.precisione()?),
            "geo.line_builder" => Self::Linea,
            "geo.polygon_builder" => Self::Poligono,
            "geo.line_merge" => Self::FondiLinee,
            "geo.polygonize" => {
                let letta: PolygonizeConfig = config(op, valore)?;
                Self::Poligonizza(
                    PolygonizeParams {
                        node_input: letta.node_input,
                        require_complete: letta.require_complete,
                    },
                    geometria.precisione()?,
                )
            }
            "geo.collect" => {
                let letta: CollectConfig = config(op, valore)?;
                let chiavi = letta
                    .group_by
                    .iter()
                    .map(|nome| {
                        contratto.schema.index_of(nome).map_err(|_| {
                            PlenoraError::Internal(format!(
                                "{op}: colonna chiave assente dopo l'analisi"
                            ))
                        })
                    })
                    .collect::<Result<Vec<_>>>()?;
                Self::Raccogli(chiavi)
            }
            "geo.voronoi" => {
                let letta: VoronoiConfig = config(op, valore)?;
                Self::Voronoi {
                    punti: in_usize(
                        op,
                        "max_points",
                        letta.max_points.unwrap_or(VORONOI_PUNTI_PREDEFINITI),
                    )?,
                    precisione: geometria.precisione()?,
                }
            }
            "geo.clean_topology" => {
                let letta: CleanTopologyConfig = config(op, valore)?;
                Self::Pulisci {
                    aggancio: letta.snap_tolerance,
                    sovrapposizioni: letta.remove_overlaps,
                    buchi: letta.fill_gaps,
                    precisione: geometria.precisione()?,
                }
            }
            "geo.cluster_dbscan" => {
                let letta: ClusterDbscanConfig = config(op, valore)?;
                Self::Raggruppa {
                    eps: letta.eps,
                    punti: letta.min_points,
                }
            }
            "geo.coverage_validate" => {
                let letta: CoverageValidateConfig = config(op, valore)?;
                Self::Copertura {
                    tolleranza: letta.tolerance.unwrap_or(0.0),
                    problemi: letta.max_issues.unwrap_or(extensions3::DEFAULT_MAX_ISSUES),
                    precisione: geometria.precisione()?,
                }
            }
            "geo.shared_paths" => {
                let letta: SharedPathsConfig = config(op, valore)?;
                Self::TrattiComuni {
                    tolleranza: letta.tolerance.unwrap_or(0.0),
                    lunghezza: letta.min_length.unwrap_or(0.0),
                }
            }
            _ => return Ok(None),
        }))
    }

    /// Le colonne dell'uscita, nell'ordine del contratto, e le righe.
    #[allow(clippy::too_many_lines)] // Un braccio per operazione.
    pub(super) fn esegui(
        &self,
        op: &str,
        lato: Option<&Lato>,
        batch: &RecordBatch,
        righe_massime: u64,
        margine: MargineMemoria,
    ) -> Result<(Vec<ArrayRef>, usize)> {
        if let Self::Griglia {
            estensione,
            lato,
            forma,
            centroidi,
        } = self
        {
            return griglia(estensione, *lato, *forma, *centroidi);
        }
        let lato = lato.ok_or_else(|| {
            PlenoraError::Internal(format!("{op}: operazione senza colonna geometria"))
        })?;
        let celle = lato.celle(batch)?;
        let righe = batch.num_rows();
        match self {
            Self::Espandi(espansione) => espandi(op, espansione, lato, batch, righe_massime),
            Self::Dividi {
                lama,
                tolleranza,
                precisione,
            } => {
                let lame: BinaryArray = (0..righe).map(|_| Some(lama.as_slice())).collect();
                let (_, uscite) = split_batches(
                    &batch.schema(),
                    std::slice::from_ref(batch),
                    &lato.nome,
                    &lame,
                    *tolleranza,
                    righe_massime,
                    *precisione,
                )?;
                un_batch(op, &uscite)
            }
            Self::Ripara(precisione) => {
                let uscite = make_valid_batches(
                    &batch.schema(),
                    std::slice::from_ref(batch),
                    &lato.nome,
                    *precisione,
                )?;
                let riparate = uscite
                    .first()
                    .and_then(|uscita| uscita.columns().get(lato.indice))
                    .cloned()
                    .ok_or_else(|| {
                        PlenoraError::Internal(format!("{op}: l'adapter non ha reso la colonna"))
                    })?;
                let mut colonne = batch.columns().to_vec();
                sostituisci(op, &mut colonne, lato.indice, vec![riparate])?;
                Ok((colonne, righe))
            }
            Self::Dissolvi(precisione) => {
                let poligoni: Vec<Geometry<f64>> =
                    decodifica(celle)?.into_iter().flatten().collect();
                let unione = if poligoni.is_empty() {
                    None
                } else {
                    Some(
                        topology::dissolve_validated(&poligoni, *precisione)
                            .map_err(|e| del_kernel(op, &e))?,
                    )
                };
                una_geometria(unione.as_ref())
            }
            Self::Linea => {
                let linea = construction::line_from_ordered_points(&decodifica(celle)?)
                    .map_err(|e| del_kernel(op, &e))?;
                una_geometria(linea.as_ref())
            }
            Self::Poligono => {
                let poligono = construction::polygon_from_ordered_points(&decodifica(celle)?)
                    .map_err(|e| del_kernel(op, &e))?;
                una_geometria(poligono.as_ref())
            }
            Self::FondiLinee => {
                let linee: GeometryCollection<f64> =
                    decodifica(celle)?.into_iter().flatten().collect();
                let fuse = extended_algorithms::line_merge(
                    &Geometry::GeometryCollection(linee),
                    MAX_CLEAN_VERTICES,
                    righe_massime,
                )
                .map_err(|e| del_kernel(op, &e))?;
                let celle = fuse
                    .into_iter()
                    .map(|linea| encode_geometry(&Geometry::LineString(linea)).map(Some))
                    .collect::<Result<Vec<_>>>()?;
                let righe = celle.len();
                Ok((vec![binaria(&celle)], righe))
            }
            Self::Poligonizza(parametri, precisione) => {
                let (_, uscite) = polygonize_batches(
                    &batch.schema(),
                    std::slice::from_ref(batch),
                    &lato.nome,
                    *parametri,
                    righe_massime,
                    *precisione,
                )?;
                un_batch(op, &uscite)
            }
            Self::Raccogli(chiavi) => raccogli(op, celle, batch, chiavi),
            Self::Voronoi { punti, precisione } => {
                allineate_alle_righe(op, lato, batch, |geometrie| {
                    advanced::voronoi_cells(geometrie, *punti, *precisione)
                        .map(|celle| celle.into_iter().map(Some).collect())
                        .map_err(|e| del_kernel(op, &e))
                })
            }
            Self::Pulisci {
                aggancio,
                sovrapposizioni,
                buchi,
                precisione,
            } => allineate_alle_righe(op, lato, batch, |geometrie| {
                topology::clean_valid_polygon_topology_validated(
                    geometrie,
                    *aggancio,
                    *sovrapposizioni,
                    *buchi,
                    PULIZIA_GEOMETRIE_MASSIME,
                    MAX_CLEAN_VERTICES,
                    *precisione,
                )
                .map_err(|e| del_kernel(op, &e))
            }),
            Self::Raggruppa { eps, punti } => {
                let etichette = cluster::dbscan_column(celle, *eps, *punti)?;
                let mut colonne = batch.columns().to_vec();
                colonne.push(Arc::new(UInt64Array::from(etichette)));
                Ok((colonne, righe))
            }
            Self::Copertura {
                tolleranza,
                problemi,
                precisione,
            } => {
                // Una riga d'uscita per problema: tipo, due indici, area.
                let problemi = extensions3::coverage_validate_rows_con_margine(
                    celle,
                    *tolleranza,
                    *problemi,
                    *precisione,
                    margine.con_uscita_per_risultato(BYTE_RIGA_PROBLEMA),
                )?;
                let righe = problemi.len();
                Ok((
                    vec![
                        Arc::new(StringArray::from(
                            problemi.iter().map(|p| p.issue_type).collect::<Vec<_>>(),
                        )),
                        Arc::new(UInt64Array::from_iter_values(
                            problemi.iter().map(|p| p.index_a),
                        )),
                        Arc::new(UInt64Array::from_iter_values(
                            problemi.iter().map(|p| p.index_b),
                        )),
                        Arc::new(Float64Array::from_iter_values(
                            problemi.iter().map(|p| p.area),
                        )),
                        Arc::new(
                            problemi
                                .iter()
                                .map(|p| Some(p.wkb.as_slice()))
                                .collect::<BinaryArray>(),
                        ),
                    ],
                    righe,
                ))
            }
            Self::TrattiComuni {
                tolleranza,
                lunghezza,
            } => {
                let tratti = extensions3::shared_paths_rows(celle, *tolleranza, *lunghezza)?;
                let righe = tratti.len();
                Ok((
                    vec![
                        Arc::new(UInt64Array::from_iter_values(
                            tratti.iter().map(|t| t.index_a),
                        )),
                        Arc::new(UInt64Array::from_iter_values(
                            tratti.iter().map(|t| t.index_b),
                        )),
                        Arc::new(Float64Array::from_iter_values(
                            tratti.iter().map(|t| t.shared_length),
                        )),
                        Arc::new(
                            tratti
                                .iter()
                                .map(|t| Some(t.wkb.as_slice()))
                                .collect::<BinaryArray>(),
                        ),
                    ],
                    righe,
                ))
            }
            Self::Griglia { .. } => Err(PlenoraError::Internal(format!(
                "{op}: griglia sul percorso delle geometrie"
            ))),
        }
    }
}

/// Tutte le celle decodificate e validate (OGC), null compresi.
pub(super) fn decodifica(celle: &BinaryArray) -> Result<Vec<Option<Geometry<f64>>>> {
    map_nullable(celle, |cella| decode_geometry_cell(cella).map(Some))
}

/// Una riga con una sola colonna geometria (null se non c'e').
fn una_geometria(geometria: Option<&Geometry<f64>>) -> Result<(Vec<ArrayRef>, usize)> {
    let cella = geometria.map(encode_geometry).transpose()?;
    Ok((vec![binaria(&[cella])], 1))
}

/// Le colonne dell'unico batch reso da un adapter dei kernel.
fn un_batch(op: &str, uscite: &[RecordBatch]) -> Result<(Vec<ArrayRef>, usize)> {
    let [uscita] = uscite else {
        return Err(PlenoraError::Internal(format!(
            "{op}: l'adapter non ha reso un solo batch"
        )));
    };
    Ok((uscita.columns().to_vec(), uscita.num_rows()))
}

/// Espansione 1:N: una riga per parte con gli attributi della riga madre e
/// il suo indice in `__parent_index`. Come a `190c493`: `explode` e
/// `delaunay` saltano le geometrie null (nessuna parte), `subdivide` le
/// tiene come una riga con geometria null.
fn espandi(
    op: &str,
    espansione: &Espansione,
    lato: &Lato,
    batch: &RecordBatch,
    righe_massime: u64,
) -> Result<(Vec<ArrayRef>, usize)> {
    let parti = map_nullable(lato.celle(batch)?, |cella| {
        let parti: Vec<Vec<u8>> = match espansione {
            Espansione::Suddividi {
                vertici,
                precisione,
            } => extensions2::subdivide_wkb(cella, *vertici, *precisione)?,
            Espansione::Esplodi => {
                let geometria = plenora_kernels_geo::wkb_decoder::decode_validated(cella)?;
                operations::explode(&geometria)
                    .map_err(|e| del_kernel(op, &e))?
                    .iter()
                    .map(encode_geometry)
                    .collect::<Result<_>>()?
            }
            Espansione::Triangola => {
                let geometria = plenora_kernels_geo::wkb_decoder::decode_validated(cella)?;
                extended_algorithms::delaunay(&geometria, MAX_CELL_COORDINATES, righe_massime)
                    .map_err(|e| del_kernel(op, &e))?
                    .into_iter()
                    .map(|triangolo| encode_geometry(&Geometry::Polygon(triangolo)))
                    .collect::<Result<_>>()?
            }
        };
        Ok(Some(parti))
    })?;
    let tiene_i_null = matches!(espansione, Espansione::Suddividi { .. });
    let mut madri: Vec<u64> = Vec::new();
    let mut celle: Vec<Option<Vec<u8>>> = Vec::new();
    for (riga, parti) in parti.into_iter().enumerate() {
        let madre = u64::try_from(riga)
            .map_err(|_| PlenoraError::Internal(format!("{op}: riga non rappresentabile")))?;
        match parti {
            Some(parti) => {
                for parte in parti {
                    madri.push(madre);
                    celle.push(Some(parte));
                }
            }
            None if tiene_i_null => {
                madri.push(madre);
                celle.push(None);
            }
            None => {}
        }
        let prodotte = u64::try_from(celle.len())
            .map_err(|_| PlenoraError::Internal(format!("{op}: righe non rappresentabili")))?;
        if prodotte > righe_massime {
            return Err(PlenoraError::ResourceLimit(format!(
                "{op}: righe prodotte oltre il limite {righe_massime} dell'arco"
            )));
        }
    }
    let indici = UInt64Array::from(madri);
    let mut colonne = Vec::with_capacity(batch.num_columns() + 1);
    for (indice, colonna) in batch.columns().iter().enumerate() {
        if indice == lato.indice {
            colonne.push(binaria(&celle));
        } else {
            colonne.push(take(colonna.as_ref(), &indici, None)?);
        }
    }
    let righe = indici.len();
    colonne.push(Arc::new(indici));
    Ok((colonne, righe))
}

/// Un kernel collettivo sulle geometrie non-null, con un risultato per
/// ciascuna, riportato alla sua riga; le righe null restano null e gli
/// attributi invariati.
fn allineate_alle_righe(
    op: &str,
    lato: &Lato,
    batch: &RecordBatch,
    kernel: impl FnOnce(&[Geometry<f64>]) -> Result<Vec<Option<Geometry<f64>>>>,
) -> Result<(Vec<ArrayRef>, usize)> {
    let tutte = decodifica(lato.celle(batch)?)?;
    let mut posizioni = Vec::new();
    let mut presenti = Vec::new();
    for (riga, geometria) in tutte.into_iter().enumerate() {
        if let Some(geometria) = geometria {
            posizioni.push(riga);
            presenti.push(geometria);
        }
    }
    let risultati = kernel(&presenti)?;
    if risultati.len() != posizioni.len() {
        return Err(PlenoraError::Internal(format!(
            "{op}: il kernel non ha reso un risultato per ogni riga non nulla"
        )));
    }
    let mut celle: Vec<Option<Vec<u8>>> = vec![None; batch.num_rows()];
    for (riga, risultato) in posizioni.into_iter().zip(risultati) {
        let cella = celle
            .get_mut(riga)
            .ok_or_else(|| PlenoraError::Internal(format!("{op}: posizione oltre le righe")))?;
        *cella = risultato.as_ref().map(encode_geometry).transpose()?;
    }
    let mut colonne = batch.columns().to_vec();
    sostituisci(op, &mut colonne, lato.indice, vec![binaria(&celle)])?;
    Ok((colonne, batch.num_rows()))
}

/// `collect`: gruppi nell'**ordine naturale dei valori tipizzati** delle
/// chiavi, dalla prima: il confronto di `table.sort`
/// (`compare_cells_typed` dei kernel tabellari: numeri per valore, testo per
/// byte, date e istanti per istante, `Float64` con `total_cmp`, null dopo i
/// valori). Le geometrie del gruppo restano in ordine d'ingresso; per ogni
/// gruppo la geometria raccolta e i valori chiave della prima riga.
///
/// A `190c493` l'ordine era quello di una chiave testuale con la lunghezza
/// del valore scritta in testa come testo: `"10"` prima di `"9"`, ma anche
/// un valore di 10 caratteri prima di uno di 9.
///
/// La permutazione e' quella di `table.sort` (stabile, crescente) su una
/// tabella delle sole chiavi con l'indice di riga in coda: le righe di uno
/// stesso gruppo restano nell'ordine d'ingresso, e il gruppo cambia dove
/// una chiave differisce per lo stesso comparatore.
fn raccogli(
    op: &str,
    celle: &BinaryArray,
    batch: &RecordBatch,
    chiavi: &[usize],
) -> Result<(Vec<ArrayRef>, usize)> {
    let geometrie = decodifica(celle)?;
    let gruppi = gruppi_in_ordine(op, batch, chiavi)?;
    let mut raccolte: Vec<Option<Vec<u8>>> = Vec::with_capacity(gruppi.len());
    let mut rappresentanti: Vec<u64> = Vec::with_capacity(gruppi.len());
    for righe in &gruppi {
        let gruppo: Vec<Option<Geometry<f64>>> = righe
            .iter()
            .map(|&riga| geometrie.get(riga).cloned().flatten())
            .collect();
        let raccolta = extensions::collect_geometries(&gruppo).map_err(|e| del_kernel(op, &e))?;
        raccolte.push(raccolta.as_ref().map(encode_geometry).transpose()?);
        let prima = righe
            .first()
            .copied()
            .ok_or_else(|| PlenoraError::Internal(format!("{op}: gruppo senza righe")))?;
        rappresentanti.push(
            u64::try_from(prima)
                .map_err(|_| PlenoraError::Internal(format!("{op}: riga non rappresentabile")))?,
        );
    }
    let indici = UInt64Array::from(rappresentanti);
    let mut colonne: Vec<ArrayRef> = Vec::with_capacity(chiavi.len() + 1);
    colonne.push(binaria(&raccolte));
    for &indice in chiavi {
        let colonna = batch.columns().get(indice).ok_or_else(|| {
            PlenoraError::Internal(format!("{op}: colonna chiave oltre lo schema"))
        })?;
        colonne.push(take(colonna.as_ref(), &indici, None)?);
    }
    Ok((colonne, indici.len()))
}

/// I gruppi di `collect` in ordine naturale delle chiavi, ognuno con le sue
/// righe in ordine d'ingresso.
fn gruppi_in_ordine(op: &str, batch: &RecordBatch, chiavi: &[usize]) -> Result<Vec<Vec<usize>>> {
    const INDICE: &str = "__plenora_riga";
    let mut campi = Vec::with_capacity(chiavi.len() + 1);
    let mut colonne: Vec<ArrayRef> = Vec::with_capacity(chiavi.len() + 1);
    let mut nomi = Vec::with_capacity(chiavi.len());
    for (posizione, &indice) in chiavi.iter().enumerate() {
        let colonna = batch.columns().get(indice).ok_or_else(|| {
            PlenoraError::Internal(format!("{op}: colonna chiave oltre lo schema"))
        })?;
        // Nomi posizionali: nessuna collisione con l'indice ne' fra chiavi.
        let nome = format!("k{posizione}");
        campi.push(Field::new(&nome, colonna.data_type().clone(), true));
        colonne.push(Arc::clone(colonna));
        nomi.push(nome);
    }
    let righe = u64::try_from(batch.num_rows())
        .map_err(|_| PlenoraError::Internal(format!("{op}: righe non rappresentabili")))?;
    campi.push(Field::new(INDICE, DataType::UInt64, false));
    colonne.push(Arc::new(UInt64Array::from_iter_values(0..righe)));
    let chiavi_e_indice =
        plenora_core::batch_with_rows(Arc::new(Schema::new(campi)), colonne, batch.num_rows())?;
    let ordinata = plenora_kernels_table::aggregation::sort(
        &chiavi_e_indice,
        &plenora_kernels_table::aggregation::Sort {
            columns: nomi,
            ascending: true,
        },
    )?;
    let indici = ordinata
        .column(chiavi.len())
        .as_any()
        .downcast_ref::<UInt64Array>()
        .ok_or_else(|| PlenoraError::Internal(format!("{op}: indice di riga non UInt64")))?;
    let mut gruppi: Vec<Vec<usize>> = Vec::new();
    for posizione in 0..ordinata.num_rows() {
        let riga = usize::try_from(indici.value(posizione))
            .map_err(|_| PlenoraError::Internal(format!("{op}: riga non rappresentabile")))?;
        let stesso_gruppo = match posizione.checked_sub(1) {
            None => false,
            Some(precedente) => {
                let mut uguali = true;
                for colonna in &ordinata.columns()[..chiavi.len()] {
                    if plenora_kernels_table::aggregation::compare_cells_typed(
                        colonna, precedente, colonna, posizione,
                    )? != std::cmp::Ordering::Equal
                    {
                        uguali = false;
                        break;
                    }
                }
                uguali
            }
        };
        match gruppi.last_mut() {
            Some(gruppo) if stesso_gruppo => gruppo.push(riga),
            _ => gruppi.push(vec![riga]),
        }
    }
    Ok(gruppi)
}

/// `generate_grid`: geometria, `cell_i`, `cell_j` e, se chiesti, i
/// centroidi; l'ingresso fa solo da innesco (come a `190c493`).
fn griglia(
    estensione: &GridExtent,
    lato: f64,
    forma: GridShape,
    centroidi: bool,
) -> Result<(Vec<ArrayRef>, usize)> {
    let celle = extensions2::generate_grid_rows(estensione, lato, forma)?;
    let mut colonne: Vec<ArrayRef> = vec![
        Arc::new(
            celle
                .iter()
                .map(|cella| Some(cella.wkb.as_slice()))
                .collect::<BinaryArray>(),
        ),
        Arc::new(UInt64Array::from_iter_values(
            celle.iter().map(|c| c.cell_i),
        )),
        Arc::new(UInt64Array::from_iter_values(
            celle.iter().map(|c| c.cell_j),
        )),
    ];
    if centroidi {
        colonne.push(Arc::new(Float64Array::from_iter_values(
            celle.iter().map(|c| c.centroid_x),
        )));
        colonne.push(Arc::new(Float64Array::from_iter_values(
            celle.iter().map(|c| c.centroid_y),
        )));
    }
    Ok((colonne, celle.len()))
}
