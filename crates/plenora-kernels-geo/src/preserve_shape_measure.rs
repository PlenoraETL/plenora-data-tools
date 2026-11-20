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

pub(crate) mod certificate;

use std::collections::BTreeMap;
use std::sync::atomic::AtomicBool;
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
    /// Scadenza monotona; prevale sulla cancellazione allo stesso checkpoint.
    pub deadline: Option<Instant>,
}

/// Certificato del modello matematico, non dell'accuratezza di rilievo.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShapeMeasure {
    /// Metri per lunghezza, metri quadrati per area.
    pub value: f64,
    /// Maggiorante assoluto composto di trasformazione, discretizzazione e numerica.
    /// Soddisfa anche `absolute + relative * abs(valore_esatto)` richiesto.
    pub absolute_error_bound: f64,
    /// Bound relativo pubblicato: zero, tutto il certificato e' assoluto.
    pub relative_error_bound: f64,
    /// Scomposizione nell'unita' della misura, con somma pari al bound assoluto.
    pub error_components: ShapeErrorComponents,
    /// Ellissoide del datum sorgente; il CRS e' quello passato alla chiamata.
    pub ellipsoid: Ellipsoid,
    /// Lavoro effettivamente contato, inclusi i tentativi scartati.
    pub work: u64,
}

/// Componenti del certificato GOP-006, tutte finite e non negative.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ShapeErrorComponents {
    /// Zero: nessun cambio datum o approssimazione aggiuntiva del modello.
    pub transformation: f64,
    /// Approssimazione del percorso continuo e quadratura.
    pub discretization: f64,
    /// Valutazione dell'inversa, interpolazione, primitive, residuo d'area e somme.
    pub numerical: f64,
}

#[derive(Default)]
struct MeasureErrors {
    transformation: MeasureSum,
    discretization: MeasureSum,
    numerical: MeasureSum,
}
impl MeasureErrors {
    fn components(&self) -> ShapeErrorComponents {
        ShapeErrorComponents {
            transformation: self.transformation.value(),
            discretization: self.discretization.value(),
            numerical: self.numerical.value(),
        }
    }
    fn value(&self) -> f64 {
        (self.transformation.value() + self.discretization.value()) + self.numerical.value()
    }
    fn merge(&mut self, other: &Self) {
        self.transformation.add(other.transformation.value());
        self.discretization.add(other.discretization.value());
        self.numerical.add(other.numerical.value());
    }
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
    /// Inversa definita nel CRS, ma fuori dal dominio del certificato (execution).
    #[error("GEOMETRY_ACCURACY_UNPROVEN: dominio certificato escluso")]
    CertifiedDomain,
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
    #[error("GEOMETRY_ACCURACY_UNPROVEN")]
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
    interrupt: crate::noding::Work<'a>,
    used: u64,
}

impl<'a> MeasureWork<'a> {
    fn new(limits: ShapeMeasureLimits, cancelled: Option<&'a AtomicBool>) -> Self {
        Self {
            limits,
            interrupt: crate::noding::Work::new(
                crate::predicates::RelateLimits {
                    max_steps: 1_u64 << 50,
                    deadline: limits.deadline,
                    ..crate::predicates::RelateLimits::default()
                },
                cancelled,
            ),
            used: 0,
        }
    }

    fn checkpoint(&mut self) -> Result<(), ShapeMeasureError> {
        self.interrupt.step().map_err(|error| match error {
            crate::predicates::PredicateError::DeadlineExceeded => {
                ShapeMeasureError::DeadlineExceeded
            }
            crate::predicates::PredicateError::Cancelled => ShapeMeasureError::Cancelled,
            _ => ShapeMeasureError::WorkLimit,
        })
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

enum ShapeDerivatives {
    Separable(SeparableShapeBounds),
    Transverse(certificate::TransverseCertificate),
    Lambert(certificate::LambertCertificate),
    Azimuthal(certificate::AzimuthalCertificate),
    Stereographic(certificate::StereographicCertificate),
    Hotine(certificate::HotineCertificate),
}

struct ShapeContext {
    projection: Riproiettore,
    ellipsoid: EllissoideGeodetico,
    geographic: bool,
    derivatives: ShapeDerivatives,
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
        let derivatives = match projection.inverse_shape_certificate() {
            InverseShapeCertificate::Separable(bounds) => ShapeDerivatives::Separable(bounds),
            InverseShapeCertificate::TransverseMercator(bounds) => ShapeDerivatives::Transverse(
                certificate::TransverseCertificate::new(bounds, parameters.inverse_flattening)?,
            ),
            InverseShapeCertificate::LambertConicConformal(bounds) => ShapeDerivatives::Lambert(
                certificate::LambertCertificate::new(bounds, parameters.inverse_flattening)?,
            ),
            InverseShapeCertificate::LambertAzimuthalEqualArea(bounds) => {
                ShapeDerivatives::Azimuthal(certificate::AzimuthalCertificate::new(
                    bounds,
                    parameters.inverse_flattening,
                )?)
            }
            InverseShapeCertificate::ObliqueStereographic(bounds) => {
                ShapeDerivatives::Stereographic(certificate::StereographicCertificate::new(
                    bounds,
                    parameters.inverse_flattening,
                )?)
            }
            InverseShapeCertificate::HotineObliqueMercatorB(bounds) => ShapeDerivatives::Hotine(
                certificate::HotineCertificate::new(bounds, parameters.inverse_flattening)?,
            ),
            InverseShapeCertificate::Unproven { .. } => {
                return Err(ShapeMeasureError::AccuracyUnproven)
            }
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
        #[cfg(test)]
        crate::noding::gop028::unit();
        let (x, y) = self
            .projection
            .geografiche_sorgente(point.x, point.y)
            .map_err(|_| ShapeMeasureError::CrsDomain)?;

        if let ShapeDerivatives::Transverse(certificate) = &self.derivatives {
            certificate
                .check_domain(point.x, point.y)
                .map_err(|error| match error {
                    ShapeMeasureError::AccuracyUnproven => ShapeMeasureError::CertifiedDomain,
                    other => other,
                })?;
        }
        if let ShapeDerivatives::Lambert(certificate) = &self.derivatives {
            certificate
                .check_domain(point.x, point.y)
                .map_err(|error| match error {
                    ShapeMeasureError::AccuracyUnproven => ShapeMeasureError::CertifiedDomain,
                    other => other,
                })?;
        }
        if let ShapeDerivatives::Azimuthal(certificate) = &self.derivatives {
            certificate
                .check_domain(point.x, point.y)
                .map_err(|error| match error {
                    ShapeMeasureError::AccuracyUnproven => ShapeMeasureError::CertifiedDomain,
                    other => other,
                })?;
        }
        if let ShapeDerivatives::Stereographic(certificate) = &self.derivatives {
            certificate
                .check_domain(point.x, point.y)
                .map_err(|error| match error {
                    ShapeMeasureError::AccuracyUnproven => ShapeMeasureError::CertifiedDomain,
                    other => other,
                })?;
        }
        if let ShapeDerivatives::Hotine(certificate) = &self.derivatives {
            certificate
                .check_domain(point.x, point.y)
                .map_err(|error| match error {
                    ShapeMeasureError::AccuracyUnproven => ShapeMeasureError::CertifiedDomain,
                    other => other,
                })?;
        }
        let ShapeDerivatives::Separable(derivatives) = &self.derivatives else {
            if let ShapeDerivatives::Transverse(certificate) = &self.derivatives {
                certificate.certify(point.x, point.y, x, y)?;
            }
            if let ShapeDerivatives::Lambert(certificate) = &self.derivatives {
                certificate.certify(point.x, point.y, x, y)?;
            }
            if let ShapeDerivatives::Azimuthal(certificate) = &self.derivatives {
                certificate.certify(point.x, point.y, x, y)?;
            }
            if let ShapeDerivatives::Stereographic(certificate) = &self.derivatives {
                certificate.certify(point.x, point.y, x, y)?;
            }
            if let ShapeDerivatives::Hotine(certificate) = &self.derivatives {
                certificate.certify(point.x, point.y, x, y)?;
            }
            return Ok(Coord { x, y });
        };
        if !self.geographic {
            let raw_longitude = derivatives
                .angular_scale
                .mul_add(point.x, derivatives.longitude_offset);
            // Il certificato usa un solo ramo continuo, mai la riduzione modulo 2pi.
            if raw_longitude.abs() >= std::f64::consts::PI || y.abs() >= 85.06 {
                return Err(ShapeMeasureError::CrsDomain);
            }
        }
        if derivatives.method == 9804 {
            let parameters = self.ellipsoid.parametri();
            certificate::mercator_residual(
                y,
                point.y,
                derivatives.projected_divisor,
                derivatives.false_northing,
                parameters.inverse_flattening,
                derivatives.latitude_first,
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
        let scale = match &self.derivatives {
            ShapeDerivatives::Separable(bounds) => bounds.angular_scale,
            ShapeDerivatives::Transverse(certificate) => certificate.scale(),
            ShapeDerivatives::Lambert(certificate) => certificate.scale(),
            ShapeDerivatives::Azimuthal(certificate) => certificate.scale(),
            ShapeDerivatives::Stereographic(certificate) => certificate.scale(),
            ShapeDerivatives::Hotine(certificate) => certificate.scale(),
        };
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
    error: &MeasureErrors,
    work: &mut MeasureWork<'_>,
) -> Result<ShapeMeasure, ShapeMeasureError> {
    work.checkpoint()?;
    Ok(ShapeMeasure {
        value,
        absolute_error_bound: error.value(),
        relative_error_bound: 0.0,
        error_components: error.components(),
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
/// Dominio certificato: geografici, i due Mercator, TM, LCC, LAEA, stereografica
/// obliqua e caso svizzero Hotine B (angoli 90 gradi),
/// nei domini qualificati in `docs/maggioranti-forma.md`.
///
/// # Errors
/// [`ShapeMeasureError`], nessun risultato parziale. Dominio o precisione
/// non certificabili producono `AccuracyUnproven`.
pub fn preserve_shape_length_with_limits(
    parts: &[LineString<f64>],
    crs: &ResolvedCrs,
    tolerance: ShapeTolerance,
    limits: ShapeMeasureLimits,
    cancelled: Option<&AtomicBool>,
) -> Result<ShapeMeasure, ShapeMeasureError> {
    let mut work = MeasureWork::new(limits, cancelled);
    work.checkpoint()?;
    let context = ShapeContext::new(crs, tolerance)?;
    let mut divisions = 1_u32;
    loop {
        #[cfg(test)]
        crate::noding::gop028::cycle(
            "preserve_shape_measure.rs::preserve_shape_length_with_limits:1",
        );
        let mut value = MeasureSum::default();
        let mut error = MeasureErrors::default();
        for part in parts {
            #[cfg(test)]
            crate::noding::gop028::cycle(
                "preserve_shape_measure.rs::preserve_shape_length_with_limits:2",
            );
            work.charge()?;
            for &point in &part.0 {
                #[cfg(test)]
                crate::noding::gop028::cycle(
                    "preserve_shape_measure.rs::preserve_shape_length_with_limits:3",
                );
                context.convert(point, &mut work)?;
            }
            for pair in part.0.windows(2) {
                #[cfg(test)]
                crate::noding::gop028::cycle(
                    "preserve_shape_measure.rs::preserve_shape_length_with_limits:4",
                );
                length_edge(
                    &context, pair[0], pair[1], divisions, &mut work, &mut value, &mut error,
                )?;
            }
        }
        error
            .numerical
            .add(value.value().abs() * 64.0 * f64::EPSILON);
        let bound = error.value();
        if accepted(value.value(), bound, tolerance) {
            return result(&context, value.value(), &error, &mut work);
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
    error: &mut MeasureErrors,
) -> Result<(), ShapeMeasureError> {
    if start == end || (context.geographic && start.y == end.y && start.y.abs() == 90.0) {
        work.charge()?;
        return Ok(());
    }
    let a = context.ellipsoid.parametri().semi_major_axis_metre;
    let (dx, dy) = context.deltas(start, end, divisions)?;
    let (speed, acceleration, numerical) = match &context.derivatives {
        ShapeDerivatives::Separable(bounds) => {
            let speed = dx.abs() + bounds.latitude_first * dy.abs();
            (
                speed,
                1.1 * a * (speed.powi(2) + bounds.latitude_second * dy * dy),
                1e-6,
            )
        }
        ShapeDerivatives::Transverse(_)
        | ShapeDerivatives::Lambert(_)
        | ShapeDerivatives::Azimuthal(_)
        | ShapeDerivatives::Stereographic(_)
        | ShapeDerivatives::Hotine(_) => {
            let delta = dx.abs() + dy.abs();
            (7.0 * delta, 1.1 * a * 78.0 * delta.powi(2), 3e-5)
        }
    };
    let mut previous = context.convert(start, work)?;
    for i in 1..=divisions {
        #[cfg(test)]
        crate::noding::gop028::cycle("preserve_shape_measure.rs::length_edge:1");
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
        let lower = distance - numerical;
        if lower <= 0.0 {
            // Il maggiorante della velocita' resta valido anche ai poli o ai coincidenti.
            error.discretization.add(1.1 * a * speed);
        } else {
            // Varianza della velocita' ECEF: L-d <= A^2/(12d), senza test di convergenza.
            error
                .discretization
                .add(acceleration.powi(2) / (12.0 * lower));
        }
        error.numerical.add(numerical);
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
    let mut work = MeasureWork::new(limits, cancelled);
    work.checkpoint()?;
    let context = ShapeContext::new(crs, tolerance)?;
    for polygon in polygons {
        #[cfg(test)]
        crate::noding::gop028::cycle(
            "preserve_shape_measure.rs::preserve_shape_area_with_limits:1",
        );
        validate_polygon(polygon, &context, &mut work)?;
    }
    let mut divisions = 1_u32;
    loop {
        #[cfg(test)]
        crate::noding::gop028::cycle(
            "preserve_shape_measure.rs::preserve_shape_area_with_limits:2",
        );
        let mut value = MeasureSum::default();
        let mut error = MeasureErrors::default();
        for polygon in polygons {
            #[cfg(test)]
            crate::noding::gop028::cycle(
                "preserve_shape_measure.rs::preserve_shape_area_with_limits:3",
            );
            work.charge()?;
            for (index, ring) in std::iter::once(polygon.exterior())
                .chain(polygon.interiors())
                .enumerate()
            {
                #[cfg(test)]
                crate::noding::gop028::cycle(
                    "preserve_shape_measure.rs::preserve_shape_area_with_limits:4",
                );
                let (area, bound) = ring_area(&context, ring, divisions, &mut work)?;
                value.add(if index == 0 { area.abs() } else { -area.abs() });
                error.merge(&bound);
            }
        }
        error
            .numerical
            .add(value.value().abs() * 64.0 * f64::EPSILON);
        let bound = error.value();
        if value.value() >= 0.0 && accepted(value.value(), bound, tolerance) {
            return result(&context, value.value(), &error, &mut work);
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
        #[cfg(test)]
        crate::noding::gop028::cycle("preserve_shape_measure.rs::area_primitive:1");
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
) -> Result<(f64, MeasureErrors), ShapeMeasureError> {
    let mut polygon = PolygonArea::new(context.ellipsoid.geodetica(), Winding::CounterClockwise);
    let mut integral = MeasureSum::default();
    let mut error = MeasureErrors::default();
    let parameters = context.ellipsoid.parametri();
    for pair in ring.0.windows(2) {
        #[cfg(test)]
        crate::noding::gop028::cycle("preserve_shape_measure.rs::ring_area:1");
        let (dx, dy) = context.deltas(pair[0], pair[1], divisions)?;
        for i in 0..divisions {
            #[cfg(test)]
            crate::noding::gop028::cycle("preserve_shape_measure.rs::ring_area:2");
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
            match &context.derivatives {
                ShapeDerivatives::Separable(bounds) => {
                    integral.add(-area_primitive(midpoint.y, parameters) * dx);
                    let derivative_bound =
                        1.1 * (bounds.latitude_first.powi(2) + bounds.latitude_second);
                    let scale = dx.abs() * parameters.semi_major_axis_metre.powi(2);
                    error
                        .discretization
                        .add(scale * derivative_bound * dy * dy / 24.0);
                    error.numerical.add(scale * 512.0 * f64::EPSILON);
                }
                ShapeDerivatives::Transverse(_)
                | ShapeDerivatives::Lambert(_)
                | ShapeDerivatives::Azimuthal(_)
                | ShapeDerivatives::Stereographic(_)
                | ShapeDerivatives::Hotine(_) => {
                    let end = context.convert(
                        interpolated(pair[0], pair[1], f64::from(i + 1) / f64::from(divisions)),
                        work,
                    )?;
                    let longitude_delta = (end.x - point.x).to_radians();
                    integral.add(-area_primitive(midpoint.y, parameters) * longitude_delta);
                    // Taylor di Q(phi) attorno al centro; lambda' non e' costante.
                    let delta = dx.abs() + dy.abs();
                    let scale = parameters.semi_major_axis_metre.powi(2);
                    error.discretization.add(scale * 8.25 * delta.powi(3));
                    error.numerical.add(scale * 16384.0 * f64::EPSILON);
                }
            }
        }
    }
    work.charge()?;
    let (_, area, _) = polygon.compute(true);
    // Il valore GeographicLib si confronta con un integrale indipendente con bound.
    // Anche l'errore della primitiva area e della sua accumulazione entra nel residuo.
    error
        .numerical
        .add((area - integral.value()).abs() + 64.0 * f64::EPSILON * area.abs());
    let bound = error.value();
    if !area.is_finite() || !bound.is_finite() {
        return Err(ShapeMeasureError::AccuracyUnproven);
    }
    Ok((area, error))
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
        #[cfg(test)]
        crate::noding::gop028::cycle("preserve_shape_measure.rs::validate_polygon:1");
        if ring.0.len() < 4 || ring.0.first() != ring.0.last() {
            return Err(ShapeMeasureError::UnsupportedTopology);
        }
        for &point in &ring.0 {
            #[cfg(test)]
            crate::noding::gop028::cycle("preserve_shape_measure.rs::validate_polygon:2");
            let geographic = context.convert(point, work)?;
            if geographic.y.abs() >= 90.0 {
                return Err(ShapeMeasureError::UnsupportedSurface);
            }
            west = west.min(geographic.x);
            east = east.max(geographic.x);
        }
        for (i, first) in ring.lines().enumerate() {
            #[cfg(test)]
            crate::noding::gop028::cycle("preserve_shape_measure.rs::validate_polygon:3");
            work.charge()?;
            if first.start == first.end {
                return Err(ShapeMeasureError::UnsupportedTopology);
            }
            for (j, second) in ring.lines().enumerate().skip(i + 1) {
                #[cfg(test)]
                crate::noding::gop028::cycle("preserve_shape_measure.rs::validate_polygon:4");
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
        #[cfg(test)]
        crate::noding::gop028::cycle("preserve_shape_measure.rs::validate_polygon:5");
        inside(hole, shell, true, work)?;
        disjoint_boundaries(hole, shell, work)?;
        for other in polygon.interiors().iter().skip(i + 1) {
            #[cfg(test)]
            crate::noding::gop028::cycle("preserve_shape_measure.rs::validate_polygon:6");
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
        #[cfg(test)]
        crate::noding::gop028::cycle("preserve_shape_measure.rs::inside:1");
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
        #[cfg(test)]
        crate::noding::gop028::cycle("preserve_shape_measure.rs::disjoint_boundaries:1");
        for b in second.lines() {
            #[cfg(test)]
            crate::noding::gop028::cycle("preserve_shape_measure.rs::disjoint_boundaries:2");
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
