//! Checksummed output framing for spatial-join index pairs.

use std::io::{Read, Write};

use sha2::{Digest, Sha256};
use thiserror::Error;

use plenora_kernels_geo::spatial_join::JoinPair;

use super::framing::{self, DifettoChiusura};

pub const PAIR_MAGIC: &[u8; 8] = b"PLNPAIR1";
pub const PAIR_TRAILER_MAGIC: &[u8; 8] = b"PAIREND1";
pub const MAX_PAIRS: u64 = 10_000_000;

#[derive(Debug, Error)]
pub enum PairProtocolError {
    #[error("errore I/O protocollo coppie: {0}")]
    Io(#[from] std::io::Error),
    #[error("magic protocollo coppie non valido")]
    InvalidMagic,
    #[error("trailer protocollo coppie non valido")]
    InvalidTrailer,
    #[error("checksum protocollo coppie non valido")]
    ChecksumMismatch,
    #[error("pair_count {0} oltre il limite {MAX_PAIRS}")]
    TooManyPairs(u64),
    #[error("byte inattesi dopo il trailer coppie")]
    TrailingBytes,
}

fn header_bytes(pair_count: u64) -> Result<[u8; 16], PairProtocolError> {
    if pair_count > MAX_PAIRS {
        return Err(PairProtocolError::TooManyPairs(pair_count));
    }
    Ok(framing::header(*PAIR_MAGIC, pair_count))
}

/// Codifica le coppie di indici con header, un frame per coppia e trailer
/// con checksum SHA-256 dell'intero stream.
///
/// # Errors
///
/// - `PairProtocolError::TooManyPairs`: piu' di `MAX_PAIRS` coppie;
/// - `PairProtocolError::Io`: errore di scrittura o flush del writer.
pub fn write_pairs<W: Write>(
    mut writer: W,
    pairs: &[JoinPair],
) -> Result<(W, [u8; 32]), PairProtocolError> {
    let pair_count =
        u64::try_from(pairs.len()).map_err(|_| PairProtocolError::TooManyPairs(u64::MAX))?;
    let header = header_bytes(pair_count)?;
    writer.write_all(&header)?;
    let mut hasher = Sha256::new();
    hasher.update(header);
    for pair in pairs {
        let mut frame = [0_u8; 16];
        frame[..8].copy_from_slice(&pair.left.to_le_bytes());
        frame[8..].copy_from_slice(&pair.right.to_le_bytes());
        writer.write_all(&frame)?;
        hasher.update(frame);
    }
    let digest: [u8; 32] = hasher.finalize().into();
    framing::scrivi_chiusura(&mut writer, *PAIR_TRAILER_MAGIC, &digest)?;
    Ok((writer, digest))
}

/// Decodifica le coppie verificando magic, limite di conteggio, trailer e
/// checksum; i byte residui dopo il trailer sono rifiutati (fail-closed).
///
/// # Errors
///
/// - `PairProtocolError::Io`: stream troncato o errore di lettura;
/// - `PairProtocolError::InvalidMagic` / `InvalidTrailer`: magic di header o
///   trailer non corrispondente;
/// - `PairProtocolError::TooManyPairs`: `pair_count` dichiarato oltre
///   `MAX_PAIRS`;
/// - `PairProtocolError::ChecksumMismatch`: digest SHA-256 non coincidente;
/// - `PairProtocolError::TrailingBytes`: byte inattesi dopo il trailer.
pub fn read_pairs<R: Read>(mut reader: R) -> Result<Vec<JoinPair>, PairProtocolError> {
    let mut header = [0_u8; 16];
    reader.read_exact(&mut header)?;
    let Some(pair_count) = framing::contatore_dichiarato(&header, *PAIR_MAGIC) else {
        return Err(PairProtocolError::InvalidMagic);
    };
    if pair_count > MAX_PAIRS {
        return Err(PairProtocolError::TooManyPairs(pair_count));
    }
    let capacity =
        usize::try_from(pair_count).map_err(|_| PairProtocolError::TooManyPairs(pair_count))?;
    let mut pairs = Vec::with_capacity(capacity);
    let mut hasher = Sha256::new();
    hasher.update(header);
    for _ in 0..pair_count {
        let mut frame = [0_u8; 16];
        reader.read_exact(&mut frame)?;
        hasher.update(frame);
        let [l0, l1, l2, l3, l4, l5, l6, l7, r0, r1, r2, r3, r4, r5, r6, r7] = frame;
        pairs.push(JoinPair {
            left: u64::from_le_bytes([l0, l1, l2, l3, l4, l5, l6, l7]),
            right: u64::from_le_bytes([r0, r1, r2, r3, r4, r5, r6, r7]),
        });
    }
    let digest: [u8; 32] = hasher.finalize().into();
    framing::verifica_chiusura(&mut reader, *PAIR_TRAILER_MAGIC, &digest).map_err(|difetto| {
        match difetto {
            DifettoChiusura::Io(errore) => PairProtocolError::Io(errore),
            DifettoChiusura::Trailer => PairProtocolError::InvalidTrailer,
            DifettoChiusura::Checksum => PairProtocolError::ChecksumMismatch,
            DifettoChiusura::ByteResidui => PairProtocolError::TrailingBytes,
        }
    })?;
    Ok(pairs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug)]
    struct AlwaysFail;

    impl Write for AlwaysFail {
        fn write(&mut self, _bytes: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("intentional write failure"))
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Err(std::io::Error::other("intentional flush failure"))
        }
    }

    #[test]
    fn roundtrip_and_checksum() {
        let expected = vec![
            JoinPair { left: 0, right: 4 },
            JoinPair { left: 3, right: 9 },
        ];
        let encoded = write_pairs(Vec::new(), &expected).unwrap().0;
        assert_eq!(read_pairs(encoded.as_slice()).unwrap(), expected);

        let mut corrupt = encoded;
        corrupt[20] ^= 1;
        assert!(matches!(
            read_pairs(corrupt.as_slice()),
            Err(PairProtocolError::ChecksumMismatch)
        ));
    }

    #[test]
    fn rejects_truncation_and_trailing_bytes() {
        let encoded = write_pairs(Vec::new(), &[JoinPair { left: 1, right: 2 }])
            .unwrap()
            .0;
        for cut in 1..=40 {
            assert!(read_pairs(&encoded[..encoded.len() - cut]).is_err());
        }
        let mut extra = encoded;
        extra.push(0);
        assert!(matches!(
            read_pairs(extra.as_slice()),
            Err(PairProtocolError::TrailingBytes)
        ));
    }

    /// La chiusura di `PLNPAIR1`, un difetto per volta e con la variante
    /// attesa. Le attese restano qui: quelle di `PLNGEO2` provano un altro
    /// formato, e farle coincidere per costruzione le renderebbe una sola.
    #[test]
    fn ogni_difetto_della_chiusura_ha_la_propria_variante() {
        let valido = write_pairs(Vec::new(), &[JoinPair { left: 1, right: 2 }])
            .expect("scrittura")
            .0;
        let inizio_trailer = valido.len() - 40;
        let inizio_digest = valido.len() - 32;

        let mut magic = valido.clone();
        magic[0] ^= 0x01;
        assert!(matches!(
            read_pairs(magic.as_slice()),
            Err(PairProtocolError::InvalidMagic)
        ));

        let mut trailer = valido.clone();
        trailer[inizio_trailer] ^= 0x01;
        assert!(matches!(
            read_pairs(trailer.as_slice()),
            Err(PairProtocolError::InvalidTrailer)
        ));

        let mut checksum = valido.clone();
        checksum[inizio_digest] ^= 0x01;
        assert!(matches!(
            read_pairs(checksum.as_slice()),
            Err(PairProtocolError::ChecksumMismatch)
        ));

        let mut residui = valido.clone();
        residui.push(0);
        assert!(matches!(
            read_pairs(residui.as_slice()),
            Err(PairProtocolError::TrailingBytes)
        ));

        assert!(matches!(
            read_pairs(&valido[..inizio_digest + 4]),
            Err(PairProtocolError::Io(_))
        ));
    }

    #[test]
    fn bounds_and_writer_io_errors_are_explicit() {
        assert!(matches!(
            header_bytes(MAX_PAIRS + 1),
            Err(PairProtocolError::TooManyPairs(_))
        ));
        assert!(matches!(
            write_pairs(AlwaysFail, &[JoinPair { left: 0, right: 0 }]),
            Err(PairProtocolError::Io(_))
        ));
        assert!(matches!(
            read_pairs([].as_slice()),
            Err(PairProtocolError::Io(_))
        ));
    }
}
