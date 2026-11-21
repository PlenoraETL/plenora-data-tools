//! Percorso 6.1 conservato come oracolo differenziale della voce memoria 14.
use super::*;
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
        (Geometry::LineString(a), Geometry::LineString(b)) => line_string_eq(a, b, tolerance),
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
    if value == 0.0 {
        0.0
    } else {
        value
    }
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
/// percorrenza. L'anello resta chiuso (ultimo vertice = primo) se lo e' in
/// ingresso.
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
    let mut interiors: Vec<LineString<f64>> =
        polygon.interiors().iter().map(normalize_ring).collect();
    interiors.sort_by(cmp_line_string);
    Polygon::new(exterior, interiors)
}

/// Normalizzazione topologica opzionale: rende il confronto
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
pub(super) fn normalize_geometry(geometry: &Geometry<f64>) -> Geometry<f64> {
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

/// Uguaglianza geometrica con tolleranza dichiarata.
///
/// Confronto per-coordinate sulla struttura, con normalizzazione topologica
/// opzionale quando `comparison.normalize` e' `true`.
#[must_use]
pub(super) fn geo_equals_with_tolerance(
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
