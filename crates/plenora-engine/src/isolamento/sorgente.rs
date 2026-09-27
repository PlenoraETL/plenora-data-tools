//! Una lettura che si puo' fermare, senza mentire su cosa e' successo.
//!
//! Un lettore fermo dentro una `read` non si sveglia se qualcuno altrove lascia
//! cadere un mandante: e' il caso del discendente del worker che tiene aperto
//! l'estremo di scrittura, e quella non-garanzia deve diventare un ritardo
//! dichiarato, non un blocco. Il descrittore va in modalita' **non
//! bloccante**, e la lettura passa da un adattatore che aspetta a piccoli
//! passi guardando un interruttore.
//!
//! Proprieta' implementative, vere sempre:
//!
//! - l'interruttore viene guardato **prima di ogni nuova lettura e prima di
//!   ogni attesa**. Non c'e' cammino in cui la lettura riparta senza averlo
//!   guardato, nemmeno dopo un `Interrupted`;
//! - l'adattatore **non chiede mai volontariamente** un'attesa piu' lunga di un
//!   passo. E' cio' che dipende da lui, ed e' tutto cio' che puo' promettere.
//!
//! Non-garanzia: nessun limite di tempo reale. `sleep` promette un minimo,
//! non un massimo, e senza scheduler real-time il ritorno sulla CPU lo decide
//! il kernel; un tetto sull'orologio da parete va preso altrove (timeout di
//! livello piu' alto, `SIGKILL`). La soglia di qualificazione e' un attrezzo
//! dei casi, sotto `cfg(test)`, non un contratto.
//!
//! **Non finge un EOF**: fermato, rende un errore e non `Ok(0)`, che
//! affermerebbe che l'altro capo ha chiuso. **Non ricompone i frame**: lo
//! stato del frame a meta' vive in chi legge i frame.

use std::io::{ErrorKind, Read};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// Ogni quanto l'attesa si riapre per guardare l'interruttore.
///
/// Governa l'attesa che l'adattatore **chiede**, non quanto dura davvero. Il
/// valore tiene l'attesa fitta senza farne un giro a vuoto. E' la
/// granularita' con cui guardiamo un flag, non un tempo di risposta del
/// worker.
pub(super) const PASSO_DI_ATTESA: Duration = Duration::from_millis(10);

/// Quanti `Interrupted` di fila prima di prendere fiato.
///
/// Il freno da' una via d'uscita, non un limite al consumo: una sorgente che
/// rende sempre `Interrupted` occuperebbe un core finche' nessuno frena. Dopo
/// la soglia si chiede un passo di attesa, come per `WouldBlock`; il
/// contatore riparte a ogni altro esito, e i segnali sporadici non pagano
/// nulla.
const INTERRUZIONI_PRIMA_DEL_RESPIRO: u32 = 16;

/// La soglia entro cui l'arresto si vede **nell'ambiente di qualificazione**.
///
/// **Non e' una garanzia**: senza scheduler real-time non esiste un limite di
/// tempo reale, e per questo vive sotto `cfg(test)`, dove nessuno la legge
/// come promessa. E' il numero oltre il quale, su una macchina che non e' in
/// ginocchio, un arresto non visto e' un difetto: fa fallire un caso se il
/// passo si allunga o l'interruttore smette di essere guardato.
#[cfg(test)]
const SOGLIA_DI_QUALIFICAZIONE_DELL_ARRESTO: Duration = Duration::from_millis(200);

/// Chi puo' chiedere l'arresto.
///
/// Il [`Freno`] **chiede** e la [`SorgenteTerminabile`] **ubbidisce**: chi ha
/// il freno non legge, e chi legge non si ferma da solo. Un booleano condiviso
/// non direbbe chi comanda.
#[derive(Debug, Clone)]
pub(super) struct Freno {
    fermati: Arc<AtomicBool>,
}

/// Chi ubbidisce.
///
/// Esiste separato dalla sorgente perche' **non tutti i produttori leggono**:
/// l'orologio e il sorvegliante non hanno una sorgente da cui aspettare byte,
/// ma hanno la stessa identica necessita' di potersi fermare. Legarla a una
/// sorgente finta li obbligherebbe a fingere di leggere.
#[derive(Debug, Clone)]
pub(super) struct Interruttore {
    fermati: Arc<AtomicBool>,
}

impl Interruttore {
    /// Se qualcuno ha chiesto l'arresto.
    pub(super) fn fermato(&self) -> bool {
        // `Acquire` in coppia con il `Release` di chi ferma: chi si accorge
        // dell'arresto vede anche tutto cio' che chi ferma ha scritto prima.
        self.fermati.load(Ordering::Acquire)
    }
}

/// Un interruttore e il suo freno.
pub(super) fn interruttore() -> (Interruttore, Freno) {
    let fermati = Arc::new(AtomicBool::new(false));
    (
        Interruttore {
            fermati: Arc::clone(&fermati),
        },
        Freno { fermati },
    )
}

impl Freno {
    /// Chiede al lettore di fermarsi.
    ///
    /// Non aspetta: **dice**. Quanto ci metta a essere visto non lo promette
    /// nessuno — vedi le tre affermazioni in cima al modulo. Cio' che e'
    /// garantito e' che l'interruttore venga guardato prima di ogni nuova
    /// lettura e prima di ogni attesa, e che l'attesa che si **chiede** non
    /// superi mai un passo.
    pub(super) fn ferma(&self) {
        // `Release` perche' chi si ferma deve vedere anche tutto cio' che chi
        // ferma ha scritto prima: l'ordine fra le due cose e' cio' che rende la
        // richiesta d'arresto un fatto e non una coincidenza.
        self.fermati.store(true, Ordering::Release);
    }
}

/// Una sorgente che si lascia fermare.
///
/// Generica sulla sorgente e non legata a un descrittore: i casi che contano —
/// un byte per volta, un arresto a meta' frame — si compongono su una sorgente
/// finta, e su un descrittore vero non si producono a comando.
#[derive(Debug)]
pub(super) struct SorgenteTerminabile<R: Read> {
    sorgente: R,
    interruttore: Interruttore,
    passo: Duration,
}

impl<R: Read> SorgenteTerminabile<R> {
    /// La sorgente, e il freno per fermarla.
    ///
    /// Crea il freno **insieme** alla sorgente, per chi ascolta per primo
    /// (`macchina::produttori::avvia_lettore`). Il worker, che ha il freno
    /// prima del thread che legge, passa da [`Self::con_interruttore`].
    pub(super) fn nuova(sorgente: R) -> (Self, Freno) {
        Self::con_passo(sorgente, PASSO_DI_ATTESA)
    }

    /// [`Self::nuova`] con il passo in mano al chiamante.
    ///
    /// Esiste per i casi che misurano la latenza: con il passo vero un caso
    /// durerebbe quanto il passo per ogni giro, e un caso lento non lo esegue
    /// nessuno. Cio' che un chiamante di prova puo' variare e' **quanto** si
    /// aspetta fra un giro e l'altro, mai che cosa si decide.
    pub(super) fn con_passo(sorgente: R, passo: Duration) -> (Self, Freno) {
        let (interruttore, freno) = interruttore();
        (
            Self {
                sorgente,
                interruttore,
                passo,
            },
            freno,
        )
    }

    /// La sorgente legata a un interruttore **gia' esistente**.
    ///
    /// Serve quando il freno deve esistere prima della sorgente — per esempio
    /// perche' la sorgente stessa deve poterlo tirare.
    pub(super) const fn con_interruttore(
        sorgente: R,
        passo: Duration,
        interruttore: Interruttore,
    ) -> Self {
        Self {
            sorgente,
            interruttore,
            passo,
        }
    }

    /// Se qualcuno ha chiesto l'arresto.
    ///
    /// Lo chiede anche chi legge **dopo** un errore: la sorgente rende un
    /// errore di I/O ordinario quando la si ferma, e distinguere l'arresto dal
    /// testo del messaggio vorrebbe dire confrontare stringhe.
    pub(super) fn fermato(&self) -> bool {
        self.interruttore.fermato()
    }

    /// La sorgente grezza, per chi deve continuare a leggerla con un
    /// meccanismo diverso.
    ///
    /// Handshake e resto del dialogo hanno due interruttori diversi (tetti
    /// diversi), e un interruttore non si cambia a meta': si costruiscono due
    /// `SorgenteTerminabile` in sequenza sulla stessa sorgente, e qui la prima
    /// la restituisce alla seconda.
    pub(super) fn dentro(self) -> R {
        self.sorgente
    }
}

/// L'errore con cui una lettura fermata si distingue da tutto il resto.
///
/// # Perche' un genere suo
///
/// Perche' chi lo riceve deve sapere che **non e' successo niente al canale**:
/// non e' un filo rotto, non e' un protocollo violato, non e' una fine. E'
/// una nostra decisione, e va accodata come tale.
pub(super) const MOTIVO_DELL_ARRESTO: &str = "lettura fermata su richiesta";

impl<R: Read> Read for SorgenteTerminabile<R> {
    /// Legge, aspettando a piccoli passi finche' non c'e' niente.
    ///
    /// - `WouldBlock`: niente adesso; si guarda l'interruttore e si aspetta un
    ///   passo.
    /// - `Interrupted`: si riprova **senza aspettare**, tornando in cima, dove
    ///   c'e' il controllo dell'interruttore; dopo
    ///   [`INTERRUZIONI_PRIMA_DEL_RESPIRO`] consecutivi si aspetta un passo.
    /// - Ogni altro errore e' del canale, e si rende cosi' com'e'.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::Other`] con [`MOTIVO_DELL_ARRESTO`] quando qualcuno ha
    /// chiesto di fermarsi, oppure l'errore del descrittore.
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let mut interruzioni_di_fila = 0_u32;
        loop {
            // L'interruttore si guarda **prima** di ogni tentativo e prima di
            // ogni attesa: chi chiede l'arresto mentre siamo fermi lo vede
            // rispettato al giro successivo, e chi lo chiede prima che si
            // cominci non aspetta affatto.
            if self.fermato() {
                return Err(std::io::Error::other(MOTIVO_DELL_ARRESTO));
            }
            match self.sorgente.read(buffer) {
                Ok(quanti) => return Ok(quanti),
                Err(errore) if errore.kind() == ErrorKind::Interrupted => {
                    interruzioni_di_fila += 1;
                    if interruzioni_di_fila >= INTERRUZIONI_PRIMA_DEL_RESPIRO {
                        // Non e' una mancanza di dati, ma a questo punto e' un
                        // giro a vuoto: si prende fiato, e si torna comunque in
                        // cima a guardare il freno.
                        interruzioni_di_fila = 0;
                        std::thread::sleep(self.passo);
                    }
                }
                Err(errore) if errore.kind() == ErrorKind::WouldBlock => {
                    interruzioni_di_fila = 0;
                    std::thread::sleep(self.passo);
                }
                Err(errore) => return Err(errore),
            }
        }
    }
}

/// Mette un descrittore in modalita' non bloccante, **conservando il resto**.
///
/// `F_SETFL` scrive l'insieme intero: il solo `O_NONBLOCK` spegnerebbe in
/// silenzio gli altri flag. Si legge, si aggiunge, si riscrive.
///
/// # Errors
///
/// [`PlenoraError::IsolationUnavailable`] se i flag non si leggono o non si
/// riscrivono.
#[cfg(target_os = "linux")]
pub(super) fn rendi_non_bloccante(
    descrittore: impl std::os::fd::AsFd,
) -> plenora_core::error::Result<()> {
    use crate::isolamento::non_disponibile;

    let fd = descrittore.as_fd();
    let attuali = rustix::fs::fcntl_getfl(fd).map_err(|errore| {
        non_disponibile("lettura", &format!("i flag non si leggono: {errore}"))
    })?;
    rustix::fs::fcntl_setfl(fd, attuali | rustix::fs::OFlags::NONBLOCK).map_err(|errore| {
        non_disponibile("lettura", &format!("i flag non si riscrivono: {errore}"))
    })
}

#[cfg(test)]
mod tests {
    use std::io::{ErrorKind, Read};
    use std::time::Duration;

    use super::{SorgenteTerminabile, MOTIVO_DELL_ARRESTO, SOGLIA_DI_QUALIFICAZIONE_DELL_ARRESTO};

    /// Una sorgente che risponde come le si dice, un pezzo per volta.
    struct Copione {
        battute: Vec<std::io::Result<Vec<u8>>>,
        /// Quante volte e' stata interrogata.
        letture: std::cell::Cell<usize>,
    }

    impl Copione {
        fn nuovo(battute: Vec<std::io::Result<Vec<u8>>>) -> Self {
            Self {
                battute,
                letture: std::cell::Cell::new(0),
            }
        }
    }

    impl Read for Copione {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            self.letture.set(self.letture.get() + 1);
            if self.battute.is_empty() {
                return Ok(0);
            }
            match self.battute.remove(0) {
                Ok(pezzo) => {
                    let quanti = pezzo.len().min(buffer.len());
                    buffer[..quanti].copy_from_slice(&pezzo[..quanti]);
                    Ok(quanti)
                }
                Err(errore) => Err(errore),
            }
        }
    }

    fn blocca() -> std::io::Result<Vec<u8>> {
        Err(std::io::Error::from(ErrorKind::WouldBlock))
    }

    /// **Byte singoli alternati a `WouldBlock`**: il contenuto arriva tutto, e
    /// nell'ordine.
    ///
    /// E' il caso che un adattatore scritto male perde: trattare `WouldBlock`
    /// come una fine renderebbe `Ok(0)` a meta' messaggio, e chi legge i frame
    /// vedrebbe un EOF che non c'e'.
    #[test]
    fn byte_singoli_alternati_a_wouldblock_arrivano_tutti() {
        let copione = Copione::nuovo(vec![
            Ok(vec![b'p']),
            blocca(),
            Ok(vec![b'l']),
            blocca(),
            blocca(),
            Ok(vec![b'e']),
            Ok(vec![]),
        ]);
        let (mut sorgente, _freno) =
            SorgenteTerminabile::con_passo(copione, Duration::from_millis(1));
        let mut letto = Vec::new();
        sorgente.read_to_end(&mut letto).expect("si legge tutto");
        assert_eq!(letto, b"ple");
    }

    /// `Interrupted` si riprova **senza aspettare**.
    ///
    /// Un segnale non e' una mancanza di dati: dormire un passo a ogni segnale
    /// aggiungerebbe un ritardo che non serve a niente.
    #[test]
    fn interrupted_si_riprova_senza_aspettare() {
        let copione = Copione::nuovo(vec![
            Err(std::io::Error::from(ErrorKind::Interrupted)),
            Err(std::io::Error::from(ErrorKind::Interrupted)),
            Ok(vec![b'x']),
        ]);
        // Il passo e' lungo di proposito: se `Interrupted` dormisse, questo caso
        // durerebbe due secondi invece di niente.
        let (mut sorgente, _freno) =
            SorgenteTerminabile::con_passo(copione, Duration::from_secs(1));
        let inizio = std::time::Instant::now();
        let mut buffer = [0_u8; 1];
        assert_eq!(sorgente.read(&mut buffer).expect("legge"), 1);
        assert_eq!(buffer[0], b'x');
        assert!(
            inizio.elapsed() < Duration::from_millis(500),
            "un segnale non deve costare un passo di attesa"
        );
    }

    /// Un errore vero del canale si rende **com'e'**.
    #[test]
    fn un_errore_del_canale_non_si_riclassifica() {
        let copione = Copione::nuovo(vec![Err(std::io::Error::from(ErrorKind::BrokenPipe))]);
        let (mut sorgente, _freno) =
            SorgenteTerminabile::con_passo(copione, Duration::from_millis(1));
        let mut buffer = [0_u8; 4];
        let errore = sorgente.read(&mut buffer).expect_err("il filo si e' rotto");
        assert_eq!(errore.kind(), ErrorKind::BrokenPipe);
    }

    /// **L'arresto a meta' frame non finge una fine.**
    ///
    /// Il messaggio e' arrivato a meta' quando si chiede di fermarsi. Cio' che
    /// si rende e' un errore che dice «fermata su richiesta», non `Ok(0)`: un
    /// `Ok(0)` sarebbe un EOF, cioe' uno dei quattro fatti positivi del
    /// successo, e dirlo perche' abbiamo smesso di ascoltare trasformerebbe una
    /// nostra decisione in un'affermazione sul worker.
    #[test]
    fn l_arresto_a_meta_frame_non_finge_una_fine() {
        let copione = Copione::nuovo(vec![Ok(vec![b'm', b'e', b't', b'a']), blocca(), blocca()]);
        let (mut sorgente, freno) =
            SorgenteTerminabile::con_passo(copione, Duration::from_millis(1));

        let mut buffer = [0_u8; 8];
        assert_eq!(
            sorgente.read(&mut buffer).expect("la prima meta' arriva"),
            4
        );
        assert_eq!(&buffer[..4], b"meta");

        freno.ferma();
        let errore = sorgente
            .read(&mut buffer)
            .expect_err("fermata non e' finita");
        assert_ne!(errore.kind(), ErrorKind::UnexpectedEof);
        assert!(
            errore.to_string().contains(MOTIVO_DELL_ARRESTO),
            "il motivo non e' quello: {errore}"
        );
    }

    /// **L'arresto si vede entro la soglia di qualificazione.**
    ///
    /// Il lettore e' fermo su una sorgente muta, e l'arresto chiesto da un
    /// altro thread deve farlo tornare entro la soglia. Appartiene alla
    /// **qualificazione dell'ambiente**: se fallisce, si guarda prima la
    /// macchina. Che il freno venga guardato lo prova, contando i giri, il caso
    /// dell'`Interrupted` perpetuo.
    #[test]
    fn l_arresto_si_vede_entro_la_soglia_di_qualificazione() {
        /// Una sorgente che non da' mai niente e non finisce mai.
        struct Muta;
        impl Read for Muta {
            fn read(&mut self, _buffer: &mut [u8]) -> std::io::Result<usize> {
                Err(std::io::Error::from(ErrorKind::WouldBlock))
            }
        }

        let (mut sorgente, freno) = SorgenteTerminabile::nuova(Muta);
        let filo = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(20));
            freno.ferma();
        });

        let inizio = std::time::Instant::now();
        let mut buffer = [0_u8; 4];
        let errore = sorgente.read(&mut buffer).expect_err("si ferma");
        let trascorso = inizio.elapsed();
        filo.join().expect("chi ferma finisce");

        assert!(errore.to_string().contains(MOTIVO_DELL_ARRESTO), "{errore}");
        assert!(
            trascorso < SOGLIA_DI_QUALIFICAZIONE_DELL_ARRESTO,
            "l'arresto si e' visto dopo {trascorso:?}, oltre la soglia di              {SOGLIA_DI_QUALIFICAZIONE_DELL_ARRESTO:?}"
        );
    }

    /// **`Interrupted` perpetuo: il freno si vede lo stesso.**
    ///
    /// Il caso conta i giri, non il tempo: la sorgente si ferma da sola dopo un
    /// numero fissato di `Interrupted`, e l'arresto chiesto **prima** deve
    /// essere visto al primo giro, senza interrogarla affatto.
    #[test]
    fn interrupted_perpetuo_non_rende_inefficace_il_freno() {
        /// Una sorgente che rende sempre `Interrupted`, e conta quante volte le
        /// si chiede.
        struct SempreInterrotta {
            interrogazioni: std::sync::Arc<std::sync::atomic::AtomicUsize>,
        }
        impl Read for SempreInterrotta {
            fn read(&mut self, _buffer: &mut [u8]) -> std::io::Result<usize> {
                self.interrogazioni
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                Err(std::io::Error::from(ErrorKind::Interrupted))
            }
        }

        let interrogazioni = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let (mut sorgente, freno) = SorgenteTerminabile::con_passo(
            SempreInterrotta {
                interrogazioni: std::sync::Arc::clone(&interrogazioni),
            },
            Duration::from_millis(1),
        );

        // Il freno **prima**: se il controllo mancasse in cima al giro, questa
        // lettura non tornerebbe mai.
        freno.ferma();
        let mut buffer = [0_u8; 4];
        let errore = sorgente.read(&mut buffer).expect_err("il freno si vede");
        assert!(errore.to_string().contains(MOTIVO_DELL_ARRESTO), "{errore}");
        assert_eq!(
            interrogazioni.load(std::sync::atomic::Ordering::Relaxed),
            0,
            "con il freno gia' tirato la sorgente non si interroga nemmeno una volta"
        );
    }

    /// **Il presidio contro il giro a vuoto**: dopo un numero limitato di
    /// interruzioni consecutive, l'adattatore prende fiato.
    ///
    /// Il conteggio delle interrogazioni e' un'uguaglianza esatta. Il tempo si
    /// misura solo verso il basso («almeno due passi»), l'unica direzione in
    /// cui `sleep` promette qualcosa: senza respiri il caso finirebbe subito.
    #[test]
    fn dopo_troppe_interruzioni_di_fila_si_prende_fiato() {
        /// Rende sempre `Interrupted`, e frena da sola dopo un numero fissato.
        struct InterrottaFinoA {
            quante: std::sync::Arc<std::sync::atomic::AtomicU32>,
            limite: u32,
            freno: Option<super::Freno>,
        }
        impl Read for InterrottaFinoA {
            fn read(&mut self, _buffer: &mut [u8]) -> std::io::Result<usize> {
                let quante = self
                    .quante
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                    + 1;
                if quante >= self.limite {
                    if let Some(freno) = self.freno.take() {
                        freno.ferma();
                    }
                }
                Err(std::io::Error::from(ErrorKind::Interrupted))
            }
        }

        const QUANTE: u32 = 40;
        let passo = Duration::from_millis(20);
        let quante = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));

        // La sorgente ha bisogno del freno, quindi il freno nasce prima di lei:
        // e' il motivo per cui l'interruttore e' un tipo a se'.
        let (interruttore, freno) = super::interruttore();
        let mut sorgente = SorgenteTerminabile::con_interruttore(
            InterrottaFinoA {
                quante: std::sync::Arc::clone(&quante),
                limite: QUANTE,
                freno: Some(freno),
            },
            passo,
            interruttore,
        );

        let inizio = std::time::Instant::now();
        let mut buffer = [0_u8; 4];
        let errore = sorgente
            .read(&mut buffer)
            .expect_err("il freno chiude il giro");
        let trascorso = inizio.elapsed();

        assert!(errore.to_string().contains(MOTIVO_DELL_ARRESTO), "{errore}");
        assert_eq!(
            quante.load(std::sync::atomic::Ordering::Relaxed),
            QUANTE,
            "la sorgente si interroga esattamente fino a quando frena"
        );
        assert!(
            trascorso >= passo * 2,
            "quaranta interruzioni con soglia sedici vogliono due respiri, e ne sono \
             passati solo {trascorso:?}"
        );
    }

    /// E il freno tirato **durante** una sequenza di `Interrupted` la chiude.
    ///
    /// Qui la sorgente viene interrogata davvero, e il freno arriva da un altro
    /// thread mentre il giro sta correndo. Senza il controllo in cima, il giro
    /// non finirebbe.
    #[test]
    fn il_freno_chiude_una_sequenza_di_interrupted() {
        struct SempreInterrotta;
        impl Read for SempreInterrotta {
            fn read(&mut self, _buffer: &mut [u8]) -> std::io::Result<usize> {
                Err(std::io::Error::from(ErrorKind::Interrupted))
            }
        }

        let (mut sorgente, freno) =
            SorgenteTerminabile::con_passo(SempreInterrotta, Duration::from_millis(1));
        let filo = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(20));
            freno.ferma();
        });
        let mut buffer = [0_u8; 4];
        let errore = sorgente
            .read(&mut buffer)
            .expect_err("il freno chiude il giro");
        filo.join().expect("chi ferma finisce");
        assert!(errore.to_string().contains(MOTIVO_DELL_ARRESTO), "{errore}");
    }

    /// Un arresto chiesto **prima** di cominciare non fa aspettare niente.
    #[test]
    fn un_arresto_gia_chiesto_non_fa_aspettare() {
        let (mut sorgente, freno) = SorgenteTerminabile::con_passo(
            Copione::nuovo(vec![Ok(vec![b'x'])]),
            Duration::from_secs(10),
        );
        freno.ferma();
        let mut buffer = [0_u8; 1];
        assert!(sorgente.read(&mut buffer).is_err());
    }

    /// `F_SETFL` **conserva gli altri flag**.
    ///
    /// Scrivere il solo `O_NONBLOCK` spegnerebbe tutto il resto, e lo farebbe in
    /// silenzio perche' la chiamata riesce. Qui si parte da un descrittore con
    /// un flag in piu' e si verifica che dopo ci siano entrambi.
    #[test]
    #[cfg(target_os = "linux")]
    fn rendere_non_bloccante_conserva_gli_altri_flag() {
        use std::os::fd::AsFd as _;

        let (legge, _scrive) = std::io::pipe().expect("la pipe si crea");
        // Un flag di stato che non c'entra col bloccare, acceso prima.
        let prima = rustix::fs::fcntl_getfl(legge.as_fd()).expect("i flag si leggono");
        rustix::fs::fcntl_setfl(legge.as_fd(), prima | rustix::fs::OFlags::APPEND)
            .expect("si aggiunge APPEND");

        super::rendi_non_bloccante(legge.as_fd()).expect("si mette non bloccante");

        let dopo = rustix::fs::fcntl_getfl(legge.as_fd()).expect("i flag si rileggono");
        assert!(
            dopo.contains(rustix::fs::OFlags::NONBLOCK),
            "manca NONBLOCK: {dopo:?}"
        );
        assert!(
            dopo.contains(rustix::fs::OFlags::APPEND),
            "APPEND e' stato spento: {dopo:?}"
        );
    }

    /// E su una pipe vera la lettura non blocca piu'.
    #[test]
    #[cfg(target_os = "linux")]
    fn una_pipe_non_bloccante_rende_wouldblock() {
        use std::os::fd::AsFd as _;
        let (mut legge, _scrive) = std::io::pipe().expect("la pipe si crea");
        super::rendi_non_bloccante(legge.as_fd()).expect("si mette non bloccante");
        let mut buffer = [0_u8; 4];
        let errore = legge
            .read(&mut buffer)
            .expect_err("non c'e' niente da leggere");
        assert_eq!(errore.kind(), ErrorKind::WouldBlock);
    }
}
