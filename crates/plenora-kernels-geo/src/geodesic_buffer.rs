//! Profilo esatto dei buffer degeneri GOP-008: vuoti e punti/linee a raggio zero.
//!
//! Il profilo vuoto non costruisce coordinate; [`point`] espone il disco locale.
//! I vertici del confine di un buffer geodetico ricadono nel certificato GOP-006;
//! GOP-015A riguarda invece i nodi delle booleane e i punti dell'editing.

#![allow(clippy::float_cmp, reason = "Identita e simmetrie geodetiche esatte")]

use std::sync::atomic::AtomicBool;
use std::time::Instant;

use geo::{Coord, Geometry, LineString, Polygon};
use geographiclib_rs::InverseGeodesic;
use plenora_core::crs::{CrsKind, ResolvedCrs};
use thiserror::Error;

use crate::geodetica::EllissoideGeodetico;

pub mod point;
pub mod projected;

/// Budget cumulativi finiti; zero consente zero lavoro o zero output.
#[derive(Clone, Copy, Debug)]
pub struct DegenerateBufferLimits {
    /// Letture di raggi, geometrie, parti, coordinate e primitive inverse.
    /// Include ogni elemento di output assemblato.
    pub max_work: u64,
    /// Numero di poligoni emessi, anche vuoti.
    pub max_output_items: u64,
    /// Memoria dinamica consentita in byte: questo profilo ne usa zero.
    /// Input e CRS sono presi in prestito; output compatto e stack costante
    /// sono esclusi. Non vengono emesse coordinate o fatte allocazioni.
    pub max_reserved_bytes: u64,
    /// Scadenza monotona; prevale sull'annullamento allo stesso checkpoint.
    pub deadline: Option<Instant>,
}

/// Certificato GOP-006 dell'output poligonale vuoto esatto.
#[derive(Clone, Copy, Debug)]
pub struct EmptyBufferCertificate<'a> {
    /// CRS risolto, inclusi datum ed ellissoide; XY lon/lat in gradi.
    pub crs: &'a ResolvedCrs,
}

impl EmptyBufferCertificate<'_> {
    /// Modello degli ingressi e della distanza.
    pub const MODEL: &'static str = "shortest_geodesic";
    /// Unita' dei raggi e del limite di Hausdorff.
    pub const UNIT: &'static str = "metre";
    /// Limite assoluto: output ed esatto sono entrambi il vuoto GOP-008.
    pub const ABSOLUTE_M: f64 = 0.0;
    /// Limite relativo adimensionale.
    pub const RELATIVE: f64 = 0.0;
    /// Nessuna trasformazione.
    pub const TRANSFORMATION_M: f64 = 0.0;
    /// Nessun campionamento del bordo.
    pub const DISCRETIZATION_M: f64 = 0.0;
    /// Nessuna coordinata costruita o arrotondata.
    pub const NUMERICAL_M: f64 = 0.0;
}

/// Batch atomico, ordinato per distanza e poi per ingresso.
#[derive(Debug)]
pub struct DegenerateBufferResult<'a> {
    /// Solo Polygon vuoti tipizzati. Con unione: uno per distanza,
    /// anche se la lista degli ingressi e' vuota.
    pub polygons: EmptyPolygonBatch,
    /// Garanzia esatta, riferita al datum del chiamante.
    pub certificate: EmptyBufferCertificate<'a>,
    /// Lavoro effettivamente addebitato.
    pub work: u64,
    /// Capacita' viva massima dell'output.
    pub peak_reserved_bytes: u64,
}

/// Sequenza compatta di Polygon vuoti, senza allocazione ne' deduplicazione.
///
/// La cardinalita' conserva anche righe uguali. La materializzazione da parte
/// del chiamante non appartiene alla memoria viva della chiamata.
#[derive(Clone, Copy, Debug)]
pub struct EmptyPolygonBatch {
    count: usize,
}

impl EmptyPolygonBatch {
    /// Numero di risultati, incluse tutte le ripetizioni.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.count
    }

    /// Nessuna distanza, o nessun ingresso senza unione.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// Visita in ordine i poligoni gia' certificati. Ogni passo e' costante,
    /// senza allocazioni o calcoli geodetici; nessuna validazione differita.
    #[must_use]
    pub fn iter(&self) -> impl ExactSizeIterator<Item = Polygon> + '_ {
        (0..self.count).map(|_| Polygon::new(LineString::new(Vec::new()), Vec::new()))
    }
}

/// Errori statici, senza dati degli ingressi.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum DegenerateBufferError {
    /// Categoria crs.
    #[error("buffer geodetico richiede un CRS geografico")]
    GeographicRequired,
    /// Categoria unsupported.
    #[error("ellissoide geodetico non supportato")]
    UnsupportedEllipsoid,
    /// Categoria `invalid_configuration`: raggio non finito o negativo
    /// applicato a un punto o una linea, anche vuoti.
    #[error("raggio del buffer geodetico non valido")]
    InvalidRadius,
    /// Categoria `data_mapping`.
    #[error("geometria geografica non valida")]
    InvalidGeometry,
    /// Categoria unsupported: tipo o buffer fuori dal profilo degenere.
    #[error("buffer fuori dal profilo degenere")]
    UnsupportedProfile,
    /// Categoria unsupported, G2 Multiple shortest geodesics.
    #[error("geodetica minima ambigua")]
    AmbiguousGeodesic,
    /// Categoria execution, codice `GEOMETRY_ACCURACY_UNPROVEN`.
    #[error("GEOMETRY_ACCURACY_UNPROVEN")]
    AccuracyUnproven,
    /// Dominio locale del disco, poli o antimeridiano esclusi.
    #[error("superficie del buffer geodetico non supportata")]
    UnsupportedSurface,
    /// Tolleranza non finita o non positiva.
    #[error("tolleranza del buffer geodetico non valida")]
    InvalidTolerance,
    /// Coordinate emesse, compresa la chiusura.
    #[error("limite di vertici del buffer superato")]
    VertexLimit,
    /// Memoria richiesta prima della crescita.
    #[error("limite di memoria del buffer superato")]
    MemoryLimit,
    /// Prenotazione fallita.
    #[error("allocazione del buffer fallita")]
    Allocation,
    /// Categoria `resource_limit`.
    #[error("limite di lavoro del buffer superato")]
    WorkLimit,
    /// Categoria `resource_limit`.
    #[error("limite di output del buffer superato")]
    OutputLimit,
    /// Contorno pubblicato o relazione insiemistica non validi.
    #[error("topologia del buffer non valida")]
    InvalidTopology,
    /// Categoria cancelled.
    #[error("buffer geodetico annullato")]
    Cancelled,
    /// Categoria timeout.
    #[error("scadenza del buffer geodetico raggiunta")]
    DeadlineExceeded,
}

struct BufferWork<'a> {
    limits: DegenerateBufferLimits,
    cancel: Option<&'a AtomicBool>,
    count: u64,
}

impl<'a> BufferWork<'a> {
    const fn new(limits: DegenerateBufferLimits, cancel: Option<&'a AtomicBool>) -> Self {
        Self {
            limits,
            cancel,
            count: 0,
        }
    }
    #[cfg_attr(test, track_caller)]
    fn checkpoint(&self) -> Result<(), DegenerateBufferError> {
        crate::noding::check_interruption(
            self.limits.deadline,
            self.cancel,
            DegenerateBufferError::DeadlineExceeded,
            DegenerateBufferError::Cancelled,
        )
    }

    #[cfg_attr(test, track_caller)]
    fn step(&mut self) -> Result<(), DegenerateBufferError> {
        self.checkpoint()?;
        self.count = self
            .count
            .checked_add(1)
            .ok_or(DegenerateBufferError::WorkLimit)?;
        if self.count > self.limits.max_work {
            return Err(DegenerateBufferError::WorkLimit);
        }
        Ok(())
    }

    fn coordinate(&mut self, point: Coord) -> Result<(), DegenerateBufferError> {
        self.step()?;
        #[cfg(test)]
        crate::noding::gop028::unit();
        if !((-180.0..=180.0).contains(&point.x) && (-90.0..=90.0).contains(&point.y)) {
            return Err(DegenerateBufferError::InvalidGeometry);
        }
        Ok(())
    }

    fn line(
        &mut self,
        line: &LineString,
        ellipsoid: &EllissoideGeodetico,
    ) -> Result<(), DegenerateBufferError> {
        self.step()?;
        if line.0.len() == 1 {
            return Err(DegenerateBufferError::InvalidGeometry);
        }
        for &point in &line.0 {
            #[cfg(test)]
            crate::noding::gop028::cycle("geodesic_buffer.rs::line:1");
            self.checkpoint()?;
            self.coordinate(point)?;
        }
        for edge in line.0.windows(2) {
            #[cfg(test)]
            crate::noding::gop028::cycle("geodesic_buffer.rs::line:2");
            self.step()?;
            let a = edge[0];
            let b = edge[1];
            let (length, azimuth, final_azimuth, _): (f64, f64, f64, f64) =
                ellipsoid.geodetica().inverse(a.y, a.x, b.y, b.x);
            if !(length.is_finite() && azimuth.is_finite() && final_azimuth.is_finite()) {
                return Err(DegenerateBufferError::AccuracyUnproven);
            }
            // G2: latitudini opposte e azimut distinti sull'ellissoide oblato.
            if length > 0.0 && a.y == -b.y && (a.y.abs() == 90.0 || azimuth != final_azimuth) {
                return Err(DegenerateBufferError::AmbiguousGeodesic);
            }
            let delta = (a.x - b.x).abs();
            if delta.min(360.0 - delta) > 178.0
                && a.y != -b.y
                && (a.y + b.y).abs() < 1e-12
                && azimuth != final_azimuth
            {
                return Err(DegenerateBufferError::AccuracyUnproven);
            }
        }
        Ok(())
    }

    fn empty_surface(&mut self, polygon: &Polygon) -> Result<(), DegenerateBufferError> {
        self.step()?;
        if !polygon.exterior().0.is_empty() {
            return Err(DegenerateBufferError::UnsupportedProfile);
        }
        if !polygon.interiors().is_empty() {
            return Err(DegenerateBufferError::InvalidGeometry);
        }
        Ok(())
    }

    fn geometry(
        &mut self,
        geometry: &Geometry,
        radius: f64,
        ellipsoid: &EllissoideGeodetico,
    ) -> Result<(), DegenerateBufferError> {
        self.step()?;
        let empty = match geometry {
            Geometry::Polygon(polygon) => return self.empty_surface(polygon),
            Geometry::MultiPolygon(polygons) => {
                for polygon in &polygons.0 {
                    #[cfg(test)]
                    crate::noding::gop028::cycle("geodesic_buffer.rs::geometry:1");
                    self.checkpoint()?;
                    self.empty_surface(polygon)?;
                }
                return Ok(());
            }
            Geometry::Point(point) => {
                self.coordinate(point.0)?;
                false
            }
            Geometry::MultiPoint(points) => {
                for point in &points.0 {
                    #[cfg(test)]
                    crate::noding::gop028::cycle("geodesic_buffer.rs::geometry:2");
                    self.checkpoint()?;
                    self.coordinate(point.0)?;
                }
                points.0.is_empty()
            }
            Geometry::LineString(line) => {
                self.line(line, ellipsoid)?;
                line.0.is_empty()
            }
            Geometry::MultiLineString(lines) => {
                let mut empty = true;
                for line in &lines.0 {
                    #[cfg(test)]
                    crate::noding::gop028::cycle("geodesic_buffer.rs::geometry:3");
                    self.checkpoint()?;
                    self.line(line, ellipsoid)?;
                    empty &= line.0.is_empty();
                }
                empty
            }
            _ => return Err(DegenerateBufferError::UnsupportedProfile),
        };
        if radius < 0.0 {
            return Err(DegenerateBufferError::InvalidRadius);
        }
        if !empty && radius != 0.0 {
            return Err(DegenerateBufferError::UnsupportedProfile);
        }
        Ok(())
    }
}

/// Buffer esatto del solo profilo degenere `geodesic_empty_buffer_v1`.
///
/// Accetta Point/MultiPoint/LineString/MultiLineString a raggio zero e
/// ingressi vuoti con raggio finito (negativo solo per superfici).
/// Polygon/MultiPolygon non vuoti e buffer positivi non vuoti sono
/// `UnsupportedProfile`: non costituisce disponibilita' del buffer generale.
/// Le linee usano archi minimi unici; il CRS geografico risolve il datum,
/// con 6e6<=a<=7e6 metri e 250<=1/f<=350. Nessun default di servizio.
///
/// GOP-008 prescrive il vuoto poligonale anche per punti/linee non vuoti
/// a raggio zero. Il certificato confronta questo output normativo con
/// se stesso (Hausdorff esteso d(vuoto,vuoto)=0), non con l'insieme lineare.
/// GOP-005 e' vacuo sull'output senza anelli. GOP-015A non governa
/// l'approssimazione del confine di un buffer geodetico.
/// Nessuna riparazione, trasformazione o serializzazione e' implicita.
///
/// Distanze in metri, ordine distance-major/input-major; `union_results`
/// emette un vuoto per distanza. Nessun prefisso in caso di errore.
/// Valida ogni raggio anche senza ingressi; senza raggi non visita ingressi.
/// Output compatto senza allocazioni; i vertici emessi sono zero. Al massimo una
/// primitiva inverse fra checkpoint; parsing e assemblaggio sono controllati
/// per elemento. Nessun limite di latenza wall-clock e nessun effetto remoto.
///
/// # Errors
/// [`DegenerateBufferError`] distingue dominio, precisione, risorse e interruzione.
pub fn geodesic_empty_buffer_with_limits<'a>(
    inputs: &[Geometry],
    crs: &'a ResolvedCrs,
    radii_m: &[f64],
    union_results: bool,
    limits: DegenerateBufferLimits,
    cancelled: Option<&AtomicBool>,
) -> Result<DegenerateBufferResult<'a>, DegenerateBufferError> {
    let mut work = BufferWork::new(limits, cancelled);
    work.checkpoint()?;
    if crs.kind() != CrsKind::Geographic {
        return Err(DegenerateBufferError::GeographicRequired);
    }
    let ellipsoid = EllissoideGeodetico::da_crs(crs)
        .map_err(|_| DegenerateBufferError::UnsupportedEllipsoid)?;
    let parameters = ellipsoid.parametri();
    if !(6e6..=7e6).contains(&parameters.semi_major_axis_metre)
        || !(250.0..=350.0).contains(&parameters.inverse_flattening)
    {
        return Err(DegenerateBufferError::UnsupportedEllipsoid);
    }
    let mut items = 0_u64;
    for &radius in radii_m {
        #[cfg(test)]
        crate::noding::gop028::cycle("geodesic_buffer.rs::geodesic_empty_buffer_with_limits:1");
        work.step()?;
        if !radius.is_finite() {
            return Err(DegenerateBufferError::InvalidRadius);
        }
        for input in inputs {
            #[cfg(test)]
            crate::noding::gop028::cycle("geodesic_buffer.rs::geodesic_empty_buffer_with_limits:2");
            work.checkpoint()?;
            work.geometry(input, radius, &ellipsoid)?;
        }
        let count = if union_results {
            1
        } else {
            u64::try_from(inputs.len()).map_err(|_| DegenerateBufferError::OutputLimit)?
        };
        items = items
            .checked_add(count)
            .ok_or(DegenerateBufferError::OutputLimit)?;
        if items > limits.max_output_items {
            return Err(DegenerateBufferError::OutputLimit);
        }
    }
    work.checkpoint()?;
    let count = usize::try_from(items).map_err(|_| DegenerateBufferError::OutputLimit)?;
    for _ in 0..count {
        #[cfg(test)]
        crate::noding::gop028::cycle("geodesic_buffer.rs::geodesic_empty_buffer_with_limits:3");
        work.step()?;
    }
    let polygons = EmptyPolygonBatch { count };
    work.checkpoint()?;
    Ok(DegenerateBufferResult {
        polygons,
        certificate: EmptyBufferCertificate { crs },
        work: work.count,
        peak_reserved_bytes: 0,
    })
}

// Verificatore unico del contorno pubblicato: validazione OGC esatta del
// motore comune, poi orientamento razionale. Nessuna riparazione implicita.
fn verify_surface(
    geometry: &Geometry,
    work: &mut crate::noding::Work<'_>,
) -> Result<(), point::union::UnionBufferError> {
    use crate::predicates::arrangement_indexed::relate;
    let polygons = match geometry {
        Geometry::Polygon(p) => std::slice::from_ref(p),
        Geometry::MultiPolygon(p) => &p.0,
        _ => return Err(DegenerateBufferError::InvalidTopology.into()),
    };
    let empty = Geometry::Polygon(Polygon::empty());
    match relate(geometry, &empty, work) {
        Err(crate::predicates::PredicateError::InvalidGeometry { .. }) => {
            return Err(DegenerateBufferError::InvalidTopology.into());
        }
        result => {
            result?;
        }
    }
    for polygon in polygons {
        #[cfg(test)]
        crate::noding::gop028::cycle("geodesic_buffer.rs::verify_surface:1");
        work.step()?;
        for (index, ring) in std::iter::once(polygon.exterior())
            .chain(polygon.interiors())
            .enumerate()
        {
            #[cfg(test)]
            crate::noding::gop028::cycle("geodesic_buffer.rs::verify_surface:2");
            work.step()?;
            if ring.0.is_empty() {
                continue;
            }
            let expected = if index == 0 {
                std::cmp::Ordering::Greater
            } else {
                std::cmp::Ordering::Less
            };
            if ring_orientation(ring, work)? != expected {
                return Err(DegenerateBufferError::InvalidTopology.into());
            }
        }
    }
    Ok(())
}

fn ring_orientation(
    ring: &LineString,
    work: &mut crate::noding::Work<'_>,
) -> Result<std::cmp::Ordering, point::union::UnionBufferError> {
    use crate::noding::P;
    let mut area = work.integer(0)?;
    for edge in ring.0.windows(2) {
        #[cfg(test)]
        crate::noding::gop028::cycle("geodesic_buffer.rs::ring_orientation:1");
        work.step()?;
        let a = P::of_in(edge[0], work)?;
        let b = P::of_in(edge[1], work)?;
        area = area.try_add(&a.try_cross(&b)?)?;
    }
    Ok(area.sign())
}

fn verify_coverage(
    container: &Geometry,
    operand: &Geometry,
    work: &mut crate::noding::Work<'_>,
) -> Result<(), point::union::UnionBufferError> {
    let matrix = crate::predicates::arrangement_indexed::relate(container, operand, work)?;
    // Celle esterno/interno ed esterno/confine: vale anche per il vuoto.
    if matrix.cells()[6] != b'F' || matrix.cells()[7] != b'F' {
        return Err(DegenerateBufferError::InvalidTopology.into());
    }
    Ok(())
}

#[cfg(test)]
#[deny(clippy::disallowed_methods)]
mod tests {
    use super::*;
    use geo::{MultiLineString, MultiPoint, MultiPolygon, Point};
    use plenora_core::crs::resolve_crs;

    fn limits() -> DegenerateBufferLimits {
        DegenerateBufferLimits {
            max_work: 10_000,
            max_output_items: 100,
            max_reserved_bytes: 0,
            deadline: None,
        }
    }

    fn empty() -> Polygon {
        Polygon::new(LineString::new(Vec::new()), Vec::new())
    }

    #[test]
    #[deny(clippy::disallowed_methods)]
    fn zero_points_lines_poles_and_seam() {
        let crs = resolve_crs("EPSG:4326", "crs").unwrap();
        let inputs = [
            Point::new(0.0, 90.0).into(),
            Point::new(180.0, -90.0).into(),
            MultiPoint::new(vec![Point::new(-180.0, 0.0), Point::new(180.0, 0.0)]).into(),
            LineString::from(vec![(179.0, 0.0), (-179.0, 0.0)]).into(),
            MultiLineString::new(vec![LineString::from(vec![(0.0, 80.0), (180.0, 80.0)])]).into(),
        ];
        let result =
            geodesic_empty_buffer_with_limits(&inputs, &crs, &[0.0, -0.0], false, limits(), None)
                .unwrap();
        // GOP-008: zero per punti/linee = Polygon vuoto; due distanze per cinque ingressi.
        assert_eq!(result.polygons.len(), 10);
        assert!(result.polygons.iter().all(|polygon| polygon == empty()));
        // GOP-006: identita' esatta, nessuna trasformazione o discretizzazione.
        assert!(result.certificate.crs.semantically_equals(&crs));
        assert_eq!(EmptyBufferCertificate::ABSOLUTE_M, 0.0);
        assert_eq!(EmptyBufferCertificate::RELATIVE, 0.0);
        assert_eq!(
            EmptyBufferCertificate::TRANSFORMATION_M
                + EmptyBufferCertificate::DISCRETIZATION_M
                + EmptyBufferCertificate::NUMERICAL_M,
            0.0
        );
        assert_eq!(result.peak_reserved_bytes, 0);
    }

    #[test]
    #[deny(clippy::disallowed_methods)]
    fn empty_inputs_multiple_radii_and_union() {
        let crs = resolve_crs("EPSG:4326", "crs").unwrap();
        let inputs = [
            empty().into(),
            MultiPolygon::new(vec![empty(), empty()]).into(),
        ];
        // GOP-008: un vuoto per ingresso/distanza, oppure uno per distanza con unione.
        for (union, count) in [(false, 6), (true, 3)] {
            let result = geodesic_empty_buffer_with_limits(
                &inputs,
                &crs,
                &[-1.0, 0.0, 1e30],
                union,
                limits(),
                None,
            )
            .unwrap();
            assert_eq!(result.polygons.len(), count);
            assert!(result.polygons.iter().all(|polygon| polygon == empty()));
        }
        // GOP-008: unione dell'insieme vuoto = vuoto per ogni distanza.
        assert_eq!(
            geodesic_empty_buffer_with_limits(&[], &crs, &[1.0, 2.0], true, limits(), None)
                .unwrap()
                .polygons
                .len(),
            2
        );
        assert!(
            geodesic_empty_buffer_with_limits(&[], &crs, &[1.0], false, limits(), None)
                .unwrap()
                .polygons
                .is_empty()
        );
        let no_surface = [
            MultiPoint::new(Vec::new()).into(),
            LineString::new(Vec::new()).into(),
            MultiLineString::new(Vec::new()).into(),
        ];
        assert_eq!(
            geodesic_empty_buffer_with_limits(&no_surface, &crs, &[2.0], false, limits(), None)
                .unwrap()
                .polygons
                .len(),
            3
        );
    }

    #[test]
    #[deny(clippy::disallowed_methods)]
    fn rejects_non_degenerate_and_negative_point_line() {
        let crs = resolve_crs("EPSG:4326", "crs").unwrap();
        // Profilo dichiarato: nessun raggio positivo su punto/linea non vuoti.
        for input in [
            Point::new(1.0, 2.0).into(),
            LineString::from(vec![(0.0, 0.0), (1.0, 0.0)]).into(),
        ] {
            assert!(matches!(
                geodesic_empty_buffer_with_limits(
                    std::slice::from_ref(&input),
                    &crs,
                    &[0.0, 1.0],
                    false,
                    limits(),
                    None
                ),
                Err(DegenerateBufferError::UnsupportedProfile)
            ));
            // GOP-008: r negativo solo per superfici.
            assert!(matches!(
                geodesic_empty_buffer_with_limits(&[input], &crs, &[-1.0], false, limits(), None),
                Err(DegenerateBufferError::InvalidRadius)
            ));
        }
        let polygon: Geometry = Polygon::new(
            LineString::from(vec![(0.0, 0.0), (1.0, 0.0), (0.0, 1.0), (0.0, 0.0)]),
            Vec::new(),
        )
        .into();
        // Profilo dichiarato: erosione, collasso e raggio zero di superfici non vuote esclusi.
        for radius in [-1e9, -1.0, 0.0, 1.0] {
            assert!(matches!(
                geodesic_empty_buffer_with_limits(
                    std::slice::from_ref(&polygon),
                    &crs,
                    &[radius],
                    false,
                    limits(),
                    None
                ),
                Err(DegenerateBufferError::UnsupportedProfile)
            ));
        }
        assert!(matches!(
            geodesic_empty_buffer_with_limits(
                &[MultiPoint::new(Vec::new()).into()],
                &crs,
                &[-1.0],
                false,
                limits(),
                None
            ),
            Err(DegenerateBufferError::InvalidRadius)
        ));
    }

    #[test]
    #[deny(clippy::disallowed_methods)]
    fn invalid_geometry_and_ambiguous_path() {
        let crs = resolve_crs("EPSG:4326", "crs").unwrap();
        // GOP-002/004: coordinate finite nel dominio e struttura lineare valida.
        for input in [
            Point::new(f64::NAN, 0.0).into(),
            Point::new(0.0, 91.0).into(),
            LineString::from(vec![(0.0, 0.0)]).into(),
            Polygon::new(
                LineString::new(Vec::new()),
                vec![LineString::new(Vec::new())],
            )
            .into(),
        ] {
            assert!(matches!(
                geodesic_empty_buffer_with_limits(&[input], &crs, &[0.0], false, limits(), None),
                Err(DegenerateBufferError::InvalidGeometry)
            ));
        }
        // G2 Multiple shortest geodesics: gli antipodi non definiscono un lato unico.
        assert!(matches!(
            geodesic_empty_buffer_with_limits(
                &[LineString::from(vec![(0.0, 0.0), (180.0, 0.0)]).into()],
                &crs,
                &[0.0],
                false,
                limits(),
                None
            ),
            Err(DegenerateBufferError::AmbiguousGeodesic)
        ));
        // GOP-002 e profilo: nessuna raccolta generica reinterpretata come poligono.
        assert!(matches!(
            geodesic_empty_buffer_with_limits(
                &[Geometry::GeometryCollection(
                    geo::GeometryCollection::empty()
                )],
                &crs,
                &[0.0],
                false,
                limits(),
                None
            ),
            Err(DegenerateBufferError::UnsupportedProfile)
        ));
    }

    #[test]
    #[deny(clippy::disallowed_methods)]
    fn exact_limits_and_zero_work() {
        let crs = resolve_crs("EPSG:4326", "crs").unwrap();
        let inputs = [Point::new(1.0, 2.0).into()];
        let measured =
            geodesic_empty_buffer_with_limits(&inputs, &crs, &[0.0], false, limits(), None)
                .unwrap();
        let mut budget = limits();
        budget.max_work = measured.work;
        budget.max_output_items = 1;
        // GOP-026/027: limite esatto ammesso, nessuna memoria dinamica richiesta.
        assert_eq!(
            geodesic_empty_buffer_with_limits(&inputs, &crs, &[0.0], false, budget, None)
                .unwrap()
                .polygons
                .len(),
            1
        );
        budget.max_work -= 1;
        assert!(matches!(
            geodesic_empty_buffer_with_limits(&inputs, &crs, &[0.0], false, budget, None),
            Err(DegenerateBufferError::WorkLimit)
        ));
        budget = limits();
        budget.max_output_items = 0;
        assert!(matches!(
            geodesic_empty_buffer_with_limits(&inputs, &crs, &[0.0], false, budget, None),
            Err(DegenerateBufferError::OutputLimit)
        ));
        budget.max_work = 0;
        assert_eq!(
            geodesic_empty_buffer_with_limits(&inputs, &crs, &[], false, budget, None)
                .unwrap()
                .work,
            0
        );
        assert!(matches!(
            geodesic_empty_buffer_with_limits(&[], &crs, &[0.0], false, budget, None),
            Err(DegenerateBufferError::WorkLimit)
        ));
    }

    #[test]
    #[deny(clippy::disallowed_methods)]
    fn deadline_precedes_cancel_and_invalid_radii() {
        let crs = resolve_crs("EPSG:4326", "crs").unwrap();
        let cancel = AtomicBool::new(true);
        // GOP-028: interruzione anche sul batch vuoto, deadline precede cancel.
        assert!(matches!(
            geodesic_empty_buffer_with_limits(&[], &crs, &[], false, limits(), Some(&cancel)),
            Err(DegenerateBufferError::Cancelled)
        ));
        let mut budget = limits();
        budget.deadline = Some(Instant::now());
        assert!(matches!(
            geodesic_empty_buffer_with_limits(&[], &crs, &[], false, budget, Some(&cancel)),
            Err(DegenerateBufferError::DeadlineExceeded)
        ));
        // GOP-029: parametro non finito = invalid_configuration, anche senza ingressi.
        for radius in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert!(matches!(
                geodesic_empty_buffer_with_limits(&[], &crs, &[radius], false, limits(), None),
                Err(DegenerateBufferError::InvalidRadius)
            ));
        }
        // GOP-004: nessuna conversione automatica del CRS proiettato.
        let projected = resolve_crs("EPSG:3857", "crs").unwrap();
        assert!(matches!(
            geodesic_empty_buffer_with_limits(&[], &projected, &[], false, limits(), None),
            Err(DegenerateBufferError::GeographicRequired)
        ));
    }
}
