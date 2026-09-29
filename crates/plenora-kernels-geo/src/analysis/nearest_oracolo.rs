//! Oracolo di `geo.nearest`: la forza bruta O(n*m) di prima dell'indice
//! R-tree, copiata alla lettera, confrontata input per input con
//! [`nearest_matches`] e [`nearest_matches_validated`]. Il confronto e' sui
//! bit delle distanze (`to_bits`) e sul testo degli errori. Dove la forza
//! bruta andava in panico dentro `geo`, il contratto chiede
//! `CalcoloNonConcluso`: il percorso pubblico non deve mai andare in panico.

// Le coordinate di prova sono scritte come `a + k * b`: `mul_add` le
// renderebbe meno leggibili senza cambiare che cosa si prova.
#![allow(clippy::suboptimal_flops)]

use super::*;
use geo::{
    Coord, GeometryCollection, Line, MultiLineString, MultiPoint, MultiPolygon, Point, Rect,
    Triangle,
};

// --- oracolo: copia letterale di `nearest_matches_impl` a main a44d58b ------

#[allow(clippy::too_many_lines)]
fn forza_bruta(
    left: &[Option<Geometry<f64>>],
    right: &[Option<Geometry<f64>>],
    max_distance: Option<f64>,
    max_comparisons: u64,
    max_results: u64,
    validated: bool,
) -> Result<Vec<NearestMatch>, AnalysisError> {
    if max_comparisons == 0 || max_results == 0 {
        return Err(AnalysisError::InvalidWorkLimit);
    }
    if max_distance.is_some_and(|value| !value.is_finite() || value < 0.0) {
        return Err(AnalysisError::InvalidMaximumDistance);
    }
    if !validated {
        validate_geometries(left, "left")?;
        validate_geometries(right, "right")?;
    }
    let usable_right: Vec<_> = right
        .iter()
        .enumerate()
        .filter_map(|(index, geometry)| {
            geometry
                .as_ref()
                .filter(|value| value.coords_count() > 0)
                .map(|value| (index, value))
        })
        .collect();
    let comparisons = u64::try_from(left.iter().flatten().count())
        .map_err(|_| AnalysisError::IndexOverflow)?
        .checked_mul(u64::try_from(usable_right.len()).map_err(|_| AnalysisError::IndexOverflow)?)
        .ok_or(AnalysisError::WorkLimitExceeded {
            limit: max_comparisons,
        })?;
    if comparisons > max_comparisons {
        return Err(AnalysisError::WorkLimitExceeded {
            limit: max_comparisons,
        });
    }

    let result_count = AtomicU64::new(0);
    // architettura.md#determinismo: i `Result` sono raccolti per riga (ordine preservato) e il
    // primo errore IN ORDINE DI RIGA e' selezionato dal collect
    // sequenziale — il collect parallelo diretto sarebbe non deterministico.
    let groups: Vec<Result<Vec<NearestMatch>, AnalysisError>> = left
        .par_iter()
        .enumerate()
        .map(|(left_index, geometry)| {
            let Some(geometry) = geometry.as_ref().filter(|value| value.coords_count() > 0) else {
                return Ok(Vec::new());
            };
            let mut distances: Vec<_> = usable_right
                .iter()
                .map(|(right_index, right)| (*right_index, Euclidean.distance(geometry, *right)))
                .collect();
            let Some(minimum) = distances
                .iter()
                .map(|(_, distance)| *distance)
                .reduce(f64::min)
            else {
                return Ok(Vec::new());
            };
            if max_distance.is_some_and(|limit| minimum > limit) {
                return Ok(Vec::new());
            }
            // Uguaglianza esatta corretta per costruzione: `minimum` e' il
            // minimo degli stessi valori (reduce(f64::min)), non una stima.
            #[allow(clippy::float_cmp)]
            distances.retain(|(_, distance)| *distance == minimum);
            distances.sort_unstable_by_key(|(right_index, _)| *right_index);
            let additional =
                u64::try_from(distances.len()).map_err(|_| AnalysisError::IndexOverflow)?;
            result_count
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                    current
                        .checked_add(additional)
                        .filter(|next| *next <= max_results)
                })
                .map_err(|_| AnalysisError::ResultLimitExceeded { limit: max_results })?;
            let left = u64::try_from(left_index).map_err(|_| AnalysisError::IndexOverflow)?;
            distances
                .into_iter()
                .map(|(right_index, distance)| {
                    Ok(NearestMatch {
                        left,
                        right: u64::try_from(right_index)
                            .map_err(|_| AnalysisError::IndexOverflow)?,
                        distance,
                    })
                })
                .collect()
        })
        .collect();
    let grouped: Result<Vec<Vec<NearestMatch>>, AnalysisError> = groups.into_iter().collect();
    Ok(grouped?.into_iter().flatten().collect())
}

// --- confronto ---------------------------------------------------------------

type Colonna = Vec<Option<Geometry<f64>>>;
type Impronta = Result<Vec<(u64, u64, u64)>, String>;

/// Impronta dell'oracolo: un suo panico (per esempio
/// `nearest_neighbour_distance` di `geo` su una linea di un solo punto) e' il
/// caso in cui il contratto chiede `CalcoloNonConcluso`.
fn impronta_oracolo(esegui: impl FnOnce() -> Result<Vec<NearestMatch>, AnalysisError>) -> Impronta {
    let Ok(esito) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(esegui)) else {
        return Err(CALCOLO_NON_CONCLUSO.to_owned());
    };
    impronta(esito)
}

/// La variante senza la forma del payload, che dipende da `geo`.
const CALCOLO_NON_CONCLUSO: &str = "CalcoloNonConcluso";

/// Impronta del percorso pubblico: nessun `catch_unwind`, un panico fa
/// fallire il test.
fn impronta(esito: Result<Vec<NearestMatch>, AnalysisError>) -> Impronta {
    esito
        .map(|righe| {
            righe
                .into_iter()
                .map(|riga| (riga.left, riga.right, riga.distance.to_bits()))
                .collect()
        })
        .map_err(|errore| match errore {
            AnalysisError::CalcoloNonConcluso(_) => CALCOLO_NON_CONCLUSO.to_owned(),
            altro => format!("{altro:?}"),
        })
}

/// Confronta i due percorsi pubblici con l'oracolo; restituisce l'esito
/// dell'oracolo del percorso validated per i controlli di copertura.
fn confronta_ed_esito(
    left: &[Option<Geometry<f64>>],
    right: &[Option<Geometry<f64>>],
    max_distance: Option<f64>,
    max_comparisons: u64,
    max_results: u64,
) -> Impronta {
    let atteso_gated = impronta_oracolo(|| {
        forza_bruta(
            left,
            right,
            max_distance,
            max_comparisons,
            max_results,
            false,
        )
    });
    let ottenuto_gated = impronta(nearest_matches(
        left,
        right,
        max_distance,
        max_comparisons,
        max_results,
    ));
    assert_eq!(ottenuto_gated, atteso_gated, "percorso gated");
    let atteso = impronta_oracolo(|| {
        forza_bruta(
            left,
            right,
            max_distance,
            max_comparisons,
            max_results,
            true,
        )
    });
    let ottenuto = impronta(nearest_matches_validated(
        left,
        right,
        max_distance,
        max_comparisons,
        max_results,
    ));
    assert_eq!(ottenuto, atteso, "percorso validated");
    atteso
}

fn confronta(
    left: &[Option<Geometry<f64>>],
    right: &[Option<Geometry<f64>>],
    max_distance: Option<f64>,
    max_comparisons: u64,
    max_results: u64,
) {
    let _ = confronta_ed_esito(left, right, max_distance, max_comparisons, max_results);
}

/// Confronto con limiti larghi e con le soglie di distanza piu' taglienti:
/// il minimo esatto di ogni riga, il suo predecessore e zero.
fn confronta_con_soglie(left: &[Option<Geometry<f64>>], right: &[Option<Geometry<f64>>]) {
    let esito = confronta_ed_esito(left, right, None, u64::MAX, u64::MAX);
    confronta(left, right, Some(0.0), u64::MAX, u64::MAX);
    let Ok(trovate) = esito else {
        return;
    };
    let mut soglie: Vec<u64> = trovate.iter().map(|riga| riga.2).collect();
    soglie.sort_unstable();
    soglie.dedup();
    for bits in soglie.into_iter().take(6) {
        let minimo = f64::from_bits(bits);
        if minimo.is_finite() {
            confronta(left, right, Some(minimo), u64::MAX, u64::MAX);
            confronta(
                left,
                right,
                Some(minimo.next_down().max(0.0)),
                u64::MAX,
                u64::MAX,
            );
        }
    }
    // Limite sui risultati: esatto e uno sotto.
    let totale = u64::try_from(trovate.len()).unwrap();
    if totale > 0 {
        confronta(left, right, None, u64::MAX, totale);
        if totale > 1 {
            confronta(left, right, None, u64::MAX, totale - 1);
        }
    }
    // Limite sui confronti: esatto e uno sotto, come prima (n * m).
    let non_nulli = u64::try_from(left.iter().flatten().count()).unwrap();
    let usabili = u64::try_from(
        right
            .iter()
            .flatten()
            .filter(|geometria| geometria.coords_count() > 0)
            .count(),
    )
    .unwrap();
    let confronti = non_nulli * usabili;
    if confronti > 0 {
        confronta(left, right, None, confronti, u64::MAX);
        if confronti > 1 {
            confronta(left, right, None, confronti - 1, u64::MAX);
        }
    }
}

// --- costruttori ---------------------------------------------------------------

// L'Option serve a comporre colonne con null.
#[allow(clippy::unnecessary_wraps)]
fn punto(x: f64, y: f64) -> Option<Geometry<f64>> {
    Some(Geometry::Point(Point::new(x, y)))
}

fn linea(coordinate: &[(f64, f64)]) -> LineString<f64> {
    LineString::from(coordinate.to_vec())
}

fn rettangolo(x0: f64, y0: f64, x1: f64, y1: f64) -> Polygon<f64> {
    Polygon::new(
        linea(&[(x0, y0), (x1, y0), (x1, y1), (x0, y1), (x0, y0)]),
        vec![],
    )
}

fn con_buco(esterno: (f64, f64, f64, f64), buco: (f64, f64, f64, f64)) -> Polygon<f64> {
    let (x0, y0, x1, y1) = buco;
    Polygon::new(
        rettangolo(esterno.0, esterno.1, esterno.2, esterno.3)
            .exterior()
            .clone(),
        vec![linea(&[(x0, y0), (x0, y1), (x1, y1), (x1, y0), (x0, y0)])],
    )
}

fn poligono_vuoto() -> Polygon<f64> {
    Polygon::new(LineString::new(vec![]), vec![])
}

// --- casi avversari deterministici -----------------------------------------------

#[test]
fn griglia_di_pari_equidistanti() {
    let right: Colonna = (0..=10)
        .flat_map(|x| (0..=10).map(move |y| punto(f64::from(x), f64::from(y))))
        .collect();
    let mut left: Colonna = Vec::new();
    for x in 0..10 {
        for y in 0..10 {
            let (x, y) = (f64::from(x), f64::from(y));
            left.push(punto(x + 0.5, y + 0.5)); // quattro pari
            left.push(punto(x + 0.5, y)); // due pari
            left.push(punto(x, y)); // distanza zero
        }
    }
    left.push(punto(-3.0, -3.0)); // fuori dalla griglia
    left.push(punto(5.0, 20.0));
    let esito = confronta_ed_esito(&left, &right, None, u64::MAX, u64::MAX).unwrap();
    // La griglia produce davvero i pari: quattro per i centri delle celle.
    assert_eq!(esito.iter().filter(|riga| riga.0 == 0).count(), 4);
    confronta_con_soglie(&left[..30], &right);
}

#[test]
fn bersagli_duplicati_intercalati_a_nulli_e_vuoti() {
    let right: Colonna = vec![
        punto(1.0, 1.0),
        None,
        punto(1.0, 1.0),
        Some(Geometry::LineString(LineString::new(vec![]))),
        punto(1.0, 1.0),
        punto(-1.0, -1.0),
        Some(Geometry::MultiPoint(MultiPoint::new(vec![]))),
        punto(1.0, 1.0),
        punto(-1.0, -1.0),
    ];
    let left: Colonna = vec![
        punto(0.0, 0.0),
        None,
        punto(1.0, 1.0),
        Some(Geometry::Polygon(poligono_vuoto())),
        punto(3.0, 3.0),
    ];
    confronta_con_soglie(&left, &right);
}

#[test]
fn pari_esatti_su_una_circonferenza() {
    let mut right: Colonna = vec![
        punto(1.0, 0.0),
        punto(0.0, 1.0),
        punto(-1.0, 0.0),
        punto(0.0, -1.0),
        punto(0.6, 0.8),
        punto(0.8, 0.6),
        punto(-0.6, -0.8),
        punto(3.0, 4.0),
        punto(-5.0, 0.0),
    ];
    for passo in 0..64 {
        let angolo = f64::from(passo) * std::f64::consts::TAU / 64.0;
        right.push(punto(angolo.cos(), angolo.sin()));
    }
    let left: Colonna = vec![punto(0.0, 0.0), punto(0.0, 0.0), punto(1e-17, 0.0)];
    confronta_con_soglie(&left, &right);
}

#[test]
fn geometrie_che_si_toccano_o_si_contengono() {
    let right: Colonna = vec![
        Some(Geometry::Polygon(rettangolo(0.0, 0.0, 1.0, 1.0))),
        Some(Geometry::Polygon(con_buco(
            (10.0, 10.0, 20.0, 20.0),
            (12.0, 12.0, 18.0, 18.0),
        ))),
        Some(Geometry::LineString(linea(&[(0.0, 5.0), (5.0, 0.0)]))),
        Some(Geometry::Line(Line::new(
            Coord { x: -2.0, y: -2.0 },
            Coord { x: -1.0, y: -1.0 },
        ))),
        Some(Geometry::MultiPolygon(MultiPolygon::new(vec![
            rettangolo(30.0, 0.0, 31.0, 1.0),
            rettangolo(33.0, 0.0, 34.0, 1.0),
        ]))),
        Some(Geometry::Rect(Rect::new(
            Coord { x: 40.0, y: 0.0 },
            Coord { x: 41.0, y: 1.0 },
        ))),
        Some(Geometry::Triangle(Triangle::new(
            Coord { x: 50.0, y: 0.0 },
            Coord { x: 52.0, y: 0.0 },
            Coord { x: 51.0, y: 2.0 },
        ))),
    ];
    let left: Colonna = vec![
        Some(Geometry::Polygon(rettangolo(1.0, 0.0, 2.0, 1.0))), // lato comune
        Some(Geometry::Polygon(rettangolo(1.0, 1.0, 2.0, 2.0))), // solo un vertice
        punto(0.5, 0.5),                                         // dentro
        punto(15.0, 15.0),                                       // nel buco
        punto(12.0, 15.0),                                       // sul bordo del buco
        Some(Geometry::Polygon(rettangolo(13.0, 13.0, 14.0, 14.0))), // nel buco
        Some(Geometry::Polygon(rettangolo(-5.0, -5.0, 60.0, 60.0))), // contiene
        Some(Geometry::LineString(linea(&[(0.0, 0.0), (5.0, 5.0)]))), // attraversa
        punto(2.5, 2.5),                                         // sulla diagonale
        punto(32.0, 0.5),                                        // fra due parti
        punto(40.5, 0.5),
        punto(51.0, -1.0),
        Some(Geometry::MultiPoint(MultiPoint::new(vec![
            Point::new(-100.0, -100.0),
            Point::new(100.0, 100.0),
        ]))),
        Some(Geometry::GeometryCollection(GeometryCollection::new_from(
            vec![
                Geometry::Point(Point::new(45.0, 0.5)),
                Geometry::LineString(linea(&[(25.0, -1.0), (25.0, 2.0)])),
            ],
        ))),
    ];
    confronta_con_soglie(&left, &right);
}

#[test]
fn coordinate_enormi_e_minime() {
    let mut casi: Vec<(Colonna, Colonna)> = Vec::new();
    for scala in [
        1e-320, 1e-300, 1e-200, 1e-160, 1e-130, 3.9e-121, 1e-120, 1e-6, 1.0, 5e6, 1e120, 2.5e120,
        3e120, 1e200, 1e300, 1e307,
    ] {
        let right: Colonna = vec![
            punto(scala, 0.0),
            punto(-scala, 0.0),
            punto(0.0, scala),
            punto(scala, scala),
            Some(Geometry::LineString(linea(&[
                (-scala, 2.0 * scala),
                (scala, 3.0 * scala),
            ]))),
            punto(1.0, 1.0),
        ];
        let left: Colonna = vec![
            punto(0.0, 0.0),
            punto(scala * 0.5, 0.0),
            punto(-scala * 0.5, scala * 0.5),
            punto(scala, 2.5 * scala),
            punto(-f64::MAX, 0.0),
            punto(f64::MAX, f64::MAX),
        ];
        casi.push((left, right));
    }
    // Distanze che vanno a infinito: due pari a +inf.
    casi.push((
        vec![punto(f64::MAX, 0.0)],
        vec![
            punto(-f64::MAX, 0.0),
            punto(-f64::MAX, 0.0),
            punto(0.0, f64::MAX),
        ],
    ));
    // Scale mescolate nella stessa colonna.
    casi.push((
        vec![punto(0.0, 0.0), punto(1e300, 1e300), punto(1e-300, 0.0)],
        vec![
            punto(1e-300, 1e-300),
            punto(1e300, -1e300),
            punto(1.0, 0.0),
            punto(2e-300, 0.0),
        ],
    ));
    for (left, right) in &casi {
        confronta_con_soglie(left, right);
    }
}

#[test]
fn coordinate_non_finite_nel_percorso_validated() {
    // Il gate le rifiuta; il percorso validated le riceve solo a precondizione
    // violata, e deve comunque dire quello che diceva la forza bruta.
    let right: Colonna = vec![
        punto(f64::NAN, 0.0),
        punto(1.0, 0.0),
        punto(f64::INFINITY, 0.0),
        punto(1.0, 0.0),
    ];
    let left: Colonna = vec![
        punto(0.0, 0.0),
        punto(f64::NAN, f64::NAN),
        punto(f64::NEG_INFINITY, 0.0),
    ];
    confronta_con_soglie(&left, &right);
    confronta_con_soglie(&left, &[punto(f64::NAN, 1.0)]);
}

#[test]
fn parti_vuote_e_degeneri_non_si_scartano() {
    // `geo` da' distanza zero da un poligono vuoto dentro una collezione e da
    // un anello interno vuoto, e usa una tolleranza assoluta in f32 per la
    // linea di un solo punto: il rettangolo d'ingombro non e' un limite
    // inferiore per queste geometrie.
    let collezione_con_vuoto = Geometry::GeometryCollection(GeometryCollection::new_from(vec![
        Geometry::Point(Point::new(100.0, 100.0)),
        Geometry::Polygon(poligono_vuoto()),
    ]));
    let right: Colonna = vec![
        punto(1.0, 0.0),
        Some(collezione_con_vuoto.clone()),
        Some(Geometry::LineString(linea(&[(0.0, 5e-8)]))),
        Some(Geometry::MultiPolygon(MultiPolygon::new(vec![
            rettangolo(200.0, 200.0, 201.0, 201.0),
            poligono_vuoto(),
        ]))),
        Some(Geometry::Polygon(Polygon::new(
            rettangolo(300.0, 300.0, 301.0, 301.0).exterior().clone(),
            vec![LineString::new(vec![])],
        ))),
        Some(Geometry::MultiLineString(MultiLineString::new(vec![
            linea(&[(400.0, 0.0), (401.0, 0.0)]),
            LineString::new(vec![]),
        ]))),
        Some(Geometry::GeometryCollection(GeometryCollection::new_from(
            vec![
                Geometry::Point(Point::new(500.0, 0.0)),
                Geometry::MultiPoint(MultiPoint::new(vec![])),
            ],
        ))),
        punto(1e-9, 0.0),
    ];
    let left: Colonna = vec![
        punto(0.0, 0.0),
        punto(0.0, 1e-7),
        punto(50.0, 50.0),
        Some(collezione_con_vuoto),
        Some(Geometry::LineString(linea(&[(1e-9, 1e-9)]))),
        Some(Geometry::Polygon(Polygon::new(
            rettangolo(-1.0, -1.0, 1.0, 1.0).exterior().clone(),
            vec![LineString::new(vec![])],
        ))),
    ];
    confronta_con_soglie(&left, &right);
    // Una riga per volta, cosi' ogni right irregolare e' il solo pari.
    for indice in 1..right.len() {
        let coppia = vec![right[0].clone(), right[indice].clone(), punto(1e-9, 0.0)];
        confronta_con_soglie(&left, &coppia);
    }
}

// --- confronti casuali deterministici ---------------------------------------------

/// splitmix64: sequenza fissata dal seme, nessuna dipendenza da tempo o
/// thread.
struct Sequenza(u64);

impl Sequenza {
    fn prossimo(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn fino_a(&mut self, limite: u64) -> u64 {
        self.prossimo() % limite
    }

    /// Coordinata su una griglia intera piccola (molti pari) scalata.
    fn coordinata(&mut self, lato: u64, scala: f64, origine: f64) -> f64 {
        let passo = f64::from(u32::try_from(self.fino_a(lato)).unwrap());
        origine + passo * scala
    }
}

fn geometria_casuale(
    sequenza: &mut Sequenza,
    lato: u64,
    scala: f64,
    origine: f64,
) -> Option<Geometry<f64>> {
    let c = |s: &mut Sequenza| s.coordinata(lato, scala, origine);
    let tipo = sequenza.fino_a(14);
    let (x0, y0) = (c(sequenza), c(sequenza));
    let (x1, y1) = (c(sequenza), c(sequenza));
    let lato_min = scala;
    Some(match tipo {
        0..=4 => Geometry::Point(Point::new(x0, y0)),
        5 => Geometry::Line(Line::new(Coord { x: x0, y: y0 }, Coord { x: x1, y: y1 })),
        6 => Geometry::LineString(linea(&[(x0, y0), (x1, y1), (c(sequenza), c(sequenza))])),
        7 => Geometry::Polygon(rettangolo(x0, y0, x0 + lato_min, y0 + 2.0 * lato_min)),
        8 => Geometry::MultiPoint(MultiPoint::new(vec![
            Point::new(x0, y0),
            Point::new(x1, y1),
        ])),
        9 => Geometry::Rect(Rect::new(Coord { x: x0, y: y0 }, Coord { x: x1, y: y1 })),
        10 => Geometry::Triangle(Triangle::new(
            Coord { x: x0, y: y0 },
            Coord {
                x: x0 + lato_min,
                y: y0,
            },
            Coord {
                x: x0,
                y: y0 + lato_min,
            },
        )),
        11 => Geometry::Polygon(con_buco(
            (x0, y0, x0 + 4.0 * lato_min, y0 + 4.0 * lato_min),
            (
                x0 + lato_min,
                y0 + lato_min,
                x0 + 3.0 * lato_min,
                y0 + 3.0 * lato_min,
            ),
        )),
        12 => Geometry::MultiPolygon(MultiPolygon::new(vec![
            rettangolo(x0, y0, x0 + lato_min, y0 + lato_min),
            rettangolo(x0 + 3.0 * lato_min, y0, x0 + 4.0 * lato_min, y0 + lato_min),
        ])),
        _ => return None,
    })
}

#[test]
fn confronti_casuali_con_l_oracolo() {
    let mut sequenza = Sequenza(0x5EED_0000_0000_A44D);
    let scale_di_prova: [(f64, f64); 7] = [
        (1.0, 0.0),
        (0.25, -3.0),
        (1e-3, 0.0),
        (10.0, 5e6),
        (1e-130, 0.0),
        (1e118, 0.0),
        (1e-200, 0.0),
    ];
    for giro in 0..240 {
        let (scala, origine) = scale_di_prova[giro % scale_di_prova.len()];
        let lato = 2 + sequenza.fino_a(9);
        let n = usize::try_from(1 + sequenza.fino_a(40)).unwrap();
        let m = usize::try_from(1 + sequenza.fino_a(60)).unwrap();
        let left: Colonna = (0..n)
            .map(|_| geometria_casuale(&mut sequenza, lato, scala, origine))
            .collect();
        let right: Colonna = (0..m)
            .map(|_| geometria_casuale(&mut sequenza, lato, scala, origine))
            .collect();
        if giro % 8 == 0 {
            confronta_con_soglie(&left, &right);
        } else {
            confronta(&left, &right, None, u64::MAX, u64::MAX);
        }
    }
}

#[test]
fn confronti_casuali_di_punti_su_griglie_grandi() {
    // Molte righe per colonna: l'albero scarta davvero (vedi il test sotto),
    // e le griglie strette creano pari a ogni riga.
    let mut sequenza = Sequenza(0x0BAD_5EED_0000_0001);
    for (lato, scala, origine) in [(30, 1.0, 0.0), (8, 0.5, 1e6), (1000, 1e-3, -7.0)] {
        let left: Colonna = (0..400)
            .map(|_| {
                punto(
                    sequenza.coordinata(lato, scala, origine) + scala / 2.0,
                    sequenza.coordinata(lato, scala, origine),
                )
            })
            .collect();
        let right: Colonna = (0..600)
            .map(|_| {
                punto(
                    sequenza.coordinata(lato, scala, origine),
                    sequenza.coordinata(lato, scala, origine),
                )
            })
            .collect();
        confronta(&left, &right, None, u64::MAX, u64::MAX);
        confronta(&left, &right, Some(scala), u64::MAX, u64::MAX);
    }
}

#[test]
fn l_indice_scarta_davvero_e_tiene_i_pari() {
    let right: Colonna = (0..50)
        .flat_map(|x| (0..50).map(move |y| punto(f64::from(x), f64::from(y))))
        .collect();
    let usable_right: Vec<_> = right
        .iter()
        .enumerate()
        .filter_map(|(indice, geometria)| geometria.as_ref().map(|valore| (indice, valore)))
        .collect();
    let indice = IndiceVicini::nuovo(&usable_right);
    let sinistra = Geometry::Point(Point::new(10.5, 20.5));
    let candidati = indice.candidati(&sinistra, &usable_right).unwrap();
    // I quattro pari ci sono, e i candidati sono una frazione della colonna.
    for atteso in [10 * 50 + 20, 10 * 50 + 21, 11 * 50 + 20, 11 * 50 + 21] {
        assert!(candidati.contains(&atteso));
    }
    assert!(candidati.len() <= 16, "{}", candidati.len());
    assert!(candidati.windows(2).all(|coppia| coppia[0] < coppia[1]));
    // Un right irregolare e' candidato di ogni riga.
    let mut con_irregolare = right.clone();
    con_irregolare.push(Some(Geometry::LineString(linea(&[(1000.0, 1000.0)]))));
    let usable: Vec<_> = con_irregolare
        .iter()
        .enumerate()
        .filter_map(|(indice, geometria)| geometria.as_ref().map(|valore| (indice, valore)))
        .collect();
    let indice = IndiceVicini::nuovo(&usable);
    assert!(indice
        .candidati(&sinistra, &usable)
        .unwrap()
        .contains(&2500));
    // Un left irregolare prende tutto.
    let irregolare = Geometry::LineString(linea(&[(10.5, 20.5)]));
    assert_eq!(
        indice.candidati(&irregolare, &usable).unwrap().len(),
        usable.len()
    );
}

#[test]
fn tolleranza_di_geo_fuori_dal_rettangolo_d_ingombro() {
    // `line_string_contains_point` di geo accetta il punto (1, 2^20 + ulp)
    // sul segmento (0, 0)-(1, 2^20) per la sua tolleranza parametrica: la
    // distanza calcolata e' zero anche se il rettangolo del segmento dista un
    // ulp. Il punto B e' piu' vicino per rettangolo (2^-40) ed e' il primo
    // candidato: senza il margine lo scarto perderebbe il segmento, che e' il
    // vero minimo della forza bruta.
    let alto = f64::from(1_u32 << 20);
    let sopra = alto.next_up();
    let distanza_b = f64::from_bits((1023 - 40) << 52);
    let left: Colonna = vec![punto(1.0, sopra)];
    let right: Colonna = vec![
        punto(1.0 + distanza_b, sopra),
        Some(Geometry::LineString(linea(&[(0.0, 0.0), (1.0, alto)]))),
    ];
    let esito = confronta_ed_esito(&left, &right, None, u64::MAX, u64::MAX).unwrap();
    assert_eq!(esito, vec![(0, 1, 0.0_f64.to_bits())]);
    confronta_con_soglie(&left, &right);
}

/// Coppia valida (passa il gate) su cui `geo` va in panico: con coordinate
/// intorno a 1e-200 i quadrati delle differenze vanno in underflow e
/// `nearest_neighbour_distance` di `geo` chiede a `rstar` un confronto con NaN.
fn coppia_che_fa_panicare_geo() -> (Colonna, Colonna) {
    (
        vec![Some(Geometry::LineString(linea(&[
            (3e-200, 1e-200),
            (4e-200, 0.0),
            (2e-200, 0.0),
        ])))],
        vec![Some(Geometry::Rect(Rect::new(
            Coord {
                x: 1e-200,
                y: 1e-200,
            },
            Coord {
                x: 2e-200,
                y: 4e-200,
            },
        )))],
    )
}

#[test]
fn un_panico_di_geo_diventa_calcolo_non_concluso() {
    let (left, right) = coppia_che_fa_panicare_geo();
    // La forza bruta di prima andava davvero in panico su questo ingresso.
    assert!(std::panic::catch_unwind(|| forza_bruta(&left, &right, None, 10, 10, false)).is_err());
    for esito in [
        nearest_matches(&left, &right, None, 10, 10),
        nearest_matches_validated(&left, &right, None, 10, 10),
    ] {
        assert!(
            matches!(esito, Err(AnalysisError::CalcoloNonConcluso(_))),
            "{esito:?}"
        );
    }
    for esito in [
        minimum_distances(&left, &right, 10),
        minimum_distances_validated(&left, &right, 10),
    ] {
        assert!(
            matches!(esito, Err(AnalysisError::CalcoloNonConcluso(_))),
            "{esito:?}"
        );
    }
    // Linea di un solo punto (solo a precondizione violata): stesso esito.
    let un_punto: Colonna = vec![Some(Geometry::LineString(linea(&[(0.0, 0.0)])))];
    let segmento: Colonna = vec![Some(Geometry::LineString(linea(&[(1.0, 0.0), (2.0, 0.0)])))];
    assert!(matches!(
        nearest_matches_validated(&un_punto, &segmento, None, 10, 10),
        Err(AnalysisError::CalcoloNonConcluso(_))
    ));
    assert!(matches!(
        minimum_distances_validated(&un_punto, &segmento, 10),
        Err(AnalysisError::CalcoloNonConcluso(_))
    ));
}

#[test]
fn il_calcolo_non_concluso_vince_sul_limite_dei_risultati() {
    // Righe che superano `max_results` e una riga che fa panicare `geo`:
    // l'errore non dipende dall'ordine dei thread.
    let (sinistra, destra) = coppia_che_fa_panicare_geo();
    let mut left: Colonna = (0..64)
        .map(|indice| punto(f64::from(indice), 0.0))
        .collect();
    left.extend(sinistra);
    let mut right: Colonna = vec![punto(0.5, 1e-200), punto(0.5, -1e-200)];
    right.extend(destra);
    for _ in 0..20 {
        assert!(matches!(
            nearest_matches_validated(&left, &right, None, u64::MAX, 3),
            Err(AnalysisError::CalcoloNonConcluso(_))
        ));
    }
    confronta(&left, &right, None, u64::MAX, 3);
}
