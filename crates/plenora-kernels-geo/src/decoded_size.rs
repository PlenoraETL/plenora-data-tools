//! Preflight della forma decodificata (architettura.md#geometrie D14.4).
//!
//! [`decoded_size_xy`] calcola la dimensione in byte della geometria
//! `Geometry<f64>` che il decoder validante ([`crate::wkb_decoder`])
//! costruirebbe da una cella WKB, SENZA decodificarla — la reservation del
//! governor avviene prima dell'allocazione (R7 nell'ordine giusto: riservare
//! prima di decodificare, rifiutare prima di allocare).
//!
//! La camminata e' speculare a quella del decoder (`decode_geometry`): stesso
//! ordine di valutazione (byte order, type code, conteggi contro i byte
//! residui, profondita', componenti), stessi limiti ([`MAX_WKB_BYTES`],
//! [`MAX_WKB_DEPTH`], [`MAX_WKB_COMPONENTS`]) — ma salta i VALORI delle
//! coordinate (la finitezza e' un controllo del decoder, irrilevante per la
//! misura) e accumula byte invece di costruire. I controlli sono un
//! sottoinsieme di quelli del decoder nello stesso ordine: se il preflight
//! fallisce su una cella, il decode fallisce sulla stessa cella — il
//! chiamante usa l'esito solo per dimensionare e delega SEMPRE al decode il
//! rapporto d'errore canonico (una sola fonte di verita', D14.3).
//!
//! # Modello di memoria (dichiarato)
//!
//! La formula conta il layout fisico denso della forma decodificata
//! (`Vec` con capacita' esatta, come li produce il decoder — vedi il test di
//! conservativita' di questo modulo):
//!
//! - ogni valore `Geometry<f64>` occupa [`GEOMETRY_BYTES`] (l'enum intero,
//!   qualunque sia la variante: e' il costo di uno slot in
//!   `Vec<Geometry<f64>>` dei figli di una collection);
//! - lo slot radice `Option<Geometry<f64>>` della colonna decodificata e'
//!   [`OPTION_SLOT_BYTES`] per riga (contato dal chiamante per cella, null
//!   incluse);
//! - heap per sequenza: [`COORD_BYTES`] per coordinata (`Vec<Coord<f64>>` di
//!   linestring e anelli, punti inline di una `MultiPoint`),
//!   [`POINT_BYTES`] per figlio di `MultiPoint`, [`LINESTRING_BYTES`] per
//!   anello interno e per figlio di `MultiLineString`, [`POLYGON_BYTES`] per
//!   figlio di `MultiPolygon`, [`GEOMETRY_BYTES`] per figlio di collection
//!   (piu' la ricorsione sul figlio).
//!
//! Deviazione dichiarata rispetto allo schizzo dell'ADR (`16·n_coord` +
//! `24·n_vec` + `8·n_enum` + 24 per slot): quelle costanti NON coprono il layout
//! fisico — un valore `Geometry<f64>` costa `size_of::<Geometry<f64>>()`
//! qualunque sia la variante (8 di enum + 16 di coordinata non pagano i 56
//! byte dello slot), e la stima non sarebbe conservativa. Le costanti qui
//! sono i `size_of` reali, verificate per costruzione dal test di
//! conservativita' su corpus multi-tipo (stima ≥ memoria reale misurata —
//! stesso ruolo del test errori-e-limiti.md#limiti-dichiarati). Lo slack di allocazione (`Vec` con
//! capacita' > lunghezza) e' fuori modello per costruzione: il decoder
//! alloca ogni `Vec` con capacita' esatta (`with_capacity`), invariante
//! verificata dal test.

use geo::Geometry;

use plenora_core::PlenoraError;

use crate::{
    checked_count, invalid_wkb_structure, parse_wkb_type_code, GeometryDimensions, WkbCursor,
    MAX_WKB_BYTES, MAX_WKB_COMPONENTS, MAX_WKB_DEPTH,
};

/// Byte di una coordinata XY decodificata (`size_of::<Coord<f64>>()`).
pub const COORD_BYTES: u64 = size_of::<geo::Coord<f64>>() as u64;
/// Byte di un `Point<f64>` inline (`size_of`, figli di `MultiPoint`).
pub const POINT_BYTES: u64 = size_of::<geo::Point<f64>>() as u64;
/// Byte di una `LineString<f64>` inline (`size_of`, anelli interni e figli
/// di `MultiLineString`).
pub const LINESTRING_BYTES: u64 = size_of::<geo::LineString<f64>>() as u64;
/// Byte di un `Polygon<f64>` inline (`size_of`, figli di `MultiPolygon`).
pub const POLYGON_BYTES: u64 = size_of::<geo::Polygon<f64>>() as u64;
/// Byte di un valore `Geometry<f64>` (`size_of` dell'enum intero: figli di
/// `GeometryCollection`).
pub const GEOMETRY_BYTES: u64 = size_of::<Geometry<f64>>() as u64;
/// Byte dello slot `Option<Geometry<f64>>` nella colonna decodificata
/// (`size_of`; contato per riga, null inclusi).
pub const OPTION_SLOT_BYTES: u64 = size_of::<Option<Geometry<f64>>>() as u64;

/// Dimensione in byte della forma decodificata di una cella WKB (architettura.md#geometrie
/// D14.4).
///
/// Heap della `Geometry<f64>` che il decoder validante costruirebbe, SENZA
/// lo slot `Option` radice (contato dal chiamante via
/// [`OPTION_SLOT_BYTES`]). Funzione pura della struttura: nessuna
/// serializzazione, nessuna decodifica dei valori.
///
/// # Errors
///
/// Come [`crate::wkb_decoder::decode_validated`] per il sottoinsieme di
/// controlli strutturali necessario alla camminata (limiti di byte,
/// profondita', componenti, conteggi contro i byte residui, type code).
/// L'esito d'errore NON e' destinato al rapporto canonico: il chiamante lo
/// tratta come "stima interrotta" e lascia al decode validante l'errore
/// definitivo sulla stessa cella (D14.3, una sola fonte di verita').
pub fn decoded_size_xy(payload: &[u8]) -> Result<u64, PlenoraError> {
    if payload.len() > MAX_WKB_BYTES {
        return Err(invalid_wkb_structure("WKB oltre il limite di 64 MiB"));
    }
    let mut cursor = WkbCursor::new(payload);
    let mut components = 0_u64;
    let mut total = 0_u64;
    decoded_size_geometry(&mut cursor, 0, MAX_WKB_DEPTH, &mut components, &mut total)?;
    if cursor.remaining() != 0 {
        return Err(invalid_wkb_structure("byte residui dopo la geometria"));
    }
    Ok(total)
}

/// Camminata speculare a `wkb_decoder::decode_geometry`: stesso ordine di
/// valutazione e stessi controlli strutturali (sottoinsieme: i valori delle
/// coordinate sono saltati via stride, mai letti), con accumulo dei byte
/// della forma decodificata al posto della costruzione. Restituisce il type
/// code della geometria (per il controllo di compatibilita' dei figli,
/// nello stesso punto del decoder).
// Dispatcher esaustivo per tipo geometrico WKB, speculare al decoder riga
// per riga (stesso criterio di `decode_geometry`): spezzarlo peggiorerebbe
// il confronto con la camminata di riferimento.
#[allow(clippy::too_many_lines)]
fn decoded_size_geometry(
    cursor: &mut WkbCursor<'_>,
    depth: usize,
    max_depth: usize,
    components: &mut u64,
    total: &mut u64,
) -> Result<u32, PlenoraError> {
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
        // Point: coordinata inline nell'enum, nessun heap.
        1 => {
            cursor.skip(stride)?;
            Ok(1)
        }
        2 => {
            let count = cursor.read_u32(little_endian)?;
            let count = checked_count(count, cursor.remaining(), stride)?;
            if count == 1 {
                return Err(invalid_wkb_structure(
                    "LineString deve essere vuota o avere almeno due coordinate",
                ));
            }
            cursor.skip(count * stride)?;
            *total = total.saturating_add(COORD_BYTES.saturating_mul(count as u64));
            Ok(2)
        }
        3 => {
            let rings = cursor.read_u32(little_endian)?;
            let rings = checked_count(rings, cursor.remaining(), 4)?;
            for ring_index in 0..rings {
                let count = cursor.read_u32(little_endian)?;
                let count = checked_count(count, cursor.remaining(), stride)?;
                cursor.skip(count * stride)?;
                *total = total.saturating_add(COORD_BYTES.saturating_mul(count as u64));
                // Anelli interni: header `LineString` nel buffer degli
                // interni (l'esterno e' inline nel `Polygon`).
                if ring_index > 0 {
                    *total = total.saturating_add(LINESTRING_BYTES);
                }
            }
            Ok(3)
        }
        4..=7 => {
            let children = cursor.read_u32(little_endian)?;
            let children = checked_count(children, cursor.remaining(), 5)?;
            for _ in 0..children {
                let child_type =
                    decoded_size_geometry(cursor, depth + 1, max_depth, components, total)?;
                // Controllo per figlio, immediatamente dopo la sua
                // camminata: stesso ordine del decoder.
                let compatible = match geometry_type {
                    4 => child_type == 1,
                    5 => child_type == 2,
                    6 => child_type == 3,
                    _ => true,
                };
                if !compatible {
                    return Err(invalid_wkb_structure(
                        "tipo figlio incompatibile con multi-geometria",
                    ));
                }
                // Slot del figlio nel buffer del contenitore (inline).
                let slot = match geometry_type {
                    4 => POINT_BYTES,
                    5 => LINESTRING_BYTES,
                    6 => POLYGON_BYTES,
                    _ => GEOMETRY_BYTES,
                };
                *total = total.saturating_add(slot);
            }
            Ok(geometry_type)
        }
        _ => Err(invalid_wkb_structure("tipo geometria non supportato")),
    }
}

#[cfg(test)]
mod tests {
    use geo::{
        Coord, GeometryCollection, LineString, MultiLineString, MultiPoint, MultiPolygon, Point,
        Polygon,
    };
    use geozero::{CoordDimensions, ToWkb};

    use super::*;
    use crate::geometry_from_wkb;

    /// Le costanti della formula SONO i `size_of` del layout fisico: se il
    /// layout di `geo` cambia, questo test fallisce prima di qualunque
    /// conservativita' — la formula va riletta, mai aggiustata in silenzio.
    #[test]
    fn constants_are_the_physical_sizeof() {
        assert_eq!(COORD_BYTES, 16);
        assert_eq!(POINT_BYTES, 16);
        assert_eq!(LINESTRING_BYTES, 24);
        assert_eq!(POLYGON_BYTES, 48);
        assert_eq!(GEOMETRY_BYTES, 56);
        assert_eq!(OPTION_SLOT_BYTES, 56);
    }

    /// Memoria reale della forma decodificata, misurata sulle capacita'
    /// effettive dei `Vec` (non sulla formula): e' l'oracolo indipendente
    /// della conservativita'. Copre lo heap di una geometria radice; lo
    /// slot `Option` e' contato dal chiamante del preflight.
    fn measured_heap(geometry: &Geometry<f64>) -> u64 {
        let vec_heap = |capacity: usize, element: u64| (capacity as u64) * element;
        match geometry {
            Geometry::Point(_) | Geometry::Line(_) | Geometry::Rect(_) | Geometry::Triangle(_) => 0,
            Geometry::LineString(line) => vec_heap(line.0.capacity(), COORD_BYTES),
            Geometry::Polygon(polygon) => polygon_heap(polygon),
            Geometry::MultiPoint(multi) => vec_heap(multi.0.capacity(), POINT_BYTES),
            Geometry::MultiLineString(multi) => {
                vec_heap(multi.0.capacity(), LINESTRING_BYTES)
                    + multi
                        .0
                        .iter()
                        .map(|line| vec_heap(line.0.capacity(), COORD_BYTES))
                        .sum::<u64>()
            }
            Geometry::MultiPolygon(multi) => {
                vec_heap(multi.0.capacity(), POLYGON_BYTES)
                    + multi.0.iter().map(polygon_heap).sum::<u64>()
            }
            Geometry::GeometryCollection(collection) => {
                vec_heap(collection.0.capacity(), GEOMETRY_BYTES)
                    + collection.0.iter().map(measured_heap).sum::<u64>()
            }
        }
    }

    fn polygon_heap(polygon: &Polygon<f64>) -> u64 {
        let exterior = polygon.exterior().0.capacity() as u64 * COORD_BYTES;
        // `interiors()` e' esposta come slice: la capacita' non e'
        // osservabile senza unsafe. Si misura a lunghezza — esatta per
        // costruzione: il decoder alloca il buffer degli interni con
        // `with_capacity(rings - 1)` (invariante dichiarato in
        // `wkb_decoder`, unico `Vec` non osservabile del percorso).
        let interiors_buffer = polygon.interiors().len() as u64 * LINESTRING_BYTES;
        let interiors = polygon
            .interiors()
            .iter()
            .map(|ring| ring.0.capacity() as u64 * COORD_BYTES)
            .sum::<u64>();
        exterior + interiors_buffer + interiors
    }

    fn to_wkb(geometry: &Geometry<f64>) -> Vec<u8> {
        geometry
            .to_wkb(CoordDimensions::xy())
            .expect("encode fixture")
    }

    /// Corpus multi-tipo (stessa batteria di `geometry_contract`): punti,
    /// linee, poligoni con e senza buchi, multi-*, collection annidate,
    /// vuote.
    fn corpus() -> Vec<(&'static str, Geometry<f64>)> {
        let triangle = Geometry::Polygon(Polygon::new(
            LineString::from(vec![(0.0, 0.0), (4.0, 0.0), (2.0, 3.0), (0.0, 0.0)]),
            Vec::new(),
        ));
        let holed = Geometry::Polygon(Polygon::new(
            LineString::from(vec![
                (0.0, 0.0),
                (10.0, 0.0),
                (10.0, 10.0),
                (0.0, 10.0),
                (0.0, 0.0),
            ]),
            vec![LineString::from(vec![
                (2.0, 2.0),
                (4.0, 2.0),
                (2.0, 4.0),
                (2.0, 2.0),
            ])],
        ));
        vec![
            ("point", Geometry::Point(Point::new(1.5, -2.5))),
            (
                "linestring",
                Geometry::LineString(LineString::from(vec![(0.0, 0.0), (1.0, 1.0), (2.0, 0.5)])),
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
                    LineString::from(vec![(0.0, 0.0), (1.0, 1.0)]),
                    LineString::from(vec![(2.0, 2.0), (3.0, 3.0), (4.0, 2.0)]),
                ])),
            ),
            (
                "multipolygon",
                Geometry::MultiPolygon(MultiPolygon::new(vec![
                    Polygon::new(
                        LineString::from(vec![(0.0, 0.0), (5.0, 0.0), (5.0, 5.0), (0.0, 0.0)]),
                        Vec::new(),
                    ),
                    Polygon::new(
                        LineString::from(vec![
                            (10.0, 10.0),
                            (17.0, 10.0),
                            (17.0, 17.0),
                            (10.0, 10.0),
                        ]),
                        Vec::new(),
                    ),
                ])),
            ),
            (
                "collection annidata",
                Geometry::GeometryCollection(GeometryCollection::new_from(vec![
                    Geometry::Point(Point::new(0.0, 0.0)),
                    Geometry::GeometryCollection(GeometryCollection::new_from(vec![
                        Geometry::LineString(LineString::from(vec![(0.0, 0.0), (5.0, 5.0)])),
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

    /// Conservativita' (architettura.md#geometrie D14.4, ruolo del test errori-e-limiti.md#limiti-dichiarati): su tutto il
    /// corpus la stima del preflight e' >= la memoria reale misurata sulla
    /// geometria decodificata dal decoder validante — e qui esattamente
    /// uguale, perche' il decoder alloca ogni `Vec` con capacita' esatta
    /// (invariante che questa uguaglianza stessa presidia: uno slack di
    /// capacita' la violerebbe).
    #[test]
    fn estimate_is_conservative_on_the_multi_type_corpus() {
        for (label, geometry) in corpus() {
            let payload = to_wkb(&geometry);
            let estimate = decoded_size_xy(&payload).expect("preflight fixture");
            let decoded = geometry_from_wkb(&payload).expect("decode fixture");
            let measured = measured_heap(&decoded);
            assert!(
                estimate >= measured,
                "{label}: stima {estimate} sotto la memoria reale {measured}"
            );
            assert_eq!(
                estimate, measured,
                "{label}: la forma decodificata dal decoder non e' densa (capacita' > lunghezza)"
            );
        }
    }

    /// Lo slot radice e' contato a parte: per cella la somma slot + heap
    /// copre il costo completo della cella decodificata.
    #[test]
    fn option_slot_plus_heap_covers_the_full_cell() {
        for (label, geometry) in corpus() {
            let payload = to_wkb(&geometry);
            let total = OPTION_SLOT_BYTES + decoded_size_xy(&payload).expect("preflight fixture");
            let decoded = geometry_from_wkb(&payload).expect("decode fixture");
            let measured = OPTION_SLOT_BYTES + measured_heap(&decoded);
            assert_eq!(total, measured, "{label}: costo cella non esatto");
        }
    }

    /// Coordinate NaN: il preflight salta i valori (misura comunque); il
    /// decode le rifiuta — l'errore canonico resta del decoder.
    #[test]
    fn non_finite_coordinates_are_sized_and_left_to_the_decoder() {
        let mut payload = vec![1_u8];
        payload.extend_from_slice(&1_u32.to_le_bytes());
        payload.extend_from_slice(&f64::NAN.to_le_bytes());
        payload.extend_from_slice(&0.0_f64.to_le_bytes());
        assert_eq!(decoded_size_xy(&payload).expect("preflight"), 0);
        assert!(geometry_from_wkb(&payload).is_err());
    }

    /// Errori strutturali: il preflight fallisce sulle stesse celle del
    /// decoder (sottoinsieme di controlli, stesso ordine) — il chiamante lo
    /// usa solo per interrompere la stima.
    #[test]
    fn structural_errors_match_the_decoder_on_the_same_cells() {
        let cases: Vec<Vec<u8>> = vec![
            Vec::new(), // byte mancante
            vec![1, 2], // type code troncato
            vec![2],    // byte order non valido
            {
                // linestring da una coordinata
                let mut p = vec![1_u8];
                p.extend_from_slice(&2_u32.to_le_bytes());
                p.extend_from_slice(&1_u32.to_le_bytes());
                p.extend_from_slice(&[0_u8; 16]);
                p
            },
            {
                // conteggio oltre i byte disponibili
                let mut p = vec![1_u8];
                p.extend_from_slice(&2_u32.to_le_bytes());
                p.extend_from_slice(&1_000_u32.to_le_bytes());
                p
            },
            {
                // figlio incompatibile in multipoint (una linestring)
                let mut p = vec![1_u8];
                p.extend_from_slice(&4_u32.to_le_bytes());
                p.extend_from_slice(&1_u32.to_le_bytes());
                p.extend_from_slice(&[1_u8]);
                p.extend_from_slice(&2_u32.to_le_bytes());
                p.extend_from_slice(&0_u32.to_le_bytes());
                p
            },
        ];
        for (index, payload) in cases.iter().enumerate() {
            assert!(
                decoded_size_xy(payload).is_err(),
                "caso {index}: preflight accetta una cella che il decoder rifiuta"
            );
            assert!(
                crate::wkb_decoder::decode_validated(payload).is_err(),
                "caso {index}: decoder"
            );
        }
    }

    /// Profondita' oltre il limite: come il decoder.
    #[test]
    fn nesting_beyond_the_depth_limit_is_rejected() {
        let mut geometry = Geometry::Point(Point::new(0.0, 0.0));
        for _ in 0..(MAX_WKB_DEPTH + 6) {
            geometry = Geometry::GeometryCollection(GeometryCollection::new_from(vec![geometry]));
        }
        let payload = to_wkb(&geometry);
        assert!(decoded_size_xy(&payload).is_err());
        assert!(crate::wkb_decoder::decode_validated(&payload).is_err());
    }

    /// Big-endian: la camminata segue il byte order dichiarato dalla cella.
    #[test]
    fn big_endian_cells_are_walked() {
        let mut payload = vec![0_u8];
        payload.extend_from_slice(&2_u32.to_be_bytes());
        payload.extend_from_slice(&2_u32.to_be_bytes());
        for (x, y) in [(1.0_f64, 2.0_f64), (3.0, 4.0)] {
            payload.extend_from_slice(&x.to_be_bytes());
            payload.extend_from_slice(&y.to_be_bytes());
        }
        assert_eq!(
            decoded_size_xy(&payload).expect("big-endian"),
            2 * COORD_BYTES
        );
    }

    /// Fixture usata dai test di conservativita' con un tipo composto
    /// costruito a mano (Coord non coperto altrove).
    #[test]
    fn coord_sizeof_is_two_f64() {
        assert_eq!(size_of::<Coord<f64>>() as u64, COORD_BYTES);
    }
}
