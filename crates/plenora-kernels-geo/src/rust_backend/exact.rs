//! Segno e confronto **esatti** delle aree degli anelli.
//!
//! Il laboratorio decideva orientamento e annidamento delle facce dal segno
//! e dall'ordine di aree calcolate in `f64` senza compensazione: su
//! coordinate grandi la cancellazione le azzera o le scambia (il quadrato
//! unitario con vertice in `(2^30, 2^30)` ha area calcolata `0`), e la
//! faccia spariva fra gli anelli invalidi senza errore. Qui ogni decisione
//! passa da due stadi:
//!
//! 1. **filtro**: la somma di Gauss in `f64` con un limite d'errore
//!    dimostrato; se il valore supera il limite, il suo segno e' quello vero;
//! 2. **esatto**: altrimenti la stessa somma come espansione non
//!    sovrapposta di Shewchuk (prodotti esatti con `mul_add`, somme esatte
//!    con `two_sum`), il cui segno e' quello del componente piu' grande.
//!
//! Lo stadio esatto vale solo se ogni prodotto e il suo errore sono
//! rappresentabili senza overflow ne' sottoflusso: coordinate nulle o con
//! modulo in `[2^-450, 2^450]`. Fuori da quel dominio, se il filtro non
//! decide, la risposta e' [`FuoriDominio`], mai un segno indovinato.
//!
//! I **valori** delle aree (output, controlli di conservazione dello split)
//! restano quelli di `geo`: qui si decidono solo segni e ordini.

// Il limite d'errore del filtro e' dimostrato per prodotti e somme
// arrotondati separatamente, come li calcola `approssima`: un `mul_add`
// cambierebbe la catena di arrotondamenti analizzata.
#![allow(clippy::suboptimal_flops)]

use std::cmp::Ordering;

use geo::{Coord, Polygon};

/// Le coordinate non consentono l'aritmetica esatta e il filtro non ha
/// deciso.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FuoriDominio;

/// Modulo minimo e massimo delle coordinate non nulle ammesse dallo stadio
/// esatto: `2^-450` e `2^450`. Un prodotto sta in `[2^-900, 2^900]` e il suo
/// errore sopra `2^-1006`, quindi normale.
const MINIMO: f64 = f64::from_bits(573 << 52);
const MASSIMO: f64 = f64::from_bits(1473 << 52);

/// Approssimazione del doppio dell'area orientata con il suo limite d'errore:
/// `|valore - esatto| <= limite`.
#[derive(Clone, Copy, Debug)]
pub struct Approssimazione {
    valore: f64,
    limite: f64,
}

/// Doppio dell'area orientata dell'anello chiuso, in `f64`, con il limite
/// d'errore della somma ricorsiva.
///
/// Ogni termine `a*b - c*d` e ogni accumulo costano al piu' tre
/// arrotondamenti: su `n` finestre la catena ne conta al piu' `3n`, e il
/// limite classico `gamma_k * sum |parti|` con `k = 3n + 2` e' maggiorato
/// raddoppiando `k * EPSILON` (che e' `2u`). Il termine assoluto copre i
/// prodotti che scendono sotto il minimo normale.
fn approssima(anello: &[Coord<f64>]) -> Approssimazione {
    let mut valore = 0.0_f64;
    let mut somma_moduli = 0.0_f64;
    for coppia in anello.windows(2) {
        let primo = coppia[0].x * coppia[1].y;
        let secondo = coppia[1].x * coppia[0].y;
        valore = valore + primo - secondo;
        somma_moduli = somma_moduli + primo.abs() + secondo.abs();
    }
    let finestre = f64::from(u32::try_from(anello.len()).unwrap_or(u32::MAX));
    let passi = 3.0 * finestre + 2.0;
    let limite = 2.0 * passi * f64::EPSILON * somma_moduli + 4.0 * finestre * f64::MIN_POSITIVE;
    Approssimazione {
        valore,
        limite: if limite.is_finite() && valore.is_finite() {
            limite
        } else {
            f64::INFINITY
        },
    }
}

fn two_sum(a: f64, b: f64) -> (f64, f64) {
    let somma = a + b;
    let virtuale = somma - a;
    let errore = (a - (somma - virtuale)) + (b - virtuale);
    (somma, errore)
}

fn two_product(a: f64, b: f64) -> (f64, f64) {
    let prodotto = a * b;
    // `mul_add` e' un FMA con un solo arrotondamento: l'errore del prodotto
    // e' esatto quando e' rappresentabile (garantito dal dominio).
    let errore = a.mul_add(b, -prodotto);
    (prodotto, errore)
}

/// Aggiunge `valore` a un'espansione non sovrapposta (Grow-Expansion di
/// Shewchuk con eliminazione degli zeri).
fn accresci(espansione: &mut Vec<f64>, valore: f64) {
    let mut q = valore;
    let mut uscita = Vec::with_capacity(espansione.len() + 1);
    for &componente in espansione.iter() {
        let (somma, errore) = two_sum(q, componente);
        if errore != 0.0 {
            uscita.push(errore);
        }
        q = somma;
    }
    if q != 0.0 {
        uscita.push(q);
    }
    *espansione = uscita;
}

fn nel_dominio(coordinata: f64) -> bool {
    coordinata == 0.0 || (coordinata.abs() >= MINIMO && coordinata.abs() <= MASSIMO)
}

/// Il doppio dell'area orientata come espansione esatta, con segno
/// `verso` (`1.0` o `-1.0`, negazione esatta), accumulata in `espansione`.
fn accumula_esatto(
    espansione: &mut Vec<f64>,
    anello: &[Coord<f64>],
    verso: f64,
) -> Result<(), FuoriDominio> {
    if !anello.iter().all(|c| nel_dominio(c.x) && nel_dominio(c.y)) {
        return Err(FuoriDominio);
    }
    for coppia in anello.windows(2) {
        let (p, e) = two_product(coppia[0].x, coppia[1].y);
        let (q, f) = two_product(coppia[1].x, coppia[0].y);
        for parte in [p, e, -q, -f] {
            accresci(espansione, verso * parte);
        }
    }
    Ok(())
}

/// Segno di un'espansione non sovrapposta: quello del componente piu'
/// grande, cioe' l'ultimo.
fn segno(espansione: &[f64]) -> Ordering {
    espansione
        .last()
        .map_or(Ordering::Equal, |ultimo| ultimo.total_cmp(&0.0))
}

fn segno_approssimato(approssimazione: Approssimazione) -> Option<Ordering> {
    (approssimazione.valore.abs() > approssimazione.limite)
        .then(|| approssimazione.valore.total_cmp(&0.0))
}

/// Segno esatto del doppio dell'area orientata dell'anello chiuso: `Greater`
/// antiorario, `Less` orario, `Equal` solo se l'area e' davvero nulla.
///
/// # Errors
///
/// [`FuoriDominio`] se il filtro non decide e le coordinate escono dal
/// dominio dello stadio esatto.
pub fn orientamento(anello: &[Coord<f64>]) -> Result<Ordering, FuoriDominio> {
    if let Some(segno) = segno_approssimato(approssima(anello)) {
        return Ok(segno);
    }
    let mut espansione = Vec::new();
    accumula_esatto(&mut espansione, anello, 1.0)?;
    Ok(segno(&espansione))
}

/// Area (doppia, senza segno) di un poligono: esterno meno buchi, come
/// `Area::unsigned_area` di `geo`, con il limite d'errore.
#[derive(Clone, Copy, Debug)]
pub struct AreaPoligono(Approssimazione);

impl AreaPoligono {
    /// L'area di `poligono` con il suo limite d'errore.
    #[must_use]
    pub fn di(poligono: &Polygon<f64>) -> Self {
        let esterno = approssima(&poligono.exterior().0);
        let mut valore = esterno.valore.abs();
        let mut limite = esterno.limite;
        let mut moduli = valore;
        for buco in poligono.interiors() {
            let approssimazione = approssima(&buco.0);
            valore -= approssimazione.valore.abs();
            limite += approssimazione.limite;
            moduli += approssimazione.valore.abs();
        }
        // Ogni sottrazione fra anelli arrotonda una volta, su parziali mai
        // piu' grandi della somma dei moduli.
        let anelli = f64::from(u32::try_from(poligono.interiors().len()).unwrap_or(u32::MAX));
        limite += 2.0 * (anelli + 1.0) * f64::EPSILON * moduli;
        Self(Approssimazione {
            valore,
            limite: if limite.is_finite() {
                limite
            } else {
                f64::INFINITY
            },
        })
    }
}

/// Accumula `verso` volte l'area senza segno esatta del poligono.
fn accumula_poligono(
    espansione: &mut Vec<f64>,
    poligono: &Polygon<f64>,
    verso: f64,
) -> Result<(), FuoriDominio> {
    for (indice, anello) in std::iter::once(poligono.exterior())
        .chain(poligono.interiors())
        .enumerate()
    {
        // |area| = area * segno(area); i buchi si sottraggono.
        let segno_anello = match orientamento(&anello.0)? {
            Ordering::Less => -1.0,
            Ordering::Equal => continue,
            Ordering::Greater => 1.0,
        };
        let ruolo = if indice == 0 { 1.0 } else { -1.0 };
        accumula_esatto(espansione, &anello.0, verso * segno_anello * ruolo)?;
    }
    Ok(())
}

/// Se l'area senza segno del poligono (esterno meno buchi) e' positiva, in
/// modo esatto.
///
/// # Errors
///
/// [`FuoriDominio`] se il filtro non decide e le coordinate escono dal
/// dominio dello stadio esatto.
pub fn area_positiva(poligono: &Polygon<f64>) -> Result<bool, FuoriDominio> {
    let area = AreaPoligono::di(poligono).0;
    if area.valore.is_finite() && area.valore.abs() > area.limite {
        return Ok(area.valore > 0.0);
    }
    let mut espansione = Vec::new();
    accumula_poligono(&mut espansione, poligono, 1.0)?;
    Ok(segno(&espansione) == Ordering::Greater)
}

/// Confronto esatto delle aree senza segno di due poligoni.
///
/// # Errors
///
/// [`FuoriDominio`] se il filtro non decide e le coordinate escono dal
/// dominio dello stadio esatto.
pub fn confronta_aree(
    primo: &Polygon<f64>,
    area_primo: AreaPoligono,
    secondo: &Polygon<f64>,
    area_secondo: AreaPoligono,
) -> Result<Ordering, FuoriDominio> {
    let differenza = area_primo.0.valore - area_secondo.0.valore;
    let limite = area_primo.0.limite + area_secondo.0.limite + differenza.abs() * f64::EPSILON;
    if differenza.is_finite() && differenza.abs() > limite {
        return Ok(differenza.total_cmp(&0.0));
    }
    let mut espansione = Vec::new();
    accumula_poligono(&mut espansione, primo, 1.0)?;
    accumula_poligono(&mut espansione, secondo, -1.0)?;
    Ok(segno(&espansione))
}

// Le attese esatte si confrontano esatte.
#[cfg(test)]
#[allow(clippy::float_cmp)]
mod tests {
    use super::*;
    use geo::{polygon, LineString};

    fn quadrato(x: f64, y: f64, lato: f64) -> Vec<Coord<f64>> {
        vec![
            Coord { x, y },
            Coord { x: x + lato, y },
            Coord {
                x: x + lato,
                y: y + lato,
            },
            Coord { x, y: y + lato },
            Coord { x, y },
        ]
    }

    #[test]
    fn i_limiti_del_dominio_sono_le_potenze_dichiarate() {
        assert_eq!(MINIMO, 2_f64.powi(-450));
        assert_eq!(MASSIMO, 2_f64.powi(450));
    }

    /// Il caso limite: area esatta 1, somma ingenua 0.
    #[test]
    fn il_quadrato_unitario_a_2_alla_30_e_antiorario() {
        let a = 2_f64.powi(30);
        let anello = quadrato(a, a, 1.0);
        let ingenua = anello
            .windows(2)
            .fold(0.0, |area, c| area + c[0].x * c[1].y - c[1].x * c[0].y);
        assert_eq!(ingenua, 0.0, "la cancellazione che il filtro deve vedere");
        assert_eq!(orientamento(&anello), Ok(Ordering::Greater));
        let mut inverso = anello;
        inverso.reverse();
        assert_eq!(orientamento(&inverso), Ok(Ordering::Less));
    }

    #[test]
    fn ogni_vertice_iniziale_da_lo_stesso_segno() {
        // Fino a 2^52: oltre, `a + 1` non e' piu' rappresentabile.
        for esponente in [0, 20, 30, 40, 50, 52] {
            let a = 2_f64.powi(esponente);
            let base = quadrato(a, -a, 1.0);
            let aperto = &base[..4];
            for inizio in 0..4 {
                let mut anello: Vec<_> = (0..4).map(|i| aperto[(inizio + i) % 4]).collect();
                anello.push(anello[0]);
                assert_eq!(
                    orientamento(&anello),
                    Ok(Ordering::Greater),
                    "2^{esponente}"
                );
                anello.reverse();
                assert_eq!(orientamento(&anello), Ok(Ordering::Less), "2^{esponente}");
            }
        }
    }

    #[test]
    fn zero_solo_per_l_anello_davvero_degenere() {
        let a = 2_f64.powi(40);
        let collineare = vec![
            Coord { x: a, y: a },
            Coord {
                x: a + 1.0,
                y: a + 1.0,
            },
            Coord {
                x: a + 2.0,
                y: a + 2.0,
            },
            Coord { x: a, y: a },
        ];
        assert_eq!(orientamento(&collineare), Ok(Ordering::Equal));
    }

    #[test]
    fn fuori_dominio_e_un_errore_non_un_segno() {
        let a = 2_f64.powi(500);
        let anello = quadrato(a, a, 2_f64.powi(448));
        assert_eq!(orientamento(&anello), Err(FuoriDominio));
        // Il filtro decide da solo quando l'area domina l'errore.
        assert_eq!(
            orientamento(&quadrato(0.0, 0.0, 2_f64.powi(500))),
            Ok(Ordering::Greater)
        );
    }

    #[test]
    fn confronto_esatto_di_aree_quasi_uguali_a_grande_distanza() {
        let a = 2_f64.powi(30);
        let grande = Polygon::new(LineString::new(quadrato(a, a, 2.0)), Vec::new());
        let piccolo = Polygon::new(LineString::new(quadrato(a, a, 1.0)), Vec::new());
        let area = AreaPoligono::di;
        assert_eq!(
            confronta_aree(&grande, area(&grande), &piccolo, area(&piccolo)),
            Ok(Ordering::Greater)
        );
        assert_eq!(
            confronta_aree(&piccolo, area(&piccolo), &grande, area(&grande)),
            Ok(Ordering::Less)
        );
        assert_eq!(
            confronta_aree(&piccolo, area(&piccolo), &piccolo, area(&piccolo)),
            Ok(Ordering::Equal)
        );
        let con_buco = polygon!(
            exterior: [(x: 0.0, y: 0.0), (x: 4.0, y: 0.0), (x: 4.0, y: 4.0), (x: 0.0, y: 4.0), (x: 0.0, y: 0.0)],
            interiors: [[(x: 1.0, y: 1.0), (x: 3.0, y: 1.0), (x: 3.0, y: 3.0), (x: 1.0, y: 3.0), (x: 1.0, y: 1.0)]],
        );
        // Area 16 - 4 = 12 contro un quadrato di lato s = fl(sqrt(12)): s^2
        // non e' 12, e il segno di 12 - s^2 si legge dal prodotto esatto.
        let lato = 12_f64.sqrt();
        let pieno = Polygon::new(LineString::new(quadrato(0.0, 0.0, lato)), Vec::new());
        let (prodotto, errore) = two_product(lato, lato);
        let atteso = if prodotto == 12.0 {
            0.0_f64.total_cmp(&errore)
        } else {
            12.0_f64.total_cmp(&prodotto)
        };
        assert_ne!(atteso, Ordering::Equal);
        assert_eq!(
            confronta_aree(&con_buco, area(&con_buco), &pieno, area(&pieno)),
            Ok(atteso)
        );
    }
}
