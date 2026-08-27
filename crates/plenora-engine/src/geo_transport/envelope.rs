//! Envelope checksummed `PLNGEO3`: lettore e scrittore con hasher
//! incrementale.

use std::io::{Read, Write};

use sha2::{Digest, Sha256};

use super::error::ArrowTransportError;
use super::framing::{self, DifettoChiusura};
use super::protocol::MAX_STREAM_BYTES;
use super::transport::{ENVELOPE_MAGIC, ENVELOPE_TRAILER_MAGIC};

const PAYLOAD_CHUNK_BYTES: u64 = 8 * 1024 * 1024;

/// Lettore dell'envelope v3 con hasher incrementale, nello stile di
/// `protocol::FrameReader`.
pub struct EnvelopeReader<R> {
    inner: R,
    hasher: Sha256,
    payload_len: u64,
}

impl<R: Read> EnvelopeReader<R> {
    /// Costruisce il lettore e verifica magic e lunghezza dichiarata.
    ///
    /// # Errors
    ///
    /// `ArrowTransportError::InvalidMagic` se il magic non corrisponde,
    /// `ArrowTransportError::StreamTooLarge` se il payload dichiarato supera
    /// `MAX_STREAM_BYTES`, `ArrowTransportError::Io` per errori di lettura.
    pub fn new(mut inner: R) -> Result<Self, ArrowTransportError> {
        let mut header = [0_u8; 16];
        inner.read_exact(&mut header)?;
        let Some(payload_len) = framing::contatore_dichiarato(&header, *ENVELOPE_MAGIC) else {
            return Err(ArrowTransportError::InvalidMagic);
        };
        if payload_len > MAX_STREAM_BYTES {
            return Err(ArrowTransportError::StreamTooLarge);
        }
        let mut hasher = Sha256::new();
        hasher.update(header);
        Ok(Self {
            inner,
            hasher,
            payload_len,
        })
    }

    /// Legge il payload a chunk, cosi' la memoria cresce solo con i byte che
    /// arrivano davvero, e verifica trailer, checksum e byte residui.
    ///
    /// # Errors
    ///
    /// `ArrowTransportError::InvalidTrailer` se il trailer non corrisponde,
    /// `ArrowTransportError::ChecksumMismatch` se il digest non coincide,
    /// `ArrowTransportError::TrailingBytes` se restano byte dopo il trailer,
    /// `ArrowTransportError::Io` per errori di lettura.
    pub fn read_payload(mut self) -> Result<Vec<u8>, ArrowTransportError> {
        let mut payload = Vec::new();
        let mut remaining = self.payload_len;
        while remaining > 0 {
            let take = remaining.min(PAYLOAD_CHUNK_BYTES) as usize;
            let start = payload.len();
            payload.resize(start + take, 0);
            self.inner.read_exact(&mut payload[start..])?;
            remaining -= take as u64;
        }
        self.hasher.update(&payload);

        let digest: [u8; 32] = self.hasher.finalize().into();
        framing::verifica_chiusura(&mut self.inner, *ENVELOPE_TRAILER_MAGIC, &digest).map_err(
            |difetto| match difetto {
                DifettoChiusura::Io(errore) => ArrowTransportError::Io(errore),
                DifettoChiusura::Trailer => ArrowTransportError::InvalidTrailer,
                DifettoChiusura::Checksum => ArrowTransportError::ChecksumMismatch,
                DifettoChiusura::ByteResidui => ArrowTransportError::TrailingBytes,
            },
        )?;
        Ok(payload)
    }
}

/// Scrittore dell'envelope v3 con lunghezza dichiarata e hasher incrementale.
pub struct EnvelopeWriter<W> {
    inner: W,
    hasher: Sha256,
    payload_len: u64,
    written: u64,
}

impl<W: Write> EnvelopeWriter<W> {
    /// Costruisce lo scrittore e scrive l'header con la lunghezza dichiarata.
    ///
    /// # Errors
    ///
    /// `ArrowTransportError::StreamTooLarge` se `payload_len` supera
    /// `MAX_STREAM_BYTES`, `ArrowTransportError::Io` per errori di scrittura.
    pub fn new(mut inner: W, payload_len: u64) -> Result<Self, ArrowTransportError> {
        if payload_len > MAX_STREAM_BYTES {
            return Err(ArrowTransportError::StreamTooLarge);
        }
        let header = framing::header(*ENVELOPE_MAGIC, payload_len);
        inner.write_all(&header)?;
        let mut hasher = Sha256::new();
        hasher.update(header);
        Ok(Self {
            inner,
            hasher,
            payload_len,
            written: 0,
        })
    }

    /// Accoda un chunk di payload aggiornando il checksum incrementale.
    ///
    /// # Errors
    ///
    /// `ArrowTransportError::StreamTooLarge` se i byte scritti superano la
    /// lunghezza dichiarata, `ArrowTransportError::Io` per errori di
    /// scrittura.
    pub fn write_payload(&mut self, bytes: &[u8]) -> Result<(), ArrowTransportError> {
        let next = self
            .written
            .checked_add(bytes.len() as u64)
            .filter(|next| *next <= self.payload_len)
            .ok_or(ArrowTransportError::StreamTooLarge)?;
        self.inner.write_all(bytes)?;
        self.hasher.update(bytes);
        self.written = next;
        Ok(())
    }

    /// Chiude l'envelope scrivendo trailer e digest; restituisce il writer
    /// sottostante e il checksum.
    ///
    /// # Errors
    ///
    /// `ArrowTransportError::PayloadLengthMismatch` se i byte scritti non
    /// coincidono con la lunghezza dichiarata, `ArrowTransportError::Io` per
    /// errori di scrittura o flush.
    pub fn finish(mut self) -> Result<(W, [u8; 32]), ArrowTransportError> {
        if self.written != self.payload_len {
            return Err(ArrowTransportError::PayloadLengthMismatch {
                declared: self.payload_len,
                written: self.written,
            });
        }
        let digest: [u8; 32] = self.hasher.finalize().into();
        framing::scrivi_chiusura(&mut self.inner, *ENVELOPE_TRAILER_MAGIC, &digest)?;
        Ok((self.inner, digest))
    }
}
