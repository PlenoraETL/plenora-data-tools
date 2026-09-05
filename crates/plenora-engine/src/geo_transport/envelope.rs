//! Envelope checksummed `PLNGEO3`: lettore e scrittore con hasher
//! incrementale.

use std::io::{Read, Write};

use sha2::{Digest, Sha256};

use super::error::ArrowTransportError;
use super::framing::{self, DifettoChiusura};
use super::protocol::MAX_STREAM_BYTES;
use super::transport::{ENVELOPE_MAGIC, ENVELOPE_TRAILER_MAGIC};

/// Buffer di lettura del payload: **fisso, piccolo e riusato**.
///
/// Non e' un tetto sul payload — quello e' [`MAX_STREAM_BYTES`] — ed e' la
/// ragione per cui e' piccolo: dimensiona una `read`, non un risultato. Un
/// buffer grande quanto un chunk andrebbe allocato e azzerato **prima** di
/// sapere se quei byte esistano, e sedici byte di header con una lunghezza
/// dichiarata qualsiasi basterebbero a farne toccare milioni.
///
/// 16 KiB e' il piu' grande che il progetto ammetta sullo stack — oltre,
/// `clippy::large_stack_arrays` lo rifiuta — ed e' gia' ben oltre la taglia
/// dove il costo per byte di una `read` smette di migliorare. Sullo stack e
/// non nello heap perche' cosi' non chiede niente all'allocatore, che e'
/// proprio la risorsa che un buffer dimensionato sul dichiarato
/// sovraccaricherebbe.
const BUFFER_LETTURA_BYTES: usize = 16 * 1024;

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

    /// Legge il payload e verifica trailer, checksum e byte residui.
    ///
    /// # La memoria cresce con i byte letti, non con quelli dichiarati
    ///
    /// Il payload si accumula **solo** con i byte che una `read` ha davvero
    /// reso. `payload_len` viene dall'ingresso: fidarsene per dimensionare
    /// un'allocazione significa lasciare che sia l'ingresso a decidere quanta
    /// memoria si tocca, e il tetto di [`MAX_STREAM_BYTES`] governa quanto
    /// l'ingresso puo' **dichiarare**, non quanto noi allochiamo subito.
    ///
    /// Ne segue che non si usa ne' `with_capacity(payload_len)` ne' un buffer
    /// temporaneo grande quanto un chunk: entrambi rimetterebbero
    /// l'amplificazione: pochi byte di ingresso, molti megabyte allocati e
    /// azzerati. Il buffer e' **fisso, piccolo e riusato** a ogni giro.
    ///
    /// # Errors
    ///
    /// `ArrowTransportError::InvalidTrailer` se il trailer non corrisponde,
    /// `ArrowTransportError::ChecksumMismatch` se il digest non coincide,
    /// `ArrowTransportError::TrailingBytes` se restano byte dopo il trailer,
    /// `ArrowTransportError::Io` per errori di lettura — fra cui
    /// `UnexpectedEof` quando la sorgente finisce prima dei byte dichiarati.
    pub fn read_payload(mut self) -> Result<Vec<u8>, ArrowTransportError> {
        let mut payload = Vec::new();
        let mut buffer = [0_u8; BUFFER_LETTURA_BYTES];
        let mut remaining = self.payload_len;
        while remaining > 0 {
            // La finestra chiesta non supera mai il buffer fisso: quanto resta
            // da leggere non dimensiona nulla.
            //
            // La conversione rende un errore invece di ripiegare su un valore:
            // un `unwrap_or` qui nasconderebbe un'impossibilita' dietro un
            // numero plausibile, e chi legge non saprebbe dire se il buffer sia
            // quello voluto o il ripiego.
            let vuole =
                usize::try_from(remaining.min(BUFFER_LETTURA_BYTES as u64)).map_err(|_| {
                    ArrowTransportError::Internal("finestra di lettura non rappresentabile")
                })?;
            let letti = match self.inner.read(&mut buffer[..vuole]) {
                Ok(letti) => letti,
                // Un'interruzione non e' una fine: si riprova senza consumare
                // nulla di cio' che resta.
                Err(errore) if errore.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(errore) => return Err(ArrowTransportError::Io(errore)),
            };
            if letti == 0 {
                // La sorgente e' finita prima dei byte dichiarati: un `Ok(0)`
                // prematuro non e' un successo, ed e' la condizione che
                // `UnexpectedEof` nomina.
                return Err(ArrowTransportError::Io(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "payload dell'envelope piu' corto di quanto dichiarato",
                )));
            }
            if letti > vuole {
                // `Read::read` promette di non rendere piu' byte della fetta
                // che ha ricevuto. La promessa e' del `Read`, non del tipo, e
                // un implementatore rotto la viola.
                //
                // Non si tronca e non si prosegue: accodare la fetta intera
                // farebbe entrare nel payload — e nel digest — byte che
                // nessuno ha scritto, cioe' un risultato sbagliato al posto di
                // un errore.
                //
                // `Io` e non `Internal`: `Internal` nomina un difetto del
                // trasporto, e qui il trasporto fa la cosa giusta — e' il
                // `Read` che gli e' stato dato a rompere il proprio contratto.
                // Stessa lettura e stesso messaggio del lettore dei frame; il
                // messaggio e' costante e non porta ne' taglie ne' contenuti.
                return Err(ArrowTransportError::Io(std::io::Error::other(
                    "il lettore ha reso piu' byte della finestra richiesta",
                )));
            }
            // L'hash si aggiorna **dentro** il ciclo, sui byte appena letti.
            // Farlo alla fine sul payload intero costringerebbe a ripercorrere
            // tutto una seconda volta, e chiamerebbe «incrementale» un hasher
            // che riceve un blocco solo.
            self.hasher.update(&buffer[..letti]);
            payload.extend_from_slice(&buffer[..letti]);
            remaining -= letti as u64;
        }

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

/// Le taglie che il lettore chiede a `read`.
///
/// **Che cosa misurano, e che cosa no.** Osservano la lunghezza delle slice
/// passate a `Read::read`, non l'heap: non contano byte allocati e non
/// userebbero un allocatore strumentato per farlo. Sono validi contro la
/// regressione specifica — un buffer dimensionato sul valore dichiarato
/// dall'ingresso — perche' quella si manifesta per intero nella taglia
/// richiesta. Chiamarli misure dell'allocazione sarebbe dire piu' di quanto
/// facciano.
#[cfg(test)]
mod taglie_delle_letture_richieste {
    use std::io::Read;

    use super::{framing, ArrowTransportError, EnvelopeReader, BUFFER_LETTURA_BYTES};
    use crate::geo_transport::transport::ENVELOPE_MAGIC;

    /// Sorgente che dichiara molto e consegna poco, **e registra ogni
    /// richiesta di lettura**.
    ///
    /// La registrazione e' il punto: un lettore corretto puo' fallire anche
    /// avendo chiesto un buffer enorme, e l'esito da solo non
    /// distinguerebbe i due casi. Sono le taglie richieste a dirlo.
    struct SorgenteTroncata<'a> {
        byte: &'a [u8],
        letto: usize,
        richieste: Vec<usize>,
    }

    impl Read for SorgenteTroncata<'_> {
        fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
            self.richieste.push(out.len());
            let resto = self.byte.len().saturating_sub(self.letto);
            let quanti = resto.min(out.len());
            out[..quanti].copy_from_slice(&self.byte[self.letto..self.letto + quanti]);
            self.letto += quanti;
            Ok(quanti)
        }
    }

    /// Header valido che dichiara `payload_len`, seguito da `payload` byte di
    /// riempimento — di norma molti meno di quanti ne dichiara.
    fn ingresso(payload_len: u64, payload: usize) -> Vec<u8> {
        let mut byte = framing::header(*ENVELOPE_MAGIC, payload_len).to_vec();
        byte.extend(std::iter::repeat_n(0x41_u8, payload));
        byte
    }

    /// Un payload dichiarato enorme con sorgente troncata **fallisce**, e
    /// nessuna singola lettura e' grande quanto il dichiarato.
    ///
    /// Un lettore che dimensionasse il buffer sul valore dichiarato
    /// allocherebbe e azzererebbe un chunk intero prima di sapere se quei byte
    /// esistano: sedici byte di header basterebbero a farne toccare milioni.
    /// Il caso lo sorveglia dalle **taglie richieste**, non dall'esito.
    #[test]
    fn un_payload_dichiarato_enorme_non_dimensiona_le_letture() {
        const DICHIARATI: u64 = 4 * 1024 * 1024 * 1024;

        let byte = ingresso(DICHIARATI, 32);
        let mut sorgente = SorgenteTroncata {
            byte: &byte,
            letto: 0,
            richieste: Vec::new(),
        };
        let lettore = EnvelopeReader::new(&mut sorgente).expect("header valido");
        let esito = lettore.read_payload();

        assert!(
            matches!(esito, Err(ArrowTransportError::Io(_))),
            "una sorgente troncata deve fallire: {esito:?}"
        );
        let massima = sorgente.richieste.iter().copied().max().unwrap_or(0);
        assert!(
            massima <= BUFFER_LETTURA_BYTES,
            "una lettura ha chiesto {massima} byte, oltre il buffer fisso di {BUFFER_LETTURA_BYTES}"
        );
        assert!(
            u64::try_from(massima).unwrap_or(u64::MAX) < DICHIARATI,
            "la richiesta segue il dichiarato invece del buffer"
        );
    }

    /// La stessa prova con il vecchio chunk come metro: nessuna lettura si
    /// avvicina a 8 MiB.
    ///
    /// Vale come regressione esplicita: se qualcuno reintroducesse un buffer
    /// grande quanto il chunk, questo caso lo direbbe.
    #[test]
    fn nessuna_lettura_e_grande_quanto_il_vecchio_chunk() {
        const VECCHIO_CHUNK: usize = 8 * 1024 * 1024;

        let byte = ingresso(64 * 1024 * 1024, 16);
        let mut sorgente = SorgenteTroncata {
            byte: &byte,
            letto: 0,
            richieste: Vec::new(),
        };
        let lettore = EnvelopeReader::new(&mut sorgente).expect("header valido");
        let _ = lettore.read_payload();

        for richiesta in &sorgente.richieste {
            assert!(
                *richiesta < VECCHIO_CHUNK,
                "una lettura ha chiesto {richiesta} byte, quanto il chunk di prima"
            );
        }
    }

    /// Un `Ok(0)` prematuro diventa `UnexpectedEof`, non un successo.
    #[test]
    fn la_sorgente_che_finisce_prima_e_un_errore_non_un_payload_corto() {
        let byte = ingresso(1024, 10);
        let mut sorgente = SorgenteTroncata {
            byte: &byte,
            letto: 0,
            richieste: Vec::new(),
        };
        let lettore = EnvelopeReader::new(&mut sorgente).expect("header valido");
        match lettore.read_payload() {
            Err(ArrowTransportError::Io(errore)) => {
                assert_eq!(errore.kind(), std::io::ErrorKind::UnexpectedEof, "{errore}");
            }
            altro => panic!("atteso UnexpectedEof, ottenuto {altro:?}"),
        }
    }
}

/// Il ramo `Interrupted` e le letture parziali.
#[cfg(test)]
mod letture_parziali_e_interruzioni {
    use std::io::{Error, ErrorKind, Read};

    use super::{ArrowTransportError, EnvelopeReader};

    /// Sorgente che consegna **un copione**: a ogni `read` esegue la mossa
    /// successiva, cosi' i due comportamenti che `read_exact` assorbe da se' —
    /// interruzione e lettura parziale — restano osservabili qui, dove sono
    /// responsabilita' del ciclo.
    struct SorgenteACopione<'a> {
        byte: &'a [u8],
        letto: usize,
        /// `None` = interruzione; `Some(n)` = consegna al piu' `n` byte.
        copione: std::vec::IntoIter<Option<usize>>,
    }

    impl Read for SorgenteACopione<'_> {
        fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
            match self.copione.next() {
                Some(None) => Err(Error::new(ErrorKind::Interrupted, "interrotta")),
                mossa => {
                    let tetto = match mossa {
                        Some(Some(n)) => n,
                        _ => out.len(),
                    };
                    let resto = self.byte.len().saturating_sub(self.letto);
                    let quanti = resto.min(out.len()).min(tetto);
                    out[..quanti].copy_from_slice(&self.byte[self.letto..self.letto + quanti]);
                    self.letto += quanti;
                    Ok(quanti)
                }
            }
        }
    }

    /// Envelope completo e valido: header, payload, trailer con checksum.
    fn envelope(payload: &[u8]) -> Vec<u8> {
        let mut byte = Vec::new();
        {
            let mut scrittore =
                super::EnvelopeWriter::new(&mut byte, payload.len() as u64).expect("writer");
            scrittore.write_payload(payload).expect("payload");
            scrittore.finish().expect("finish");
        }
        byte
    }

    /// Un'interruzione **non consuma nulla** e non fa perdere byte: il payload
    /// riletto e' identico all'originale.
    ///
    /// Con `read_exact` il ramo non sarebbe nostro: lo gestisce la libreria.
    /// Con `read` lo e', e un `continue` sbagliato — che scartasse i byte gia'
    /// letti, o che li ricontasse — romperebbe il checksum invece di
    /// somigliare a un errore di lettura.
    #[test]
    fn un_interruzione_non_perde_byte() {
        let atteso: Vec<u8> = (0..5000_u32).map(|i| (i % 251) as u8).collect();
        let byte = envelope(&atteso);
        let mut sorgente = SorgenteACopione {
            byte: &byte,
            letto: 0,
            // header, poi interruzione, poi due letture parziali, poi il resto.
            copione: vec![None, Some(700), None, Some(1300), None].into_iter(),
        };
        let lettore = EnvelopeReader::new(&mut sorgente).expect("header valido");
        let letto = lettore.read_payload().expect("payload valido");
        assert_eq!(letto, atteso, "l'interruzione ha alterato il payload");
    }

    /// Letture parziali ripetute compongono comunque il payload esatto, e il
    /// checksum regge: l'hash incrementale vede gli stessi byte nello stesso
    /// ordine, comunque siano spezzati.
    #[test]
    fn le_letture_parziali_compongono_il_payload_e_il_checksum_regge() {
        let atteso: Vec<u8> = (0..9000_u32).map(|i| (i % 253) as u8).collect();
        let byte = envelope(&atteso);
        let mut sorgente = SorgenteACopione {
            byte: &byte,
            letto: 0,
            copione: vec![Some(1), Some(7), Some(64), Some(4096), Some(3)].into_iter(),
        };
        let lettore = EnvelopeReader::new(&mut sorgente).expect("header valido");
        match lettore.read_payload() {
            Ok(letto) => assert_eq!(letto, atteso),
            Err(ArrowTransportError::ChecksumMismatch) => {
                panic!("l'hash incrementale non ha visto gli stessi byte")
            }
            Err(altro) => panic!("payload valido rifiutato: {altro:?}"),
        }
    }
}

/// Un `Read` che viola il proprio contratto non produce byte validi.
///
/// `Read::read` promette di non rendere piu' byte della fetta ricevuta. Non
/// e' una promessa che il tipo garantisce: un implementatore rotto — o un
/// wrapper scritto male a valle — puo' dichiarare di aver riempito piu' di
/// quanto gli e' stato dato.
///
/// Troncare quella dichiarazione e proseguire sarebbe la scelta comoda, e
/// sarebbe la scelta sbagliata: farebbe entrare nel payload e nel digest byte
/// che nessuno ha scritto, cioe' un risultato **sbagliato** al posto di un
/// errore. I due lettori si fermano, e questi casi lo pretendono.
#[cfg(test)]
mod un_read_scorretto_non_passa_per_valido {
    use std::io::Read;

    use super::{ArrowTransportError, EnvelopeReader, EnvelopeWriter};

    /// Envelope completo e valido: header, payload, trailer con checksum.
    fn envelope(payload: &[u8]) -> Vec<u8> {
        let mut byte = Vec::new();
        {
            let mut scrittore =
                EnvelopeWriter::new(&mut byte, payload.len() as u64).expect("writer");
            scrittore.write_payload(payload).expect("payload");
            scrittore.finish().expect("finish");
        }
        byte
    }

    /// Serve l'header per intero, poi mente: dichiara un byte piu' della
    /// fetta senza averla riempita.
    struct SorgenteBugiarda<'a> {
        byte: &'a [u8],
        letto: usize,
    }

    impl Read for SorgenteBugiarda<'_> {
        fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
            let resto = self.byte.len().saturating_sub(self.letto);
            let quanti = resto.min(out.len());
            out[..quanti].copy_from_slice(&self.byte[self.letto..self.letto + quanti]);
            self.letto += quanti;
            // L'header passa da `read_exact`, che rifiuterebbe subito una
            // dichiarazione in eccesso: la bugia arriva quando il ciclo del
            // payload chiede la sua prima fetta.
            if self.letto <= 16 {
                return Ok(quanti);
            }
            Ok(out.len() + 1)
        }
    }

    #[test]
    fn l_envelope_si_ferma_invece_di_accodare_byte_mai_scritti() {
        let atteso: Vec<u8> = (0..3000_u32).map(|i| (i % 251) as u8).collect();
        let byte = envelope(&atteso);
        let mut sorgente = SorgenteBugiarda {
            byte: &byte,
            letto: 0,
        };
        let lettore = EnvelopeReader::new(&mut sorgente).expect("header valido");
        match lettore.read_payload() {
            Err(ArrowTransportError::Io(errore)) => {
                assert_eq!(
                    errore.to_string(),
                    "il lettore ha reso piu' byte della finestra richiesta"
                );
            }
            Ok(_) => panic!("un `Read` scorretto non deve produrre un payload valido"),
            Err(altro) => panic!("atteso un errore di I/O, ottenuto {altro:?}"),
        }
    }
}
