//! Esadecimale minuscolo, la forma di digest, token e identificativi.
//!
//! Una tabella dei nibble indicizzata da un valore in `0..16` (shift e
//! maschera su `u8`): esatta per costruzione, senza il `Result` di `write!` da
//! scartare e senza primitive di panic (gate R6).

const CIFRE: &[u8; 16] = b"0123456789abcdef";

/// Aggiunge in coda a `testo` i byte in esadecimale minuscolo, due cifre per
/// byte.
pub fn aggiungi_esadecimale(testo: &mut String, byte: &[u8]) {
    testo.reserve(byte.len() * 2);
    for &grezzo in byte {
        testo.push(char::from(CIFRE[usize::from(grezzo >> 4)]));
        testo.push(char::from(CIFRE[usize::from(grezzo & 0x0F)]));
    }
}

/// I byte in esadecimale minuscolo.
#[must_use]
pub fn esadecimale(byte: &[u8]) -> String {
    let mut testo = String::new();
    aggiungi_esadecimale(&mut testo, byte);
    testo
}

#[cfg(test)]
mod tests {
    use super::{aggiungi_esadecimale, esadecimale};

    /// Oracolo: `format!("{:02x}")` byte per byte, su tutti i 256 valori.
    #[test]
    fn coincide_con_la_formattazione_di_std() {
        let tutti = (0..=u8::MAX).collect::<Vec<_>>();
        let mut atteso = String::new();
        for byte in &tutti {
            use std::fmt::Write as _;
            let _ = write!(atteso, "{byte:02x}");
        }
        assert_eq!(esadecimale(&tutti), atteso);
        let mut in_coda = String::from("x");
        aggiungi_esadecimale(&mut in_coda, &[0x0f, 0xa0]);
        assert_eq!(in_coda, "x0fa0");
        assert_eq!(esadecimale(&[]), "");
    }
}
