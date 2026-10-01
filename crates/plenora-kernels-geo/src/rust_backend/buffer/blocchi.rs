//! Buffer a blocchi delle linee: il tratto di una linea i cui offset si
//! sovrappongono su molti segmenti lontani si calcola per blocchi di
//! segmenti consecutivi, poi i buffer dei blocchi si uniscono a coppie.
//!
//! **Perche'.** Il tratto di `i_overlay` (`stroke_as`) mette in un solo
//! overlay i contorni di tutti i segmenti e di tutte le giunzioni, e calcola
//! ogni incrocio fra loro prima di scegliere il bordo: su una linea a
//! zig-zag stretta rispetto alla distanza (1.000 vertici distanti 0,8 m,
//! buffer di 200 m) ogni contorno ne attraversa centinaia, e il calcolo ha
//! misurato 21 GiB e 110 s per una geometria di 16 KB. A blocchi di
//! [`SEGMENTI_PER_BLOCCO`] segmenti ogni tratto ha pochi incroci, e
//! l'unione di due blocchi vicini attraversa solo i loro bordi, che la
//! sovrapposizione ha gia' ridotto.
//!
//! **Perche' il risultato e' lo stesso buffer.** I blocchi si sovrappongono
//! di un segmento (il blocco `k + 1` comincia dal penultimo vertice del
//! blocco `k`), e quel segmento condiviso non degenera sulla griglia di
//! `i_overlay` (sotto). Allora:
//!
//! - ogni segmento della linea sta in almeno un blocco, e ogni vertice
//!   interno della linea e' interno ad almeno un blocco, con i suoi due
//!   segmenti: l'ultimo vertice del blocco `k` e' interno al blocco `k + 1`
//!   (che ne contiene il segmento precedente, quello condiviso, e il
//!   successivo), il primo del blocco `k + 1` e' interno al blocco `k`;
//! - con estremita' **piatte** il buffer di una linea con giunzioni tonde e'
//!   l'unione dei rettangoli dei suoi segmenti (i punti che si proiettano
//!   nel segmento, entro `|d|`) e dei settori di giunzione sui vertici
//!   interni (il settore di raggio `|d|` fra le normali dei due segmenti, dal
//!   lato esterno della svolta), che dipendono solo dai due segmenti del
//!   vertice. Ogni blocco ha rettangoli e settori della linea, quindi il suo
//!   buffer sta nel buffer della linea; ogni rettangolo e ogni settore della
//!   linea e' di un blocco (punto sopra): l'unione dei buffer dei blocchi
//!   e' il buffer della linea, anche con segmenti cortissimi (nessun disco
//!   intero ai vertici, solo i settori);
//! - con estremita' **tonde** e giunzioni tonde il buffer e' la somma di
//!   Minkowski della linea con il disco di raggio `|d|`, che si distribuisce
//!   sull'unione: la linea e' l'unione dei blocchi, e il buffer l'unione dei
//!   loro buffer.
//!
//! Le estremita' **quadrate** sporgerebbero di `|d|` oltre il vertice
//! condiviso: con quelle si resta al tratto unico. I punti ripetuti
//! consecutivi si tolgono prima (stesso insieme di punti, stesso buffer).
//!
//! **Segmenti sotto la griglia.** `i_overlay` porta le coordinate su interi
//! di passo `g` e scarta i segmenti che vi degenerano: rettangoli e settori
//! sono quelli della linea senza quei segmenti. Il segmento condiviso fra
//! due blocchi si sceglie con almeno una componente oltre `2 g` (il
//! blocco si allunga finche' non lo trova; senza, resta un blocco solo),
//! quindi non degenera (la griglia di un blocco, sul suo ingombro, non e'
//! piu' larga di `g`), e i settori ai suoi estremi sono fra lui e i
//! segmenti non degeneri vicini, come nel tratto unico. Nessun blocco
//! degenera per intero (contiene quel segmento). Un segmento sotto la
//! griglia dentro un blocco puo' essere tenuto o scartato diversamente dal
//! tratto unico (la griglia di un blocco e' piu' fine di quella della
//! linea): e' una feature piu' vicina della precisione, fuori dalla
//! garanzia come in ogni overlay (README, «Feature d'ingresso piu' vicine
//! della precisione»).
//!
//! **Scostamento dal tratto unico.** Ogni buffer di blocco sta entro
//! `p / 2` dal buffer esatto del blocco verso l'esterno ed entro `f + p /
//! 2` verso l'interno lungo gli archi (`super`), e l'unione conserva i due
//! limiti: un punto a distanza `|d| - f - p / 2` dalla linea sta a quella
//! distanza da un segmento, e quindi dal blocco che lo contiene; un punto
//! dell'unione sta in un buffer di blocco, quindi entro `|d| + p / 2` dalla
//! linea. Il risultato differisce dal tratto unico entro la stessa fascia
//! (gli archi dei blocchi cominciano da angoli diversi, e la loro unione e'
//! piu' vicina al cerchio), mai oltre `f + p`. Gli overlay delle unioni sono
//! in catena con il tratto: `super::buffer_con_freccia` li mette nel
//! bilancio della griglia prima del calcolo.

use geo::algorithm::buffer::BufferStyle;
use geo::{BooleanOps, Buffer, Coord, Geometry, LineString, MultiPolygon};

use crate::margine::MargineMemoria;

/// I segmenti di un blocco. Misurato sul zig-zag di 1.000 vertici con
/// buffer di 200 m: 2, 4, 8, 16, 32 segmenti costano 29, 21, 24, 37, 62
/// ms (release, un thread).
pub(super) const SEGMENTI_PER_BLOCCO: usize = 8;

/// Le coppie di segmenti piu' vicine di cosi' nella stessa linea non
/// contano nella previsione del tratto unico: i loro offset si toccano
/// anche su una linea liscia senza incrociarsi molto.
const VICINI_ESCLUSI: usize = 2 * SEGMENTI_PER_BLOCCO;

/// Le coppie di segmenti lontani a rettangoli allargati sovrapposti oltre
/// cui il tratto unico si considera troppo costoso: 4 per segmento, e
/// almeno 4.096.
const COPPIE_PER_SEGMENTO: usize = 4;
const COPPIE_MINIME: usize = 4_096;

/// I confronti della scansione oltre cui si smette di contare (e il tratto
/// unico si considera troppo costoso): 64 per segmento, e almeno 65.536.
const CONFRONTI_PER_SEGMENTO: usize = 64;
const CONFRONTI_MINIMI: usize = 65_536;

/// Le linee di una geometria lineare (`LineString`, `MultiLineString`);
/// `None` per gli altri tipi, che restano al buffer di `geo`.
pub(super) fn linee_di(geometry: &Geometry<f64>) -> Option<Vec<&LineString<f64>>> {
    match geometry {
        Geometry::LineString(linea) => Some(vec![linea]),
        Geometry::MultiLineString(linee) => Some(linee.0.iter().collect()),
        _ => None,
    }
}

/// Se il tratto unico delle `linee` a distanza `distanza` sarebbe troppo
/// costoso: le coppie di segmenti a rettangoli allargati di `|d|`
/// sovrapposti, escluse quelle vicine nella stessa linea
/// ([`VICINI_ESCLUSI`]), superano [`COPPIE_PER_SEGMENTO`] per segmento (o i
/// confronti della scansione [`CONFRONTI_PER_SEGMENTO`]). E' un maggiorante
/// degli incroci fra gli offset di segmenti lontani, che e' cio' che fa
/// esplodere il tratto unico. Solo la scelta dell'algoritmo: i due
/// risultati stanno nella stessa fascia (vedi il modulo). Deterministico:
/// dipende solo dalle coordinate.
pub(super) fn tratto_unico_troppo_costoso(linee: &[&LineString<f64>], distanza: f64) -> bool {
    let margine = distanza.abs();
    // (min e max sull'asse di scansione, min e max sull'altro, linea, indice).
    let mut rettangoli: Vec<(f64, f64, f64, f64, usize, usize)> = Vec::new();
    for (numero, linea) in linee.iter().enumerate() {
        for (indice, segmento) in linea.lines().enumerate() {
            rettangoli.push((
                segmento.start.x.min(segmento.end.x) - margine,
                segmento.start.x.max(segmento.end.x) + margine,
                segmento.start.y.min(segmento.end.y) - margine,
                segmento.start.y.max(segmento.end.y) + margine,
                numero,
                indice,
            ));
        }
    }
    let segmenti = rettangoli.len();
    if segmenti <= VICINI_ESCLUSI {
        return false;
    }
    // Scansione sull'asse dove i rettangoli sono piu' sottili rispetto
    // all'estensione (solo prestazioni del conteggio).
    let (mut larghezze, mut altezze) = (0.0_f64, 0.0_f64);
    let (mut min_x, mut max_x, mut min_y, mut max_y) = (f64::MAX, f64::MIN, f64::MAX, f64::MIN);
    for r in &rettangoli {
        larghezze += r.1 - r.0;
        altezze += r.3 - r.2;
        min_x = min_x.min(r.0);
        max_x = max_x.max(r.1);
        min_y = min_y.min(r.2);
        max_y = max_y.max(r.3);
    }
    if larghezze * (max_y - min_y) > altezze * (max_x - min_x) {
        for r in &mut rettangoli {
            *r = (r.2, r.3, r.0, r.1, r.4, r.5);
        }
    }
    rettangoli.sort_by(|a, b| a.0.total_cmp(&b.0));
    let limite_coppie = segmenti
        .saturating_mul(COPPIE_PER_SEGMENTO)
        .max(COPPIE_MINIME);
    let limite_confronti = segmenti
        .saturating_mul(CONFRONTI_PER_SEGMENTO)
        .max(CONFRONTI_MINIMI);
    let (mut coppie, mut confronti) = (0_usize, 0_usize);
    for (posizione, a) in rettangoli.iter().enumerate() {
        for b in &rettangoli[posizione + 1..] {
            // Un NaN (coordinate non finite) non interrompe: conta come
            // sovrapposto, per eccesso.
            if b.0 > a.1 {
                break;
            }
            confronti += 1;
            let lontani = a.4 != b.4 || a.5.abs_diff(b.5) > VICINI_ESCLUSI;
            if lontani && !(b.2 > a.3 || a.2 > b.3) {
                coppie += 1;
            }
            if coppie > limite_coppie || confronti > limite_confronti {
                return true;
            }
        }
    }
    false
}

/// I blocchi di una linea: senza punti ripetuti consecutivi, al piu'
/// [`SEGMENTI_PER_BLOCCO`] segmenti, sovrapposti di un segmento (vedi il
/// modulo). Una linea di un solo punto distinto resta un blocco di un
/// punto: `geo` ne fa il buffer di un punto, come per la linea intera.
fn blocchi(linea: &LineString<f64>, passo: f64) -> Vec<LineString<f64>> {
    let mut punti: Vec<Coord<f64>> = Vec::with_capacity(linea.0.len());
    for punto in &linea.0 {
        if punti.last() != Some(punto) {
            punti.push(*punto);
        }
    }
    if punti.len() <= SEGMENTI_PER_BLOCCO + 1 {
        return vec![LineString::new(punti)];
    }
    let ultimo = punti.len() - 1;
    // Il segmento che finisce in `i` non degenera sulla griglia di passo
    // `passo`: una componente oltre `2 passo` (vedi il modulo).
    let lungo = |i: usize| {
        let (a, b) = (punti[i - 1], punti[i]);
        (b.x - a.x).abs().max((b.y - a.y).abs()) > 2.0 * passo
    };
    let mut out = Vec::with_capacity(punti.len() / (SEGMENTI_PER_BLOCCO - 1) + 1);
    let mut inizio = 0;
    loop {
        let mut fine = inizio + SEGMENTI_PER_BLOCCO;
        while fine < ultimo && !lungo(fine) {
            fine += 1;
        }
        if fine >= ultimo {
            out.push(LineString::new(punti[inizio..].to_vec()));
            break;
        }
        out.push(LineString::new(punti[inizio..=fine].to_vec()));
        inizio = fine - 1;
    }
    out
}

/// I blocchi di tutte le linee, nell'ordine delle linee, con il passo
/// della griglia di `i_overlay`; le linee vuote non ne hanno.
pub(super) fn blocchi_di(linee: &[&LineString<f64>], passo: f64) -> Vec<LineString<f64>> {
    linee
        .iter()
        .filter(|linea| !linea.0.is_empty())
        .flat_map(|linea| blocchi(linea, passo))
        .collect()
}

/// I livelli dell'unione a coppie di `parti` buffer: `ceil(log2(parti))`.
pub(super) fn livelli_di_unione(parti: usize) -> u32 {
    parti.max(1).next_power_of_two().trailing_zeros()
}

/// Il buffer di ogni blocco con lo stile dato, poi l'unione a coppie di
/// blocchi adiacenti, livello per livello ([`livelli_di_unione`] livelli,
/// ordine deterministico); ogni calcolo di `geo` dietro la barriera dei
/// panici.
///
/// **Memoria.** Si contano i byte vivi delle parti (capacita' vere,
/// [`crate::margine::byte_multipoligono`]) e dei vettori che le tengono, a
/// capacita' esatta: ogni buffer di blocco e ogni unione appena calcolati
/// devono stare nel `margine` insieme a cio' che e' ancora vivo, prima di
/// essere tenuti; un'unione libera i due operandi. Il transitorio dentro
/// `geo` e `i_overlay` resta fuori (README, «Modelli di costo geo»).
pub(super) fn buffer_dei_blocchi(
    blocchi: &[LineString<f64>],
    stile: &BufferStyle<f64>,
    margine: MargineMemoria,
) -> Result<MultiPolygon<f64>, super::ErroreBuffer> {
    let byte = crate::margine::byte_multipoligono;
    let elemento = std::mem::size_of::<MultiPolygon<f64>>() as u64;
    let alloca = |quante: usize| {
        let mut parti: Vec<MultiPolygon<f64>> = Vec::new();
        parti
            .try_reserve_exact(quante)
            .map(|()| parti)
            .map_err(|_| super::ErroreBuffer::CalcoloNonConcluso("allocazione dei blocchi"))
    };
    let quante = |n: usize| u64::try_from(n).unwrap_or(u64::MAX);
    let mut parti = alloca(blocchi.len())?;
    let mut vive = quante(parti.capacity()).saturating_mul(elemento);
    margine.verifica(vive)?;
    for blocco in blocchi {
        let buffer = super::protetto(|| blocco.buffer_with_style(stile.clone()))?;
        vive = vive.saturating_add(byte(&buffer));
        margine.verifica(vive)?;
        parti.push(buffer);
    }
    while parti.len() > 1 {
        let mut prossime = alloca(parti.len().div_ceil(2))?;
        vive = vive.saturating_add(quante(prossime.capacity()).saturating_mul(elemento));
        margine.verifica(vive)?;
        let vecchie = quante(parti.capacity()).saturating_mul(elemento);
        let mut resto = parti.into_iter();
        while let Some(a) = resto.next() {
            match resto.next() {
                Some(b) => {
                    let unione = super::protetto(|| a.union(&b))?;
                    let nuovi = byte(&unione);
                    margine.verifica(vive.saturating_add(nuovi))?;
                    vive = vive
                        .saturating_add(nuovi)
                        .saturating_sub(byte(&a))
                        .saturating_sub(byte(&b));
                    prossime.push(unione);
                }
                None => prossime.push(a),
            }
        }
        vive = vive.saturating_sub(vecchie);
        parti = prossime;
    }
    Ok(parti.pop().unwrap_or_else(|| MultiPolygon::new(Vec::new())))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn zigzag(vertici: usize, passo: f64, altezza: f64) -> LineString<f64> {
        LineString::new(
            (0..vertici)
                .map(|j| {
                    #[allow(clippy::cast_precision_loss)]
                    let x = j as f64 * passo;
                    Coord {
                        x,
                        y: if j % 2 == 0 { 0.0 } else { altezza },
                    }
                })
                .collect(),
        )
    }

    #[test]
    fn i_blocchi_coprono_ogni_segmento_e_ogni_vertice_interno() {
        for vertici in [1, 2, 3, 9, 10, 11, 17, 100, 101] {
            let linea = zigzag(vertici, 1.0, 5.0);
            let parti = blocchi(&linea, 0.0);
            // Il primo e l'ultimo punto, e la sovrapposizione di un
            // segmento fra blocchi consecutivi.
            assert_eq!(parti.first().and_then(|b| b.0.first()), linea.0.first());
            assert_eq!(parti.last().and_then(|b| b.0.last()), linea.0.last());
            for coppia in parti.windows(2) {
                let (a, b) = (&coppia[0].0, &coppia[1].0);
                assert_eq!(a[a.len() - 2..], b[..2]);
                assert!(a.len() >= 3 && b.len() >= 3);
            }
            for parte in &parti {
                assert!(parte.0.len() <= SEGMENTI_PER_BLOCCO + 1);
            }
        }
    }

    #[test]
    fn i_punti_ripetuti_consecutivi_si_tolgono() {
        let linea = LineString::from(vec![(0.0, 0.0), (0.0, 0.0), (1.0, 0.0), (1.0, 0.0)]);
        assert_eq!(
            blocchi(&linea, 0.0),
            vec![LineString::from(vec![(0.0, 0.0), (1.0, 0.0)])]
        );
        let punto = LineString::from(vec![(2.0, 3.0), (2.0, 3.0)]);
        assert_eq!(
            blocchi(&punto, 0.0),
            vec![LineString::from(vec![(2.0, 3.0)])]
        );
    }

    #[test]
    fn la_previsione_separa_zig_zag_e_linee_lisce() {
        // Zig-zag stretto rispetto alla distanza: ogni offset ne attraversa
        // centinaia.
        let stretto = zigzag(1_000, 0.8, 600.0);
        assert!(tratto_unico_troppo_costoso(&[&stretto], 200.0));
        assert!(!tratto_unico_troppo_costoso(&[&stretto], 0.1));
        // Una linea liscia densa: i vicini nella linea non contano.
        let liscia = LineString::new(
            (0_i32..10_000)
                .map(|j| {
                    let t = f64::from(j) * 1e-3;
                    Coord {
                        x: 1_000.0 * t,
                        y: 50.0 * (t * 3.0).sin(),
                    }
                })
                .collect(),
        );
        assert!(!tratto_unico_troppo_costoso(&[&liscia], 5.0));
    }

    #[test]
    fn livelli() {
        assert_eq!(livelli_di_unione(0), 0);
        assert_eq!(livelli_di_unione(1), 0);
        assert_eq!(livelli_di_unione(2), 1);
        assert_eq!(livelli_di_unione(3), 2);
        assert_eq!(livelli_di_unione(4), 2);
        assert_eq!(livelli_di_unione(125), 7);
    }
    /// Oracolo del buffer a blocchi (AGENTS.md, regola 3) sul percorso di
    /// produzione (`buffer_controllato`, che sceglie i blocchi), contro la
    /// definizione e contro il tratto unico di `geo` (il percorso generico):
    ///
    /// - verso l'esterno ogni vertice del risultato sta entro `|d| + p / 2`
    ///   dalla linea;
    /// - verso l'interno i punti a `|d| - f - p` dalla linea (offset dei
    ///   punti medi dei segmenti e, con estremita' tonde, raggi attorno ai
    ///   vertici) stanno nel risultato;
    /// - i bordi dei due risultati distano al piu' `f + p` l'uno dall'altro
    ///   (vertice per vertice, nei due versi).
    #[test]
    #[allow(clippy::too_many_lines)]
    fn oracolo_a_blocchi_contro_tratto_unico_e_definizione() {
        use geo::algorithm::buffer::{LineCap, LineJoin};
        use geo::{Distance, Euclidean, Intersects, Line, MultiLineString, Point};

        use super::super::{
            angolo_degli_archi, buffer_controllato, freccia_degli_archi, Estremita,
            CONTRAZIONE_ARCHI,
        };
        use crate::rust_backend::precision::Precision;

        struct Lcg(u64);
        impl Lcg {
            fn unitario(&mut self) -> f64 {
                self.0 = self
                    .0
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1_442_695_040_888_963_407);
                #[allow(clippy::cast_precision_loss)]
                let valore = (self.0 >> 11) as f64 / (1_u64 << 53) as f64;
                valore
            }
        }

        fn sposta(linea: &LineString<f64>) -> LineString<f64> {
            LineString::new(
                linea
                    .0
                    .iter()
                    .map(|c| Coord {
                        x: c.x + 1_000_000.0,
                        y: c.y + 5_000_000.0,
                    })
                    .collect(),
            )
        }

        fn segmenti_del_bordo(poligoni: &MultiPolygon<f64>) -> Vec<Line<f64>> {
            poligoni
                .iter()
                .flat_map(|p| std::iter::once(p.exterior()).chain(p.interiors()))
                .flat_map(LineString::lines)
                .collect()
        }

        fn distanza(punto: Point<f64>, segmenti: &[Line<f64>]) -> f64 {
            segmenti
                .iter()
                .map(|s| Euclidean.distance(&punto, s))
                .fold(f64::INFINITY, f64::min)
        }

        let p = Precision::new(0.01).expect("1 cm");
        let mut rng = Lcg(0x5EED_B10C);
        let cammino: LineString<f64> = LineString::new(
            (0..150)
                .map(|_| Coord {
                    x: 100.0 * rng.unitario(),
                    y: 100.0 * rng.unitario(),
                })
                .collect(),
        );
        let mut chiuso = zigzag(120, 0.8, 300.0);
        chiuso.0.push(chiuso.0[0]);
        let incrociato = LineString::new(
            zigzag(120, 0.8, 300.0)
                .0
                .iter()
                .map(|c| Coord {
                    x: c.y / 3.0,
                    y: c.x * 3.0,
                })
                .collect(),
        );
        // Segmenti cortissimi: dopo ogni vertice un punto a 1 micron, in una
        // direzione che ruota (svolte strette su segmenti sotto la precisione
        // ma sopra la griglia).
        let mut corti = Vec::new();
        for (k, c) in zigzag(150, 0.8, 600.0).0.into_iter().enumerate() {
            corti.push(c);
            #[allow(clippy::cast_precision_loss)]
            let a = k as f64 * 0.7;
            corti.push(Coord {
                x: 1e-6f64.mul_add(a.cos(), c.x),
                y: 1e-6f64.mul_add(a.sin(), c.y),
            });
        }
        let corti = LineString::new(corti);
        let casi: Vec<(Geometry<f64>, f64)> = vec![
            (
                Geometry::LineString(sposta(&zigzag(150, 0.8, 600.0))),
                200.0,
            ),
            (Geometry::LineString(sposta(&corti)), 120.0),
            (Geometry::LineString(sposta(&zigzag(200, 0.8, 600.0))), 50.0),
            (Geometry::LineString(sposta(&cammino)), 30.0),
            (Geometry::LineString(sposta(&chiuso)), 100.0),
            (
                Geometry::MultiLineString(MultiLineString::new(vec![
                    sposta(&zigzag(120, 0.8, 300.0)),
                    sposta(&incrociato),
                ])),
                40.0,
            ),
        ];
        for (geometria, d) in &casi {
            let linee = linee_di(geometria).expect("lineare");
            assert!(
                tratto_unico_troppo_costoso(&linee, *d),
                "il caso deve andare a blocchi"
            );
            let segmenti_linea: Vec<Line<f64>> = linee.iter().flat_map(|l| l.lines()).collect();
            let f = freccia_degli_archi(*d, p);
            for (estremita, tonde) in [(Estremita::Tonde, true), (Estremita::Piatte, false)] {
                let a_blocchi = buffer_controllato(geometria, *d, estremita, p).expect("buffer");
                let angolo = angolo_degli_archi(*d, CONTRAZIONE_ARCHI.mul_add(-d, f));
                let stile = BufferStyle::new(*d)
                    .line_join(LineJoin::Round(angolo))
                    .line_cap(if tonde {
                        LineCap::Round(angolo)
                    } else {
                        LineCap::Butt
                    });
                let unico = geometria.buffer_with_style(stile);

                let bordo_blocchi = segmenti_del_bordo(&a_blocchi);
                let bordo_unico = segmenti_del_bordo(&unico);
                for segmento in &bordo_blocchi {
                    let vertice = Point(segmento.start);
                    let fuori = distanza(vertice, &segmenti_linea);
                    assert!(
                        fuori <= d + 0.5 * p.value(),
                        "verso l'esterno: {fuori} su {d}"
                    );
                    let scarto = distanza(vertice, &bordo_unico);
                    assert!(scarto <= f + p.value(), "dal tratto unico: {scarto}");
                }
                for segmento in &bordo_unico {
                    let scarto = distanza(Point(segmento.start), &bordo_blocchi);
                    assert!(scarto <= f + p.value(), "dal tratto unico: {scarto}");
                }
                // Verso l'interno: punti a `|d| - f - p` dalla linea.
                let r = d - f - p.value();
                for segmento in &segmenti_linea {
                    let (dx, dy) = (segmento.dx(), segmento.dy());
                    let lunghezza = dx.hypot(dy);
                    if lunghezza == 0.0 {
                        continue;
                    }
                    let medio = segmento.start + segmento.delta() * 0.5;
                    for verso in [-1.0, 1.0] {
                        let interno = Point::new(
                            (-dy / lunghezza * verso).mul_add(r, medio.x),
                            (dx / lunghezza * verso).mul_add(r, medio.y),
                        );
                        assert!(a_blocchi.intersects(&interno), "punto interno mancante");
                    }
                    if tonde {
                        for k in 0..8 {
                            let direzione = f64::from(k) * std::f64::consts::FRAC_PI_4;
                            let interno = Point::new(
                                direzione.cos().mul_add(r, segmento.start.x),
                                direzione.sin().mul_add(r, segmento.start.y),
                            );
                            assert!(a_blocchi.intersects(&interno), "disco del vertice");
                        }
                    }
                }
            }
        }
    }

    /// Sotto la griglia: segmenti che degenerano sul passo di `i_overlay`
    /// (componenti di `1e-20` vicino all'origine) non sono mai il segmento
    /// condiviso fra due blocchi, e nessun blocco e' fatto solo di loro
    /// (vedi il modulo, «Segmenti sotto la griglia»).
    #[test]
    fn il_segmento_condiviso_non_degenera_sulla_griglia() {
        let passo = 1e-18;
        let lungo =
            |a: Coord<f64>, b: Coord<f64>| (b.x - a.x).abs().max((b.y - a.y).abs()) > 2.0 * passo;
        let mut punti = Vec::new();
        for k in 0_i32..400 {
            let c = Coord {
                x: f64::from(k) * 0.8,
                y: if k % 2 == 0 { 0.0 } else { 600.0 },
            };
            punti.push(c);
            // Una raffica di punti sotto la griglia vicino all'origine e
            // in mezzo alla linea.
            if k % 37 == 0 || k < 3 {
                for j in 1..=20 {
                    punti.push(Coord {
                        x: f64::from(j).mul_add(1e-20, c.x),
                        y: f64::from(j % 2).mul_add(1e-20, c.y),
                    });
                }
            }
        }
        let linea = LineString::new(punti);
        let parti = blocchi(&linea, passo);
        assert!(parti.len() > 1);
        for parte in &parti {
            assert!(
                parte.lines().any(|s| lungo(s.start, s.end)),
                "blocco degenere"
            );
        }
        for coppia in parti.windows(2) {
            let (a, b) = (&coppia[0].0, &coppia[1].0);
            assert_eq!(a[a.len() - 2..], b[..2]);
            assert!(lungo(b[0], b[1]), "segmento condiviso sotto la griglia");
        }
        // Tutti i punti, in ordine, una volta (piu' i condivisi).
        let ricomposti: Vec<Coord<f64>> = parti
            .iter()
            .enumerate()
            .flat_map(|(k, parte)| parte.0.iter().skip(if k == 0 { 0 } else { 2 }).copied())
            .collect();
        let mut attesi = linea.0;
        attesi.dedup();
        assert_eq!(ricomposti, attesi);
    }
}
