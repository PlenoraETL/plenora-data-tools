//! Il margine di memoria passato ai kernel geo dal runner (README, «Modelli
//! di costo geo»): su un profilo avversario che il modello non copre (ogni
//! punto in ogni poligono, righe di left larghe) il passo si ferma nel
//! kernel, con un `ResourceLimit` che nomina il margine, prima di allocare
//! le coppie e l'uscita; con un budget ampio il risultato e' quello di
//! sempre.

mod comune_geo;

use geo::{Coord, Geometry, LineString, Point, Polygon};
use plenora_core::arrow::array::RecordBatch;
use plenora_core::ErrorCategory;
use serde_json::json;

use comune_geo::{con_budget, esegui, passo, piano, tabella, UTM, X0, Y0};

/// `n` cerchi da 1.000 vertici (righe di circa 16 KB) e `m` punti dentro
/// tutti.
#[allow(clippy::cast_precision_loss)]
fn tutti_in_tutti(n: usize, m: usize) -> (RecordBatch, RecordBatch) {
    let cerchi: Vec<Option<Geometry<f64>>> = (0..n)
        .map(|k| {
            let mut anello: Vec<Coord<f64>> = (0..1_000)
                .map(|j| {
                    let a = std::f64::consts::TAU * f64::from(j) / 1_000.0;
                    Coord {
                        x: 400.0f64.mul_add(a.cos(), X0 + 500.0 + k as f64),
                        y: 400.0f64.mul_add(a.sin(), Y0 + 500.0),
                    }
                })
                .collect();
            anello.push(anello[0]);
            Some(Geometry::Polygon(Polygon::new(
                LineString::new(anello),
                vec![],
            )))
        })
        .collect();
    let punti: Vec<Option<Geometry<f64>>> = (0..m)
        .map(|k| {
            Some(Geometry::Point(Point::new(
                X0 + 450.0 + (k % 10) as f64,
                Y0 + 450.0 + (k / 10) as f64,
            )))
        })
        .collect();
    (tabella(UTM, &cerchi), tabella(UTM, &punti))
}

#[test]
fn il_join_con_ogni_coppia_si_ferma_al_margine_del_kernel() {
    let (cerchi, punti) = tutti_in_tutti(20, 150);
    let pipeline = piano(
        &["c", "p"],
        vec![passo(
            "x",
            "geo.sjoin",
            &["c", "p"],
            json!({"predicate": "intersects"}),
        )],
        &["x"],
    );
    // Budget ampio: 3.000 coppie, ognuna con la riga del cerchio.
    let esito = esegui(&pipeline, &[("c", cerchi.clone()), ("p", punti.clone())])
        .expect("sjoin con budget ampio");
    assert_eq!(esito.outputs[0].1.num_rows(), 20 * 150);
    // 16 MiB: il modello lascia girare il passo, le coppie con la riga del
    // cerchio ripetuta (circa 48 MB) non ci stanno, e il kernel si ferma col
    // margine invece di allocarle.
    let errore = esegui(
        &con_budget(pipeline, 16 * 1024 * 1024),
        &[("c", cerchi), ("p", punti)],
    )
    .expect_err("margine superato");
    assert_eq!(errore.category(), ErrorCategory::ResourceLimit);
    let testo = errore.to_string();
    assert!(testo.contains("margine"), "{testo}");
    assert!(testo.contains("geo.sjoin"), "{testo}");
}

/// Una sola riga larga di left ripetuta per migliaia di coppie: il conto
/// per coppia usa la riga piu' larga, non la media (che qui e' piccola),
/// e il kernel si ferma col margine invece di lasciar allocare l'uscita.
#[test]
fn la_riga_larga_ripetuta_conta_per_intero() {
    let (cerchio, _) = tutti_in_tutti(1, 0);
    // Left: il cerchio (circa 16 KB) e 999 punti lontani da tutto.
    let geometrie_left: Vec<Option<Geometry<f64>>> =
        std::iter::once(comune_geo::geometrie(&cerchio, "geometry")[0].clone())
            .chain((0..999).map(|k| {
                Some(Geometry::Point(Point::new(
                    X0 + 5_000.0 + f64::from(k),
                    Y0 + 5_000.0,
                )))
            }))
            .collect();
    let left = tabella(UTM, &geometrie_left);
    // Right: 2.000 punti dentro il cerchio.
    let right = tabella(
        UTM,
        &(0..2_000)
            .map(|k| {
                Some(Geometry::Point(Point::new(
                    X0 + 400.0 + f64::from(k % 40),
                    Y0 + 400.0 + f64::from(k / 40),
                )))
            })
            .collect::<Vec<_>>(),
    );
    let pipeline = piano(
        &["l", "r"],
        vec![passo(
            "x",
            "geo.sjoin",
            &["l", "r"],
            json!({"predicate": "intersects"}),
        )],
        &["x"],
    );
    let esito = esegui(&pipeline, &[("l", left.clone()), ("r", right.clone())])
        .expect("sjoin con budget ampio");
    assert_eq!(esito.outputs[0].1.num_rows(), 2_000);
    // 2.000 copie della riga del cerchio sono circa 32 MB: con 16 MiB il
    // kernel si ferma col margine.
    let errore = esegui(
        &con_budget(pipeline, 16 * 1024 * 1024),
        &[("l", left), ("r", right)],
    )
    .expect_err("margine superato");
    assert_eq!(errore.category(), ErrorCategory::ResourceLimit);
    assert!(errore.to_string().contains("margine"), "{errore}");
}
