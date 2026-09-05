//! Stima della memoria nativa delle geometrie decodificate
//! (architettura.md#memoria).
//!
//! architettura.md#memoria ("Resource accounting e reservation protocol") prescrive che la
//! memoria nativa delle geometrie decodificate (oggi `geo::Geometry<f64>`,
//! in futuro strutture GEOS nel backend feature-gated) sia **stimata e
//! dichiarata come stima**, mai presentata come conteggio preciso. Questo
//! modulo implementa l'euristica di stima per il livello geo:
//!
//! - [`estimate_geometry_native_bytes`]: STIMA dei byte nativi di una singola
//!   geometria decodificata;
//! - [`estimate_geometries_native_bytes`]: aggregazione su una sequenza di
//!   geometrie (es. colonna decodificata di un batch);
//! - [`DecodedNativeBytesEstimate`]: accumulatore thread-safe per i punti di
//!   decode (adapter Arrow), pensato per essere letto dal governor come
//!   metrica "stimata" (separata da riservato/osservato, architettura.md#memoria).
//!
//! # Formula di stima (dichiarata)
//!
//! Tutti i valori restituiti da questo modulo sono STIME euristiche, non
//! misure. La formula per tipo e':
//!
//! - ogni coordinata XY: [`COORD_XY_BYTES`] = 16 byte (2 x `size_of::<f64>()`);
//! - ogni nodo geometrico (variante di `Geometry`): [`STRUCT_OVERHEAD_BYTES`]
//!   = 32 byte di overhead dichiarato (tag enum + header della struttura
//!   nativa, comprensivo di allineamento);
//! - ogni `Vec` di coordinate/anelli/componenti: [`VEC_OVERHEAD_BYTES`] =
//!   24 byte (puntatore + lunghezza + capacita').
//!
//! Per tipo:
//!
//! - `Point`: STRUCT + 1 coordinata;
//! - `Line`/`Rect`: STRUCT + 2 coordinate; `Triangle`: STRUCT + 3 coordinate;
//! - `LineString` (e ogni anello): STRUCT + VEC + N coordinate;
//! - `Polygon`: STRUCT + VEC (vettore anelli interni) + anello esterno +
//!   un anello per ogni buco;
//! - `MultiPoint`: STRUCT + VEC + N coordinate (i punti sono inline nel
//!   vettore);
//! - `MultiLineString`: STRUCT + VEC + per ogni figlio VEC + N coordinate;
//! - `MultiPolygon`: STRUCT + VEC + per ogni figlio il corpo poligonale
//!   (come `Polygon`, senza ripetere il tag enum);
//! - `GeometryCollection`: STRUCT + VEC + somma ricorsiva delle stime dei
//!   figli.
//!
//! La stima e' intenzionalmente una **approssimazione per eccesso controllato
//! del solo costo dei dati**: non modella la capacita' allocata in eccesso
//! dei `Vec`, le indirezioni di un eventuale backend GEOS ne' le strutture
//! ausiliarie (indici spaziali, envelope precalcolati). Va quindi riportata
//! nelle metriche come "memoria nativa stimata", mai come conteggio preciso
//! (architettura.md#memoria, paragrafo "Perimetro di `max_governed_memory_bytes`").

use std::sync::atomic::{AtomicU64, Ordering};

use geo::Geometry;

/// Byte stimati per coordinata XY (2 x `size_of::<f64>()`).
pub const COORD_XY_BYTES: u64 = 16;

/// Overhead di struttura STIMATO per nodo geometrico (tag enum + header
/// nativo, allineamento incluso). Euristica dichiarata, non una misura.
pub const STRUCT_OVERHEAD_BYTES: u64 = 32;

/// Overhead STIMATO per `Vec` (puntatore + lunghezza + capacita').
pub const VEC_OVERHEAD_BYTES: u64 = 24;

/// Stima (saturating) di `N` coordinate XY.
const fn coords_bytes(count: usize) -> u64 {
    (count as u64).saturating_mul(COORD_XY_BYTES)
}

/// Stima di una sequenza di coordinate allocata in un `Vec` (`LineString` o
/// anello poligonale), con l'overhead di nodo del contenitore.
const fn coord_vec_bytes(container_overhead: u64, count: usize) -> u64 {
    container_overhead
        .saturating_add(VEC_OVERHEAD_BYTES)
        .saturating_add(coords_bytes(count))
}

/// Corpo di un poligono (senza il tag enum della variante `Geometry`).
fn polygon_body_bytes(polygon: &geo::Polygon<f64>) -> u64 {
    let exterior = coord_vec_bytes(0, polygon.exterior().0.len());
    let interiors = polygon.interiors().iter().fold(0_u64, |total, ring| {
        total.saturating_add(coord_vec_bytes(0, ring.0.len()))
    });
    VEC_OVERHEAD_BYTES
        .saturating_add(exterior)
        .saturating_add(interiors)
}

/// STIMA euristica dei byte nativi di una geometria decodificata.
///
/// **Non e' un conteggio preciso**: e' l'euristica dichiarata da architettura.md#memoria
/// per la memoria nativa delle geometrie (formula nel doc-comment di
/// modulo). Va esposta nelle metriche come "stimata", separata da memoria
/// riservata e osservata.
#[must_use]
pub fn estimate_geometry_native_bytes(geometry: &Geometry<f64>) -> u64 {
    let node = STRUCT_OVERHEAD_BYTES;
    match geometry {
        Geometry::Point(_) => node.saturating_add(COORD_XY_BYTES),
        Geometry::Line(_) | Geometry::Rect(_) => node.saturating_add(2 * COORD_XY_BYTES),
        Geometry::Triangle(_) => node.saturating_add(3 * COORD_XY_BYTES),
        Geometry::LineString(line) => coord_vec_bytes(node, line.0.len()),
        Geometry::Polygon(polygon) => node.saturating_add(polygon_body_bytes(polygon)),
        Geometry::MultiPoint(points) => node
            .saturating_add(VEC_OVERHEAD_BYTES)
            .saturating_add(coords_bytes(points.0.len())),
        Geometry::MultiLineString(lines) => {
            let children = lines.0.iter().fold(0_u64, |total, line| {
                total.saturating_add(coord_vec_bytes(0, line.0.len()))
            });
            node.saturating_add(VEC_OVERHEAD_BYTES)
                .saturating_add(children)
        }
        Geometry::MultiPolygon(polygons) => {
            let children = polygons.0.iter().fold(0_u64, |total, polygon| {
                total.saturating_add(polygon_body_bytes(polygon))
            });
            node.saturating_add(VEC_OVERHEAD_BYTES)
                .saturating_add(children)
        }
        Geometry::GeometryCollection(collection) => {
            let children = collection.0.iter().fold(0_u64, |total, child| {
                total.saturating_add(estimate_geometry_native_bytes(child))
            });
            node.saturating_add(VEC_OVERHEAD_BYTES)
                .saturating_add(children)
        }
    }
}

/// STIMA aggregata dei byte nativi di una sequenza di geometrie decodificate
/// (es. la colonna geometria di un `RecordBatch` dopo il decode).
///
/// Stessa natura dichiarata di [`estimate_geometry_native_bytes`]: somma
/// saturante delle stime per cella, da riportare come "memoria nativa
/// stimata" (architettura.md#memoria).
pub fn estimate_geometries_native_bytes<'a>(
    geometries: impl IntoIterator<Item = &'a Geometry<f64>>,
) -> u64 {
    geometries.into_iter().fold(0_u64, |total, geometry| {
        total.saturating_add(estimate_geometry_native_bytes(geometry))
    })
}

/// Accumulatore thread-safe della STIMA dei byte nativi decodificati
/// (architettura.md#memoria: metrica "stimata", separata da riservato/osservato).
///
/// Punto di accumulo naturale per gli adapter che decodificano celle WKB in
/// parallelo (rayon): ogni cella decodificata contribuisce la propria stima
/// e il totale puo' essere letto in qualsiasi momento dal governor.
/// L'integrazione con `plenora-engine` e' volutamente rimandata: qui vive
/// solo il contatore e le funzioni di stima.
#[derive(Debug, Default)]
pub struct DecodedNativeBytesEstimate {
    total: AtomicU64,
}

impl DecodedNativeBytesEstimate {
    /// Accumulatore vuoto (stima corrente zero).
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Registra la STIMA di una geometria appena decodificata.
    pub fn record(&self, geometry: &Geometry<f64>) {
        self.add_bytes(estimate_geometry_native_bytes(geometry));
    }

    /// Registra una STIMA gia' calcolata (saturating lato lettura).
    pub fn add_bytes(&self, bytes: u64) {
        self.total.fetch_add(bytes, Ordering::Relaxed);
    }

    /// STIMA corrente accumulata (byte).
    #[must_use]
    pub fn total(&self) -> u64 {
        self.total.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use geo::{line_string, polygon, LineString, Point};

    /// STIMA nota di un punto: STRUCT + una coordinata XY.
    #[test]
    fn point_estimate_matches_the_declared_formula() {
        let point = Geometry::Point(Point::new(1.0, 2.0));
        assert_eq!(
            estimate_geometry_native_bytes(&point),
            STRUCT_OVERHEAD_BYTES + COORD_XY_BYTES
        );
    }

    /// STIMA nota di una `LineString`: STRUCT + VEC + N coordinate.
    #[test]
    fn linestring_estimate_matches_the_declared_formula() {
        let line = Geometry::LineString(line_string![
            (x: 0.0, y: 0.0),
            (x: 1.0, y: 0.0),
            (x: 1.0, y: 1.0),
            (x: 2.0, y: 1.0),
        ]);
        assert_eq!(
            estimate_geometry_native_bytes(&line),
            STRUCT_OVERHEAD_BYTES + VEC_OVERHEAD_BYTES + 4 * COORD_XY_BYTES
        );
    }

    /// STIMA nota di un poligono con un buco: STRUCT + VEC interni + anello
    /// esterno (5 coordinate chiuse) + anello interno (4 coordinate).
    #[test]
    fn polygon_with_hole_estimate_counts_every_ring() {
        let with_hole = Geometry::Polygon(polygon!(
            exterior: [
                (x: 0.0, y: 0.0), (x: 8.0, y: 0.0), (x: 8.0, y: 8.0),
                (x: 0.0, y: 8.0), (x: 0.0, y: 0.0),
            ],
            interiors: [[
                (x: 2.0, y: 2.0), (x: 4.0, y: 2.0),
                (x: 4.0, y: 4.0), (x: 2.0, y: 2.0),
            ]],
        ));
        let expected = STRUCT_OVERHEAD_BYTES
            + VEC_OVERHEAD_BYTES
            + (VEC_OVERHEAD_BYTES + 5 * COORD_XY_BYTES)
            + (VEC_OVERHEAD_BYTES + 4 * COORD_XY_BYTES);
        assert_eq!(estimate_geometry_native_bytes(&with_hole), expected);

        let without_hole = Geometry::Polygon(polygon![
            (x: 0.0, y: 0.0), (x: 8.0, y: 0.0), (x: 8.0, y: 8.0),
            (x: 0.0, y: 8.0), (x: 0.0, y: 0.0),
        ]);
        assert!(
            estimate_geometry_native_bytes(&with_hole)
                > estimate_geometry_native_bytes(&without_hole)
        );
    }

    /// Monotonia: a parita' di tipo, piu' coordinate -> STIMA maggiore.
    #[test]
    fn estimate_is_monotonic_in_the_coordinate_count() {
        let short = Geometry::LineString(LineString::from(vec![(0.0, 0.0), (1.0, 1.0)]));
        let long = Geometry::LineString(LineString::from(
            (0..10)
                .map(|index| (f64::from(index), f64::from(index)))
                .collect::<Vec<_>>(),
        ));
        assert!(estimate_geometry_native_bytes(&long) > estimate_geometry_native_bytes(&short));

        let single = Geometry::MultiPoint(geo::MultiPoint::new(vec![Point::new(0.0, 0.0)]));
        let many = Geometry::MultiPoint(geo::MultiPoint::new(
            (0..5)
                .map(|index| Point::new(f64::from(index), 0.0))
                .collect(),
        ));
        assert!(estimate_geometry_native_bytes(&many) > estimate_geometry_native_bytes(&single));
    }

    /// Collezioni e multi-geometrie: la STIMA ricorsiva domina i figli.
    #[test]
    fn collection_estimate_aggregates_children_recursively() {
        let point = Geometry::Point(Point::new(1.0, 2.0));
        let collection = Geometry::GeometryCollection(geo::GeometryCollection(vec![
            point.clone(),
            point.clone(),
        ]));
        let children = 2 * estimate_geometry_native_bytes(&point);
        assert_eq!(
            estimate_geometry_native_bytes(&collection),
            STRUCT_OVERHEAD_BYTES + VEC_OVERHEAD_BYTES + children
        );
    }

    /// Aggregazione su sequenza: somma saturante delle STIME per geometria.
    #[test]
    fn aggregate_estimate_sums_per_geometry_estimates() {
        let geometries = vec![
            Geometry::Point(Point::new(0.0, 0.0)),
            Geometry::LineString(line_string![(x: 0.0, y: 0.0), (x: 1.0, y: 1.0)]),
        ];
        let expected = estimate_geometries_native_bytes(&geometries);
        let sum = geometries
            .iter()
            .map(estimate_geometry_native_bytes)
            .sum::<u64>();
        assert_eq!(expected, sum);
        assert_eq!(estimate_geometries_native_bytes(&[]), 0);
    }

    /// L'accumulatore thread-safe registra le STIME delle celle decodificate.
    #[test]
    fn accumulator_tracks_the_running_estimate() {
        let accumulator = DecodedNativeBytesEstimate::new();
        assert_eq!(accumulator.total(), 0);
        let point = Geometry::Point(Point::new(3.0, 4.0));
        accumulator.record(&point);
        accumulator.record(&point);
        assert_eq!(
            accumulator.total(),
            2 * estimate_geometry_native_bytes(&point)
        );
    }
}
