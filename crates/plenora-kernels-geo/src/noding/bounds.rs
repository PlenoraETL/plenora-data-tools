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
//! Questi sono limiti dei VALORI normalizzati, non delle capacita' del
//! backend `BigInt`. Non bastano a prenotare tutti i temporanei: prima della
//! riduzione add/sub producono fino a max(an+bd,bn+ad)+1 bit al numeratore
//! e ad+bd al denominatore; mul produce an+bn e ad+bd; div an+bd e ad+bn.
//! Anche i buffer di gcd, confronto e allocatore devono avere ricevute.
//! La decomposizione in celle di relate introduce formule ulteriori:
//! non e' corretto applicare 6299/5273 a tutti i suoi campioni.

const COORDINATE_NUMERATOR_BITS: u64 = 6299;
const COORDINATE_DENOMINATOR_BITS: u64 = 5273;

#[deny(clippy::disallowed_methods)]
mod tests {
    use super::*;
    use crate::noding::{intersections, Edge, Work, P};
    use crate::predicates::RelateLimits;

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
                    let edge = |a: (f64, f64), b: (f64, f64)| Edge {
                        a: P::of(a.into()),
                        b: P::of(b.into()),
                    };
                    let a = edge((-x, offset), (x, y));
                    let b = edge((-x, y), (x, -y));
                    let hits = intersections(&a, &b, &mut Work::new(RelateLimits::default(), None))
                        .expect("incroci del corpus estremo");
                    for p in [&a.a, &a.b, &b.a, &b.b]
                        .into_iter()
                        .chain(hits.iter().flatten())
                    {
                        for q in [&p.x, &p.y] {
                            assert!(q.n.bits() <= COORDINATE_NUMERATOR_BITS);
                            assert!(q.d.bits() <= COORDINATE_DENOMINATOR_BITS);
                            largest_n = largest_n.max(q.n.bits());
                            largest_d = largest_d.max(q.d.bits());
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
