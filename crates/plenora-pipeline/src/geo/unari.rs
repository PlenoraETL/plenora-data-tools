//! Le operazioni geo 1:1 sulle righe: trasformazioni in place, misure,
//! accessori, distanze e predicati contro una geometria della config,
//! diagnostica, `reproject`, `snap`, e i produttori `from_coords` e
//! `from_wkt`.
//!
//! Porting dei bracci per cella di `transform_cells` e `apply_transform_cell`
//! (`plenora-engine/src/geo_transport/unary.rs`) e delle funzioni
//! `geo_*_batch` (`executor/geo.rs`) a `190c493`, sulle forme del contratto
//! dell'analisi: le misure **aggiungono** una colonna (il trasporto legacy
//! sostituiva la geometria), la diagnostica sostituisce la geometria con le
//! sue dieci colonne. Ogni cella si decodifica (struttura WKB, dominio del
//! CRS) e passa al kernel che valida la geometria: la validazione OGC e'
//! una per geometria. Righe indipendenti: `map_nullable` le calcola in
//! parallelo e rende il primo errore in ordine di riga.

use std::sync::Arc;

use geo::{Geometry, LineString, Point};
use plenora_core::arrow::array::{
    Array, ArrayRef, BooleanArray, Float64Array, Int64Array, RecordBatch, StringArray, UInt64Array,
};
use plenora_core::arrow::DataType;
use plenora_core::contract::arrow_metadata::MAX_CELL_COORDINATES;
use plenora_core::contract::DataContract;
use plenora_core::crs::ResolvedCrs;
use plenora_core::{PlenoraError, Result};
use plenora_kernels_geo::analyze::config::{
    AccessorFieldParam, AffineTransformConfig, BufferCapParam, BufferConfig, ConcaveHullConfig,
    DensifyConfig, FromCoordsConfig, FromWktConfig, GeometryAccessorsConfig,
    LineInterpolatePointConfig, LineLocatePointConfig, LineSubstringConfig, OtherWkbConfig,
    RotateConfig, ScaleConfig, SimplifyConfig, SimplifyPolicyParam, SnapConfig, SnapToGridConfig,
    TranslateConfig,
};
use plenora_kernels_geo::analyze::{DEFAULT_X_COLUMN, DEFAULT_Y_COLUMN};
use plenora_kernels_geo::arrow_adapter::{encode_geometry, map_nullable};
use plenora_kernels_geo::extensions::OnWktError;
use plenora_kernels_geo::geodetica::EllissoideGeodetico;
use plenora_kernels_geo::margine::{byte_heap_geometria, MargineMemoria};
use plenora_kernels_geo::operations::{BufferCapStyle, SimplifyPolicy};
use plenora_kernels_geo::predicates::SpatialPredicate;
use plenora_kernels_geo::riproiezione::ReprojectParams;
use plenora_kernels_geo::rust_backend::precision::Precision;
use plenora_kernels_geo::{
    construction, extended, extended_algorithms, extensions, extensions2, operations, predicates,
    riproiezione, Operation,
};
use serde_json::Value;

use super::errori::del_kernel;
use super::{binaria, config, nel_dominio, sostituisci, Lato};

/// Tetto delle coppie di coordinate di Hausdorff e Frechet (lavoro
/// quadratico per riga): lo stesso ordine di `MAX_NODING_WORK` e
/// `MAX_SPLIT_WORK` dei kernel.
const MAX_COPPIE_COORDINATE: u64 = 100_000_000;

/// Trasformazione di una geometria in un'altra, in place.
#[derive(Debug)]
pub(super) enum Trasformazione {
    /// `centroid`, `convex_hull`, `envelope`: la pipeline canonica dei
    /// kernel (validazione OGC in ingresso e in uscita).
    Canonica(Operation),
    Contorno,
    PuntoInterno,
    Buffer {
        distanza: f64,
        estremita: BufferCapStyle,
        precisione: Precision,
    },
    Semplifica {
        tolleranza: f64,
        politica: SimplifyPolicy,
    },
    Affine([f64; 6]),
    Trasla {
        x: f64,
        y: f64,
    },
    Scala {
        x: f64,
        y: f64,
        origine: Point<f64>,
    },
    Ruota {
        gradi: f64,
        origine: Point<f64>,
    },
    InviluppoConcavo {
        concavita: f64,
        soglia: f64,
    },
    Densifica(f64),
    SuGriglia(f64),
    Sottolinea {
        inizio: f64,
        fine: f64,
    },
    PuntoSullaLinea(f64),
}

/// Distanza fra la geometria della riga e quella della config. Le
/// geografiche portano l'ellissoide del datum del CRS della colonna (in una
/// `Box`: i due problemi geodetici pronti occupano un migliaio di byte).
#[derive(Clone, Debug)]
pub(super) enum Distanza {
    Euclidea,
    Hausdorff,
    Frechet,
    Haversine(Box<EllissoideGeodetico>),
    Geodetica(Box<EllissoideGeodetico>),
    Azimut(Box<EllissoideGeodetico>),
}

/// Misura di una geometria: una colonna in coda.
#[derive(Debug)]
pub(super) enum Misura {
    Area,
    Lunghezza,
    LunghezzaGeodetica(Box<EllissoideGeodetico>),
    AreaGeodetica(Box<EllissoideGeodetico>),
    Vertici,
    Wkt,
    PosizioneSullaLinea(Point<f64>),
    Distanza {
        tipo: Distanza,
        altra: Geometry<f64>,
    },
    Predicato {
        predicato: SpatialPredicate,
        altra: Geometry<f64>,
    },
}

/// Il kernel di un'operazione 1:1, con la config letta.
#[derive(Debug)]
pub(super) enum KernelUnario {
    Trasforma(Trasformazione),
    Riproietta(Box<ReprojectParams>),
    Aggancia {
        riferimento: Geometry<f64>,
        tolleranza: f64,
    },
    Misura(Misura),
    Limiti,
    Accessori(Vec<AccessorFieldParam>),
    Diagnostica,
    /// I produttori portano il CRS dell'uscita: ogni geometria prodotta
    /// deve stare nel suo dominio di validita', come ogni ingresso.
    DaCoordinate {
        x: usize,
        y: usize,
        crs: ResolvedCrs,
    },
    DaWkt {
        colonna: usize,
        on_error: OnWktError,
        crs: ResolvedCrs,
    },
}

/// La geometria di un parametro WKB esadecimale della config, gia'
/// accettato dall'analisi (struttura, validita' OGC, dominio).
fn geometria_di_config(op: &str, nome: &str, esadecimale: &str) -> Result<Geometry<f64>> {
    let byte = plenora_kernels_geo::wkb_hex_to_bytes(esadecimale)
        .ok_or_else(|| PlenoraError::InvalidPlan(format!("{op}: parametro `{nome}` non valido")))?;
    plenora_kernels_geo::geometry_from_wkb(&byte)
}

fn punto(op: &str, geometria: &Geometry<f64>) -> Result<Point<f64>> {
    match geometria {
        Geometry::Point(punto) => Ok(*punto),
        altra => Err(tipo_inatteso(op, "Point", altra)),
    }
}

fn linea<'a>(op: &str, geometria: &'a Geometry<f64>) -> Result<&'a LineString<f64>> {
    match geometria {
        Geometry::LineString(linea) => Ok(linea),
        altra => Err(tipo_inatteso(op, "LineString", altra)),
    }
}

fn tipo_inatteso(op: &str, atteso: &str, geometria: &Geometry<f64>) -> PlenoraError {
    let ricevuto = match geometria {
        Geometry::Point(_) => "Point",
        Geometry::Line(_) => "Line",
        Geometry::LineString(_) => "LineString",
        Geometry::Polygon(_) => "Polygon",
        Geometry::MultiPoint(_) => "MultiPoint",
        Geometry::MultiLineString(_) => "MultiLineString",
        Geometry::MultiPolygon(_) => "MultiPolygon",
        Geometry::GeometryCollection(_) => "GeometryCollection",
        Geometry::Rect(_) => "Rect",
        Geometry::Triangle(_) => "Triangle",
    };
    PlenoraError::InvalidPlan(format!(
        "{op}: tipo geometria non supportato: atteso {atteso}, ricevuto {ricevuto}"
    ))
}

impl Trasformazione {
    fn prepara(op: &str, valore: &Value, lato: &Lato) -> Result<Option<Self>> {
        Ok(Some(match op {
            "geo.centroid" => Self::Canonica(Operation::Centroid),
            "geo.convex_hull" => Self::Canonica(Operation::ConvexHull),
            "geo.envelope" => Self::Canonica(Operation::Envelope),
            "geo.boundary" => Self::Contorno,
            "geo.point_on_surface" => Self::PuntoInterno,
            "geo.buffer" => {
                let letta: BufferConfig = config(op, valore)?;
                Self::Buffer {
                    distanza: letta.distance,
                    estremita: match letta.cap.unwrap_or(BufferCapParam::Round) {
                        BufferCapParam::Round => BufferCapStyle::Round,
                        BufferCapParam::Flat => BufferCapStyle::Flat,
                        BufferCapParam::Square => BufferCapStyle::Square,
                    },
                    precisione: lato.precisione()?,
                }
            }
            "geo.simplify" => {
                let letta: SimplifyConfig = config(op, valore)?;
                let (politica, soglia) = letta.soglia(op)?;
                Self::Semplifica {
                    tolleranza: soglia,
                    politica: match politica {
                        SimplifyPolicyParam::DouglasPeucker => SimplifyPolicy::DouglasPeucker,
                        SimplifyPolicyParam::PreserveTopology => SimplifyPolicy::PreserveTopology,
                    },
                }
            }
            "geo.affine_transform" => {
                let letta: AffineTransformConfig = config(op, valore)?;
                Self::Affine(letta.coefficients.try_into().map_err(|_| {
                    PlenoraError::InvalidPlan(format!("{op}: servono 6 coefficienti"))
                })?)
            }
            "geo.translate" => {
                let letta: TranslateConfig = config(op, valore)?;
                Self::Trasla {
                    x: letta.x_offset,
                    y: letta.y_offset,
                }
            }
            "geo.scale" => {
                let letta: ScaleConfig = config(op, valore)?;
                Self::Scala {
                    x: letta.x_factor,
                    y: letta.y_factor,
                    origine: Point::new(
                        letta.x_origin.unwrap_or(0.0),
                        letta.y_origin.unwrap_or(0.0),
                    ),
                }
            }
            "geo.rotate" => {
                let letta: RotateConfig = config(op, valore)?;
                Self::Ruota {
                    gradi: letta.degrees,
                    origine: Point::new(
                        letta.x_origin.unwrap_or(0.0),
                        letta.y_origin.unwrap_or(0.0),
                    ),
                }
            }
            "geo.concave_hull" => {
                let letta: ConcaveHullConfig = config(op, valore)?;
                Self::InviluppoConcavo {
                    concavita: letta.concavity,
                    soglia: letta.length_threshold.unwrap_or(0.0),
                }
            }
            "geo.densify" => {
                let letta: DensifyConfig = config(op, valore)?;
                Self::Densifica(letta.max_segment_length)
            }
            "geo.snap_to_grid" => {
                let letta: SnapToGridConfig = config(op, valore)?;
                Self::SuGriglia(letta.grid_size)
            }
            "geo.line_substring" => {
                let letta: LineSubstringConfig = config(op, valore)?;
                Self::Sottolinea {
                    inizio: letta.start_ratio,
                    fine: letta.end_ratio,
                }
            }
            "geo.line_interpolate_point" => {
                let letta: LineInterpolatePointConfig = config(op, valore)?;
                Self::PuntoSullaLinea(letta.ratio)
            }
            _ => return Ok(None),
        }))
    }

    /// La geometria trasformata; `None` dove il kernel non ne ha una (una
    /// geometria vuota), che diventa null.
    fn applica(
        &self,
        op: &str,
        geometria: &Geometry<f64>,
        margine: MargineMemoria,
    ) -> Result<Option<Geometry<f64>>> {
        Ok(match self {
            Self::Canonica(operazione) => Some(plenora_kernels_geo::transform_geometry(
                *operazione,
                geometria,
            )?),
            Self::Contorno => {
                Some(operations::boundary(geometria).map_err(|e| del_kernel(op, &e))?)
            }
            Self::PuntoInterno => {
                operations::point_on_surface(geometria).map_err(|e| del_kernel(op, &e))?
            }
            Self::Buffer {
                distanza,
                estremita,
                precisione,
            } => Some(
                operations::buffer_with_cap_con_margine(
                    geometria,
                    *distanza,
                    *estremita,
                    *precisione,
                    margine,
                )
                .map_err(|e| del_kernel(op, &e))?,
            ),
            Self::Semplifica {
                tolleranza,
                politica,
            } => Some(
                operations::simplify_with_policy(geometria, *tolleranza, *politica)
                    .map_err(|e| del_kernel(op, &e))?,
            ),
            Self::Affine(coefficienti) => Some(
                extended::affine_transform(geometria, *coefficienti)
                    .map_err(|e| del_kernel(op, &e))?,
            ),
            Self::Trasla { x, y } => {
                Some(extended::translate(geometria, *x, *y).map_err(|e| del_kernel(op, &e))?)
            }
            Self::Scala { x, y, origine } => Some(
                extended::scale_about(geometria, *x, *y, *origine)
                    .map_err(|e| del_kernel(op, &e))?,
            ),
            Self::Ruota { gradi, origine } => Some(
                extended::rotate_about(geometria, *gradi, *origine)
                    .map_err(|e| del_kernel(op, &e))?,
            ),
            Self::InviluppoConcavo { concavita, soglia } => Some(
                extended::concave_hull(geometria, *concavita, *soglia, MAX_CELL_COORDINATES)
                    .map_err(|e| del_kernel(op, &e))?,
            ),
            Self::Densifica(lunghezza) => Some(
                extended_algorithms::densify(geometria, *lunghezza, MAX_CELL_COORDINATES)
                    .map_err(|e| del_kernel(op, &e))?,
            ),
            Self::SuGriglia(passo) => Some(
                extended_algorithms::snap_to_grid(geometria, *passo)
                    .map_err(|e| del_kernel(op, &e))?,
            ),
            Self::Sottolinea { inizio, fine } => {
                extended_algorithms::line_substring(linea(op, geometria)?, *inizio, *fine)
                    .map_err(|e| del_kernel(op, &e))?
            }
            Self::PuntoSullaLinea(frazione) => {
                extended_algorithms::line_interpolate_point(linea(op, geometria)?, *frazione)
                    .map_err(|e| del_kernel(op, &e))?
                    .map(Geometry::Point)
            }
        })
    }
}

impl Misura {
    fn prepara(op: &str, valore: &Value, lato: &Lato) -> Result<Option<Self>> {
        let con_altra = |valore: &Value| -> Result<Geometry<f64>> {
            let letta: OtherWkbConfig = config(op, valore)?;
            geometria_di_config(op, "other_wkb", &letta.other_wkb)
        };
        let distanza = |tipo: Distanza| -> Result<Self> {
            Ok(Self::Distanza {
                tipo,
                altra: con_altra(valore)?,
            })
        };
        let predicato = |predicato: SpatialPredicate| -> Result<Self> {
            Ok(Self::Predicato {
                predicato,
                altra: con_altra(valore)?,
            })
        };
        Ok(Some(match op {
            // `output_column` e' gia' nel contratto: qui non serve.
            "geo.area" => Self::Area,
            "geo.length" | "geo.perimeter" => Self::Lunghezza,
            "geo.geodesic_line_length" => Self::LunghezzaGeodetica(Box::new(lato.ellissoide(op)?)),
            "geo.geodesic_area" => Self::AreaGeodetica(Box::new(lato.ellissoide(op)?)),
            "geo.vertex_count" => Self::Vertici,
            "geo.to_wkt" => Self::Wkt,
            "geo.line_locate_point" => {
                let letta: LineLocatePointConfig = config(op, valore)?;
                let riferimento = geometria_di_config(op, "point_wkb", &letta.point_wkb)?;
                Self::PosizioneSullaLinea(punto(op, &riferimento)?)
            }
            "geo.distance" => distanza(Distanza::Euclidea)?,
            "geo.hausdorff_distance" => distanza(Distanza::Hausdorff)?,
            "geo.frechet_distance" => distanza(Distanza::Frechet)?,
            "geo.haversine_distance" => {
                distanza(Distanza::Haversine(Box::new(lato.ellissoide(op)?)))?
            }
            "geo.geodesic_distance" => {
                distanza(Distanza::Geodetica(Box::new(lato.ellissoide(op)?)))?
            }
            "geo.bearing" => distanza(Distanza::Azimut(Box::new(lato.ellissoide(op)?)))?,
            "geo.predicate_intersects" => predicato(SpatialPredicate::Intersects)?,
            "geo.predicate_disjoint" => predicato(SpatialPredicate::Disjoint)?,
            "geo.predicate_contains" => predicato(SpatialPredicate::Contains)?,
            "geo.predicate_within" => predicato(SpatialPredicate::Within)?,
            "geo.predicate_equals_topo" => predicato(SpatialPredicate::EqualsTopo)?,
            "geo.predicate_covers" => predicato(SpatialPredicate::Covers)?,
            "geo.predicate_covered_by" => predicato(SpatialPredicate::CoveredBy)?,
            "geo.predicate_contains_properly" => predicato(SpatialPredicate::ContainsProperly)?,
            "geo.predicate_touches" => predicato(SpatialPredicate::Touches)?,
            "geo.predicate_crosses" => predicato(SpatialPredicate::Crosses)?,
            "geo.predicate_overlaps" => predicato(SpatialPredicate::Overlaps)?,
            _ => return Ok(None),
        }))
    }

    /// La colonna della misura, riga per riga (null per le geometrie null).
    fn colonna(&self, op: &str, lato: &Lato, batch: &RecordBatch) -> Result<ArrayRef> {
        let celle = lato.celle(batch)?;
        let reale = |f: &(dyn Fn(&Geometry<f64>) -> Result<Option<f64>> + Sync)| {
            per_cella(celle, f).map(|valori| Arc::new(Float64Array::from(valori)) as ArrayRef)
        };
        Ok(match self {
            Self::Area => reale(&|g| {
                operations::area(g)
                    .map(Some)
                    .map_err(|e| del_kernel(op, &e))
            })?,
            Self::Lunghezza => reale(&|g| {
                operations::length(g)
                    .map(Some)
                    .map_err(|e| del_kernel(op, &e))
            })?,
            Self::LunghezzaGeodetica(ellissoide) => reale(&|g| {
                extended::geodesic_line_length_m(linea(op, g)?, ellissoide)
                    .map(Some)
                    .map_err(|e| del_kernel(op, &e))
            })?,
            Self::AreaGeodetica(ellissoide) => reale(&|g| {
                if !matches!(g, Geometry::Polygon(_) | Geometry::MultiPolygon(_)) {
                    return Err(tipo_inatteso(op, "Polygon/MultiPolygon", g));
                }
                extended_algorithms::geodesic_area_m2(g, ellissoide)
                    .map(Some)
                    .map_err(|e| del_kernel(op, &e))
            })?,
            Self::PosizioneSullaLinea(riferimento) => reale(&|g| {
                extensions::line_locate_point(g, riferimento).map_err(|e| del_kernel(op, &e))
            })?,
            Self::Distanza { tipo, altra } => reale(&|g| distanza(op, tipo, g, altra))?,
            Self::Vertici => Arc::new(UInt64Array::from(per_cella(celle, |g| {
                operations::vertex_count(g)
                    .map(Some)
                    .map_err(|e| del_kernel(op, &e))
            })?)),
            Self::Wkt => Arc::new(StringArray::from(per_cella(celle, |g| {
                operations::to_wkt(g)
                    .map(Some)
                    .map_err(|e| del_kernel(op, &e))
            })?)),
            Self::Predicato { predicato, altra } => {
                Arc::new(BooleanArray::from(per_cella(celle, |g| {
                    predicates::evaluate(g, altra, *predicato)
                        .map(Some)
                        .map_err(|e| del_kernel(op, &e))
                })?))
            }
        })
    }
}

/// La distanza di una riga dalla geometria della config.
fn distanza(
    op: &str,
    tipo: &Distanza,
    geometria: &Geometry<f64>,
    altra: &Geometry<f64>,
) -> Result<Option<f64>> {
    match tipo {
        Distanza::Euclidea => {
            operations::distance(geometria, altra).map_err(|e| del_kernel(op, &e))
        }
        Distanza::Hausdorff => {
            extended::hausdorff_distance(geometria, altra, MAX_COPPIE_COORDINATE)
                .map_err(|e| del_kernel(op, &e))
        }
        Distanza::Frechet => extended_algorithms::frechet_distance(
            linea(op, geometria)?,
            linea(op, altra)?,
            MAX_COPPIE_COORDINATE,
        )
        .map_err(|e| del_kernel(op, &e)),
        Distanza::Haversine(ellissoide) => {
            extended::haversine_distance_m(punto(op, geometria)?, punto(op, altra)?, ellissoide)
                .map(Some)
                .map_err(|e| del_kernel(op, &e))
        }
        Distanza::Geodetica(ellissoide) => {
            extended::geodesic_distance_m(punto(op, geometria)?, punto(op, altra)?, ellissoide)
                .map(Some)
                .map_err(|e| del_kernel(op, &e))
        }
        Distanza::Azimut(ellissoide) => extended_algorithms::geodesic_bearing_degrees(
            punto(op, geometria)?,
            punto(op, altra)?,
            ellissoide,
        )
        .map(Some)
        .map_err(|e| del_kernel(op, &e)),
    }
}

/// `f` su ogni cella non-null dopo la decodifica strutturale (dominio del
/// CRS e tipi sono gia' verificati dal passo, la validazione OGC e' del
/// kernel che `f` chiama); i null restano null.
fn per_cella<T: Send>(
    celle: &plenora_core::arrow::array::BinaryArray,
    f: impl Fn(&Geometry<f64>) -> Result<Option<T>> + Sync,
) -> Result<Vec<Option<T>>> {
    map_nullable(celle, |cella| {
        let geometria = plenora_kernels_geo::wkb_decoder::decode_validated(cella)?;
        f(&geometria)
    })
}

/// Le righe trasformate insieme da [`celle_nel_margine`]: un numero fisso,
/// non i thread della macchina, cosi' le decisioni sul margine non
/// dipendono dall'hardware.
const RIGHE_IN_VOLO: usize = 64;

/// Come [`per_cella`], con **un solo conto** della memoria del passo nel
/// `margine`: l'uscita gia' tenuta (le celle codificate, a capacita' vera,
/// e la loro copia nella colonna d'uscita) piu' il transitorio delle righe in
/// lavorazione.
///
/// Le righe si trasformano a blocchi di [`RIGHE_IN_VOLO`] in parallelo, e ogni
/// riga del blocco riceve una parte uguale di cio' che resta del margine
/// (meno la propria geometria decodificata): le righe in volo non lo
/// superano insieme. Dopo il blocco le uscite si accolgono in ordine di
/// riga, contandole prima di tenerle; il primo errore e' quello della prima
/// riga in ordine. Deterministico: il blocco e le parti dipendono solo dai
/// dati e dal margine.
fn celle_nel_margine(
    op: &str,
    celle: &plenora_core::arrow::array::BinaryArray,
    margine: MargineMemoria,
    kernel: impl Fn(&Geometry<f64>, MargineMemoria) -> Result<Option<Geometry<f64>>> + Sync,
) -> Result<Vec<Option<Vec<u8>>>> {
    use plenora_core::arrow::array::Array;
    let oltre = |superato: plenora_kernels_geo::margine::MargineSuperato| {
        PlenoraError::ResourceLimit(format!("{op}: {superato}"))
    };
    let righe = celle.len();
    let mut uscita: Vec<Option<Vec<u8>>> = Vec::new();
    uscita
        .try_reserve_exact(righe)
        .map_err(|_| PlenoraError::ResourceLimit(format!("{op}: allocazione delle celle")))?;
    // Il vettore delle celle e gli offset della colonna d'uscita.
    let mut usati = u64::try_from(righe)
        .unwrap_or(u64::MAX)
        .saturating_mul((std::mem::size_of::<Option<Vec<u8>>>() + 8) as u64);
    margine.verifica(usati).map_err(oltre)?;
    let mut inizio = 0;
    while inizio < righe {
        let fine = (inizio + RIGHE_IN_VOLO).min(righe);
        let per_riga = (margine.byte_disponibili() - usati) / RIGHE_IN_VOLO as u64;
        let fetta = celle.slice(inizio, fine - inizio);
        let blocco = map_nullable(&fetta, |payload| {
            let geometria = plenora_kernels_geo::wkb_decoder::decode_validated(payload)?;
            // La geometria decodificata vive per tutta la riga.
            let decodificata = byte_heap_geometria(&geometria);
            MargineMemoria::byte(per_riga)
                .verifica(decodificata)
                .map_err(oltre)?;
            kernel(&geometria, MargineMemoria::byte(per_riga - decodificata))?
                .map(|risultato| encode_geometry(&risultato))
                .transpose()
        })?;
        for risultato in blocco {
            if let Some(wkb) = &risultato {
                // La cella (capacita' vera) e la sua copia nella colonna.
                usati = usati
                    .saturating_add(u64::try_from(wkb.capacity()).unwrap_or(u64::MAX))
                    .saturating_add(u64::try_from(wkb.len()).unwrap_or(u64::MAX));
                margine.verifica(usati).map_err(oltre)?;
            }
            uscita.push(risultato);
        }
        inizio = fine;
    }
    Ok(uscita)
}

/// La colonna di un nome dello schema d'ingresso.
fn indice_colonna(op: &str, contratto: &DataContract, nome: &str) -> Result<usize> {
    contratto.schema.index_of(nome).map_err(|_| {
        PlenoraError::Internal(format!(
            "{op}: colonna `{nome}` assente dallo schema dopo l'analisi"
        ))
    })
}

impl KernelUnario {
    /// Il kernel di `op`, se e' un'operazione 1:1 di questo modulo.
    pub(super) fn prepara(
        op: &str,
        valore: &Value,
        lati: &[Option<Lato>],
        ingressi: &[DataContract],
        uscita: &DataContract,
        _righe_massime: u64,
    ) -> Result<Option<Self>> {
        let [contratto] = ingressi else {
            return Ok(None);
        };
        let crs_prodotto = || {
            uscita
                .active_geometry_column()
                .and_then(|geometria| geometria.crs.as_resolved().cloned())
                .ok_or_else(|| {
                    PlenoraError::Internal(format!("{op}: produttore senza CRS dopo l'analisi"))
                })
        };
        match op {
            "geo.from_coords" => {
                let letta: FromCoordsConfig = config(op, valore)?;
                let x = letta.x_column.as_deref().unwrap_or(DEFAULT_X_COLUMN);
                let y = letta.y_column.as_deref().unwrap_or(DEFAULT_Y_COLUMN);
                return Ok(Some(Self::DaCoordinate {
                    x: indice_colonna(op, contratto, x)?,
                    y: indice_colonna(op, contratto, y)?,
                    crs: crs_prodotto()?,
                }));
            }
            "geo.from_wkt" => {
                let letta: FromWktConfig = config(op, valore)?;
                return Ok(Some(Self::DaWkt {
                    colonna: indice_colonna(op, contratto, &letta.wkt_column)?,
                    on_error: letta.on_error.unwrap_or(OnWktError::Null),
                    crs: crs_prodotto()?,
                }));
            }
            _ => {}
        }
        let Some(Some(geometria)) = lati.first() else {
            return Ok(None);
        };
        if let Some(trasformazione) = Trasformazione::prepara(op, valore, geometria)? {
            return Ok(Some(Self::Trasforma(trasformazione)));
        }
        if let Some(misura) = Misura::prepara(op, valore, geometria)? {
            return Ok(Some(Self::Misura(misura)));
        }
        Ok(Some(match op {
            "geo.reproject" => Self::Riproietta(Box::new(ReprojectParams::da_config(
                op,
                valore,
                &geometria.crs,
            )?)),
            "geo.snap" => {
                let letta: SnapConfig = config(op, valore)?;
                Self::Aggancia {
                    riferimento: geometria_di_config(op, "reference_wkb", &letta.reference_wkb)?,
                    tolleranza: letta.tolerance,
                }
            }
            "geo.bounds_extractor" => Self::Limiti,
            "geo.geometry_diagnostics" => Self::Diagnostica,
            "geo.geometry_accessors" => {
                let letta: GeometryAccessorsConfig = config(op, valore)?;
                let mut scelti = letta.fields.unwrap_or_else(|| {
                    vec![
                        AccessorFieldParam::GeometryType,
                        AccessorFieldParam::NumGeometries,
                        AccessorFieldParam::NumInteriorRings,
                        AccessorFieldParam::StartPoint,
                        AccessorFieldParam::EndPoint,
                        AccessorFieldParam::IsClosed,
                    ]
                });
                // Ordine canonico dell'uscita, come l'analisi.
                scelti.sort_by_key(|campo| campo.column_index());
                Self::Accessori(scelti)
            }
            _ => return Ok(None),
        }))
    }

    /// Le colonne dell'uscita, nell'ordine del contratto, e le righe.
    pub(super) fn esegui(
        &self,
        op: &str,
        lato: Option<&Lato>,
        batch: &RecordBatch,
        _uscita: &DataContract,
        margine: MargineMemoria,
    ) -> Result<(Vec<ArrayRef>, usize)> {
        let righe = batch.num_rows();
        let mut colonne = batch.columns().to_vec();
        match self {
            Self::DaCoordinate { x, y, crs } => {
                colonne.push(da_coordinate(op, batch, *x, *y, crs)?);
                return Ok((colonne, righe));
            }
            Self::DaWkt {
                colonna,
                on_error,
                crs,
            } => {
                colonne.push(da_wkt(op, batch, *colonna, *on_error, crs)?);
                return Ok((colonne, righe));
            }
            _ => {}
        }
        let lato = lato.ok_or_else(|| {
            PlenoraError::Internal(format!("{op}: operazione senza colonna geometria"))
        })?;
        match self {
            Self::Trasforma(trasformazione) => {
                let celle = if matches!(trasformazione, Trasformazione::Buffer { .. }) {
                    celle_nel_margine(op, lato.celle(batch)?, margine, |g, margine_riga| {
                        trasformazione.applica(op, g, margine_riga)
                    })?
                } else {
                    per_cella(lato.celle(batch)?, |g| {
                        trasformazione
                            .applica(op, g, MargineMemoria::ILLIMITATO)?
                            .map(|uscita| encode_geometry(&uscita))
                            .transpose()
                    })?
                };
                sostituisci(op, &mut colonne, lato.indice, vec![binaria(&celle)])?;
            }
            Self::Riproietta(parametri) => {
                // L'adapter dei kernel: stesse righe, geometria riproiettata
                // (dominio sorgente e target, densificazione).
                let (_, uscite) = riproiezione::reproject_batches(
                    &batch.schema(),
                    std::slice::from_ref(batch),
                    &lato.nome,
                    &lato.crs,
                    parametri,
                )?;
                let [riproiettata] = uscite.as_slice() else {
                    return Err(PlenoraError::Internal(format!(
                        "{op}: l'adapter non ha reso un batch per l'ingresso"
                    )));
                };
                let geometrie = riproiettata
                    .columns()
                    .get(lato.indice)
                    .cloned()
                    .ok_or_else(|| {
                        PlenoraError::Internal(format!(
                            "{op}: colonna geometria riproiettata assente"
                        ))
                    })?;
                sostituisci(op, &mut colonne, lato.indice, vec![geometrie])?;
            }
            Self::Aggancia {
                riferimento,
                tolleranza,
            } => {
                let celle = extensions2::snap_column(lato.celle(batch)?, riferimento, *tolleranza)?;
                sostituisci(op, &mut colonne, lato.indice, vec![binaria(&celle)])?;
            }
            Self::Misura(misura) => colonne.push(misura.colonna(op, lato, batch)?),
            Self::Limiti => {
                let limiti = per_cella(lato.celle(batch)?, |g| {
                    operations::bounds(g).map_err(|e| del_kernel(op, &e))
                })?;
                for asse in 0..4 {
                    colonne.push(Arc::new(Float64Array::from(
                        limiti
                            .iter()
                            .map(|valore| valore.map(|limite| limite[asse]))
                            .collect::<Vec<_>>(),
                    )));
                }
            }
            Self::Accessori(scelti) => {
                let accessori = per_cella(lato.celle(batch)?, |g| {
                    extensions::geometry_accessors(g)
                        .map(Some)
                        .map_err(|e| del_kernel(op, &e))
                })?;
                for campo in scelti {
                    colonne.push(colonna_accessoria(*campo, &accessori));
                }
            }
            Self::Diagnostica => {
                let diagnostiche = diagnostica(op, lato, batch)?;
                sostituisci(op, &mut colonne, lato.indice, diagnostiche)?;
            }
            Self::DaCoordinate { .. } | Self::DaWkt { .. } => {}
        }
        Ok((colonne, righe))
    }
}

fn colonna_accessoria(
    campo: AccessorFieldParam,
    accessori: &[Option<extensions::GeometryAccessors>],
) -> ArrayRef {
    let valori = accessori.iter().map(Option::as_ref);
    match campo {
        AccessorFieldParam::GeometryType => Arc::new(StringArray::from(
            valori
                .map(|a| a.map(|a| a.geometry_type))
                .collect::<Vec<_>>(),
        )),
        AccessorFieldParam::NumGeometries => Arc::new(UInt64Array::from(
            valori
                .map(|a| a.map(|a| a.num_geometries))
                .collect::<Vec<_>>(),
        )),
        AccessorFieldParam::NumInteriorRings => Arc::new(UInt64Array::from(
            valori
                .map(|a| a.map(|a| a.num_interior_rings))
                .collect::<Vec<_>>(),
        )),
        AccessorFieldParam::StartPoint => Arc::new(StringArray::from(
            valori
                .map(|a| a.and_then(|a| a.start_point.as_deref()))
                .collect::<Vec<_>>(),
        )),
        AccessorFieldParam::EndPoint => Arc::new(StringArray::from(
            valori
                .map(|a| a.and_then(|a| a.end_point.as_deref()))
                .collect::<Vec<_>>(),
        )),
        AccessorFieldParam::IsClosed => Arc::new(BooleanArray::from(
            valori.map(|a| a.map(|a| a.is_closed)).collect::<Vec<_>>(),
        )),
    }
}

/// Le dieci colonne diagnostiche (`DIAGNOSTIC_COLUMNS`). Solo la struttura
/// WKB si verifica: una geometria OGC non valida e' il dato che la
/// diagnostica riporta, non un errore (come a `190c493`).
fn diagnostica(op: &str, lato: &Lato, batch: &RecordBatch) -> Result<Vec<ArrayRef>> {
    let valori = per_cella(lato.celle(batch)?, |g| {
        extended_algorithms::geometry_diagnostics(g)
            .map(Some)
            .map_err(|e| del_kernel(op, &e))
    })?;
    let di = |f: &dyn Fn(&extended_algorithms::GeometryDiagnostics) -> Option<f64>| {
        Arc::new(Float64Array::from(
            valori
                .iter()
                .map(|valore| valore.as_ref().and_then(f))
                .collect::<Vec<_>>(),
        )) as ArrayRef
    };
    Ok(vec![
        Arc::new(StringArray::from(
            valori
                .iter()
                .map(|v| v.as_ref().map(|v| v.geometry_type))
                .collect::<Vec<_>>(),
        )),
        Arc::new(UInt64Array::from(
            valori
                .iter()
                .map(|v| v.as_ref().map(|v| v.coordinate_count))
                .collect::<Vec<_>>(),
        )),
        Arc::new(BooleanArray::from(
            valori
                .iter()
                .map(|v| v.as_ref().map(|v| v.is_empty))
                .collect::<Vec<_>>(),
        )),
        Arc::new(BooleanArray::from(
            valori
                .iter()
                .map(|v| v.as_ref().map(|v| v.is_finite))
                .collect::<Vec<_>>(),
        )),
        Arc::new(BooleanArray::from(
            valori
                .iter()
                .map(|v| v.as_ref().map(|v| v.is_valid))
                .collect::<Vec<_>>(),
        )),
        Arc::new(StringArray::from(
            valori
                .iter()
                .map(|v| v.as_ref().and_then(|v| v.validity_reason.as_deref()))
                .collect::<Vec<_>>(),
        )),
        di(&|v| v.bounds.map(|b| b[0])),
        di(&|v| v.bounds.map(|b| b[1])),
        di(&|v| v.bounds.map(|b| b[2])),
        di(&|v| v.bounds.map(|b| b[3])),
    ])
}

/// I valori di una colonna di coordinate (`Float64`, o `Int64` entro
/// 2^53 in modulo, dove la conversione e' esatta).
fn coordinate(op: &str, batch: &RecordBatch, indice: usize) -> Result<Vec<Option<f64>>> {
    let colonna = batch.column(indice);
    match colonna.data_type() {
        DataType::Float64 => Ok(colonna
            .as_any()
            .downcast_ref::<Float64Array>()
            .ok_or_else(|| PlenoraError::Internal(format!("{op}: colonna non Float64")))?
            .iter()
            .collect()),
        DataType::Int64 => {
            const MASSIMO_ESATTO: u64 = 1 << 53;
            colonna
                .as_any()
                .downcast_ref::<Int64Array>()
                .ok_or_else(|| PlenoraError::Internal(format!("{op}: colonna non Int64")))?
                .iter()
                .map(|valore| {
                    valore
                        .map(|intero| {
                            if intero.unsigned_abs() > MASSIMO_ESATTO {
                                Err(PlenoraError::InvalidPlan(format!(
                                    "{op}: coordinata intera oltre 2^53 in modulo, non \
                                     rappresentabile esattamente come f64"
                                )))
                            } else {
                                // Esatta: |intero| <= 2^53.
                                #[allow(clippy::cast_precision_loss)]
                                Ok(intero as f64)
                            }
                        })
                        .transpose()
                })
                .collect()
        }
        altro => Err(PlenoraError::Internal(format!(
            "{op}: colonna di coordinate di tipo {altro} dopo l'analisi"
        ))),
    }
}

/// `geo.from_coords`: un punto per riga. Il contratto dichiara la geometria
/// non nullable: una coordinata null e' un errore esplicito (a `190c493` il
/// punto diventava null in una colonna dichiarata non-null).
fn da_coordinate(
    op: &str,
    batch: &RecordBatch,
    x: usize,
    y: usize,
    crs: &ResolvedCrs,
) -> Result<ArrayRef> {
    let xs = coordinate(op, batch, x)?;
    let ys = coordinate(op, batch, y)?;
    let mut celle = Vec::with_capacity(xs.len());
    for (x, y) in xs.into_iter().zip(ys) {
        let (Some(x), Some(y)) = (x, y) else {
            return Err(PlenoraError::InvalidPlan(format!(
                "{op}: coordinata null: la colonna geometria prodotta non ammette null"
            )));
        };
        let punto = construction::point_from_lon_lat(x, y).map_err(|e| del_kernel(op, &e))?;
        nel_dominio(op, &punto, crs)?;
        celle.push(Some(encode_geometry(&punto)?));
    }
    Ok(binaria(&celle))
}

/// `geo.from_wkt`: l'adapter dei kernel, che rifiuta ogni cella non WKT
/// valido con la diagnostica per riga.
fn da_wkt(
    op: &str,
    batch: &RecordBatch,
    colonna: usize,
    on_error: OnWktError,
    crs: &ResolvedCrs,
) -> Result<ArrayRef> {
    let valori = batch
        .column(colonna)
        .as_any()
        .downcast_ref::<StringArray>()
        .ok_or_else(|| PlenoraError::Internal(format!("{op}: colonna WKT non Utf8")))?;
    let nome = batch.schema().field(colonna).name().clone();
    let celle = extensions::from_wkt_column_named(valori, on_error, Some(&nome))?;
    let prodotte: plenora_core::arrow::array::BinaryArray =
        celle.iter().map(Option::as_deref).collect();
    super::verifica_dominio(op, &prodotte, crs)?;
    Ok(Arc::new(prodotte))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Il conto unico del passo: ogni riga da sola sta nel margine, ma le
    /// uscite accumulate no, e il passo si ferma (prima ogni riga riceveva
    /// lo stesso margine, e l'uscita accumulata non lo riduceva); con un
    /// margine ampio l'uscita e' quella riga per riga.
    #[test]
    fn il_buffer_conta_l_uscita_accumulata_nel_margine() {
        let celle: plenora_core::arrow::array::BinaryArray = (0_i32..1_000)
            .map(|k| {
                let punto = Geometry::Point(Point::new(
                    f64::from(k).mul_add(10.0, 500_000.0),
                    5_000_000.0,
                ));
                Some(encode_geometry(&punto).expect("wkb"))
            })
            .collect::<Vec<_>>()
            .iter()
            .map(Option::as_deref)
            .collect();
        let precisione = Precision::new(0.01).expect("1 cm");
        let kernel = |g: &Geometry<f64>, margine: MargineMemoria| {
            operations::buffer_with_cap_con_margine(
                g,
                1_000.0,
                BufferCapStyle::Round,
                precisione,
                margine,
            )
            .map(Some)
            .map_err(|e| del_kernel("geo.buffer", &e))
        };
        let ampio = celle_nel_margine("geo.buffer", &celle, MargineMemoria::byte(1 << 30), kernel)
            .expect("margine ampio");
        let attese = per_cella(&celle, |g| {
            kernel(g, MargineMemoria::ILLIMITATO)?
                .map(|b| encode_geometry(&b))
                .transpose()
        })
        .expect("senza margine");
        assert_eq!(ampio, attese);
        let uscita: u64 = ampio
            .iter()
            .flatten()
            .map(|wkb| (wkb.capacity() + wkb.len()) as u64)
            .sum();
        // Meta' dell'uscita: ogni riga (qualche KB) sta nella sua parte,
        // l'accumulo no.
        let errore = celle_nel_margine(
            "geo.buffer",
            &celle,
            MargineMemoria::byte(uscita / 2),
            kernel,
        )
        .expect_err("uscita accumulata oltre il margine");
        assert!(errore.to_string().contains("margine"), "{errore}");
    }
}
