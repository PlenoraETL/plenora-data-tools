//! Booleane binarie planari XY con almeno un Point/MultiPoint.
//!
//! Profilo parziale GOP-014/015: l'altro operando e' Point, MultiPoint,
//! LineString o Polygon. Stesso CRS e assi a carico del chiamante.
//! Nessuno snap: appartenenza razionale esatta delle coordinate f64 finite.
//! Non modifica le booleane poligonali esistenti.

use super::{point, validate_structure, Shape};
use crate::noding::{CountedVec, Work};
use crate::predicates::{PredicateError, RelateLimits};
use geo::{
    Geometry, GeometryCollection, LineString, MultiLineString, MultiPoint, MultiPolygon, Point,
    Polygon,
};
use std::sync::atomic::AtomicBool;

/// Operazione insiemistica OGC SFA 1.2.1, 6.1.2 e 6.1.15.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PointBooleanOp {
    /// Unione, con eliminazione dei siti coperti dalla componente superiore.
    Union,
    /// Siti comuni, inclusi i confini.
    Intersection,
    /// Differenza direzionale left meno right, con chiusura GOP-015.
    Difference,
}

/// Errori tipizzati senza coordinate o valori degli operandi.
#[derive(Debug, thiserror::Error)]
pub enum PointBooleanError {
    /// Tipi esterni al profilo puntuale binario dichiarato.
    #[error("operandi non supportati dal profilo booleano puntuale")]
    UnsupportedOperands,
    /// Il filtro ammette soltanto 0, 1, 2.
    #[error("dimensione di uscita non valida")]
    InvalidOutputDimension,
    /// Validazione, risorse e interruzioni del motore indicizzato.
    #[error(transparent)]
    Predicate(#[from] PredicateError),
}

const fn dimension(g: &Geometry<f64>) -> Result<u8, PointBooleanError> {
    match g {
        Geometry::Point(_) | Geometry::MultiPoint(_) => Ok(0),
        Geometry::LineString(_) => Ok(1),
        Geometry::Polygon(_) => Ok(2),
        _ => Err(PointBooleanError::UnsupportedOperands),
    }
}

/// Esegue una booleana con gli stessi limiti, deadline e flag di relate.
///
/// Il risultato omogeneo e' sempre un contenitore multi. L'unione mista
/// contiene `MultiPoint` e poi il multi superiore, senza componenti vuote.
/// I siti sono unici e ordinati lessicograficamente XY, con il segno degli zeri del primo vertice in ingresso.
/// Le coordinate della componente superiore conservano l'ordine d'ingresso.
/// Il vuoto generale e' `GeometryCollection` vuota; con filtro esplicito e'
/// il multi vuoto della dimensione richiesta. Il filtro conserva componenti,
/// non estrae confini. Nessun filtro implicito: None conserva tutto.
/// Linea/poligono meno punti conserva la componente superiore per chiusura
/// dei residui (GOP-015). Sono supportati anche gli operandi vuoti tipizzati.
/// Tutte le prenotazioni d'uscita sono contate fino al ritorno; nessun prefisso
/// viene restituito dopo un errore. Deadline scaduta precede cancellazione.
///
/// # Errors
/// Tipi non supportati, filtro invalido, geometrie invalide/non finite,
/// limiti superati, allocazione fallita, cancellazione o deadline.
pub(crate) fn boolean_with_limits(
    left: &Geometry<f64>,
    right: &Geometry<f64>,
    operation: PointBooleanOp,
    output_dimension: Option<u8>,
    limits: RelateLimits,
    cancelled: Option<&AtomicBool>,
) -> Result<Geometry<f64>, PointBooleanError> {
    let mut work = Work::new(limits, cancelled);
    execute(left, right, operation, output_dimension, &mut work)
}

#[allow(clippy::too_many_lines)]
fn execute(
    left: &Geometry<f64>,
    right: &Geometry<f64>,
    operation: PointBooleanOp,
    filter: Option<u8>,
    work: &mut Work<'_>,
) -> Result<Geometry<f64>, PointBooleanError> {
    work.step()?;
    if filter.is_some_and(|d| d > 2) {
        return Err(PointBooleanError::InvalidOutputDimension);
    }
    let (ld, rd) = (dimension(left)?, dimension(right)?);
    if ld != 0 && rd != 0 {
        return Err(PointBooleanError::UnsupportedOperands);
    }
    validate_structure(left, "left", work)?;
    validate_structure(right, "right", work)?;
    let (sites, other) = if ld == 0 {
        (left, right)
    } else {
        (right, left)
    };
    if matches!(other, Geometry::Polygon(_)) {
        let empty = Geometry::MultiPoint(MultiPoint::new(Vec::new()));
        if ld == 0 {
            super::relate(&empty, other, work)?;
        } else {
            super::relate(other, &empty, work)?;
        }
    }
    let mut shape = Shape::default();
    let mut other_points = CountedVec::new();
    let mut other_ids = CountedVec::new();
    if ld == 0 && rd == 0 {
        for c in geo::CoordsIter::coords_iter(other) {
            work.nodes(1)?;
            work.push(&mut other_ids, other_points.len())?;
            work.push(&mut other_points, canonical(c))?;
        }
        crate::predicates::interval_index::sort_ids(
            &mut other_ids,
            |a, b| Ok(site_cmp(&other_points[a], &other_points[b])),
            work,
        )?;
    } else {
        shape.geometry(other, work)?;
        shape.build_indices(work)?;
    }
    let mut selected = CountedVec::new();
    for c in geo::CoordsIter::coords_iter(sites) {
        let covered = if ld == 0 && rd == 0 {
            work.nodes(1)?;
            let site = canonical(c);
            let (mut low, mut high) = (0, other_ids.len());
            while low < high {
                work.step()?;
                let middle = low + (high - low) / 2;
                if site_cmp(&other_points[other_ids[middle]], &site).is_lt() {
                    low = middle + 1;
                } else {
                    high = middle;
                }
            }
            other_ids
                .get(low)
                .is_some_and(|id| other_points[*id] == site)
        } else {
            let p = point(c, work)?;
            shape.position(&p, work)? != 2
        };
        let keep = match operation {
            PointBooleanOp::Union => !covered,
            PointBooleanOp::Intersection => covered,
            PointBooleanOp::Difference => ld == 0 && !covered,
        };
        if keep {
            work.nodes(1)?;
            work.push(&mut selected, canonical(c))?;
        }
    }
    if ld == 0 && rd == 0 && operation == PointBooleanOp::Union {
        for c in geo::CoordsIter::coords_iter(other) {
            work.nodes(1)?;
            work.push(&mut selected, canonical(c))?;
        }
    }
    let mut ids = CountedVec::new();
    for id in 0..selected.len() {
        work.push(&mut ids, id)?;
    }
    crate::predicates::interval_index::sort_ids(
        &mut ids,
        |a, b| {
            Ok(selected[a]
                .x()
                .total_cmp(&selected[b].x())
                .then(selected[a].y().total_cmp(&selected[b].y())))
        },
        work,
    )?;
    let originals = super::line_boolean::Originals::new(left, right, work)?;
    let mut points = CountedVec::new();
    for id in ids {
        work.step()?;
        if points.last() != Some(&selected[id]) {
            work.nodes(1)?;
            let exact = point(selected[id].0, work)?;
            let coordinate = originals
                .coordinate(&exact, work)
                .map_err(|error| match error {
                    super::line_boolean::LineBooleanError::Predicate(error) => error,
                    _ => PredicateError::CalcoloNonConcluso("provenienza puntuale"),
                })?;
            work.push(&mut points, Point(coordinate))?;
        }
    }
    let higher = ld.max(rd);
    let keep_higher = higher > 0
        && (operation == PointBooleanOp::Union
            || (operation == PointBooleanOp::Difference && ld > 0));
    let mut components = CountedVec::new();
    if !points.is_empty() && filter.is_none_or(|d| d == 0) {
        let points = points.export(work)?;
        work.push(
            &mut components,
            Geometry::MultiPoint(MultiPoint::new(points)),
        )?;
    }
    if keep_higher && filter.is_none_or(|d| d == higher) {
        if let Some(g) = copy_higher(other, work)? {
            work.push(&mut components, g)?;
        }
    }
    work.step()?;
    if components.len() == 1 {
        if let Some(g) = components.pop() {
            return Ok(g);
        }
    }
    if components.is_empty() {
        return Ok(match filter {
            Some(0) => Geometry::MultiPoint(MultiPoint::new(Vec::new())),
            Some(1) => Geometry::MultiLineString(MultiLineString::new(Vec::new())),
            Some(2) => Geometry::MultiPolygon(MultiPolygon::new(Vec::new())),
            _ => Geometry::GeometryCollection(GeometryCollection::empty()),
        });
    }
    Ok(Geometry::GeometryCollection(GeometryCollection(
        components.export(work)?,
    )))
}

#[allow(clippy::float_cmp)]
fn canonical(c: geo::Coord<f64>) -> Point<f64> {
    Point::new(
        if c.x == 0.0 { 0.0 } else { c.x },
        if c.y == 0.0 { 0.0 } else { c.y },
    )
}

pub(super) fn copy_line(
    line: &LineString<f64>,
    work: &mut Work<'_>,
) -> Result<LineString<f64>, PredicateError> {
    let mut coords = CountedVec::new();
    for c in &line.0 {
        work.nodes(1)?;
        work.push(&mut coords, *c)?;
    }
    Ok(LineString::new(coords.export(work)?))
}

pub(super) fn copy_higher(
    g: &Geometry<f64>,
    work: &mut Work<'_>,
) -> Result<Option<Geometry<f64>>, PredicateError> {
    match g {
        Geometry::LineString(line) if !line.0.is_empty() => {
            let line = copy_line(line, work)?;
            let mut lines = CountedVec::new();
            work.push(&mut lines, line)?;
            Ok(Some(Geometry::MultiLineString(MultiLineString::new(
                lines.export(work)?,
            ))))
        }
        Geometry::Polygon(p) if !p.exterior().0.is_empty() => {
            let shell = copy_line(p.exterior(), work)?;
            let mut holes = CountedVec::new();
            for h in p.interiors() {
                let h = copy_line(h, work)?;
                work.push(&mut holes, h)?;
            }
            let polygon = Polygon::new(shell, holes.export(work)?);
            let mut polygons = CountedVec::new();
            work.push(&mut polygons, polygon)?;
            Ok(Some(Geometry::MultiPolygon(MultiPolygon::new(
                polygons.export(work)?,
            ))))
        }
        _ => Ok(None),
    }
}

#[cfg(test)]
#[deny(clippy::disallowed_methods)]
mod tests;

fn site_cmp(a: &Point<f64>, b: &Point<f64>) -> std::cmp::Ordering {
    a.x().total_cmp(&b.x()).then(a.y().total_cmp(&b.y()))
}

/// Booleana puntuale: semantica, vuoti e limiti di [`boolean_with_limits`].
///
/// # Errors
/// Gli stessi errori di [`boolean_with_limits`].
pub(crate) fn union_with_limits(
    left: &Geometry<f64>,
    right: &Geometry<f64>,
    output_dimension: Option<u8>,
    limits: RelateLimits,
    cancelled: Option<&AtomicBool>,
) -> Result<Geometry<f64>, PointBooleanError> {
    boolean_with_limits(
        left,
        right,
        PointBooleanOp::Union,
        output_dimension,
        limits,
        cancelled,
    )
}

/// Booleana puntuale: semantica, vuoti e limiti di [`boolean_with_limits`].
///
/// # Errors
/// Gli stessi errori di [`boolean_with_limits`].
pub(crate) fn intersection_with_limits(
    left: &Geometry<f64>,
    right: &Geometry<f64>,
    output_dimension: Option<u8>,
    limits: RelateLimits,
    cancelled: Option<&AtomicBool>,
) -> Result<Geometry<f64>, PointBooleanError> {
    boolean_with_limits(
        left,
        right,
        PointBooleanOp::Intersection,
        output_dimension,
        limits,
        cancelled,
    )
}

/// Booleana puntuale: semantica, vuoti e limiti di [`boolean_with_limits`].
///
/// # Errors
/// Gli stessi errori di [`boolean_with_limits`].
pub(crate) fn difference_with_limits(
    left: &Geometry<f64>,
    right: &Geometry<f64>,
    output_dimension: Option<u8>,
    limits: RelateLimits,
    cancelled: Option<&AtomicBool>,
) -> Result<Geometry<f64>, PointBooleanError> {
    boolean_with_limits(
        left,
        right,
        PointBooleanOp::Difference,
        output_dimension,
        limits,
        cancelled,
    )
}

/// Ingressi con CRS verificato e consenso planare esplicito.
pub mod with_crs {
    use super::{AtomicBool, Geometry, PointBooleanError, PointBooleanOp, RelateLimits};

    crate::crs::checked_kernel!(boolean_with_limits, "geo.union", 2,
        (left: &Geometry<f64>, right: &Geometry<f64>, operation: PointBooleanOp, output_dimension: Option<u8>, limits: RelateLimits, cancelled: Option<&AtomicBool>) -> (Geometry<f64>, PointBooleanError),
        |context| {  super::boolean_with_limits(left, right, operation, output_dimension, limits, cancelled) });
    crate::crs::checked_kernel!(union_with_limits, "geo.union", 2,
        (left: &Geometry<f64>, right: &Geometry<f64>, output_dimension: Option<u8>, limits: RelateLimits, cancelled: Option<&AtomicBool>) -> (Geometry<f64>, PointBooleanError),
        |context| {  super::union_with_limits(left, right, output_dimension, limits, cancelled) });
    crate::crs::checked_kernel!(intersection_with_limits, "geo.intersection", 2,
        (left: &Geometry<f64>, right: &Geometry<f64>, output_dimension: Option<u8>, limits: RelateLimits, cancelled: Option<&AtomicBool>) -> (Geometry<f64>, PointBooleanError),
        |context| {  super::intersection_with_limits(left, right, output_dimension, limits, cancelled) });
    crate::crs::checked_kernel!(difference_with_limits, "geo.difference", 2,
        (left: &Geometry<f64>, right: &Geometry<f64>, output_dimension: Option<u8>, limits: RelateLimits, cancelled: Option<&AtomicBool>) -> (Geometry<f64>, PointBooleanError),
        |context| {  super::difference_with_limits(left, right, output_dimension, limits, cancelled) });
}
