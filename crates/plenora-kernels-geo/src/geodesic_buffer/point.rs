//! Disco locale di un punto, senza unione. Prova: `docs/buffer-punto.md`.

use super::{BufferWork, DegenerateBufferError, DegenerateBufferLimits};
use crate::geodetica::EllissoideGeodetico;
use geo::kernels::{Kernel, Orientation, RobustKernel};
use geo::{Coord, Geometry, LineString, Polygon};
use geographiclib_rs::DirectGeodesic;
use plenora_core::crs::{CrsKind, ResolvedCrs};
use std::sync::atomic::AtomicBool;

/// Budget cumulativi del batch atomico.
#[derive(Clone, Copy, Debug)]
pub struct PointBufferLimits {
    /// Lavoro, numero di risultati, memoria richiesta e deadline.
    /// La memoria conta gli slot dei risultati e delle coordinate, senza
    /// input, stack costante e overhead dell'allocatore.
    pub batch: DegenerateBufferLimits,
    /// Coordinate di tutti gli anelli, incluse le chiusure.
    pub max_vertices: u64,
}

/// Certificato GOP-006, Hausdorff simmetrico nella metrica dell'ellissoide.
/// Non promette inclusione in un solo verso; relativo riferito al raggio.
#[derive(Clone, Copy, Debug)]
pub struct PointBufferCertificate<'a> {
    /// Datum ed ellissoide del chiamante, nessuna trasformazione.
    pub crs: &'a ResolvedCrs,
    /// Maggiorante totale assoluto in metri, arrotondato verso l'alto.
    pub absolute_m: f64,
    /// Maggiorante totale diviso per il raggio, verso l'alto.
    pub relative: f64,
    /// Campionamento del confine esatto.
    pub discretization_m: f64,
    /// Cambio da arco geodetico a segmento lineare nelle coordinate lon/lat.
    pub model_change_m: f64,
    /// Due errori di primitiva da 0.1 micrometri; fonte in docs.
    pub geographiclib_m: f64,
    /// Due margini di coordinate/azimut e aritmetica, da 1 micrometro.
    pub rounding_m: f64,
}

impl PointBufferCertificate<'_> {
    /// Metrica del disco esatto e della distanza certificata.
    pub const MODEL: &'static str = "shortest_geodesic";
    /// Unita' dei raggi e dell'errore assoluto.
    pub const UNIT: &'static str = "metre";
    /// Nessun cambio di CRS.
    pub const TRANSFORMATION_M: f64 = 0.0;
}

/// Un disco, anello antiorario chiuso senza buchi.
#[derive(Debug)]
pub struct PointBufferDisk<'a> {
    /// Il certificato copre sia archi geodetici sia segmenti lineari lon/lat.
    pub polygon: Polygon,
    /// Garanzia sul confine e sul disco pieno, con topologia di disco.
    pub certificate: PointBufferCertificate<'a>,
}

/// Risultati distance-major/input-major; nessun prefisso in caso di errore.
#[derive(Debug)]
pub struct PointBufferResult<'a> {
    /// Un disco per coppia raggio/ingresso, duplicati conservati.
    pub disks: Vec<PointBufferDisk<'a>>,
    /// Coordinate emesse, incluse le chiusure.
    pub vertices: u64,
    /// Byte richiesti cumulativi, tutti vivi alla pubblicazione.
    pub peak_reserved_bytes: u64,
    /// Unita' di lavoro effettive, inclusa la prevalidazione.
    pub work: u64,
}

const NUMERIC_M: f64 = 2e-7;
const ROUNDING_M: f64 = 2e-6;

/// Maggiorante metrico del cambio di modello, in metri.
/// Premesse: ellissoide del profilo, estremi entro 81 gradi, nessun seam,
/// lunghezza geodetica <= h <= 60 km. Prova in docs/buffer-punto.md.
fn side_model_error_m(h: f64) -> Result<f64, DegenerateBufferError> {
    if !h.is_finite() || !(0.0..=60_000.0).contains(&h) {
        return Err(DegenerateBufferError::AccuracyUnproven);
    }
    if h == 0.0 {
        return Ok(0.0);
    }
    Ok(((8e-6_f64.next_up() * h).next_up() * h).next_up())
}

fn center(input: &Geometry, work: &mut BufferWork<'_>) -> Result<Coord, DegenerateBufferError> {
    work.step()?;
    match input {
        Geometry::Point(p) => {
            work.coordinate(p.0)?;
            Ok(p.0)
        }
        Geometry::MultiPoint(points) => {
            let first = points
                .0
                .first()
                .ok_or(DegenerateBufferError::UnsupportedProfile)?
                .0;
            for p in &points.0 {
                work.coordinate(p.0)?;
                if p.0 != first {
                    return Err(DegenerateBufferError::UnsupportedProfile);
                }
            }
            Ok(first)
        }
        _ => Err(DegenerateBufferError::UnsupportedProfile),
    }
}

fn domain(p: Coord, radius: f64) -> Result<(), DegenerateBufferError> {
    // M >= 5.8e6 m; con |lat|<=80 e r<=60 km il disco resta sotto 81 gradi.
    // N cos(81 gradi) * pi/180 > 15000 m/grado: barriera del seam.
    if !(1.0..=60_000.0).contains(&radius)
        || p.y.abs() > 80.0
        || ((180.0 - p.x.abs()).next_down() * 15_000.0).next_down() <= (radius + 1.0).next_up()
    {
        return Err(DegenerateBufferError::UnsupportedSurface);
    }
    Ok(())
}

fn plan(
    radius: f64,
    tolerance_m: f64,
    crs: &ResolvedCrs,
) -> Result<(u32, PointBufferCertificate<'_>), DegenerateBufferError> {
    if !radius.is_finite() || radius <= 0.0 {
        return Err(DegenerateBufferError::InvalidRadius);
    }
    if !(1.0..=60_000.0).contains(&radius) {
        return Err(DegenerateBufferError::UnsupportedSurface);
    }
    // Il pavimento separa gli azimut anche con l'errore delle primitive.
    if tolerance_m < 0.0001 {
        return Err(DegenerateBufferError::AccuracyUnproven);
    }
    let mut n = 16_u32;
    loop {
        // 4 > pi; n potenza di due, conversione u32 -> f64 esatta.
        let discretization_m = (4.0 * radius / f64::from(n)).next_up();
        // 8 > 2*pi; include lo spostamento di entrambi gli estremi numerici.
        let h = ((8.0 * radius / f64::from(n)).next_up() + NUMERIC_M).next_up();
        let model_change_m = side_model_error_m((h + ROUNDING_M).next_up())?;
        let absolute_m = (((discretization_m + NUMERIC_M).next_up() + ROUNDING_M).next_up()
            + model_change_m)
            .next_up();
        if absolute_m <= tolerance_m.min(0.01) {
            return Ok((
                n,
                PointBufferCertificate {
                    crs,
                    absolute_m,
                    relative: (absolute_m / radius).next_up(),
                    discretization_m,
                    model_change_m,
                    geographiclib_m: NUMERIC_M,
                    rounding_m: ROUNDING_M,
                },
            ));
        }
        n = n
            .checked_mul(2)
            .ok_or(DegenerateBufferError::AccuracyUnproven)?;
    }
}

fn ring(
    p: Coord,
    radius: f64,
    n: u32,
    ellipsoid: &EllissoideGeodetico,
    checkpoint: impl FnMut() -> Result<(), DegenerateBufferError>,
) -> Result<Polygon, DegenerateBufferError> {
    ring_shared(p, radius, n, ellipsoid, checkpoint)
}

fn ring_shared<E: From<DegenerateBufferError>>(
    p: Coord,
    radius: f64,
    n: u32,
    ellipsoid: &EllissoideGeodetico,
    mut checkpoint: impl FnMut() -> Result<(), E>,
) -> Result<Polygon, E> {
    checkpoint()?;
    let count =
        usize::try_from(u64::from(n) + 1).map_err(|_| DegenerateBufferError::VertexLimit)?;
    let mut vertices = Vec::new();
    vertices
        .try_reserve_exact(count)
        .map_err(|_| DegenerateBufferError::Allocation)?;
    if vertices.capacity() != count {
        return Err(DegenerateBufferError::MemoryLimit.into());
    }
    for k in 0..n {
        checkpoint()?;
        // Azimut da nord in senso orario: il segno meno rende l'esterno antiorario.
        let azimuth = -360.0 * f64::from(k) / f64::from(n);
        let (lat, lon): (f64, f64) = ellipsoid.geodetica().direct(p.y, p.x, azimuth, radius);
        if !(lat.is_finite() && lon.is_finite() && lat.abs() < 81.0 && lon.abs() < 180.0) {
            return Err(DegenerateBufferError::AccuracyUnproven.into());
        }
        vertices.push(Coord { x: lon, y: lat });
    }
    checkpoint()?;
    let first = vertices
        .first()
        .copied()
        .ok_or(DegenerateBufferError::AccuracyUnproven)?;
    vertices.push(first);
    verify_planar_fan(p, &vertices, &mut checkpoint)?;
    Ok(Polygon::new(LineString::new(vertices), Vec::new()))
}

/// Settori strettamente antiorari, con un solo giro: disco planare stellato.
fn verify_planar_fan<E: From<DegenerateBufferError>>(
    center: Coord,
    vertices: &[Coord],
    mut checkpoint: impl FnMut() -> Result<(), E>,
) -> Result<(), E> {
    let mut crossings = 0_u32;
    for edge in vertices.windows(2) {
        checkpoint()?;
        let [a, b] = [edge[0], edge[1]];
        if RobustKernel::orient2d(center, a, b) != Orientation::CounterClockwise {
            return Err(DegenerateBufferError::AccuracyUnproven.into());
        }
        // Ogni attraversamento crescente della semiretta est conta un giro.
        if a.y <= center.y && b.y > center.y {
            crossings = crossings
                .checked_add(1)
                .ok_or(DegenerateBufferError::AccuracyUnproven)?;
        }
    }
    if crossings != 1 {
        return Err(DegenerateBufferError::AccuracyUnproven.into());
    }
    Ok(())
}

/// Buffer positivo di `Point` o `MultiPoint` con un solo punto distinto.
///
/// Dominio: 6e6<=a<=7e6, 250<=1/f<=350, 1<=r<=60000 metri,
/// |latitudine|<=80 gradi e barriera conservativa dal seam descritta in docs.
/// Poli, seam, unione e altri tipi sono esclusi. `tolerance_m` e' un budget
/// assoluto positivo; si impone anche il centimetro del motore e un pavimento
/// di 0.1 mm. Il relativo pubblicato e' absolute/r, non un budget aggiuntivo.
/// Preconta output, coordinate e memoria prima di allocare; un checkpoint
/// per coordinata e prima/dopo ciascuna fase, al massimo una direct fra due.
///
/// # Errors
/// Errori tipizzati di dominio, tolleranza, risorse e interruzione; senza dati.
pub fn geodesic_point_buffer_with_limits<'a>(
    inputs: &[Geometry],
    crs: &'a ResolvedCrs,
    radii_m: &[f64],
    tolerance_m: f64,
    limits: PointBufferLimits,
    cancelled: Option<&AtomicBool>,
) -> Result<PointBufferResult<'a>, DegenerateBufferError> {
    let mut work = BufferWork {
        limits: limits.batch,
        cancel: cancelled,
        count: 0,
    };
    work.checkpoint()?;
    if crs.kind() != CrsKind::Geographic {
        return Err(DegenerateBufferError::GeographicRequired);
    }
    if !tolerance_m.is_finite() || tolerance_m <= 0.0 {
        return Err(DegenerateBufferError::InvalidTolerance);
    }
    let ellipsoid = EllissoideGeodetico::da_crs(crs)
        .map_err(|_| DegenerateBufferError::UnsupportedEllipsoid)?;
    let parameters = ellipsoid.parametri();
    if !(6e6..=7e6).contains(&parameters.semi_major_axis_metre)
        || !(250.0..=350.0).contains(&parameters.inverse_flattening)
    {
        return Err(DegenerateBufferError::UnsupportedEllipsoid);
    }
    let items = inputs
        .len()
        .checked_mul(radii_m.len())
        .ok_or(DegenerateBufferError::OutputLimit)?;
    if u64::try_from(items).map_err(|_| DegenerateBufferError::OutputLimit)?
        > limits.batch.max_output_items
    {
        return Err(DegenerateBufferError::OutputLimit);
    }
    let mut vertices = 0_u64;
    for &radius in radii_m {
        work.step()?;
        let (n, _) = plan(radius, tolerance_m, crs)?;
        for input in inputs {
            domain(center(input, &mut work)?, radius)?;
            vertices = vertices
                .checked_add(u64::from(n) + 1)
                .ok_or(DegenerateBufferError::VertexLimit)?;
            if vertices > limits.max_vertices {
                return Err(DegenerateBufferError::VertexLimit);
            }
        }
    }
    let metadata = items
        .checked_mul(size_of::<PointBufferDisk<'_>>())
        .ok_or(DegenerateBufferError::MemoryLimit)?;
    let bytes = vertices
        .checked_mul(
            u64::try_from(size_of::<Coord>()).map_err(|_| DegenerateBufferError::MemoryLimit)?,
        )
        .and_then(|v| v.checked_add(u64::try_from(metadata).ok()?))
        .ok_or(DegenerateBufferError::MemoryLimit)?;
    if bytes > limits.batch.max_reserved_bytes {
        return Err(DegenerateBufferError::MemoryLimit);
    }
    work.checkpoint()?;
    let mut disks = Vec::new();
    disks
        .try_reserve_exact(items)
        .map_err(|_| DegenerateBufferError::Allocation)?;
    for &radius in radii_m {
        work.step()?;
        let (n, certificate) = plan(radius, tolerance_m, crs)?;
        for input in inputs {
            let p = center(input, &mut work)?;
            let polygon = ring(p, radius, n, &ellipsoid, || work.step())?;
            work.step()?;
            disks.push(PointBufferDisk {
                polygon,
                certificate,
            });
        }
    }
    work.checkpoint()?;
    Ok(PointBufferResult {
        disks,
        vertices,
        peak_reserved_bytes: bytes,
        work: work.count,
    })
}

#[cfg(test)]
#[deny(clippy::disallowed_methods)]
mod tests;

/// Unioni certificate di dischi per punti distinti.
pub mod union;
