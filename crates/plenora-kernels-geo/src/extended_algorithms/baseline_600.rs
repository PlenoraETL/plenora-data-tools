// Snapshot letterale delle funzioni pubblicate in v6.0.0.
// Blob sorgente: bffc2e47508877056318de4dd9990e27746f2b24.
// Copiati anche gli helper dello split; barriera e validazione restano comuni.
// Corpus e provenienza: compatibility_600.rs nello stesso percorso.
use super::*;

fn protetto<T>(calcolo: impl FnOnce() -> T) -> Result<T, ExtendedAlgorithmError> {
    crate::calcolo_protetto(calcolo).map_err(ExtendedAlgorithmError::CalcoloNonConcluso)
}

fn validate_input(geometry: &Geometry<f64>) -> Result<(), ExtendedAlgorithmError> {
    if geometry
        .coords_iter()
        .any(|coordinate| !coordinate.x.is_finite() || !coordinate.y.is_finite())
    {
        return Err(ExtendedAlgorithmError::InvalidInput(
            "coordinate NaN o infinite".to_owned(),
        ));
    }
    geometry.validazione_protetta().map_err(|esito| {
        esito.separa(
            |ragione| ExtendedAlgorithmError::InvalidInput(ragione.to_string()),
            ExtendedAlgorithmError::ValidazioneNonConclusa,
        )
    })
}

fn validate_output(geometry: Geometry<f64>) -> Result<Geometry<f64>, ExtendedAlgorithmError> {
    if geometry
        .coords_iter()
        .any(|coordinate| !coordinate.x.is_finite() || !coordinate.y.is_finite())
    {
        return Err(ExtendedAlgorithmError::InvalidOutput(
            "coordinate NaN o infinite".to_owned(),
        ));
    }
    geometry.validazione_protetta().map_err(|esito| {
        esito.separa(
            |ragione| ExtendedAlgorithmError::InvalidOutput(ragione.to_string()),
            ExtendedAlgorithmError::ValidazioneNonConclusa,
        )
    })?;
    Ok(geometry)
}

fn coordinate_count(geometry: &Geometry<f64>) -> Result<u64, ExtendedAlgorithmError> {
    u64::try_from(geometry.coords_count()).map_err(|_| ExtendedAlgorithmError::IndexOverflow)
}

fn validate_ratio(value: f64, name: &'static str) -> Result<(), ExtendedAlgorithmError> {
    if !value.is_finite() || !(0.0..=1.0).contains(&value) {
        return Err(ExtendedAlgorithmError::InvalidParameter {
            name,
            reason: "deve essere finito e compreso tra zero e uno",
        });
    }
    Ok(())
}

pub fn line_substring(
    line: &LineString<f64>,
    start_ratio: f64,
    end_ratio: f64,
) -> Result<Option<Geometry<f64>>, ExtendedAlgorithmError> {
    validate_ratio(start_ratio, "start_ratio")?;
    validate_ratio(end_ratio, "end_ratio")?;
    if start_ratio > end_ratio {
        return Err(ExtendedAlgorithmError::InvalidParameter {
            name: "start_ratio/end_ratio",
            reason: "start_ratio non puo superare end_ratio",
        });
    }
    validate_input(&Geometry::LineString(line.clone()))?;
    let Some(start) = protetto(|| Euclidean.point_at_ratio_from_start(line, start_ratio))? else {
        return Ok(None);
    };
    // Uguaglianza esatta intenzionale: rapporti uguali come numeri (`==`,
    // quindi `-0.0 == 0.0`) definiscono il caso degenere (punto), senza
    // tolleranze implicite.
    #[allow(clippy::float_cmp)]
    if start_ratio == end_ratio {
        return Ok(Some(Geometry::Point(start)));
    }
    let Some(end) = protetto(|| Euclidean.point_at_ratio_from_start(line, end_ratio))? else {
        return Ok(None);
    };
    let total = protetto(|| geo::algorithm::line_measures::Length::length(&Euclidean, line))?;
    if total == 0.0 {
        return Ok(Some(Geometry::Point(start)));
    }
    let start_distance = start_ratio * total;
    let end_distance = end_ratio * total;
    let mut coordinates = vec![start.0];
    let mut traversed = 0.0;
    for segment in line.lines() {
        let segment_length =
            (segment.end.x - segment.start.x).hypot(segment.end.y - segment.start.y);
        traversed += segment_length;
        if traversed > start_distance
            && traversed < end_distance
            && coordinates.last() != Some(&segment.end)
        {
            coordinates.push(segment.end);
        }
    }
    if coordinates.last() != Some(&end.0) {
        coordinates.push(end.0);
    }
    validate_output(Geometry::LineString(LineString::new(coordinates))).map(Some)
}

fn splitter_primitives(
    geometry: &Geometry<f64>,
    points: &mut Vec<Point<f64>>,
    lines: &mut Vec<Line<f64>>,
) -> Result<(), ExtendedAlgorithmError> {
    match geometry {
        Geometry::Point(point) => points.push(*point),
        Geometry::MultiPoint(multi) => points.extend(multi.0.iter().copied()),
        Geometry::LineString(line) => lines.extend(line.lines()),
        Geometry::MultiLineString(multi) => {
            for line in &multi.0 {
                lines.extend(line.lines());
            }
        }
        Geometry::Polygon(polygon) => {
            lines.extend(polygon.exterior().lines());
            for ring in polygon.interiors() {
                lines.extend(ring.lines());
            }
        }
        Geometry::MultiPolygon(multi) => {
            for polygon in &multi.0 {
                splitter_primitives(&Geometry::Polygon(polygon.clone()), points, lines)?;
            }
        }
        Geometry::GeometryCollection(collection) => {
            for child in &collection.0 {
                splitter_primitives(child, points, lines)?;
            }
        }
        _ => {
            return Err(ExtendedAlgorithmError::UnsupportedGeometry {
                operation: "split",
                actual: geometry_type(geometry),
            });
        }
    }
    Ok(())
}

fn ratio_on_source_segment(
    segment: Line<f64>,
    coordinate: Coord<f64>,
    distance_before: f64,
    total_length: f64,
) -> f64 {
    let segment_length = (segment.end.x - segment.start.x).hypot(segment.end.y - segment.start.y);
    if segment_length == 0.0 || total_length == 0.0 {
        return 0.0;
    }
    let local = (coordinate.x - segment.start.x).hypot(coordinate.y - segment.start.y);
    (distance_before + local.min(segment_length)) / total_length
}

#[derive(Clone, Copy)]
struct IndexedSegment {
    line: Line<f64>,
    distance_before: f64,
    length: f64,
    envelope: AABB<[f64; 2]>,
}

impl IndexedSegment {
    fn new(line: Line<f64>, distance_before: f64) -> Self {
        let length = (line.end.x - line.start.x).hypot(line.end.y - line.start.y);
        Self {
            line,
            distance_before,
            length,
            envelope: AABB::from_corners(
                [line.start.x.min(line.end.x), line.start.y.min(line.end.y)],
                [line.start.x.max(line.end.x), line.start.y.max(line.end.y)],
            ),
        }
    }
}

impl RTreeObject for IndexedSegment {
    type Envelope = AABB<[f64; 2]>;

    fn envelope(&self) -> Self::Envelope {
        self.envelope
    }
}

fn point_ratio_on_segment(
    segment: &IndexedSegment,
    point: Point<f64>,
    tolerance: f64,
    total_length: f64,
) -> Option<f64> {
    let dx = segment.line.end.x - segment.line.start.x;
    let dy = segment.line.end.y - segment.line.start.y;
    // Niente mul_add/FMA: la fusione cambia l'arrotondamento IEEE e
    // renderebbe il risultato diverso da piattaforma a piattaforma; la forma
    // non fusa e' il contratto numerico.
    #[allow(clippy::suboptimal_flops)]
    let length_squared = dx * dx + dy * dy;
    if length_squared == 0.0 {
        return None;
    }
    // Stessa forma non fusa, per lo stesso motivo.
    #[allow(clippy::suboptimal_flops)]
    let parameter = (((point.x() - segment.line.start.x) * dx
        + (point.y() - segment.line.start.y) * dy)
        / length_squared)
        .clamp(0.0, 1.0);
    let projected = Coord {
        x: segment.line.start.x + parameter * dx,
        y: segment.line.start.y + parameter * dy,
    };
    let distance = (point.x() - projected.x).hypot(point.y() - projected.y);
    let ratio = (segment.distance_before + parameter * segment.length) / total_length;
    // La proiezione puo' spostare di qualche ULP un punto decimale
    // esattamente collineare (per esempio x=14 su un segmento 0..100): una
    // tolleranza zero deve voler dire coincidenza topologica, non
    // uguaglianza dei bit.
    let coordinate_scale = segment
        .line
        .start
        .x
        .abs()
        .max(segment.line.start.y.abs())
        .max(segment.line.end.x.abs())
        .max(segment.line.end.y.abs())
        .max(point.x().abs())
        .max(point.y().abs())
        .max(1.0);
    let numeric_slack = coordinate_scale * f64::EPSILON * 16.0;
    if distance <= tolerance + numeric_slack {
        Some(ratio)
    } else {
        None
    }
}

fn expanded_point_envelope(point: Point<f64>, tolerance: f64) -> AABB<[f64; 2]> {
    let lower = |value: f64| {
        let result = value - tolerance;
        if result.is_finite() {
            result
        } else {
            -f64::MAX
        }
    };
    let upper = |value: f64| {
        let result = value + tolerance;
        if result.is_finite() {
            result
        } else {
            f64::MAX
        }
    };
    AABB::from_corners(
        [lower(point.x()), lower(point.y())],
        [upper(point.x()), upper(point.y())],
    )
}

/// Taglia una `LineString` con punti, linee o bordi di poligoni (le
/// sorgenti lineari di `geo.split`).
///
/// Un punto taglia dove dista dalla linea al piu' `tolerance` piu' un
/// margine numerico proporzionale al modulo delle coordinate; un lato del
/// tagliatore taglia nei punti d'incrocio e agli estremi di una
/// sovrapposizione. I tagli entro `tolerance` l'uno dall'altro (in
/// lunghezza) si fondono. Il lavoro si limita prima del ciclo quadratico
/// dei test d'intersezione, e le parti devono conservare la lunghezza della
/// sorgente.
///
/// # Errors
///
/// - `InvalidInput`: coordinate NaN o infinite, geometria OGC non valida, o
///   lunghezza della sorgente non finita per overflow numerico;
///   `ValidazioneNonConclusa` se la validazione non conclude;
/// - `CalcoloNonConcluso`: un calcolo di `geo` o `rstar` e' andato in
///   panico;
/// - `CoordinateLimit`: coordinate combinate (sorgente + splitter) oltre
///   `max_input_coordinates`;
/// - `InvalidParameter`: `tolerance` non finita o negativa;
/// - `UnsupportedGeometry`: splitter di tipo `Line`, `Rect` o `Triangle`;
/// - `IndexOverflow`: conteggio non rappresentabile come `u64`;
/// - `WorkLimit`: test di intersezione oltre `max_intersection_tests`;
/// - `OutputLimit`: parti o coordinate di output oltre i limiti richiesti;
/// - `InvalidOutput`: porzione prodotta non valida (propagata da
///   `line_substring`), o split che non conserva la lunghezza della
///   sorgente.
// Pipeline unica di split (walking dei segmenti, test di intersezione
// bounded, ricostruzione delle parti): lunghezza data dalla sequenza
// lineare dei passi del contratto, non da complessita' logica.
#[allow(clippy::too_many_lines)]
pub fn split_line(
    source: &LineString<f64>,
    splitter: &Geometry<f64>,
    tolerance: f64,
    max_input_coordinates: u64,
    max_intersection_tests: u64,
    max_output_parts: u64,
    max_output_coordinates: u64,
) -> Result<Vec<LineString<f64>>, ExtendedAlgorithmError> {
    validate_input(&Geometry::LineString(source.clone()))?;
    validate_input(splitter)?;
    let input_coordinates = u64::try_from(source.0.len())
        .map_err(|_| ExtendedAlgorithmError::IndexOverflow)?
        .checked_add(coordinate_count(splitter)?)
        .ok_or(ExtendedAlgorithmError::IndexOverflow)?;
    if input_coordinates > max_input_coordinates {
        return Err(ExtendedAlgorithmError::CoordinateLimit {
            actual: input_coordinates,
            limit: max_input_coordinates,
        });
    }
    if !tolerance.is_finite() || tolerance < 0.0 {
        return Err(ExtendedAlgorithmError::InvalidParameter {
            name: "tolerance",
            reason: "deve essere finita e non negativa",
        });
    }
    let total_length = protetto(|| Euclidean.length(source))?;
    if !total_length.is_finite() {
        return Err(ExtendedAlgorithmError::InvalidInput(
            "lunghezza non finita per overflow numerico".to_owned(),
        ));
    }
    if total_length == 0.0 {
        return Ok(vec![source.clone()]);
    }
    let mut distance_before = 0.0;
    let source_segments: Vec<_> = source
        .lines()
        .map(|line| {
            let indexed = IndexedSegment::new(line, distance_before);
            distance_before += indexed.length;
            indexed
        })
        .collect();
    let mut points = Vec::new();
    let mut splitter_segments = Vec::new();
    splitter_primitives(splitter, &mut points, &mut splitter_segments)?;
    let source_count =
        u64::try_from(source_segments.len()).map_err(|_| ExtendedAlgorithmError::IndexOverflow)?;
    let splitter_primitive_count = splitter_segments
        .len()
        .checked_add(points.len())
        .ok_or(ExtendedAlgorithmError::IndexOverflow)?;
    let splitter_work = u64::try_from(splitter_primitive_count)
        .map_err(|_| ExtendedAlgorithmError::IndexOverflow)?;
    let work =
        source_count
            .checked_mul(splitter_work)
            .ok_or(ExtendedAlgorithmError::WorkLimit {
                actual: u64::MAX,
                limit: max_intersection_tests,
            })?;
    if work > max_intersection_tests {
        return Err(ExtendedAlgorithmError::WorkLimit {
            actual: work,
            limit: max_intersection_tests,
        });
    }

    let mut ratios = Vec::new();
    let source_tree = protetto(|| RTree::bulk_load(source_segments.clone()))?;
    let coordinate_scale = source
        .coords_iter()
        .chain(splitter.coords_iter())
        .fold(1.0_f64, |scale, coordinate| {
            scale.max(coordinate.x.abs()).max(coordinate.y.abs())
        });
    // Niente mul_add/FMA: la fusione cambia l'arrotondamento IEEE e
    // renderebbe il risultato diverso da piattaforma a piattaforma; la forma
    // non fusa e' il contratto numerico.
    #[allow(clippy::suboptimal_flops)]
    let query_tolerance = (tolerance + coordinate_scale * f64::EPSILON * 16.0).min(f64::MAX);
    for point in points {
        let envelope = expanded_point_envelope(point, query_tolerance);
        let vicini: Vec<&IndexedSegment> = protetto(|| {
            source_tree
                .locate_in_envelope_intersecting(&envelope)
                .collect()
        })?;
        ratios.extend(
            vicini.into_iter().filter_map(|segment| {
                point_ratio_on_segment(segment, point, tolerance, total_length)
            }),
        );
    }
    let splitter_indexed: Vec<IndexedSegment> = splitter_segments
        .iter()
        .copied()
        .map(|line| IndexedSegment::new(line, 0.0))
        .collect();
    let splitter_tree = protetto(|| RTree::bulk_load(splitter_indexed))?;
    for source_segment in &source_segments {
        let incroci: Vec<&IndexedSegment> = protetto(|| {
            splitter_tree
                .locate_in_envelope_intersecting(&source_segment.envelope)
                .collect()
        })?;
        for splitter_segment in incroci {
            let (sorgente, taglio) = (source_segment.line, splitter_segment.line);
            match protetto(|| line_intersection(sorgente, taglio))? {
                Some(LineIntersection::SinglePoint { intersection, .. }) => {
                    ratios.push(ratio_on_source_segment(
                        source_segment.line,
                        intersection,
                        source_segment.distance_before,
                        total_length,
                    ));
                }
                Some(LineIntersection::Collinear { intersection }) => {
                    ratios.push(ratio_on_source_segment(
                        source_segment.line,
                        intersection.start,
                        source_segment.distance_before,
                        total_length,
                    ));
                    ratios.push(ratio_on_source_segment(
                        source_segment.line,
                        intersection.end,
                        source_segment.distance_before,
                        total_length,
                    ));
                }
                None => {}
            }
        }
    }
    ratios.retain(|ratio| ratio.is_finite() && *ratio > 0.0 && *ratio < 1.0);
    ratios.sort_by(f64::total_cmp);
    // La tolleranza del chiamante puo' fondere tagli vicini, per scelta.
    // Con tolleranza esatta (zero) si assorbono solo i duplicati numerici
    // attorno a un vertice condiviso, cosi' i pezzi minuscoli legittimi
    // restano.
    let ratio_tolerance = (tolerance / total_length).max(f64::EPSILON * 8.0);
    ratios.dedup_by(|left, right| (*left - *right).abs() <= ratio_tolerance);
    let part_count =
        u64::try_from(ratios.len() + 1).map_err(|_| ExtendedAlgorithmError::IndexOverflow)?;
    if part_count > max_output_parts {
        return Err(ExtendedAlgorithmError::OutputLimit {
            actual: part_count,
            limit: max_output_parts,
        });
    }
    let mut boundaries = Vec::with_capacity(ratios.len() + 2);
    boundaries.push(0.0);
    boundaries.extend(ratios);
    boundaries.push(1.0);
    let mut output = Vec::with_capacity(boundaries.len() - 1);
    for window in boundaries.windows(2) {
        let Some(piece) = line_substring(source, window[0], window[1])? else {
            continue;
        };
        let Geometry::LineString(piece) = piece else {
            continue;
        };
        output.push(piece);
    }
    let output_length: f64 = output
        .iter()
        .map(|line| protetto(|| Euclidean.length(line)))
        .collect::<Result<Vec<f64>, _>>()?
        .into_iter()
        .sum();
    let allowed_error = total_length.abs().max(1.0) * 1e-10;
    if (output_length - total_length).abs() > allowed_error {
        return Err(ExtendedAlgorithmError::InvalidOutput(
            "lo split non conserva la lunghezza".to_owned(),
        ));
    }
    let output_coordinates = output.iter().try_fold(0_u64, |total, line| {
        let count =
            u64::try_from(line.0.len()).map_err(|_| ExtendedAlgorithmError::IndexOverflow)?;
        total
            .checked_add(count)
            .ok_or(ExtendedAlgorithmError::IndexOverflow)
    })?;
    if output_coordinates > max_output_coordinates {
        return Err(ExtendedAlgorithmError::OutputLimit {
            actual: output_coordinates,
            limit: max_output_coordinates,
        });
    }
    Ok(output)
}
