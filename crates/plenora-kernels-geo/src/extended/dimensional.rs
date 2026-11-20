//! Hull dimensionale opt-in: classificazione esatta o soglia metrica esplicita.

use super::{validate_input, ExtendedError};
use crate::noding::{CountedVec, Edge, Work, P};
use crate::predicates::{PredicateError, RelateLimits};
use geo::{Coord, CoordsIter, Geometry, LineString, Point, Polygon};
use std::sync::atomic::AtomicBool;
use thiserror::Error;

mod concaveman;

/// Errori del percorso dimensionale; nessun messaggio contiene coordinate.
#[derive(Debug, Error)]
pub enum DimensionalHullError {
    /// Validazione e algoritmo poligonale preesistenti.
    #[error(transparent)]
    Extended(#[from] ExtendedError),
    /// Risorse e interruzioni della classificazione razionale.
    #[error(transparent)]
    Predicate(#[from] PredicateError),
    /// Errore della primitiva metrica condivisa.
    #[error(transparent)]
    Metric(#[from] crate::operations::OperationError),
}

/// Concave hull dimensionale con soglia esplicita, separata dalla concavita'.
///
/// `degeneracy_threshold = 0` riconosce coincidenza e collinearita' esatte
/// dei binary64. Un insieme vuoto rende `Polygon EMPTY`, un solo punto
/// distinto rende `Point`, una retta rende `LineString` con i due estremi
/// lessicografici. I parametri del concave hull si validano anche su vuoti
/// e degeneri; negli altri casi si usa il percorso poligonale preesistente.
/// Nessuna coordinata viene costruita o arrotondata.
///
/// Con soglia positiva, tutti i punti entro la soglia inclusiva dal minimo XY
/// rendono quel punto; altrimenti, se tutti sono entro la soglia dal segmento
/// minimo-massimo XY, rendono il segmento. L'adattatore metrico unico usa
/// `operations::distance` di main (binary64), in attesa della primitiva esatta condivisa.
///
/// `max_coordinates` conta prima di duplicare. `limits` governa le scansioni,
/// la classificazione, la memoria razionale, le visite e i buffer propri di
/// concaveman. Quickhull, interni degli indici e validazione OGC restano
/// fasi opache con checkpoint ai confini: niente qualifica integrale GOP-027
/// o promessa di latenza interna. Un passo conta una visita o operazione
/// di coda; le coppie contano candidati metrici e confronti geometrici.
///
/// # Errors
/// Parametri non validi, distanza non rappresentabile, input non valido,
/// limiti di coordinate/lavoro/memoria, annullamento, scadenza; sul percorso
/// poligonale anche gli errori di [`super::concave_hull`].
pub fn concave_hull_dimensional_with_limits(
    geometry: &Geometry<f64>,
    concavity: f64,
    length_threshold: f64,
    degeneracy_threshold: f64,
    max_coordinates: u64,
    limits: RelateLimits,
    cancelled: Option<&AtomicBool>,
) -> Result<Geometry<f64>, DimensionalHullError> {
    let mut work = Work::new(limits, cancelled);
    work.step()?;
    super::validate_hull_parameters(concavity, length_threshold)?;
    if !degeneracy_threshold.is_finite() || degeneracy_threshold < 0.0 {
        return Err(ExtendedError::InvalidParameter {
            name: "degeneracy_threshold",
            reason: "deve essere finita e non negativa",
        }
        .into());
    }
    let mut extrema: Option<(Coord<f64>, Coord<f64>)> = None;
    let mut count = 0_u64;
    visit_coordinates(geometry, &mut work, |mut coordinate, _work| {
        count = count.checked_add(1).ok_or(ExtendedError::IndexOverflow)?;
        if count > max_coordinates {
            return Err(ExtendedError::CoordinateLimit {
                actual: count,
                limit: max_coordinates,
            }
            .into());
        }
        if !coordinate.x.is_finite() || !coordinate.y.is_finite() {
            return Err(ExtendedError::InvalidInput("coordinate non finite".into()).into());
        }
        // Canonicalizza soltanto lo zero con segno: nessun punto si sposta.
        if coordinate.x == 0.0 {
            coordinate.x = 0.0;
        }
        if coordinate.y == 0.0 {
            coordinate.y = 0.0;
        }
        let (lo, hi) = extrema.get_or_insert((coordinate, coordinate));
        if compare(coordinate, *lo).is_lt() {
            *lo = coordinate;
        }
        if compare(coordinate, *hi).is_gt() {
            *hi = coordinate;
        }
        Ok(())
    })?;
    work.step()?;
    let validation = validate_input(geometry);
    work.step()?;
    validation?;
    let Some((lo, hi)) = extrema else {
        return Ok(Polygon::empty().into());
    };
    if compare(lo, hi).is_eq() {
        return Ok(Point(lo).into());
    }
    if degeneracy_threshold > 0.0 {
        if let Some(result) = within_threshold(geometry, lo, hi, degeneracy_threshold, &mut work)? {
            return Ok(result);
        }
    }
    work.nodes(2)?;
    let edge = Edge {
        a: P::of_in(lo, &work)?,
        b: P::of_in(hi, &work)?,
    };
    let mut collinear = true;
    visit_coordinates(geometry, &mut work, |coordinate, work| {
        if collinear {
            work.nodes(1)?;
            let point = P::of_in(coordinate, work)?;
            collinear = edge.contains(&point)?;
        }
        Ok(())
    })?;
    work.step()?;
    if collinear {
        work.reserve_export(2 * std::mem::size_of::<Coord<f64>>() as u64)?;
        return Ok(LineString(vec![lo, hi]).into());
    }
    let mut coordinates = CountedVec::new();
    visit_coordinates(geometry, &mut work, |coordinate, work| {
        work.push(&mut coordinates, coordinate)?;
        Ok(())
    })?;
    let result = super::protetto(|| {
        concaveman::run(
            &mut coordinates,
            &geo::concave_hull::ConcaveHullOptions {
                concavity,
                length_threshold,
            },
            &mut work,
        )
    });
    // Deadline precede anche l'errore del backend opaco.
    work.step()?;
    let hull = result??;
    let result = super::validate_output(hull.into());
    work.step()?;
    Ok(result?)
}

// Punto unico da adeguare alla primitiva esatta condivisa: usa oggi l'interfaccia di main.
// Non replica la metrica. None, overflow e valori non validi sono errori.
fn metric_within(
    coordinate: Coord<f64>,
    target: &Geometry<f64>,
    threshold: f64,
    work: &mut Work<'_>,
) -> Result<bool, DimensionalHullError> {
    work.pair()?;
    let upper = f64::from_bits(1043_u64 << 52); // 2^20: dominio metrico provvisorio.
    if let Geometry::Point(point) = target {
        for delta in [coordinate.x - point.x(), coordinate.y - point.y()] {
            if !delta.is_finite() || delta.abs() > upper {
                return Err(crate::operations::OperationError::PrecisionInsufficient.into());
            }
        }
    }
    if let Geometry::Line(line) = target {
        // Guardia di dominio, non una seconda metrica: i delta non nulli in
        // [2^-128, 2^20] tengono prodotti, somme e rapporti legacy lontani
        // da overflow e underflow. La metrica esatta condivisa supera questo limite.
        let lower = f64::from_bits(895_u64 << 52);
        for delta in [
            line.end.x - line.start.x,
            line.end.y - line.start.y,
            coordinate.x - line.start.x,
            coordinate.y - line.start.y,
            coordinate.x - line.end.x,
            coordinate.y - line.end.y,
        ] {
            if delta != 0.0 && !(lower..=upper).contains(&delta.abs()) {
                return Err(crate::operations::OperationError::PrecisionInsufficient.into());
            }
        }
    }
    let result = crate::operations::distance(&Point(coordinate).into(), target);
    work.step()?;
    let distance = result?.ok_or(crate::operations::OperationError::Internal(
        "distanza assente nella classificazione dimensionale",
    ))?;
    if !distance.is_finite() || distance < 0.0 {
        return Err(crate::operations::OperationError::PrecisionInsufficient.into());
    }
    if distance == 0.0 {
        let exact_zero = match target {
            Geometry::Point(point) => coordinate == point.0,
            Geometry::Line(line) => Edge {
                a: P::of_in(line.start, work)?,
                b: P::of_in(line.end, work)?,
            }
            .contains(&P::of_in(coordinate, work)?)?,
            _ => false,
        };
        if !exact_zero {
            return Err(crate::operations::OperationError::PrecisionInsufficient.into());
        }
    }
    Ok(distance <= threshold)
}

fn within_threshold(
    geometry: &Geometry<f64>,
    lo: Coord<f64>,
    hi: Coord<f64>,
    threshold: f64,
    work: &mut Work<'_>,
) -> Result<Option<Geometry<f64>>, DimensionalHullError> {
    let point = Point(lo).into();
    let mut near_point = true;
    visit_coordinates(geometry, work, |coordinate, work| {
        if near_point {
            near_point = metric_within(coordinate, &point, threshold, work)?;
        }
        Ok(())
    })?;
    work.step()?;
    if near_point {
        return Ok(Some(point));
    }
    let segment = geo::Line::new(lo, hi).into();
    let mut near_segment = true;
    visit_coordinates(geometry, work, |coordinate, work| {
        if near_segment {
            near_segment = metric_within(coordinate, &segment, threshold, work)?;
        }
        Ok(())
    })?;
    work.step()?;
    if near_segment {
        let mut coordinates = CountedVec::new();
        work.push(&mut coordinates, lo)?;
        work.push(&mut coordinates, hi)?;
        Ok(Some(LineString(coordinates.export(work)?).into()))
    } else {
        Ok(None)
    }
}

// Traversal iterativo: anche parti vuote e contenitori consumano lavoro.
// Solo lo stack delle GeometryCollection alloca; la capacita' e' addebitata.
fn visit_coordinates(
    geometry: &Geometry<f64>,
    work: &mut Work<'_>,
    mut visit: impl FnMut(Coord<f64>, &mut Work<'_>) -> Result<(), DimensionalHullError>,
) -> Result<(), DimensionalHullError> {
    fn line(
        line: &LineString<f64>,
        work: &mut Work<'_>,
        visit: &mut impl FnMut(Coord<f64>, &mut Work<'_>) -> Result<(), DimensionalHullError>,
    ) -> Result<(), DimensionalHullError> {
        work.step()?;
        for &coordinate in &line.0 {
            work.step()?;
            visit(coordinate, work)?;
        }
        Ok(())
    }
    fn polygon(
        polygon: &Polygon<f64>,
        work: &mut Work<'_>,
        visit: &mut impl FnMut(Coord<f64>, &mut Work<'_>) -> Result<(), DimensionalHullError>,
    ) -> Result<(), DimensionalHullError> {
        line(polygon.exterior(), work, visit)?;
        for ring in polygon.interiors() {
            line(ring, work, visit)?;
        }
        Ok(())
    }
    let mut pending = CountedVec::new();
    work.push(&mut pending, geometry)?;
    while let Some(geometry) = pending.pop() {
        work.step()?;
        match geometry {
            Geometry::GeometryCollection(collection) => {
                for part in collection.0.iter().rev() {
                    work.push(&mut pending, part)?;
                }
            }
            Geometry::MultiPolygon(polygons) => {
                for part in &polygons.0 {
                    polygon(part, work, &mut visit)?;
                }
            }
            Geometry::Polygon(part) => polygon(part, work, &mut visit)?,
            Geometry::MultiLineString(lines) => {
                for part in &lines.0 {
                    line(part, work, &mut visit)?;
                }
            }
            _ => {
                for coordinate in geometry.coords_iter() {
                    work.step()?;
                    visit(coordinate, work)?;
                }
            }
        }
    }
    Ok(())
}

fn compare(a: Coord<f64>, b: Coord<f64>) -> std::cmp::Ordering {
    a.x.total_cmp(&b.x).then_with(|| a.y.total_cmp(&b.y))
}

#[cfg(test)]
#[deny(clippy::disallowed_methods)]
mod tests;
