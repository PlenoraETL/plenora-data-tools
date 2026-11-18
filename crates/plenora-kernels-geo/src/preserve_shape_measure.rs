//! Misure ellissoidiche del percorso rettilineo nello SR originale.
//!
//! Modello, certificato e dominio: `docs/misure-forma-conservata.md`.
//! Nessun cambio di datum, unwrap o modifica delle misure preesistenti.

#![allow(
    clippy::float_cmp,
    reason = "Identita delle coordinate e degli estremi"
)]
#![allow(
    clippy::suboptimal_flops,
    reason = "Maggioranti scritti nella forma delle dimostrazioni"
)]

mod certificate;

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use geo::kernels::RobustKernel;
use geo::{Coord, Intersects, Kernel, Line, LineString, Orientation, Polygon};
use geographiclib_rs::{InverseGeodesic, PolygonArea, Winding};
use plenora_core::crs::riproiezione::{
    InverseShapeCertificate, OpzioniRiproiezione, PianoRiproiezione, Riproiettore,
    SeparableShapeBounds,
};
use plenora_core::crs::{CrsKind, Ellipsoid, ResolvedCrs};
use thiserror::Error;

use crate::geodetica::EllissoideGeodetico;

/// Tolleranza separata per ciascuna chiamata: metri o metri quadrati.
#[derive(Clone, Copy, Debug)]
pub struct ShapeTolerance {
    /// Termine assoluto finito e non negativo, nell'unita' della misura.
    pub absolute: f64,
    /// Termine relativo finito in [0,1), adimensionale.
    pub relative: f64,
}

/// Budget cumulativo, comprende validazione e tutti i raffinamenti scartati.
#[derive(Clone, Copy, Debug)]
pub struct ShapeMeasureLimits {
    /// Unita': visita di coordinata, conversione, primitiva geodetica,
    /// confronto di segmenti o passo del test di contenimento.
    pub max_work: u64,
    /// Scadenza monotona; cancellazione prioritaria.
    pub deadline: Option<Instant>,
}

/// Certificato del modello matematico, non dell'accuratezza di rilievo.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShapeMeasure {
    /// Metri per lunghezza, metri quadrati per area.
    pub value: f64,
    /// Maggiorante assoluto composto di discretizzazione e numerica.
    /// Soddisfa anche `absolute + relative * abs(valore_esatto)` richiesto.
    pub absolute_error_bound: f64,
    /// Ellissoide del datum sorgente; il CRS e' quello passato alla chiamata.
    pub ellipsoid: Ellipsoid,
    /// Lavoro effettivamente contato, inclusi i tentativi scartati.
    pub work: u64,
}

/// Errori senza coordinate, definizioni CRS o valori dell'ingresso.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum ShapeMeasureError {
    /// Tolleranza non finita, negativa, nulla su entrambi i termini o relativa >=1.
    #[error("tolleranza della misura non valida")]
    InvalidTolerance,
    /// CRS non convertibile o coordinata fuori dal dominio dell'inversa.
    #[error("dominio CRS della misura non supportato")]
    CrsDomain,
    /// Ellissoide fuori dal dominio terrestre qualificato.
    #[error("ellissoide della misura non supportato")]
    UnsupportedEllipsoid,
    /// Anello degenere, non semplice, buco esterno o anelli in contatto.
    #[error("topologia della superficie non supportata")]
    UnsupportedTopology,
    /// Superficie non contenuta in una striscia di longitudine aperta <180 gradi.
    #[error("dominio della superficie non supportato")]
    UnsupportedSurface,
    /// Manca una dimostrazione del bound, o precisione numerica insufficiente.
    #[error("accuratezza della misura non dimostrata")]
    AccuracyUnproven,
    /// Budget realmente consumato.
    #[error("limite di lavoro della misura superato")]
    WorkLimit,
    /// Annullamento cooperativo.
    #[error("misura a forma conservata annullata")]
    Cancelled,
    /// Scadenza raggiunta.
    #[error("scadenza della misura a forma conservata raggiunta")]
    DeadlineExceeded,
}

struct MeasureWork<'a> {
    limits: ShapeMeasureLimits,
    cancelled: Option<&'a AtomicBool>,
    used: u64,
}

impl MeasureWork<'_> {
    fn checkpoint(&self) -> Result<(), ShapeMeasureError> {
        if self
            .cancelled
            .is_some_and(|flag| flag.load(Ordering::Relaxed))
        {
            return Err(ShapeMeasureError::Cancelled);
        }
        if self
            .limits
            .deadline
            .is_some_and(|end| Instant::now() >= end)
        {
            return Err(ShapeMeasureError::DeadlineExceeded);
        }
        Ok(())
    }

    fn charge(&mut self) -> Result<(), ShapeMeasureError> {
        self.checkpoint()?;
        self.used = self
            .used
            .checked_add(1)
            .ok_or(ShapeMeasureError::WorkLimit)?;
        if self.used > self.limits.max_work {
            return Err(ShapeMeasureError::WorkLimit);
        }
        // Mantiene trascurabile il termine n*epsilon^2 della somma compensata.
        if self.used > (1_u64 << 40) {
            return Err(ShapeMeasureError::AccuracyUnproven);
        }
        Ok(())
    }
}

/// Somma compensata di Neumaier, ordine dell'ingresso invariato.
#[derive(Default)]
struct MeasureSum {
    total: f64,
    correction: f64,
}

impl MeasureSum {
    fn add(&mut self, value: f64) {
        let next = self.total + value;
        self.correction += if self.total.abs() >= value.abs() {
            (self.total - next) + value
        } else {
            (value - next) + self.total
        };
        self.total = next;
    }

    fn value(&self) -> f64 {
        self.total + self.correction
    }
}

struct ShapeContext {
    projection: Riproiettore,
    ellipsoid: EllissoideGeodetico,
    geographic: bool,
    derivatives: SeparableShapeBounds,
}

impl ShapeContext {
    fn new(crs: &ResolvedCrs, tolerance: ShapeTolerance) -> Result<Self, ShapeMeasureError> {
        if !(tolerance.absolute.is_finite()
            && tolerance.absolute >= 0.0
            && tolerance.relative.is_finite()
            && (0.0..1.0).contains(&tolerance.relative)
            && (tolerance.absolute > 0.0 || tolerance.relative > 0.0))
        {
            return Err(ShapeMeasureError::InvalidTolerance);
        }
        let plan = PianoRiproiezione::nuovo(crs, crs, &OpzioniRiproiezione::default())
            .map_err(|_| ShapeMeasureError::CrsDomain)?;
        let projection =
            Riproiettore::nuovo(plan, BTreeMap::new()).map_err(|_| ShapeMeasureError::CrsDomain)?;
        let ellipsoid = EllissoideGeodetico::da_crs(crs)
            .map_err(|_| ShapeMeasureError::UnsupportedEllipsoid)?;
        let parameters = ellipsoid.parametri();
        if !(6e6..=7e6).contains(&parameters.semi_major_axis_metre)
            || !(250.0..=350.0).contains(&parameters.inverse_flattening)
        {
            return Err(ShapeMeasureError::UnsupportedEllipsoid);
        }
        let geographic = crs.kind() == CrsKind::Geographic;
        let InverseShapeCertificate::Separable(derivatives) =
            projection.inverse_shape_certificate()
        else {
            return Err(ShapeMeasureError::AccuracyUnproven);
        };
        Ok(Self {
            projection,
            ellipsoid,
            geographic,
            derivatives,
        })
    }

    fn convert(
        &self,
        point: Coord<f64>,
        work: &mut MeasureWork<'_>,
    ) -> Result<Coord<f64>, ShapeMeasureError> {
        work.charge()?;
        let (x, y) = self
            .projection
            .geografiche_sorgente(point.x, point.y)
            .map_err(|_| ShapeMeasureError::CrsDomain)?;
        if !self.geographic {
            let raw_longitude = self
                .derivatives
                .angular_scale
                .mul_add(point.x, self.derivatives.longitude_offset);
            // Il certificato usa un solo ramo continuo, mai la riduzione modulo 2pi.
            if raw_longitude.abs() >= std::f64::consts::PI || y.abs() >= 85.06 {
                return Err(ShapeMeasureError::CrsDomain);
            }
        }
        if self.derivatives.method == 9804 {
            let parameters = self.ellipsoid.parametri();
            certificate::mercator_residual(
                y,
                point.y,
                self.derivatives.projected_divisor,
                self.derivatives.false_northing,
                parameters.inverse_flattening,
                self.derivatives.latitude_first,
            )?;
        }
        Ok(Coord { x, y })
    }

    fn deltas(
        &self,
        start: Coord<f64>,
        end: Coord<f64>,
        divisions: u32,
    ) -> Result<(f64, f64), ShapeMeasureError> {
        let scale = self.derivatives.angular_scale;
        let dx = (end.x - start.x) * scale / f64::from(divisions);
        let dy = (end.y - start.y) * scale / f64::from(divisions);
        if (start.x != end.x && !dx.is_normal()) || (start.y != end.y && !dy.is_normal()) {
            return Err(ShapeMeasureError::AccuracyUnproven);
        }
        Ok((dx, dy))
    }
}

fn interpolated(start: Coord<f64>, end: Coord<f64>, fraction: f64) -> Coord<f64> {
    if fraction == 0.0 {
        return start;
    }
    if fraction == 1.0 {
        return end;
    }
    Coord {
        x: (end.x - start.x).mul_add(fraction, start.x),
        y: (end.y - start.y).mul_add(fraction, start.y),
    }
}

fn accepted(value: f64, bound: f64, tolerance: ShapeTolerance) -> bool {
    value.is_finite()
        && bound.is_finite()
        && bound >= 0.0
        && bound
            <= tolerance
                .relative
                .mul_add((value.abs() - bound).max(0.0), tolerance.absolute)
}

fn result(
    context: &ShapeContext,
    value: f64,
    bound: f64,
    work: &MeasureWork<'_>,
) -> Result<ShapeMeasure, ShapeMeasureError> {
    work.checkpoint()?;
    Ok(ShapeMeasure {
        value,
        absolute_error_bound: bound,
        ellipsoid: context.ellipsoid.parametri(),
        work: work.used,
    })
}

/// Lunghezza in metri, sommata su parti indipendenti (anche anelli espliciti).
///
/// XY segue la retta nello SR, senza unwrap: 179 -> -179 passa per zero.
/// Parti vuote e coordinate ripetute contribuiscono zero. Il campionamento
/// raddoppia adattivamente finche' il bound analitico soddisfa la tolleranza.
/// Memoria ausiliaria costante, nessun vettore di punti campionati.
/// Dominio certificato: CRS geografici integrati, Pseudo Mercator e Mercator A.
///
/// # Errors
/// [`ShapeMeasureError`], nessun risultato parziale. Gli altri CRS
/// riproiettabili falliscono con `AccuracyUnproven`.
pub fn preserve_shape_length_with_limits(
    parts: &[LineString<f64>],
    crs: &ResolvedCrs,
    tolerance: ShapeTolerance,
    limits: ShapeMeasureLimits,
    cancelled: Option<&AtomicBool>,
) -> Result<ShapeMeasure, ShapeMeasureError> {
    let mut work = MeasureWork {
        limits,
        cancelled,
        used: 0,
    };
    work.checkpoint()?;
    let context = ShapeContext::new(crs, tolerance)?;
    let mut divisions = 1_u32;
    loop {
        let mut value = MeasureSum::default();
        let mut error = MeasureSum::default();
        for part in parts {
            work.charge()?;
            for &point in &part.0 {
                context.convert(point, &mut work)?;
            }
            for pair in part.0.windows(2) {
                length_edge(
                    &context, pair[0], pair[1], divisions, &mut work, &mut value, &mut error,
                )?;
            }
        }
        let bound = error.value() + value.value().abs() * 32.0 * f64::EPSILON;
        if accepted(value.value(), bound, tolerance) {
            return result(&context, value.value(), bound, &work);
        }
        divisions = divisions
            .checked_mul(2)
            .ok_or(ShapeMeasureError::AccuracyUnproven)?;
    }
}

fn length_edge(
    context: &ShapeContext,
    start: Coord<f64>,
    end: Coord<f64>,
    divisions: u32,
    work: &mut MeasureWork<'_>,
    value: &mut MeasureSum,
    error: &mut MeasureSum,
) -> Result<(), ShapeMeasureError> {
    if start == end || (context.geographic && start.y == end.y && start.y.abs() == 90.0) {
        work.charge()?;
        return Ok(());
    }
    let a = context.ellipsoid.parametri().semi_major_axis_metre;
    let (dx, dy) = context.deltas(start, end, divisions)?;
    let first = context.derivatives.latitude_first;
    let second = context.derivatives.latitude_second;
    let speed = dx.abs() + first * dy.abs();
    let acceleration = 1.1 * a * (speed.powi(2) + second * dy * dy);
    let mut previous = context.convert(start, work)?;
    for i in 1..=divisions {
        let point = context.convert(
            interpolated(start, end, f64::from(i) / f64::from(divisions)),
            work,
        )?;
        work.charge()?;
        let distance: f64 = context
            .ellipsoid
            .geodetica()
            .inverse(previous.y, previous.x, point.y, point.x);
        if !distance.is_finite() {
            return Err(ShapeMeasureError::AccuracyUnproven);
        }
        // G1/G2: 15 nm per inverse; 1 micrometro include conversione e interpolazione.
        let lower = distance - 1e-6;
        if lower <= 0.0 {
            // Il maggiorante della velocita' resta valido anche ai poli o ai coincidenti.
            error.add(1.1 * a * speed + 1e-6);
        } else {
            // Varianza della velocita' ECEF: L-d <= A^2/(12d), senza test di convergenza.
            error.add(acceleration.powi(2) / (12.0 * lower) + 1e-6);
        }
        value.add(distance);
        previous = point;
    }
    Ok(())
}

/// Area positiva del guscio meno aree positive dei buchi, in metri quadrati.
///
/// Il verso degli anelli non cambia il risultato; internamente l'area firmata
/// e' positiva antioraria. Poligoni vuoti contribuiscono zero. Ogni poligono
/// deve essere semplice, senza contatti, e contenuto in una striscia aperta
/// di longitudine larga meno di 180 gradi, senza poli. Le parti si sommano:
/// non si calcola l'unione di poligoni sovrapposti.
/// Dominio CRS e modello come [`preserve_shape_length_with_limits`].
///
/// # Errors
/// [`ShapeMeasureError`], incluso il rifiuto delle superfici fuori profilo,
/// dei buchi esterni e dei budget esauriti durante la validazione.
pub fn preserve_shape_area_with_limits(
    polygons: &[Polygon<f64>],
    crs: &ResolvedCrs,
    tolerance: ShapeTolerance,
    limits: ShapeMeasureLimits,
    cancelled: Option<&AtomicBool>,
) -> Result<ShapeMeasure, ShapeMeasureError> {
    let mut work = MeasureWork {
        limits,
        cancelled,
        used: 0,
    };
    work.checkpoint()?;
    let context = ShapeContext::new(crs, tolerance)?;
    for polygon in polygons {
        validate_polygon(polygon, &context, &mut work)?;
    }
    let mut divisions = 1_u32;
    loop {
        let mut value = MeasureSum::default();
        let mut error = MeasureSum::default();
        for polygon in polygons {
            work.charge()?;
            for (index, ring) in std::iter::once(polygon.exterior())
                .chain(polygon.interiors())
                .enumerate()
            {
                let (area, bound) = ring_area(&context, ring, divisions, &mut work)?;
                value.add(if index == 0 { area.abs() } else { -area.abs() });
                error.add(bound);
            }
        }
        let bound = error.value() + value.value().abs() * 32.0 * f64::EPSILON;
        if value.value() >= 0.0 && accepted(value.value(), bound, tolerance) {
            return result(&context, value.value(), bound, &work);
        }
        divisions = divisions
            .checked_mul(2)
            .ok_or(ShapeMeasureError::AccuracyUnproven)?;
    }
}

// Q(phi) = a^2(1-e^2) sum (n+1)/(2n+1) e^(2n) sin(phi)^(2n+1).
// Nove termini: resto < 1e-18 a^2 per e^2<0.008; evita atanh(e sin(phi))/e.
fn area_primitive(latitude: f64, ellipsoid: Ellipsoid) -> f64 {
    let flattening = 1.0 / ellipsoid.inverse_flattening;
    let eccentricity = flattening * (2.0 - flattening);
    let sine = latitude.to_radians().sin();
    let ratio = eccentricity * sine * sine;
    let mut power = sine;
    let mut sum = MeasureSum::default();
    for n in 0..9 {
        sum.add(f64::from(n + 1) / f64::from(2 * n + 1) * power);
        power *= ratio;
    }
    ellipsoid.semi_major_axis_metre.powi(2) * (1.0 - eccentricity) * sum.value()
}

fn ring_area(
    context: &ShapeContext,
    ring: &LineString<f64>,
    divisions: u32,
    work: &mut MeasureWork<'_>,
) -> Result<(f64, f64), ShapeMeasureError> {
    let mut polygon = PolygonArea::new(context.ellipsoid.geodetica(), Winding::CounterClockwise);
    let mut integral = MeasureSum::default();
    let mut error = MeasureSum::default();
    let parameters = context.ellipsoid.parametri();
    for pair in ring.0.windows(2) {
        let (dx, dy) = context.deltas(pair[0], pair[1], divisions)?;
        for i in 0..divisions {
            let point = context.convert(
                interpolated(pair[0], pair[1], f64::from(i) / f64::from(divisions)),
                work,
            )?;
            let midpoint = context.convert(
                interpolated(
                    pair[0],
                    pair[1],
                    (f64::from(i) + 0.5) / f64::from(divisions),
                ),
                work,
            )?;
            work.charge()?;
            polygon.add_point(point.y, point.x);
            integral.add(-area_primitive(midpoint.y, parameters) * dx);
            let derivative_bound = 1.1
                * (context.derivatives.latitude_first.powi(2)
                    + context.derivatives.latitude_second);
            error.add(
                dx.abs()
                    * parameters.semi_major_axis_metre.powi(2)
                    * (derivative_bound * dy * dy / 24.0 + 512.0 * f64::EPSILON),
            );
        }
    }
    work.charge()?;
    let (_, area, _) = polygon.compute(true);
    // Il valore GeographicLib si confronta con un integrale indipendente con bound.
    // Anche l'errore della primitiva area e della sua accumulazione entra nel residuo.
    let bound = (area - integral.value()).abs() + error.value() + 32.0 * f64::EPSILON * area.abs();
    if !area.is_finite() || !bound.is_finite() {
        return Err(ShapeMeasureError::AccuracyUnproven);
    }
    Ok((area, bound))
}

fn validate_polygon(
    polygon: &Polygon<f64>,
    context: &ShapeContext,
    work: &mut MeasureWork<'_>,
) -> Result<(), ShapeMeasureError> {
    work.charge()?;
    let shell = polygon.exterior();
    if shell.0.is_empty() {
        return if polygon.interiors().is_empty() {
            Ok(())
        } else {
            Err(ShapeMeasureError::UnsupportedTopology)
        };
    }
    let mut west = f64::INFINITY;
    let mut east = f64::NEG_INFINITY;
    for ring in std::iter::once(shell).chain(polygon.interiors()) {
        if ring.0.len() < 4 || ring.0.first() != ring.0.last() {
            return Err(ShapeMeasureError::UnsupportedTopology);
        }
        for &point in &ring.0 {
            let geographic = context.convert(point, work)?;
            if geographic.y.abs() >= 90.0 {
                return Err(ShapeMeasureError::UnsupportedSurface);
            }
            west = west.min(geographic.x);
            east = east.max(geographic.x);
        }
        for (i, first) in ring.lines().enumerate() {
            work.charge()?;
            if first.start == first.end {
                return Err(ShapeMeasureError::UnsupportedTopology);
            }
            for (j, second) in ring.lines().enumerate().skip(i + 1) {
                work.charge()?;
                let adjacent = j == i + 1 || (i == 0 && j == ring.0.len() - 2);
                if adjacent {
                    let (outer_start, common, outer_end) = if j == i + 1 {
                        (first.start, first.end, second.end)
                    } else {
                        (first.end, first.start, second.start)
                    };
                    if Line::new(common, outer_start).intersects(&outer_end)
                        || Line::new(common, outer_end).intersects(&outer_start)
                    {
                        return Err(ShapeMeasureError::UnsupportedTopology);
                    }
                } else if first.intersects(&second) {
                    return Err(ShapeMeasureError::UnsupportedTopology);
                }
            }
        }
    }
    if east - west >= 180.0 {
        return Err(ShapeMeasureError::UnsupportedSurface);
    }
    for (i, hole) in polygon.interiors().iter().enumerate() {
        inside(hole, shell, true, work)?;
        disjoint_boundaries(hole, shell, work)?;
        for other in polygon.interiors().iter().skip(i + 1) {
            disjoint_boundaries(hole, other, work)?;
            inside(hole, other, false, work)?;
            inside(other, hole, false, work)?;
        }
    }
    Ok(())
}

fn inside(
    ring: &LineString<f64>,
    boundary: &LineString<f64>,
    expected: bool,
    work: &mut MeasureWork<'_>,
) -> Result<(), ShapeMeasureError> {
    let Some(&point) = ring.0.first() else {
        return Err(ShapeMeasureError::UnsupportedTopology);
    };
    let mut contained = false;
    for line in boundary.lines() {
        work.charge()?;
        if line.intersects(&point) {
            return Err(ShapeMeasureError::UnsupportedTopology);
        }
        if (line.start.y > point.y) != (line.end.y > point.y) {
            let orientation = RobustKernel::orient2d(line.start, line.end, point);
            if (orientation == Orientation::CounterClockwise) == (line.end.y > line.start.y) {
                contained = !contained;
            }
        }
    }
    if contained != expected {
        return Err(ShapeMeasureError::UnsupportedTopology);
    }
    work.checkpoint()
}

fn disjoint_boundaries(
    first: &LineString<f64>,
    second: &LineString<f64>,
    work: &mut MeasureWork<'_>,
) -> Result<(), ShapeMeasureError> {
    for a in first.lines() {
        for b in second.lines() {
            work.charge()?;
            if a.intersects(&b) {
                return Err(ShapeMeasureError::UnsupportedTopology);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
#[deny(clippy::disallowed_methods)]
mod tests;
