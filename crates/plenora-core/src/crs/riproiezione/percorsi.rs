//! Percorsi fra due datum: catene di trasformazioni EPSG della tabella.
//!
//! Un percorso e' una sequenza di al piu' [`MAX_PASSI_PERCORSO`]
//! trasformazioni, ognuna nel verso del registro o nell'inverso, che non
//! ripassa per lo stesso datum. Entrano le trasformazioni senza griglia e
//! quelle `NTv2` di cui l'utente fornisce il file.
//!
//! Ordine di preferenza, totale e deterministico: accuratezza (somma delle
//! accuratezze EPSG dei passi), poi numero di passi, poi l'area d'uso piu'
//! piccola (somma dei riquadri dei passi in gradi quadrati: a parita' di
//! accuratezza vince la trasformazione piu' specifica, per esempio Monte
//! Mario to WGS 84 (2) per la Sardegna prima della (4) per l'Italia
//! continentale, il cui riquadro contiene anche la Sardegna), poi i codici
//! EPSG in ordine lessicografico, poi il verso (registro prima
//! dell'inverso).

use std::collections::BTreeSet;

use super::tabella::{trasformazioni, Trasformazione};
use super::{PassoPercorso, PercorsoDatum, MAX_PASSI_PERCORSO};

/// Tutti i percorsi da `da` ad `a`, in ordine di preferenza. Lo stesso
/// datum ha il solo percorso vuoto, di accuratezza 0.
pub(super) fn enumera(da: u32, a: u32, griglie: &BTreeSet<u32>) -> Vec<PercorsoDatum> {
    if da == a {
        return vec![PercorsoDatum {
            passi: Vec::new(),
            accuratezza_m: 0.0,
        }];
    }
    let usabili: Vec<&'static Trasformazione> = trasformazioni()
        .iter()
        .filter(|t| !t.a_griglia() || griglie.contains(&t.codice))
        .collect();
    let mut trovati = Vec::new();
    let mut passi = Vec::new();
    let mut visitati = vec![da];
    visita(da, a, &usabili, &mut passi, &mut visitati, &mut trovati);
    let mut percorsi: Vec<PercorsoDatum> = trovati
        .into_iter()
        .map(|passi: Vec<PassoPercorso>| {
            let accuratezza_m = passi.iter().map(PassoPercorso::accuratezza_m).sum();
            PercorsoDatum {
                passi,
                accuratezza_m,
            }
        })
        .collect();
    percorsi.sort_by(|x, y| {
        x.accuratezza_m
            .total_cmp(&y.accuratezza_m)
            .then(x.passi.len().cmp(&y.passi.len()))
            .then_with(|| area(x).total_cmp(&area(y)))
            .then_with(|| x.codici().cmp(&y.codici()))
            .then_with(|| {
                let versi =
                    |p: &PercorsoDatum| p.passi.iter().map(|s| s.inversa).collect::<Vec<_>>();
                versi(x).cmp(&versi(y))
            })
    });
    percorsi
}

/// Somma dei riquadri d'uso dei passi, in gradi quadrati (un riquadro
/// oltre l'antimeridiano conta la sua larghezza vera).
fn area(percorso: &PercorsoDatum) -> f64 {
    percorso
        .passi
        .iter()
        .map(|passo| {
            let riquadro = passo.trasformazione.area;
            let mut larghezza = riquadro.east_longitude - riquadro.west_longitude;
            if larghezza < 0.0 {
                larghezza += 360.0;
            }
            larghezza * (riquadro.north_latitude - riquadro.south_latitude)
        })
        .sum()
}

fn visita(
    corrente: u32,
    arrivo: u32,
    usabili: &[&'static Trasformazione],
    passi: &mut Vec<PassoPercorso>,
    visitati: &mut Vec<u32>,
    trovati: &mut Vec<Vec<PassoPercorso>>,
) {
    if passi.len() == MAX_PASSI_PERCORSO {
        return;
    }
    for trasformazione in usabili {
        let (prossimo, inversa) = if trasformazione.da == corrente {
            (trasformazione.a, false)
        } else if trasformazione.a == corrente {
            (trasformazione.da, true)
        } else {
            continue;
        };
        if visitati.contains(&prossimo) {
            continue;
        }
        passi.push(PassoPercorso {
            trasformazione,
            inversa,
        });
        if prossimo == arrivo {
            trovati.push(passi.clone());
        } else {
            visitati.push(prossimo);
            visita(prossimo, arrivo, usabili, passi, visitati, trovati);
            visitati.pop();
        }
        passi.pop();
    }
}
