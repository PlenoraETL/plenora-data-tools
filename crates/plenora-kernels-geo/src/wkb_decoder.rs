//! Decoder WKB validante (architettura.md#geometrie).
//!
//! UNA passata sui byte che esegue la validazione strutturale del contratto
//! e costruisce la geometria nella stessa camminata — le due scansioni del
//! percorso precedente (`validate_wkb_contract` + `geozero::Wkb::to_geo`)
//! fuse senza cedere nulla della garanzia fail-closed.
//!
//! Contratto di parita' (verificato dall'oracolo differenziale nei test di
//! questo modulo): ogni payload accettato/rifiutato dal percorso
//! `validate_wkb_contract` + `Wkb::to_geo` produce qui lo stesso esito, la
//! stessa categoria di errore e la stessa geometria, coordinata per
//! coordinata. Le regole sono quelle del validatore strutturale esistente,
//! nello stesso ordine di valutazione: byte order, type code (protocollo
//! 2D: niente SRID/riservati/serie Z-M), conteggi contro i byte residui,
//! X/Y finite, anelli chiusi (confronto esatto first == last), annidamento
//! limitato, tipi figli delle multi-geometrie, nessun byte residuo.

use geo::{
    Geometry, GeometryCollection, LineString, MultiLineString, MultiPoint, MultiPolygon, Point,
    Polygon,
};

use plenora_core::PlenoraError;

use crate::{
    checked_count, invalid_wkb_structure, parse_wkb_type_code, GeometryDimensions, WkbCursor,
    MAX_WKB_BYTES, MAX_WKB_COMPONENTS, MAX_WKB_DEPTH,
};

/// Decodifica e valida un payload WKB in una sola passata (architettura.md#geometrie).
///
/// Copre la classe di input del protocollo 2D (come
/// [`crate::geometry_from_wkb`]): serie Z/M e SRID rifiutate come
/// [`crate::Unsupported`]. La validazione OGC della geometria risultante
/// NON e' compresa: resta responsabilita' del chiamante (topologica, non
/// strutturale — vedi architettura.md#geometrie).
///
/// # Errors
///
/// `PlenoraError::InvalidPlan` se la struttura WKB non e' valida (byte o
/// conteggi oltre i limiti, anelli non chiusi, coordinate NaN o infinite,
/// byte residui, annidamento oltre [`MAX_WKB_DEPTH`]);
/// `PlenoraError::Unsupported` se il payload porta dimensioni Z/M o SRID
/// non preservabili nel protocollo 2D.
pub fn decode_validated(payload: &[u8]) -> Result<Geometry<f64>, PlenoraError> {
    decode_validated_with_depth(payload, MAX_WKB_DEPTH)
}

/// Variante con profondita' di annidamento configurabile (come
/// [`crate::validate_wkb_contract_with_depth`]): il limite arriva dai
/// `Limits` effettivi del piano.
///
/// # Errors
///
/// Come [`decode_validated`].
pub fn decode_validated_with_depth(
    payload: &[u8],
    max_depth: usize,
) -> Result<Geometry<f64>, PlenoraError> {
    if payload.len() > MAX_WKB_BYTES {
        return Err(invalid_wkb_structure("WKB oltre il limite di 64 MiB"));
    }
    let mut cursor = WkbCursor::new(payload);
    let mut components = 0_u64;
    let (_, geometry) = decode_geometry(&mut cursor, 0, max_depth, &mut components)?;
    if cursor.remaining() != 0 {
        return Err(invalid_wkb_structure("byte residui dopo la geometria"));
    }
    Ok(geometry)
}

/// Camminata unica valida+costruisce, speculare a
/// `crate::validate_wkb_geometry_with_dimensions`: stesse regole, stesso
/// ordine di valutazione, stessi errori — con la geometria costruita dalle
/// stesse coordinate consumate.
// Dispatcher esaustivo per tipo geometrico WKB: un solo corpo tiene le
// regole di validazione e costruzione allineate alla speculare del
// validatore strutturale (architettura.md#geometrie); spezzarlo peggiorerebbe il confronto
// riga-per-riga con `validate_wkb_geometry_with_dimensions`.
#[allow(clippy::too_many_lines)]
fn decode_geometry(
    cursor: &mut WkbCursor<'_>,
    depth: usize,
    max_depth: usize,
    components: &mut u64,
) -> Result<(u32, Geometry<f64>), PlenoraError> {
    if depth > max_depth {
        return Err(invalid_wkb_structure(
            "annidamento geometrie oltre il limite",
        ));
    }
    *components = components
        .checked_add(1)
        .ok_or_else(|| invalid_wkb_structure("conteggio componenti oltre il limite"))?;
    if *components > MAX_WKB_COMPONENTS {
        return Err(invalid_wkb_structure(
            "conteggio componenti oltre il limite",
        ));
    }
    let byte_order = cursor.read_u8()?;
    let little_endian = match byte_order {
        0 => false,
        1 => true,
        _ => return Err(invalid_wkb_structure("byte order non valido")),
    };
    let raw_type = cursor.read_u32(little_endian)?;
    let (geometry_type, stride) = parse_wkb_type_code(raw_type, GeometryDimensions::Xy)?;
    match geometry_type {
        1 => {
            let (x, y) = cursor.read_coordinate(little_endian, stride)?;
            Ok((1, Geometry::Point(Point::new(x, y))))
        }
        2 => {
            let count = cursor.read_u32(little_endian)?;
            let count = checked_count(count, cursor.remaining(), stride)?;
            if count == 1 {
                return Err(invalid_wkb_structure(
                    "LineString deve essere vuota o avere almeno due coordinate",
                ));
            }
            let mut coordinates = Vec::with_capacity(count);
            for _ in 0..count {
                let (x, y) = cursor.read_coordinate(little_endian, stride)?;
                coordinates.push((x, y));
            }
            Ok((2, Geometry::LineString(LineString::from(coordinates))))
        }
        3 => {
            let rings = cursor.read_u32(little_endian)?;
            let rings = checked_count(rings, cursor.remaining(), 4)?;
            let mut exterior: Option<LineString<f64>> = None;
            // Capacita' esatta (un anello e' l'esterno): la forma
            // decodificata resta densa, invariante del preflight D14.4
            // (`decoded_size_xy` — il test di conservativita' misura le
            // capacita' reali).
            let mut interiors: Vec<LineString<f64>> = Vec::with_capacity(rings.saturating_sub(1));
            for _ in 0..rings {
                let count = cursor.read_u32(little_endian)?;
                let count = checked_count(count, cursor.remaining(), stride)?;
                if count < 4 {
                    return Err(invalid_wkb_structure(
                        "anello poligonale con meno di quattro coordinate",
                    ));
                }
                let first = cursor.read_coordinate(little_endian, stride)?;
                let mut ring = Vec::with_capacity(count);
                ring.push(first);
                let mut last = first;
                for _ in 1..count {
                    last = cursor.read_coordinate(little_endian, stride)?;
                    ring.push(last);
                }
                if first != last {
                    return Err(invalid_wkb_structure("anello poligonale non chiuso"));
                }
                // Il primo anello e' l'esterno, i successivi gli interni.
                // Mai `exterior.take()` nel discriminante: con un secondo
                // anello scarterebbe il primo (bug trovato il 2026-07-29 —
                // l'oracolo copriva solo poligoni a un anello).
                if exterior.is_none() {
                    exterior = Some(LineString::from(ring));
                } else {
                    interiors.push(LineString::from(ring));
                }
            }
            // Zero anelli: poligono vuoto, accettato come dal percorso
            // precedente (validatore strutturale e geozero concordi).
            let exterior = exterior.unwrap_or_else(|| LineString::from(Vec::<(f64, f64)>::new()));
            Ok((3, Geometry::Polygon(Polygon::new(exterior, interiors))))
        }
        4..=7 => {
            let children = cursor.read_u32(little_endian)?;
            let children = checked_count(children, cursor.remaining(), 5)?;
            let mut decoded: Vec<(u32, Geometry<f64>)> = Vec::with_capacity(children);
            for _ in 0..children {
                let child = decode_geometry(cursor, depth + 1, max_depth, components)?;
                // Controllo per figlio, immediatamente dopo il suo decode:
                // stesso ordine del validatore strutturale.
                let compatible = match geometry_type {
                    4 => child.0 == 1,
                    5 => child.0 == 2,
                    6 => child.0 == 3,
                    _ => true,
                };
                if !compatible {
                    return Err(invalid_wkb_structure(
                        "tipo figlio incompatibile con multi-geometria",
                    ));
                }
                decoded.push(child);
            }
            let incompatible =
                || invalid_wkb_structure("tipo figlio incompatibile con multi-geometria");
            match geometry_type {
                4 => {
                    // Nessun `collect::<Result<Vec<_>>>`: la raccolta con
                    // Result lascia capacita' in eccesso (crescita a
                    // raddoppio) e la forma decodificata non sarebbe densa —
                    // invariante del preflight D14.4 (`decoded_size_xy`, il
                    // test di conservativita' misura le capacita' reali).
                    // Il tipo figlio e' gia' verificato per costruzione nel
                    // ciclo sopra: il ramo d'errore resta come difesa.
                    let mut points = Vec::with_capacity(decoded.len());
                    for (_, geometry) in decoded {
                        match geometry {
                            Geometry::Point(point) => points.push(point),
                            _ => return Err(incompatible()),
                        }
                    }
                    Ok((4, Geometry::MultiPoint(MultiPoint::new(points))))
                }
                5 => {
                    let mut lines = Vec::with_capacity(decoded.len());
                    for (_, geometry) in decoded {
                        match geometry {
                            Geometry::LineString(line) => lines.push(line),
                            _ => return Err(incompatible()),
                        }
                    }
                    Ok((5, Geometry::MultiLineString(MultiLineString::new(lines))))
                }
                6 => {
                    let mut polygons = Vec::with_capacity(decoded.len());
                    for (_, geometry) in decoded {
                        match geometry {
                            Geometry::Polygon(polygon) => polygons.push(polygon),
                            _ => return Err(incompatible()),
                        }
                    }
                    Ok((6, Geometry::MultiPolygon(MultiPolygon::new(polygons))))
                }
                _ => Ok((
                    7,
                    Geometry::GeometryCollection(GeometryCollection::new_from(
                        decoded.into_iter().map(|(_, geometry)| geometry).collect(),
                    )),
                )),
            }
        }
        _ => Err(invalid_wkb_structure("tipo geometria non supportato")),
    }
}

#[cfg(test)]
mod tests {
    use geozero::{wkb::Wkb, ToGeo};

    use super::*;
    use crate::validate_wkb_contract;

    /// Oracolo differenziale (architettura.md#geometrie): per ogni payload, il percorso
    /// precedente (`validate_wkb_contract` + `Wkb::to_geo`) e il decoder
    /// validante devono produrre lo stesso esito e, in caso di successo,
    /// la stessa geometria coordinata per coordinata.
    fn assert_parity(payload: &[u8], label: &str) {
        let reference = validate_wkb_contract(payload).and_then(|()| {
            Wkb(payload)
                .to_geo()
                .map_err(|error| PlenoraError::InvalidPlan(error.to_string()))
        });
        let decoded = decode_validated(payload);
        match (&reference, &decoded) {
            (Ok(expected), Ok(actual)) => {
                assert_eq!(expected, actual, "{label}: geometrie diverse");
            }
            (Err(_), Err(_)) => {}
            (Ok(_), Err(error)) => panic!("{label}: riferimento Ok, decoder Err: {error}"),
            (Err(error), Ok(_)) => panic!("{label}: riferimento Err ({error}), decoder Ok"),
        }
    }

    fn point_wkb_le(x: f64, y: f64) -> Vec<u8> {
        let mut payload = vec![1_u8];
        payload.extend_from_slice(&1_u32.to_le_bytes());
        payload.extend_from_slice(&x.to_le_bytes());
        payload.extend_from_slice(&y.to_le_bytes());
        payload
    }

    fn linestring_wkb_le(points: &[(f64, f64)]) -> Vec<u8> {
        let mut payload = vec![1_u8];
        payload.extend_from_slice(&2_u32.to_le_bytes());
        payload.extend_from_slice(
            &u32::try_from(points.len())
                .expect("fixture entro u32")
                .to_le_bytes(),
        );
        for (x, y) in points {
            payload.extend_from_slice(&x.to_le_bytes());
            payload.extend_from_slice(&y.to_le_bytes());
        }
        payload
    }

    fn polygon_wkb_le(ring: &[(f64, f64)]) -> Vec<u8> {
        let mut payload = vec![1_u8];
        payload.extend_from_slice(&3_u32.to_le_bytes());
        payload.extend_from_slice(&1_u32.to_le_bytes());
        payload.extend_from_slice(
            &u32::try_from(ring.len())
                .expect("fixture entro u32")
                .to_le_bytes(),
        );
        for (x, y) in ring {
            payload.extend_from_slice(&x.to_le_bytes());
            payload.extend_from_slice(&y.to_le_bytes());
        }
        payload
    }

    /// Poligono con anelli interni (la parita' multi-anello e' la classe del
    /// bug 2026-07-29: l'esterno andava perso con un secondo anello).
    fn polygon_with_interiors_wkb_le(
        exterior: &[(f64, f64)],
        interiors: &[&[(f64, f64)]],
    ) -> Vec<u8> {
        let mut payload = vec![1_u8];
        payload.extend_from_slice(&3_u32.to_le_bytes());
        payload.extend_from_slice(
            &u32::try_from(1 + interiors.len())
                .expect("fixture entro u32")
                .to_le_bytes(),
        );
        for ring in std::iter::once(exterior).chain(interiors.iter().copied()) {
            payload.extend_from_slice(
                &u32::try_from(ring.len())
                    .expect("fixture entro u32")
                    .to_le_bytes(),
            );
            for (x, y) in ring {
                payload.extend_from_slice(&x.to_le_bytes());
                payload.extend_from_slice(&y.to_le_bytes());
            }
        }
        payload
    }

    fn multipolygon_wkb_le(polygons: &[Vec<u8>]) -> Vec<u8> {
        let mut payload = vec![1_u8];
        payload.extend_from_slice(&6_u32.to_le_bytes());
        payload.extend_from_slice(
            &u32::try_from(polygons.len())
                .expect("fixture entro u32")
                .to_le_bytes(),
        );
        for polygon in polygons {
            payload.extend_from_slice(polygon);
        }
        payload
    }

    fn multipoint_wkb_le(points: &[(f64, f64)]) -> Vec<u8> {
        let mut payload = vec![1_u8];
        payload.extend_from_slice(&4_u32.to_le_bytes());
        payload.extend_from_slice(
            &u32::try_from(points.len())
                .expect("fixture entro u32")
                .to_le_bytes(),
        );
        for (x, y) in points {
            payload.extend(point_wkb_le(*x, *y));
        }
        payload
    }

    fn collection_wkb_le(parts: &[Vec<u8>]) -> Vec<u8> {
        let mut payload = vec![1_u8];
        payload.extend_from_slice(&7_u32.to_le_bytes());
        payload.extend_from_slice(
            &u32::try_from(parts.len())
                .expect("fixture entro u32")
                .to_le_bytes(),
        );
        for part in parts {
            payload.extend_from_slice(part);
        }
        payload
    }

    #[test]
    fn parity_on_valid_geometries() {
        assert_parity(&point_wkb_le(1.5, -2.5), "point");
        assert_parity(
            &linestring_wkb_le(&[(0.0, 0.0), (1.0, 1.0), (2.0, 0.5)]),
            "linestring",
        );
        assert_parity(
            &polygon_wkb_le(&[(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 0.0)]),
            "polygon aperto in coda chiuso",
        );
        assert_parity(&multipoint_wkb_le(&[(0.0, 0.0), (3.0, 4.0)]), "multipoint");
        assert_parity(
            &collection_wkb_le(&[
                point_wkb_le(0.0, 0.0),
                linestring_wkb_le(&[(0.0, 0.0), (5.0, 5.0)]),
                polygon_wkb_le(&[(0.0, 0.0), (2.0, 0.0), (2.0, 2.0), (0.0, 0.0)]),
            ]),
            "geometrycollection annidata",
        );
        // Poligoni multi-anello (classe del bug 2026-07-29: l'oracolo
        // copriva solo poligoni a un anello e l'esterno andava perso).
        let exterior = [
            (0.0, 0.0),
            (10.0, 0.0),
            (10.0, 10.0),
            (0.0, 10.0),
            (0.0, 0.0),
        ];
        let hole_a = [(2.0, 2.0), (4.0, 2.0), (2.0, 4.0), (2.0, 2.0)];
        let hole_b = [(6.0, 6.0), (8.0, 6.0), (6.0, 8.0), (6.0, 6.0)];
        assert_parity(
            &polygon_with_interiors_wkb_le(&exterior, &[&hole_a]),
            "polygon con un anello interno",
        );
        assert_parity(
            &polygon_with_interiors_wkb_le(&exterior, &[&hole_a, &hole_b]),
            "polygon con due anelli interni",
        );
        assert_parity(
            &multipolygon_wkb_le(&[
                polygon_wkb_le(&[(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 0.0)]),
                polygon_with_interiors_wkb_le(&exterior, &[&hole_a]),
            ]),
            "multipolygon con figlio con anello interno",
        );
        assert_parity(
            &collection_wkb_le(&[polygon_with_interiors_wkb_le(&exterior, &[&hole_a])]),
            "collection con polygon con anello interno",
        );
        // Big-endian.
        let mut big = vec![0_u8];
        big.extend_from_slice(&1_u32.to_be_bytes());
        big.extend_from_slice(&3.25_f64.to_be_bytes());
        big.extend_from_slice(&(-7.5_f64).to_be_bytes());
        assert_parity(&big, "point big-endian");
    }

    #[test]
    fn parity_on_rejected_payloads() {
        assert_parity(&[], "vuoto");
        assert_parity(&[1, 2, 3], "troncato");
        assert_parity(&point_wkb_le(f64::NAN, 0.0), "NaN in X");
        assert_parity(&point_wkb_le(0.0, f64::INFINITY), "inf in Y");
        assert_parity(&linestring_wkb_le(&[(0.0, 0.0)]), "linestring da un punto");
        assert_parity(
            &polygon_wkb_le(&[(0.0, 0.0), (1.0, 0.0), (1.0, 1.0)]),
            "anello con 3 coordinate",
        );
        assert_parity(
            &polygon_wkb_le(&[(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (9.0, 9.0)]),
            "anello non chiuso",
        );
        {
            let mut payload = point_wkb_le(0.0, 0.0);
            payload.push(0);
            assert_parity(&payload, "byte residui");
        }
        {
            let mut payload = vec![1_u8];
            payload.extend_from_slice(&1001_u32.to_le_bytes());
            payload.extend_from_slice(&1.0_f64.to_le_bytes());
            payload.extend_from_slice(&2.0_f64.to_le_bytes());
            payload.extend_from_slice(&3.0_f64.to_le_bytes());
            assert_parity(&payload, "serie Z rifiutata (protocollo 2D)");
        }
        {
            let mut payload = vec![1_u8];
            payload.extend_from_slice(&(1_u32 | 0x2000_0000).to_le_bytes());
            payload.extend_from_slice(&32632_u32.to_le_bytes());
            payload.extend_from_slice(&1.0_f64.to_le_bytes());
            payload.extend_from_slice(&2.0_f64.to_le_bytes());
            assert_parity(&payload, "SRID EWKB rifiutato");
        }
        // Annidamento oltre il limite.
        let mut nested = point_wkb_le(0.0, 0.0);
        for _ in 0..70 {
            nested = collection_wkb_le(&[nested]);
        }
        assert_parity(&nested, "annidamento oltre 64");
        // Tipo figlio incompatibile nella multi.
        {
            let mut payload = vec![1_u8];
            payload.extend_from_slice(&4_u32.to_le_bytes());
            payload.extend_from_slice(&1_u32.to_le_bytes());
            payload.extend_from_slice(&linestring_wkb_le(&[(0.0, 0.0), (1.0, 1.0)]));
            assert_parity(&payload, "multipoint con figlio linestring");
        }
    }

    #[test]
    fn empty_linestring_and_multipoint_match_reference() {
        assert_parity(&linestring_wkb_le(&[]), "linestring vuota");
        assert_parity(&multipoint_wkb_le(&[]), "multipoint vuota");
        assert_parity(&collection_wkb_le(&[]), "collection vuota");
    }
}
