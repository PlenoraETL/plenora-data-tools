//! Limiti algebrici PER OGGETTO, indipendenti dal numero di segmenti.
//!
//! Ogni f64 finito x si scrive X/S con S=2^1074 e |X|<2^2098:
//! il minimo subnormale e' 1/S; il massimo finito e' <2^1024.
//! Una differenza R=B-A ha |R|<2^2099. Un determinante di due
//! differenze D=Rx*Sy-Ry*Sx ha |D|<2^(2*2099+1)=2^4199.
//! Le due coordinate di un'intersezione parametrica sono
//! (A*D+R*T)/(S*D), dove T e' un altro determinante e D != 0.
//! Il numeratore ha modulo <2^(2099+4199+1)=2^6299 e il denominatore
//! positivo e' <2^(1074+4199)=2^5273. La riduzione per gcd puo' solo
//! diminuire questi limiti. Estremi e overlap sono inclusi nel dominio.
//!
//! ## Intermedi prima della riduzione (noder e DE-9IM)
//!
//! Una coppia (n,d) significa |numeratore| < 2^n, 0 < |denominatore| < 2^d.
//! Il denominatore diventa positivo nella normalizzazione del segno.
//! Per zero bastano zero bit; il segno e' separato. Per binary64 normalizzato
//! basta F=(1024,1075): il numeratore di un valore frazionario ha al massimo
//! 53 bit, quello intero al massimo 1024; il denominatore divide 2^1074.
//! Per interi 0,+/-1 basta I=(1,1), per 2 basta H=(2,1).
//!
//! Le regole, valide ANCHE prima del gcd, sono:
//! A +/- B = (max(An+Bd,Bn+Ad)+1, Ad+Bd);
//! A * B = (An+Bn,Ad+Bd); A / B = (An+Bd,Ad+Bn), B != 0.
//! Un confronto incrociato usa max(An+Bd,Bn+Ad) bit. Negazione, valore
//! assoluto, copie e confronti con zero non crescono. Nel gcd euclideo il
//! resto e' minore del divisore; nella divisione esatta per gcd il quoziente
//! non cresce. Queste ultime osservazioni NON dimensionano gli scratch
//! interni di una particolare implementazione di divisione/moltiplicazione.
//!
//! La tabella applica le regole SENZA assumere cancellazioni da gcd, salvo
//! il dominio C delle intersezioni dimostrato sopra. E' quindi volutamente
//! conservativa. Il massimo globale include le righe prima e dopo la
//! restrizione al dominio C. `cross(A,B)` indica componenti entrambe
//! limitate da A/B: mul(A,B), poi sub dei due prodotti.
//!
//! | valore / formula nel sorgente | n | d |
//! | --- | ---: | ---: |
//! | R = sub(F,F), differenze degli estremi | 2100 | 2150 |
//! | prodotto dei determinanti = mul(R,R) | 4200 | 4300 |
//! | D,T,U = cross(R,R) | 8501 | 8600 |
//! | t,u = div(T,D) | 17101 | 17101 |
//! | mul(R,t) | 19201 | 19251 |
//! | add(F,mul(R,t)), intersezione prima del gcd | 20277 | 20326 |
//! | C, intersezione ridotta (dimostrazione sopra) | 6299 | 5273 |
//! | add(C,C), somma per il campione X | 11573 | 10546 |
//! | X = div(add(C,C),H), campione X | 11574 | 10548 |
//! | sub(X,F), in `Edge::at_x` | 12650 | 11623 |
//! | mul(R,sub(X,F)) | 14750 | 13773 |
//! | div(mul(R,sub(X,F)),R) | 16900 | 15873 |
//! | Y = add(F,div(mul(R,sub(X,F)),R)) | 17976 | 16948 |
//! | add(Y,Y), somma per il campione Y | 34925 | 33896 |
//! | P = div(add(Y,Y),H), campione Y e dominio uniforme del punto | 34926 | 33898 |
//! | V = sub(F,P), raggi, p-estremo, test above | 36002 | 34973 |
//! | W = add(V,V), direzione di settore | 70976 | 69946 |
//! | mul(R,V), prodotto per `contains`/`ring_contains` | 38102 | 37123 |
//! | cross(R,V), `contains`/`ring_contains` | 75226 | 74246 |
//! | mul(V,V), prodotto per `angle_cmp` | 72004 | 69946 |
//! | cross(V,V), `angle_cmp` | 141951 | 139892 |
//! | mul(R,W), prodotto per perturbazione | 73076 | 72096 |
//! | cross(R,W), `ring_contains` con direzione | 145173 | 144192 |
//!
//! I campioni esterni +/-1 sono inclusi: add(C,I)=(6301,5274) <= X,
//! add(Y,I)=(17978,16949) <= P. Il campione vuoto 0 e gli assi +/-1
//! sono inclusi nei rispettivi domini. Le componenti X dei punti sono
//! <= P: usare un dominio uniforme allarga, non restringe, il limite.
//! Le direzioni vengono SOMMATE A COPPIE, mai accumulate nel ciclo;
//! i campioni vengono costruiti dai valori critici, mai da altri campioni.
//! Il noder riceve sempre estremi binary64, mai nodi reinseriti come estremi.
//! Le booleane lineari usano anche Edge con estremi C per contains, senza
//! rinodarli: sub(C,C)=(11573,10546), cross(sub(C,C),sub(C,C))=(44239,42184).
//! Questi intermedi sono inclusi nel massimo globale; i loro punti medi
//! sono nel dominio X, incluso nel dominio uniforme P della classificazione.
//! Le frontiere poligonali usano midpoint(C,C), normali ottenute da
//! sub(C,C) e orientamenti cross(sub(C,C),sub(C,C)). Per assegnare i buchi,
//! `ring_contains` riceve lati C, punto X e direzione nulla: sub(X,C) =
//! (16848,15821), cross(sub(C,C),sub(X,C)) = (54789,52734).
//! Il verso delle facce usa un solo determinante al minimo lessicografico.
//! I buchi usano contenimento esatto, senza accumulare aree razionali.
//! Tutti questi intermedi restano sotto il massimo globale indipendente da n.
//! Questa proprieta' e' indispensabile: rinodare segmenti con estremi
//! razionali richiederebbe un'altra derivazione, non la stessa costante.
//!
//! Confronti incrociati: F/F <=2099, t/I <=17102, C/C <=11572,
//! X/F <=12649, Y/Y <=34924, P/F <=36001 bit. Gli ordinamenti dei raggi
//! usano il determinante gia' incluso, non un confronto Q fra direzioni.
//! Le uguaglianze confrontano numeratori/denominatori canonici separati.
//! Anche un confronto P/P (non necessario oggi) sta in 68824 bit.
//! Triangle, overlap e validazione anelli usano sottoinsiemi delle formule.
//! Il massimo dei valori aritmetici e' dunque 145173 bit di modulo:
//! 2269 limb u64 (145216 bit) sarebbero sufficienti PER I VALORI, con segno
//! separato, se gli algoritmi dei limb non introducono intermedi maggiori.
//! Non e' un limite heap del `BigInt` attuale, ne' una scelta gia' implementata
//! di rappresentazione: memorizzare ogni estremo con tale larghezza sarebbe
//! molto costoso. La prova dei valori non sostituisce quella delle allocazioni.

// Il confronto d'area usa un filtro a 128 bit frazionari, poi divisioni
// binarie progressive. Ogni termine cross(C,C) ha al massimo 23145 bit
// al numeratore e 21092 al denominatore; lo shift iniziale usa 23273 bit.
// Un prodotto di coordinate finite e' < 2^2048, la differenza < 2^2049;
// fino a 2^64 termini accumulano meno di 2241 bit alla griglia iniziale.
// Dopo ogni cifra indecisa il quoziente cumulativo ha modulo < n; il passo
// successivo ha modulo < 3n e usa al massimo usize::BITS + 2 bit. I resti
// rimangono minori dei rispettivi denominatori, senza denominatore comune.
// Se S != 0, |S| >= 1/prodotto(d_i) > 2^-sum(bits(d_i)). Dopo
// sum(bits(d_i)) + usize::BITS + 2 cifre, un intervallo ancora indeciso
// certifica S=0. Cresce il lavoro, addebitato a Work, non la larghezza
// degli interi. Nessun tetto di precisione dipendente dal numero di lati.
// Il tetto interno resta distinto dai budget del chiamante; tutti gli
// intermedi sono controllati prima di add/sub/mul/shift e contabilizzati.
pub const INTERMEDIATE_BITS: u64 = 145_173;
/// Massimo addebito post-creazione di un intero (non scratch del backend).
pub const MAX_VALUE_BYTES: u64 = INTERMEDIATE_BITS.div_ceil(usize::BITS as u64)
    * std::mem::size_of::<usize>() as u64
    + std::mem::size_of::<num_bigint::BigInt>() as u64;

#[cfg(test)]
#[deny(clippy::disallowed_methods)]
mod tests {
    use super::*;
    const COORDINATE_NUMERATOR_BITS: u64 = 6299;
    const COORDINATE_DENOMINATOR_BITS: u64 = 5273;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    struct Envelope(u64, u64);

    impl Envelope {
        fn add(self, b: Self) -> Self {
            Self((self.0 + b.1).max(b.0 + self.1) + 1, self.1 + b.1)
        }

        const fn mul(self, b: Self) -> Self {
            Self(self.0 + b.0, self.1 + b.1)
        }

        const fn div(self, b: Self) -> Self {
            Self(self.0 + b.1, self.1 + b.0)
        }

        fn cross(self, b: Self) -> Self {
            let product = self.mul(b);
            product.add(product)
        }

        fn midpoint(self) -> Self {
            self.add(self).div(Self(2, 1))
        }

        fn contains(self, q: &crate::noding::oracle::Q) -> bool {
            q.n.bits() <= self.0 && q.d.bits() <= self.1
        }
    }

    use crate::noding::{intersections, Edge, Work, P};
    use crate::predicates::RelateLimits;

    /// IEEE 754 binary64 e regole dei prodotti di frazioni documentate
    /// sopra: questa prova rende verificabili tutte le righe della tabella,
    /// comprese quelle non raggiunte da un particolare corpus geometrico.
    #[test]
    fn formula_envelopes_cover_the_entire_classification_dag() {
        let f = Envelope(1024, 1075);
        let r = f.add(f);
        let det = r.cross(r);
        let t = det.div(det);
        let c = Envelope(COORDINATE_NUMERATOR_BITS, COORDINATE_DENOMINATOR_BITS);
        let x = c.midpoint();
        let offset = x.add(f);
        let scaled = r.mul(offset);
        let quotient = scaled.div(r);
        let y = f.add(quotient);
        let p = y.midpoint();
        let ray = f.add(p);
        let direction = ray.add(ray);
        for (actual, expected) in [
            (r, Envelope(2100, 2150)),
            (r.mul(r), Envelope(4200, 4300)),
            (det, Envelope(8501, 8600)),
            (t, Envelope(17101, 17101)),
            (r.mul(t), Envelope(19201, 19251)),
            (f.add(r.mul(t)), Envelope(20277, 20326)),
            (c.add(c), Envelope(11573, 10546)),
            (c.add(c).cross(c.add(c)), Envelope(44239, 42184)),
            (x, Envelope(11574, 10548)),
            (x.add(c), Envelope(16848, 15821)),
            (c.add(c).cross(x.add(c)), Envelope(54789, 52734)),
            (offset, Envelope(12650, 11623)),
            (scaled, Envelope(14750, 13773)),
            (quotient, Envelope(16900, 15873)),
            (y, Envelope(17976, 16948)),
            (y.add(y), Envelope(34925, 33896)),
            (p, Envelope(34926, 33898)),
            (ray, Envelope(36002, 34973)),
            (direction, Envelope(70976, 69946)),
            (r.mul(ray), Envelope(38102, 37123)),
            (r.cross(ray), Envelope(75226, 74246)),
            (ray.mul(ray), Envelope(72004, 69946)),
            (ray.cross(ray), Envelope(141_951, 139_892)),
            (r.mul(direction), Envelope(73076, 72096)),
            (r.cross(direction), Envelope(145_173, 144_192)),
        ] {
            assert_eq!(actual, expected);
            assert!(actual.0 <= INTERMEDIATE_BITS && actual.1 <= INTERMEDIATE_BITS);
        }
        assert_eq!(INTERMEDIATE_BITS.div_ceil(64), 2269);
        assert_eq!(c.add(Envelope(1, 1)), Envelope(6301, 5274));
        assert_eq!(y.add(Envelope(1, 1)), Envelope(17978, 16949));
    }

    // Conserva il dominio teorico separato dai bit effettivi. Le operazioni
    // costruiscono anche numeratore e denominatore NON RIDOTTI con BigInt:
    // verificare soltanto il risultato di Q::new perderebbe proprio il
    // problema di capacita' che il bilancio dell'aritmetica deve coprire.
    struct Witness {
        value: crate::noding::oracle::Q,
        envelope: Envelope,
    }

    impl Witness {
        fn new(value: crate::noding::oracle::Q, envelope: Envelope) -> Self {
            assert!(envelope.contains(&value));
            Self { value, envelope }
        }

        fn add(&self, b: &Self, subtract: bool) -> Self {
            let left = &self.value.n * &b.value.d;
            let right = &b.value.n * &self.value.d;
            assert!(left.bits() <= self.envelope.0 + b.envelope.1);
            assert!(right.bits() <= b.envelope.0 + self.envelope.1);
            let raw = crate::noding::oracle::Q {
                n: if subtract { left - right } else { left + right },
                d: &self.value.d * &b.value.d,
            };
            let envelope = self.envelope.add(b.envelope);
            assert!(envelope.contains(&raw));
            Self::new(
                if subtract {
                    self.value.sub(&b.value)
                } else {
                    self.value.add(&b.value)
                },
                envelope,
            )
        }

        fn mul(&self, b: &Self, divide: bool) -> Self {
            let (raw, envelope) = if divide {
                assert_ne!(b.value.sign(), std::cmp::Ordering::Equal);
                (
                    crate::noding::oracle::Q {
                        n: &self.value.n * &b.value.d,
                        d: &self.value.d * &b.value.n,
                    },
                    self.envelope.div(b.envelope),
                )
            } else {
                (
                    crate::noding::oracle::Q {
                        n: &self.value.n * &b.value.n,
                        d: &self.value.d * &b.value.d,
                    },
                    self.envelope.mul(b.envelope),
                )
            };
            assert!(envelope.contains(&raw));
            Self::new(
                if divide {
                    self.value.div(&b.value)
                } else {
                    self.value.mul(&b.value)
                },
                envelope,
            )
        }

        fn midpoint(&self, b: &Self) -> Self {
            self.add(b, false).mul(
                &Self::new(crate::noding::oracle::Q::int(2), Envelope(2, 1)),
                true,
            )
        }
    }

    /// IEEE 754 binary64, formula parametrica e decomposizione verticale:
    /// tutte le operazioni sono misurate prima del gcd su subnormali,
    /// massimi esponenti e segni opposti; anche raggi e settori non assiali.
    #[test]
    fn unreduced_extreme_samples_and_directions_fit_the_envelopes() {
        use crate::noding::oracle::Q;

        let tiny = f64::from_bits(1);
        let values = [tiny, f64::MIN_POSITIVE, 1.0, 2.0_f64.powi(1023), f64::MAX];
        let input = |x| Witness::new(Q::float(x), Envelope(1024, 1075));
        let cross = |a: &[Witness; 2], b: &[Witness; 2]| {
            a[0].mul(&b[1], false).add(&a[1].mul(&b[0], false), true)
        };
        let mut large_n = 0;
        let mut large_d = 0;
        for x in values {
            for y in values {
                for offset in [0.0, tiny, -tiny] {
                    let a = [input(-x), input(offset)];
                    let end = [input(x), input(y)];
                    let b = [input(-x), input(y)];
                    let other = [input(x), input(-y)];
                    let r = [end[0].add(&a[0], true), end[1].add(&a[1], true)];
                    let s = [other[0].add(&b[0], true), other[1].add(&b[1], true)];
                    let delta = [b[0].add(&a[0], true), b[1].add(&a[1], true)];
                    let denominator = cross(&r, &s);
                    let t = cross(&delta, &s).mul(&denominator, true);
                    let u = cross(&delta, &r).mul(&denominator, true);
                    // Entrambi i parametri usano lo stesso dominio.
                    assert_eq!(t.envelope, u.envelope);
                    let hit = a[0].add(&r[0].mul(&t, false), false);
                    let critical = Witness::new(hit.value, Envelope(6299, 5273));
                    let sample_x =
                        critical.midpoint(&Witness::new(Q::float(x), Envelope(6299, 5273)));
                    let at_x = |start: &[Witness; 2], diff: &[Witness; 2]| {
                        start[1].add(
                            &diff[1]
                                .mul(&sample_x.add(&start[0], true), false)
                                .mul(&diff[0], true),
                            false,
                        )
                    };
                    let ya = at_x(&a, &r);
                    let yb = at_x(&b, &s);
                    let sample_y = ya.midpoint(&yb);
                    assert_eq!(sample_y.value, ya.value.midpoint(&yb.value));
                    let p = [
                        Witness::new(sample_x.value, Envelope(34926, 33898)),
                        sample_y,
                    ];
                    let ray = [a[0].add(&p[0], true), a[1].add(&p[1], true)];
                    let ray_b = [b[0].add(&p[0], true), b[1].add(&p[1], true)];
                    let direction = [ray[0].add(&ray_b[0], false), ray[1].add(&ray_b[1], false)];
                    for value in [cross(&r, &ray), cross(&ray, &ray_b), cross(&r, &direction)] {
                        large_n = large_n.max(value.value.n.bits());
                        large_d = large_d.max(value.value.d.bits());
                        assert!(value.envelope.0 <= INTERMEDIATE_BITS);
                        assert!(value.envelope.1 <= INTERMEDIATE_BITS);
                    }
                }
            }
        }
        assert!(large_n > 1024);
        assert!(large_d > 1075);
    }

    /// IEEE 754 binary64 e formula parametrica espansa sopra: include
    /// subnormali, massimi esponenti, segni opposti e coordinate miste.
    #[test]
    fn normalized_intersections_fit_the_per_object_bound() {
        let tiny = f64::from_bits(1);
        let values = [tiny, f64::MIN_POSITIVE, 1.0, 2.0_f64.powi(1023), f64::MAX];
        let mut largest_n = 0;
        let mut largest_d = 0;
        for x in values {
            for y in values {
                for offset in [0.0, tiny, -tiny] {
                    let mut work = Work::new(RelateLimits::default(), None);
                    let edge = |a: (f64, f64), b: (f64, f64)| Edge {
                        a: P::of_in(a.into(), &work).expect("punto"),
                        b: P::of_in(b.into(), &work).expect("punto"),
                    };
                    let a = edge((-x, offset), (x, y));
                    let b = edge((-x, y), (x, -y));
                    let hits =
                        intersections(&a, &b, &mut work).expect("incroci del corpus estremo");
                    for p in [&a.a, &a.b, &b.a, &b.b]
                        .into_iter()
                        .chain(hits.iter().flatten())
                    {
                        for q in [&p.x, &p.y] {
                            assert!(q.bits().0 <= COORDINATE_NUMERATOR_BITS);
                            assert!(q.bits().1 <= COORDINATE_DENOMINATOR_BITS);
                            largest_n = largest_n.max(q.bits().0);
                            largest_d = largest_d.max(q.bits().1);
                        }
                    }
                }
            }
        }
        // Il corpus esercita davvero razionali piu' grandi delle sole
        // coordinate iniziali: il test non passa solo su interi piccoli.
        assert!(largest_n > 1024);
        assert!(largest_d > 1075);
    }
}
