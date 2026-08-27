//! Checksummed, bounded framing for geometry payloads.

use std::io::{Read, Write};

use sha2::{Digest, Sha256};
use thiserror::Error;

use super::framing::{self, DifettoChiusura};

pub const PROTOCOL_MAGIC: &[u8; 8] = b"PLNGEO2\0";
pub const TRAILER_MAGIC: &[u8; 8] = b"GEOEND2\0";
pub const NULL_FRAME_LENGTH: u32 = u32::MAX;
pub const MAX_GEOMETRY_BYTES: u32 = 64 * 1024 * 1024;
pub const MAX_STREAM_BYTES: u64 = 16 * 1024 * 1024 * 1024;
pub const MAX_ROWS: u64 = 100_000_000;

#[derive(Debug, Error)]
pub enum ProtocolError {
    #[error("errore I/O protocollo: {0}")]
    Io(#[from] std::io::Error),
    #[error("magic protocollo Plenora-Geo non valido")]
    InvalidMagic,
    #[error("trailer protocollo Plenora-Geo non valido")]
    InvalidTrailer,
    #[error("checksum protocollo Plenora-Geo non valido")]
    ChecksumMismatch,
    #[error("row_count {0} oltre il limite {MAX_ROWS}")]
    TooManyRows(u64),
    #[error("row_count non coerente: schema={schema}, stream={stream}")]
    RowCountMismatch { schema: u64, stream: u64 },
    #[error("geometria da {0} byte oltre il limite {MAX_GEOMETRY_BYTES}")]
    GeometryTooLarge(u32),
    #[error("stream oltre il limite di {MAX_STREAM_BYTES} byte")]
    StreamTooLarge,
    #[error("frame richiesti oltre il row_count dichiarato")]
    TooManyFrames,
    #[error("frame mancanti: attesi {expected}, scritti {actual}")]
    MissingFrames { expected: u64, actual: u64 },
    #[error("byte inattesi dopo il trailer")]
    TrailingBytes,
}

#[derive(Debug)]
pub enum Frame {
    Null,
    Wkb(Vec<u8>),
}

fn header_bytes(rows: u64) -> [u8; 16] {
    framing::header(*PROTOCOL_MAGIC, rows)
}

pub struct FrameReader<R> {
    inner: R,
    expected_rows: u64,
    rows_read: u64,
    total_bytes: u64,
    hasher: Sha256,
    verified: bool,
}

impl<R: Read> FrameReader<R> {
    /// Costruisce il lettore: legge l'header, verifica magic e limiti e
    /// controlla che il `row_count` dello stream coincida con quello atteso.
    ///
    /// # Errors
    ///
    /// - `ProtocolError::TooManyRows`: `schema_rows` o il `row_count` dello
    ///   stream oltre `MAX_ROWS`;
    /// - `ProtocolError::InvalidMagic`: magic dell'header non corrispondente;
    /// - `ProtocolError::RowCountMismatch`: `row_count` di schema e stream
    ///   diversi;
    /// - `ProtocolError::Io`: stream troncato o errore di lettura.
    pub fn new(mut inner: R, schema_rows: u64) -> Result<Self, ProtocolError> {
        if schema_rows > MAX_ROWS {
            return Err(ProtocolError::TooManyRows(schema_rows));
        }
        let mut header = [0_u8; 16];
        inner.read_exact(&mut header)?;
        let Some(stream_rows) = framing::contatore_dichiarato(&header, *PROTOCOL_MAGIC) else {
            return Err(ProtocolError::InvalidMagic);
        };
        if stream_rows > MAX_ROWS {
            return Err(ProtocolError::TooManyRows(stream_rows));
        }
        if stream_rows != schema_rows {
            return Err(ProtocolError::RowCountMismatch {
                schema: schema_rows,
                stream: stream_rows,
            });
        }
        let mut hasher = Sha256::new();
        hasher.update(header);
        Ok(Self {
            inner,
            expected_rows: stream_rows,
            rows_read: 0,
            total_bytes: 0,
            hasher,
            verified: false,
        })
    }

    /// Legge il frame successivo; esaurite le righe attese verifica trailer
    /// e checksum una sola volta, poi restituisce `Ok(None)`.
    ///
    /// # Errors
    ///
    /// - `ProtocolError::GeometryTooLarge`: frame oltre `MAX_GEOMETRY_BYTES`;
    /// - `ProtocolError::StreamTooLarge`: totale dello stream oltre
    ///   `MAX_STREAM_BYTES`;
    /// - `ProtocolError::InvalidTrailer` / `ChecksumMismatch` /
    ///   `TrailingBytes`: chiusura dello stream non valida;
    /// - `ProtocolError::Io`: stream troncato o errore di lettura.
    pub fn next_frame(&mut self) -> Result<Option<Frame>, ProtocolError> {
        if self.rows_read == self.expected_rows {
            if !self.verified {
                self.verify_trailer()?;
            }
            return Ok(None);
        }

        let mut length_bytes = [0_u8; 4];
        self.inner.read_exact(&mut length_bytes)?;
        self.hasher.update(length_bytes);
        let length = u32::from_le_bytes(length_bytes);
        self.rows_read += 1;
        if length == NULL_FRAME_LENGTH {
            return Ok(Some(Frame::Null));
        }
        if length > MAX_GEOMETRY_BYTES {
            return Err(ProtocolError::GeometryTooLarge(length));
        }
        self.total_bytes = self
            .total_bytes
            .checked_add(u64::from(length))
            .ok_or(ProtocolError::StreamTooLarge)?;
        if self.total_bytes > MAX_STREAM_BYTES {
            return Err(ProtocolError::StreamTooLarge);
        }
        let mut payload = vec![0_u8; length as usize];
        self.inner.read_exact(&mut payload)?;
        self.hasher.update(&payload);
        Ok(Some(Frame::Wkb(payload)))
    }

    fn verify_trailer(&mut self) -> Result<(), ProtocolError> {
        let digest: [u8; 32] = self.hasher.clone().finalize().into();
        framing::verifica_chiusura(&mut self.inner, *TRAILER_MAGIC, &digest).map_err(
            |difetto| match difetto {
                DifettoChiusura::Io(errore) => ProtocolError::Io(errore),
                DifettoChiusura::Trailer => ProtocolError::InvalidTrailer,
                DifettoChiusura::Checksum => ProtocolError::ChecksumMismatch,
                DifettoChiusura::ByteResidui => ProtocolError::TrailingBytes,
            },
        )?;
        self.verified = true;
        Ok(())
    }
}

pub struct FrameWriter<W> {
    inner: W,
    expected_rows: u64,
    rows_written: u64,
    total_bytes: u64,
    hasher: Sha256,
}

impl<W: Write> FrameWriter<W> {
    /// Costruisce lo scrittore e scrive l'header con il `row_count` atteso.
    ///
    /// # Errors
    ///
    /// - `ProtocolError::TooManyRows`: `expected_rows` oltre `MAX_ROWS`;
    /// - `ProtocolError::Io`: errore di scrittura dell'header.
    pub fn new(mut inner: W, expected_rows: u64) -> Result<Self, ProtocolError> {
        if expected_rows > MAX_ROWS {
            return Err(ProtocolError::TooManyRows(expected_rows));
        }
        let header = header_bytes(expected_rows);
        inner.write_all(&header)?;
        let mut hasher = Sha256::new();
        hasher.update(header);
        Ok(Self {
            inner,
            expected_rows,
            rows_written: 0,
            total_bytes: 0,
            hasher,
        })
    }

    /// Scrive un frame (`None` per null) aggiornando il checksum
    /// incrementale e i contatori dei limiti.
    ///
    /// # Errors
    ///
    /// - `ProtocolError::TooManyFrames`: frame oltre il `row_count` dichiarato;
    /// - `ProtocolError::GeometryTooLarge`: payload oltre
    ///   `MAX_GEOMETRY_BYTES`;
    /// - `ProtocolError::StreamTooLarge`: totale dello stream oltre
    ///   `MAX_STREAM_BYTES`;
    /// - `ProtocolError::Io`: errore di scrittura.
    pub fn write_frame(&mut self, frame: Option<&[u8]>) -> Result<(), ProtocolError> {
        if self.rows_written >= self.expected_rows {
            return Err(ProtocolError::TooManyFrames);
        }
        self.rows_written += 1;
        match frame {
            None => {
                let length = NULL_FRAME_LENGTH.to_le_bytes();
                self.inner.write_all(&length)?;
                self.hasher.update(length);
            }
            Some(payload) => {
                let length = u32::try_from(payload.len())
                    .map_err(|_| ProtocolError::GeometryTooLarge(u32::MAX - 1))?;
                if length > MAX_GEOMETRY_BYTES {
                    return Err(ProtocolError::GeometryTooLarge(length));
                }
                self.total_bytes = self
                    .total_bytes
                    .checked_add(u64::from(length))
                    .ok_or(ProtocolError::StreamTooLarge)?;
                if self.total_bytes > MAX_STREAM_BYTES {
                    return Err(ProtocolError::StreamTooLarge);
                }
                let length_bytes = length.to_le_bytes();
                self.inner.write_all(&length_bytes)?;
                self.inner.write_all(payload)?;
                self.hasher.update(length_bytes);
                self.hasher.update(payload);
            }
        }
        Ok(())
    }

    /// Chiude lo stream scrivendo trailer e digest; restituisce il writer
    /// sottostante e il checksum.
    ///
    /// # Errors
    ///
    /// - `ProtocolError::MissingFrames`: frame scritti diversi dal `row_count`
    ///   dichiarato;
    /// - `ProtocolError::Io`: errore di scrittura o flush.
    pub fn finish(mut self) -> Result<(W, [u8; 32]), ProtocolError> {
        if self.rows_written != self.expected_rows {
            return Err(ProtocolError::MissingFrames {
                expected: self.expected_rows,
                actual: self.rows_written,
            });
        }
        let digest: [u8; 32] = self.hasher.finalize().into();
        framing::scrivi_chiusura(&mut self.inner, *TRAILER_MAGIC, &digest)?;
        Ok((self.inner, digest))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use std::io;

    #[derive(Debug)]
    struct FailAfter {
        limit: usize,
        written: usize,
        fail_flush: bool,
    }

    impl Write for FailAfter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if self.written >= self.limit {
                return Err(io::Error::other("intentional write failure"));
            }
            let count = bytes.len().min(self.limit - self.written);
            self.written += count;
            Ok(count)
        }

        fn flush(&mut self) -> io::Result<()> {
            if self.fail_flush {
                Err(io::Error::other("intentional flush failure"))
            } else {
                Ok(())
            }
        }
    }

    fn valid_stream(frames: &[Option<&[u8]>]) -> Vec<u8> {
        let mut writer = FrameWriter::new(Vec::new(), frames.len() as u64).expect("writer");
        for frame in frames {
            writer.write_frame(*frame).expect("frame");
        }
        writer.finish().expect("finish").0
    }

    #[test]
    fn roundtrip_preserves_frames_and_nulls() {
        let stream = valid_stream(&[Some(b"abc"), None, Some(b"xyz")]);
        let mut reader = FrameReader::new(stream.as_slice(), 3).expect("reader");
        assert!(matches!(reader.next_frame().unwrap(), Some(Frame::Wkb(v)) if v == b"abc"));
        assert!(matches!(reader.next_frame().unwrap(), Some(Frame::Null)));
        assert!(matches!(reader.next_frame().unwrap(), Some(Frame::Wkb(v)) if v == b"xyz"));
        assert!(reader.next_frame().unwrap().is_none());
    }

    #[test]
    fn checksum_detects_single_byte_corruption() {
        let mut stream = valid_stream(&[Some(b"abcdef")]);
        let payload_offset = 16 + 4;
        stream[payload_offset + 2] ^= 0x01;
        let mut reader = FrameReader::new(stream.as_slice(), 1).expect("reader");
        assert!(reader.next_frame().unwrap().is_some());
        assert!(matches!(
            reader.next_frame(),
            Err(ProtocolError::ChecksumMismatch)
        ));
    }

    #[test]
    fn truncation_and_extra_bytes_fail_closed() {
        let stream = valid_stream(&[Some(b"abcdef")]);
        for cut in 1..=40 {
            let truncated = &stream[..stream.len() - cut];
            let mut reader = FrameReader::new(truncated, 1).expect("header");
            let _ = reader.next_frame();
            assert!(reader.next_frame().is_err(), "cut={cut}");
        }

        let mut extra = stream;
        extra.push(0);
        let mut reader = FrameReader::new(extra.as_slice(), 1).expect("reader");
        assert!(reader.next_frame().unwrap().is_some());
        assert!(matches!(
            reader.next_frame(),
            Err(ProtocolError::TrailingBytes)
        ));
    }

    /// La chiusura di `PLNGEO2`, un difetto per volta e con la variante attesa.
    ///
    /// Senza la variante, una traduzione scambiata — trailer letto come
    /// checksum — resterebbe verde: i test di troncamento guardano solo che
    /// ci sia un errore, non quale.
    #[test]
    fn ogni_difetto_della_chiusura_ha_la_propria_variante() {
        let valido = valid_stream(&[Some(b"abcdef")]);
        let inizio_trailer = valido.len() - 40;
        let inizio_digest = valido.len() - 32;

        let mut magic = valido.clone();
        magic[0] ^= 0x01;
        assert!(matches!(
            FrameReader::new(magic.as_slice(), 1),
            Err(ProtocolError::InvalidMagic)
        ));

        let mut trailer = valido.clone();
        trailer[inizio_trailer] ^= 0x01;
        assert!(matches!(
            dopo_il_primo_frame(&trailer),
            Err(ProtocolError::InvalidTrailer)
        ));

        let mut checksum = valido.clone();
        checksum[inizio_digest] ^= 0x01;
        assert!(matches!(
            dopo_il_primo_frame(&checksum),
            Err(ProtocolError::ChecksumMismatch)
        ));

        let mut residui = valido.clone();
        residui.push(0);
        assert!(matches!(
            dopo_il_primo_frame(&residui),
            Err(ProtocolError::TrailingBytes)
        ));

        // Chiusura troncata a meta': il canale e' finito prima, e questo e'
        // un errore di I/O, non una violazione del protocollo.
        let troncato = &valido[..inizio_digest + 4];
        assert!(matches!(
            dopo_il_primo_frame(troncato),
            Err(ProtocolError::Io(_))
        ));
    }

    /// Consuma il primo frame e rende cio' che dice il secondo, cioe' la
    /// chiusura.
    fn dopo_il_primo_frame(stream: &[u8]) -> Result<Option<Frame>, ProtocolError> {
        let mut reader = FrameReader::new(stream, 1).expect("header valido");
        reader.next_frame().expect("primo frame");
        reader.next_frame()
    }

    #[test]
    fn schema_and_stream_row_counts_must_match() {
        let stream = valid_stream(&[None, None]);
        assert!(matches!(
            FrameReader::new(stream.as_slice(), 1),
            Err(ProtocolError::RowCountMismatch { .. })
        ));
    }

    #[test]
    fn all_io_failures_and_resource_guards_fail_closed() {
        assert!(matches!(
            FrameReader::new([].as_slice(), 0),
            Err(ProtocolError::Io(_))
        ));
        assert!(matches!(
            FrameReader::new([0_u8; 16].as_slice(), MAX_ROWS + 1),
            Err(ProtocolError::TooManyRows(_))
        ));
        let mut too_many_rows = header_bytes(MAX_ROWS + 1).to_vec();
        too_many_rows.extend_from_slice(TRAILER_MAGIC);
        too_many_rows.extend_from_slice(&[0; 32]);
        assert!(matches!(
            FrameReader::new(too_many_rows.as_slice(), MAX_ROWS),
            Err(ProtocolError::TooManyRows(_))
        ));

        assert!(matches!(
            FrameWriter::new(
                FailAfter {
                    limit: 0,
                    written: 0,
                    fail_flush: false,
                },
                0,
            ),
            Err(ProtocolError::Io(_))
        ));
        let mut writer = FrameWriter::new(
            FailAfter {
                limit: 20,
                written: 0,
                fail_flush: false,
            },
            1,
        )
        .unwrap();
        assert!(matches!(
            writer.write_frame(Some(b"x")),
            Err(ProtocolError::Io(_))
        ));

        let mut writer = FrameWriter::new(
            FailAfter {
                limit: usize::MAX,
                written: 0,
                fail_flush: true,
            },
            0,
        )
        .unwrap();
        writer.total_bytes = 0;
        assert!(matches!(writer.finish(), Err(ProtocolError::Io(_))));

        let mut overflow_writer = FrameWriter {
            inner: Vec::new(),
            expected_rows: 1,
            rows_written: 0,
            total_bytes: u64::MAX,
            hasher: Sha256::new(),
        };
        assert!(matches!(
            overflow_writer.write_frame(Some(b"x")),
            Err(ProtocolError::StreamTooLarge)
        ));
        let mut limit_writer = FrameWriter {
            inner: Vec::new(),
            expected_rows: 1,
            rows_written: 0,
            total_bytes: MAX_STREAM_BYTES,
            hasher: Sha256::new(),
        };
        assert!(matches!(
            limit_writer.write_frame(Some(b"x")),
            Err(ProtocolError::StreamTooLarge)
        ));

        let frame = [1_u8, 0, 0, 0, 0];
        for total_bytes in [u64::MAX, MAX_STREAM_BYTES] {
            let mut reader = FrameReader {
                inner: frame.as_slice(),
                expected_rows: 1,
                rows_read: 0,
                total_bytes,
                hasher: Sha256::new(),
                verified: false,
            };
            assert!(matches!(
                reader.next_frame(),
                Err(ProtocolError::StreamTooLarge)
            ));
        }

        let stream = valid_stream(&[]);
        let mut reader = FrameReader::new(stream.as_slice(), 0).unwrap();
        assert!(reader.next_frame().unwrap().is_none());
        assert!(reader.next_frame().unwrap().is_none());
    }

    proptest! {
        #[test]
        fn arbitrary_frames_roundtrip_without_byte_loss(
            frames in prop::collection::vec(
                prop::option::of(prop::collection::vec(any::<u8>(), 0..4096)),
                0..128,
            )
        ) {
            let borrowed: Vec<Option<&[u8]>> = frames
                .iter()
                .map(|frame| frame.as_deref())
                .collect();
            let stream = valid_stream(&borrowed);
            let mut reader = FrameReader::new(stream.as_slice(), frames.len() as u64)
                .expect("reader");
            for expected in &frames {
                match (reader.next_frame().expect("frame"), expected) {
                    (Some(Frame::Null), None) => {}
                    (Some(Frame::Wkb(actual)), Some(expected)) => {
                        prop_assert_eq!(&actual, expected);
                    }
                    (actual, expected) => {
                        prop_assert!(false, "frame mismatch: {actual:?} != {expected:?}");
                    }
                }
            }
            prop_assert!(reader.next_frame().expect("trailer").is_none());
        }
    }
}
