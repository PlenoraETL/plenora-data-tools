//! Confronto ESATTO fra un `Decimal128` e un `f64`.
//!
//! # Perche' serve
//!
//! Un decimale e un double sono due razionali con denominatori diversi:
//! `u / 10^s` contro `m * 2^e`. Convertire il decimale a double per
//! confrontarli collassa valori distinti: `0.100000000000000001` e il double
//! `1e-1` (`0.1000000000000000055511151231257827…`) diventano lo stesso
//! numero e risultano uguali, mentre il primo e' strettamente minore.
//!
//! # Come
//!
//! Si porta il confronto su interi, senza divisioni:
//!
//! ```text
//!   u / 10^s   ?   m * 2^e
//!   u / (2^s · 5^s)  ?  m · 2^e
//!   u · 5^max(0,-s)  ?  m · 5^max(0,s) · 2^(e+s)
//! ```
//!
//! # Dimensione dell'aritmetica
//!
//! La scala di un `Decimal128` **non e' limitata inferiormente** da Arrow:
//! `validate_decimal_precision_and_scale` (arrow-array 59.1.0) rifiuta solo
//! `scale > 38`, quindi `Decimal128(38, -100)` e' un tipo valido e i suoi
//! valori vanno confrontati, non dichiarati indecidibili. Con `scale = -128`
//! il fattore e' `5^128 < 2^298`, che moltiplicato per `|u| < 2^127` arriva a
//! **425 bit**: i 256 bit della versione precedente non bastavano e la
//! funzione restituiva `None` — «non so rispondere» su un valore legittimo.
//!
//! Si usano quindi 512 bit. Entrambi i lati nascono da un `u128` moltiplicato
//! ripetutamente per 5, quindi non serve un prodotto fra numeri grandi: basta
//! la moltiplicazione per una cifra. Il caso peggiore di ciascun lato —
//! `|u| · 5^128 < 2^425` a sinistra, `2^53 · 5^127 < 2^348` a destra — ci sta
//! con margine.
//!
//! La potenza di due residua puo' invece essere enorme (fino a `2^1202` con i
//! subnormali) e non serve calcolarla: se lo spostamento porta un lato oltre
//! i 512 bit, quel lato supera l'altro — che per costruzione ci sta dentro —
//! e il confronto e' deciso senza aritmetica a precisione arbitraria.

use std::cmp::Ordering;

/// Numero di limbi a 64 bit: 8 x 64 = 512.
const LIMBS: usize = 8;

/// Intero senza segno a 512 bit, il minimo indispensabile per questo
/// confronto: costruzione da `u128`, moltiplicazione per una cifra,
/// spostamento controllato, ordine.
///
/// Limbi in ordine little-endian (`limbs[0]` e' il meno significativo).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Big {
    limbs: [u64; LIMBS],
}

impl Big {
    const ZERO: Self = Self { limbs: [0; LIMBS] };

    #[allow(clippy::cast_possible_truncation)] // Meta' di un u128: esatte per costruzione.
    const fn from_u128(value: u128) -> Self {
        let mut limbs = [0_u64; LIMBS];
        // Le due meta' di un `u128` stanno in `u64` per costruzione.
        limbs[0] = value as u64;
        limbs[1] = (value >> 64) as u64;
        Self { limbs }
    }

    /// Moltiplica per una cifra; `None` se il risultato esce dai 512 bit.
    fn checked_mul_small(mut self, factor: u64) -> Option<Self> {
        let mut riporto: u128 = 0;
        for limb in &mut self.limbs {
            let prodotto = u128::from(*limb) * u128::from(factor) + riporto;
            // La parte bassa sta in `u64` per costruzione: `try_from` lo
            // dimostra al compilatore invece di affermarlo con un cast.
            *limb = u64::try_from(prodotto & u128::from(u64::MAX)).unwrap_or(u64::MAX);
            riporto = prodotto >> 64;
        }
        if riporto == 0 {
            Some(self)
        } else {
            None
        }
    }

    /// Numero di bit significativi (0 per lo zero).
    fn bit_len(self) -> u32 {
        for (indice, limb) in self.limbs.iter().enumerate().rev() {
            if *limb != 0 {
                let posizione = u32::try_from(indice).unwrap_or(0);
                return posizione * 64 + (64 - limb.leading_zeros());
            }
        }
        0
    }

    /// Spostamento a sinistra; `None` se il risultato non entra in 512 bit.
    fn checked_shl(self, shift: u32) -> Option<Self> {
        if self == Self::ZERO {
            return Some(self);
        }
        if self.bit_len().checked_add(shift)? > 512 {
            return None;
        }
        let salto = (shift / 64) as usize;
        let resto = shift % 64;
        let mut risultato = [0_u64; LIMBS];
        for indice in (0..LIMBS).rev() {
            let destinazione = indice + salto;
            if destinazione >= LIMBS {
                continue;
            }
            let mut valore = self.limbs[indice] << resto;
            if resto > 0 && indice > 0 {
                valore |= self.limbs[indice - 1] >> (64 - resto);
            }
            risultato[destinazione] |= valore;
        }
        Some(Self { limbs: risultato })
    }
}

impl Ord for Big {
    fn cmp(&self, other: &Self) -> Ordering {
        for indice in (0..LIMBS).rev() {
            let ordine = self.limbs[indice].cmp(&other.limbs[indice]);
            if ordine != Ordering::Equal {
                return ordine;
            }
        }
        Ordering::Equal
    }
}

impl PartialOrd for Big {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// `base * 5^exponent`; `None` solo se esce dai 512 bit — impossibile sul
/// dominio di questa funzione (vedi la nota sulla dimensione in testa al
/// modulo), ma il caso resta esplicito invece di essere assunto.
fn scale_by_pow5(base: u128, exponent: u32) -> Option<Big> {
    let mut valore = Big::from_u128(base);
    for _ in 0..exponent {
        valore = valore.checked_mul_small(5)?;
    }
    Some(valore)
}

/// Decomposizione esatta di un double finito: `(mantissa, esponente)` con
/// `valore = mantissa * 2^esponente` e `mantissa <= 2^53`.
const fn decompose(value: f64) -> (u64, i32) {
    let bits = value.to_bits();
    let raw_exponent = ((bits >> 52) & 0x7ff) as i32;
    let mantissa = bits & ((1_u64 << 52) - 1);
    if raw_exponent == 0 {
        // Subnormale: nessun bit implicito.
        (mantissa, -1074)
    } else {
        (mantissa | (1_u64 << 52), raw_exponent - 1075)
    }
}

/// Confronto esatto fra `unscaled * 10^(-scale)` e il double `expected`.
///
/// `None` **solo** se `expected` e' NaN (confronto non definito, come IEEE).
/// Ogni altra combinazione di `unscaled`, `scale` (tutto il dominio `i8`) e
/// double finito o infinito ha una risposta: un `Decimal128` valido non deve
/// mai risultare indecidibile.
#[must_use]
pub fn compare_decimal_with_f64(unscaled: i128, scale: i8, expected: f64) -> Option<Ordering> {
    if expected.is_nan() {
        return None;
    }
    if expected == f64::INFINITY {
        return Some(Ordering::Less);
    }
    if expected == f64::NEG_INFINITY {
        return Some(Ordering::Greater);
    }
    // Zeri e segni decidono prima di qualunque aritmetica.
    let decimal_sign = unscaled.signum();
    let double_sign = if expected == 0.0 {
        0_i128
    } else if expected > 0.0 {
        1
    } else {
        -1
    };
    if decimal_sign != double_sign {
        return Some(decimal_sign.cmp(&double_sign));
    }
    if decimal_sign == 0 {
        return Some(Ordering::Equal);
    }

    let (mantissa, exponent) = decompose(expected.abs());
    // u * 5^a  ?  m * 5^b * 2^(e+s)
    let cinque_sinistra = u32::from(scale.unsigned_abs()) * u32::from(scale < 0);
    let cinque_destra = u32::from(scale.unsigned_abs()) * u32::from(scale > 0);
    let left = scale_by_pow5(unscaled.unsigned_abs(), cinque_sinistra)?;
    let right = scale_by_pow5(u128::from(mantissa), cinque_destra)?;
    // La somma sta in `i64` per costruzione: `|e| <= 1074`, `|s| <= 128`.
    let shift = i64::from(exponent) + i64::from(scale);

    let magnitudes = if shift >= 0 {
        // Lo spostamento va a destra dell'equazione.
        let shift = u32::try_from(shift).ok()?;
        right.checked_shl(shift).map_or(
            // Il lato destro esce dai 512 bit: ha piu' bit del sinistro, che
            // ci sta per costruzione, quindi e' maggiore.
            Ordering::Less,
            |right| left.cmp(&right),
        )
    } else {
        let shift = u32::try_from(-shift).ok()?;
        left.checked_shl(shift)
            .map_or(Ordering::Greater, |left| left.cmp(&right))
    };
    // Con entrambi i lati negativi l'ordine delle magnitudini si rovescia.
    Some(if decimal_sign < 0 {
        magnitudes.reverse()
    } else {
        magnitudes
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decimal(unscaled: i128, scale: i8) -> impl Fn(f64) -> Option<Ordering> {
        move |expected| compare_decimal_with_f64(unscaled, scale, expected)
    }

    #[test]
    fn il_decimale_non_collassa_sul_double() {
        // Il caso della review: `1e-1` come double vale
        // 0.1000000000000000055511151231257827…, quindi
        // 0.100000000000000001 e' STRETTAMENTE minore. Convertendo il decimal
        // a double i due risultavano uguali.
        let sotto = decimal(100_000_000_000_000_001, 18);
        assert_eq!(sotto(0.1), Some(Ordering::Less));
        // E un decimale appena sopra il valore binario esatto e' maggiore.
        let sopra = decimal(100_000_000_000_000_010, 18);
        assert_eq!(sopra(0.1), Some(Ordering::Greater));
    }

    #[test]
    fn i_valori_esattamente_rappresentabili_sono_uguali() {
        assert_eq!(decimal(5, 1)(0.5), Some(Ordering::Equal));
        assert_eq!(decimal(25, 2)(0.25), Some(Ordering::Equal));
        assert_eq!(decimal(-125, 3)(-0.125), Some(Ordering::Equal));
        assert_eq!(decimal(3, 0)(3.0), Some(Ordering::Equal));
        // Scala negativa: 3 * 10^2 = 300.
        assert_eq!(decimal(3, -2)(300.0), Some(Ordering::Equal));
    }

    #[test]
    fn zeri_segni_e_non_finiti() {
        assert_eq!(decimal(0, 0)(0.0), Some(Ordering::Equal));
        assert_eq!(decimal(0, 38)(0.0), Some(Ordering::Equal));
        assert_eq!(decimal(0, -38)(0.0), Some(Ordering::Equal));
        assert_eq!(decimal(0, 0)(-0.0), Some(Ordering::Equal));
        assert_eq!(decimal(1, 0)(0.0), Some(Ordering::Greater));
        assert_eq!(decimal(-1, 0)(0.0), Some(Ordering::Less));
        assert_eq!(decimal(1, 0)(f64::INFINITY), Some(Ordering::Less));
        assert_eq!(decimal(1, 0)(f64::NEG_INFINITY), Some(Ordering::Greater));
        assert_eq!(decimal(1, 0)(f64::NAN), None);
    }

    #[test]
    fn i_negativi_rovesciano_l_ordine_delle_magnitudini() {
        // -0.2 < -0.1: la magnitudine maggiore e' il valore minore.
        assert_eq!(decimal(-2, 1)(-0.1), Some(Ordering::Less));
        assert_eq!(decimal(-1, 1)(-0.2), Some(Ordering::Greater));
    }

    #[test]
    fn gli_estremi_di_magnitudine_restano_decisi() {
        // Decimal enorme contro double minuscolo e viceversa.
        assert_eq!(decimal(i128::MAX, 0)(1e-300), Some(Ordering::Greater));
        assert_eq!(decimal(1, 38)(1e300), Some(Ordering::Less));
        assert_eq!(decimal(-1, 38)(-1e300), Some(Ordering::Greater));
    }

    #[test]
    fn le_scale_negative_estreme_restano_decidibili() {
        // Arrow rifiuta solo `scale > 38`: non c'e' limite inferiore, quindi
        // `Decimal128(38, -100)` e' un tipo valido. Con i 256 bit precedenti
        // `5^|scale|` non entrava e la funzione rispondeva `None` — cioe' «non
        // so» su un valore legittimo, che per un comparatore e' peggio di un
        // errore.
        for scale in [-56_i8, -80, -100, -128] {
            let esito = compare_decimal_with_f64(1, scale, 1.0);
            assert_eq!(
                esito,
                Some(Ordering::Greater),
                "scale {scale}: 1 * 10^{} deve superare 1.0",
                -i32::from(scale)
            );
            // E lo zero resta zero a qualunque scala.
            assert_eq!(
                compare_decimal_with_f64(0, scale, 0.0),
                Some(Ordering::Equal)
            );
            assert_eq!(
                compare_decimal_with_f64(0, scale, 1.0),
                Some(Ordering::Less)
            );
        }
        // `7e56` come double NON e' esattamente 7 * 10^56 (servirebbero piu'
        // di 53 bit di mantissa): il comparatore deve vedere la differenza,
        // che e' esattamente cio' per cui esiste.
        assert_ne!(
            compare_decimal_with_f64(7, -56, 7e56),
            Some(Ordering::Equal),
            "il decimale e il double arrotondato non sono lo stesso numero"
        );
        // Dove il valore E' rappresentabile, l'uguaglianza c'e'.
        assert_eq!(compare_decimal_with_f64(1, -1, 10.0), Some(Ordering::Equal));
        assert_eq!(
            compare_decimal_with_f64(2, -3, 2000.0),
            Some(Ordering::Equal)
        );
    }

    #[test]
    fn nessuna_combinazione_del_dominio_resta_indecisa() {
        // Il contratto della funzione: `None` solo per NaN. Si campiona tutto
        // il dominio della scala, i due segni e le magnitudini estreme.
        for scale in [i8::MIN, -100, -56, -38, -1, 0, 1, 38, 127] {
            for unscaled in [0_i128, 1, -1, i128::MAX, i128::MIN + 1] {
                for expected in [0.0_f64, 1.0, -1.0, 1e300, -1e300, 5e-324, f64::MAX] {
                    assert!(
                        compare_decimal_with_f64(unscaled, scale, expected).is_some(),
                        "indeciso su unscaled={unscaled} scale={scale} expected={expected}"
                    );
                }
            }
        }
    }

    /// Oracolo INDIPENDENTE: confronta `unscaled / 10^scale` con il valore
    /// esatto del double usando le frazioni razionali di `num`… che qui non
    /// c'e'. Si costruisce quindi il razionale a mano con `i128` estesi a
    /// `u128` in forma `numeratore/denominatore` normalizzata, per i casi in
    /// cui entrambi i lati stanno nei 128 bit.
    ///
    /// Non e' una seconda implementazione della stessa idea: il comparatore
    /// lavora per potenze di 5 e spostamenti, l'oracolo per prodotti incrociati
    /// di frazioni. Se condividessero un errore, dovrebbero sbagliare in due
    /// modi diversi allo stesso momento.
    fn oracolo_razionale(unscaled: i128, scale: i8, expected: f64) -> Option<Ordering> {
        if !expected.is_finite() {
            return None;
        }
        let (mantissa, exponent) = decompose(expected.abs());
        // decimale = unscaled / 10^scale ; double = ±mantissa * 2^exponent
        // Confronto incrociato: unscaled * den_double  ?  num_double * 10^scale
        // con tutti i fattori portati a numeratore.
        let (num_d, den_d): (i128, i128) = if exponent >= 0 {
            (
                i128::from(mantissa).checked_shl(u32::try_from(exponent).ok()?)?,
                1,
            )
        } else {
            (
                i128::from(mantissa),
                1_i128.checked_shl(u32::try_from(-exponent).ok()?)?,
            )
        };
        let num_d = if expected.is_sign_negative() {
            -num_d
        } else {
            num_d
        };
        let (num_dec, den_dec): (i128, i128) = if scale >= 0 {
            (
                unscaled,
                i128::checked_pow(10, u32::from(scale.unsigned_abs()))?,
            )
        } else {
            (
                unscaled.checked_mul(i128::checked_pow(10, u32::from(scale.unsigned_abs()))?)?,
                1,
            )
        };
        let sinistra = num_dec.checked_mul(den_d)?;
        let destra = num_d.checked_mul(den_dec)?;
        Some(sinistra.cmp(&destra))
    }

    /// Secondo oracolo indipendente, a precisione ARBITRARIA.
    ///
    /// [`oracolo_razionale`] lavora con prodotti incrociati in `i128` e quindi
    /// si arrende — restituendo `None` — proprio nella regione che il
    /// comparatore risolve con l'aritmetica a 512 bit: scale estreme,
    /// `unscaled` a fondo scala, double vicini a `f64::MAX` o subnormali. Li'
    /// il comparatore non aveva un giudice.
    ///
    /// Questo oracolo non ha un dominio: rappresenta le magnitudini come cifre
    /// in base 2^32 su un `Vec` che cresce quanto serve, moltiplica per dieci
    /// ripetutamente e sposta bit a bit. Non condivide nulla con il
    /// comparatore — ne' le tabelle di potenze di 5, ne' i limbi a lunghezza
    /// fissa, ne' la normalizzazione della scala: e' l'aritmetica delle
    /// elementari, lenta e ovvia.
    #[derive(Clone, PartialEq, Eq)]
    struct Cifre(Vec<u32>);

    impl Cifre {
        fn da_u128(mut valore: u128) -> Self {
            let mut cifre = Vec::new();
            while valore > 0 {
                cifre.push((valore & 0xffff_ffff) as u32);
                valore >>= 32;
            }
            Self(cifre)
        }

        fn e_zero(&self) -> bool {
            self.0.is_empty()
        }

        /// Moltiplicazione per una cifra piccola, con riporto.
        fn per_piccolo(&mut self, fattore: u32) {
            let mut riporto = 0_u64;
            for cifra in &mut self.0 {
                let prodotto = u64::from(*cifra) * u64::from(fattore) + riporto;
                *cifra = (prodotto & 0xffff_ffff) as u32;
                riporto = prodotto >> 32;
            }
            while riporto > 0 {
                self.0.push((riporto & 0xffff_ffff) as u32);
                riporto >>= 32;
            }
        }

        /// `self *= 10^volte`, una moltiplicazione alla volta.
        fn per_dieci(&mut self, volte: u32) {
            for _ in 0..volte {
                if self.e_zero() {
                    return;
                }
                self.per_piccolo(10);
            }
        }

        /// `self <<= bit`, spostando prima il resto e poi le cifre intere.
        fn sposta(&mut self, bit: u32) {
            if self.e_zero() {
                return;
            }
            let cifre = (bit / 32) as usize;
            let resto = bit % 32;
            if resto > 0 {
                let mut riporto = 0_u32;
                for cifra in &mut self.0 {
                    let nuovo = (*cifra << resto) | riporto;
                    riporto = *cifra >> (32 - resto);
                    *cifra = nuovo;
                }
                if riporto > 0 {
                    self.0.push(riporto);
                }
            }
            if cifre > 0 {
                let mut prefisso = vec![0_u32; cifre];
                prefisso.append(&mut self.0);
                self.0 = prefisso;
            }
        }

        fn confronta(&self, altro: &Self) -> Ordering {
            if self.0.len() != altro.0.len() {
                return self.0.len().cmp(&altro.0.len());
            }
            for indice in (0..self.0.len()).rev() {
                let ordine = self.0[indice].cmp(&altro.0[indice]);
                if ordine != Ordering::Equal {
                    return ordine;
                }
            }
            Ordering::Equal
        }
    }

    /// Confronta `unscaled / 10^scale` con `expected` senza alcun limite di
    /// magnitudine. Risponde su OGNI double finito: non ha un dominio da
    /// dichiarare.
    fn oracolo_esatto(unscaled: i128, scale: i8, expected: f64) -> Ordering {
        // Segni per primi: se differiscono, la magnitudine non serve.
        let segno_dec = unscaled.signum();
        let segno_dou = if expected == 0.0 {
            0
        } else if expected < 0.0 {
            -1
        } else {
            1
        };
        if segno_dec != segno_dou {
            return segno_dec.cmp(&segno_dou);
        }
        if segno_dec == 0 {
            return Ordering::Equal;
        }

        // decimale = unscaled / 10^scale ; double = mantissa * 2^exponent.
        // Si portano entrambi i lati a numeratore moltiplicando ciascuno per
        // il denominatore dell'altro: nessuna divisione, nessun
        // arrotondamento, nessun limite di larghezza.
        let (mantissa, exponent) = decompose(expected.abs());
        let mut sinistra = Cifre::da_u128(unscaled.unsigned_abs());
        let mut destra = Cifre::da_u128(u128::from(mantissa));
        if scale >= 0 {
            destra.per_dieci(u32::from(scale.unsigned_abs()));
        } else {
            sinistra.per_dieci(u32::from(scale.unsigned_abs()));
        }
        if exponent >= 0 {
            destra.sposta(exponent.unsigned_abs());
        } else {
            sinistra.sposta(exponent.unsigned_abs());
        }
        let ordine = sinistra.confronta(&destra);
        if segno_dec < 0 {
            ordine.reverse()
        } else {
            ordine
        }
    }

    #[test]
    fn l_oracolo_esatto_giudica_anche_la_regione_a_512_bit() {
        // Qui vivono i casi che l'oracolo razionale non poteva giudicare:
        // `unscaled` a fondo scala, tutto il dominio della scala `i8`, double
        // agli estremi dei normali e dei subnormali.
        let unscaled_estremi = [
            0_i128,
            1,
            -1,
            i128::MAX,
            i128::MIN + 1,
            i128::MAX / 3,
            -(i128::MAX / 7),
            1_i128 << 126,
            -(1_i128 << 100),
        ];
        let scale_estreme = [i8::MIN, -127, -100, -56, -39, -1, 0, 1, 38, 56, 100, 127];
        let double_estremi = [
            0.0_f64,
            1.0,
            -1.0,
            f64::MIN_POSITIVE,
            5e-324,
            -5e-324,
            1e300,
            -1e300,
            f64::MAX,
            f64::MIN,
        ];

        let mut confrontati = 0_u32;
        for unscaled in unscaled_estremi {
            for scale in scale_estreme {
                for expected in double_estremi {
                    let atteso = oracolo_esatto(unscaled, scale, expected);
                    let ottenuto = compare_decimal_with_f64(unscaled, scale, expected)
                        .expect("ogni double finito ha una risposta");
                    assert_eq!(
                        ottenuto, atteso,
                        "disaccordo su unscaled={unscaled} scale={scale} expected={expected}"
                    );
                    confrontati += 1;
                }
            }
        }
        let combinazioni = unscaled_estremi.len() * scale_estreme.len() * double_estremi.len();
        assert_eq!(
            confrontati,
            u32::try_from(combinazioni).expect("conteggio"),
            "ogni combinazione dev'essere stata giudicata, nessuna saltata"
        );

        // Campagna pseudo-casuale su `unscaled` a 128 bit PIENI, scala su
        // tutto il dominio `i8` e double presi dai bit: e' la parte che
        // l'oracolo razionale scartava per traboccamento.
        let mut stato: u64 = 0x9E37_79B9_7F4A_7C15;
        let mut casuali = 0_u32;
        let mut magnitudine = 0_u32;
        for _ in 0..1_000 {
            stato = stato
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            let alto = stato;
            stato = stato
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            let basso = stato;
            stato = stato
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            let unscaled = ((u128::from(alto) << 64) | u128::from(basso)).cast_signed();
            let scale = (((stato >> 16) & 0xff) as u8).cast_signed();
            stato = stato
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            let expected = f64::from_bits(stato);
            if !expected.is_finite() {
                continue;
            }
            let atteso = oracolo_esatto(unscaled, scale, expected);
            let ottenuto = compare_decimal_with_f64(unscaled, scale, expected)
                .expect("ogni double finito ha una risposta");
            assert_eq!(
                ottenuto, atteso,
                "disaccordo su unscaled={unscaled} scale={scale} expected={expected}"
            );
            casuali += 1;
            // Un accordo deciso dal solo SEGNO non dice nulla sull'aritmetica
            // a 512 bit: si contano a parte i casi in cui entrambi i lati
            // hanno lo stesso segno e il verdetto dipende davvero dalla
            // magnitudine.
            if unscaled.signum() != 0 && expected != 0.0 && (unscaled < 0) == (expected < 0.0) {
                magnitudine += 1;
            }
        }
        assert!(
            casuali > 800,
            "la campagna casuale deve giudicare quasi tutti i casi, non {casuali}"
        );
        assert!(
            magnitudine > 300,
            "l'accordo dev'essere deciso dalla magnitudine, non dal segno: {magnitudine}"
        );
    }

    #[test]
    fn l_oracolo_indipendente_concorda_sul_dominio_rappresentabile() {
        // Campagna deterministica: valori scelti per coprire i bordi (esatti,
        // appena sopra, appena sotto, segni, scale piccole) piu' una sequenza
        // pseudo-casuale riproducibile.
        let mut casi: Vec<(i128, i8, f64)> = vec![
            (1, 1, 0.1),
            (5, 1, 0.5),
            (100_000_000_000_000_001, 18, 0.1),
            (-3, 2, -0.03),
            (7, 0, 7.0),
            (7, 0, 7.000_000_000_000_001),
            (123_456_789, 4, 12_345.678_9),
            (1, -3, 1000.0),
            (-1, -3, -1000.0),
            (999_999_999_999, 6, 999_999.999_999),
        ];
        // Sequenza riproducibile (LCG): niente dipendenze, nessun `rand`.
        let mut stato: u64 = 0x2545_F491_4F6C_DD1D;
        for _ in 0..2_000 {
            stato = stato
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            let unscaled = i128::from((stato >> 32) as i32);
            let scale = i8::try_from((stato >> 8) % 13).unwrap_or(0) - 6;
            let grezzo = f64::from((stato & 0xffff) as u16) / 97.0;
            casi.push((unscaled, scale, grezzo));
            casi.push((unscaled, scale, -grezzo));
        }
        let mut confrontati = 0_u32;
        for (unscaled, scale, expected) in casi {
            let Some(atteso) = oracolo_razionale(unscaled, scale, expected) else {
                // Fuori dal dominio dell'oracolo (traboccamento delle
                // frazioni): il comparatore deve comunque rispondere.
                assert!(
                    compare_decimal_with_f64(unscaled, scale, expected).is_some(),
                    "indeciso su {unscaled}e-{scale} vs {expected}"
                );
                continue;
            };
            let ottenuto = compare_decimal_with_f64(unscaled, scale, expected)
                .expect("il comparatore risponde su ogni valore finito");
            assert_eq!(
                ottenuto, atteso,
                "disaccordo con l'oracolo su unscaled={unscaled} scale={scale} expected={expected}"
            );
            confrontati += 1;
        }
        assert!(
            confrontati > 1_000,
            "l'oracolo deve coprire la maggior parte dei casi, non {confrontati}"
        );
    }

    #[test]
    fn l_aritmetica_a_512_bit_e_esatta() {
        // Moltiplicazione per una cifra con riporto attraverso i limbi.
        let uno = Big::from_u128(u128::MAX);
        let cinque = uno.checked_mul_small(5).expect("entra in 512 bit");
        assert_eq!(cinque.bit_len(), 131, "(2^128-1)*5 ha 131 bit");
        // Spostamento fino al limite e oltre.
        assert!(Big::from_u128(1).checked_shl(511).is_some());
        assert!(Big::from_u128(1).checked_shl(512).is_none());
        assert_eq!(Big::ZERO.checked_shl(1000), Some(Big::ZERO));
        // Ordine fra magnitudini diverse.
        assert!(Big::from_u128(1).checked_shl(400) > Some(Big::from_u128(u128::MAX)));
        // `5^128` sta nei 512 bit e ha la lunghezza attesa (297,4 bit).
        let potenza = scale_by_pow5(1, 128).expect("5^128 entra in 512 bit");
        assert_eq!(potenza.bit_len(), 298);
        // Il caso peggiore reale: |i128::MIN| * 5^128.
        assert!(scale_by_pow5(1_u128 << 127, 128).is_some());
    }
}
