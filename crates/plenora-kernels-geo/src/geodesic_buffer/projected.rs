//! Buffer mediante TM locale; certificato in docs/buffer-proiezione.md.

#![allow(
    clippy::many_single_char_names,
    reason = "Coordinate e formule geometriche"
)]

use super::point::union::{UnionBufferError, UnionBufferLimits};
use super::DegenerateBufferError as BufferFailure;
use crate::geodetica::EllissoideGeodetico;
use crate::noding::{CountedVec, Work};
use crate::polygon_boolean::{self, PolygonBooleanOp};
use crate::preserve_shape_measure::certificate::TransverseCertificate;
use geo::{Coord, Geometry, LineString, MultiPolygon, Polygon};
use geographiclib_rs::{DirectGeodesic, InverseGeodesic};
use plenora_core::crs::riproiezione::LocalTransverseMercator;
use plenora_core::crs::ResolvedCrs;
use std::sync::atomic::AtomicBool;

mod erosion;
mod surface;

/// GOP-006 nella metrica dell'ellissoide del chiamante.
#[derive(Clone, Copy, Debug)]
pub struct ProjectedBufferCertificate<'a> {
    /// CRS originale, nessun cambio di datum.
    pub crs: &'a ResolvedCrs,
    /// Fattore minimo terreno/piano sul box allargato.
    pub inverse_scale_min: f64,
    /// Fattore massimo terreno/piano sul box allargato.
    pub inverse_scale_max: f64,
    /// Raggio per massima deviazione del fattore inverso da uno.
    pub distortion_m: f64,
    /// Geodetiche di ingresso sostituite con corde nel piano.
    pub input_model_m: f64,
    /// Freccia dei dischi planari, trasportata nella metrica terrestre.
    pub arcs_m: f64,
    /// Inverse dei lati sostituite con segmenti lon/lat.
    pub output_model_m: f64,
    /// Conversioni, costruzioni e overlay cumulativi.
    pub numerical_m: f64,
    /// Copertura continua comune del risultato e dell'erosione esatta.
    pub stability_m: f64,
    /// Somma arrotondata in alto.
    pub absolute_m: f64,
    /// Errore assoluto diviso per il raggio, zero per il vuoto esatto.
    pub relative: f64,
}

impl ProjectedBufferCertificate<'_> {
    /// Modello delle linee originali e della distanza certificata.
    pub const MODEL: &'static str = "shortest_geodesic";
    /// Unita' del raggio e delle componenti del certificato.
    pub const UNIT: &'static str = "metre";
    /// Interpretazione dei lati costruiti; a zero vale il modello dell'operando.
    pub const OUTPUT_MODEL: &'static str = "linear_lonlat";
}

/// Superficie con lati lineari lon/lat; vuoti rappresentati da Polygon.
#[derive(Debug)]
pub struct ProjectedBufferSurface<'a> {
    /// Polygon oppure `MultiPolygon`.
    pub geometry: Geometry,
    /// Bound sul pieno, non una promessa di equivalenza dei contatti.
    pub certificate: ProjectedBufferCertificate<'a>,
    /// A raggio zero conserva i lati geodetici dell'operando superficiale.
    pub preserves_geodesic_operand: bool,
}

/// Batch atomico distance-major/input-major.
#[derive(Debug)]
pub struct ProjectedBufferResult<'a> {
    /// Uno per ingresso e distanza, oppure unione per distanza.
    pub surfaces: Vec<ProjectedBufferSurface<'a>>,
    /// Lavoro cumulativo incluse le booleane.
    pub work: u64,
}

struct LocalContext {
    projection: LocalTransverseMercator,
    certificate: TransverseCertificate,
    ellipsoid: EllissoideGeodetico,
}

impl LocalContext {
    fn forward(&self, point: Coord, work: &mut Work<'_>) -> Result<Coord, UnionBufferError> {
        work.step()?;
        let (x, y) = self
            .projection
            .forward(point.x, point.y)
            .map_err(|_| BufferFailure::UnsupportedSurface)?;
        self.certificate
            .certify(x, y, point.x, point.y)
            .map_err(|_| BufferFailure::AccuracyUnproven)?;
        Ok(Coord { x, y })
    }

    fn inverse(&self, point: Coord, work: &mut Work<'_>) -> Result<Coord, UnionBufferError> {
        work.step()?;
        self.certificate
            .check_domain(point.x, point.y)
            .map_err(|_| BufferFailure::AccuracyUnproven)?;
        let (x, y) = self
            .projection
            .inverse(point.x, point.y)
            .map_err(|_| BufferFailure::AccuracyUnproven)?;
        self.certificate
            .certify(point.x, point.y, x, y)
            .map_err(|_| BufferFailure::AccuracyUnproven)?;
        Ok(Coord { x, y })
    }
}

fn parts(input: &Geometry) -> Result<impl Iterator<Item = &LineString>, UnionBufferError> {
    let (single, multiple, polygons) = match input {
        Geometry::LineString(line) => (Some(line), &[][..], &[][..]),
        Geometry::MultiLineString(lines) => (None, lines.0.as_slice(), &[][..]),
        Geometry::Polygon(polygon) => (None, &[][..], std::slice::from_ref(polygon)),
        Geometry::MultiPolygon(polygons) => (None, &[][..], polygons.0.as_slice()),
        _ => return Err(BufferFailure::UnsupportedProfile.into()),
    };
    Ok(single.into_iter().chain(multiple).chain(
        polygons
            .iter()
            .flat_map(|p| std::iter::once(p.exterior()).chain(p.interiors())),
    ))
}

fn polygons(input: &Geometry) -> &[Polygon] {
    match input {
        Geometry::Polygon(polygon) => std::slice::from_ref(polygon),
        Geometry::MultiPolygon(polygons) => &polygons.0,
        _ => &[],
    }
}

/// Buffer locale di linee e superfici, mantenendo il nome dell'API delle linee.
///
/// Usa TM con meridiano centrale a meta' dell'inviluppo longitudinale.
/// Poli e seam sono esclusi. Il dominio metrico e' piu' stretto di quello
/// della riproiezione ordinaria; |r|<=60000 metri. A zero conserva le superfici.
/// Un unico Work governa tutte le fasi e tutte le distanze.
/// # Errors
/// Errori del buffer e della 1c-c; nessun risultato fuori certificato.
pub fn geodesic_line_buffer_with_limits<'a>(
    inputs: &[Geometry],
    crs: &'a ResolvedCrs,
    radii_m: &[f64],
    tolerance_m: f64,
    union_per_distance: bool,
    limits: UnionBufferLimits,
    cancelled: Option<&AtomicBool>,
) -> Result<ProjectedBufferResult<'a>, UnionBufferError> {
    let mut work = Work::new(limits.overlay.work, cancelled);
    execute(
        inputs,
        crs,
        radii_m,
        tolerance_m,
        union_per_distance,
        limits,
        &mut work,
    )
}

fn execute<'a>(
    inputs: &[Geometry],
    crs: &'a ResolvedCrs,
    radii_m: &[f64],
    tolerance_m: f64,
    union_per_distance: bool,
    limits: UnionBufferLimits,
    work: &mut Work<'_>,
) -> Result<ProjectedBufferResult<'a>, UnionBufferError> {
    work.step()?;
    if !tolerance_m.is_finite() || tolerance_m <= 0.0 {
        return Err(BufferFailure::InvalidTolerance.into());
    }
    let tolerance = tolerance_m;
    if tolerance < 0.001 {
        return Err(BufferFailure::AccuracyUnproven.into());
    }
    let groups = if union_per_distance { 1 } else { inputs.len() };
    let count = groups
        .checked_mul(radii_m.len())
        .ok_or(BufferFailure::OutputLimit)?;
    if u64::try_from(count).map_err(|_| BufferFailure::OutputLimit)? > limits.max_results {
        return Err(BufferFailure::OutputLimit.into());
    }
    let mut output = CountedVec::new();
    let mut vertices = 0_u64;
    for &radius in radii_m {
        #[cfg(test)]
        crate::noding::gop028::cycle("geodesic_buffer/projected.rs::execute:1");
        work.step()?;
        if !radius.is_finite() || radius.abs() > 60_000.0 {
            return Err(BufferFailure::InvalidRadius.into());
        }
        if radius < 0.0 {
            for input in inputs {
                #[cfg(test)]
                crate::noding::gop028::cycle("geodesic_buffer/projected.rs::execute:2");
                work.step()?;
                if !matches!(input, Geometry::Polygon(_) | Geometry::MultiPolygon(_)) {
                    return Err(BufferFailure::InvalidRadius.into());
                }
            }
        }
        for group in 0..groups {
            #[cfg(test)]
            crate::noding::gop028::cycle("geodesic_buffer/projected.rs::execute:3");
            work.step()?;
            let batch = if union_per_distance {
                inputs
            } else {
                &inputs[group..=group]
            };
            let surface = build(batch, crs, radius, tolerance, limits, &mut vertices, work)?;
            super::verify_surface(&surface.geometry, work)?;
            work.push(&mut output, surface)?;
        }
    }
    let surfaces = output.export(work)?;
    work.step()?;
    Ok(ProjectedBufferResult {
        surfaces,
        work: work.steps,
    })
}

const fn empty_certificate(crs: &ResolvedCrs) -> ProjectedBufferCertificate<'_> {
    ProjectedBufferCertificate {
        crs,
        inverse_scale_min: 1.0,
        inverse_scale_max: 1.0,
        distortion_m: 0.0,
        input_model_m: 0.0,
        arcs_m: 0.0,
        output_model_m: 0.0,
        numerical_m: 0.0,
        stability_m: 0.0,
        absolute_m: 0.0,
        relative: 0.0,
    }
}

fn context(
    inputs: &[Geometry],
    crs: &ResolvedCrs,
    work: &mut Work<'_>,
) -> Result<LocalContext, UnionBufferError> {
    let mut west = f64::INFINITY;
    let mut east = f64::NEG_INFINITY;
    for input in inputs {
        #[cfg(test)]
        crate::noding::gop028::cycle("geodesic_buffer/projected.rs::context:1");
        work.step()?;
        for line in parts(input)? {
            #[cfg(test)]
            crate::noding::gop028::cycle("geodesic_buffer/projected.rs::context:2");
            work.step()?;
            if line.0.len() == 1 {
                return Err(BufferFailure::InvalidGeometry.into());
            }
            for point in &line.0 {
                #[cfg(test)]
                crate::noding::gop028::cycle("geodesic_buffer/projected.rs::context:3");
                work.step()?;
                if !point.x.is_finite()
                    || !point.y.is_finite()
                    || point.x.abs() >= 139.0
                    || point.y.abs() >= 50.0
                {
                    return Err(BufferFailure::UnsupportedSurface.into());
                }
                west = west.min(point.x);
                east = east.max(point.x);
            }
        }
    }
    if east - west >= 1.0 {
        return Err(BufferFailure::UnsupportedSurface.into());
    }
    let longitude = if west.is_finite() {
        west.midpoint(east)
    } else {
        0.0
    };
    let projection = LocalTransverseMercator::new(crs, longitude)
        .map_err(|_| BufferFailure::UnsupportedSurface)?;
    let ellipsoid =
        EllissoideGeodetico::da_crs(crs).map_err(|_| BufferFailure::UnsupportedEllipsoid)?;
    let certificate = TransverseCertificate::new(
        projection.inverse_shape_bounds(),
        ellipsoid.parametri().inverse_flattening,
    )
    .map_err(|_| BufferFailure::AccuracyUnproven)?;
    Ok(LocalContext {
        projection,
        certificate,
        ellipsoid,
    })
}

fn vertex(
    points: &mut CountedVec<Coord>,
    point: Coord,
    vertices: &mut u64,
    limits: UnionBufferLimits,
    work: &mut Work<'_>,
) -> Result<(), UnionBufferError> {
    *vertices = vertices
        .checked_add(1)
        .filter(|n| *n <= limits.max_vertices)
        .ok_or(BufferFailure::VertexLimit)?;
    work.push(points, point)?;
    Ok(())
}

fn divisions(length: f64, maximum: f64) -> Result<u32, UnionBufferError> {
    if !length.is_finite() || length < 0.0 {
        return Err(BufferFailure::AccuracyUnproven.into());
    }
    let mut n = 1_u32;
    loop {
        #[cfg(test)]
        crate::noding::gop028::cycle("geodesic_buffer/projected.rs::divisions:1");
        if (length / f64::from(n)).next_up() <= maximum {
            break;
        }
        n = n.checked_mul(2).ok_or(BufferFailure::VertexLimit)?;
    }
    Ok(n)
}

fn projected_line(
    line: &LineString,
    local: &LocalContext,
    tolerance: f64,
    limits: UnionBufferLimits,
    vertices: &mut u64,
    work: &mut Work<'_>,
) -> Result<(LineString, f64), UnionBufferError> {
    let mut points = CountedVec::new();
    let mut bound = 0.0_f64;
    let maximum = (tolerance / 0.001).sqrt().next_down();
    if let Some(&first) = line.0.first() {
        let p = local.forward(first, work)?;
        vertex(&mut points, p, vertices, limits, work)?;
    }
    for pair in line.0.windows(2) {
        #[cfg(test)]
        crate::noding::gop028::cycle("geodesic_buffer/projected.rs::projected_line:1");
        work.step()?;
        let (a, b) = (pair[0], pair[1]);
        let (length, azimuth, _, _): (f64, f64, f64, f64) =
            local.ellipsoid.geodetica().inverse(a.y, a.x, b.y, b.x);
        if length > 60_000.0 {
            return Err(BufferFailure::UnsupportedSurface.into());
        }
        // Certifica la direzione inverse con il residuo all'estremo, non con
        // un'ipotesi sull'errore angolare: stabilita' di exp nel dominio locale.
        work.step()?;
        let (end_latitude, end_longitude): (f64, f64) = local
            .ellipsoid
            .geodetica()
            .direct(a.y, a.x, azimuth, length);
        work.step()?;
        let residual: f64 =
            local
                .ellipsoid
                .geodetica()
                .inverse(b.y, b.x, end_latitude, end_longitude);
        if !residual.is_finite() || residual > 1e-6 {
            return Err(BufferFailure::AccuracyUnproven.into());
        }
        let n = divisions((length + 2e-6).next_up(), maximum)?;
        let h = ((length / f64::from(n)).next_up() + 8e-6).next_up();
        for i in 1..=n {
            #[cfg(test)]
            crate::noding::gop028::cycle("geodesic_buffer/projected.rs::projected_line:2");
            work.step()?;
            let geographic = if i == n {
                b
            } else {
                let (y, x) = local.ellipsoid.geodetica().direct(
                    a.y,
                    a.x,
                    azimuth,
                    length * (f64::from(i) / f64::from(n)),
                );
                Coord { x, y }
            };
            let p = local.forward(geographic, work)?;
            if let Some(previous) = points.last() {
                let span = ((p.x - previous.x).abs().next_up()
                    + (p.y - previous.y).abs().next_up())
                .next_up();
                let inverse_chord = ((1e-6 * span).next_up() * span).next_up();
                let geodesic_chord = ((8e-6 * h).next_up() * h).next_up();
                bound = bound.max((inverse_chord + geodesic_chord).next_up());
            }
            vertex(&mut points, p, vertices, limits, work)?;
        }
    }
    Ok((LineString(points.export(work)?), bound))
}

#[allow(clippy::too_many_lines)]
fn build<'a>(
    inputs: &[Geometry],
    crs: &'a ResolvedCrs,
    radius: f64,
    tolerance: f64,
    limits: UnionBufferLimits,
    vertices: &mut u64,
    work: &mut Work<'_>,
) -> Result<ProjectedBufferSurface<'a>, UnionBufferError> {
    if radius < 0.0 && inputs.len() > 1 {
        return erosion_union(inputs, crs, radius, tolerance, limits, vertices, work);
    }
    let local = context(inputs, crs, work)?;
    build_local(
        inputs, crs, radius, tolerance, limits, vertices, work, &local, true,
    )
}

#[allow(clippy::too_many_lines, clippy::too_many_arguments)]
fn build_local<'a>(
    inputs: &[Geometry],
    crs: &'a ResolvedCrs,
    radius: f64,
    tolerance: f64,
    limits: UnionBufferLimits,
    vertices: &mut u64,
    work: &mut Work<'_>,
    local: &LocalContext,
    publish_output: bool,
) -> Result<ProjectedBufferSurface<'a>, UnionBufferError> {
    if radius == 0.0 {
        return zero_surface(inputs, crs, &local.ellipsoid, limits, vertices, work);
    }
    for input in inputs {
        #[cfg(test)]
        crate::noding::gop028::cycle("geodesic_buffer/projected.rs::build_local:1");
        work.step()?;
        if matches!(input, Geometry::Polygon(_) | Geometry::MultiPolygon(_)) {
            surface::validate(input, &local.ellipsoid, work)?;
            polygon_boolean::execute(
                input,
                &Geometry::Polygon(Polygon::empty()),
                PolygonBooleanOp::Union,
                Some(2),
                limits.overlay,
                work,
            )?;
        }
    }
    let mut certificate = empty_certificate(crs);
    let mut lines = CountedVec::new();
    let mut lower = Coord {
        x: f64::INFINITY,
        y: f64::INFINITY,
    };
    let mut upper = Coord {
        x: f64::NEG_INFINITY,
        y: f64::NEG_INFINITY,
    };
    for input in inputs {
        #[cfg(test)]
        crate::noding::gop028::cycle("geodesic_buffer/projected.rs::build_local:2");
        work.step()?;
        for line in parts(input)? {
            #[cfg(test)]
            crate::noding::gop028::cycle("geodesic_buffer/projected.rs::build_local:3");
            let (projected, error) =
                projected_line(line, local, tolerance.min(0.5), limits, vertices, work)?;
            certificate.input_model_m = certificate.input_model_m.max(error);
            for p in &projected.0 {
                #[cfg(test)]
                crate::noding::gop028::cycle("geodesic_buffer/projected.rs::build_local:4");
                work.step()?;
                lower.x = lower.x.min(p.x);
                lower.y = lower.y.min(p.y);
                upper.x = upper.x.max(p.x);
                upper.y = upper.y.max(p.y);
            }
            work.push(&mut lines, projected)?;
        }
    }
    if certificate.input_model_m >= 0.01 {
        return Err(BufferFailure::AccuracyUnproven.into());
    }
    if !lower.x.is_finite() {
        return Ok(ProjectedBufferSurface {
            geometry: Geometry::Polygon(Polygon::empty()),
            certificate: empty_certificate(crs),
            preserves_geodesic_operand: false,
        });
    }
    let erosion = radius < 0.0;
    let radius = radius.abs();
    let margin = radius.mul_add(2.0, 1.0).next_up();
    let (minimum, maximum) = local
        .certificate
        .local_metric_bounds(
            (
                (lower.x - margin).next_down(),
                (lower.y - margin).next_down(),
            ),
            ((upper.x + margin).next_up(), (upper.y + margin).next_up()),
        )
        .map_err(|_| BufferFailure::AccuracyUnproven)?;
    if (minimum * margin).next_down()
        <= ((radius + certificate.input_model_m).next_up() + 0.0002).next_up()
    {
        return Err(BufferFailure::AccuracyUnproven.into());
    }
    certificate.inverse_scale_min = minimum;
    certificate.inverse_scale_max = maximum;
    if erosion {
        // Ogni punto del pieno proiettato dista dal primo vertice al piu'
        // il diametro L1 del box; l'omotopia aggiunge il cambio di modello.
        let span = ((upper.x - lower.x).next_up() + (upper.y - lower.y).next_up()).next_up();
        let diameter =
            (((maximum * span).next_up() + certificate.input_model_m).next_up() + 0.0002).next_up();
        if radius > diameter {
            return Ok(ProjectedBufferSurface {
                geometry: Geometry::Polygon(Polygon::empty()),
                certificate: empty_certificate(crs),
                preserves_geodesic_operand: false,
            });
        }
    }
    certificate.distortion_m =
        (radius * (1.0 - minimum).next_up().max((maximum - 1.0).next_up())).next_up();
    certificate.numerical_m = 0.000_203;
    certificate.output_model_m = tolerance / 16.0;
    if erosion {
        certificate.stability_m = 0.75 * tolerance;
    }
    let arc_budget = 0.75
        * (tolerance
            - certificate.distortion_m
            - certificate.input_model_m
            - certificate.output_model_m
            - certificate.numerical_m
            - certificate.stability_m);
    let arc_budget = if erosion {
        arc_budget.min(tolerance / 32.0)
    } else {
        arc_budget
    };
    if arc_budget <= 0.0 {
        return Err(BufferFailure::AccuracyUnproven.into());
    }
    let mut n = 16_u32;
    loop {
        #[cfg(test)]
        crate::noding::gop028::cycle("geodesic_buffer/projected.rs::build_local:5");
        certificate.arcs_m =
            ((radius / (2.0 * f64::from(n).powi(2))).next_up() * maximum).next_up();
        if certificate.arcs_m <= arc_budget {
            break;
        }
        n = n.checked_mul(2).ok_or(BufferFailure::VertexLimit)?;
    }
    certify_total(&mut certificate, radius, tolerance)?;
    let mut accumulated = CountedVec::new();
    for line in &lines[..] {
        #[cfg(test)]
        crate::noding::gop028::cycle("geodesic_buffer/projected.rs::build_local:6");
        work.step()?;
        for &p in &line.0 {
            #[cfg(test)]
            crate::noding::gop028::cycle("geodesic_buffer/projected.rs::build_local:7");
            let disk = disk(p, radius, n, limits, vertices, work)?;
            unite(
                &mut accumulated,
                disk,
                limits,
                &mut certificate.numerical_m,
                work,
            )?;
        }
        for pair in line.0.windows(2) {
            #[cfg(test)]
            crate::noding::gop028::cycle("geodesic_buffer/projected.rs::build_local:8");
            work.step()?;
            if pair[0] == pair[1] {
                continue;
            }
            let corridor = rectangle(pair[0], pair[1], radius, limits, vertices, work)?;
            unite(
                &mut accumulated,
                corridor,
                limits,
                &mut certificate.numerical_m,
                work,
            )?;
        }
    }
    let source_begin = accumulated.len();
    let mut operands = CountedVec::new();
    let mut index = 0;
    for input in inputs {
        #[cfg(test)]
        crate::noding::gop028::cycle("geodesic_buffer/projected.rs::build_local:9");
        work.step()?;
        if polygons(input).is_empty() {
            index += parts(input)?.count();
            continue;
        }
        for polygon in polygons(input) {
            #[cfg(test)]
            crate::noding::gop028::cycle("geodesic_buffer/projected.rs::build_local:10");
            let mut rings = CountedVec::new();
            for _ in std::iter::once(polygon.exterior()).chain(polygon.interiors()) {
                #[cfg(test)]
                crate::noding::gop028::cycle("geodesic_buffer/projected.rs::build_local:11");
                let ring = copy_ring(&lines[index], limits, vertices, work)?;
                work.push(&mut rings, ring)?;
                index += 1;
            }
            let operand = lattice(rings_polygon(rings, work)?, work)?;
            if erosion {
                work.push(&mut operands, operand)?;
            } else {
                unite(
                    &mut accumulated,
                    Geometry::Polygon(operand),
                    limits,
                    &mut certificate.numerical_m,
                    work,
                )?;
            }
        }
    }
    let planar = union_many(&accumulated, limits, work)?;
    let planar = if erosion {
        let operand = Geometry::MultiPolygon(MultiPolygon(operands.export(work)?));
        // Le corde delle parti devono restare disgiunte: un'unione che
        // cancellasse frontiere non conserva il bound della distanza firmata.
        let operand = polygon_boolean::execute(
            &operand,
            &Geometry::Polygon(Polygon::empty()),
            PolygonBooleanOp::Union,
            Some(2),
            limits.overlay,
            work,
        )?;
        certificate.numerical_m = (certificate.numerical_m + 2e-9).next_up();
        difference(&operand, &planar, radius, &mut certificate, limits, work)?
    } else {
        planar
    };
    if !erosion {
        for operand in &accumulated[source_begin..] {
            #[cfg(test)]
            crate::noding::gop028::cycle("geodesic_buffer/projected.rs::build_local:12");
            super::verify_coverage(&planar, operand, work)?;
        }
        let margin = topology_margin(&certificate)?;
        for line in &lines[..] {
            #[cfg(test)]
            crate::noding::gop028::cycle("geodesic_buffer/projected.rs::build_local:13");
            erosion::clearance(line, &planar, margin, work)?;
        }
    }
    let geometry = if publish_output {
        publish(
            planar,
            local,
            certificate.output_model_m,
            limits,
            vertices,
            work,
        )?
    } else {
        planar
    };
    certify_total(&mut certificate, radius, tolerance)?;
    work.step()?;
    Ok(ProjectedBufferSurface {
        geometry,
        certificate,
        preserves_geodesic_operand: false,
    })
}

fn topology_margin(c: &ProjectedBufferCertificate<'_>) -> Result<f64, UnionBufferError> {
    let margin = (((c.input_model_m + c.output_model_m).next_up()
        + (c.numerical_m + 4e-9).next_up())
    .next_up()
        / c.inverse_scale_min)
        .next_up();
    if !margin.is_finite() || margin < 0.0 {
        return Err(BufferFailure::AccuracyUnproven.into());
    }
    Ok(margin)
}

fn difference(
    operand: &Geometry,
    corridors: &Geometry,
    radius: f64,
    certificate: &mut ProjectedBufferCertificate<'_>,
    limits: UnionBufferLimits,
    work: &mut Work<'_>,
) -> Result<Geometry, UnionBufferError> {
    let result = polygon_boolean::execute(
        operand,
        corridors,
        PolygonBooleanOp::Difference,
        Some(2),
        limits.overlay,
        work,
    )?;
    certificate.numerical_m = (certificate.numerical_m + 2e-9).next_up();
    erosion::certify(operand, &result, radius, certificate, work)?;
    super::verify_coverage(operand, &result, work)?;
    let margin = topology_margin(certificate)?;
    for ring in parts(&result)? {
        #[cfg(test)]
        crate::noding::gop028::cycle("geodesic_buffer/projected.rs::difference:1");
        erosion::clearance(ring, operand, margin, work)?;
    }
    Ok(result)
}

fn erosion_union<'a>(
    inputs: &[Geometry],
    crs: &'a ResolvedCrs,
    radius: f64,
    tolerance: f64,
    limits: UnionBufferLimits,
    vertices: &mut u64,
    work: &mut Work<'_>,
) -> Result<ProjectedBufferSurface<'a>, UnionBufferError> {
    let local = context(inputs, crs, work)?;
    let mut accumulated = CountedVec::new();
    let mut certificate = empty_certificate(crs);
    let mut rounding = 0.0;
    for input in inputs {
        #[cfg(test)]
        crate::noding::gop028::cycle("geodesic_buffer/projected.rs::erosion_union:1");
        work.step()?;
        let surface = build_local(
            std::slice::from_ref(input),
            crs,
            radius,
            tolerance,
            limits,
            vertices,
            work,
            &local,
            false,
        )?;
        let c = surface.certificate;
        certificate.inverse_scale_min = certificate.inverse_scale_min.min(c.inverse_scale_min);
        certificate.inverse_scale_max = certificate.inverse_scale_max.max(c.inverse_scale_max);
        certificate.distortion_m = certificate.distortion_m.max(c.distortion_m);
        certificate.input_model_m = certificate.input_model_m.max(c.input_model_m);
        certificate.arcs_m = certificate.arcs_m.max(c.arcs_m);
        certificate.output_model_m = certificate.output_model_m.max(c.output_model_m);
        certificate.numerical_m = certificate.numerical_m.max(c.numerical_m);
        certificate.stability_m = certificate.stability_m.max(c.stability_m);
        unite(
            &mut accumulated,
            surface.geometry,
            limits,
            &mut rounding,
            work,
        )?;
    }
    // Una pubblicazione nel piano comune, prima della densificazione.
    certificate.numerical_m = (certificate.numerical_m + (2.0 * rounding).next_up()).next_up();
    certify_total(&mut certificate, radius.abs(), tolerance)?;
    let geometry = union_many(&accumulated, limits, work)?;
    let geometry = publish(
        geometry,
        &local,
        certificate.output_model_m,
        limits,
        vertices,
        work,
    )?;
    let geometry = match geometry {
        Geometry::MultiPolygon(multi) if multi.0.is_empty() => Geometry::Polygon(Polygon::empty()),
        geometry => geometry,
    };
    Ok(ProjectedBufferSurface {
        geometry,
        certificate,
        preserves_geodesic_operand: false,
    })
}

fn copy_ring(
    ring: &LineString,
    limits: UnionBufferLimits,
    vertices: &mut u64,
    work: &mut Work<'_>,
) -> Result<LineString, UnionBufferError> {
    let mut points = CountedVec::new();
    for &p in &ring.0 {
        #[cfg(test)]
        crate::noding::gop028::cycle("geodesic_buffer/projected.rs::copy_ring:1");
        work.step()?;
        vertex(&mut points, p, vertices, limits, work)?;
    }
    Ok(LineString(points.export(work)?))
}

fn rings_polygon(
    rings: CountedVec<LineString>,
    work: &mut Work<'_>,
) -> Result<Polygon, UnionBufferError> {
    let mut rings = rings.export(work)?.into_iter();
    let shell = rings.next().ok_or(BufferFailure::InvalidGeometry)?;
    let mut holes = CountedVec::new();
    for hole in rings {
        #[cfg(test)]
        crate::noding::gop028::cycle("geodesic_buffer/projected.rs::rings_polygon:1");
        work.push(&mut holes, hole)?;
    }
    Ok(Polygon::new(shell, holes.export(work)?))
}

fn zero_surface<'a>(
    inputs: &[Geometry],
    crs: &'a ResolvedCrs,
    ellipsoid: &EllissoideGeodetico,
    limits: UnionBufferLimits,
    vertices: &mut u64,
    work: &mut Work<'_>,
) -> Result<ProjectedBufferSurface<'a>, UnionBufferError> {
    let mut result = CountedVec::new();
    let mut surface_inputs = 0_u64;
    let mut preserve_multi = false;
    for input in inputs {
        #[cfg(test)]
        crate::noding::gop028::cycle("geodesic_buffer/projected.rs::zero_surface:1");
        work.step()?;
        if !polygons(input).is_empty() {
            surface_inputs += 1;
            preserve_multi |= matches!(input, Geometry::MultiPolygon(_));
        }
        for polygon in polygons(input) {
            #[cfg(test)]
            crate::noding::gop028::cycle("geodesic_buffer/projected.rs::zero_surface:2");
            work.step()?;
            let mut rings = CountedVec::new();
            for ring in std::iter::once(polygon.exterior()).chain(polygon.interiors()) {
                #[cfg(test)]
                crate::noding::gop028::cycle("geodesic_buffer/projected.rs::zero_surface:3");
                let ring = copy_ring(ring, limits, vertices, work)?;
                work.push(&mut rings, ring)?;
            }
            let validated = Geometry::Polygon(rings_polygon(rings, work)?);
            surface::validate(&validated, ellipsoid, work)?;
            polygon_boolean::execute(
                &validated,
                &Geometry::Polygon(Polygon::empty()),
                PolygonBooleanOp::Union,
                Some(2),
                limits.overlay,
                work,
            )?;
            if polygon.exterior().0.is_empty() {
                continue;
            }
            let mut rings = CountedVec::new();
            for (index, ring) in std::iter::once(polygon.exterior())
                .chain(polygon.interiors())
                .enumerate()
            {
                #[cfg(test)]
                crate::noding::gop028::cycle("geodesic_buffer/projected.rs::zero_surface:4");
                let mut copied = copy_ring(ring, limits, vertices, work)?;
                let expected = if index == 0 {
                    std::cmp::Ordering::Greater
                } else {
                    std::cmp::Ordering::Less
                };
                if super::ring_orientation(&copied, work)? != expected {
                    // Chiusura identica: reversal conserva il primo sito e tutti i lati.
                    for _ in &copied.0 {
                        #[cfg(test)]
                        crate::noding::gop028::cycle(
                            "geodesic_buffer/projected.rs::zero_surface:5",
                        );
                        work.step()?;
                    }
                    copied.0.reverse();
                }
                work.push(&mut rings, copied)?;
            }
            let polygon = rings_polygon(rings, work)?;
            let mut duplicate = false;
            for previous in &result[..] {
                #[cfg(test)]
                crate::noding::gop028::cycle("geodesic_buffer/projected.rs::zero_surface:6");
                if same_polygon(previous, &polygon, work)? {
                    duplicate = true;
                    break;
                }
            }
            if duplicate {
                continue;
            }
            work.push(&mut result, polygon)?;
        }
    }
    let geometry = match result.len() {
        0 => Geometry::Polygon(Polygon::empty()),
        1 if !preserve_multi => {
            Geometry::Polygon(result.pop().ok_or(BufferFailure::InvalidGeometry)?)
        }
        _ => Geometry::MultiPolygon(MultiPolygon(result.export(work)?)),
    };
    surface::validate_contacts(&geometry, ellipsoid, work)?;
    let normalized = zero_union(&geometry, limits, vertices, work)?;
    let geometry = if polygons(&geometry).len() <= 1 {
        geometry
    } else {
        normalized
    };
    Ok(ProjectedBufferSurface {
        geometry,
        certificate: empty_certificate(crs),
        preserves_geodesic_operand: surface_inputs != 0,
    })
}

fn zero_union(
    geometry: &Geometry,
    limits: UnionBufferLimits,
    vertices: &mut u64,
    work: &mut Work<'_>,
) -> Result<Geometry, UnionBufferError> {
    let mut components = CountedVec::new();
    for polygon in polygons(geometry) {
        #[cfg(test)]
        crate::noding::gop028::cycle("geodesic_buffer/projected.rs::zero_union:1");
        let mut rings = CountedVec::new();
        for ring in std::iter::once(polygon.exterior()).chain(polygon.interiors()) {
            #[cfg(test)]
            crate::noding::gop028::cycle("geodesic_buffer/projected.rs::zero_union:2");
            let copied = copy_ring(ring, limits, vertices, work)?;
            work.push(&mut rings, copied)?;
        }
        let operand = Geometry::Polygon(rings_polygon(rings, work)?);
        work.push(&mut components, operand)?;
    }
    let normalized = union_many(&components, limits, work)?;
    // Una booleana a zero puo' eliminare lati, non sostituire geodetiche
    // con sottoarchi lon/lat: ogni lato pubblicato deve avere provenienza.
    for ring in parts(&normalized)? {
        #[cfg(test)]
        crate::noding::gop028::cycle("geodesic_buffer/projected.rs::zero_union:3");
        work.step()?;
        for edge in ring.0.windows(2) {
            #[cfg(test)]
            crate::noding::gop028::cycle("geodesic_buffer/projected.rs::zero_union:4");
            let mut found = false;
            for original in parts(geometry)? {
                #[cfg(test)]
                crate::noding::gop028::cycle("geodesic_buffer/projected.rs::zero_union:5");
                work.step()?;
                for source in original.0.windows(2) {
                    #[cfg(test)]
                    crate::noding::gop028::cycle("geodesic_buffer/projected.rs::zero_union:6");
                    work.step()?;
                    if (edge[0] == source[0] && edge[1] == source[1])
                        || (edge[1] == source[0] && edge[0] == source[1])
                        || (surface::axis_contains(source[0], source[1], edge[0])
                            && surface::axis_contains(source[0], source[1], edge[1]))
                    {
                        found = true;
                    }
                }
            }
            if !found {
                return Err(BufferFailure::AccuracyUnproven.into());
            }
        }
    }
    Ok(normalized)
}

fn same_polygon(a: &Polygon, b: &Polygon, work: &mut Work<'_>) -> Result<bool, UnionBufferError> {
    work.step()?;
    if a.interiors().len() != b.interiors().len() {
        return Ok(false);
    }
    for (a, b) in std::iter::once(a.exterior())
        .chain(a.interiors())
        .zip(std::iter::once(b.exterior()).chain(b.interiors()))
    {
        #[cfg(test)]
        crate::noding::gop028::cycle("geodesic_buffer/projected.rs::same_polygon:1");
        work.step()?;
        if a.0.len() != b.0.len() {
            return Ok(false);
        }
        for (a, b) in a.0.iter().zip(&b.0) {
            #[cfg(test)]
            crate::noding::gop028::cycle("geodesic_buffer/projected.rs::same_polygon:2");
            work.step()?;
            if a != b {
                return Ok(false);
            }
        }
    }
    Ok(true)
}

fn certify_total(
    c: &mut ProjectedBufferCertificate<'_>,
    radius: f64,
    tolerance: f64,
) -> Result<(), UnionBufferError> {
    c.absolute_m = [
        c.distortion_m,
        c.input_model_m,
        c.arcs_m,
        c.output_model_m,
        c.numerical_m,
        c.stability_m,
    ]
    .into_iter()
    .fold(0.0_f64, |sum, term| (sum + term).next_up());
    if !c.absolute_m.is_finite() || c.absolute_m > tolerance {
        return Err(BufferFailure::AccuracyUnproven.into());
    }
    c.relative = (c.absolute_m / radius).next_up();
    if !c.relative.is_finite() {
        return Err(BufferFailure::AccuracyUnproven.into());
    }
    Ok(())
}

fn disk(
    center: Coord,
    radius: f64,
    n: u32,
    limits: UnionBufferLimits,
    vertices: &mut u64,
    work: &mut Work<'_>,
) -> Result<Geometry, UnionBufferError> {
    let mut points = CountedVec::new();
    for quadrant in 0..4 {
        #[cfg(test)]
        crate::noding::gop028::cycle("geodesic_buffer/projected.rs::disk:1");
        for i in 0..n {
            #[cfg(test)]
            crate::noding::gop028::cycle("geodesic_buffer/projected.rs::disk:2");
            work.step()?;
            let t = f64::from(i) / f64::from(n);
            let divisor = t.mul_add(t, 1.0);
            let cosine = (-t).mul_add(t, 1.0) / divisor;
            let sine = 2.0 * t / divisor;
            let (x, y) = match quadrant {
                0 => (cosine, sine),
                1 => (-sine, cosine),
                2 => (-cosine, -sine),
                _ => (sine, -cosine),
            };
            vertex(
                &mut points,
                Coord {
                    x: radius.mul_add(x, center.x),
                    y: radius.mul_add(y, center.y),
                },
                vertices,
                limits,
                work,
            )?;
        }
    }
    let first = *points.first().ok_or(BufferFailure::AccuracyUnproven)?;
    vertex(&mut points, first, vertices, limits, work)?;
    let polygon = Polygon::new(LineString(points.export(work)?), Vec::new());
    Ok(Geometry::Polygon(lattice(polygon, work)?))
}

fn rectangle(
    a: Coord,
    b: Coord,
    radius: f64,
    limits: UnionBufferLimits,
    vertices: &mut u64,
    work: &mut Work<'_>,
) -> Result<Geometry, UnionBufferError> {
    work.step()?;
    let dx = b.x - a.x;
    let dy = b.y - a.y;
    let squared_length = dx.mul_add(dx, dy * dy);
    if !squared_length.is_normal() {
        return Err(BufferFailure::AccuracyUnproven.into());
    }
    let length = squared_length.sqrt();
    if !length.is_normal() {
        return Err(BufferFailure::AccuracyUnproven.into());
    }
    let nx = -radius * dy / length;
    let ny = radius * dx / length;
    let mut points = CountedVec::new();
    for (p, sign) in [(a, -1.0), (b, -1.0), (b, 1.0), (a, 1.0), (a, -1.0)] {
        #[cfg(test)]
        crate::noding::gop028::cycle("geodesic_buffer/projected.rs::rectangle:1");
        vertex(
            &mut points,
            Coord {
                x: nx.mul_add(sign, p.x),
                y: ny.mul_add(sign, p.y),
            },
            vertices,
            limits,
            work,
        )?;
    }
    let polygon = Polygon::new(LineString(points.export(work)?), Vec::new());
    Ok(Geometry::Polygon(lattice(polygon, work)?))
}

// Reticolo dyadico delle sole primitive, prima del grafo razionale. Su
// |XY|<=7e6 la scalatura e' esatta; due spostamenti L1 trasportati con
// bmax<1.062 sono <2.1 micrometri, coperti dai 3 micrometri aggiunti.
fn lattice(polygon: Polygon, work: &mut Work<'_>) -> Result<Polygon, UnionBufferError> {
    let (mut shell, mut holes) = polygon.into_inner();
    for ring in std::iter::once(&mut shell).chain(&mut holes) {
        #[cfg(test)]
        crate::noding::gop028::cycle("geodesic_buffer/projected.rs::lattice:1");
        work.step()?;
        for point in &mut ring.0 {
            #[cfg(test)]
            crate::noding::gop028::cycle("geodesic_buffer/projected.rs::lattice:2");
            work.step()?;
            if !point.x.is_finite()
                || !point.y.is_finite()
                || point.x.abs().max(point.y.abs()) > 7e6
            {
                return Err(BufferFailure::AccuracyUnproven.into());
            }
            let x = (point.x * 1_048_576.0).round_ties_even() / 1_048_576.0;
            let y = (point.y * 1_048_576.0).round_ties_even() / 1_048_576.0;
            if ((x - point.x).abs().next_up() + (y - point.y).abs().next_up()).next_up()
                > (1.0_f64 / 1_048_576.0).next_up().next_up()
            {
                return Err(BufferFailure::AccuracyUnproven.into());
            }
            *point = Coord { x, y };
        }
    }
    // Nessuna eliminazione o riparazione: la booleana valida ogni primitiva.
    Ok(Polygon::new(shell, holes))
}

fn unite(
    accumulated: &mut CountedVec<Geometry>,
    operand: Geometry,
    _limits: UnionBufferLimits,
    numerical: &mut f64,
    work: &mut Work<'_>,
) -> Result<(), UnionBufferError> {
    work.push(accumulated, operand)?;
    // Una sola pubblicazione; questa somma resta un maggiorante conservativo.
    *numerical = (*numerical + 2e-9).next_up();
    Ok(())
}

fn union_many(
    operands: &[Geometry],
    limits: UnionBufferLimits,
    work: &mut Work<'_>,
) -> Result<Geometry, UnionBufferError> {
    Ok(polygon_boolean::execute_many(
        operands,
        &[],
        PolygonBooleanOp::Union,
        Some(2),
        &limits.overlay,
        work,
    )?)
}

fn publish(
    geometry: Geometry,
    local: &LocalContext,
    model: f64,
    limits: UnionBufferLimits,
    vertices: &mut u64,
    work: &mut Work<'_>,
) -> Result<Geometry, UnionBufferError> {
    let mut polygons = CountedVec::new();
    let (single, multiple) = match geometry {
        Geometry::Polygon(p) => (Some(p), Vec::new()),
        Geometry::MultiPolygon(p) => (None, p.0),
        _ => return Err(BufferFailure::AccuracyUnproven.into()),
    };
    for polygon in single.into_iter().chain(multiple) {
        #[cfg(test)]
        crate::noding::gop028::cycle("geodesic_buffer/projected.rs::publish:1");
        let mut rings = CountedVec::new();
        for ring in std::iter::once(polygon.exterior()).chain(polygon.interiors()) {
            #[cfg(test)]
            crate::noding::gop028::cycle("geodesic_buffer/projected.rs::publish:2");
            let mut points = CountedVec::new();
            for pair in ring.0.windows(2) {
                #[cfg(test)]
                crate::noding::gop028::cycle("geodesic_buffer/projected.rs::publish:3");
                work.step()?;
                let a = pair[0];
                let b = pair[1];
                let length = ((a.x - b.x).abs().next_up() + (a.y - b.y).abs().next_up()).next_up();
                let n = divisions(length, (model / 1e-6).sqrt().next_down())?;
                for i in 0..n {
                    #[cfg(test)]
                    crate::noding::gop028::cycle("geodesic_buffer/projected.rs::publish:4");
                    let fraction = f64::from(i) / f64::from(n);
                    let p = Coord {
                        x: (b.x - a.x).mul_add(fraction, a.x),
                        y: (b.y - a.y).mul_add(fraction, a.y),
                    };
                    let geographic = local.inverse(p, work)?;
                    vertex(&mut points, geographic, vertices, limits, work)?;
                }
            }
            if let Some(&first) = points.first() {
                vertex(&mut points, first, vertices, limits, work)?;
            }
            let coordinates = points.export(work)?;
            work.push(&mut rings, LineString(coordinates))?;
        }
        let mut rings = rings.export(work)?.into_iter();
        let shell = rings.next().ok_or(BufferFailure::AccuracyUnproven)?;
        let mut holes = CountedVec::new();
        for hole in rings {
            #[cfg(test)]
            crate::noding::gop028::cycle("geodesic_buffer/projected.rs::publish:5");
            work.push(&mut holes, hole)?;
        }
        let interiors = holes.export(work)?;
        work.push(&mut polygons, Polygon::new(shell, interiors))?;
    }
    if polygons.is_empty() {
        work.step()?;
        return Ok(Geometry::Polygon(Polygon::empty()));
    }
    let geometry = match polygons.len() {
        0 => Geometry::Polygon(Polygon::empty()),
        1 => Geometry::Polygon(polygons.pop().ok_or(BufferFailure::AccuracyUnproven)?),
        _ => Geometry::MultiPolygon(MultiPolygon(polygons.export(work)?)),
    };
    // La booleana opera prima della densificazione. Il verificatore legge
    // il contorno finale senza costruire o arrotondare altri nodi.
    super::verify_surface(&geometry, work)?;
    Ok(geometry)
}

#[cfg(test)]
#[deny(clippy::disallowed_methods)]
mod tests;
