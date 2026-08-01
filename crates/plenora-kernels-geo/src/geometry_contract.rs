//! Contratto di geometria su forme decodificate (ADR-0012, D12.3/D12.4).
//!
//! Due helper che riproducono SU `Geometry<f64>` cio' che il percorso non
//! fuso ottiene con il round-trip WKB (encode canonico `to_wkb` +
//! validazione del decoder ADR-0011):
//!
//! - [`wkb_size_xy`]: dimensione ESATTA del WKB ISO XY prodotto da
//!   `geozero::ToWkb::to_wkb(CoordDimensions::xy())` — camminata
//!   strutturale, nessuna serializzazione, nessuna stima;
//! - [`validate_geometry_structural`]: le stesse regole del decoder
//!   validante (`wkb_decoder`), nello stesso ordine di valutazione e con
//!   gli stessi messaggi d'errore.
//!
//! Note di parita' con l'encoder canonico (geozero 0.15), verificate dai
//! test di questo modulo:
//!
//! - un `Polygon` e' SEMPRE codificato con l'anello esterno piu' gli
//!   interni: un poligono vuoto produce un anello esterno a zero
//!   coordinate, che il decoder rifiuta — quindi anche qui il poligono
//!   vuoto e' rifiutato, esattamente come nel percorso non fuso (che lo
//!   scopre alla ri-validazione post-encode);
//! - `Line` e' codificata come `LineString` di due coordinate, `Rect` e
//!   `Triangle` come poligoni (un anello chiuso di 5 e 4 coordinate):
//!   misura e validazione seguono questa mappa canonica;
//! - i figli delle multi-geometrie sono tipizzati per costruzione in
//!   `geo` (un `MultiPoint` contiene solo `Point`): l'incoerenza di tipo
//!   figlio prevista dal decoder non e' rappresentabile su geometria.

use geo::{Coord, Geometry, LineString, Polygon};

use plenora_core::PlenoraError;

use crate::{invalid_wkb_structure, non_finite_coordinate};

/// Byte dell'header di ogni geometria WKB: byte order (1) + type code (4).
const HEADER_BYTES: u64 = 5;
/// Byte di un conteggio (coordinate, anelli o figli): un `u32`.
const COUNT_BYTES: u64 = 4;
/// Byte di una coordinata XY: due `f64`.
const COORD_BYTES: u64 = 16;

/// Dimensione esatta in byte del WKB ISO XY di `geometry` (ADR-0012 D12.3).
///
/// Camminata strutturale speculare all'encoder canonico
/// (`geozero::ToWkb::to_wkb(CoordDimensions::xy())`): un header per ogni
/// geometria (figli di multi e collezioni inclusi), un conteggio per ogni
/// sequenza di coordinate, anelli o figli, 16 byte per coordinata. Le
/// geometrie vuote mantengono i propri header e conteggi. Il risultato e'
/// funzione pura della struttura: nessuna stima, nessuna serializzazione.
#[must_use]
pub fn wkb_size_xy(geometry: &Geometry<f64>) -> u64 {
    match geometry {
        Geometry::Point(_) => HEADER_BYTES + COORD_BYTES,
        // Mappa canonica dell'encoder: Line come LineString di due punti,
        // Rect/Triangle come poligoni a un anello di 5 e 4 coordinate.
        Geometry::Line(_) => HEADER_BYTES + COUNT_BYTES + 2 * COORD_BYTES,
        Geometry::LineString(line) => linestring_size_xy(line),
        Geometry::Polygon(polygon) => polygon_size_xy(polygon),
        Geometry::MultiPoint(multi) => {
            HEADER_BYTES + COUNT_BYTES + multi.0.len() as u64 * (HEADER_BYTES + COORD_BYTES)
        }
        Geometry::MultiLineString(multi) => {
            HEADER_BYTES + COUNT_BYTES + multi.0.iter().map(linestring_size_xy).sum::<u64>()
        }
        Geometry::MultiPolygon(multi) => {
            HEADER_BYTES + COUNT_BYTES + multi.0.iter().map(polygon_size_xy).sum::<u64>()
        }
        Geometry::GeometryCollection(collection) => {
            HEADER_BYTES + COUNT_BYTES + collection.0.iter().map(wkb_size_xy).sum::<u64>()
        }
        Geometry::Rect(_) => HEADER_BYTES + 2 * COUNT_BYTES + 5 * COORD_BYTES,
        Geometry::Triangle(_) => HEADER_BYTES + 2 * COUNT_BYTES + 4 * COORD_BYTES,
    }
}

/// `LineString` codificata: header + conteggio + coordinate.
const fn linestring_size_xy(line: &LineString<f64>) -> u64 {
    HEADER_BYTES + COUNT_BYTES + COORD_BYTES * line.0.len() as u64
}

/// Poligono codificato: header + numero anelli + (conteggio + coordinate)
/// per ogni anello. L'anello esterno e' sempre presente, anche se vuoto.
fn polygon_size_xy(polygon: &Polygon<f64>) -> u64 {
    HEADER_BYTES
        + COUNT_BYTES
        + std::iter::once(polygon.exterior())
            .chain(polygon.interiors())
            .map(|ring| COUNT_BYTES + COORD_BYTES * ring.0.len() as u64)
            .sum::<u64>()
}

/// Validazione strutturale di una geometria decodificata (ADR-0012 D12.4).
///
/// L'equivalente su `Geometry<f64>` delle regole del decoder validante
/// ([`crate::wkb_decoder`], ADR-0011), nello stesso ordine di valutazione
/// e con gli stessi messaggi d'errore: profondita' di annidamento,
/// conteggio dei componenti (una unita' per geometria, figli inclusi),
/// cardinalita' delle `LineString`, coordinate finite (NaN e infiniti
/// rifiutati), cardinalita' e chiusura esatta degli anelli
/// (first == last, confronto su X/Y).
///
/// La validazione OGC NON e' compresa: resta responsabilita' del
/// chiamante (topologica, non strutturale — come in ADR-0011).
///
/// # Errors
///
/// `PlenoraError::InvalidPlan` se l'annidamento supera `max_depth`, se i
/// componenti superano `max_components`, se una `LineString` ha esattamente
/// una coordinata, se una coordinata e' NaN o infinita, o se un anello ha
/// meno di quattro coordinate o non e' chiuso.
pub fn validate_geometry_structural(
    geometry: &Geometry<f64>,
    max_depth: usize,
    max_components: u64,
) -> Result<(), PlenoraError> {
    let mut components = 0_u64;
    validate_node(geometry, 0, max_depth, max_components, &mut components)
}

/// Contabilita' per nodo, nello stesso ordine del decoder: prima il
/// limite di profondita', poi il conteggio dei componenti.
fn enter_node(
    depth: usize,
    max_depth: usize,
    max_components: u64,
    components: &mut u64,
) -> Result<(), PlenoraError> {
    if depth > max_depth {
        return Err(invalid_wkb_structure(
            "annidamento geometrie oltre il limite",
        ));
    }
    *components = components
        .checked_add(1)
        .ok_or_else(|| invalid_wkb_structure("conteggio componenti oltre il limite"))?;
    if *components > max_components {
        return Err(invalid_wkb_structure(
            "conteggio componenti oltre il limite",
        ));
    }
    Ok(())
}

fn validate_node(
    geometry: &Geometry<f64>,
    depth: usize,
    max_depth: usize,
    max_components: u64,
    components: &mut u64,
) -> Result<(), PlenoraError> {
    enter_node(depth, max_depth, max_components, components)?;
    match geometry {
        Geometry::Point(point) => check_finite(&point.0),
        Geometry::Line(line) => {
            check_finite(&line.start)?;
            check_finite(&line.end)
        }
        Geometry::LineString(line) => check_linestring(line),
        Geometry::Polygon(polygon) => check_polygon(polygon),
        Geometry::MultiPoint(multi) => multi.0.iter().try_for_each(|point| {
            enter_node(depth + 1, max_depth, max_components, components)?;
            check_finite(&point.0)
        }),
        Geometry::MultiLineString(multi) => multi.0.iter().try_for_each(|line| {
            enter_node(depth + 1, max_depth, max_components, components)?;
            check_linestring(line)
        }),
        Geometry::MultiPolygon(multi) => multi.0.iter().try_for_each(|polygon| {
            enter_node(depth + 1, max_depth, max_components, components)?;
            check_polygon(polygon)
        }),
        Geometry::GeometryCollection(collection) => collection.0.iter().try_for_each(|child| {
            validate_node(child, depth + 1, max_depth, max_components, components)
        }),
        // Rect/Triangle sono codificati come poligoni chiusi per
        // costruzione: resta solo la finitezza delle coordinate.
        Geometry::Rect(rect) => {
            check_finite(&rect.min())?;
            check_finite(&rect.max())
        }
        Geometry::Triangle(triangle) => {
            check_finite(&triangle.v1())?;
            check_finite(&triangle.v2())?;
            check_finite(&triangle.v3())
        }
    }
}

/// X/Y finite (NaN e infiniti rifiutati, ADR-0001), stesso errore del
/// decoder.
fn check_finite(coord: &Coord<f64>) -> Result<(), PlenoraError> {
    if coord.x.is_finite() && coord.y.is_finite() {
        Ok(())
    } else {
        Err(non_finite_coordinate())
    }
}

/// `LineString`: vuota o almeno due coordinate, tutte finite.
fn check_linestring(line: &LineString<f64>) -> Result<(), PlenoraError> {
    if line.0.len() == 1 {
        return Err(invalid_wkb_structure(
            "LineString deve essere vuota o avere almeno due coordinate",
        ));
    }
    line.0.iter().try_for_each(check_finite)
}

/// Poligono: anello esterno sempre presente (come nell'encoder canonico:
/// un poligono vuoto e' rifiutato) piu' gli anelli interni.
fn check_polygon(polygon: &Polygon<f64>) -> Result<(), PlenoraError> {
    std::iter::once(polygon.exterior())
        .chain(polygon.interiors())
        .try_for_each(check_ring)
}

/// Anello: almeno quattro coordinate, tutte finite, chiusura esatta
/// (first == last su X/Y — i NaN sono gia' esclusi dalla finitezza,
/// nessun margine epsilon: determinismo ADR-0001).
fn check_ring(ring: &LineString<f64>) -> Result<(), PlenoraError> {
    if ring.0.len() < 4 {
        return Err(invalid_wkb_structure(
            "anello poligonale con meno di quattro coordinate",
        ));
    }
    ring.0.iter().try_for_each(check_finite)?;
    if ring.0.first() != ring.0.last() {
        return Err(invalid_wkb_structure("anello poligonale non chiuso"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use geo::{
        line_string, polygon, GeometryCollection, Line, MultiLineString, MultiPoint, MultiPolygon,
        Point, Rect, Triangle,
    };
    use geozero::{CoordDimensions, ToWkb};

    use super::*;
    use crate::{wkb_decoder, MAX_WKB_COMPONENTS, MAX_WKB_DEPTH};

    /// Batteria di geometrie valide usata da entrambe le parita'.
    fn valid_fixtures() -> Vec<(&'static str, Geometry<f64>)> {
        let triangle = Geometry::Polygon(polygon![
            (x: 0.0, y: 0.0),
            (x: 4.0, y: 0.0),
            (x: 2.0, y: 3.0),
            (x: 0.0, y: 0.0),
        ]);
        let holed = Geometry::Polygon(polygon!(
            exterior: [
                (x: 0.0, y: 0.0),
                (x: 10.0, y: 0.0),
                (x: 10.0, y: 10.0),
                (x: 0.0, y: 10.0),
                (x: 0.0, y: 0.0),
            ],
            interiors: [[
                (x: 2.0, y: 2.0),
                (x: 4.0, y: 2.0),
                (x: 2.0, y: 4.0),
                (x: 2.0, y: 2.0),
            ]],
        ));
        vec![
            ("point", Geometry::Point(Point::new(1.5, -2.5))),
            (
                "line",
                Geometry::Line(Line::new(
                    Coord { x: 0.0, y: 0.0 },
                    Coord { x: 3.0, y: 4.0 },
                )),
            ),
            (
                "linestring",
                Geometry::LineString(
                    line_string![(x: 0.0, y: 0.0), (x: 1.0, y: 1.0), (x: 2.0, y: 0.5)],
                ),
            ),
            (
                "linestring vuota",
                Geometry::LineString(LineString::from(Vec::<(f64, f64)>::new())),
            ),
            ("polygon semplice", triangle.clone()),
            ("polygon con buco", holed.clone()),
            (
                "multipoint",
                Geometry::MultiPoint(MultiPoint::new(vec![
                    Point::new(0.0, 0.0),
                    Point::new(3.0, 4.0),
                ])),
            ),
            (
                "multipoint vuota",
                Geometry::MultiPoint(MultiPoint::new(Vec::new())),
            ),
            (
                "multilinestring",
                Geometry::MultiLineString(MultiLineString::new(vec![
                    line_string![(x: 0.0, y: 0.0), (x: 1.0, y: 1.0)],
                    line_string![(x: 2.0, y: 2.0), (x: 3.0, y: 3.0), (x: 4.0, y: 2.0)],
                ])),
            ),
            (
                "multipolygon",
                Geometry::MultiPolygon(MultiPolygon::new(vec![
                    match &triangle {
                        Geometry::Polygon(p) => p.clone(),
                        _ => unreachable!("fixture"),
                    },
                    match &holed {
                        Geometry::Polygon(p) => p.clone(),
                        _ => unreachable!("fixture"),
                    },
                ])),
            ),
            (
                "rect",
                Geometry::Rect(Rect::new(
                    Coord { x: 0.0, y: 0.0 },
                    Coord { x: 2.0, y: 1.0 },
                )),
            ),
            (
                "triangle",
                Geometry::Triangle(Triangle::new(
                    Coord { x: 0.0, y: 0.0 },
                    Coord { x: 4.0, y: 0.0 },
                    Coord { x: 2.0, y: 3.0 },
                )),
            ),
            (
                "collection annidata",
                Geometry::GeometryCollection(GeometryCollection::new_from(vec![
                    Geometry::Point(Point::new(0.0, 0.0)),
                    Geometry::GeometryCollection(GeometryCollection::new_from(vec![
                        Geometry::LineString(line_string![(x: 0.0, y: 0.0), (x: 5.0, y: 5.0)]),
                        triangle,
                    ])),
                    holed,
                ])),
            ),
            (
                "collection vuota",
                Geometry::GeometryCollection(GeometryCollection::new_from(Vec::new())),
            ),
        ]
    }

    /// Parita' di misura (D12.3): la camminata strutturale deve dare
    /// esattamente la lunghezza del WKB prodotto dall'encoder canonico.
    #[test]
    fn wkb_size_xy_matches_canonical_encoder() {
        for (label, geometry) in valid_fixtures() {
            let encoded = geometry
                .to_wkb(CoordDimensions::xy())
                .expect("encode fixture");
            assert_eq!(
                wkb_size_xy(&geometry),
                encoded.len() as u64,
                "{label}: dimensione diversa dall'encoder"
            );
        }
    }

    /// Anche il poligono vuoto: l'encoder scrive un anello esterno a zero
    /// coordinate e la misura deve combaciare lo stesso.
    #[test]
    fn wkb_size_xy_matches_encoder_on_empty_polygon() {
        let empty = Geometry::Polygon(Polygon::new(
            LineString::from(Vec::<(f64, f64)>::new()),
            Vec::new(),
        ));
        let encoded = empty.to_wkb(CoordDimensions::xy()).expect("encode fixture");
        assert_eq!(wkb_size_xy(&empty), encoded.len() as u64);
    }

    /// Parita' di validazione (D12.4): l'esito (e il messaggio d'errore)
    /// deve coincidere con il percorso non fuso, cioe' encode canonico +
    /// decode validante.
    fn assert_validation_parity(geometry: &Geometry<f64>, label: &str) {
        let reference = geometry
            .to_wkb(CoordDimensions::xy())
            .map_err(|error| PlenoraError::InvalidPlan(error.to_string()))
            .and_then(|payload| wkb_decoder::decode_validated(&payload).map(|_| ()));
        let validated = validate_geometry_structural(geometry, MAX_WKB_DEPTH, MAX_WKB_COMPONENTS);
        match (&reference, &validated) {
            (Ok(()), Ok(())) => {}
            (Err(expected), Err(actual)) => assert_eq!(
                expected.to_string(),
                actual.to_string(),
                "{label}: messaggi d'errore diversi"
            ),
            (Ok(()), Err(error)) => panic!("{label}: round-trip Ok, validatore Err: {error}"),
            (Err(error), Ok(())) => panic!("{label}: round-trip Err ({error}), validatore Ok"),
        }
    }

    #[test]
    fn validation_parity_on_valid_geometries() {
        for (label, geometry) in valid_fixtures() {
            assert_validation_parity(&geometry, label);
        }
    }

    /// Poligono costruito con anello esterno aperto: `Polygon::new` chiude
    /// automaticamente gli anelli, quindi la forma risultante e' chiusa e
    /// valida per entrambi i percorsi (la non-chiusura non e'
    /// rappresentabile in `geo::Polygon`; il controllo resta per parita'
    /// difensiva con il decoder ed e' coperto da `check_ring`).
    fn auto_closed_polygon() -> Geometry<f64> {
        Geometry::Polygon(Polygon::new(
            LineString::from(vec![(0.0, 0.0), (4.0, 0.0), (4.0, 4.0), (0.0, 4.0)]),
            Vec::new(),
        ))
    }

    #[test]
    fn validation_parity_on_rejected_geometries() {
        let cases: Vec<(&'static str, Geometry<f64>)> = vec![
            ("point NaN", Geometry::Point(Point::new(f64::NAN, 0.0))),
            (
                "point infinito",
                Geometry::Point(Point::new(0.0, f64::INFINITY)),
            ),
            (
                "linestring con NaN",
                Geometry::LineString(line_string![(x: 0.0, y: 0.0), (x: f64::NAN, y: 1.0)]),
            ),
            (
                "linestring da una coordinata",
                Geometry::LineString(line_string![(x: 0.0, y: 0.0)]),
            ),
            (
                "anello con tre coordinate",
                Geometry::Polygon(Polygon::new(
                    LineString::from(vec![(0.0, 0.0), (1.0, 0.0), (0.0, 0.0)]),
                    Vec::new(),
                )),
            ),
            (
                "anello aperto chiuso da Polygon::new",
                auto_closed_polygon(),
            ),
            (
                "anello interno aperto chiuso da Polygon::new",
                Geometry::Polygon(polygon!(
                    exterior: [
                        (x: 0.0, y: 0.0),
                        (x: 10.0, y: 0.0),
                        (x: 10.0, y: 10.0),
                        (x: 0.0, y: 0.0),
                    ],
                    interiors: [[(x: 1.0, y: 1.0), (x: 2.0, y: 1.0), (x: 1.0, y: 2.0), (x: 9.0, y: 9.0)]],
                )),
            ),
            (
                "anello con infinito",
                Geometry::Polygon(Polygon::new(
                    LineString::from(vec![
                        (0.0, 0.0),
                        (f64::NEG_INFINITY, 0.0),
                        (1.0, 1.0),
                        (0.0, 0.0),
                    ]),
                    Vec::new(),
                )),
            ),
            (
                "poligono vuoto",
                Geometry::Polygon(Polygon::new(
                    LineString::from(Vec::<(f64, f64)>::new()),
                    Vec::new(),
                )),
            ),
            (
                "NaN in figlio di collection",
                Geometry::GeometryCollection(GeometryCollection::new_from(vec![
                    Geometry::Point(Point::new(0.0, 0.0)),
                    Geometry::Point(Point::new(f64::NAN, 1.0)),
                ])),
            ),
            (
                "NaN in figlio di multipolygon",
                Geometry::MultiPolygon(MultiPolygon::new(vec![Polygon::new(
                    LineString::from(vec![(0.0, 0.0), (f64::NAN, 0.0), (1.0, 1.0), (0.0, 0.0)]),
                    Vec::new(),
                )])),
            ),
            (
                "line con infinito",
                Geometry::Line(Line::new(
                    Coord { x: 0.0, y: 0.0 },
                    Coord {
                        x: f64::INFINITY,
                        y: 0.0,
                    },
                )),
            ),
        ];
        for (label, geometry) in cases {
            assert_validation_parity(&geometry, label);
        }
    }

    /// GC annidata con `levels` livelli sopra il punto foglia.
    fn nested_collection(levels: usize) -> Geometry<f64> {
        let mut geometry = Geometry::Point(Point::new(0.0, 0.0));
        for _ in 0..levels {
            geometry = Geometry::GeometryCollection(GeometryCollection::new_from(vec![geometry]));
        }
        geometry
    }

    #[test]
    fn validation_parity_on_nesting_depth() {
        // Foglia a profondita' 70: oltre il default MAX_WKB_DEPTH (64).
        let deep = nested_collection(70);
        assert_validation_parity(&deep, "annidamento oltre il limite");
        // Al limite esatto entrambi accettano; un livello oltre rifiutano.
        for (levels, max_depth, expected_ok) in [(2_usize, 2_usize, true), (3, 2, false)] {
            let geometry = nested_collection(levels);
            let payload = geometry
                .to_wkb(CoordDimensions::xy())
                .expect("encode fixture");
            let reference =
                wkb_decoder::decode_validated_with_depth(&payload, max_depth).map(|_| ());
            let validated = validate_geometry_structural(&geometry, max_depth, MAX_WKB_COMPONENTS);
            assert_eq!(
                reference.is_ok(),
                validated.is_ok(),
                "esito diverso a {levels} livelli con limite {max_depth}"
            );
            assert_eq!(validated.is_ok(), expected_ok);
            if let (Err(expected), Err(actual)) = (&reference, &validated) {
                assert_eq!(expected.to_string(), actual.to_string());
            }
        }
        // Messaggio esatto sul superamento del limite.
        assert!(matches!(
            validate_geometry_structural(&deep, MAX_WKB_DEPTH, MAX_WKB_COMPONENTS),
            Err(PlenoraError::InvalidPlan(reason))
                if reason == "struttura WKB non valida: annidamento geometrie oltre il limite"
        ));
    }

    #[test]
    fn component_count_limit_is_enforced_with_decoder_message() {
        // GC con tre figli: 4 componenti (radice inclusa).
        let geometry = Geometry::GeometryCollection(GeometryCollection::new_from(vec![
            Geometry::Point(Point::new(0.0, 0.0)),
            Geometry::Point(Point::new(1.0, 1.0)),
            Geometry::Point(Point::new(2.0, 2.0)),
        ]));
        assert!(validate_geometry_structural(&geometry, MAX_WKB_DEPTH, 4).is_ok());
        assert!(matches!(
            validate_geometry_structural(&geometry, MAX_WKB_DEPTH, 3),
            Err(PlenoraError::InvalidPlan(reason))
                if reason == "struttura WKB non valida: conteggio componenti oltre il limite"
        ));
        // Il limite di default e' quello del decoder (MAX_WKB_COMPONENTS).
        assert!(validate_geometry_structural(&geometry, MAX_WKB_DEPTH, MAX_WKB_COMPONENTS).is_ok());
    }

    #[test]
    fn error_messages_match_decoder_verbatim() {
        let nan = Geometry::Point(Point::new(f64::NAN, 0.0));
        assert!(matches!(
            validate_geometry_structural(&nan, MAX_WKB_DEPTH, MAX_WKB_COMPONENTS),
            Err(PlenoraError::InvalidPlan(reason))
                if reason == "WKB contiene coordinate NaN o infinite"
        ));
        let single = Geometry::LineString(line_string![(x: 0.0, y: 0.0)]);
        assert!(matches!(
            validate_geometry_structural(&single, MAX_WKB_DEPTH, MAX_WKB_COMPONENTS),
            Err(PlenoraError::InvalidPlan(reason))
                if reason == "struttura WKB non valida: LineString deve essere vuota o avere almeno due coordinate"
        ));
        let short_ring = Geometry::Polygon(Polygon::new(
            LineString::from(vec![(0.0, 0.0), (1.0, 0.0), (0.0, 0.0)]),
            Vec::new(),
        ));
        assert!(matches!(
            validate_geometry_structural(&short_ring, MAX_WKB_DEPTH, MAX_WKB_COMPONENTS),
            Err(PlenoraError::InvalidPlan(reason))
                if reason == "struttura WKB non valida: anello poligonale con meno di quattro coordinate"
        ));
        // Chiusura esatta: irraggiungibile via `Polygon::new` (chiude gli
        // anelli), verificata direttamente su `check_ring` — stessa regola
        // e stesso messaggio del decoder.
        let open_ring = LineString::from(vec![(0.0, 0.0), (4.0, 0.0), (4.0, 4.0), (0.0, 4.0)]);
        assert!(matches!(
            check_ring(&open_ring),
            Err(PlenoraError::InvalidPlan(reason))
                if reason == "struttura WKB non valida: anello poligonale non chiuso"
        ));
    }
}
