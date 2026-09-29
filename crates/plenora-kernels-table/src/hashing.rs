//! Hasher deterministici condivisi delle chiavi.
//!
//! Due hasher, ciascuno con un solo posto in cui e' definito: due copie che
//! divergono sul finalizer o sulla costante raggrupperebbero le stesse
//! chiavi in modo diverso, e solo su certi dati.
//!
//! - [`KeyHasher`]: valori nativi (interi, testi, valori di join) nelle
//!   mappe dei kernel;
//! - [`ChiaveHasher`]: chiavi binarie di riga (arena di `interning`, mappe
//!   e partizioni dello spill), dove `KeyHasher` degrada (vedi la sua
//!   documentazione).

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

/// Hasher a blocchi con ripiegamento dei bit alti, per le chiavi binarie di
/// riga (arena di `interning`, mappe e partizioni dello spill).
///
/// `KeyHasher` non va bene li': il suo passo per blocco (`rotl 5`, xor,
/// prodotto) propaga le differenze solo verso i bit alti, e quando i byte
/// che variano stanno in cima a un blocco e nel blocco di coda — la chiave
/// compatta di un Int64, marcatore piu' 8 byte big-endian — due blocchi si
/// annullano. Un milione di interi distinti danno 32 768 hash; le chiavi di
/// 21 e 2048 hanno lo stesso hash, quindi la stessa partizione di spill per
/// qualunque numero di partizioni (test `ripartizione_…` di `spill`).
///
/// Qui ogni blocco e' seguito da un ripiegamento dei bit alti su quelli
/// bassi, la lunghezza di ogni `write` entra nel digest, e il finalizer e'
/// lo splitmix64 di `KeyHasher`. Deterministico per costruzione: nessun
/// seme, stessi byte -> stesso hash su ogni esecuzione e piattaforma (i
/// blocchi si leggono little-endian esplicito). Non e' keyed: vale lo
/// stesso rischio residuo dichiarato per `KeyHasher`.
#[derive(Default)]
pub struct ChiaveHasher(u64);

/// Moltiplicatore del passo per blocco (parte frazionaria del rapporto
/// aureo, dispari).
const K_CHIAVE: u64 = 0x9e37_79b9_7f4a_7c15;

const fn mischia(stato: u64, blocco: u64) -> u64 {
    let stato = (stato ^ blocco).wrapping_mul(K_CHIAVE);
    stato ^ (stato >> 29)
}

impl Hasher for ChiaveHasher {
    fn finish(&self) -> u64 {
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    fn write(&mut self, bytes: &[u8]) {
        let (blocchi, resto) = bytes.as_chunks::<8>();
        self.0 = mischia(self.0, bytes.len() as u64);
        for blocco in blocchi {
            self.0 = mischia(self.0, u64::from_le_bytes(*blocco));
        }
        if !resto.is_empty() {
            let mut coda = [0_u8; 8];
            coda[..resto.len()].copy_from_slice(resto);
            self.0 = mischia(self.0, u64::from_le_bytes(coda));
        }
    }
}

/// `BuildHasher` delle mappe di chiavi binarie di riga.
pub type ChiaveBuildHasher = BuildHasherDefault<ChiaveHasher>;

/// Hash di una chiave binaria di riga con [`ChiaveHasher`].
#[must_use]
pub fn hash_chiave(chiave: &[u8]) -> u64 {
    let mut hasher = ChiaveHasher::default();
    hasher.write(chiave);
    hasher.finish()
}

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

    /// Chiavi compatte di un Int64 (marcatore, poi 8 byte big-endian) per i
    /// valori `0..1_000_000`: `hash_chiave` le tiene tutte distinte
    /// (`KeyHasher`, misurato il 2026-09-29, ne distingue 32 768).
    #[test]
    fn hash_chiave_separa_le_chiavi_compatte_degli_interi() {
        let mut nostri = std::collections::HashSet::new();
        let mut condivisi = std::collections::HashSet::new();
        for valore in 0..1_000_000_i64 {
            let mut chiave = vec![1_u8];
            chiave.extend_from_slice(&valore.to_be_bytes());
            nostri.insert(hash_chiave(&chiave));
            condivisi.insert(hash(&chiave));
        }
        assert_eq!(nostri.len(), 1_000_000, "KeyHasher: {}", condivisi.len());
    }

    #[test]
    fn hash_chiave_e_stabile_e_uguale_all_hasher_in_streaming() {
        // Valori fissati: un cambio della funzione cambia le partizioni di
        // spill, e deve essere una scelta esplicita.
        assert_eq!(hash_chiave(b""), 0x0);
        assert_eq!(hash_chiave(b"chiave"), 1_370_464_986_982_898_267);
        let mut hasher = ChiaveHasher::default();
        hasher.write(b"chiave");
        assert_eq!(hasher.finish(), hash_chiave(b"chiave"));
    }
}
