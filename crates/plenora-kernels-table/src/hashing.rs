//! Hasher deterministico condiviso delle chiavi.
//!
//! Un solo hasher per tutti i kernel che raggruppano o uniscono per chiave:
//! due copie che divergono sul finalizer o sulla costante raggrupperebbero
//! le stesse chiavi in modo diverso, e solo su certi dati.

use std::hash::{BuildHasherDefault, Hasher};

/// Hasher moltiplicativo a blocchi (stile `FxHash`) con finalizer splitmix64.
///
/// `SipHash` (default std) dominerebbe il costo di build/probe su milioni di
/// righe: qui si sceglie il throughput.
///
/// # Rischio residuo dichiarato
///
/// Le chiavi sono dati di input e la funzione non e' *keyed*: collisioni
/// costruite apposta degradano build e probe fino al quadratico entro i
/// limiti di piano (`max_input_rows`, `max_rows_per_edge`), che bound-ano
/// solo `n`. Un hasher con chiave per processo toglierebbe la stabilita'
/// dell'hash fra esecuzioni, su cui poggiano piu' kernel. Registro:
/// `errori-e-limiti.md#limiti-dichiarati`.
///
/// Il finalizer non e' decorativo: senza, le chiavi con un lungo prefisso
/// comune (stesso tipo, stessa lunghezza) si concentrano in pochi bucket.
#[derive(Default)]
pub struct KeyHasher(u64);

impl Hasher for KeyHasher {
    fn finish(&self) -> u64 {
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    fn write(&mut self, bytes: &[u8]) {
        const K: u64 = 0x51_7c_c1_b7_27_22_0a_95;
        // `as_chunks::<8>()` da' blocchi tipizzati `[u8; 8]` e il resto: la
        // conversione e' totale per tipo.
        let (blocchi, remainder) = bytes.as_chunks::<8>();
        for blocco in blocchi {
            let value = u64::from_le_bytes(*blocco);
            self.0 = (self.0.rotate_left(5) ^ value).wrapping_mul(K);
        }
        if !remainder.is_empty() {
            let mut tail = 0_u64;
            for &byte in remainder {
                tail = (tail << 8) | u64::from(byte);
            }
            self.0 = (self.0.rotate_left(5) ^ tail).wrapping_mul(K);
        }
    }
}

/// `BuildHasher` da usare nelle mappe e negli insiemi di chiavi dei kernel.
pub type FastHasher = BuildHasherDefault<KeyHasher>;

#[cfg(test)]
mod tests {
    use super::*;

    fn hash(bytes: &[u8]) -> u64 {
        let mut hasher = KeyHasher::default();
        hasher.write(bytes);
        hasher.finish()
    }

    #[test]
    fn e_deterministico_e_sensibile_a_ogni_byte() {
        assert_eq!(hash(b"chiave"), hash(b"chiave"));
        assert_ne!(hash(b"chiave"), hash(b"chiavf"));
        // La coda non allineata a 8 byte entra nel digest.
        assert_ne!(hash(b"01234567"), hash(b"012345678"));
    }

    #[test]
    fn il_finalizer_sparpaglia_i_prefissi_comuni() {
        // Chiavi con prefisso lungo identico: senza finalizer finirebbero in
        // pochissimi bucket. Si verifica la dispersione dei bit bassi, quelli
        // che la HashMap usa per scegliere il bucket.
        let mut low_bits = std::collections::HashSet::new();
        for index in 0..64_u32 {
            let key = format!("prefisso-molto-lungo-e-comune-{index:04}");
            low_bits.insert(hash(key.as_bytes()) & 0x3f);
        }
        assert!(
            low_bits.len() > 32,
            "dispersione insufficiente dei bit bassi: {}",
            low_bits.len()
        );
    }
}
