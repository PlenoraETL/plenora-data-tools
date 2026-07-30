//! Uguaglianza geometrica con tolleranza dichiarata e normalizzazione
//! topologica opzionale (ADR-0001, paragrafo "Uguaglianza geometrica").
//!
//! Il confronto e' geometrico, non byte-per-byte sul WKB: si lavora su
//! `geo::Geometry<f64>` gia' decodificate (il decode WKB resta a carico di
//! [`crate::geometry_from_wkb`], che applica il contratto strutturale).
//!
//! Metrica di tolleranza scelta: **confronto per-coordinate** dopo un
//! eventuale riallineamento canonico (normalizzazione topologica), non la
//! distanza di Hausdorff. Ogni coordinata di `left` e' confrontata con la
//! coordinata corrispondente di `right`: lo scarto assoluto per asse deve
//! essere `<= tolerance`. E' l'approccio piu' semplice e deterministico;
//! non richiede il calcolo di distanze punto-geometria. Limite noto: la
//! normalizzazione e' esatta (non tiene conto della tolleranza), quindi
//! rappresentazioni con coordinate che differiscono entro la tolleranza
//! possono canonizzare il punto iniziale degli anelli in modo diverso; in
//! quel caso il confronto resta conservativo (puo' dare `false`).
//!
//! Semantica scalare (ADR-0001): `-0.0` e `+0.0` sono uguali, `NaN` e'
//! uguale a `NaN` ai soli fini del confronto (la validazione dinamica
//! continua a rifiutare coordinate non finite in ingresso).

use std::cmp::Ordering;

use geo::{Coord, Geometry, LineString, Polygon};

/// Parametri dichiarati del confronto geometrico.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GeometryComparison {
    /// Tolleranza assoluta per asse sulle coordinate (attesa non negativa).
    pub tolerance: f64,
    /// Se `true`, entrambe le geometrie sono canonizzate con
    /// [`normalize_geometry`] prima del confronto, rendendolo insensibile a
    /// rappresentazioni equivalenti (orientamento degli anelli, punto
    /// iniziale, ordine dei componenti delle multi-geometrie).
    pub normalize: bool,
}

impl GeometryComparison {
    /// Confronto con tolleranza dichiarata, senza normalizzazione.
    #[must_use] 
    pub const fn new(tolerance: f64) -> Self {
        Self {
            tolerance,
            normalize: false,
        }
    }

    /// Abilita o disabilita la normalizzazione topologica opzionale.
    #[must_use] 
    pub const fn with_normalization(mut self, normalize: bool) -> Self {
        self.normalize = normalize;
        self
    }
}

impl Default for GeometryComparison {
    fn default() -> Self {
        Self::new(0.0)
    }
}

/// Confronto scalale secondo ADR-0001: `-0.0 == +0.0` (coperto da `==`),
/// `NaN == NaN`, altrimenti scarto assoluto entro la tolleranza.
fn scalar_eq(left: f64, right: f64, tolerance: f64) -> bool {
    if left == right {
        return true;
    }
    if left.is_nan() && right.is_nan() {
        return true;
    }
    (left - right).abs() <= tolerance
}

fn coord_eq(left: &Coord<f64>, right: &Coord<f64>, tolerance: f64) -> bool {
    scalar_eq(left.x, right.x, tolerance) && scalar_eq(left.y, right.y, tolerance)
}

fn line_string_eq(left: &LineString<f64>, right: &LineString<f64>, tolerance: f64) -> bool {
    left.0.len() == right.0.len()
        && left
            .0
            .iter()
            .zip(right.0.iter())
            .all(|(a, b)| coord_eq(a, b, tolerance))
}

fn polygon_eq(left: &Polygon<f64>, right: &Polygon<f64>, tolerance: f64) -> bool {
    line_string_eq(left.exterior(), right.exterior(), tolerance)
        && left.interiors().len() == right.interiors().len()
        && left
            .interiors()
            .iter()
            .zip(right.interiors().iter())
            .all(|(a, b)| line_string_eq(a, b, tolerance))
}

/// Uguaglianza strutturale per-coordinate: stesso tipo, stessa cardinalita'
/// dei componenti, coordinate corrispondenti entro la tolleranza.
fn geometry_eq(left: &Geometry<f64>, right: &Geometry<f64>, tolerance: f64) -> bool {
    match (left, right) {
        (Geometry::Point(a), Geometry::Point(b)) => coord_eq(&a.0, &b.0, tolerance),
        (Geometry::Line(a), Geometry::Line(b)) => {
            coord_eq(&a.start, &b.start, tolerance) && coord_eq(&a.end, &b.end, tolerance)
        }
        (Geometry::LineString(a), Geometry::LineString(b)) => {
            line_string_eq(a, b, tolerance)
        }
        (Geometry::Polygon(a), Geometry::Polygon(b)) => polygon_eq(a, b, tolerance),
        (Geometry::MultiPoint(a), Geometry::MultiPoint(b)) => {
            a.0.len() == b.0.len()
                && a.0
                    .iter()
                    .zip(b.0.iter())
                    .all(|(p, q)| coord_eq(&p.0, &q.0, tolerance))
        }
        (Geometry::MultiLineString(a), Geometry::MultiLineString(b)) => {
            a.0.len() == b.0.len()
                && a.0
                    .iter()
                    .zip(b.0.iter())
                    .all(|(p, q)| line_string_eq(p, q, tolerance))
        }
        (Geometry::MultiPolygon(a), Geometry::MultiPolygon(b)) => {
            a.0.len() == b.0.len()
                && a.0
                    .iter()
                    .zip(b.0.iter())
                    .all(|(p, q)| polygon_eq(p, q, tolerance))
        }
        (Geometry::GeometryCollection(a), Geometry::GeometryCollection(b)) => {
            a.0.len() == b.0.len()
                && a.0
                    .iter()
                    .zip(b.0.iter())
                    .all(|(p, q)| geometry_eq(p, q, tolerance))
        }
        (Geometry::Rect(a), Geometry::Rect(b)) => {
            coord_eq(&a.min(), &b.min(), tolerance) && coord_eq(&a.max(), &b.max(), tolerance)
        }
        (Geometry::Triangle(a), Geometry::Triangle(b)) => {
            coord_eq(&a.v1(), &b.v1(), tolerance)
                && coord_eq(&a.v2(), &b.v2(), tolerance)
                && coord_eq(&a.v3(), &b.v3(), tolerance)
        }
        _ => false,
    }
}

/// Chiave canonica di uno scalare per l'ordinamento: `-0.0` e' mappato su
/// `+0.0`, coerentemente con la semantica di uguaglianza.
fn canonical_scalar(value: f64) -> f64 {
    if value == 0.0 { 0.0 } else { value }
}

/// Ordine totale deterministico sulle coordinate (x poi y; `total_cmp`
/// rende deterministico anche il caso `NaN`).
fn cmp_coord(left: &Coord<f64>, right: &Coord<f64>) -> Ordering {
    canonical_scalar(left.x)
        .total_cmp(&canonical_scalar(right.x))
        .then_with(|| canonical_scalar(left.y).total_cmp(&canonical_scalar(right.y)))
}

/// Ordine lessicografico su sequenze di coordinate (prefisso piu' corto
/// ordinato prima).
fn cmp_coords(left: &[Coord<f64>], right: &[Coord<f64>]) -> Ordering {
    for (a, b) in left.iter().zip(right.iter()) {
        let ordering = cmp_coord(a, b);
        if ordering != Ordering::Equal {
            return ordering;
        }
    }
    left.len().cmp(&right.len())
}

fn cmp_line_string(left: &LineString<f64>, right: &LineString<f64>) -> Ordering {
    cmp_coords(&left.0, &right.0)
}

fn cmp_polygon(left: &Polygon<f64>, right: &Polygon<f64>) -> Ordering {
    cmp_line_string(left.exterior(), right.exterior()).then_with(|| {
        for (a, b) in left.interiors().iter().zip(right.interiors().iter()) {
            let ordering = cmp_line_string(a, b);
            if ordering != Ordering::Equal {
                return ordering;
            }
        }
        left.interiors().len().cmp(&right.interiors().len())
    })
}

/// Rotazione canonica di una sequenza aperta: si parte dalla prima
/// occorrenza della coordinata minima; a parita' di vertice iniziale si
/// sceglie la rotazione lessicograficamente minima.
fn best_rotation(open: &[Coord<f64>]) -> Vec<Coord<f64>> {
    // La rotazione 0 (la sequenza stessa) e' sempre un candidato valido:
    // l'insieme delle rotazioni non e' mai vuoto per costruzione.
    let mut best: Vec<Coord<f64>> = open.to_vec();
    for start in 1..open.len() {
        let candidate: Vec<Coord<f64>> = open[start..]
            .iter()
            .chain(open[..start].iter())
            .copied()
            .collect();
        if cmp_coords(&candidate, &best) == Ordering::Less {
            best = candidate;
        }
    }
    best
}

/// Canonizzazione di un anello: punto iniziale = vertice minimo,
/// orientamento = sequenza lessicograficamente minima tra i due sensi di
/// percorrenza. L'anello resta chiuso (ultimo vertice = primo) se lo era.
fn normalize_ring(ring: &LineString<f64>) -> LineString<f64> {
    let coords = &ring.0;
    let closed = coords.len() >= 2 && coords.first() == coords.last();
    let open: &[Coord<f64>] = if closed {
        &coords[..coords.len() - 1]
    } else {
        coords
    };
    if open.is_empty() {
        return ring.clone();
    }
    // Stesso anello percorso in senso opposto, a parita' di vertice iniziale.
    let reversed: Vec<Coord<f64>> = std::iter::once(open[0])
        .chain(open[1..].iter().rev().copied())
        .collect();
    let forward = best_rotation(open);
    let backward = best_rotation(&reversed);
    let mut canonical = if cmp_coords(&forward, &backward) == Ordering::Less {
        forward
    } else {
        backward
    };
    if closed {
        canonical.push(canonical[0]);
    }
    LineString(canonical)
}

/// Canonizzazione di una `LineString`: direzione lessicograficamente minima
/// (una linea percorsa al contrario e' la stessa rappresentazione
/// geometrica).
fn normalize_line_string(line: &LineString<f64>) -> LineString<f64> {
    let reversed: Vec<Coord<f64>> = line.0.iter().rev().copied().collect();
    if cmp_coords(&reversed, &line.0) == Ordering::Less {
        LineString(reversed)
    } else {
        line.clone()
    }
}

fn normalize_polygon(polygon: &Polygon<f64>) -> Polygon<f64> {
    let exterior = normalize_ring(polygon.exterior());
    let mut interiors: Vec<LineString<f64>> = polygon
        .interiors()
        .iter()
        .map(normalize_ring)
        .collect();
    interiors.sort_by(cmp_line_string);
    Polygon::new(exterior, interiors)
}

/// Normalizzazione topologica opzionale (ADR-0001): rende il confronto
/// insensibile a rappresentazioni equivalenti della stessa geometria.
///
/// Cosa canonizza:
/// - anelli poligonali: punto iniziale canonico (vertice minimo) e
///   orientamento canonico (sequenza lessicograficamente minima);
/// - buchi interni: stessa canonizzazione degli anelli, poi ordinati;
/// - `LineString` e `Line`: direzione canonica;
/// - `MultiPoint` / `MultiLineString` / `MultiPolygon`: componenti
///   canonizzati e ordinati.
///
/// L'ordine dei figli di una `GeometryCollection` eterogenea e' invece
/// preservato, cosi' come `Rect` e `Triangle`. La normalizzazione e' esatta
/// (non dipende dalla tolleranza) ed e' idempotente.
pub fn normalize_geometry(geometry: &Geometry<f64>) -> Geometry<f64> {
    match geometry {
        Geometry::Point(_) | Geometry::Rect(_) | Geometry::Triangle(_) => geometry.clone(),
        Geometry::Line(line) => {
            if cmp_coord(&line.end, &line.start) == Ordering::Less {
                Geometry::Line(geo::Line::new(line.end, line.start))
            } else {
                geometry.clone()
            }
        }
        Geometry::LineString(line) => Geometry::LineString(normalize_line_string(line)),
        Geometry::Polygon(polygon) => Geometry::Polygon(normalize_polygon(polygon)),
        Geometry::MultiPoint(points) => {
            let mut points = points.0.clone();
            points.sort_by(|a, b| cmp_coord(&a.0, &b.0));
            Geometry::MultiPoint(geo::MultiPoint(points))
        }
        Geometry::MultiLineString(lines) => {
            let mut lines: Vec<LineString<f64>> =
                lines.0.iter().map(normalize_line_string).collect();
            lines.sort_by(cmp_line_string);
            Geometry::MultiLineString(geo::MultiLineString(lines))
        }
        Geometry::MultiPolygon(polygons) => {
            let mut polygons: Vec<Polygon<f64>> =
                polygons.0.iter().map(normalize_polygon).collect();
            polygons.sort_by(cmp_polygon);
            Geometry::MultiPolygon(geo::MultiPolygon(polygons))
        }
        Geometry::GeometryCollection(collection) => Geometry::GeometryCollection(
            geo::GeometryCollection(collection.0.iter().map(normalize_geometry).collect()),
        ),
    }
}

/// Uguaglianza geometrica con tolleranza dichiarata (ADR-0001, livello 1):
/// confronto per-coordinate sulla struttura, con normalizzazione topologica
/// opzionale quando `comparison.normalize` e' `true`.
#[must_use] 
pub fn geo_equals_with_tolerance(
    left: &Geometry<f64>,
    right: &Geometry<f64>,
    comparison: GeometryComparison,
) -> bool {
    if comparison.normalize {
        geometry_eq(
            &normalize_geometry(left),
            &normalize_geometry(right),
            comparison.tolerance,
        )
    } else {
        geometry_eq(left, right, comparison.tolerance)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use geo::{
        line_string, polygon, GeometryCollection, Line, MultiLineString, MultiPoint, MultiPolygon,
        Point, Rect, Triangle,
    };

    const EXACT: GeometryComparison = GeometryComparison {
        tolerance: 0.0,
        normalize: false,
    };
    const NORMALIZED: GeometryComparison = GeometryComparison {
        tolerance: 0.0,
        normalize: true,
    };

    fn square_cw() -> Geometry<f64> {
        Geometry::Polygon(polygon![
            (x: 0.0, y: 0.0),
            (x: 0.0, y: 2.0),
            (x: 2.0, y: 2.0),
            (x: 2.0, y: 0.0),
            (x: 0.0, y: 0.0),
        ])
    }

    #[test]
    fn anelli_con_orientamento_opposto_richiedono_la_normalizzazione() {
        let ccw = Geometry::Polygon(polygon![
            (x: 0.0, y: 0.0),
            (x: 2.0, y: 0.0),
            (x: 2.0, y: 2.0),
            (x: 0.0, y: 2.0),
            (x: 0.0, y: 0.0),
        ]);
        assert!(!geo_equals_with_tolerance(&square_cw(), &ccw, EXACT));
        assert!(geo_equals_with_tolerance(&square_cw(), &ccw, NORMALIZED));
    }

    #[test]
    fn punto_iniziale_diverso_dell_anello_richiede_la_normalizzazione() {
        let rotated = Geometry::Polygon(polygon![
            (x: 2.0, y: 2.0),
            (x: 2.0, y: 0.0),
            (x: 0.0, y: 0.0),
            (x: 0.0, y: 2.0),
            (x: 2.0, y: 2.0),
        ]);
        assert!(!geo_equals_with_tolerance(&square_cw(), &rotated, EXACT));
        assert!(geo_equals_with_tolerance(
            &square_cw(),
            &rotated,
            NORMALIZED
        ));
    }

    #[test]
    fn ordine_dei_buchi_e_dei_componenti_multi_e_canonizzato() {
        let shell = vec![
            Coord { x: 0.0, y: 0.0 },
            Coord { x: 10.0, y: 0.0 },
            Coord { x: 10.0, y: 10.0 },
            Coord { x: 0.0, y: 10.0 },
            Coord { x: 0.0, y: 0.0 },
        ];
        let hole_a = LineString(vec![
            Coord { x: 1.0, y: 1.0 },
            Coord { x: 2.0, y: 1.0 },
            Coord { x: 2.0, y: 2.0 },
            Coord { x: 1.0, y: 1.0 },
        ]);
        let hole_b = LineString(vec![
            Coord { x: 5.0, y: 5.0 },
            Coord { x: 6.0, y: 5.0 },
            Coord { x: 6.0, y: 6.0 },
            Coord { x: 5.0, y: 5.0 },
        ]);
        let holes_ab = Geometry::Polygon(Polygon::new(
            LineString(shell.clone()),
            vec![hole_a.clone(), hole_b.clone()],
        ));
        let holes_ba = Geometry::Polygon(Polygon::new(LineString(shell), vec![hole_b, hole_a]));
        assert!(!geo_equals_with_tolerance(&holes_ab, &holes_ba, EXACT));
        assert!(geo_equals_with_tolerance(&holes_ab, &holes_ba, NORMALIZED));

        let forward = Geometry::MultiPoint(MultiPoint::new(vec![
            Point::new(3.0, 3.0),
            Point::new(1.0, 1.0),
        ]));
        let backward = Geometry::MultiPoint(MultiPoint::new(vec![
            Point::new(1.0, 1.0),
            Point::new(3.0, 3.0),
        ]));
        assert!(!geo_equals_with_tolerance(&forward, &backward, EXACT));
        assert!(geo_equals_with_tolerance(&forward, &backward, NORMALIZED));
    }

    #[test]
    fn meno_zero_e_piu_zero_sono_uguali_senza_tolleranza() {
        let negative = Geometry::Point(Point::new(-0.0, 0.0));
        let positive = Geometry::Point(Point::new(0.0, -0.0));
        assert!(geo_equals_with_tolerance(&negative, &positive, EXACT));
    }

    #[test]
    fn nan_e_uguale_a_nan_solo_ai_fini_del_confronto() {
        let nan_a = Geometry::Point(Point::new(f64::NAN, 1.0));
        let nan_b = Geometry::Point(Point::new(f64::NAN, 1.0));
        assert!(geo_equals_with_tolerance(&nan_a, &nan_b, EXACT));
        let finite = Geometry::Point(Point::new(1.0, 1.0));
        assert!(!geo_equals_with_tolerance(&nan_a, &finite, EXACT));
        assert!(!geo_equals_with_tolerance(
            &nan_a,
            &finite,
            GeometryComparison::new(f64::INFINITY)
        ));
    }

    #[test]
    fn la_tolleranza_dichiarata_delimita_l_uguaglianza() {
        let reference = Geometry::Point(Point::new(1.0, 2.0));
        let near = Geometry::Point(Point::new(1.0 + 1e-3, 2.0 - 1e-3));
        assert!(!geo_equals_with_tolerance(
            &reference,
            &near,
            GeometryComparison::new(1e-4)
        ));
        assert!(geo_equals_with_tolerance(
            &reference,
            &near,
            GeometryComparison::new(1e-2)
        ));
    }

    #[test]
    fn tipi_o_cardinalita_diverse_non_sono_uguali() {
        let point = Geometry::Point(Point::new(0.0, 0.0));
        let line = Geometry::LineString(line_string![(x: 0.0, y: 0.0), (x: 1.0, y: 1.0)]);
        assert!(!geo_equals_with_tolerance(&point, &line, NORMALIZED));

        let single = Geometry::MultiPoint(MultiPoint::new(vec![Point::new(1.0, 1.0)]));
        let double = Geometry::MultiPoint(MultiPoint::new(vec![
            Point::new(1.0, 1.0),
            Point::new(2.0, 2.0),
        ]));
        assert!(!geo_equals_with_tolerance(&single, &double, NORMALIZED));
    }

    #[test]
    fn la_normalizzazione_e_idempotente() {
        let rotated = Geometry::Polygon(polygon![
            (x: 2.0, y: 2.0),
            (x: 2.0, y: 0.0),
            (x: 0.0, y: 0.0),
            (x: 0.0, y: 2.0),
            (x: 2.0, y: 2.0),
        ]);
        let once = normalize_geometry(&rotated);
        let twice = normalize_geometry(&once);
        assert!(geo_equals_with_tolerance(&once, &twice, EXACT));
        assert_eq!(once, twice);
    }

    #[test]
    fn il_builder_dichiara_tolleranza_e_normalizzazione() {
        let comparison = GeometryComparison::new(0.5).with_normalization(true);
        assert_eq!(
            comparison,
            GeometryComparison {
                tolerance: 0.5,
                normalize: true,
            }
        );
        assert_eq!(
            GeometryComparison::default(),
            GeometryComparison::new(0.0)
        );
    }

    #[test]
    fn segmenti_rettangoli_e_triangoli_si_confrontano_per_coordinate() {
        let line_a = Geometry::Line(Line::new(Coord { x: 0.0, y: 0.0 }, Coord { x: 1.0, y: 1.0 }));
        let line_b = Geometry::Line(Line::new(Coord { x: 0.0, y: 0.0 }, Coord { x: 1.0, y: 1.0 }));
        let line_far = Geometry::Line(Line::new(Coord { x: 0.0, y: 0.0 }, Coord { x: 9.0, y: 9.0 }));
        assert!(geo_equals_with_tolerance(&line_a, &line_b, EXACT));
        assert!(!geo_equals_with_tolerance(&line_a, &line_far, EXACT));

        let rect_a = Geometry::Rect(Rect::new(Coord { x: 0.0, y: 0.0 }, Coord { x: 2.0, y: 2.0 }));
        let rect_b = Geometry::Rect(Rect::new(Coord { x: 0.0, y: 0.0 }, Coord { x: 2.0, y: 2.0 }));
        let rect_far = Geometry::Rect(Rect::new(Coord { x: 0.0, y: 0.0 }, Coord { x: 3.0, y: 3.0 }));
        assert!(geo_equals_with_tolerance(&rect_a, &rect_b, EXACT));
        assert!(!geo_equals_with_tolerance(&rect_a, &rect_far, EXACT));

        let triangle_a = Geometry::Triangle(Triangle(
            Coord { x: 0.0, y: 0.0 },
            Coord { x: 1.0, y: 0.0 },
            Coord { x: 0.0, y: 1.0 },
        ));
        let triangle_b = Geometry::Triangle(Triangle(
            Coord { x: 0.0, y: 0.0 },
            Coord { x: 1.0, y: 0.0 },
            Coord { x: 0.0, y: 1.0 },
        ));
        let triangle_far = Geometry::Triangle(Triangle(
            Coord { x: 0.0, y: 0.0 },
            Coord { x: 1.0, y: 0.0 },
            Coord { x: 5.0, y: 5.0 },
        ));
        assert!(geo_equals_with_tolerance(&triangle_a, &triangle_b, EXACT));
        assert!(!geo_equals_with_tolerance(&triangle_a, &triangle_far, EXACT));
        // La normalizzazione non riordina Rect/Triangle: vertici permutati
        // restano diversi.
        let triangle_permuted = Geometry::Triangle(Triangle(
            Coord { x: 1.0, y: 0.0 },
            Coord { x: 0.0, y: 1.0 },
            Coord { x: 0.0, y: 0.0 },
        ));
        assert!(!geo_equals_with_tolerance(
            &triangle_a,
            &triangle_permuted,
            NORMALIZED
        ));
    }

    #[test]
    fn la_direzione_di_linee_e_linestring_e_canonizzata() {
        // Line: end < start -> ribaltata dalla normalizzazione.
        let flipped = Geometry::Line(Line::new(Coord { x: 1.0, y: 1.0 }, Coord { x: 0.0, y: 0.0 }));
        let canonical = Geometry::Line(Line::new(Coord { x: 0.0, y: 0.0 }, Coord { x: 1.0, y: 1.0 }));
        assert!(!geo_equals_with_tolerance(&flipped, &canonical, EXACT));
        assert!(geo_equals_with_tolerance(&flipped, &canonical, NORMALIZED));
        let normalized = normalize_geometry(&flipped);
        assert_eq!(normalized, canonical);

        // LineString: direzione lessicograficamente minima.
        let forward = Geometry::LineString(line_string![(x: 0.0, y: 0.0), (x: 1.0, y: 1.0)]);
        let backward = Geometry::LineString(line_string![(x: 1.0, y: 1.0), (x: 0.0, y: 0.0)]);
        assert!(!geo_equals_with_tolerance(&forward, &backward, EXACT));
        assert!(geo_equals_with_tolerance(&forward, &backward, NORMALIZED));
    }

    #[test]
    fn multilinestring_e_multipolygon_sono_ordinati_nella_normalizzazione() {
        let short = line_string![(x: 0.0, y: 0.0), (x: 1.0, y: 1.0)];
        // Prefisso di `long`: l'ordinamento lessicografico decide sulla
        // lunghezza solo dopo il prefisso comune.
        let long = line_string![(x: 0.0, y: 0.0), (x: 1.0, y: 1.0), (x: 2.0, y: 2.0)];
        let forward = Geometry::MultiLineString(MultiLineString::new(vec![long.clone(), short.clone()]));
        let backward = Geometry::MultiLineString(MultiLineString::new(vec![short, long]));
        assert!(!geo_equals_with_tolerance(&forward, &backward, EXACT));
        assert!(geo_equals_with_tolerance(&forward, &backward, NORMALIZED));

        // MultiPolygon: stessa shell, buchi diversi -> l'ordine canonico
        // dei poligoni si decide sugli interni (cmp_polygon oltre l'esterno).
        let shell = || {
            LineString(vec![
                Coord { x: 0.0, y: 0.0 },
                Coord { x: 10.0, y: 0.0 },
                Coord { x: 10.0, y: 10.0 },
                Coord { x: 0.0, y: 10.0 },
                Coord { x: 0.0, y: 0.0 },
            ])
        };
        let hole = |offset: f64| {
            LineString(vec![
                Coord { x: offset, y: offset },
                Coord { x: offset + 1.0, y: offset },
                Coord { x: offset + 1.0, y: offset + 1.0 },
                Coord { x: offset, y: offset },
            ])
        };
        let poly_low = Polygon::new(shell(), vec![hole(1.0)]);
        let poly_high = Polygon::new(shell(), vec![hole(5.0)]);
        let forward = Geometry::MultiPolygon(MultiPolygon::new(vec![poly_high.clone(), poly_low.clone()]));
        let backward = Geometry::MultiPolygon(MultiPolygon::new(vec![poly_low, poly_high]));
        assert!(!geo_equals_with_tolerance(&forward, &backward, EXACT));
        assert!(geo_equals_with_tolerance(&forward, &backward, NORMALIZED));

        // Buchi in prefisso comune: l'ordine si decide sul numero di buchi
        // (i primi confrontati risultano uguali, poi vince il piu' corto).
        let one_hole = Polygon::new(shell(), vec![hole(1.0)]);
        let two_holes = Polygon::new(shell(), vec![hole(1.0), hole(5.0)]);
        let unsorted = Geometry::MultiPolygon(MultiPolygon::new(vec![two_holes, one_hole]));
        let normalized = normalize_geometry(&unsorted);
        let Geometry::MultiPolygon(polygons) = &normalized else {
            panic!("atteso MultiPolygon, ottenuto {normalized:?}");
        };
        assert_eq!(polygons.0[0].interiors().len(), 1);
        assert_eq!(polygons.0[1].interiors().len(), 2);
    }

    #[test]
    fn l_ordine_dei_figli_della_collection_e_preservato() {
        let point = Geometry::Point(Point::new(1.0, 1.0));
        let line = Geometry::LineString(line_string![(x: 0.0, y: 0.0), (x: 2.0, y: 2.0)]);
        let forward = Geometry::GeometryCollection(GeometryCollection::from(vec![
            point.clone(),
            line.clone(),
        ]));
        let backward =
            Geometry::GeometryCollection(GeometryCollection::from(vec![line, point]));
        // Eterogenea: i figli NON sono riordinati, la collezione resta diversa.
        assert!(!geo_equals_with_tolerance(&forward, &backward, NORMALIZED));

        // Ma la normalizzazione scende ricorsivamente nei figli.
        let rotated_child = Geometry::Polygon(polygon![
            (x: 2.0, y: 2.0),
            (x: 2.0, y: 0.0),
            (x: 0.0, y: 0.0),
            (x: 0.0, y: 2.0),
            (x: 2.0, y: 2.0),
        ]);
        let with_rotated =
            Geometry::GeometryCollection(GeometryCollection::from(vec![rotated_child]));
        let with_canonical =
            Geometry::GeometryCollection(GeometryCollection::from(vec![square_cw()]));
        assert!(!geo_equals_with_tolerance(&with_rotated, &with_canonical, EXACT));
        assert!(geo_equals_with_tolerance(
            &with_rotated,
            &with_canonical,
            NORMALIZED
        ));
    }

    #[test]
    fn anelli_aperti_e_vuoti_sono_canonizzati_senza_chiusura() {
        // Anello aperto: la canonizzazione non chiude, ruota e orienta.
        let open_a = Polygon::new(
            LineString(vec![
                Coord { x: 1.0, y: 1.0 },
                Coord { x: 0.0, y: 0.0 },
                Coord { x: 2.0, y: 0.0 },
            ]),
            Vec::new(),
        );
        let open_b = Polygon::new(
            LineString(vec![
                Coord { x: 0.0, y: 0.0 },
                Coord { x: 2.0, y: 0.0 },
                Coord { x: 1.0, y: 1.0 },
            ]),
            Vec::new(),
        );
        let a = Geometry::Polygon(open_a);
        let b = Geometry::Polygon(open_b);
        assert!(!geo_equals_with_tolerance(&a, &b, EXACT));
        assert!(geo_equals_with_tolerance(&a, &b, NORMALIZED));

        // Anello vuoto: restato invariato, nessuna rotazione possibile.
        let empty = Geometry::Polygon(Polygon::new(LineString(Vec::new()), Vec::new()));
        let normalized = normalize_geometry(&empty);
        assert_eq!(normalized, empty);
    }
}
