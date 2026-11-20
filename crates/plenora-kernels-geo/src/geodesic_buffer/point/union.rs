//! Unione planare dei dischi certificati; composizione GOP-006 in docs/buffer-punto.md.

use super::{
    domain, plan, ring_shared, DegenerateBufferError, EllissoideGeodetico, PointBufferCertificate,
};
use crate::noding::{CountedVec, Work};
use crate::polygon_boolean::{self, PolygonBooleanError, PolygonBooleanLimits, PolygonBooleanOp};
use crate::predicates::PredicateError;
use geo::{Coord, Geometry};
use plenora_core::crs::{CrsKind, ResolvedCrs};
use std::sync::atomic::AtomicBool;

/// Un solo budget per dischi, esportazioni e tutte le unioni del batch.
#[derive(Clone, Copy, Debug)]
pub struct UnionBufferLimits {
    /// Limiti cumulativi della 1c-c, inclusi lati, facce e anelli.
    pub overlay: PolygonBooleanLimits,
    /// Risultati distance-major/input-major, oppure uno per distanza.
    pub max_results: u64,
    /// Vertici generati nei dischi, incluse le chiusure.
    pub max_vertices: u64,
}

/// Errori originali conservati, senza valori degli ingressi.
#[derive(Debug, thiserror::Error)]
pub enum UnionBufferError {
    /// Dominio e certificazione del disco.
    #[error(transparent)]
    Disk(#[from] DegenerateBufferError),
    /// Errore tipizzato della booleana, senza conversione in un risultato.
    #[error(transparent)]
    Overlay(#[from] PolygonBooleanError),
}
impl From<PredicateError> for UnionBufferError {
    fn from(error: PredicateError) -> Self {
        Self::Overlay(PolygonBooleanError::Predicate(error))
    }
}

/// Superficie con lati lineari lon/lat, nella metrica ellissoidale.
#[derive(Debug)]
pub struct UnionBufferSurface<'a> {
    /// Polygon o `MultiPolygon` certificato dalla 1c-c.
    pub geometry: Geometry,
    /// Massimo dei certificati dei dischi, incluso il cambio di modello.
    pub disk_certificate: PointBufferCertificate<'a>,
    /// Maggiorante conservativo del rounding della pubblicazione, in metri.
    pub overlay_m: f64,
    /// GOP-006: max degli errori dei dischi + `overlay_m`.
    pub absolute_m: f64,
    /// Errore assoluto diviso per il raggio, verso l'alto.
    pub relative: f64,
}

/// Batch atomico con lavoro cumulativo misurato.
#[derive(Debug)]
pub struct UnionBufferResult<'a> {
    /// Distanza esterna, ingresso interno; oppure unione per distanza.
    pub surfaces: Vec<UnionBufferSurface<'a>>,
    /// Passi comprendenti generazione, unione ed esportazione.
    pub work: u64,
}

// |lon|<180, |lat|<81: mezzo ulp massimo rispettivamente 2^-46 e 2^-47.
// Nel dominio ellissoidale M e N < 7.1e6 m: 125000 m/grado maggiora
// entrambe le scale a ogni latitudine, inclusa quella peggiore.
// La norma L1 maggiora quella metrica; next_up include i rounding del bound.
fn overlay_error_m() -> f64 {
    (((2.0_f64.powi(-46) + 2.0_f64.powi(-47)).next_up()) * 125_000.0).next_up()
}

fn points(input: &Geometry) -> Result<impl Iterator<Item = Coord> + '_, UnionBufferError> {
    let (single, multiple) = match input {
        Geometry::Point(point) => (Some(point), &[][..]),
        Geometry::MultiPoint(points) if !points.0.is_empty() => (None, points.0.as_slice()),
        _ => return Err(DegenerateBufferError::UnsupportedProfile.into()),
    };
    Ok(single.into_iter().chain(multiple).map(|p| p.0))
}

/// Buffer locale di `Point`/`MultiPoint` distinti, con unione opzionale per distanza.
///
/// Il dominio e le primitive sono quelli del disco; l'output usa segmenti lon/lat.
/// Le ricevute dei buffer esportati restano addebitate fino al ritorno del batch.
/// # Errors
/// Dominio, tolleranza, risorse, interruzione o errore originale della 1c-c.
pub fn geodesic_multipoint_buffer_with_limits<'a>(
    inputs: &[Geometry],
    crs: &'a ResolvedCrs,
    radii_m: &[f64],
    tolerance_m: f64,
    union_per_distance: bool,
    limits: UnionBufferLimits,
    cancelled: Option<&AtomicBool>,
) -> Result<UnionBufferResult<'a>, UnionBufferError> {
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

#[allow(clippy::too_many_lines)]
fn execute<'a>(
    inputs: &[Geometry],
    crs: &'a ResolvedCrs,
    radii_m: &[f64],
    tolerance_m: f64,
    union_per_distance: bool,
    limits: UnionBufferLimits,
    work: &mut Work<'_>,
) -> Result<UnionBufferResult<'a>, UnionBufferError> {
    work.step()?;
    if crs.kind() != CrsKind::Geographic {
        return Err(DegenerateBufferError::GeographicRequired.into());
    }
    if !tolerance_m.is_finite() || tolerance_m <= 0.0 {
        return Err(DegenerateBufferError::InvalidTolerance.into());
    }
    let ellipsoid = EllissoideGeodetico::da_crs(crs)
        .map_err(|_| DegenerateBufferError::UnsupportedEllipsoid)?;
    let parameters = ellipsoid.parametri();
    if !(6e6..=7e6).contains(&parameters.semi_major_axis_metre)
        || !(250.0..=350.0).contains(&parameters.inverse_flattening)
    {
        return Err(DegenerateBufferError::UnsupportedEllipsoid.into());
    }
    let mut count = 0_u32;
    for input in inputs {
        #[cfg(test)]
        crate::noding::gop028::cycle("geodesic_buffer/point/union.rs::execute:1");
        work.step()?;
        for p in points(input)? {
            #[cfg(test)]
            crate::noding::gop028::cycle("geodesic_buffer/point/union.rs::execute:2");
            work.step()?;
            if !((-180.0..=180.0).contains(&p.x) && (-90.0..=90.0).contains(&p.y)) {
                return Err(DegenerateBufferError::InvalidGeometry.into());
            }
            count = count
                .checked_add(1)
                .ok_or(DegenerateBufferError::VertexLimit)?;
        }
    }
    let per_distance = if union_per_distance {
        usize::from(count > 0)
    } else {
        inputs.len()
    };
    let items = per_distance
        .checked_mul(radii_m.len())
        .ok_or(DegenerateBufferError::OutputLimit)?;
    if u64::try_from(items).map_err(|_| DegenerateBufferError::OutputLimit)? > limits.max_results {
        return Err(DegenerateBufferError::OutputLimit.into());
    }
    let overlay_allowance = (f64::from(count.saturating_sub(1)) * overlay_error_m()).next_up();
    let disk_tolerance = (tolerance_m.min(0.01) - overlay_allowance).next_down();
    let mut vertices = 0_u64;
    for &radius in radii_m {
        #[cfg(test)]
        crate::noding::gop028::cycle("geodesic_buffer/point/union.rs::execute:3");
        work.step()?;
        let (n, _) = plan(radius, disk_tolerance, crs)?;
        let mut west = f64::INFINITY;
        let mut east = f64::NEG_INFINITY;
        for input in inputs {
            #[cfg(test)]
            crate::noding::gop028::cycle("geodesic_buffer/point/union.rs::execute:4");
            work.step()?;
            for p in points(input)? {
                #[cfg(test)]
                crate::noding::gop028::cycle("geodesic_buffer/point/union.rs::execute:5");
                work.step()?;
                domain(p, radius)?;
                // La barriera del disco usa 15000 m/grado fino a 81 gradi.
                // Una fascia <180 gradi contiene meno di mezzo ellissoide.
                let extent = ((radius + 1.0).next_up() / 15000.0).next_up();
                west = west.min((p.x - extent).next_down());
                east = east.max((p.x + extent).next_up());
                if (east - west).next_up() >= 180.0 {
                    return Err(DegenerateBufferError::UnsupportedSurface.into());
                }
                vertices = vertices
                    .checked_add(u64::from(n) + 1)
                    .ok_or(DegenerateBufferError::VertexLimit)?;
                if vertices > limits.max_vertices {
                    return Err(DegenerateBufferError::VertexLimit.into());
                }
            }
            if !union_per_distance {
                west = f64::INFINITY;
                east = f64::NEG_INFINITY;
            }
        }
    }
    let mut output = CountedVec::new();
    for &radius in radii_m {
        #[cfg(test)]
        crate::noding::gop028::cycle("geodesic_buffer/point/union.rs::execute:6");
        work.step()?;
        let (n, certificate) = plan(radius, disk_tolerance, crs)?;
        let mut accumulated = CountedVec::new();
        let mut error = 0.0;
        for input in inputs {
            #[cfg(test)]
            crate::noding::gop028::cycle("geodesic_buffer/point/union.rs::execute:7");
            work.step()?;
            for p in points(input)? {
                #[cfg(test)]
                crate::noding::gop028::cycle("geodesic_buffer/point/union.rs::execute:8");
                work.step()?;
                let bytes = (u64::from(n) + 1)
                    .checked_mul(
                        u64::try_from(size_of::<Coord>())
                            .map_err(|_| DegenerateBufferError::MemoryLimit)?,
                    )
                    .ok_or(DegenerateBufferError::MemoryLimit)?;
                work.reserve_export(bytes)?;
                let disk = Geometry::Polygon(ring_shared(p, radius, n, &ellipsoid, || {
                    work.step().map_err(UnionBufferError::from)
                })?);
                work.push(&mut accumulated, disk)?;
                error = (error + overlay_error_m()).next_up();
            }
            if !union_per_distance {
                publish(
                    &mut accumulated,
                    &mut output,
                    certificate,
                    error,
                    radius,
                    tolerance_m,
                    limits,
                    work,
                )?;
                if let Some(surface) = output.last() {
                    super::super::verify_coverage(&surface.geometry, input, work)?;
                }
                error = 0.0;
            }
        }
        if union_per_distance {
            publish(
                &mut accumulated,
                &mut output,
                certificate,
                error,
                radius,
                tolerance_m,
                limits,
                work,
            )?;
            if let Some(surface) = output.last() {
                for input in inputs {
                    #[cfg(test)]
                    crate::noding::gop028::cycle("geodesic_buffer/point/union.rs::execute:9");
                    work.step()?;
                    super::super::verify_coverage(&surface.geometry, input, work)?;
                }
            }
        }
    }
    let surfaces = output.export(work)?;
    work.step()?;
    Ok(UnionBufferResult {
        surfaces,
        work: work.steps,
    })
}

#[allow(clippy::too_many_arguments)]
fn publish<'a>(
    geometry: &mut CountedVec<Geometry>,
    output: &mut CountedVec<UnionBufferSurface<'a>>,
    certificate: PointBufferCertificate<'a>,
    overlay_m: f64,
    radius: f64,
    tolerance_m: f64,
    limits: UnionBufferLimits,
    work: &mut Work<'_>,
) -> Result<(), UnionBufferError> {
    if !geometry.is_empty() {
        let united = polygon_boolean::execute_many(
            geometry,
            &[],
            PolygonBooleanOp::Union,
            Some(2),
            &limits.overlay,
            work,
        )?;
        *geometry = CountedVec::new();
        let geometry = united;
        super::super::verify_surface(&geometry, work)?;
        let absolute_m = (certificate.absolute_m + overlay_m).next_up();
        if absolute_m > tolerance_m.min(0.01) {
            return Err(DegenerateBufferError::AccuracyUnproven.into());
        }
        work.push(
            output,
            UnionBufferSurface {
                geometry,
                disk_certificate: certificate,
                overlay_m,
                absolute_m,
                relative: (absolute_m / radius).next_up(),
            },
        )?;
    }
    Ok(())
}

#[cfg(test)]
#[deny(clippy::disallowed_methods)]
mod tests;
