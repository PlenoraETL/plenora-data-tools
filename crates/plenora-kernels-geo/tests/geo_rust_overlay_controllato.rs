//! `make_valid` e la precisione dichiarata: 1 cm a terra.
//!
//! La politica (README, «Limiti dichiarati»; AGENTS.md, regola 1): sotto la
//! precisione un risultato puo' differire dall'esatto (vertici spostati,
//! feature sottili fuse o sparite); sopra, ogni errore e' esplicito. Il solo
//! rifiuto legato alla precisione e' una griglia di overlay piu' grossa
//! della precisione (`PrecisionInsufficient`). Coordinate fuori dal dominio
//! dell'aritmetica esatta restano `NumericRange`.
//!
//! Qui le coordinate sono metri e la precisione e' 1 cm.

// Le attese si calcolano con l'aritmetica ordinaria: la tolleranza e' la
// precisione dichiarata, molto sopra l'arrotondamento di un mul_add.
#![allow(clippy::suboptimal_flops)]

use geo::{Area, BoundingRect, Geometry, LineString, MultiPolygon, Polygon};
use geozero::{CoordDimensions, ToWkb};
use plenora_kernels_geo::geometry_from_wkb;
use plenora_kernels_geo::rust_backend::make_valid::{
    make_valid_geometry_rust, make_valid_geometry_rust_bounded,
    make_valid_geometry_rust_with_limits, MakeValidError, MakeValidLimits, RepairMethod,
};
use plenora_kernels_geo::rust_backend::precision::Precision;
use plenora_kernels_geo::rust_backend::{
    make_valid_wkb, RepairMethod as MetodoAdapter, RustBackendError,
};

/// 1 cm in metri.
const CENTIMETRO: f64 = 0.01;

fn precisione() -> Precision {
    Precision::new(CENTIMETRO).expect("precisione")
}

fn rettangolo(min_x: f64, min_y: f64, max_x: f64, max_y: f64) -> LineString<f64> {
    LineString::from(vec![
        (min_x, min_y),
        (max_x, min_y),
        (max_x, max_y),
        (min_x, max_y),
        (min_x, min_y),
    ])
}

fn cornice(lato: f64, margine: f64) -> Polygon<f64> {
    Polygon::new(
        rettangolo(0.0, 0.0, lato, lato),
        vec![rettangolo(margine, margine, lato - margine, lato - margine)],
    )
}

fn perimetro(poligono: &Polygon<f64>) -> f64 {
    std::iter::once(poligono.exterior())
        .chain(poligono.interiors())
        .flat_map(LineString::lines)
        .map(|lato| (lato.end.x - lato.start.x).hypot(lato.end.y - lato.start.y))
        .sum()
}

const LIMITI: MakeValidLimits = MakeValidLimits {
    max_input_coordinates: 1_000_000,
    max_noding_work: 1_000_000_000,
    max_output_geometries: 1_000_000,
    max_output_coordinates: 1_000_000,
};

/// Cornice valida con `L = 2^500`: il segno dell'area non e' decidibile e
/// le coordinate sono fuori dal dominio esatto. Errore esplicito, mai una
/// riparazione che svuoti il poligono.
#[test]
fn cornice_fuori_dominio_e_un_errore_esplicito() {
    let input = Geometry::Polygon(cornice(2_f64.powi(500), 2_f64.powi(448)));
    for method in [RepairMethod::Structure, RepairMethod::Linework] {
        for keep_collapsed in [false, true] {
            let esiti = [
                make_valid_geometry_rust(&input, method, keep_collapsed, CENTIMETRO),
                make_valid_geometry_rust_with_limits(
                    &input,
                    method,
                    keep_collapsed,
                    LIMITI,
                    CENTIMETRO,
                ),
                make_valid_geometry_rust_bounded(
                    &input,
                    method,
                    keep_collapsed,
                    LIMITI,
                    CENTIMETRO,
                ),
            ];
            for esito in esiti {
                assert!(
                    matches!(esito, Err(MakeValidError::NumericRange)),
                    "{method:?}: atteso NumericRange, ottenuto {esito:?}"
                );
            }
        }
    }
}

fn farfalla() -> Polygon<f64> {
    Polygon::new(
        LineString::from(vec![
            (50.0, 50.0),
            (70.0, 70.0),
            (50.0, 70.0),
            (70.0, 50.0),
            (50.0, 50.0),
        ]),
        Vec::new(),
    )
}

/// I poligoni dell'output dentro `[0, 10]^2`: la cornice, separata dalle
/// facce della farfalla.
fn poligoni_della_cornice(output: &Geometry<f64>) -> Vec<Polygon<f64>> {
    let mut trovati = Vec::new();
    let mut raccogli = |singolo: &Polygon<f64>| {
        if singolo
            .bounding_rect()
            .is_some_and(|rettangolo| rettangolo.max().x <= 10.0 + CENTIMETRO)
        {
            trovati.push(singolo.clone());
        }
    };
    let mut pila = vec![output];
    while let Some(geometria) = pila.pop() {
        match geometria {
            Geometry::Polygon(singolo) => raccogli(singolo),
            Geometry::MultiPolygon(multi) => multi.0.iter().for_each(&mut raccogli),
            Geometry::GeometryCollection(collezione) => pila.extend(collezione.0.iter()),
            _ => {}
        }
    }
    trovati
}

/// La cornice sopra la precisione esce con il buco e con l'area entro
/// perimetro per precisione.
fn cornice_conservata(output: &Geometry<f64>, attesa: &Polygon<f64>) -> bool {
    let candidati = poligoni_della_cornice(output);
    let area = candidati.iter().map(Area::unsigned_area).sum::<f64>();
    let buchi = candidati.iter().map(|p| p.interiors().len()).sum::<usize>();
    buchi == 1 && (area - attesa.unsigned_area()).abs() <= perimetro(attesa) * CENTIMETRO
}

/// Cornici di 10 m accanto a una farfalla che obbliga alla riparazione.
/// Margine di qualche centimetro o piu': la cornice resta, con il buco,
/// entro perimetro per 1 cm. Sotto il centimetro: qualunque esito tranne un
/// panico o `NumericRange` (la cornice puo' sparire, come dichiarato).
#[test]
fn cornice_sottile_segue_la_politica_del_centimetro() {
    for margine in [0.001, 0.005, 0.04, 0.08, 0.1, 1.0] {
        let sottile = cornice(10.0, margine);
        let input = Geometry::MultiPolygon(MultiPolygon::new(vec![sottile.clone(), farfalla()]));
        let sopra = margine >= 0.04;
        for method in [RepairMethod::Structure, RepairMethod::Linework] {
            for keep_collapsed in [false, true] {
                let esito = make_valid_geometry_rust(&input, method, keep_collapsed, CENTIMETRO);
                if sopra {
                    let output = esito.unwrap_or_else(|errore| {
                        panic!("{margine} m {method:?}: errore inatteso {errore}")
                    });
                    assert!(
                        cornice_conservata(&output, &sottile),
                        "{margine} m {method:?}: cornice non conservata"
                    );
                } else {
                    assert!(
                        !matches!(esito, Err(MakeValidError::NumericRange)),
                        "{margine} m {method:?}: NumericRange nel dominio"
                    );
                }
            }
        }
        let payload = input.to_wkb(CoordDimensions::xy()).expect("wkb");
        for method in [MetodoAdapter::Structure, MetodoAdapter::Linework] {
            let esito = make_valid_wkb(&payload, method, true, precisione());
            if sopra {
                let output = esito.unwrap_or_else(|errore| {
                    panic!("{margine} m adapter {method:?}: errore inatteso {errore}")
                });
                let output = geometry_from_wkb(&output).expect("valida");
                assert!(
                    cornice_conservata(&output, &sottile),
                    "{margine} m adapter {method:?}: cornice non conservata"
                );
            } else {
                assert!(
                    !matches!(esito, Err(RustBackendError::NumericRange)),
                    "{margine} m adapter {method:?}: NumericRange nel dominio"
                );
            }
        }
    }
}

/// Primo controesempio della terza revisione, in metri: il buco largo
/// `2^-40` m e' sotto la precisione e puo' sparire; il quadrato resta.
#[test]
fn buco_sotto_la_precisione_puo_sparire_il_resto_resta() {
    let largo = 0.5 + 2_f64.powi(-40);
    let buco = rettangolo(0.5, 0.25, largo, 0.75);
    let quadrato = Polygon::new(rettangolo(0.0, 0.0, 1.0, 1.0), vec![buco]);
    let input = Geometry::MultiPolygon(MultiPolygon::new(vec![quadrato.clone(), farfalla()]));
    for method in [RepairMethod::Structure, RepairMethod::Linework] {
        let output = make_valid_geometry_rust(&input, method, false, CENTIMETRO)
            .unwrap_or_else(|errore| panic!("{method:?}: {errore}"));
        let area = poligoni_della_cornice(&output)
            .iter()
            .map(Area::unsigned_area)
            .sum::<f64>();
        assert!(
            (area - 1.0).abs() <= perimetro(&quadrato) * CENTIMETRO,
            "{method:?}: area {area}"
        );
    }
}

/// Il controllo della griglia: un poligono da riparare esteso 20.000 km in
/// metri ha la griglia dell'overlay sopra il centimetro (circa 1,9 cm) ed e'
/// rifiutato; lo stesso a 1.300 km (l'Italia) passa.
#[test]
fn griglia_oltre_il_centimetro_e_l_unico_rifiuto() {
    let invalido = |lato: f64| {
        // Buco che esce dalla shell: la riparazione passa da un overlay.
        Geometry::Polygon(Polygon::new(
            rettangolo(0.0, 0.0, lato, lato),
            vec![rettangolo(
                0.75 * lato,
                0.25 * lato,
                1.25 * lato,
                0.75 * lato,
            )],
        ))
    };
    for method in [RepairMethod::Structure, RepairMethod::Linework] {
        assert!(
            matches!(
                make_valid_geometry_rust(&invalido(20_000_000.0), method, false, CENTIMETRO),
                Err(MakeValidError::PrecisionInsufficient)
            ),
            "{method:?}: 20.000 km"
        );
        assert!(
            make_valid_geometry_rust(&invalido(1_300_000.0), method, false, CENTIMETRO).is_ok(),
            "{method:?}: 1.300 km"
        );
    }
    let payload = invalido(20_000_000.0)
        .to_wkb(CoordDimensions::xy())
        .expect("wkb");
    assert!(matches!(
        make_valid_wkb(&payload, MetodoAdapter::Linework, true, precisione()),
        Err(RustBackendError::PrecisionInsufficient)
    ));
}

/// La cornice larga resta riparata come prima.
#[test]
fn cornice_larga_resta_riparata() {
    let larga = cornice(10.0, 2.5);
    let input = Geometry::MultiPolygon(MultiPolygon::new(vec![larga.clone(), farfalla()]));
    for method in [RepairMethod::Structure, RepairMethod::Linework] {
        let output = make_valid_geometry_rust(&input, method, false, CENTIMETRO).expect("riparata");
        assert!(cornice_conservata(&output, &larga), "{method:?}");
    }
}

type Chiave = ((u64, u64), (u64, u64));

fn chiave(a: (f64, f64), b: (f64, f64)) -> Chiave {
    let (a, b) = (
        (a.0.to_bits(), a.1.to_bits()),
        (b.0.to_bits(), b.1.to_bits()),
    );
    if a <= b {
        (a, b)
    } else {
        (b, a)
    }
}

/// Le linee di un buco a `2^30` m dall'origine, sopra la precisione, restano:
/// il laboratorio le scartava con una tolleranza di circa `1.5e-5`
/// proporzionale alle coordinate, qualunque fosse l'estensione. Shell di
/// 1.000 x 10 m, buco alto 5 cm a 2 m dal bordo inferiore, che tocca il lato
/// destro: con `LINEWORK` restano la shell e i tre lati del buco che non
/// stanno sul suo bordo.
#[test]
fn linework_conserva_le_linee_sopra_la_precisione_a_2_alla_30() {
    let base = 1_073_741_824.0; // 2^30
    let shell = rettangolo(base, base, base + 1_000.0, base + 10.0);
    let buco = rettangolo(base + 100.0, base + 2.0, base + 1_000.0, base + 2.05);
    let input = Geometry::Polygon(Polygon::new(shell.clone(), vec![buco]));
    let mut attese = vec![
        chiave((base + 100.0, base + 2.0), (base + 1_000.0, base + 2.0)),
        chiave((base + 100.0, base + 2.05), (base + 1_000.0, base + 2.05)),
        chiave((base + 100.0, base + 2.0), (base + 100.0, base + 2.05)),
    ];
    attese.sort_unstable();
    let shell = Polygon::new(shell, Vec::new());
    let verifica = |output: &Geometry<f64>| {
        let Geometry::GeometryCollection(parti) = output else {
            panic!("attesa una collezione: {output:?}");
        };
        let mut segmenti = Vec::new();
        let mut area = 0.0;
        for parte in &parti.0 {
            match parte {
                Geometry::Polygon(singolo) => area += singolo.unsigned_area(),
                Geometry::MultiLineString(multi) => segmenti.extend(
                    multi
                        .0
                        .iter()
                        .flat_map(LineString::lines)
                        .map(|l| chiave(l.start.x_y(), l.end.x_y())),
                ),
                Geometry::LineString(tratto) => {
                    segmenti.extend(tratto.lines().map(|l| chiave(l.start.x_y(), l.end.x_y())));
                }
                other => panic!("componente inattesa: {other:?}"),
            }
        }
        segmenti.sort_unstable();
        assert_eq!(segmenti, attese, "linee del buco");
        assert!(
            (area - shell.unsigned_area()).abs() <= perimetro(&shell) * CENTIMETRO,
            "area della shell {area}"
        );
    };
    for keep_collapsed in [false, true] {
        let output =
            make_valid_geometry_rust(&input, RepairMethod::Linework, keep_collapsed, CENTIMETRO)
                .expect("riparata");
        verifica(&output);
    }
    let payload = input.to_wkb(CoordDimensions::xy()).expect("wkb");
    let output =
        make_valid_wkb(&payload, MetodoAdapter::Linework, true, precisione()).expect("adapter");
    verifica(&geometry_from_wkb(&output).expect("valida"));
}

/// Il caso con la shell lunga `larghezza` e il buco dal suo secondo
/// milione di metri alla fine, alti 7 e 2 micrometri, a quota `2^30`.
fn caso_92(inizio: f64, larghezza: f64) -> (Geometry<f64>, Polygon<f64>) {
    let base = 1_073_741_824.0; // 2^30
    let shell = rettangolo(
        inizio,
        base + 0.000_003,
        inizio + larghezza,
        base + 0.000_01,
    );
    let buco = rettangolo(
        inizio + 1_000_000.0,
        base + 0.000_005,
        inizio + larghezza,
        base + 0.000_007,
    );
    (
        Geometry::Polygon(Polygon::new(shell.clone(), vec![buco])),
        Polygon::new(shell, Vec::new()),
    )
}

/// Campagna differenziale traslata di `2^30`, seme 1 caso 92: shell larga
/// 6.000 km e alta 7 micrometri, in metri. Il passo della griglia
/// dell'overlay (circa 5,6 mm) sta sotto il centimetro, ma il bilancio di
/// spostamento (arrotondamento piu' aggancio, due diagonali del passo:
/// 11,2 mm) no: errore esplicito.
#[test]
fn caso_92_in_metri_oltre_il_bilancio_e_un_errore() {
    let (input, _) = caso_92(1_071_741_824.0, 6_000_000.0);
    for keep_collapsed in [false, true] {
        assert!(matches!(
            make_valid_geometry_rust(&input, RepairMethod::Linework, keep_collapsed, CENTIMETRO),
            Err(MakeValidError::PrecisionInsufficient)
        ));
    }
}

/// Lo stesso caso lungo 4.000 km: bilancio circa 7,5 mm, sotto il
/// centimetro. Le linee del buco (a 2 micrometri dal bordo) sono sotto la
/// precisione: possono restare o sparire. Il risultato deve esserci, con
/// l'area della shell entro perimetro per 1 cm.
#[test]
fn caso_92_in_metri_e_entro_la_precisione() {
    let (input, shell) = caso_92(1_072_741_824.0, 4_000_000.0);
    for keep_collapsed in [false, true] {
        let output =
            make_valid_geometry_rust(&input, RepairMethod::Linework, keep_collapsed, CENTIMETRO)
                .expect("entro la precisione");
        let area = output.unsigned_area();
        assert!(
            (area - shell.unsigned_area()).abs() <= perimetro(&shell) * CENTIMETRO,
            "area {area}"
        );
    }
}

fn area_triangolo(a: (f64, f64), b: (f64, f64), c: (f64, f64)) -> f64 {
    ((b.0 - a.0) * (c.1 - a.1) - (c.0 - a.0) * (b.1 - a.1)).abs() * 0.5
}

/// `epsilon_ladders_match_geos` del laboratorio, per `make_valid`: con
/// `keep_collapsed = false` entrambi i metodi riescono, con l'area esatta
/// entro perimetro per 1 cm (coordinate lette come metri).
#[test]
fn scala_di_epsilon_del_laboratorio_entro_la_precisione() {
    for epsilon in [1e-15, 1e-13, 1e-11, 1e-9, 1e-7] {
        // Farfalla: i lati (0,0)-(10,10) e (0,10+e)-(10,0) si incrociano in
        // P; le due facce sono (10,10),(0,10+e),P e (0,0),P,(10,0).
        let px = 10.0 * (10.0 + epsilon) / (20.0 + epsilon);
        let attesa_farfalla = area_triangolo((10.0, 10.0), (0.0, 10.0 + epsilon), (px, px))
            + area_triangolo((0.0, 0.0), (px, px), (10.0, 0.0));
        let farfalla = Geometry::Polygon(Polygon::new(
            LineString::from(vec![
                (0.0, 0.0),
                (10.0, 10.0),
                (0.0, 10.0 + epsilon),
                (10.0, 0.0),
                (0.0, 0.0),
            ]),
            Vec::new(),
        ));
        for method in [RepairMethod::Structure, RepairMethod::Linework] {
            let area = make_valid_geometry_rust(&farfalla, method, false, CENTIMETRO)
                .unwrap_or_else(|errore| panic!("farfalla {epsilon} {method:?}: {errore}"))
                .unsigned_area();
            assert!(
                (area - attesa_farfalla).abs() <= 50.0 * CENTIMETRO,
                "farfalla {epsilon} {method:?}: {area} contro {attesa_farfalla}"
            );
        }
        for direzione in [-1.0, 1.0] {
            let x = 5.0 + direzione * epsilon;
            let buchi = Geometry::Polygon(Polygon::new(
                rettangolo(0.0, 0.0, 10.0, 10.0),
                vec![rettangolo(1.0, 1.0, 5.0, 5.0), rettangolo(x, 3.0, 9.0, 8.0)],
            ));
            // Senza sovrapposizione 100 - 16 - 5 (9 - x); la sovrapposizione
            // (area 2 e, se x < 5) STRUCTURE la sottrae una volta, LINEWORK
            // (parita') la riempie.
            let sovrapposizione = if direzione < 0.0 { 2.0 * epsilon } else { 0.0 };
            for (method, attesa) in [
                (
                    RepairMethod::Structure,
                    5.0f64.mul_add(-(9.0 - x), 84.0) + sovrapposizione,
                ),
                (
                    RepairMethod::Linework,
                    5.0f64.mul_add(-(9.0 - x), 84.0) + 2.0 * sovrapposizione,
                ),
            ] {
                let area = make_valid_geometry_rust(&buchi, method, false, CENTIMETRO)
                    .unwrap_or_else(|errore| {
                        panic!("buchi {epsilon} {direzione} {method:?}: {errore}")
                    })
                    .unsigned_area();
                assert!(
                    (area - attesa).abs() <= 80.0 * CENTIMETRO,
                    "buchi {epsilon} {direzione} {method:?}: {area} contro {attesa}"
                );
            }
        }
    }
}
