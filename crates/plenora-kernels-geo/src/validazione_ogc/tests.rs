//! Oracolo differenziale della validazione OGC rapida (AGENTS.md, regola 3).
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

use super::{
    autointersezione_con, errori_di_validazione, limite_confronti, relate_non_e_disgiunta,
    scansione_coppie, visita_geometria, visita_multipoligono_con, visita_poligono_con,
    CoppieCandidate, Percorso, Preparate, RicercaCoppie, ValidazioneOgc,
};
use geo::algorithm::validation::{
    InvalidGeometry, InvalidMultiPolygon, InvalidPolygon, Validation,
};
use geo::coordinate_position::CoordPos;
use geo::dimensions::Dimensions;
use geo::{Coord, Geometry, Intersects, LineString, MultiPolygon, Polygon, Rect, Relate};
use proptest::prelude::*;

use crate::test_support::{casi, test_lunghi};

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
    if let Geometry::Polygon(poligono) = geometria {
        confronta_metodo(
            "errori_del_poligono",
            geometria,
            || poligono.validation_errors(),
            || super::errori_del_poligono(poligono),
        );
    }
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
    // Suite di default: un ottavo dei casi dallo stesso generatore.
    let (piccoli, medi, grandi) = if test_lunghi() {
        (20_000, 2_000, 8)
    } else {
        (2_500, 250, 2)
    };
    // Piccoli su griglia: collinearita' e tocchi fitti.
    for _ in 0..piccoli {
        let vertici = 3 + rng.fino_a(40);
        let lato = 1 + rng.fino_a(6);
        let caso = anello_su_griglia(&mut rng, vertici, lato);
        con_autointersezione += u32::from(verifica_predicato(&caso));
        valutati += 1;
    }
    // Stellati di media taglia, validi e alterati.
    for _ in 0..medi {
        let vertici = 4 + rng.fino_a(300);
        let granularita = [1.0, 0.5, 8.0, 64.0][rng.indice(4)];
        let caso = anello_stellato(&mut rng, vertici, granularita);
        con_autointersezione += u32::from(verifica_predicato(&caso));
        verifica_geometria(&Geometry::Polygon(Polygon::new(caso, vec![])));
        valutati += 1;
    }
    // Pochi grandi: il riferimento e' quadratico.
    for _ in 0..grandi {
        let vertici = 1_000 + rng.fino_a(1_000);
        let caso = anello_stellato(&mut rng, vertici, 0.25);
        con_autointersezione += u32::from(verifica_predicato(&caso));
        valutati += 1;
    }
    // Entrambi i verdetti devono essere rappresentati in quantita', o il
    // differenziale non prova niente: almeno 1 000 su 22 008 nella suite
    // lunga, in proporzione in quella di default.
    assert_eq!(valutati, piccoli + medi + grandi);
    let minimo = valutati / 22;
    assert!(
        con_autointersezione >= minimo && valutati - con_autointersezione >= minimo,
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
    #![proptest_config(ProptestConfig { cases: casi(500, 4_000), failure_persistence: None, ..ProptestConfig::default() })]

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

// ---------------------------------------------------------------------------
// Coppie di poligoni e di buchi: scansione contro doppio ciclo
// ---------------------------------------------------------------------------

/// I percorsi delle coppie: il doppio ciclo di `geo` senza filtro, la
/// scansione di produzione, e la scansione che rinuncia subito (limite 0) o
/// dopo una coppia (limite 1) e passa al doppio ciclo filtrato.
const PERCORSI: [Percorso; 7] = [
    Percorso {
        doppio_ciclo: true,
        limite_coppie: None,
        limite_confronti: None,
    },
    Percorso::PRODUZIONE,
    Percorso {
        doppio_ciclo: false,
        limite_coppie: Some(0),
        limite_confronti: None,
    },
    Percorso {
        doppio_ciclo: false,
        limite_coppie: Some(1),
        limite_confronti: None,
    },
    // Solo la scansione su `x`.
    Percorso {
        doppio_ciclo: false,
        limite_coppie: None,
        limite_confronti: Some(usize::MAX),
    },
    // Solo l'R-tree.
    Percorso {
        doppio_ciclo: false,
        limite_coppie: None,
        limite_confronti: Some(0),
    },
    // L'R-tree dopo qualche confronto della scansione.
    Percorso {
        doppio_ciclo: false,
        limite_coppie: None,
        limite_confronti: Some(3),
    },
];

/// Gli errori emessi da una visita fino all'eventuale panico, e se c'e'
/// stato panico.
fn emessi<E>(
    visita: impl FnOnce(&mut dyn FnMut(E) -> Result<(), std::convert::Infallible>),
) -> (Vec<E>, bool) {
    let raccolti = std::cell::RefCell::new(Vec::new());
    let panico = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        visita(&mut |errore| {
            raccolti.borrow_mut().push(errore);
            Ok(())
        });
    }))
    .is_err();
    (raccolti.into_inner(), panico)
}

/// Il primo errore, o `None` se la visita va in panico.
fn primo<R>(visita: impl FnOnce() -> R) -> Option<R> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(visita)).ok()
}

/// Il `MultiPolygon` su ogni percorso contro `geo` (il doppio ciclo del
/// vendor): primo errore (variante e indici) e sequenza completa degli
/// errori emessi, panico compreso. Rende il primo errore di `geo`.
fn verifica_multi(poligoni: &MultiPolygon<f64>) -> Option<Result<(), InvalidMultiPolygon>> {
    let atteso_primo = primo(|| poligoni.check_validation());
    let atteso_tutti = emessi(|gestisci| {
        let _: Result<(), std::convert::Infallible> = poligoni.visit_validation(Box::new(gestisci));
    });
    for percorso in PERCORSI {
        let primo_rapido = primo(|| visita_multipoligono_con(poligoni, percorso, &mut Err));
        assert_eq!(
            primo_rapido, atteso_primo,
            "primo errore divergente ({percorso:?}) su {poligoni:?}"
        );
        let tutti = emessi(|gestisci| {
            let _ = visita_multipoligono_con(poligoni, percorso, gestisci);
        });
        assert_eq!(
            tutti, atteso_tutti,
            "sequenza degli errori divergente ({percorso:?}) su {poligoni:?}"
        );
    }
    verifica_geometria(&Geometry::MultiPolygon(poligoni.clone()));
    atteso_primo
}

/// Lo stesso per i buchi di un `Polygon`.
fn verifica_buchi(poligono: &Polygon<f64>) -> Option<Result<(), InvalidPolygon>> {
    let atteso_primo = primo(|| poligono.check_validation());
    let atteso_tutti = emessi(|gestisci| {
        let _: Result<(), std::convert::Infallible> = poligono.visit_validation(Box::new(gestisci));
    });
    for percorso in PERCORSI {
        let primo_rapido = primo(|| visita_poligono_con(poligono, percorso, &mut Err));
        assert_eq!(
            primo_rapido, atteso_primo,
            "primo errore divergente ({percorso:?}) su {poligono:?}"
        );
        let tutti = emessi(|gestisci| {
            let _ = visita_poligono_con(poligono, percorso, gestisci);
        });
        assert_eq!(
            tutti, atteso_tutti,
            "sequenza degli errori divergente ({percorso:?}) su {poligono:?}"
        );
    }
    verifica_geometria(&Geometry::Polygon(poligono.clone()));
    atteso_primo
}

fn rettangolo(x: f64, y: f64, larghezza: f64, altezza: f64) -> LineString<f64> {
    chiuso(&[
        (x, y),
        (x + larghezza, y),
        (x + larghezza, y + altezza),
        (x, y + altezza),
    ])
}

fn parte(anello: LineString<f64>) -> Polygon<f64> {
    Polygon::new(anello, vec![])
}

/// Quadrati di lato 1 su un reticolo di passo 2, `colonne` per riga.
#[allow(clippy::cast_precision_loss)]
fn reticolo_di_parti(quante: usize, colonne: usize) -> Vec<Polygon<f64>> {
    (0..quante)
        .map(|indice| {
            parte(rettangolo(
                2.0 * (indice % colonne) as f64,
                2.0 * (indice / colonne) as f64,
                1.0,
                1.0,
            ))
        })
        .collect()
}

#[test]
#[allow(clippy::too_many_lines)]
fn multipoligoni_con_rettangoli_che_si_toccano() {
    let quadrato = |x: f64, y: f64| parte(rettangolo(x, y, 1.0, 1.0));
    let triangolo = |punti: &[(f64, f64)]| parte(chiuso(punti));
    let casi: Vec<Vec<Polygon<f64>>> = vec![
        // Lato in comune: tocco lungo una linea.
        vec![quadrato(0.0, 0.0), quadrato(1.0, 0.0)],
        vec![quadrato(0.0, 0.0), quadrato(0.0, 1.0)],
        // Solo un vertice in comune: valido.
        vec![quadrato(0.0, 0.0), quadrato(1.0, 1.0)],
        vec![quadrato(0.0, 0.0), quadrato(1.0, -1.0)],
        // Parte di lato in comune.
        vec![quadrato(0.0, 0.0), parte(rettangolo(1.0, 0.5, 1.0, 1.0))],
        // Rettangoli che si toccano, geometrie separate.
        vec![
            triangolo(&[(0.0, 0.0), (1.0, 0.0), (0.0, 1.0)]),
            triangolo(&[(1.0, 1.0), (2.0, 1.0), (2.0, 2.0)]),
        ],
        // Rettangoli sovrapposti, geometrie separate.
        vec![
            triangolo(&[(0.0, 0.0), (4.0, 0.0), (0.0, 4.0)]),
            triangolo(&[(4.0, 4.0), (1.0, 4.0), (4.0, 1.0)]),
        ],
        // Rettangoli separati da un solo ulp.
        vec![
            quadrato(0.0, 0.0),
            quadrato(f64::from_bits(1.0_f64.to_bits() + 1), 0.0),
        ],
        // Lato in comune a x = 0 con zeri di segno opposto.
        vec![
            parte(chiuso(&[
                (-1.0, 0.0),
                (-0.0, 0.0),
                (-0.0, 1.0),
                (-1.0, 1.0),
            ])),
            parte(chiuso(&[(0.0, -0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)])),
        ],
        // Sovrapposizione, contenimento, parti identiche.
        vec![quadrato(0.0, 0.0), parte(rettangolo(0.5, 0.5, 1.0, 1.0))],
        vec![
            parte(rettangolo(0.0, 0.0, 4.0, 4.0)),
            parte(rettangolo(1.0, 1.0, 1.0, 1.0)),
        ],
        vec![quadrato(0.0, 0.0), quadrato(0.0, 0.0)],
        vec![quadrato(3.0, 3.0), quadrato(0.0, 0.0), quadrato(3.0, 3.0)],
        // Parte dentro il buco di un'altra: valido; poi che tocca il buco.
        vec![
            Polygon::new(
                rettangolo(0.0, 0.0, 10.0, 10.0),
                vec![rettangolo(2.0, 2.0, 6.0, 6.0)],
            ),
            parte(rettangolo(3.0, 3.0, 2.0, 2.0)),
        ],
        vec![
            Polygon::new(
                rettangolo(0.0, 0.0, 10.0, 10.0),
                vec![rettangolo(2.0, 2.0, 6.0, 6.0)],
            ),
            parte(rettangolo(2.0, 3.0, 2.0, 2.0)),
            parte(rettangolo(5.0, 5.0, 3.0, 3.0)),
        ],
        // Parti vuote in mezzo, anche con buchi.
        vec![
            parte(LineString(vec![])),
            quadrato(0.0, 0.0),
            Polygon::new(LineString(vec![]), vec![rettangolo(0.0, 0.0, 1.0, 1.0)]),
            quadrato(1.0, 0.0),
            parte(LineString(vec![])),
        ],
        // Parti degeneri: area nulla, un punto, rettangoli di larghezza 0.
        vec![
            parte(chiuso(&[(0.0, 0.0), (1.0, 0.0), (2.0, 0.0)])),
            parte(chiuso(&[(1.0, 0.0), (1.0, 1.0), (1.0, 2.0)])),
            quadrato(0.0, -1.0),
        ],
        vec![parte(chiuso(&[(1.0, 1.0); 3])), quadrato(0.0, 0.0)],
        // Parti invalide che si sovrappongono: errori della parte e della
        // coppia intercalati.
        vec![
            parte(chiuso(&[(0.0, 0.0), (2.0, 2.0), (2.0, 0.0), (0.0, 2.0)])),
            quadrato(0.5, 0.5),
            parte(chiuso(&[(0.0, 0.0), (1.0, 0.0)])),
        ],
        // Coordinate estreme.
        vec![
            parte(rettangolo(-f64::MAX, -f64::MAX, f64::MAX, f64::MAX)),
            parte(chiuso(&[
                (0.0, 0.0),
                (f64::MAX, 0.0),
                (f64::MAX, f64::MAX),
                (0.0, f64::MAX),
            ])),
            parte(rettangolo(-1.0, -1.0, 1.0, 1.0)),
        ],
        vec![
            parte(rettangolo(0.0, 0.0, f64::from_bits(1), f64::from_bits(1))),
            parte(rettangolo(
                f64::from_bits(1),
                0.0,
                f64::from_bits(1),
                f64::from_bits(1),
            )),
            parte(rettangolo(
                0.0,
                f64::from_bits(1),
                f64::from_bits(1),
                f64::from_bits(1),
            )),
        ],
    ];
    for parti in casi {
        let poligoni = MultiPolygon(parti.clone());
        verifica_multi(&poligoni);
        let mut rovesciati = parti;
        rovesciati.reverse();
        verifica_multi(&MultiPolygon(rovesciati));
    }
}

#[test]
fn multipoligoni_con_coordinate_non_finite() {
    for cattivo in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        for posizione in 0..4 {
            let mut parti = reticolo_di_parti(8, 3);
            parti.push(parte(rettangolo(0.5, 0.5, 1.0, 1.0)));
            let mut punti: Vec<Coord<f64>> = parti[posizione * 2].exterior().0.clone();
            punti[posizione].x = cattivo;
            parti[posizione * 2] = parte(LineString(punti));
            verifica_multi(&MultiPolygon(parti.clone()));
            // Nel buco di una parte: il rettangolo non lo vede.
            let mut con_buco = parti;
            con_buco[1] = Polygon::new(
                rettangolo(2.0, 0.0, 1.0, 1.0),
                vec![chiuso(&[(2.2, 0.2), (cattivo, 0.2), (2.5, 0.8)])],
            );
            verifica_multi(&MultiPolygon(con_buco));
        }
    }
}

#[test]
#[allow(clippy::cast_precision_loss)]
fn multipoligoni_su_reticolo_con_un_errore_in_testa_o_in_coda() {
    for quante in [1_usize, 2, 3, 17, 100, 400] {
        let colonne = quante.isqrt().max(1);
        let reticolo = reticolo_di_parti(quante, colonne);
        assert_eq!(
            verifica_multi(&MultiPolygon(reticolo.clone())),
            Some(Ok(()))
        );
        if quante < 2 {
            continue;
        }
        let ultima = reticolo[quante - 1].clone();
        let sposta = |poligono: &Polygon<f64>, dx: f64, dy: f64| {
            parte(LineString(
                poligono
                    .exterior()
                    .0
                    .iter()
                    .map(|c| Coord {
                        x: c.x + dx,
                        y: c.y + dy,
                    })
                    .collect(),
            ))
        };
        // Sovrapposizione alla fine: l'ultima parte sulla penultima.
        let mut in_coda = reticolo.clone();
        in_coda.push(sposta(&ultima, 0.5, 0.5));
        let errore = verifica_multi(&MultiPolygon(in_coda));
        assert_eq!(
            errore,
            Some(Err(InvalidMultiPolygon::ElementsOverlaps(
                geo::algorithm::validation::GeometryIndex(quante - 1),
                geo::algorithm::validation::GeometryIndex(quante),
            )))
        );
        // All'inizio: la prima parte copre l'ultima (coppia (0, k-1)), e un
        // tocco su un lato piu' avanti; l'ordine degli errori e' (i, j).
        let mut in_testa = reticolo.clone();
        in_testa[0] = sposta(&ultima, 0.25, 0.25);
        in_testa[quante / 2] = sposta(&reticolo[quante / 2], 1.0, 0.0);
        verifica_multi(&MultiPolygon(in_testa.clone()));
        // Stesse parti, un lato in comune sia con la parte 0 sia con una
        // successiva: il primo errore e' quello di indice minore.
        in_testa.push(sposta(&ultima, 1.0, 0.0));
        verifica_multi(&MultiPolygon(in_testa));
        // Tocchi di vertice fra tutte le parti vicine: scacchiera valida.
        let scacchiera: Vec<Polygon<f64>> = reticolo
            .iter()
            .enumerate()
            .map(|(indice, poligono)| {
                let riga = indice / colonne;
                let dx = if riga % 2 == 1 { 1.0 } else { 0.0 };
                let dy = -(riga as f64);
                sposta(poligono, dx, dy)
            })
            .collect();
        verifica_multi(&MultiPolygon(scacchiera));
        // Parti tutte identiche: ogni coppia sovrapposta.
        if quante <= 17 {
            verifica_multi(&MultiPolygon(vec![ultima.clone(); quante]));
        }
    }
}

#[test]
#[allow(clippy::cast_precision_loss)]
fn buchi_che_si_toccano_fra_loro_e_con_il_guscio() {
    let guscio = rettangolo(0.0, 0.0, 10.0, 10.0);
    let buco = |x: f64, y: f64, lato: f64| rettangolo(x, y, lato, lato);
    let casi: Vec<Vec<LineString<f64>>> = vec![
        vec![buco(1.0, 1.0, 1.0), buco(2.0, 1.0, 1.0)],
        vec![buco(1.0, 1.0, 1.0), buco(2.0, 2.0, 1.0)],
        vec![buco(1.0, 1.0, 2.0), buco(2.0, 2.0, 2.0)],
        vec![buco(1.0, 1.0, 4.0), buco(2.0, 2.0, 1.0)],
        vec![buco(1.0, 1.0, 1.0), buco(1.0, 1.0, 1.0)],
        vec![buco(0.0, 1.0, 1.0), buco(1.0, 1.0, 1.0)],
        vec![buco(0.0, 0.0, 1.0), buco(9.0, 9.0, 1.0)],
        vec![buco(12.0, 12.0, 1.0), buco(1.0, 1.0, 1.0)],
        vec![
            LineString(vec![]),
            buco(1.0, 1.0, 1.0),
            LineString(vec![]),
            buco(2.0, 1.0, 1.0),
        ],
        vec![
            chiuso(&[(1.0, 1.0), (3.0, 1.0), (1.0, 3.0)]),
            chiuso(&[(3.0, 3.0), (3.0, 2.0), (2.0, 3.0)]),
        ],
        vec![
            chiuso(&[(1.0, 1.0), (4.0, 1.0), (1.0, 4.0)]),
            chiuso(&[(4.0, 4.0), (4.0, 2.0), (2.0, 4.0)]),
        ],
        vec![
            chiuso(&[(1.0, 1.0), (2.0, 2.0), (2.0, 1.0), (1.0, 2.0)]),
            buco(1.5, 1.5, 1.0),
        ],
        vec![
            chiuso(&[(-0.0, 1.0), (1.0, 1.0), (1.0, 2.0), (-0.0, 2.0)]),
            chiuso(&[(1.0, 1.0), (2.0, 1.0), (2.0, 2.0), (1.0, 2.0)]),
        ],
    ];
    for buchi in casi {
        verifica_buchi(&Polygon::new(guscio.clone(), buchi.clone()));
        let mut rovesciati = buchi;
        rovesciati.reverse();
        verifica_buchi(&Polygon::new(guscio.clone(), rovesciati));
    }
    // Un reticolo di buchi, con un tocco o una sovrapposizione all'inizio o
    // alla fine.
    for quanti in [2_usize, 30, 200] {
        let colonne = quanti.isqrt().max(1);
        let lato = 2.0f64.mul_add((quanti.div_ceil(colonne) + colonne) as f64, 4.0);
        let guscio = rettangolo(0.0, 0.0, lato, lato);
        let buchi: Vec<LineString<f64>> = reticolo_di_parti(quanti, colonne)
            .into_iter()
            .map(|poligono| {
                LineString(
                    poligono
                        .exterior()
                        .0
                        .iter()
                        .map(|c| Coord {
                            x: c.x + 1.0,
                            y: c.y + 1.0,
                        })
                        .collect(),
                )
            })
            .collect();
        assert_eq!(
            verifica_buchi(&Polygon::new(guscio.clone(), buchi.clone())),
            Some(Ok(()))
        );
        let mut in_coda = buchi.clone();
        in_coda.push(buco(1.5, 1.5, 1.0));
        in_coda.push(buco(0.0, 3.0, 1.0));
        verifica_buchi(&Polygon::new(guscio.clone(), in_coda));
        let mut in_testa = buchi.clone();
        in_testa.insert(0, buco(2.0, 1.0, 1.0));
        verifica_buchi(&Polygon::new(guscio.clone(), in_testa));
        let mut fuori = buchi;
        fuori.insert(1, buco(lato + 1.0, 1.0, 1.0));
        verifica_buchi(&Polygon::new(guscio, fuori));
    }
}

#[test]
fn buchi_con_coordinate_non_finite() {
    for cattivo in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        let guscio = rettangolo(0.0, 0.0, 10.0, 10.0);
        let buchi = vec![
            rettangolo(1.0, 1.0, 1.0, 1.0),
            chiuso(&[(1.5, 1.5), (cattivo, 1.5), (2.5, 2.5)]),
            rettangolo(2.0, 1.0, 1.0, 1.0),
            rettangolo(5.0, 5.0, 1.0, 1.0),
        ];
        verifica_buchi(&Polygon::new(guscio, buchi));
    }
}

/// La scansione, da sola: l'elenco e' esattamente l'insieme delle coppie
/// `i < j` per cui `relate` non prende il ramo disgiunto, in ordine
/// `(i, j)`, su rettangoli scelti per toccarsi (valori interi piccoli, zeri
/// di segno opposto, larghezze nulle, estremi dell'intervallo finito).
#[test]
fn scansione_uguale_al_doppio_ciclo_dei_rettangoli() {
    let valori = [
        -f64::MAX,
        -3.0,
        -1.0,
        -0.0,
        0.0,
        f64::from_bits(1),
        1.0,
        f64::from_bits(1.0_f64.to_bits() + 1),
        2.0,
        5.0,
        f64::MAX,
    ];
    let mut rng = Lcg(0x5EED_0000_0000_0002);
    let mut elenchi = 0_u32;
    for caso in 0..3_000 {
        let quanti = rng.indice(if caso % 10 == 0 { 120 } else { 16 });
        let ingombri: Vec<Option<Rect<f64>>> = (0..quanti)
            .map(|_| {
                if rng.fino_a(10) == 0 {
                    return None;
                }
                let mut estrai = || valori[rng.indice(valori.len())];
                let (x0, y0, x1, y1) = (estrai(), estrai(), estrai(), estrai());
                Some(Rect::new((x0, y0), (x1, y1)))
            })
            .collect();
        let mut attese = Vec::new();
        for i in 0..quanti {
            for j in i + 1..quanti {
                if relate_non_e_disgiunta(ingombri[i], ingombri[j]) {
                    attese.push((i, j));
                }
            }
        }
        for percorso in PERCORSI {
            let coppie = CoppieCandidate::di(ingombri.clone(), true, percorso);
            if matches!(coppie, CoppieCandidate::Elenco(_)) {
                elenchi += 1;
            }
            let trovate: Vec<(usize, usize)> = (0..quanti)
                .flat_map(|i| coppie.di_indice(i).map(move |j| (i, j)))
                .collect();
            if percorso.doppio_ciclo {
                assert_eq!(trovate.len(), quanti * quanti.saturating_sub(1) / 2);
            } else {
                assert_eq!(trovate, attese, "{percorso:?} su {ingombri:?}");
            }
        }
        // Coordinate non finite dichiarate: doppio ciclo senza filtro.
        let coppie = CoppieCandidate::di(ingombri, false, Percorso::PRODUZIONE);
        assert!(matches!(coppie, CoppieCandidate::Tutte(n) if n == quanti));
    }
    assert!(elenchi > 3_000, "{elenchi}");
}

/// Il fatto del vendor su cui poggia lo scarto: dove
/// [`relate_non_e_disgiunta`] e' falso, `relate` non riporta ne'
/// Interno-Interno di area ne' Confine-Confine lineare.
#[test]
#[allow(clippy::cast_precision_loss)]
fn relate_sui_rettangoli_disgiunti_non_produce_errori() {
    let mut rng = Lcg(0x5EED_0000_0000_0003);
    let mut disgiunti = 0_u32;
    for _ in 0..4_000 {
        let (vertici, lato) = (3 + rng.fino_a(6), 1 + rng.fino_a(5));
        let a = anello_su_griglia(&mut rng, vertici, lato);
        let (vertici, lato) = (3 + rng.fino_a(6), 1 + rng.fino_a(5));
        let b = anello_su_griglia(&mut rng, vertici, lato);
        let (dx, dy) = (rng.fino_a(8) as f64 - 1.0, rng.fino_a(8) as f64 - 1.0);
        let b = LineString(
            b.0.iter()
                .map(|c| Coord {
                    x: c.x + dx,
                    y: c.y + dy,
                })
                .collect(),
        );
        let (a, b) = (parte(a), parte(b));
        if relate_non_e_disgiunta(
            geo::BoundingRect::bounding_rect(&a),
            geo::BoundingRect::bounding_rect(&b),
        ) {
            continue;
        }
        disgiunti += 1;
        let matrice = a.relate(&b);
        assert_ne!(
            matrice.get(CoordPos::Inside, CoordPos::Inside),
            Dimensions::TwoDimensional
        );
        assert_ne!(
            matrice.get(CoordPos::OnBoundary, CoordPos::OnBoundary),
            Dimensions::OneDimensional
        );
        let preparata = geo::PreparedGeometry::from(&a);
        assert_eq!(preparata.relate(&b), matrice);
    }
    assert!(disgiunti > 500, "{disgiunti}");
}

/// Multipoligoni e buchi casuali su una griglia intera piccola, con parti
/// vicine: tocchi, sovrapposizioni e parti invalide sono frequenti.
#[test]
#[allow(clippy::cast_precision_loss)]
fn differenziale_di_massa_multipoligoni_e_buchi() {
    let mut rng = Lcg(0x5EED_0000_0000_0004);
    let mut validi = 0_u32;
    let mut invalidi = 0_u32;
    // Suite di default: un quinto dei casi dallo stesso generatore.
    let totale = if test_lunghi() { 1_500 } else { 300 };
    for caso in 0..totale {
        let quante = 1 + rng.indice(if caso % 20 == 0 { 60 } else { 12 });
        let lato = 1 + rng.fino_a(4);
        let campo = 2 + rng.fino_a(20);
        let anelli: Vec<LineString<f64>> = (0..quante)
            .map(|_| {
                let (dx, dy) = (rng.fino_a(campo) as f64, rng.fino_a(campo) as f64);
                let forma = if rng.fino_a(3) == 0 {
                    let vertici = 3 + rng.fino_a(5);
                    anello_su_griglia(&mut rng, vertici, lato)
                } else {
                    let (l, a) = (1 + rng.fino_a(lato), 1 + rng.fino_a(lato));
                    rettangolo(0.0, 0.0, l as f64, a as f64)
                };
                LineString(
                    forma
                        .0
                        .iter()
                        .map(|c| Coord {
                            x: c.x + dx,
                            y: c.y + dy,
                        })
                        .collect(),
                )
            })
            .collect();
        let esito = if caso % 2 == 0 {
            verifica_multi(&MultiPolygon(anelli.into_iter().map(parte).collect()))
                .map(|esito| esito.is_ok())
        } else {
            let guscio = rettangolo(
                -1.0,
                -1.0,
                (campo + lato) as f64 + 2.0,
                (campo + lato) as f64 + 2.0,
            );
            verifica_buchi(&Polygon::new(guscio, anelli)).map(|esito| esito.is_ok())
        };
        match esito {
            Some(true) => validi += 1,
            _ => invalidi += 1,
        }
    }
    let minimo = totale / 15;
    assert!(
        validi >= minimo && invalidi >= minimo,
        "{validi}/{invalidi}"
    );
}

proptest! {
    #![proptest_config(ProptestConfig { cases: casi(125, 1_000), failure_persistence: None, ..ProptestConfig::default() })]

    /// Parti rettangolari o triangolari su una griglia con zeri di segno
    /// opposto, come `MultiPolygon` o come buchi dello stesso guscio.
    #[test]
    fn differenziale_proptest_coppie(
        parti in prop::collection::vec(
            (
                prop_oneof![(0_i32..16).prop_map(f64::from), Just(-0.0)],
                prop_oneof![(0_i32..16).prop_map(f64::from), Just(-0.0)],
                1_i32..4,
                1_i32..4,
                0_u8..3,
            ),
            0..40,
        ),
        come_buchi in any::<bool>(),
    ) {
        let anelli: Vec<LineString<f64>> = parti
            .iter()
            .map(|&(x, y, l, a, forma)| {
                let (l, a) = (f64::from(l), f64::from(a));
                match forma {
                    0 => rettangolo(x, y, l, a),
                    1 => chiuso(&[(x, y), (x + l, y), (x, y + a)]),
                    _ => chiuso(&[(x + l, y + a), (x, y + a), (x + l, y)]),
                }
            })
            .collect();
        if come_buchi {
            verifica_buchi(&Polygon::new(rettangolo(-1.0, -1.0, 22.0, 22.0), anelli));
        } else {
            verifica_multi(&MultiPolygon(anelli.into_iter().map(parte).collect()));
        }
    }
}

// ---------------------------------------------------------------------------
// Uscite con molte parti: parti preparate e ricerca delle coppie con R-tree
// ---------------------------------------------------------------------------

/// Stella di `vertici` punti su angoli equispaziati, a raggio casuale in
/// `[0.75, 1.25)` volte `raggio`, centrata in `(cx, 0)`: i lati sono quasi
/// radiali, e due stelle sfasate si intersecano in molte parti.
fn stella(rng: &mut Lcg, vertici: u32, raggio: f64, cx: f64) -> Polygon<f64> {
    let punti: Vec<(f64, f64)> = (0..vertici)
        .map(|indice| {
            let angolo = f64::from(indice) * std::f64::consts::TAU / f64::from(vertici);
            let r = raggio * rng.unitario().mul_add(0.5, 0.75);
            (r.mul_add(angolo.cos(), cx), r * angolo.sin())
        })
        .collect();
    parte(chiuso(&punti))
}

/// L'intersezione (overlay reale, `topology::boolean_operation`) di due
/// stelle sfasate: un `MultiPolygon` valido con molte parti, una grande
/// (il nucleo comune) e molte piccole attorno, i cui rettangoli si
/// sovrappongono a quello della grande.
fn intersezione_di_stelle(seme: u64, vertici: u32, spostamento: f64) -> Vec<Polygon<f64>> {
    let mut rng = Lcg(seme);
    let a = Geometry::Polygon(stella(&mut rng, vertici, 100.0, 0.0));
    let b = Geometry::Polygon(stella(&mut rng, vertici, 100.0, spostamento));
    let precisione =
        crate::rust_backend::precision::Precision::new(0.01).expect("precisione valida");
    match crate::topology::boolean_operation(
        &a,
        &b,
        crate::topology::BooleanOperation::Intersection,
        precisione,
    )
    .expect("intersezione di due stelle valide")
    {
        Geometry::MultiPolygon(parti) => parti.0,
        Geometry::Polygon(parte) => vec![parte],
        altro => panic!("uscita inattesa: {altro:?}"),
    }
}

/// L'indice della parte con piu' vertici.
fn parte_piu_grande(parti: &[Polygon<f64>]) -> usize {
    (0..parti.len())
        .max_by_key(|&indice| parti[indice].exterior().0.len())
        .expect("almeno una parte")
}

/// Uscite di overlay reali con centinaia di parti, valide e poi alterate in
/// modo che gli errori cadano sulle coppie con la parte grande, in testa e
/// in coda all'ordine: la parte grande preparata una volta e riusata su
/// molte coppie, come `i` e come `j`, deve dare la stessa sequenza di `geo`.
#[test]
fn uscite_di_intersezioni_di_stelle_con_molte_parti() {
    // (seme, vertici, spostamento): da 50 a 250 parti circa.
    let casi: &[(u64, u32, f64)] = if test_lunghi() {
        &[
            (0x5EED_0000_0000_0005, 400, 80.0),
            (0x5EED_0000_0000_0005, 400, 150.0),
            (0x5EED_0000_0000_0005, 800, 100.0),
            (0x5EED_0000_0000_0005, 800, 150.0),
            (0x5EED_0000_0000_0005, 800, 180.0),
        ]
    } else {
        &[
            (0x5EED_0000_0000_0005, 400, 80.0),
            (0x5EED_0000_0000_0005, 800, 150.0),
        ]
    };
    for &(seme, vertici, spostamento) in casi {
        let parti = intersezione_di_stelle(seme, vertici, spostamento);
        assert!(parti.len() > 40, "{} parti", parti.len());
        assert_eq!(verifica_multi(&MultiPolygon(parti.clone())), Some(Ok(())));
        let grande = parte_piu_grande(&parti);

        // La parte grande in coda: e' la `j` di tutte le sue coppie.
        let mut in_coda = parti.clone();
        let spostata = in_coda.remove(grande);
        in_coda.push(spostata.clone());
        assert_eq!(verifica_multi(&MultiPolygon(in_coda.clone())), Some(Ok(())));

        // Una copia della parte grande anche in testa: si sovrappone a se
        // stessa, e tocca le parti piccole come prima.
        let mut doppia = in_coda.clone();
        doppia.insert(0, spostata.clone());
        assert!(matches!(
            verifica_multi(&MultiPolygon(doppia)),
            Some(Err(InvalidMultiPolygon::ElementsOverlaps(..)))
        ));

        // Una parte piccola su sette traslata sul nucleo: sovrapposizioni
        // con la parte grande su molte coppie.
        let mut sovrapposte = parti.clone();
        let centro = geo::Centroid::centroid(&parti[grande]).expect("parte non vuota");
        for (indice, parte) in sovrapposte.iter_mut().enumerate() {
            if indice % 7 != 0 || indice == grande {
                continue;
            }
            let Some(proprio) = geo::Centroid::centroid(&*parte) else {
                continue;
            };
            *parte = geo::Translate::translate(
                &*parte,
                centro.x() - proprio.x(),
                centro.y() - proprio.y(),
            );
        }
        assert!(matches!(
            verifica_multi(&MultiPolygon(sovrapposte)),
            Some(Err(_))
        ));

        // Le stesse parti come buchi di un guscio che le contiene tutte:
        // la stessa preparazione sulle coppie di buchi.
        let guscio = rettangolo(-500.0, -500.0, 1_500.0, 1_000.0);
        let mut buchi: Vec<LineString<f64>> =
            in_coda.iter().map(|p| p.exterior().clone()).collect();
        assert_eq!(
            verifica_buchi(&Polygon::new(guscio.clone(), buchi.clone())),
            Some(Ok(()))
        );
        buchi.insert(0, spostata.exterior().clone());
        assert!(matches!(
            verifica_buchi(&Polygon::new(guscio, buchi)),
            Some(Err(InvalidPolygon::IntersectingRingsOnAnArea(..)))
        ));
    }
}

/// Parti che condividono la proiezione su `x` (una colonna), oltre il
/// limite dei confronti della scansione: in produzione le coppie le trova
/// l'R-tree. Parti che si toccano lungo un lato, in un vertice, o si
/// sovrappongono, piu' una parte alta quanto la colonna, in ordine diretto
/// e rovesciato.
#[test]
fn colonne_di_parti_oltre_il_limite_dei_confronti() {
    // 600 parti in colonna piu' una: C(601, 2) = 180 300 confronti, oltre
    // il limite di produzione max(256 * 601, 2^16) = 153 856.
    let quante: u32 = 600;
    for passo in [2.0, 1.0, 0.5] {
        let mut parti: Vec<Polygon<f64>> = (0..quante)
            .map(|indice| parte(rettangolo(0.0, passo * f64::from(indice), 1.0, 1.0)))
            .collect();
        // Una parte alta quanto la colonna, a destra: tocca ogni quadrato
        // lungo un lato.
        parti.push(parte(rettangolo(1.0, 0.0, 2.0, passo * f64::from(quante))));
        let ingombri: Vec<Option<Rect<f64>>> =
            parti.iter().map(geo::BoundingRect::bounding_rect).collect();
        assert!(matches!(
            scansione_coppie(&ingombri, usize::MAX, limite_confronti(parti.len())),
            RicercaCoppie::TroppiConfronti
        ));
        assert!(verifica_multi(&MultiPolygon(parti.clone())).is_some());
        parti.reverse();
        verifica_multi(&MultiPolygon(parti));
    }
}

/// Reticoli di parti che si toccano lungo i lati (errori su ogni coppia
/// adiacente) e a scacchiera (tocchi nei vertici, validi), con una parte
/// grande con un buco che circonda il reticolo, in testa, in mezzo o in
/// coda.
#[test]
fn reticoli_di_parti_che_si_toccano_con_una_parte_grande() {
    let lato = if test_lunghi() { 20 } else { 10 };
    for scacchiera in [false, true] {
        let mut parti: Vec<Polygon<f64>> = Vec::new();
        for riga in 0..lato {
            for colonna in 0..lato {
                if scacchiera && (riga + colonna) % 2 == 1 {
                    continue;
                }
                parti.push(parte(rettangolo(
                    f64::from(colonna),
                    f64::from(riga),
                    1.0,
                    1.0,
                )));
            }
        }
        let esteso = f64::from(lato);
        let grande = Polygon::new(
            rettangolo(-1.0, -1.0, esteso + 2.0, esteso + 2.0),
            vec![rettangolo(0.0, 0.0, esteso, esteso)],
        );
        assert_eq!(
            verifica_multi(&MultiPolygon(parti.clone())).map(|esito| esito.is_ok()),
            Some(scacchiera)
        );
        for posizione in [0, parti.len() / 2, parti.len()] {
            let mut con_grande = parti.clone();
            con_grande.insert(posizione, grande.clone());
            verifica_multi(&MultiPolygon(con_grande));
        }
    }
}

/// Anelli radiali (stelle a lati quasi radiali) e frattali (fiocco di Koch),
/// semplici e con un vertice spostato su un altro: il predicato e la
/// validazione completa contro `geo`.
#[test]
fn anelli_radiali_e_frattali() {
    let mut rng = Lcg(0x5EED_0000_0000_0009);
    let taglie: &[u32] = if test_lunghi() {
        &[16, 100, 500, 1_500]
    } else {
        &[16, 100, 500]
    };
    let mut anelli: Vec<LineString<f64>> = Vec::new();
    for &vertici in taglie {
        anelli.push(stella(&mut rng, vertici, 100.0, 0.0).exterior().clone());
    }
    let livelli = if test_lunghi() { 5 } else { 4 };
    for livello in 0..=livelli {
        anelli.push(koch(livello));
    }
    let mut alterati_invalidi = 0;
    let quanti = anelli.len();
    for anello in anelli {
        assert!(!verifica_caso(&anello), "{} vertici", anello.0.len());
        let mut punti = anello.0.clone();
        let n = punti.len();
        punti[n / 3] = punti[2 * n / 3];
        alterati_invalidi += usize::from(verifica_caso(&LineString(punti)));
    }
    // Il triangolo di Koch al livello 0 alterato degenera senza incroci.
    assert!(
        alterati_invalidi + 1 >= quanti,
        "{alterati_invalidi}/{quanti}"
    );
}

/// Il fiocco di Koch al `livello` dato, `3 * 4^livello` lati.
fn koch(livello: u32) -> LineString<f64> {
    let altezza = 3.0_f64.sqrt() / 2.0;
    let mut punti = vec![
        Coord { x: 0.0, y: 0.0 },
        Coord { x: 0.5, y: altezza },
        Coord { x: 1.0, y: 0.0 },
        Coord { x: 0.0, y: 0.0 },
    ];
    for _ in 0..livello {
        let mut nuovi = Vec::with_capacity(punti.len() * 4);
        for coppia in punti.windows(2) {
            let (a, b) = (coppia[0], coppia[1]);
            let d = Coord {
                x: (b.x - a.x) / 3.0,
                y: (b.y - a.y) / 3.0,
            };
            let p1 = Coord {
                x: a.x + d.x,
                y: a.y + d.y,
            };
            let p2 = Coord {
                x: 2.0_f64.mul_add(d.x, a.x),
                y: 2.0_f64.mul_add(d.y, a.y),
            };
            // Punta verso l'esterno (anello in senso orario, esterno a
            // sinistra): il terzo di lato ruotato di +60 gradi.
            let apice = Coord {
                x: (-d.y).mul_add(altezza, d.x.mul_add(0.5, p1.x)),
                y: d.y.mul_add(0.5, d.x.mul_add(altezza, p1.y)),
            };
            nuovi.extend([a, p1, apice, p2]);
        }
        nuovi.push(punti[punti.len() - 1]);
        punti = nuovi;
    }
    LineString(punti)
}

/// La politica di memoria di [`Preparate`]: una parte con buchi non resta
/// preparata come `j` (il suo grafo tiene le intersezioni fra anelli, fino a
/// quadratiche), una senza buchi si'; la `i` resta fino a `libera`. La
/// matrice e' quella di `geo` in ogni combinazione.
#[test]
fn preparate_tiene_solo_le_parti_senza_buchi() {
    let con_buco = Polygon::new(
        rettangolo(0.0, 0.0, 4.0, 4.0),
        vec![rettangolo(1.0, 1.0, 2.0, 2.0)],
    );
    let parti = vec![
        parte(rettangolo(-1.0, 0.0, 1.0, 1.0)),
        con_buco.clone(),
        parte(rettangolo(4.0, 0.0, 1.0, 4.0)),
        con_buco,
    ];
    let mut preparate = Preparate::di(&parti);
    for i in 0..parti.len() {
        for j in i + 1..parti.len() {
            assert_eq!(
                preparate.relate(i, j),
                parti[i].relate(&parti[j]),
                "({i}, {j})"
            );
            assert!(preparate.preparate[i].is_some());
            assert_eq!(
                preparate.preparate[j].is_some(),
                parti[j].interiors().is_empty(),
                "({i}, {j})"
            );
        }
        preparate.libera(i);
        assert!(preparate.preparate[i].is_none());
    }
}

/// Quadrato a lati frastagliati (come i confini ondulati di una copertura):
/// `k` vertici per lato con uno scarto pseudo-casuale fino ad `ampiezza`
/// del lato, estremi esatti.
#[allow(clippy::cast_precision_loss)]
fn quadrato_frastagliato(rng: &mut Lcg, k: u32, ampiezza: f64) -> Vec<(f64, f64)> {
    let lato = 1_000.0;
    let mut punti = Vec::new();
    let mut scarto = |j: u32| {
        if j == 0 {
            0.0
        } else {
            ampiezza
                * lato
                * (std::f64::consts::PI * f64::from(j) / f64::from(k)).sin()
                * 2.0f64.mul_add(rng.unitario(), -1.0)
        }
    };
    for j in 0..k {
        punti.push((lato * f64::from(j) / f64::from(k), scarto(j)));
    }
    for j in 0..k {
        punti.push((lato + scarto(j), lato * f64::from(j) / f64::from(k)));
    }
    for j in 0..k {
        punti.push((lato - lato * f64::from(j) / f64::from(k), lato + scarto(j)));
    }
    for j in 0..k {
        punti.push((scarto(j), lato - lato * f64::from(j) / f64::from(k)));
    }
    punti
}

/// I lati frastagliati lungo entrambi gli assi, validi e con un incrocio,
/// su ogni ramo del predicato e nella validazione completa.
#[test]
fn lati_frastagliati_su_entrambi_gli_assi() {
    let mut rng = Lcg(0x5EED_0000_0000_00F1);
    let casi: usize = if test_lunghi() { 60 } else { 12 };
    for caso in 0..casi {
        let k = [16, 64, 250][caso % 3];
        // Scarto fino allo 0,05% o all'8% del lato.
        let ampiezza = [0.0005, 0.08][caso / 3 % 2];
        let punti = quadrato_frastagliato(&mut rng, k, ampiezza);
        let anello = chiuso(&punti);
        verifica_predicato(&anello);
        // Un vertice spostato sul lato opposto: incrocio certo.
        let mut alterati = punti.clone();
        let meta = alterati.len() / 2;
        alterati[meta / 2] = (500.0, 1_200.0);
        assert!(verifica_predicato(&chiuso(&alterati)));
        verifica_geometria(&Geometry::Polygon(Polygon::new(anello, vec![])));
    }
}
