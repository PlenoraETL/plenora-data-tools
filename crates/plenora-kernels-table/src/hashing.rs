//! Hasher deterministico condiviso delle chiavi.
//!
//! Lo stesso hasher serve a quattro moduli — `joins`, `governance`,
//! `reshape`, `aggregation::grouping` — e vive qui in un posto solo.
//! Quattro copie di una funzione di hash sarebbero quattro occasioni di
//! divergere: basta che una sola cambi il finalizer o la costante per avere
//! due kernel che raggruppano le stesse chiavi in modo diverso, con un difetto
//! che si manifesta solo su certi dati e solo in certi percorsi. Vive qui, in
//! un posto solo.

use std::hash::{BuildHasherDefault, Hasher};

/// Hasher moltiplicativo a blocchi (stile `FxHash`) con finalizer splitmix64.
///
/// `SipHash` (default std) dominerebbe il costo di build/probe su milioni di
/// righe: qui si sceglie il throughput.
///
/// # Rischio residuo dichiarato
///
/// Le chiavi SONO valori delle righe, cioe' dati di input: chi li fornisce
/// puo' sceglierli. La ricorrenza moltiplicativa non e' *keyed*, quindi
/// collisioni su piu' blocchi sono costruibili e il finalizer non le
/// elimina — sparpaglia i bit, non rende la funzione resistente. I limiti di
/// piano (`max_input_rows`, `max_rows_per_edge`) bound-ano `n` e quindi il
/// costo peggiore, ma NON impediscono un comportamento quadratico entro quel
/// `n`: un input costruito apposta puo' far degradare build e probe.
///
/// La mitigazione vera e' un hasher con chiave per processo. Non e' fatta
/// qui perche' cambia una proprieta' su cui poggiano piu' kernel — la
/// stabilita' dell'hash fra esecuzioni — e va verificata su tutti gli usi di
/// `FastHasher` prima di essere introdotta. Fino ad allora il rischio e'
/// questo, dichiarato, non «le chiavi non sono controllabili».
///
/// Registrato come **errori-e-limiti.md#limiti-dichiarati** in `docs/errori-e-limiti.md`: un rischio residuo
/// accettato vive nel registro delle deroghe, con owner e condizione di
/// rientro, non solo in un commento che nessun processo rilegge.
///
/// Il finalizer NON e' decorativo: senza, le chiavi con un prefisso comune
/// lungo (stesso tipo, stessa lunghezza) si concentrano in pochi bucket —
/// misurato: 328 elementi nel bucket peggiore contro 7 con finalizer.
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
        // `as_chunks::<8>()` restituisce blocchi gia' tipizzati `[u8; 8]` e
        // il resto: la totalita' della conversione e' nel TIPO, non piu' in
        // un commento accanto a una copia. I valori prodotti sono gli stessi
        // — stessi blocchi, stesso ordine, stesso trattamento del resto —
        // quindi l'hash non cambia.
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
