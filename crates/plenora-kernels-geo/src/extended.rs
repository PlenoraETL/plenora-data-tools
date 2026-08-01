//! Operations beyond the original Manipola catalog.

use geo::algorithm::concave_hull::ConcaveHullOptions;
use geo::algorithm::line_measures::{Distance, Geodesic, Haversine, Length};
use geo::algorithm::validation::Validation;
use geo::{
    AffineOps, AffineTransform, ConcaveHull, CoordsIter, Geometry, HausdorffDistance, Point,
};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ExtendedError {
    #[error("parametro {name} non valido: {reason}")]
    InvalidParameter {
        name: &'static str,
        reason: &'static str,
    },
    #[error("geometria di input non valida: {0}")]
    InvalidInput(String),
    #[error("geometria prodotta non valida: {0}")]
    InvalidOutput(String),
    #[error("coordinate oltre il limite di {limit}: {actual}")]
    CoordinateLimit { actual: u64, limit: u64 },
    #[error("confronti Hausdorff oltre il limite di {limit}: {actual}")]
    WorkLimit { actual: u64, limit: u64 },
    #[error("coordinate geografiche fuori intervallo lon/lat")]
    InvalidGeographicCoordinate,
    #[error("indice non rappresentabile come uint64")]
    IndexOverflow,
}

fn validate_input(geometry: &Geometry<f64>) -> Result<(), ExtendedError> {
    if geometry
        .coords_iter()
        .any(|coordinate| !coordinate.x.is_finite() || !coordinate.y.is_finite())
    {
        return Err(ExtendedError::InvalidInput(
            "coordinate NaN o infinite".to_owned(),
        ));
    }
    geometry
        .check_validation()
        .map_err(|error| ExtendedError::InvalidInput(error.to_string()))
}

fn validate_output(geometry: Geometry<f64>) -> Result<Geometry<f64>, ExtendedError> {
    geometry
        .check_validation()
        .map_err(|error| ExtendedError::InvalidOutput(error.to_string()))?;
    Ok(geometry)
}

/// Applica la matrice affine 2D a sei coefficienti
/// `[a, b, xoff, d, e, yoff]`.
///
/// # Errors
///
/// - `ExtendedError::InvalidInput`: coordinate NaN o infinite, o geometria
///   di input non valida OGC;
/// - `ExtendedError::InvalidParameter`: un coefficiente non e' finito;
/// - `ExtendedError::InvalidOutput`: la geometria trasformata non supera
///   la validazione OGC.
pub fn affine_transform(
    geometry: &Geometry<f64>,
    coefficients: [f64; 6],
) -> Result<Geometry<f64>, ExtendedError> {
    validate_input(geometry)?;
    affine_transform_validated(geometry, coefficients)
}

/// Variante di [`affine_transform`] SENZA il gate OGC di ingresso.
///
/// La scansione di finitezza e' coperta dalla stessa precondizione. La
/// validazione OGC DELL'OUTPUT resta: e' la garanzia del produttore per
/// i consumatori a valle (regola delle catene, R0.1).
///
/// # Precondizione (contratto del chiamante)
///
/// La geometria di input deve essere GIA' validata: coordinate finite e
/// validita' OGC, come garantito da [`crate::geometry_from_wkb`] al decode
/// o da un kernel che valida il proprio output. Su input che viola la
/// precondizione il risultato e' indefinito (tipicamente intercettato dal
/// gate di output, ma non garantito): la variante e' per i soli percorsi
/// in cui la validazione e' dimostrata per costruzione (R0.1: mai
/// un'inferenza sui chiamanti — il gate di ingresso resta nella forma
/// pubblica [`affine_transform`]).
///
/// # Errors
///
/// Come [`affine_transform`], eccetto `InvalidInput` (gate omesso).
pub fn affine_transform_validated(
    geometry: &Geometry<f64>,
    coefficients: [f64; 6],
) -> Result<Geometry<f64>, ExtendedError> {
    if coefficients.iter().any(|value| !value.is_finite()) {
        return Err(ExtendedError::InvalidParameter {
            name: "coefficients",
            reason: "devono essere finiti",
        });
    }
    let [a, b, xoff, d, e, yoff] = coefficients;
    let transform = AffineTransform::new(a, b, xoff, d, e, yoff);
    validate_output(geometry.affine_transform(&transform))
}

/// Traslazione di `(x_offset, y_offset)` (wrapper di [`affine_transform`]).
///
/// # Errors
///
/// Come [`affine_transform`] (gli offset entrano nei coefficienti).
pub fn translate(
    geometry: &Geometry<f64>,
    x_offset: f64,
    y_offset: f64,
) -> Result<Geometry<f64>, ExtendedError> {
    affine_transform(geometry, [1.0, 0.0, x_offset, 0.0, 1.0, y_offset])
}

/// Variante di [`translate`] SENZA il gate di ingresso: stessa
/// precondizione e stesso contratto di [`affine_transform_validated`].
///
/// # Errors
///
/// Come [`translate`], eccetto `InvalidInput` (gate omesso).
pub fn translate_validated(
    geometry: &Geometry<f64>,
    x_offset: f64,
    y_offset: f64,
) -> Result<Geometry<f64>, ExtendedError> {
    affine_transform_validated(geometry, [1.0, 0.0, x_offset, 0.0, 1.0, y_offset])
}

/// Scala di `(x_factor, y_factor)` attorno a `origin` (wrapper di
/// [`affine_transform`]).
///
/// # Errors
///
/// Come [`affine_transform`] (fattori e origine entrano nei coefficienti).
pub fn scale_about(
    geometry: &Geometry<f64>,
    x_factor: f64,
    y_factor: f64,
    origin: Point<f64>,
) -> Result<Geometry<f64>, ExtendedError> {
    let x_offset = origin.x() * (1.0 - x_factor);
    let y_offset = origin.y() * (1.0 - y_factor);
    affine_transform(geometry, [x_factor, 0.0, x_offset, 0.0, y_factor, y_offset])
}

/// Variante di [`scale_about`] SENZA il gate di ingresso: stessa
/// precondizione e stesso contratto di [`affine_transform_validated`].
///
/// # Errors
///
/// Come [`scale_about`], eccetto `InvalidInput` (gate omesso).
pub fn scale_about_validated(
    geometry: &Geometry<f64>,
    x_factor: f64,
    y_factor: f64,
    origin: Point<f64>,
) -> Result<Geometry<f64>, ExtendedError> {
    let x_offset = origin.x() * (1.0 - x_factor);
    let y_offset = origin.y() * (1.0 - y_factor);
    affine_transform_validated(geometry, [x_factor, 0.0, x_offset, 0.0, y_factor, y_offset])
}

/// Rotazione di `degrees` gradi attorno a `origin` (wrapper di
/// [`affine_transform`]).
///
/// # Errors
///
/// `ExtendedError::InvalidParameter` se `degrees` non e' finito; in piu'
/// come [`affine_transform`] per input non valido, coefficienti risultanti
/// non finiti o output non valido.
pub fn rotate_about(
    geometry: &Geometry<f64>,
    degrees: f64,
    origin: Point<f64>,
) -> Result<Geometry<f64>, ExtendedError> {
    let (x_offset, y_offset, cosine, sine) = rotate_coefficients(degrees, origin)?;
    affine_transform(geometry, [cosine, -sine, x_offset, sine, cosine, y_offset])
}

/// Variante di [`rotate_about`] SENZA il gate di ingresso: stessa
/// precondizione e stesso contratto di [`affine_transform_validated`].
///
/// # Errors
///
/// Come [`rotate_about`], eccetto `InvalidInput` (gate omesso).
pub fn rotate_about_validated(
    geometry: &Geometry<f64>,
    degrees: f64,
    origin: Point<f64>,
) -> Result<Geometry<f64>, ExtendedError> {
    let (x_offset, y_offset, cosine, sine) = rotate_coefficients(degrees, origin)?;
    affine_transform_validated(geometry, [cosine, -sine, x_offset, sine, cosine, y_offset])
}

/// Coefficienti della rotazione di `degrees` attorno a `origin`, condivisi
/// fra [`rotate_about`] e [`rotate_about_validated`].
fn rotate_coefficients(
    degrees: f64,
    origin: Point<f64>,
) -> Result<(f64, f64, f64, f64), ExtendedError> {
    if !degrees.is_finite() {
        return Err(ExtendedError::InvalidParameter {
            name: "degrees",
            reason: "deve essere finito",
        });
    }
    let radians = degrees.to_radians();
    let cosine = radians.cos();
    let sine = radians.sin();
    // Niente mul_add/FMA: la fusione cambia l'arrotondamento IEEE e
    // violerebbe il determinismo bit-esatto (ADR-0001); la forma non
    // fusa e' il contratto numerico.
    #[allow(clippy::suboptimal_flops)]
    let x_offset = origin.x() - cosine * origin.x() + sine * origin.y();
    // Niente mul_add/FMA: la fusione cambia l'arrotondamento IEEE e
    // violerebbe il determinismo bit-esatto (ADR-0001); la forma non
    // fusa e' il contratto numerico.
    #[allow(clippy::suboptimal_flops)]
    let y_offset = origin.y() - sine * origin.x() - cosine * origin.y();
    Ok((x_offset, y_offset, cosine, sine))
}

/// Concave hull delle coordinate dell'input, con parametri e limiti di
/// lavoro dichiarati.
///
/// # Errors
///
/// - `ExtendedError::InvalidInput`: coordinate NaN o infinite, o geometria
///   di input non valida OGC;
/// - `ExtendedError::InvalidParameter`: `concavity` non finita o non
///   positiva, oppure `length_threshold` non finita o negativa;
/// - `ExtendedError::IndexOverflow`: numero di coordinate non
///   rappresentabile come `u64`;
/// - `ExtendedError::CoordinateLimit`: coordinate oltre `max_coordinates`;
/// - `ExtendedError::InvalidOutput`: l'hull prodotto non supera la
///   validazione OGC.
pub fn concave_hull(
    geometry: &Geometry<f64>,
    concavity: f64,
    length_threshold: f64,
    max_coordinates: u64,
) -> Result<Geometry<f64>, ExtendedError> {
    validate_input(geometry)?;
    concave_hull_validated(geometry, concavity, length_threshold, max_coordinates)
}

/// Variante di [`concave_hull`] SENZA il gate OGC di ingresso.
///
/// La scansione di finitezza e' coperta dalla stessa precondizione. La
/// validazione OGC DELL'OUTPUT resta: e' la garanzia del produttore per
/// i consumatori a valle.
///
/// # Precondizione (contratto del chiamante)
///
/// Come [`affine_transform_validated`]: input gia' validato (finitezza +
/// OGC) per costruzione, mai per inferenza sui chiamanti.
///
/// # Errors
///
/// Come [`concave_hull`], eccetto `InvalidInput` (gate omesso).
pub fn concave_hull_validated(
    geometry: &Geometry<f64>,
    concavity: f64,
    length_threshold: f64,
    max_coordinates: u64,
) -> Result<Geometry<f64>, ExtendedError> {
    if !concavity.is_finite() || concavity <= 0.0 {
        return Err(ExtendedError::InvalidParameter {
            name: "concavity",
            reason: "deve essere finita e maggiore di zero",
        });
    }
    if !length_threshold.is_finite() || length_threshold < 0.0 {
        return Err(ExtendedError::InvalidParameter {
            name: "length_threshold",
            reason: "deve essere finita e non negativa",
        });
    }
    let coordinates: Vec<_> = geometry.coords_iter().collect();
    let actual = u64::try_from(coordinates.len()).map_err(|_| ExtendedError::IndexOverflow)?;
    if actual > max_coordinates {
        return Err(ExtendedError::CoordinateLimit {
            actual,
            limit: max_coordinates,
        });
    }
    let hull = coordinates.concave_hull_with_options(ConcaveHullOptions {
        concavity,
        length_threshold,
    });
    validate_output(Geometry::Polygon(hull))
}

/// Distanza di Hausdorff per vertici, con limite di lavoro dichiarato
/// perche' la complessita' e' O(n*m).
///
/// # Errors
///
/// - `ExtendedError::InvalidInput`: coordinate NaN o infinite, o geometria
///   non valida OGC in uno dei due input;
/// - `ExtendedError::IndexOverflow`: conteggio dei vertici non
///   rappresentabile come `u64`;
/// - `ExtendedError::WorkLimit`: coppie di coordinate oltre
///   `max_coordinate_pairs` (anche per overflow del prodotto n*m).
pub fn hausdorff_distance(
    left: &Geometry<f64>,
    right: &Geometry<f64>,
    max_coordinate_pairs: u64,
) -> Result<Option<f64>, ExtendedError> {
    validate_input(left)?;
    validate_input(right)?;
    hausdorff_distance_validated(left, right, max_coordinate_pairs)
}

/// Variante di [`hausdorff_distance`] SENZA il gate di ingresso (scansione
/// di finitezza + validazione OGC su entrambi gli input).
///
/// # Precondizione (contratto del chiamante)
///
/// Come [`affine_transform_validated`]: entrambi gli input gia' validati
/// (finitezza + OGC) per costruzione, mai per inferenza sui chiamanti.
///
/// # Errors
///
/// Come [`hausdorff_distance`], eccetto `InvalidInput` (gate omesso).
pub fn hausdorff_distance_validated(
    left: &Geometry<f64>,
    right: &Geometry<f64>,
    max_coordinate_pairs: u64,
) -> Result<Option<f64>, ExtendedError> {
    let left_count =
        u64::try_from(left.coords_count()).map_err(|_| ExtendedError::IndexOverflow)?;
    let right_count =
        u64::try_from(right.coords_count()).map_err(|_| ExtendedError::IndexOverflow)?;
    if left_count == 0 || right_count == 0 {
        return Ok(None);
    }
    let actual = left_count
        .checked_mul(right_count)
        .ok_or(ExtendedError::WorkLimit {
            actual: u64::MAX,
            limit: max_coordinate_pairs,
        })?;
    if actual > max_coordinate_pairs {
        return Err(ExtendedError::WorkLimit {
            actual,
            limit: max_coordinate_pairs,
        });
    }
    Ok(Some(left.hausdorff_distance(right)))
}

fn validate_geographic_point(point: Point<f64>) -> Result<(), ExtendedError> {
    if !point.x().is_finite()
        || !point.y().is_finite()
        || !(-180.0..=180.0).contains(&point.x())
        || !(-90.0..=90.0).contains(&point.y())
    {
        return Err(ExtendedError::InvalidGeographicCoordinate);
    }
    Ok(())
}

/// Distanza haversine in metri tra due punti geografici (lon/lat).
///
/// # Errors
///
/// `ExtendedError::InvalidGeographicCoordinate` se una coordinata non e'
/// finita o e' fuori dagli intervalli lon [-180, 180] e lat [-90, 90].
pub fn haversine_distance_m(left: Point<f64>, right: Point<f64>) -> Result<f64, ExtendedError> {
    validate_geographic_point(left)?;
    validate_geographic_point(right)?;
    Ok(Haversine.distance(left, right))
}

/// Distanza geodetica in metri tra due punti geografici (lon/lat).
///
/// # Errors
///
/// `ExtendedError::InvalidGeographicCoordinate` se una coordinata non e'
/// finita o e' fuori dagli intervalli lon [-180, 180] e lat [-90, 90].
pub fn geodesic_distance_m(left: Point<f64>, right: Point<f64>) -> Result<f64, ExtendedError> {
    validate_geographic_point(left)?;
    validate_geographic_point(right)?;
    Ok(Geodesic.distance(left, right))
}

/// Lunghezza geodetica in metri di una linea geografica (lon/lat).
///
/// # Errors
///
/// `ExtendedError::InvalidGeographicCoordinate` se un vertice non e'
/// finito o e' fuori dagli intervalli lon [-180, 180] e lat [-90, 90].
pub fn geodesic_line_length_m(line: &geo::LineString<f64>) -> Result<f64, ExtendedError> {
    for coordinate in line.coords() {
        validate_geographic_point(Point::from(*coordinate))?;
    }
    Ok(Geodesic.length(line))
}

#[cfg(test)]
// Confronti float esatti intenzionali: le fixture sono costruite per
// produrre valori esatti (coordinate note, round-trip bit-esatti); il
// confronto per bit e' il contratto verificato, non un'approssimazione.
#[allow(clippy::float_cmp)]
mod tests {
    use super::*;
    use geo::{line_string, polygon, Area};

    #[test]
    fn affine_wrappers_preserve_expected_coordinates_and_area() {
        let geometry = Geometry::Polygon(polygon![
            (x: 0.0, y: 0.0), (x: 2.0, y: 0.0),
            (x: 2.0, y: 1.0), (x: 0.0, y: 1.0),
            (x: 0.0, y: 0.0),
        ]);
        let translated = translate(&geometry, 10.0, -5.0).unwrap();
        assert_eq!(translated.unsigned_area(), geometry.unsigned_area());
        let scaled = scale_about(&geometry, 2.0, 3.0, Point::new(0.0, 0.0)).unwrap();
        assert_eq!(scaled.unsigned_area(), 12.0);
        let rotated = rotate_about(&geometry, 90.0, Point::new(0.0, 0.0)).unwrap();
        assert!((rotated.unsigned_area() - 2.0).abs() < 1e-12);
    }

    #[test]
    fn concave_hull_and_hausdorff_are_bounded() {
        let geometry = Geometry::LineString(line_string![
            (x: 0.0, y: 0.0), (x: 2.0, y: 0.0),
            (x: 1.5, y: 1.0), (x: 2.0, y: 2.0),
            (x: 0.0, y: 2.0),
        ]);
        let hull = concave_hull(&geometry, 1.0, 0.0, 10).unwrap();
        assert!(hull.unsigned_area() > 0.0);
        assert!(matches!(
            concave_hull(&geometry, 1.0, 0.0, 2),
            Err(ExtendedError::CoordinateLimit { .. })
        ));
        assert_eq!(
            hausdorff_distance(&geometry, &geometry, 100).unwrap(),
            Some(0.0)
        );
        assert!(matches!(
            hausdorff_distance(&geometry, &geometry, 1),
            Err(ExtendedError::WorkLimit { .. })
        ));
    }

    #[test]
    fn validated_variants_match_the_gated_path_on_valid_inputs() {
        let geometry = Geometry::Polygon(polygon![
            (x: 0.0, y: 0.0), (x: 2.0, y: 0.0),
            (x: 2.0, y: 1.0), (x: 0.0, y: 1.0),
            (x: 0.0, y: 0.0),
        ]);
        let line = Geometry::LineString(line_string![
            (x: 0.0, y: 0.0), (x: 2.0, y: 0.0),
            (x: 1.5, y: 1.0), (x: 2.0, y: 2.0),
            (x: 0.0, y: 2.0),
        ]);
        let coefficients = [1.1, 0.05, 3.0, -0.02, 0.9, 7.0];
        assert_eq!(
            affine_transform(&geometry, coefficients).unwrap(),
            affine_transform_validated(&geometry, coefficients).unwrap()
        );
        assert_eq!(
            translate(&geometry, 10.0, -5.0).unwrap(),
            translate_validated(&geometry, 10.0, -5.0).unwrap()
        );
        assert_eq!(
            scale_about(&geometry, 2.0, 3.0, Point::new(0.0, 0.0)).unwrap(),
            scale_about_validated(&geometry, 2.0, 3.0, Point::new(0.0, 0.0)).unwrap()
        );
        assert_eq!(
            rotate_about(&geometry, 90.0, Point::new(0.0, 0.0)).unwrap(),
            rotate_about_validated(&geometry, 90.0, Point::new(0.0, 0.0)).unwrap()
        );
        assert_eq!(
            concave_hull(&line, 1.0, 0.0, 10).unwrap(),
            concave_hull_validated(&line, 1.0, 0.0, 10).unwrap()
        );
        assert_eq!(
            hausdorff_distance(&line, &geometry, 1_000).unwrap(),
            hausdorff_distance_validated(&line, &geometry, 1_000).unwrap()
        );
        // Parametri e limiti restano fail-closed nella variante validated.
        assert!(matches!(
            affine_transform_validated(&geometry, [f64::NAN; 6]),
            Err(ExtendedError::InvalidParameter {
                name: "coefficients",
                ..
            })
        ));
        assert!(matches!(
            rotate_about_validated(&geometry, f64::INFINITY, Point::new(0.0, 0.0)),
            Err(ExtendedError::InvalidParameter {
                name: "degrees",
                ..
            })
        ));
        assert!(matches!(
            concave_hull_validated(&line, 1.0, 0.0, 2),
            Err(ExtendedError::CoordinateLimit { .. })
        ));
        assert!(matches!(
            hausdorff_distance_validated(&line, &line, 1),
            Err(ExtendedError::WorkLimit { .. })
        ));
    }

    #[test]
    fn validated_variants_document_the_caller_precondition() {
        // Test di documentazione del contratto, NON un nuovo modo di
        // accettare geometrie invalide in produzione: il percorso gated
        // rifiuta il bowtie in INGRESSO (gate intatto), la variante
        // validated omette quel gate perche' la precondizione e' del
        // chiamante — qui violata ad arte. Il gate di OUTPUT resta: la
        // trasformata affine di un bowtie e' ancora un bowtie e viene
        // rifiutata in uscita.
        let bowtie = Geometry::Polygon(polygon![
            (x: 0.0, y: 0.0), (x: 2.0, y: 2.0),
            (x: 0.0, y: 2.0), (x: 2.0, y: 0.0),
            (x: 0.0, y: 0.0),
        ]);
        let line = Geometry::LineString(line_string![(x: 0.0, y: 0.0), (x: 3.0, y: 3.0)]);
        assert!(matches!(
            hausdorff_distance(&bowtie, &line, 100),
            Err(ExtendedError::InvalidInput(_))
        ));
        assert!(hausdorff_distance_validated(&bowtie, &line, 100).is_ok());
        assert!(matches!(
            affine_transform(&bowtie, [1.0, 0.0, 0.0, 0.0, 1.0, 0.0]),
            Err(ExtendedError::InvalidInput(_))
        ));
        assert!(matches!(
            affine_transform_validated(&bowtie, [1.0, 0.0, 0.0, 0.0, 1.0, 0.0]),
            Err(ExtendedError::InvalidOutput(_))
        ));
    }

    #[test]
    fn geographic_distances_validate_ranges_and_units() {
        let bologna = Point::new(11.3426, 44.4949);
        let modena = Point::new(10.9252, 44.6471);
        let geodesic = geodesic_distance_m(bologna, modena).unwrap();
        let haversine = haversine_distance_m(bologna, modena).unwrap();
        assert!(geodesic > 35_000.0 && geodesic < 40_000.0);
        assert!((geodesic - haversine).abs() < 200.0);
        assert!(geodesic_distance_m(Point::new(200.0, 0.0), modena).is_err());

        let equator = line_string![(x: 0.0, y: 0.0), (x: 1.0, y: 0.0)];
        let length = geodesic_line_length_m(&equator).unwrap();
        assert!((length - 111_319.490_793).abs() < 0.01);
    }

    #[test]
    fn every_invalid_parameter_and_empty_path_is_fail_closed() {
        let line = Geometry::LineString(line_string![(x: 0.0, y: 0.0), (x: 1.0, y: 1.0)]);
        assert!(affine_transform(&line, [f64::NAN; 6]).is_err());
        assert!(affine_transform(
            &Geometry::Point(Point::new(f64::NAN, 0.0)),
            [1.0, 0.0, 0.0, 0.0, 1.0, 0.0]
        )
        .is_err());
        assert!(rotate_about(&line, f64::INFINITY, Point::new(0.0, 0.0)).is_err());
        for (concavity, threshold) in [(0.0, 0.0), (f64::NAN, 0.0), (1.0, -1.0), (1.0, f64::NAN)] {
            assert!(concave_hull(&line, concavity, threshold, 100).is_err());
        }
        let empty = Geometry::LineString(geo::LineString::new(Vec::new()));
        assert_eq!(hausdorff_distance(&empty, &line, 100).unwrap(), None);
        assert!(hausdorff_distance(&line, &line, 1).is_err());
        for point in [
            Point::new(181.0, 0.0),
            Point::new(0.0, 91.0),
            Point::new(f64::NAN, 0.0),
        ] {
            assert!(haversine_distance_m(point, Point::new(0.0, 0.0)).is_err());
            assert!(geodesic_distance_m(point, Point::new(0.0, 0.0)).is_err());
        }
    }
}
