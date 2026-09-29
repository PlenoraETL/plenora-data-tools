//! Oracolo differenziale della validazione OGC rapida (AGENTS.md, regola 5).
//!
//! Due riferimenti, entrambi il percorso generico:
//!
//! - [`riferimento_geo`]: copia letterale di
//!   `validation::utils::linestring_has_self_intersection` di `geo` 0.33.1
//!   vendorizzato (non raggiungibile da fuori: e' `pub(crate)`), confrontata
//!   col solo predicato, su ogni ramo (doppio ciclo, scansione su `x`, su `y`,
//!   asse scelto come in produzione);
//! - `check_validation`, `validation_errors` e `visit_validation` di `geo`,
//!   che eseguono il doppio ciclo vero dentro il vendor, confrontati con la
//!   sequenza di `validazione_ogc` sull'**errore completo** (variante,
//!   anello, indici), non solo sull'esito: ogni metodo sotto il proprio
//!   `catch_unwind`, e gli errori emessi prima di un panico confrontati come
//!   prefisso.

use super::{autointersezione_con, errori_di_validazione, visita_geometria, ValidazioneOgc};
use geo::algorithm::validation::{InvalidGeometry, Validation};
use geo::{Coord, Geometry, Intersects, LineString, MultiPolygon, Polygon};
use proptest::prelude::*;

/// `linestring_has_self_intersection` di `geo` 0.33.1, riga per riga.
fn riferimento_geo(geom: &LineString<f64>) -> bool {
    for (i, line) in geom.lines().enumerate() {
        for (j, other_line) in geom.lines().enumerate() {
            if i != j
                && line.intersects(&other_line)
                && line.start != other_line.end
                && line.end != other_line.start
            {
                return true;
            }
        }
    }
    false
}

/// Tutti i rami del predicato contro il riferimento; rende il verdetto.
fn verifica_predicato(anello: &LineString<f64>) -> bool {
    let atteso = riferimento_geo(anello);
    for (doppio_ciclo, asse) in [
        (true, None),
        (false, Some(true)),
        (false, Some(false)),
        (false, None),
    ] {
        assert_eq!(
            autointersezione_con(anello, doppio_ciclo, asse),
            atteso,
            "predicato divergente (doppio ciclo {doppio_ciclo}, asse {asse:?}) su {anello:?}"
        );
    }
    atteso
}

/// Lo stesso metodo sui due percorsi, ciascuno sotto il proprio
/// `catch_unwind`: valori uguali, o panico su entrambi. Un panico su un
/// percorso solo e' una divergenza.
fn confronta_metodo<R: PartialEq + std::fmt::Debug>(
    metodo: &str,
    geometria: &Geometry<f64>,
    generico: impl FnOnce() -> R,
    rapido: impl FnOnce() -> R,
) {
    let generico = std::panic::catch_unwind(std::panic::AssertUnwindSafe(generico));
    let rapido = std::panic::catch_unwind(std::panic::AssertUnwindSafe(rapido));
    match (generico, rapido) {
        (Ok(generico), Ok(rapido)) => {
            assert_eq!(rapido, generico, "{metodo} divergente su {geometria:?}");
        }
        (Err(_), Err(_)) => {}
        (generico, rapido) => panic!(
            "{metodo}: panico solo su un percorso (generico: {}, rapido: {}) su {geometria:?}",
            generico.is_err(),
            rapido.is_err()
        ),
    }
}

/// Gli errori emessi dal visitatore **fino all'eventuale panico**, raccolti
/// fuori dal `catch_unwind`: il prefisso osservabile resta confrontabile
/// anche quando `relate` interrompe la visita.
fn prefisso_emesso(
    visita: impl FnOnce(&std::cell::RefCell<Vec<InvalidGeometry>>),
) -> (Vec<InvalidGeometry>, bool) {
    let raccolti = std::cell::RefCell::new(Vec::new());
    let panico =
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| visita(&raccolti))).is_err();
    (raccolti.into_inner(), panico)
}

/// La validazione completa contro quella di `geo`, metodo per metodo:
/// primo errore (`check_validation`), tutti gli errori
/// (`validation_errors`), e il prefisso degli errori emessi dal visitatore
/// prima di un eventuale panico di `relate`, che la sequenza rapida chiama
/// allo stesso modo.
fn verifica_geometria(geometria: &Geometry<f64>) {
    confronta_metodo(
        "check_validation",
        geometria,
        || geometria.check_validation(),
        || geometria.valida_ogc_rapida(),
    );
    confronta_metodo(
        "validation_errors",
        geometria,
        || geometria.validation_errors(),
        || errori_di_validazione(geometria),
    );
    let generico = prefisso_emesso(|raccolti| {
        let _: Result<(), std::convert::Infallible> =
            geometria.visit_validation(Box::new(|errore| {
                raccolti.borrow_mut().push(errore);
                Ok(())
            }));
    });
    let rapido = prefisso_emesso(|raccolti| {
        let _: Result<(), std::convert::Infallible> = visita_geometria(geometria, &mut |errore| {
            raccolti.borrow_mut().push(errore);
            Ok(())
        });
    });
    assert_eq!(
        rapido, generico,
        "prefisso degli errori o panico divergente su {geometria:?}"
    );
}

fn anello(punti: &[(f64, f64)]) -> LineString<f64> {
    LineString::from(punti.to_vec())
}

fn chiuso(punti: &[(f64, f64)]) -> LineString<f64> {
    let mut punti = punti.to_vec();
    if let Some(&primo) = punti.first() {
        punti.push(primo);
    }
    LineString::from(punti)
}

/// Il caso sotto le simmetrie che non cambiano la geometria: verso
/// opposto, inizio spostato, assi scambiati, segni invertiti, scale in
/// potenze di due (esatte) e verso il basso e l'alto dell'intervallo
/// finito. Ogni variante passa dal predicato e dalla validazione completa.
fn verifica_con_varianti(base: &LineString<f64>) -> bool {
    let verdetto = verifica_caso(base);
    let punti = &base.0;
    let mut varianti: Vec<Vec<Coord<f64>>> = Vec::new();
    varianti.push(punti.iter().rev().copied().collect());
    varianti.push(punti.iter().map(|c| Coord { x: c.y, y: c.x }).collect());
    varianti.push(punti.iter().map(|c| Coord { x: -c.x, y: -c.y }).collect());
    if punti.len() > 2 && punti.first() == punti.last() {
        // Rotazione dell'anello chiuso: si toglie la chiusura, si ruota, si
        // richiude.
        let mut aperto = punti[..punti.len() - 1].to_vec();
        aperto.rotate_left(1);
        if let Some(&primo) = aperto.first() {
            aperto.push(primo);
        }
        varianti.push(aperto);
    }
    for scala in [
        2.0_f64.powi(-1000),
        2.0_f64.powi(-60),
        2.0_f64.powi(60),
        2.0_f64.powi(1000),
        1e-300,
        1e300,
        3.0,
    ] {
        varianti.push(
            punti
                .iter()
                .map(|c| Coord {
                    x: c.x * scala,
                    y: c.y * scala,
                })
                .collect(),
        );
    }
    // Traslazione grande: le coordinate perdono bit, la geometria cambia,
    // il confronto resta fra due percorsi sugli stessi valori.
    varianti.push(
        punti
            .iter()
            .map(|c| Coord {
                x: c.x + 1e15,
                y: c.y - 3e15,
            })
            .collect(),
    );
    for variante in varianti {
        verifica_caso(&LineString(variante));
    }
    verdetto
}

fn verifica_caso(anello: &LineString<f64>) -> bool {
    let verdetto = verifica_predicato(anello);
    verifica_geometria(&Geometry::Polygon(Polygon::new(anello.clone(), vec![])));
    verdetto
}

/// Il caso con `quanti` vertici in piu' sul lato di chiusura (dall'ultimo
/// punto al primo), per farlo scandire su molti segmenti. I punti
/// sono interpolati: sui lati verticali o orizzontali stanno esattamente sul
/// lato, altrove possono scostarsene di un arrotondamento, che per il
/// confronto differenziale non conta.
fn con_lato_denso(punti: &[(f64, f64)], quanti: u32) -> LineString<f64> {
    let mut tutti = punti.to_vec();
    if let (Some(&(x0, y0)), Some(&(x1, y1))) = (punti.last(), punti.first()) {
        for passo in 1..quanti {
            let t = f64::from(passo) / f64::from(quanti);
            tutti.push(((x1 - x0).mul_add(t, x0), (y1 - y0).mul_add(t, y0)));
        }
    }
    chiuso(&tutti)
}

// ---------------------------------------------------------------------------
// Casi avversari deterministici
// ---------------------------------------------------------------------------

#[test]
fn quadrato_semplice_nessuna_autointersezione() {
    let quadrato = chiuso(&[(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)]);
    assert!(!verifica_con_varianti(&quadrato));
    assert!(!verifica_con_varianti(&con_lato_denso(
        &[(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)],
        40
    )));
}

#[test]
fn farfalla_si_autointerseca() {
    let farfalla = chiuso(&[(0.0, 0.0), (10.0, 10.0), (10.0, 0.0), (0.0, 10.0)]);
    assert!(verifica_con_varianti(&farfalla));
}

#[test]
fn punte_in_tutte_le_forme() {
    // Punta che ripercorre il lato precedente: `geo` la salta (coppie
    // adiacenti), e qui deve saltarla allo stesso modo.
    let punta = chiuso(&[
        (0.0, 0.0),
        (10.0, 0.0),
        (10.0, 10.0),
        (15.0, 15.0),
        (10.0, 10.0),
        (0.0, 10.0),
    ]);
    verifica_con_varianti(&punta);
    // Punta che rientra oltre il vertice di partenza.
    let punta_lunga = chiuso(&[
        (0.0, 0.0),
        (10.0, 0.0),
        (10.0, 10.0),
        (10.0, 5.0),
        (10.0, 10.0),
        (0.0, 10.0),
    ]);
    verifica_con_varianti(&punta_lunga);
    // Punta interna.
    let punta_interna = chiuso(&[
        (0.0, 0.0),
        (10.0, 0.0),
        (5.0, 5.0),
        (10.0, 0.0),
        (10.0, 10.0),
        (0.0, 10.0),
    ]);
    verifica_con_varianti(&punta_interna);
    verifica_con_varianti(&con_lato_denso(
        &[
            (0.0, 0.0),
            (10.0, 0.0),
            (10.0, 10.0),
            (15.0, 15.0),
            (10.0, 10.0),
            (0.0, 10.0),
        ],
        30,
    ));
}

#[test]
fn vertici_che_si_toccano_e_autotangenza() {
    // Otto: l'anello passa due volte per lo stesso vertice.
    let otto = chiuso(&[
        (0.0, 0.0),
        (5.0, 5.0),
        (10.0, 0.0),
        (10.0, 10.0),
        (5.0, 5.0),
        (0.0, 10.0),
    ]);
    assert!(verifica_con_varianti(&otto));
    // Un vertice che tocca l'interno di un lato non adiacente.
    let tangente = chiuso(&[
        (0.0, 0.0),
        (10.0, 0.0),
        (10.0, 10.0),
        (5.0, 0.0),
        (0.0, 10.0),
    ]);
    assert!(verifica_con_varianti(&tangente));
    // Buco invertito: l'esterno si tocca in un vertice.
    let invertito = chiuso(&[
        (0.0, 0.0),
        (10.0, 0.0),
        (10.0, 10.0),
        (5.0, 10.0),
        (7.0, 5.0),
        (3.0, 5.0),
        (5.0, 10.0),
        (0.0, 10.0),
    ]);
    assert!(verifica_con_varianti(&invertito));
    verifica_con_varianti(&con_lato_denso(
        &[
            (0.0, 0.0),
            (5.0, 5.0),
            (10.0, 0.0),
            (10.0, 10.0),
            (5.0, 5.0),
            (0.0, 10.0),
        ],
        25,
    ));
}

#[test]
fn segmenti_collineari_sovrapposti() {
    // Due lati sulla stessa retta che si sovrappongono.
    let sovrapposti = chiuso(&[
        (0.0, 0.0),
        (10.0, 0.0),
        (10.0, 5.0),
        (2.0, 5.0),
        (2.0, 0.0),
        (8.0, 0.0),
        (8.0, -5.0),
        (0.0, -5.0),
    ]);
    assert!(verifica_con_varianti(&sovrapposti));
    // Collineari contigui ma non adiacenti nell'anello: si toccano in un
    // estremo che non e' condiviso come vertice consecutivo.
    let contigui = chiuso(&[
        (0.0, 0.0),
        (5.0, 0.0),
        (5.0, 5.0),
        (10.0, 5.0),
        (10.0, 0.0),
        (15.0, 0.0),
        (15.0, 10.0),
        (0.0, 10.0),
    ]);
    verifica_con_varianti(&contigui);
    // Collineari e separati: nessuna intersezione.
    let separati = chiuso(&[
        (0.0, 0.0),
        (4.0, 0.0),
        (4.0, 2.0),
        (6.0, 2.0),
        (6.0, 0.0),
        (10.0, 0.0),
        (10.0, 5.0),
        (0.0, 5.0),
    ]);
    verifica_con_varianti(&separati);
    // Verticali (stessa x): la scansione su x li vede tutti a pari minimo.
    let verticali = chiuso(&[(0.0, 0.0), (0.0, 10.0), (0.0, 3.0), (0.0, 7.0), (-5.0, 5.0)]);
    verifica_con_varianti(&verticali);
}

#[test]
fn punti_ripetuti_e_segmenti_di_lunghezza_zero() {
    let ripetuti = chiuso(&[
        (0.0, 0.0),
        (0.0, 0.0),
        (10.0, 0.0),
        (10.0, 0.0),
        (10.0, 10.0),
        (0.0, 10.0),
        (0.0, 10.0),
    ]);
    verifica_con_varianti(&ripetuti);
    // Un segmento degenere che cade sull'interno di un lato non adiacente.
    let degenere_sul_lato = chiuso(&[
        (0.0, 0.0),
        (10.0, 0.0),
        (10.0, 10.0),
        (5.0, 0.0),
        (5.0, 0.0),
        (0.0, 10.0),
    ]);
    verifica_con_varianti(&degenere_sul_lato);
    // Tutti i punti uguali, e anelli da uno o due punti.
    verifica_con_varianti(&anello(&[(1.0, 1.0); 6]));
    verifica_con_varianti(&anello(&[(1.0, 1.0)]));
    verifica_con_varianti(&anello(&[(1.0, 1.0), (2.0, 2.0)]));
    verifica_con_varianti(&anello(&[]));
    // Anello di soli segmenti degeneri alternati, lungo.
    let mut alternati = Vec::new();
    for passo in 0..40 {
        let punto = (f64::from(passo % 4), f64::from(passo % 3));
        alternati.push(punto);
        alternati.push(punto);
    }
    verifica_con_varianti(&chiuso(&alternati));
    verifica_con_varianti(&con_lato_denso(
        &[
            (0.0, 0.0),
            (0.0, 0.0),
            (10.0, 0.0),
            (10.0, 0.0),
            (10.0, 10.0),
            (0.0, 10.0),
        ],
        30,
    ));
}

#[test]
fn zero_con_segno() {
    // `-0.0 == 0.0` per `geo` e per i rettangoli: l'ordinamento con
    // `total_cmp` li separa, e non deve contare.
    let anello_di_zeri = chiuso(&[
        (-0.0, 0.0),
        (10.0, -0.0),
        (0.0, 10.0),
        (0.0, -0.0),
        (-0.0, 5.0),
        (-5.0, 5.0),
    ]);
    verifica_con_varianti(&anello_di_zeri);
    let mut molti = Vec::new();
    for passo in 0..30 {
        let zero = if passo % 2 == 0 { 0.0 } else { -0.0 };
        molti.push((zero, f64::from(passo)));
        molti.push((f64::from(passo % 5) + 1.0, zero));
    }
    verifica_con_varianti(&chiuso(&molti));
}

#[test]
fn quasi_collineari_sul_filo_del_floating_point() {
    // Il classico 0.1 + 0.2: il punto e' collineare solo in aritmetica
    // esatta sui valori rappresentati, non su quelli "intesi".
    let a = 0.1_f64;
    let b = 0.2_f64;
    let c = a + b;
    let famiglia = [
        chiuso(&[(0.0, 0.0), (c, c), (c, 0.0), (a, a), (b, b), (0.0, c)]),
        chiuso(&[
            (0.0, 0.0),
            (1.0, 1.0),
            (1.0, 0.0),
            (0.5, 0.5 + f64::EPSILON),
            (0.0, 1.0),
        ]),
        chiuso(&[
            (0.0, 0.0),
            (1.0, 1.0),
            (1.0, 0.0),
            (0.5, 0.5 - f64::EPSILON / 4.0),
            (0.0, 1.0),
        ]),
        chiuso(&[(0.0, 0.0), (3.0, 1.0), (3.0, 0.0), (1.5, 0.5), (0.0, 1.0)]),
    ];
    for caso in &famiglia {
        verifica_con_varianti(caso);
    }
    // Un vertice spostato di un ulp alla volta attraverso un lato non
    // adiacente: sopra, sopra, sopra, sopra, sopra, poi sotto.
    let mut y = 1.0 / 3.0;
    for _ in 0..12 {
        let caso = chiuso(&[(0.0, 0.0), (3.0, 1.0), (3.0, 3.0), (1.0, y), (0.0, 3.0)]);
        verifica_con_varianti(&caso);
        y = f64::from_bits(y.to_bits() - 1);
    }
    let mut y = 1.0 / 3.0;
    for _ in 0..12 {
        let caso = con_lato_denso(
            &[(0.0, 0.0), (3.0, 1.0), (3.0, 3.0), (1.0, y), (0.0, 3.0)],
            20,
        );
        verifica_con_varianti(&caso);
        y = f64::from_bits(y.to_bits() + 1);
    }
}

#[test]
fn coordinate_piccolissime_e_enormi() {
    let minimo = f64::from_bits(1); // il subnormale piu' piccolo
    let famiglia = [
        chiuso(&[(0.0, 0.0), (minimo, 0.0), (minimo, minimo), (0.0, minimo)]),
        chiuso(&[
            (0.0, 0.0),
            (2.0 * minimo, 2.0 * minimo),
            (2.0 * minimo, 0.0),
            (0.0, 2.0 * minimo),
        ]),
        chiuso(&[
            (f64::MIN_POSITIVE, 0.0),
            (0.0, f64::MIN_POSITIVE),
            (-f64::MIN_POSITIVE, 0.0),
            (0.0, -f64::MIN_POSITIVE),
        ]),
        // Differenze che traboccano: `max - min` e' infinito.
        chiuso(&[
            (f64::MAX, f64::MAX),
            (-f64::MAX, f64::MAX),
            (-f64::MAX, -f64::MAX),
            (f64::MAX, -f64::MAX),
        ]),
        chiuso(&[
            (f64::MAX, f64::MAX),
            (-f64::MAX, -f64::MAX),
            (f64::MAX, -f64::MAX),
            (-f64::MAX, f64::MAX),
        ]),
        chiuso(&[
            (f64::MAX, 0.0),
            (-f64::MAX, 0.0),
            (0.0, f64::MAX),
            (0.0, -f64::MAX),
        ]),
        chiuso(&[
            (1e308, 1e-308),
            (-1e308, 1e-308),
            (1e-308, 1e308),
            (1e-308, -1e308),
        ]),
    ];
    for caso in &famiglia {
        verifica_con_varianti(caso);
    }
    // A molti vertici, alle stesse scale.
    for scala in [minimo, f64::MIN_POSITIVE, 1e-200, 1e200, f64::MAX / 64.0] {
        let mut punti = Vec::new();
        for passo in 0..40_i32 {
            let angolo = f64::from(passo) / 40.0 * std::f64::consts::TAU;
            punti.push((angolo.cos() * scala * 32.0, angolo.sin() * scala * 32.0));
        }
        verifica_con_varianti(&chiuso(&punti));
        // Stesso cerchio con una corda che lo attraversa.
        punti.insert(20, (0.0, 0.0));
        verifica_con_varianti(&chiuso(&punti));
    }
}

#[test]
fn coordinate_non_finite_rifiutate_come_da_geo() {
    for cattivo in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -f64::NAN] {
        for posizione in 0..5 {
            let mut punti = vec![
                (0.0, 0.0),
                (10.0, 0.0),
                (10.0, 10.0),
                (0.0, 10.0),
                (5.0, 5.0),
            ];
            punti[posizione].0 = cattivo;
            verifica_caso(&chiuso(&punti));
            punti[posizione].1 = cattivo;
            verifica_caso(&chiuso(&punti));
            // A molti vertici: la ricaduta sul doppio ciclo resta la stessa.
            let denso = con_lato_denso(&punti, 30);
            verifica_caso(&denso);
            let geometria = Geometry::Polygon(Polygon::new(denso, vec![]));
            assert!(geometria.valida_ogc_rapida().is_err());
        }
    }
}

#[test]
fn anelli_interni_e_multipoligoni_come_geo() {
    let esterno = chiuso(&[(0.0, 0.0), (100.0, 0.0), (100.0, 100.0), (0.0, 100.0)]);
    let buco = |x: f64, y: f64, lato: f64| {
        chiuso(&[(x, y), (x + lato, y), (x + lato, y + lato), (x, y + lato)])
    };
    let casi = vec![
        Polygon::new(esterno.clone(), vec![buco(10.0, 10.0, 10.0)]),
        Polygon::new(
            esterno.clone(),
            vec![buco(10.0, 10.0, 10.0), buco(15.0, 15.0, 10.0)],
        ),
        Polygon::new(
            esterno.clone(),
            vec![buco(10.0, 10.0, 10.0), buco(20.0, 10.0, 10.0)],
        ),
        Polygon::new(esterno.clone(), vec![buco(0.0, 10.0, 10.0)]),
        Polygon::new(esterno.clone(), vec![buco(90.0, 90.0, 20.0)]),
        Polygon::new(
            esterno.clone(),
            vec![chiuso(&[
                (10.0, 10.0),
                (20.0, 20.0),
                (20.0, 10.0),
                (10.0, 20.0),
            ])],
        ),
        Polygon::new(esterno, vec![LineString(vec![]), buco(10.0, 10.0, 10.0)]),
        Polygon::new(LineString(vec![]), vec![buco(10.0, 10.0, 10.0)]),
    ];
    for poligono in &casi {
        verifica_geometria(&Geometry::Polygon(poligono.clone()));
    }
    let multi = MultiPolygon(casi.clone());
    verifica_geometria(&Geometry::MultiPolygon(multi.clone()));
    verifica_geometria(&Geometry::MultiPolygon(MultiPolygon(vec![
        Polygon::new(buco(0.0, 0.0, 10.0), vec![]),
        Polygon::new(buco(10.0, 0.0, 10.0), vec![]),
        Polygon::new(buco(5.0, 5.0, 10.0), vec![]),
        Polygon::new(buco(20.0, 10.0, 10.0), vec![]),
    ])));
    verifica_geometria(&Geometry::GeometryCollection(geo::GeometryCollection(
        vec![
            Geometry::MultiPolygon(multi),
            Geometry::Polygon(casi[5].clone()),
            Geometry::Point(geo::Point::new(f64::NAN, 0.0)),
            Geometry::LineString(anello(&[(0.0, 0.0), (0.0, 0.0)])),
            Geometry::Rect(geo::Rect::new((0.0, 0.0), (0.0, 0.0))),
            Geometry::Triangle(geo::Triangle::new(
                (0.0, 0.0).into(),
                (1.0, 1.0).into(),
                (2.0, 2.0).into(),
            )),
            Geometry::Line(geo::Line::new((0.0, 0.0), (0.0, 0.0))),
            Geometry::MultiPoint(geo::MultiPoint(vec![geo::Point::new(f64::INFINITY, 0.0)])),
            Geometry::MultiLineString(geo::MultiLineString(vec![anello(&[(0.0, 0.0)])])),
        ],
    )));
}

#[test]
fn reperti_del_repository() {
    for nome in ["reperto_a.wkb", "reperto_b.wkb"] {
        let percorso = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join(nome);
        let byte = std::fs::read(&percorso).expect("reperto leggibile");
        let geometria =
            crate::wkb_decoder::decode_validated(&byte).expect("reperto decodificabile");
        verifica_geometria(&geometria);
        if let Geometry::Polygon(poligono) = &geometria {
            verifica_con_varianti(poligono.exterior());
        }
    }
}

// ---------------------------------------------------------------------------
// Differenziale casuale
// ---------------------------------------------------------------------------

/// Generatore deterministico (LCG di Knuth, stessa forma dei benchmark): il
/// differenziale di massa non dipende dal seme di proptest.
struct Lcg(u64);

impl Lcg {
    fn prossimo(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 11
    }

    fn fino_a(&mut self, limite: u64) -> u64 {
        self.prossimo() % limite
    }

    fn indice(&mut self, quanti: usize) -> usize {
        let quanti = u64::try_from(quanti).expect("usize entra in u64");
        usize::try_from(self.fino_a(quanti)).expect("sotto una lunghezza usize")
    }

    #[allow(clippy::cast_precision_loss)]
    fn unitario(&mut self) -> f64 {
        self.prossimo() as f64 / (1_u64 << 53) as f64
    }
}

/// Anello su una griglia intera piccola: collinearita', tocchi e punti
/// ripetuti sono la norma, non l'eccezione.
#[allow(clippy::cast_precision_loss)]
fn anello_su_griglia(rng: &mut Lcg, vertici: u64, lato: u64) -> LineString<f64> {
    let punti: Vec<(f64, f64)> = (0..vertici)
        .map(|_| (rng.fino_a(lato + 1) as f64, rng.fino_a(lato + 1) as f64))
        .collect();
    if rng.fino_a(4) == 0 {
        anello(&punti)
    } else {
        chiuso(&punti)
    }
}

/// Poligono stellato (angoli ordinati, raggi casuali): semplice prima delle
/// alterazioni, poi arrotondato alla griglia e alterato, cosi' una parte
/// degli anelli resta valida e una parte no.
#[allow(clippy::cast_precision_loss)]
fn anello_stellato(rng: &mut Lcg, vertici: u64, granularita: f64) -> LineString<f64> {
    let mut angoli: Vec<f64> = (0..vertici)
        .map(|_| rng.unitario() * std::f64::consts::TAU)
        .collect();
    angoli.sort_by(f64::total_cmp);
    let mut punti: Vec<(f64, f64)> = angoli
        .iter()
        .map(|angolo| {
            let raggio = rng.unitario().mul_add(900.0, 100.0);
            (
                (raggio * angolo.cos() / granularita).round() * granularita,
                (raggio * angolo.sin() / granularita).round() * granularita,
            )
        })
        .collect();
    match rng.fino_a(8) {
        // Un vertice ripetuto.
        0 if !punti.is_empty() => {
            let indice = rng.indice(punti.len());
            punti.insert(indice, punti[indice]);
        }
        // Un vertice spostato su un altro vertice (tocco o autotangenza).
        1 if punti.len() > 3 => {
            let da = rng.indice(punti.len());
            let a = rng.indice(punti.len());
            punti[da] = punti[a];
        }
        // Una punta verso l'esterno e ritorno.
        2 if !punti.is_empty() => {
            let indice = rng.indice(punti.len());
            let (x, y) = punti[indice];
            punti.insert(indice + 1, (x * 1.5, y * 1.5));
            punti.insert(indice + 2, (x, y));
        }
        // Un vertice portato al centro: il ventaglio si attraversa.
        3 if !punti.is_empty() => {
            let indice = rng.indice(punti.len());
            punti[indice] = (0.0, 0.0);
        }
        // Un vertice portato sul punto medio (esatto sulla griglia pari) di
        // un lato non adiacente.
        4 if punti.len() > 4 => {
            let lato = rng.indice(punti.len() - 1);
            let medio = (
                f64::midpoint(punti[lato].0, punti[lato + 1].0),
                f64::midpoint(punti[lato].1, punti[lato + 1].1),
            );
            let bersaglio = (lato + punti.len() / 2) % punti.len();
            punti[bersaglio] = medio;
        }
        _ => {}
    }
    chiuso(&punti)
}

#[test]
fn differenziale_di_massa_deterministico() {
    let mut rng = Lcg(0x5EED_0000_0000_0001);
    let mut valutati = 0_u32;
    let mut con_autointersezione = 0_u32;
    // Piccoli su griglia: collinearita' e tocchi fitti.
    for _ in 0..20_000 {
        let vertici = 3 + rng.fino_a(40);
        let lato = 1 + rng.fino_a(6);
        let caso = anello_su_griglia(&mut rng, vertici, lato);
        con_autointersezione += u32::from(verifica_predicato(&caso));
        valutati += 1;
    }
    // Stellati di media taglia, validi e alterati.
    for _ in 0..2_000 {
        let vertici = 4 + rng.fino_a(300);
        let granularita = [1.0, 0.5, 8.0, 64.0][rng.indice(4)];
        let caso = anello_stellato(&mut rng, vertici, granularita);
        con_autointersezione += u32::from(verifica_predicato(&caso));
        verifica_geometria(&Geometry::Polygon(Polygon::new(caso, vec![])));
        valutati += 1;
    }
    // Pochi grandi: il riferimento e' quadratico.
    for _ in 0..8 {
        let vertici = 1_000 + rng.fino_a(1_000);
        let caso = anello_stellato(&mut rng, vertici, 0.25);
        con_autointersezione += u32::from(verifica_predicato(&caso));
        valutati += 1;
    }
    // Entrambi i verdetti devono essere rappresentati in quantita', o il
    // differenziale non prova niente.
    assert_eq!(valutati, 22_008);
    assert!(
        con_autointersezione >= 1_000 && valutati - con_autointersezione >= 1_000,
        "{con_autointersezione}/{valutati}"
    );
}

fn strategia_coordinata() -> impl Strategy<Value = f64> {
    prop_oneof![
        4 => (0_i32..8).prop_map(f64::from),
        2 => -1e3_f64..1e3,
        1 => prop::sample::select(vec![
            0.0,
            -0.0,
            f64::from_bits(1),
            f64::MIN_POSITIVE,
            1.0 / 3.0,
            0.1,
            0.2,
            0.300_000_000_000_000_04,
            1e-300,
            1e300,
            f64::MAX,
            -f64::MAX,
        ]),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 4_000, failure_persistence: None, ..ProptestConfig::default() })]

    /// Anelli arbitrari (aperti o chiusi) di coordinate miste: predicato
    /// e validazione completa contro il percorso generico.
    #[test]
    fn differenziale_proptest(
        punti in prop::collection::vec((strategia_coordinata(), strategia_coordinata()), 0..60),
        chiudi in any::<bool>(),
    ) {
        let caso = if chiudi { chiuso(&punti) } else { anello(&punti) };
        verifica_predicato(&caso);
        verifica_geometria(&Geometry::Polygon(Polygon::new(caso, vec![])));
    }

    /// Poligoni con buchi e multipoligoni su griglia: la parte che segue le
    /// auto-intersezioni (contenimento, anelli intersecanti, sovrapposizioni)
    /// e' la stessa sequenza di `geo`.
    #[test]
    fn differenziale_anelli_interni_e_multi(
        anelli in prop::collection::vec(
            prop::collection::vec(((0_i32..12).prop_map(f64::from), (0_i32..12).prop_map(f64::from)), 3..8),
            1..4,
        ),
        multi in any::<bool>(),
    ) {
        let anelli: Vec<LineString<f64>> = anelli.iter().map(|punti| chiuso(punti)).collect();
        let geometria = if multi {
            Geometry::MultiPolygon(MultiPolygon(
                anelli.into_iter().map(|anello| Polygon::new(anello, vec![])).collect(),
            ))
        } else {
            let mut anelli = anelli.into_iter();
            let esterno = anelli.next().unwrap_or_else(|| LineString(vec![]));
            Geometry::Polygon(Polygon::new(esterno, anelli.collect()))
        };
        verifica_geometria(&geometria);
    }
}
