//! Kernel di clustering spaziale per densita' (`geo.cluster_dbscan`),
//! estensione di catalogo v1.3.
//!
//! Kernel puro su `geo::Point<f64>` piu' l'adapter di colonna
//! (`dbscan_column`) che mappa gli errori su [`PlenoraError`] preservando i
//! messaggi, come `extensions.rs` (v1.1), `extensions2.rs` (v1.2) ed
//! `extensions3.rs` (v1.3).
//!
//! Scelte documentate (v1):
//!
//! - **Solo Point**: l'input dev'essere puntuale; ogni altro tipo geometrico
//!   e' rifiutato esplicitamente (fail-closed, come `extensions3.rs` per i
//!   non poligonali). Niente `use_centroid` in v1: il centroide di un
//!   poligono non rappresenta la sua densita' spaziale e la scelta sarebbe
//!   silenziosa; se servira', sara' un parametro di config esplicito in v2.
//! - **DBSCAN standard**: un punto e' `core` se il suo eps-vicinato (distanza
//!   euclidea `<= eps`, **incluso il punto stesso**) contiene almeno
//!   `min_points` punti; i cluster sono le componenti connesse per densita'
//!   dei core point; i punti non-core nel vicinato di un core sono `border`;
//!   gli altri sono `noise`. Etichette `cluster_id` UInt64 `0..k-1`; noise
//!   assegnati al cluster che li raggiunge; solo i noise mai assegnati → `null` (colonna nullable). Un border
//!   raggiungibile da due cluster e' assegnato al primo che lo raggiunge
//!   (ordine di visita, vedi sotto).
//! - **Vicinato via R-tree** (`rstar`, gia' dipendenza del crate, come
//!   `spatial_join`/`snap`): range query con raggio `eps` per punto, nessuna
//!   scansione O(n²).
//! - **Determinismo obbligatorio**: la visita esterna segue l'indice di riga
//!   crescente; le liste di vicini sono ordinate per indice; l'espansione di
//!   un cluster e' una coda FIFO alimentata in ordine di indice; i cluster
//!   sono numerati **in ordine di scoperta** (il primo cluster trovato
//!   scorrendo le righe e' 0). Nessuna iterazione su hash map: stesso input
//!   → stessi id, run dopo run.
//! - Righe con geometria **null**: non partecipano al clustering e ricevono
//!   etichetta null (null propagato, non noise); conservano la posizione.
//! - `eps` finito e `> 0`, `min_points >= 1` (con `min_points = 1` ogni punto
//!   e' core: i punti isolati formano cluster di un elemento). `eps` e' in
//!   unita' di mappa: il requisito di catalogo e' `Projected`.
//!
//! Il calcolo e' globale (un vicinato dipende da tutto l'input), quindi la
//! classe di esecuzione e' `Blocking`; l'output resta allineato 1:1 alle
//! righe di input (`OneToOne`).
//!
//! Errori: le condizioni del kernel puro usano [`ClusterError`].

use std::collections::VecDeque;

use geo::algorithm::validation::Validation;
use geo::{CoordsIter, Geometry, Point};
use plenora_core::arrow::array::BinaryArray;
use plenora_core::PlenoraError;
use rstar::{PointDistance, RTree, RTreeObject, AABB};
use thiserror::Error;

use crate::arrow_adapter::{decode_geometry_cell, map_nullable};

#[derive(Debug, Error)]
pub enum ClusterError {
    #[error("parametro {name} non valido: {reason}")]
    InvalidParameter {
        name: &'static str,
        reason: &'static str,
    },
    #[error("geometria {index} non puntuale ({found}): attesa Point")]
    UnsupportedGeometry { index: usize, found: &'static str },
    #[error("geometria {index} non valida: {reason}")]
    InvalidGeometry { index: usize, reason: String },
    #[error("geometria {index} contiene coordinate NaN o infinite")]
    NonFiniteCoordinate { index: usize },
    #[error("conteggio non rappresentabile come uint64")]
    IndexOverflow,
}

fn invalid_parameter(name: &'static str, reason: &'static str) -> ClusterError {
    ClusterError::InvalidParameter { name, reason }
}

fn check_eps(eps: f64) -> Result<(), ClusterError> {
    if !eps.is_finite() || eps <= 0.0 {
        return Err(invalid_parameter(
            "eps",
            "deve essere finito e maggiore di zero",
        ));
    }
    Ok(())
}

fn check_min_points(min_points: usize) -> Result<(), ClusterError> {
    if min_points < 1 {
        return Err(invalid_parameter("min_points", "deve essere almeno 1"));
    }
    Ok(())
}

fn geometry_name(geometry: &Geometry<f64>) -> &'static str {
    match geometry {
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
    }
}

/// Punto indicizzato per l'R-tree: la posizione originale viaggia con
/// l'elemento, cosi' le liste di vicini si riordinano per indice.
#[derive(Clone, Copy)]
struct IndexedPoint {
    index: usize,
    coords: [f64; 2],
}

impl RTreeObject for IndexedPoint {
    type Envelope = AABB<[f64; 2]>;

    fn envelope(&self) -> Self::Envelope {
        AABB::from_point(self.coords)
    }
}

impl PointDistance for IndexedPoint {
    fn distance_2(&self, point: &[f64; 2]) -> f64 {
        let dx = self.coords[0] - point[0];
        let dy = self.coords[1] - point[1];
        dx * dx + dy * dy
    }
}

/// DBSCAN su punti gia' validati. Deterministico: visita per indice di riga,
/// vicini ordinati per indice, cluster numerati in ordine di scoperta.
fn dbscan_core(points: &[[f64; 2]], eps: f64, min_points: usize) -> Result<Vec<Option<u64>>, ClusterError> {
    let tree = RTree::bulk_load(
        points
            .iter()
            .enumerate()
            .map(|(index, &coords)| IndexedPoint { index, coords })
            .collect(),
    );
    // rstar ragiona in distanza quadrata; l'overflow a +inf per eps enormi
    // e' innocuo (ogni coppia rientra), l'underflow a 0 per eps minuscoli
    // lascia dentro solo i duplicati esatti (distanza 0 <= eps).
    let radius_2 = eps * eps;
    let neighbors = |index: usize| -> Vec<usize> {
        let mut found: Vec<usize> = tree
            .locate_within_distance(points[index], radius_2)
            .map(|element| element.index)
            .collect();
        found.sort_unstable();
        found
    };
    let mut labels = vec![None; points.len()];
    let mut visited = vec![false; points.len()];
    let mut queued = vec![false; points.len()];
    let mut next_cluster = 0_u64;
    for index in 0..points.len() {
        if visited[index] {
            continue;
        }
        visited[index] = true;
        let seed = neighbors(index);
        if seed.len() < min_points {
            continue; // noise provvisorio: puo' diventare border
        }
        let cluster = next_cluster;
        next_cluster = next_cluster
            .checked_add(1)
            .ok_or(ClusterError::IndexOverflow)?;
        labels[index] = Some(cluster);
        queued[index] = true;
        let mut queue: VecDeque<usize> = VecDeque::new();
        for &member in &seed {
            if member != index {
                queued[member] = true;
                queue.push_back(member);
            }
        }
        while let Some(member) = queue.pop_front() {
            if !visited[member] {
                visited[member] = true;
                let expanded = neighbors(member);
                if expanded.len() >= min_points {
                    for &reach in &expanded {
                        if !queued[reach] {
                            queued[reach] = true;
                            queue.push_back(reach);
                        }
                    }
                }
            }
            if labels[member].is_none() {
                labels[member] = Some(cluster);
            }
        }
    }
    Ok(labels)
}

/// Punti preparati per il clustering: riga di origine per punto e
/// coordinate `[x, y]` dei punti validi.
type PreparedPoints = (Vec<Option<usize>>, Vec<[f64; 2]>);

/// Estrae e valida i punti dalle geometrie nullable: solo Point con
/// coordinate finite e geometria valida; i `None` non partecipano.
fn prepare_points(
    geometries: &[Option<Geometry<f64>>],
) -> Result<PreparedPoints, ClusterError> {
    let mut row_of_point = Vec::with_capacity(geometries.len());
    let mut points = Vec::with_capacity(geometries.len());
    for (index, geometry) in geometries.iter().enumerate() {
        let Some(geometry) = geometry else {
            row_of_point.push(None);
            continue;
        };
        if geometry
            .coords_iter()
            .any(|coordinate| !coordinate.x.is_finite() || !coordinate.y.is_finite())
        {
            return Err(ClusterError::NonFiniteCoordinate { index });
        }
        geometry
            .check_validation()
            .map_err(|error| ClusterError::InvalidGeometry {
                index,
                reason: error.to_string(),
            })?;
        let Geometry::Point(point) = geometry else {
            return Err(ClusterError::UnsupportedGeometry {
                index,
                found: geometry_name(geometry),
            });
        };
        row_of_point.push(Some(points.len()));
        points.push([point.x(), point.y()]);
    }
    Ok((row_of_point, points))
}

/// Clustering DBSCAN di una colonna di geometrie puntuali: un'etichetta per
/// riga (allineata all'input), `None` per noise e per le geometrie null.
pub fn dbscan_nullable(
    geometries: &[Option<Geometry<f64>>],
    eps: f64,
    min_points: usize,
) -> Result<Vec<Option<u64>>, ClusterError> {
    check_eps(eps)?;
    check_min_points(min_points)?;
    let (row_of_point, points) = prepare_points(geometries)?;
    let point_labels = dbscan_core(&points, eps, min_points)?;
    Ok(row_of_point
        .iter()
        .map(|slot| slot.and_then(|point| point_labels[point]))
        .collect())
}

/// Clustering DBSCAN di punti (shortcut non nullable).
pub fn dbscan(
    points: &[Point<f64>],
    eps: f64,
    min_points: usize,
) -> Result<Vec<Option<u64>>, ClusterError> {
    let geometries: Vec<Option<Geometry<f64>>> = points
        .iter()
        .map(|point| Some(Geometry::Point(*point)))
        .collect();
    dbscan_nullable(&geometries, eps, min_points)
}

fn cluster_error(error: ClusterError) -> PlenoraError {
    PlenoraError::Contract(format!("geo.cluster_dbscan: {error}"))
}

/// Adapter di colonna per `geo.cluster_dbscan`: decodifica le celle WKB
/// non-null, esegue il clustering globale e restituisce un'etichetta UInt64
/// nullable per riga (null = noise o geometria null), nello stesso ordine
/// delle righe di input.
pub fn dbscan_column(
    cells: &BinaryArray,
    eps: f64,
    min_points: usize,
) -> Result<Vec<Option<u64>>, PlenoraError> {
    check_eps(eps).map_err(cluster_error)?;
    check_min_points(min_points).map_err(cluster_error)?;
    let geometries = map_nullable(cells, |payload| decode_geometry_cell(payload).map(Some))?;
    dbscan_nullable(&geometries, eps, min_points).map_err(cluster_error)
}

#[cfg(test)]
mod tests {
    use super::*;
    use geozero::{CoordDimensions, ToWkb};
    use plenora_core::arrow::array::BinaryArray;

    fn points(coords: &[(f64, f64)]) -> Vec<Point<f64>> {
        coords
            .iter()
            .map(|&(x, y)| Point::new(x, y))
            .collect()
    }

    fn cloud(center: (f64, f64), count: usize) -> Vec<(f64, f64)> {
        // Griglia densa deterministica attorno al centro (passo 0.1).
        let side = (count as f64).sqrt().ceil() as usize;
        (0..count)
            .map(|index| {
                (
                    center.0 + (index % side) as f64 * 0.1,
                    center.1 + (index / side) as f64 * 0.1,
                )
            })
            .collect()
    }

    fn wkb_column(geometries: &[Option<Geometry<f64>>]) -> BinaryArray {
        let cells: Vec<Option<Vec<u8>>> = geometries
            .iter()
            .map(|geometry| {
                geometry
                    .as_ref()
                    .map(|geometry| geometry.to_wkb(CoordDimensions::xy()).expect("encode"))
            })
            .collect();
        BinaryArray::from_iter(cells.iter().map(|cell| cell.as_deref()))
    }

    // --- kernel puro ---------------------------------------------------------

    #[test]
    fn two_separated_clouds_and_an_outlier_make_two_clusters_plus_noise() {
        let mut coords = cloud((0.0, 0.0), 25);
        coords.extend(cloud((100.0, 100.0), 25));
        coords.push((50.0, 50.0)); // outlier isolato
        let labels = dbscan(&points(&coords), 0.5, 3).expect("dbscan");
        assert_eq!(labels.len(), 51);
        // Prima nuvola -> cluster 0 (ordine di scoperta per indice di riga).
        assert!(labels[..25].iter().all(|label| *label == Some(0)));
        // Seconda nuvola -> cluster 1.
        assert!(labels[25..50].iter().all(|label| *label == Some(1)));
        // Outlier -> noise (null).
        assert_eq!(labels[50], None);
    }

    #[test]
    fn density_connected_chain_is_one_cluster() {
        // Catena di punti a passo 1.0 (< eps): ogni punto ha 1-2 vicini, con
        // min_points 2 i punti interni sono core e la catena si salda.
        let coords: Vec<(f64, f64)> = (0..20).map(|index| (index as f64, 0.0)).collect();
        let labels = dbscan(&points(&coords), 1.5, 2).expect("dbscan");
        assert!(
            labels.iter().all(|label| *label == Some(0)),
            "catena non saldata: {labels:?}"
        );
        // Due catene separate: i cluster seguono l'ordine di riga.
        let mut two = coords.clone();
        two.extend((0..20).map(|index| (100.0 + index as f64, 0.0)));
        let labels = dbscan(&points(&two), 1.5, 2).expect("dbscan");
        assert!(labels[..20].iter().all(|label| *label == Some(0)));
        assert!(labels[20..].iter().all(|label| *label == Some(1)));
    }

    #[test]
    fn min_points_boundaries_core_border_noise() {
        // Tre punti allineati a passo 1: A(0) B(1) C(2), eps 1.0.
        // min_points = 3: B e' core (A,B,C), A e C border -> un solo cluster.
        let coords = [(0.0, 0.0), (1.0, 0.0), (2.0, 0.0)];
        let labels = dbscan(&points(&coords), 1.0, 3).expect("dbscan");
        assert_eq!(labels, vec![Some(0), Some(0), Some(0)]);
        // min_points = 4: nessun core (vicinati da 2-3 punti) -> tutto noise.
        let labels = dbscan(&points(&coords), 1.0, 4).expect("dbscan");
        assert_eq!(labels, vec![None, None, None]);
        // min_points = 1: ogni punto e' core; i lontani sono cluster singoli.
        let spread = [(0.0, 0.0), (10.0, 0.0)];
        let labels = dbscan(&points(&spread), 1.0, 1).expect("dbscan");
        assert_eq!(labels, vec![Some(0), Some(1)]);
    }

    #[test]
    fn duplicate_points_count_towards_the_neighborhood() {
        // Tre duplicati esatti: distanza 0, vicinato da 3 -> core.
        let coords = [(5.0, 5.0), (5.0, 5.0), (5.0, 5.0)];
        let labels = dbscan(&points(&coords), 0.5, 3).expect("dbscan");
        assert_eq!(labels, vec![Some(0), Some(0), Some(0)]);
    }

    #[test]
    fn empty_and_single_point_inputs() {
        assert_eq!(dbscan(&[], 1.0, 2).expect("dbscan"), Vec::new());
        // Un punto solo con min_points 2 non ha vicinato sufficiente.
        let single = points(&[(1.0, 1.0)]);
        assert_eq!(dbscan(&single, 1.0, 2).expect("dbscan"), vec![None]);
        // Con min_points 1 e' un cluster di un elemento.
        assert_eq!(dbscan(&single, 1.0, 1).expect("dbscan"), vec![Some(0)]);
    }

    #[test]
    fn double_execution_gives_identical_labels() {
        let mut coords = cloud((0.0, 0.0), 40);
        coords.extend(cloud((50.0, 50.0), 40));
        coords.extend((0..10).map(|index| (200.0 + index as f64 * 7.0, -30.0)));
        let first = dbscan(&points(&coords), 0.5, 4).expect("prima");
        let second = dbscan(&points(&coords), 0.5, 4).expect("seconda");
        assert_eq!(first, second);
    }

    #[test]
    fn huge_eps_merges_everything_into_one_cluster() {
        let mut coords = cloud((0.0, 0.0), 10);
        coords.extend(cloud((1_000.0, 1_000.0), 10));
        let labels = dbscan(&points(&coords), 1e12, 3).expect("dbscan");
        assert!(labels.iter().all(|label| *label == Some(0)));
    }

    #[test]
    fn tiny_eps_makes_everything_noise() {
        let coords = cloud((0.0, 0.0), 10);
        let labels = dbscan(&points(&coords), 1e-9, 2).expect("dbscan");
        assert!(labels.iter().all(Option::is_none));
    }

    #[test]
    fn invalid_parameters_are_rejected() {
        let single = points(&[(0.0, 0.0)]);
        for bad in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            assert!(matches!(
                dbscan(&single, bad, 2),
                Err(ClusterError::InvalidParameter { name: "eps", .. })
            ));
        }
        assert!(matches!(
            dbscan(&single, 1.0, 0),
            Err(ClusterError::InvalidParameter {
                name: "min_points",
                ..
            })
        ));
    }

    #[test]
    fn non_point_geometries_are_rejected() {
        let geometries: Vec<Option<Geometry<f64>>> = vec![
            Some(Geometry::Point(Point::new(0.0, 0.0))),
            Some(Geometry::LineString(geo::LineString::from(vec![
                (0.0, 0.0),
                (1.0, 1.0),
            ]))),
        ];
        assert!(matches!(
            dbscan_nullable(&geometries, 1.0, 2),
            Err(ClusterError::UnsupportedGeometry {
                index: 1,
                found: "LineString"
            })
        ));
    }

    #[test]
    fn null_geometries_get_null_labels_without_participating() {
        // Due punti densi + un null in mezzo: il null non sposta gli indici.
        let geometries: Vec<Option<Geometry<f64>>> = vec![
            Some(Geometry::Point(Point::new(0.0, 0.0))),
            None,
            Some(Geometry::Point(Point::new(0.1, 0.0))),
        ];
        let labels = dbscan_nullable(&geometries, 0.5, 2).expect("dbscan");
        assert_eq!(labels, vec![Some(0), None, Some(0)]);
    }

    // --- adapter di colonna --------------------------------------------------

    #[test]
    fn dbscan_column_labels_rows_and_preserves_nulls() {
        let mut geometries: Vec<Option<Geometry<f64>>> = cloud((0.0, 0.0), 9)
            .iter()
            .map(|&(x, y)| Some(Geometry::Point(Point::new(x, y))))
            .collect();
        geometries.push(None);
        geometries.push(Some(Geometry::Point(Point::new(500.0, 500.0))));
        let cells = wkb_column(&geometries);
        let labels = dbscan_column(&cells, 0.5, 3).expect("colonna");
        assert_eq!(labels.len(), 11);
        assert!(labels[..9].iter().all(|label| *label == Some(0)));
        assert_eq!(labels[9], None, "geometria null -> null");
        assert_eq!(labels[10], None, "outlier -> noise");
        assert!(dbscan_column(&cells, 0.0, 3).is_err());
        assert!(dbscan_column(&cells, 0.5, 0).is_err());
    }

    #[test]
    fn dbscan_column_rejects_non_point_cells() {
        let line = Geometry::LineString(geo::LineString::from(vec![(0.0, 0.0), (1.0, 1.0)]));
        let cells = wkb_column(&[Some(line)]);
        assert!(matches!(
            dbscan_column(&cells, 1.0, 2),
            Err(PlenoraError::Contract(message))
                if message.contains("geo.cluster_dbscan") && message.contains("LineString")
        ));
    }

    // --- micro-benchmark (esplicito: cargo test -- --ignored) ----------------

    /// 100k punti: due nuvole dense su griglia + dispersione deterministica;
    /// stampa tempo e peak RSS (VmHWM, Linux). Esecuzione manuale.
    #[test]
    #[ignore = "micro-benchmark manuale"]
    fn dbscan_100k_benchmark() {
        let mut coords = cloud((0.0, 0.0), 50_000);
        coords.extend(cloud((1_000.0, 1_000.0), 49_000));
        // 1000 punti sparsi con un LCG deterministico.
        let mut state = 0x2545_F491_4F6C_DD1D_u64;
        for _ in 0..1_000 {
            state = state.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
            let x = (state >> 11) as f64 / (1u64 << 53) as f64 * 2_000.0;
            state = state.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
            let y = (state >> 11) as f64 / (1u64 << 53) as f64 * 2_000.0;
            coords.push((x, y));
        }
        assert_eq!(coords.len(), 100_000);
        let fixture = points(&coords);
        let start = std::time::Instant::now();
        let labels = dbscan(&fixture, 0.5, 4).expect("dbscan");
        let elapsed = start.elapsed();
        let clusters = labels
            .iter()
            .filter_map(|label| *label)
            .max()
            .map(|max| max + 1)
            .unwrap_or(0);
        let noise = labels.iter().filter(|label| label.is_none()).count();
        let peak_rss = std::fs::read_to_string("/proc/self/status")
            .ok()
            .and_then(|status| {
                status
                    .lines()
                    .find(|line| line.starts_with("VmHWM"))
                    .map(str::to_owned)
            })
            .unwrap_or_else(|| "VmHWM non disponibile".to_owned());
        eprintln!(
            "dbscan 100k punti: {:?}, {} cluster, {} noise, peak RSS: {}",
            elapsed, clusters, noise, peak_rss
        );
    }
}
