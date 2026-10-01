//! Le coppie di rettangoli chiusi che si sovrappongono, cercate su una
//! griglia di celle: la ricerca delle auto-intersezioni della validazione
//! OGC (`validazione_ogc`) e quella delle coppie di segmenti del noding
//! (`rust_backend::polygonize`).
//!
//! **Perche'.** Una scansione su un asse prova tutte le coppie che
//! condividono la proiezione su quell'asse: con segmenti frastagliati lungo
//! **entrambi** gli assi (i confini ondulati di una copertura) le coppie
//! provate sono quadratiche nei segmenti di un lato qualunque sia l'asse.
//! Le celle separano i rettangoli su entrambi gli assi.
//!
//! **Perche' ogni coppia si visita una volta.**
//!
//! - le celle sono `gx * gy` sull'ingombro di tutti i rettangoli, e
//!   l'indice di cella di una coordinata e' `min(g - 1, floor((v - min) *
//!   (g / estensione)))`, una funzione **non decrescente** di `v`
//!   (sottrazione, prodotto per un fattore finito non negativo e `floor`
//!   arrotondati sono monotoni; con un fattore non finito la griglia non si
//!   costruisce);
//! - ogni rettangolo va in tutte le celle fra quella del suo minimo e quella
//!   del suo massimo, su entrambi gli assi;
//! - due rettangoli chiusi sovrapposti contengono entrambi il punto
//!   `(max(min_x), max(min_y))`, e per la monotonia la cella di quel punto
//!   sta negli intervalli di celle di entrambi: la coppia si incontra in
//!   quella cella, e si visita **solo** li' (stessa funzione sugli stessi
//!   valori).
//!
//! Nessun calcolo sulle coordinate oltre agli indici di cella e ai confronti
//! dei rettangoli: i chiamanti decidono sulle coppie con i propri predicati.

/// Un rettangolo chiuso, a coordinate finite.
#[derive(Clone, Copy, Debug)]
pub struct Rettangolo {
    pub min_x: f64,
    pub max_x: f64,
    pub min_y: f64,
    pub max_y: f64,
}

impl Rettangolo {
    /// Se i due rettangoli chiusi si sovrappongono (anche solo sul bordo).
    pub fn si_sovrappone(&self, altro: &Self) -> bool {
        self.min_x <= altro.max_x
            && altro.min_x <= self.max_x
            && self.min_y <= altro.max_y
            && altro.min_y <= self.max_y
    }
}

/// Quando la griglia non conviene: oltre `celle_per_elemento` inserzioni
/// in media (rettangoli lunghi rispetto alla cella) o `coppie_per_elemento`
/// coppie provate nelle celle in media (molti rettangoli nella stessa
/// cella), i chiamanti tornano alla scansione su un asse.
#[derive(Clone, Copy, Debug)]
pub struct LimitiCelle {
    pub celle_per_elemento: usize,
    pub coppie_per_elemento: usize,
}

/// I limiti di produzione: 8 celle e 64 coppie per elemento.
pub const LIMITI_CELLE: LimitiCelle = LimitiCelle {
    celle_per_elemento: 8,
    coppie_per_elemento: 64,
};

/// Visita ogni coppia `(i, j)`, `i < j`, di rettangoli chiusi sovrapposti
/// esattamente una volta, in un ordine fissato dalle coordinate (per cella,
/// poi per indice), finche' `visita` rende `true`.
///
/// `None` senza nessuna visita se la griglia non si costruisce (nessun
/// rettangolo, estensione o fattore di scala non finiti) o se supera i
/// `limiti` (con `Some`); `Some(true)` se ha visitato tutte le coppie,
/// `Some(false)` se `visita` ha chiesto di fermarsi. I rettangoli devono
/// avere coordinate finite.
pub fn visita_coppie_sovrapposte(
    rettangoli: &[Rettangolo],
    limiti: Option<LimitiCelle>,
    mut visita: impl FnMut(usize, usize) -> bool,
) -> Option<bool> {
    let n = rettangoli.len();
    let griglia = Griglia::di(rettangoli)?;

    // Inserzioni per cella (righe compresse), col limite.
    let limite_inserzioni = limiti.map(|l| n.saturating_mul(l.celle_per_elemento));
    let mut inizi = vec![0_usize; griglia.gx * griglia.gy + 1];
    let mut inserzioni = 0_usize;
    for rettangolo in rettangoli {
        let (x0, x1, y0, y1) = griglia.celle_di(rettangolo);
        inserzioni = inserzioni.saturating_add((x1 - x0 + 1).saturating_mul(y1 - y0 + 1));
        if limite_inserzioni.is_some_and(|limite| inserzioni > limite) {
            return None;
        }
        for cy in y0..=y1 {
            for cx in x0..=x1 {
                inizi[cy * griglia.gx + cx + 1] += 1;
            }
        }
    }
    let mut coppie = 0_usize;
    for indice in 1..inizi.len() {
        let quanti = inizi[indice];
        coppie = coppie.saturating_add(quanti.saturating_mul(quanti.saturating_sub(1)) / 2);
        inizi[indice] += inizi[indice - 1];
    }
    if limiti.is_some_and(|l| coppie > n.saturating_mul(l.coppie_per_elemento)) {
        return None;
    }
    let mut posizioni = inizi.clone();
    let mut contenuto = vec![0_usize; inserzioni];
    for (k, rettangolo) in rettangoli.iter().enumerate() {
        let (x0, x1, y0, y1) = griglia.celle_di(rettangolo);
        for cy in y0..=y1 {
            for cx in x0..=x1 {
                let posto = &mut posizioni[cy * griglia.gx + cx];
                contenuto[*posto] = k;
                *posto += 1;
            }
        }
    }

    for cy in 0..griglia.gy {
        for cx in 0..griglia.gx {
            let cella = cy * griglia.gx + cx;
            // Gli indici di una cella sono crescenti (inseriti in ordine).
            let nella_cella = &contenuto[inizi[cella]..inizi[cella + 1]];
            for (posizione, &i) in nella_cella.iter().enumerate() {
                let a = rettangoli[i];
                for &j in &nella_cella[posizione + 1..] {
                    let b = rettangoli[j];
                    // Solo nella cella del punto `(max(min_x), max(min_y))`.
                    if a.si_sovrappone(&b)
                        && griglia.cella_x(a.min_x.max(b.min_x)) == cx
                        && griglia.cella_y(a.min_y.max(b.min_y)) == cy
                        && !visita(i, j)
                    {
                        return Some(false);
                    }
                }
            }
        }
    }
    Some(true)
}

/// La griglia: celle per asse, minimi e fattori di scala.
struct Griglia {
    gx: usize,
    gy: usize,
    min_x: f64,
    min_y: f64,
    fx: f64,
    fy: f64,
}

impl Griglia {
    /// La griglia sull'ingombro di tutti i rettangoli; `None` senza
    /// rettangoli o con un'estensione o un fattore di scala non finiti.
    fn di(rettangoli: &[Rettangolo]) -> Option<Self> {
        let (mut min_x, mut max_x, mut min_y, mut max_y) = (f64::MAX, f64::MIN, f64::MAX, f64::MIN);
        for rettangolo in rettangoli {
            min_x = min_x.min(rettangolo.min_x);
            max_x = max_x.max(rettangolo.max_x);
            min_y = min_y.min(rettangolo.min_y);
            max_y = max_y.max(rettangolo.max_y);
        }
        let (larghezza, altezza) = (max_x - min_x, max_y - min_y);
        if rettangoli.is_empty() || !(larghezza.is_finite() && altezza.is_finite()) {
            return None;
        }
        let (gx, gy) = dimensioni_griglia(rettangoli.len(), larghezza, altezza);
        let fattore = |celle: usize, estensione: f64| {
            if estensione > 0.0 {
                // `celle` non supera i rettangoli: esatto in `f64` per ogni
                // insieme in memoria.
                #[allow(clippy::cast_precision_loss)]
                let celle = celle as f64;
                celle / estensione
            } else {
                0.0
            }
        };
        let (fx, fy) = (fattore(gx, larghezza), fattore(gy, altezza));
        (fx.is_finite() && fy.is_finite()).then_some(Self {
            gx,
            gy,
            min_x,
            min_y,
            fx,
            fy,
        })
    }

    /// L'indice di cella, non decrescente nel valore: l'argomento di
    /// `floor` e' non negativo (o `-0.0`) e non NaN, perche' coordinate,
    /// estensione e fattore sono finiti e `v - min` non supera
    /// l'estensione; la conversione satura, e `min` riporta
    /// nell'intervallo.
    fn indice(valore: f64, minimo: f64, fattore: f64, celle: usize) -> usize {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let indice = ((valore - minimo) * fattore).floor() as usize;
        indice.min(celle - 1)
    }

    fn cella_x(&self, valore: f64) -> usize {
        Self::indice(valore, self.min_x, self.fx, self.gx)
    }

    fn cella_y(&self, valore: f64) -> usize {
        Self::indice(valore, self.min_y, self.fy, self.gy)
    }

    /// Le celle estreme di un rettangolo: `(x0, x1, y0, y1)`.
    fn celle_di(&self, rettangolo: &Rettangolo) -> (usize, usize, usize, usize) {
        (
            self.cella_x(rettangolo.min_x),
            self.cella_x(rettangolo.max_x),
            self.cella_y(rettangolo.min_y),
            self.cella_y(rettangolo.max_y),
        )
    }
}

/// Celle per asse, circa `n` in tutto nella proporzione dell'ingombro,
/// ciascuna fra 1 e `n`. Solo prestazioni: la ricerca e' completa con
/// qualunque griglia.
fn dimensioni_griglia(n: usize, larghezza: f64, altezza: f64) -> (usize, usize) {
    let n = n.max(1);
    let gx = if larghezza <= 0.0 {
        1
    } else if altezza <= 0.0 {
        n
    } else {
        // Il rapporto puo' essere infinito o nullo: la conversione satura e
        // `clamp` riporta in [1, n].
        #[allow(
            clippy::cast_precision_loss,
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss
        )]
        let gx = ((n as f64) * (larghezza / altezza)).sqrt().round() as usize;
        gx
    };
    let gx = gx.clamp(1, n);
    let gy = (n / gx).clamp(1, n);
    (gx, gy)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Lo stesso insieme di coppie del doppio ciclo sui rettangoli, ognuna
    /// una volta, su rettangoli casuali su una griglia intera piccola
    /// (tocchi, rettangoli degeneri e coincidenti frequenti) e in scala.
    #[test]
    fn stesse_coppie_del_doppio_ciclo() {
        let mut stato = 0x5EED_CE11_u64;
        let mut prossimo = |limite: u64| {
            stato = stato
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (stato >> 33) % limite
        };
        for caso in 0..400 {
            let quanti = 1 + prossimo(80);
            let lato = 1 + prossimo(20);
            let scala = [1.0, 1e-300, 3.0, 1e290, 0.1][caso % 5];
            let mut rettangoli = Vec::new();
            for _ in 0..quanti {
                #[allow(clippy::cast_precision_loss)]
                let mut valore = || prossimo(lato + 1) as f64 * scala;
                let (a, b, c, d) = (valore(), valore(), valore(), valore());
                rettangoli.push(Rettangolo {
                    min_x: a.min(b),
                    max_x: a.max(b),
                    min_y: c.min(d),
                    max_y: c.max(d),
                });
            }
            let mut attese = Vec::new();
            for i in 0..rettangoli.len() {
                for j in i + 1..rettangoli.len() {
                    if rettangoli[i].si_sovrappone(&rettangoli[j]) {
                        attese.push((i, j));
                    }
                }
            }
            let mut trovate = Vec::new();
            let esito = visita_coppie_sovrapposte(&rettangoli, None, |i, j| {
                trovate.push((i, j));
                true
            });
            assert_eq!(esito, Some(true));
            trovate.sort_unstable();
            assert_eq!(trovate, attese, "caso {caso}");
        }
    }

    #[test]
    fn si_ferma_quando_la_visita_lo_chiede_e_rifiuta_l_estensione_infinita() {
        let quadrato = Rettangolo {
            min_x: 0.0,
            max_x: 1.0,
            min_y: 0.0,
            max_y: 1.0,
        };
        let mut visite = 0;
        let esito = visita_coppie_sovrapposte(&[quadrato; 4], None, |_, _| {
            visite += 1;
            visite < 2
        });
        assert_eq!((esito, visite), (Some(false), 2));
        let enorme = Rettangolo {
            min_x: -f64::MAX,
            max_x: f64::MAX,
            min_y: 0.0,
            max_y: 1.0,
        };
        assert_eq!(
            visita_coppie_sovrapposte(&[enorme, quadrato], None, |_, _| true),
            None
        );
        assert_eq!(visita_coppie_sovrapposte(&[], None, |_, _| true), None);
    }
}
