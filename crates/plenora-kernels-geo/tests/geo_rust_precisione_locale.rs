//! La precisione dichiarata (1 cm a terra) come garanzia **locale**: tre
//! controesempi, ciascuno oltre il centimetro e ciascuno silenzioso con una
//! verifica solo globale.
//!
//! - `LINEWORK` con una soglia d'area globale (perimetro del buco per
//!   precisione, 36 m^2) perde la sporgenza di 1 m^2 di un buco che
//!   condivide un lato con la shell.
//! - Un aggancio dopo l'overlay con due passi di griglia per asse su tutti
//!   i vertici, con `span_x = 2^23` m, porta un incrocio a `x = 100`
//!   sull'ascissa `100.0155` di un vertice lontano.
//! - Il noding di `polygonize` con il punto d'incrocio arrotondato in `f64`:
//!   a `2^52` `(B + 1.5, B + 1.5)` diventa `(B + 2, B + 2)`, 0.707 m fuori
//!   da uno dei segmenti.
//!
//! Il quarto controesempio (un buco omesso dall'output di `split`) riguarda
//! le verifiche a posteriori di `split`, private: e' nei test di
//! `rust_backend::split`.
//!
//! Qui le coordinate sono metri e la precisione e' 1 cm.

use geo::{Contains, Coord, CoordsIter, Geometry, LineString, MultiLineString, Point, Polygon};
use plenora_kernels_geo::rust_backend::make_valid::{
    make_valid_geometry_rust, MakeValidError, RepairMethod,
};
use plenora_kernels_geo::rust_backend::polygonize::{
    polygonize_linework_rust, PolygonizeError, PolygonizeLimits, PolygonizeOptions,
};
use plenora_kernels_geo::rust_backend::precision::Precision;
use plenora_kernels_geo::rust_backend::{polygonize_linework, RustBackendError};

/// 1 cm in metri.
const CENTIMETRO: f64 = 0.01;

fn precisione() -> Precision {
    Precision::new(CENTIMETRO).expect("precisione")
}

fn poligoni(output: &Geometry<f64>) -> Vec<Polygon<f64>> {
    match output {
        Geometry::Polygon(poligono) => vec![poligono.clone()],
        Geometry::MultiPolygon(multi) => multi.0.clone(),
        Geometry::GeometryCollection(collezione) => {
            collezione.0.iter().flat_map(poligoni).collect()
        }
        _ => Vec::new(),
    }
}

/// Shell `[0, 1000]^2` e un anello interno che ne condivide il lato
/// inferiore e sporge in alto con un quadrato di 1 m. La sporgenza e' area:
/// resta nella parte poligonale dell'output.
#[test]
fn linework_conserva_la_sporgenza_di_un_metro() {
    let shell = LineString::from(vec![
        (0.0, 0.0),
        (1000.0, 0.0),
        (1000.0, 1000.0),
        (0.0, 1000.0),
        (0.0, 0.0),
    ]);
    let anello = LineString::from(vec![
        (100.0, 0.0),
        (900.0, 0.0),
        (900.0, 900.0),
        (501.0, 900.0),
        (501.0, 1001.0),
        (500.0, 1001.0),
        (500.0, 900.0),
        (100.0, 900.0),
        (100.0, 0.0),
    ]);
    let input = Geometry::Polygon(Polygon::new(shell, vec![anello]));
    let output = make_valid_geometry_rust(&input, RepairMethod::Linework, true, CENTIMETRO)
        .expect("linework");
    let area = poligoni(&output);
    assert!(
        area.iter()
            .any(|poligono| poligono.contains(&Point::new(500.5, 1000.5))),
        "la sporgenza di 1 m^2 non e' nell'area"
    );
    assert!(
        area.iter()
            .any(|poligono| poligono.contains(&Point::new(50.0, 500.0))),
        "la shell non e' nell'area"
    );
}

/// Shell lunga `lontano` con un vertice in alto a `x = 100 + scarto`, e un
/// buco che attraversa il lato inferiore con un incrocio esatto a `x = 100`.
fn shell_con_vertice_lontano(lontano: f64, scarto: f64) -> Geometry<f64> {
    let shell = LineString::from(vec![
        (0.0, 0.0),
        (lontano, 0.0),
        (lontano, 1000.0),
        (100.0 + scarto, 1000.0),
        (0.0, 1000.0),
        (0.0, 0.0),
    ]);
    let buco = LineString::from(vec![
        (90.0, -10.0),
        (110.0, 10.0),
        (80.0, 10.0),
        (90.0, -10.0),
    ]);
    Geometry::Polygon(Polygon::new(shell, vec![buco]))
}

/// A `span_x = 2^23` m l'aggancio del laboratorio (due passi di griglia per
/// asse) portava l'incrocio a `x = 100` sull'ascissa `100.0155` di un
/// vertice a 1 km. Con il motore `i64` il raggio d'aggancio e' fatto di
/// arrotondamenti dei `f64` (sotto il micrometro): l'overlay si esegue e
/// l'incrocio resta esatto, lontano dal vertice.
#[test]
fn aggancio_oltre_il_centimetro_non_serve_piu() {
    let input = shell_con_vertice_lontano(2_f64.powi(23), 0.0155);
    for keep_collapsed in [false, true] {
        let output =
            make_valid_geometry_rust(&input, RepairMethod::Structure, keep_collapsed, CENTIMETRO)
                .expect("structure");
        assert_incrocio_esatto(&output);
    }
}

/// L'incrocio con il lato inferiore sta esattamente a `x = 100`.
fn assert_incrocio_esatto(output: &Geometry<f64>) {
    let sul_lato: Vec<Coord<f64>> = output
        .coords_iter()
        .filter(|coordinata| coordinata.y == 0.0 && (coordinata.x - 100.0).abs() < 1.0)
        .collect();
    assert!(!sul_lato.is_empty(), "incrocio assente");
    assert!(
        sul_lato
            .iter()
            .all(|coordinata| coordinata.x.to_bits() == 100.0_f64.to_bits()),
        "incrocio agganciato a un vertice lontano"
    );
}

/// A `span_x = 2^21` m il bilancio sta sotto il centimetro. Il vertice a
/// `x = 100.0015` e' entro il raggio del laboratorio ma a 1 km di distanza:
/// l'aggancio considera solo i vertici vicini, e l'incrocio esatto resta a
/// `x = 100`.
#[test]
fn l_aggancio_considera_solo_i_vertici_vicini() {
    let input = shell_con_vertice_lontano(2_f64.powi(21), 0.0015);
    let output = make_valid_geometry_rust(&input, RepairMethod::Structure, false, CENTIMETRO)
        .expect("structure");
    assert_incrocio_esatto(&output);
}

fn diagonali(base: f64) -> Geometry<f64> {
    Geometry::MultiLineString(MultiLineString::new(vec![
        LineString::from(vec![(base, base), (base + 3.0, base + 3.0)]),
        LineString::from(vec![(base, base + 3.0), (base + 3.0, base)]),
    ]))
}

/// A `B = 2^52` l'incrocio esatto `(B + 1.5, B + 1.5)` non e'
/// rappresentabile: arrotondato a `(B + 2, B + 2)` dista 0.707 m dal
/// secondo segmento. Il noding si rifiuta, sia nel kernel sia
/// nell'adapter.
#[test]
fn noding_arrotondato_oltre_il_centimetro_e_un_errore() {
    let linee = diagonali(2_f64.powi(52));
    let kernel = polygonize_linework_rust(
        &linee,
        PolygonizeOptions {
            node_input: true,
            require_complete: false,
            limits: PolygonizeLimits::unlimited(),
            precision: CENTIMETRO,
        },
    );
    assert_eq!(kernel, Err(PolygonizeError::PrecisionInsufficient));
    assert!(matches!(
        polygonize_linework(&linee, true, false, 100, 1_000, 100, 100, precisione()),
        Err(RustBackendError::PrecisionInsufficient)
    ));
}

/// Controprova: a `2^30` lo stesso incrocio e' esatto, e il noding divide i
/// due segmenti in quattro linee residue.
#[test]
fn noding_esatto_lontano_dall_origine_resta() {
    let base = 2_f64.powi(30);
    let risultato = polygonize_linework(
        &diagonali(base),
        true,
        false,
        100,
        1_000,
        100,
        100,
        precisione(),
    )
    .expect("polygonize");
    assert!(risultato.polygons.is_empty());
    let incrocio = Coord {
        x: base + 1.5,
        y: base + 1.5,
    };
    let linee = risultato
        .cut_edges
        .iter()
        .chain(&risultato.dangles)
        .collect::<Vec<_>>();
    assert_eq!(linee.len(), 4);
    assert!(linee.iter().all(|linea| linea.0.contains(&incrocio)));
}

/// La precisione e' un argomento esplicito anche del polygonize: non
/// finita o non positiva e' un errore, prima di toccare i dati.
#[test]
fn precisione_del_polygonize_non_valida_e_un_errore() {
    for precision in [0.0, -1.0, f64::NAN, f64::INFINITY] {
        assert_eq!(
            polygonize_linework_rust(
                &diagonali(0.0),
                PolygonizeOptions {
                    node_input: true,
                    require_complete: false,
                    limits: PolygonizeLimits::unlimited(),
                    precision,
                },
            ),
            Err(PolygonizeError::InvalidPrecision)
        );
    }
}

fn quadrato(min_x: f64, min_y: f64, max_x: f64, max_y: f64) -> LineString<f64> {
    LineString::from(vec![
        (min_x, min_y),
        (max_x, min_y),
        (max_x, max_y),
        (min_x, max_y),
        (min_x, min_y),
    ])
}

/// Controesempio: shell `[0, 10]^2`, buchi `A = [3, 5]^2` e
/// `B = [2, 8] x [0, 8]`, con `B` sul lato inferiore della shell e dentro.
/// Unire `B` intero reinserirebbe nell'ordine `A, B` i 4 m^2 di `A`; si
/// unisce solo la sporgenza di `B` fuori dalla shell (vuota). L'esito non
/// dipende dall'ordine: `S \ A`.
#[test]
fn linework_non_dipende_dall_ordine_dei_buchi() {
    let a = quadrato(3.0, 3.0, 5.0, 5.0);
    let b = quadrato(2.0, 0.0, 8.0, 8.0);
    for buchi in [vec![a.clone(), b.clone()], vec![b, a]] {
        let input = Geometry::Polygon(Polygon::new(quadrato(0.0, 0.0, 10.0, 10.0), buchi));
        let output = make_valid_geometry_rust(&input, RepairMethod::Linework, true, CENTIMETRO)
            .expect("linework");
        let area = poligoni(&output);
        let totale: f64 = area.iter().map(geo::Area::unsigned_area).sum();
        assert!((totale - 96.0).abs() <= 1e-9, "area {totale}");
        assert!(
            !area
                .iter()
                .any(|poligono| poligono.contains(&Point::new(4.0, 4.0))),
            "il buco A e' stato reinserito"
        );
    }
}

/// Controesempio: traslato di `(B, B)` con `B = 2^52`, l'incrocio
/// esatto `(B, B + 1.5)` non e' rappresentabile e l'ordinata riportata
/// sbaglia di 27.7 cm, anche se il passo della griglia e' minuscolo. Oltre
/// `ulp(max |coordinata|) > p / 64` nessun kernel calcola: errore esplicito.
#[test]
fn coordinate_troppo_grandi_per_la_precisione_sono_un_errore() {
    let b = 2_f64.powi(52);
    let sposta = |linea: LineString<f64>| {
        LineString::new(
            linea
                .0
                .into_iter()
                .map(|c| Coord {
                    x: c.x + b,
                    y: c.y + b,
                })
                .collect(),
        )
    };
    let shell = sposta(quadrato(0.0, 0.0, 4.0, 4.0));
    let buco = sposta(LineString::from(vec![
        (-1.0, 0.0),
        (1.0, 3.0),
        (1.0, 0.0),
        (-1.0, 0.0),
    ]));
    let input = Geometry::Polygon(Polygon::new(shell, vec![buco]));
    for method in [RepairMethod::Structure, RepairMethod::Linework] {
        assert!(
            matches!(
                make_valid_geometry_rust(&input, method, false, CENTIMETRO),
                Err(MakeValidError::PrecisionInsufficient)
            ),
            "{method:?}"
        );
    }
}

/// Campagna differenziale su ef1d57a, `LINEWORK`, seme 1: due buchi che si
/// sovrappongono, uno sul lato della shell. Unire il buco intero lasciava
/// piena la sovrapposizione (17.5, 42 e 15.75 m^2 oltre GEOS). Con la sola
/// sporgenza l'area e' quella di GEOS, entro perimetro per 1 cm.
#[test]
fn linework_casi_della_campagna_hanno_l_area_di_geos() {
    let casi: [(&str, Vec<LineString<f64>>, f64); 3] = [
        (
            "caso 68",
            vec![
                quadrato(0.0, 0.0, 9.0, 18.0),
                quadrato(1.0, 1.0, 4.0, 13.0),
                quadrato(1.5, 6.0, 4.5, 18.0),
            ],
            126.0,
        ),
        (
            "caso 84",
            vec![
                quadrato(-23.0, -11.0, 13.0, -3.0),
                quadrato(-13.0, -10.5, 11.0, -5.5),
                quadrato(-23.0, -8.5, 1.0, -3.5),
            ],
            168.0,
        ),
        (
            "caso 107",
            vec![
                quadrato(0.3, -0.7, 3.55, 35.3),
                quadrato(0.55, 1.3, 2.3, 25.3),
                quadrato(1.175, 11.3, 2.925, 35.3),
            ],
            75.0,
        ),
    ];
    for (nome, anelli, area_geos) in casi {
        let mut anelli = anelli.into_iter();
        let shell = anelli.next().expect("shell");
        let buchi: Vec<_> = anelli.collect();
        let perimetro: f64 = shell
            .lines()
            .map(|lato| (lato.end.x - lato.start.x).hypot(lato.end.y - lato.start.y))
            .sum();
        for ordine in [buchi.clone(), buchi.iter().rev().cloned().collect()] {
            let input = Geometry::Polygon(Polygon::new(shell.clone(), ordine));
            for keep_collapsed in [false, true] {
                let output = make_valid_geometry_rust(
                    &input,
                    RepairMethod::Linework,
                    keep_collapsed,
                    CENTIMETRO,
                )
                .expect(nome);
                let area: f64 = poligoni(&output).iter().map(geo::Area::unsigned_area).sum();
                assert!(
                    (area - area_geos).abs() <= perimetro * CENTIMETRO,
                    "{nome}: area {area}, GEOS {area_geos}"
                );
            }
        }
    }
}
