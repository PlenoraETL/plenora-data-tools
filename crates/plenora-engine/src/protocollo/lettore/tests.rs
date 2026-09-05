//! Prove del lettore limitato.
//!
//! La prova che conta non e' che un frame sovradimensionato venga rifiutato —
//! quello lo fa gia' `decodifica`. E' che venga rifiutato **senza leggere il
//! payload**, e quella si dimostra solo contando i byte che la sorgente ha
//! consegnato. Per questo la sorgente e' una spia.

use std::io::{self, Read};

use super::leggi_frame;
use crate::protocollo::codifica::{codifica, MAX_PROTOCOL_FRAME_BYTES};
use crate::protocollo::messaggi::{Annulla, Corpo, Frame};

/// Una sorgente che **conta** i byte consegnati.
///
/// E' l'unico modo di provare un'affermazione sulla quantita' letta: guardare
/// il codice non e' una prova, e un errore giusto puo' arrivare dopo aver
/// letto tutto.
struct Spia {
    dati: Vec<u8>,
    consegnati: usize,
    /// Byte per chiamata, per costringere il lettore a piu' giri.
    a_morsi: usize,
}

impl Spia {
    fn nuova(dati: Vec<u8>) -> Self {
        Self {
            dati,
            consegnati: 0,
            a_morsi: usize::MAX,
        }
    }

    fn a_morsi(dati: Vec<u8>, quanti: usize) -> Self {
        Self {
            dati,
            consegnati: 0,
            a_morsi: quanti,
        }
    }
}

impl Read for Spia {
    fn read(&mut self, destinazione: &mut [u8]) -> io::Result<usize> {
        let rimasti = self.dati.len() - self.consegnati;
        let quanti = destinazione.len().min(rimasti).min(self.a_morsi);
        destinazione[..quanti]
            .copy_from_slice(&self.dati[self.consegnati..self.consegnati + quanti]);
        self.consegnati += quanti;
        Ok(quanti)
    }
}

/// Una sorgente che fallisce dopo aver consegnato `prima` byte.
struct Rotta {
    dati: Vec<u8>,
    consegnati: usize,
    prima: usize,
    genere: io::ErrorKind,
}

impl Read for Rotta {
    fn read(&mut self, destinazione: &mut [u8]) -> io::Result<usize> {
        if self.consegnati >= self.prima {
            return Err(io::Error::new(self.genere, "guasto simulato del canale"));
        }
        let disponibili = (self.prima - self.consegnati).min(self.dati.len() - self.consegnati);
        let quanti = destinazione.len().min(disponibili);
        destinazione[..quanti]
            .copy_from_slice(&self.dati[self.consegnati..self.consegnati + quanti]);
        self.consegnati += quanti;
        Ok(quanti)
    }
}

fn annulla(motivo: &str) -> Frame {
    Frame::nuovo(Corpo::Annulla(Annulla {
        motivo: motivo.to_owned(),
    }))
}

#[test]
fn un_frame_intero_si_legge() {
    let byte = codifica(&annulla("timeout")).expect("codifica");
    let mut spia = Spia::nuova(byte.clone());
    let letto = leggi_frame(&mut spia)
        .expect("lettura")
        .expect("un frame c'e'");
    assert_eq!(letto, annulla("timeout"));
    assert_eq!(
        spia.consegnati,
        byte.len(),
        "letti byte di troppo o di meno"
    );
}

/// Una sorgente che consegna un byte per volta e' comunque letta per intero.
///
/// `Read` puo' rendere meno byte di quanti gliene se ne chiedano, e un lettore
/// che non lo gestisse funzionerebbe in memoria e fallirebbe su un pipe.
#[test]
fn una_sorgente_a_morsi_si_legge_lo_stesso() {
    let byte = codifica(&annulla("timeout")).expect("codifica");
    for morso in [1_usize, 2, 3, 5] {
        let mut spia = Spia::a_morsi(byte.clone(), morso);
        let letto = leggi_frame(&mut spia)
            .expect("lettura")
            .expect("un frame c'e'");
        assert_eq!(letto, annulla("timeout"), "morso da {morso} byte");
        assert_eq!(spia.consegnati, byte.len());
    }
}

/// Due frame in fila si leggono uno dopo l'altro, e poi la sorgente finisce.
#[test]
fn i_frame_si_leggono_in_sequenza_e_la_fine_e_pulita() {
    let mut byte = codifica(&annulla("uno")).expect("codifica");
    byte.extend_from_slice(&codifica(&annulla("due")).expect("codifica"));
    let mut spia = Spia::nuova(byte);

    assert_eq!(
        leggi_frame(&mut spia).expect("primo").expect("c'e'"),
        annulla("uno")
    );
    assert_eq!(
        leggi_frame(&mut spia).expect("secondo").expect("c'e'"),
        annulla("due")
    );
    // La fine al confine giusto non e' un errore.
    assert!(
        leggi_frame(&mut spia).expect("fine").is_none(),
        "la fine pulita e' stata scambiata per un guasto"
    );
}

/// **La prova che conta.**
///
/// Un prefisso che dichiara `MAX + 1` fa consumare quattro byte e nient'altro.
/// La sorgente ne offre molti di piu': se il lettore allocasse e leggesse
/// prima di decidere, il contatore lo direbbe.
#[test]
fn un_prefisso_oltre_il_tetto_consuma_solo_quattro_byte() {
    let dichiarata = u32::try_from(MAX_PROTOCOL_FRAME_BYTES + 1).expect("sta in u32");
    let mut dati = dichiarata.to_be_bytes().to_vec();
    // Un mucchio di byte dopo il prefisso: sono li' apposta, per essere
    // **non** letti.
    dati.extend(std::iter::repeat_n(b'x', 64 * 1024));
    let offerti = dati.len();

    let mut spia = Spia::nuova(dati);
    let messaggio = leggi_frame(&mut spia)
        .expect_err("oltre il tetto")
        .to_string();

    assert_eq!(
        spia.consegnati, 4,
        "il lettore ha consumato {} byte dei {offerti} offerti: ha letto il payload \
         prima di decidere",
        spia.consegnati
    );
    assert!(
        messaggio.contains("oltre il tetto"),
        "motivo inatteso: {messaggio}"
    );
}

/// Il confine e' esclusivo anche qui: `MAX` supera il controllo di lunghezza.
///
/// Il rifiuto arriva lo stesso, ma per il motivo giusto — il payload manca
/// davvero — e non dal tetto. Senza questo caso, un tetto sbagliato di uno
/// resterebbe invisibile.
#[test]
fn al_tetto_esatto_il_rifiuto_viene_dal_troncamento() {
    let al_tetto = u32::try_from(MAX_PROTOCOL_FRAME_BYTES).expect("sta in u32");
    let mut spia = Spia::nuova(al_tetto.to_be_bytes().to_vec());
    let messaggio = leggi_frame(&mut spia)
        .expect_err("payload assente")
        .to_string();
    assert!(
        messaggio.contains("payload troncato"),
        "MAX rifiutato dal tetto invece che dal troncamento: {messaggio}"
    );
}

#[test]
fn un_prefisso_troncato_e_un_errore_ma_non_una_fine_pulita() {
    for quanti in 1..4_usize {
        let mut spia = Spia::nuova(vec![0_u8; quanti]);
        let messaggio = leggi_frame(&mut spia)
            .expect_err("prefisso troncato")
            .to_string();
        assert!(
            messaggio.contains("prefisso troncato") && messaggio.contains(&quanti.to_string()),
            "con {quanti} byte il motivo e': {messaggio}"
        );
    }
}

/// Zero byte e' la fine, non un troncamento.
#[test]
fn una_sorgente_vuota_e_una_fine_pulita() {
    let mut spia = Spia::nuova(Vec::new());
    assert!(leggi_frame(&mut spia).expect("fine pulita").is_none());
    assert_eq!(spia.consegnati, 0);
}

#[test]
fn un_payload_troncato_e_un_errore() {
    let mut byte = codifica(&annulla("timeout")).expect("codifica");
    let interi = byte.len();
    byte.truncate(interi - 3);
    let mut spia = Spia::nuova(byte);
    let messaggio = leggi_frame(&mut spia)
        .expect_err("payload troncato")
        .to_string();
    assert!(
        messaggio.contains("payload troncato"),
        "motivo inatteso: {messaggio}"
    );
}

/// Un guasto del canale resta un guasto del canale.
///
/// Riclassificarlo come violazione del protocollo direbbe che ha sbagliato
/// l'altro capo, quando invece si e' rotto il filo — e manderebbe chi indaga
/// a guardare il worker invece del sistema.
#[test]
fn un_errore_di_io_si_conserva_e_non_diventa_protocollo() {
    use plenora_core::{ErrorCategory, PlenoraError};

    // Guasto durante il prefisso.
    let mut rotta = Rotta {
        dati: vec![0_u8; 64],
        consegnati: 0,
        prima: 2,
        genere: io::ErrorKind::BrokenPipe,
    };
    let errore = leggi_frame(&mut rotta).expect_err("guasto");
    assert!(
        matches!(errore, PlenoraError::Io(_)),
        "l'errore di I/O sul prefisso e' diventato: {errore:?}"
    );
    assert_eq!(errore.category(), ErrorCategory::Io);

    // Guasto durante il payload.
    let byte = codifica(&annulla("timeout")).expect("codifica");
    let mut rotta = Rotta {
        dati: byte,
        consegnati: 0,
        prima: 6,
        genere: io::ErrorKind::ConnectionReset,
    };
    let errore = leggi_frame(&mut rotta).expect_err("guasto");
    assert!(
        matches!(errore, PlenoraError::Io(_)),
        "l'errore di I/O sul payload e' diventato: {errore:?}"
    );
}

/// `Interrupted` non e' un guasto: e' un segnale arrivato durante la syscall.
///
/// Trattarlo da errore farebbe fallire una lettura legittima ogni volta che il
/// processo riceve un segnale — cioe' proprio quando il supervisore ne manda
/// uno.
#[test]
fn una_interruzione_non_fa_fallire_la_lettura() {
    struct Interrotta {
        dati: Vec<u8>,
        consegnati: usize,
        interruzioni: usize,
    }
    impl Read for Interrotta {
        fn read(&mut self, destinazione: &mut [u8]) -> io::Result<usize> {
            if self.interruzioni > 0 {
                self.interruzioni -= 1;
                return Err(io::Error::new(io::ErrorKind::Interrupted, "segnale"));
            }
            let rimasti = self.dati.len() - self.consegnati;
            let quanti = destinazione.len().min(rimasti).min(3);
            destinazione[..quanti]
                .copy_from_slice(&self.dati[self.consegnati..self.consegnati + quanti]);
            self.consegnati += quanti;
            Ok(quanti)
        }
    }

    let mut sorgente = Interrotta {
        dati: codifica(&annulla("timeout")).expect("codifica"),
        consegnati: 0,
        interruzioni: 3,
    };
    let letto = leggi_frame(&mut sorgente)
        .expect("l'interruzione non e' un guasto")
        .expect("un frame c'e'");
    assert_eq!(letto, annulla("timeout"));
}

/// Il frame riassemblato e' identico a quello scritto, anche a morsi minuscoli.
///
/// Il nome dice cio' che il test **osserva**, e nient'altro. «In un solo
/// buffer» si attribuirebbe la prova del numero di allocazioni: dall'esterno
/// quel numero non e' osservabile, e un nome che promette piu' di quanto
/// misura e' peggio di un test assente — chi legge l'elenco crede che quella
/// proprieta' sia coperta.
///
/// Cio' che qui si prova e' il riassemblaggio: prefisso e payload tornano un
/// frame uguale all'originale anche quando la sorgente li consegna tre byte
/// per volta. E' la proprieta' che l'aritmetica del buffer puo' rompere, ed e'
/// infatti quella che la mutazione «il buffer unico e' dimensionato male»
/// fa cadere.
///
/// Il numero di allocazioni e la loro fallibilita' non si provano da qui: la
/// seconda sta nella firma — `try_reserve_exact` rende un `Result`, e
/// sostituirla con `reserve_exact` non compila.
#[test]
fn il_frame_riassemblato_e_identico_a_quello_scritto() {
    let atteso = annulla("un motivo abbastanza lungo da non essere banale");
    let byte = codifica(&atteso).expect("codifica");
    let mut spia = Spia::a_morsi(byte.clone(), 7);
    let letto = leggi_frame(&mut spia)
        .expect("lettura")
        .expect("un frame c'e'");
    assert_eq!(letto, atteso);
    assert_eq!(spia.consegnati, byte.len());
}

/// Una lunghezza che non sta in `usize` col prefisso davanti e' un errore
/// tipizzato, non un traboccamento.
///
/// Su un `usize` a 64 bit il caso non e' raggiungibile — il tetto lo esclude
/// molto prima — ma il `checked_add` c'e' lo stesso: e' il genere di somma che
/// su una piattaforma a 32 bit diventerebbe raggiungibile, e scoprirlo li'
/// sarebbe scoprirlo tardi.
#[test]
fn la_somma_col_prefisso_e_controllata() {
    // La funzione di produzione, chiamata. Asserire
    // `MAX_PROTOCOL_FRAME_BYTES.checked_add(4).is_some()` rifarebbe il calcolo
    // dentro il test: proverebbe il proprio `checked_add` e resterebbe verde
    // anche cancellando quello del lettore.
    let al_tetto = super::totale_frame(MAX_PROTOCOL_FRAME_BYTES).expect("il tetto sta dentro");
    assert_eq!(
        al_tetto,
        MAX_PROTOCOL_FRAME_BYTES + crate::protocollo::codifica::BYTE_PREFISSO
    );

    // E il ramo che il tetto rende irraggiungibile dal chiamante vero: qui si
    // raggiunge chiamando la funzione, che e' l'unico modo di provare che il
    // traboccamento renda un errore invece di avvolgersi.
    let errore = super::totale_frame(usize::MAX).expect_err("traboccamento");
    assert!(
        errore.to_string().contains("fuori intervallo"),
        "motivo inatteso: {errore}"
    );
}

/// Il lettore non introduce indulgenza: cio' che `decodifica` rifiuta resta
/// rifiutato anche arrivando da un canale.
#[test]
fn il_lettore_non_e_piu_permissivo_del_decoder() {
    let testo = r#"{"protocol_version":1,"tipo":"annulla","corpo":{"motivo":"x","extra":1}}"#;
    let mut dati = u32::try_from(testo.len())
        .expect("corto")
        .to_be_bytes()
        .to_vec();
    dati.extend_from_slice(testo.as_bytes());
    let mut spia = Spia::nuova(dati);
    let messaggio = leggi_frame(&mut spia)
        .expect_err("campo ignoto")
        .to_string();
    assert!(
        messaggio.contains("forma o tipo non conformi"),
        "motivo inatteso: {messaggio}"
    );
}
