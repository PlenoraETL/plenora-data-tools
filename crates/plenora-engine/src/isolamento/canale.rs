//! Gli estremi delle pipe: che cosa sono, e come si accerta che lo siano.
//!
//! Nessun controllo da solo basta: `is_fifo()` accetta anche una FIFO
//! nominata (che chiunque abbia scrittura sulla directory puo' aprire), il
//! verso non si vede dal tipo, e `0`, `1`, `2` sono i flussi standard. Quindi
//! quattro prove:
//!
//! | prova | che cosa dimostra |
//! |---|---|
//! | il numero e' `>= 3` e i due sono distinti | non e' uno dei canali standard, e non e' lo stesso estremo due volte |
//! | `metadata` dice `S_IFIFO` | e' una FIFO |
//! | `readlink` ha la forma **esatta** `pipe:[cifre]` | e' **anonima**: il bersaglio di una FIFO del filesystem non ha quella forma |
//! | `fdinfo` dice `O_RDONLY` o `O_WRONLY` | il verso e' quello atteso, e non e' `O_RDWR` |
//!
//! Dopo la riapertura da `/proc/self/fd/N` si confrontano l'impronta
//! `(st_dev, st_ino)` e il **verso**: la riapertura riapre la pipe, non
//! l'estremo, e nel verso opposto rende un'impronta identica (misurato).
//!
//! Non si dimostra chi c'e' dall'altro capo: una pipe anonima non porta
//! l'identita' di chi l'ha creata. Quello lo discrimina l'handshake del
//! protocollo, non il descrittore.

use std::os::fd::AsRawFd as _;
use std::os::unix::fs::{FileTypeExt as _, MetadataExt as _};

use plenora_core::error::Result;

use super::non_disponibile;

/// Il verso di un estremo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Verso {
    /// Da qui si legge.
    Lettura,
    /// Qui si scrive.
    Scrittura,
}

impl Verso {
    /// Il nome, per gli errori.
    const fn nome(self) -> &'static str {
        match self {
            Self::Lettura => "lettura",
            Self::Scrittura => "scrittura",
        }
    }
}

/// L'identita' di una pipe: il filesystem **e** l'inode.
///
/// Non il solo inode. Gli inode sono unici dentro un filesystem, non fra
/// filesystem diversi, e confrontarne uno soltanto significherebbe accettare
/// come «la stessa pipe» due oggetti che condividono un numero per caso.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Impronta {
    dispositivo: u64,
    inode: u64,
}

/// Un estremo accertato.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Estremo {
    pub(super) numero: i32,
    pub(super) verso: Verso,
    pub(super) impronta: Impronta,
}

/// Il numero piu' basso che un estremo del canale puo' avere.
///
/// `0`, `1` e `2` sono stdin, stdout e stderr: esistono sempre, e per contratto
/// non trasportano il protocollo. Accettarli vorrebbe dire che una libreria
/// loquace corrompe un messaggio.
const PRIMO_AMMESSO: i32 = 3;

/// Se un numero puo' essere un estremo del canale.
///
/// # Errors
///
/// Il motivo, in forma di frase.
pub(super) fn numero_ammissibile(numero: i32) -> std::result::Result<(), String> {
    if numero < 0 {
        return Err(format!("{numero} non e' un descrittore"));
    }
    if numero < PRIMO_AMMESSO {
        return Err(format!(
            "{numero} e' uno dei canali standard, che per contratto non trasporta il protocollo"
        ));
    }
    Ok(())
}

/// Se il bersaglio di `readlink` ha la forma di una pipe **anonima**.
///
/// La forma e' esatta, non un prefisso (che lascerebbe passare `pipe:[12x]`,
/// `pipe:[]`, `pipe:[1]/qualcosa`): `pipe:`, `[`, almeno una cifra decimale,
/// `]`, e nient'altro. Basta che il bersaglio di una FIFO del filesystem non
/// coincida con questa forma; non si promette altro su che aspetto abbia.
pub(super) fn forma_di_pipe_anonima(bersaglio: &str) -> bool {
    let Some(dentro) = bersaglio
        .strip_prefix("pipe:[")
        .and_then(|resto| resto.strip_suffix(']'))
    else {
        return false;
    };
    !dentro.is_empty() && dentro.bytes().all(|byte| byte.is_ascii_digit())
}

/// Il verso, dai flag di `fdinfo`.
///
/// I flag sono in **ottale**, e i due bit bassi sono la modalita' d'accesso:
/// `0` sola lettura, `1` sola scrittura, `2` lettura e scrittura. `O_RDWR` si
/// rifiuta: un estremo che fa entrambe le cose e' il canale intero in mano a
/// un lato solo.
///
/// # Errors
///
/// Il motivo, in forma di frase.
pub(super) fn verso_dai_flag(flag: &str, atteso: Verso) -> std::result::Result<(), String> {
    let numerici = u32::from_str_radix(flag.trim(), 8)
        .map_err(|_| format!("i flag «{flag}» non sono un numero ottale"))?;
    let trovato = match numerici & 0o3 {
        0 => Verso::Lettura,
        1 => Verso::Scrittura,
        2 => return Err(
            "aperto in lettura e scrittura: un estremo che fa entrambe le cose non e' un estremo"
                .to_owned(),
        ),
        _ => {
            return Err(format!(
                "modalita' d'accesso non riconosciuta nei flag «{flag}»"
            ))
        }
    };
    if trovato == atteso {
        Ok(())
    } else {
        Err(format!(
            "aperto in {}, atteso in {}",
            trovato.nome(),
            atteso.nome()
        ))
    }
}

/// La riga `flags:` di un `fdinfo`, se ce n'e' **esattamente una**.
///
/// Assenza e duplicazione sono due errori distinti: senza la riga non c'e'
/// verso da leggere (il descrittore o `/proc` non sono cio' che crediamo), con
/// due righe sceglierne una sarebbe una convenzione non dichiarata.
///
/// # Errors
///
/// Il motivo, in forma di frase.
fn flag_di(numero: i32) -> std::result::Result<String, String> {
    let percorso = format!("/proc/self/fdinfo/{numero}");
    // La lettura e' limitata: `leggi_limitato` si ferma a 1 MiB e rifiuta cio'
    // che va oltre, invece di crescere quanto il file dice di essere.
    let contenuto = super::lettura::leggi_limitato(std::path::Path::new(&percorso))
        .map_err(|difetto| format!("{percorso}: {difetto}"))?;
    let mut trovata: Option<&str> = None;
    for riga in contenuto.lines() {
        let Some(valore) = riga.strip_prefix("flags:") else {
            continue;
        };
        if trovata.is_some() {
            return Err(format!(
                "{percorso} porta piu' di una riga «flags:»: quale valga non lo dichiara nessuno"
            ));
        }
        trovata = Some(valore);
    }
    trovata
        .map(str::to_owned)
        .ok_or_else(|| format!("{percorso} non porta nessuna riga «flags:»"))
}

/// Guarda un descrittore ereditato e dice che cos'e', o perche' non va.
///
/// # Errors
///
/// [`PlenoraError::IsolationUnavailable`], col numero e la ragione.
pub(super) fn accerta(numero: i32, atteso: Verso) -> Result<Estremo> {
    let dove = |motivo: &str| non_disponibile(&format!("canale, fd {numero}"), motivo);

    numero_ammissibile(numero).map_err(|motivo| dove(&motivo))?;

    let percorso = format!("/proc/self/fd/{numero}");
    let dati =
        std::fs::metadata(&percorso).map_err(|errore| dove(&format!("{percorso}: {errore}")))?;
    if !dati.file_type().is_fifo() {
        return Err(dove("non e' una FIFO"));
    }

    let bersaglio =
        std::fs::read_link(&percorso).map_err(|errore| dove(&format!("{percorso}: {errore}")))?;
    let testo = bersaglio.to_string_lossy();
    if !forma_di_pipe_anonima(&testo) {
        return Err(dove(
            "non e' una pipe anonima: una FIFO del filesystem non e' il canale del supervisore",
        ));
    }

    let flag = flag_di(numero).map_err(|motivo| dove(&motivo))?;
    verso_dai_flag(&flag, atteso).map_err(|motivo| dove(&motivo))?;

    Ok(Estremo {
        numero,
        verso: atteso,
        impronta: Impronta {
            dispositivo: dati.dev(),
            inode: dati.ino(),
        },
    })
}

/// Accerta i due estremi insieme, perche' una delle condizioni li riguarda
/// entrambi.
///
/// # Errors
///
/// [`PlenoraError::IsolationUnavailable`].
pub(super) fn accerta_coppia(legge: i32, scrive: i32) -> Result<super::NumeriDelCanale> {
    if legge == scrive {
        return Err(non_disponibile(
            "canale",
            &format!("i due estremi hanno lo stesso descrittore ({legge}): non sono due estremi"),
        ));
    }
    let ingresso = accerta(legge, Verso::Lettura)?;
    let uscita = accerta(scrive, Verso::Scrittura)?;
    if ingresso.impronta == uscita.impronta {
        return Err(non_disponibile(
            "canale",
            "i due estremi puntano alla stessa pipe: un canale che parla con se stesso non e' un canale",
        ));
    }
    // I numeri escono **da qui** e da nessun altro posto: e' cio' che rende
    // «verificati» una proprieta' del tipo invece di una frase nel commento di
    // chi lo costruisce. Sono quelli osservati, non quelli ricevuti — e su una
    // macchina sana coincidono, ma coincidere non e' la stessa cosa che esserlo.
    Ok(super::NumeriDelCanale {
        legge: ingresso.numero,
        scrive: uscita.numero,
    })
}

/// Dove il worker legge i numeri dei suoi due estremi.
///
/// Sta nell'ambiente e non nella riga di comando, che appartiene a chi ha
/// chiesto l'esecuzione. Non e' una prova: [`riapri_accertato`] riguarda i
/// numeri, e un valore sbagliato porta a un rifiuto, non a un altro canale.
///
/// La scrive lo **spawner** dalla coppia rivalidata, e la **impone**: un
/// valore ereditato indicherebbe descrittori veri di un altro canale. La
/// legge il **worker**.
pub(super) const VARIABILE_DEL_CANALE: &str = "PLENORA_CANALE";

/// Lo stadio del worker: riguarda il descrittore ereditato, lo **riapre**, e
/// confronta.
///
/// Riapre perche' adottare un numero (`OwnedFd::from_raw_fd`,
/// `BorrowedFd::borrow_raw`) richiede `unsafe`: `/proc/self/fd/<n>` rende un
/// `File` posseduto, nato `CLOEXEC`. Dopo la riapertura servono due
/// confronti: l'impronta `(dispositivo, inode)` dice **stessa pipe** (il
/// numero puo' essere stato riusato), il verso dice **dal lato giusto**
/// (riaprire nel verso opposto rende un'impronta identica, misurato).
///
/// L'ereditato resta aperto e non e' `CLOEXEC`: chiuderlo richiederebbe di
/// adottarlo. E' un'**invariante operativa, non una garanzia**: finche' il
/// worker non avvia altri processi, quel descrittore non va da nessuna parte.
/// Sta fra le non-garanzie.
///
/// # Errors
///
/// [`PlenoraError::IsolationUnavailable`] se il descrittore non supera i
/// controlli, se la riapertura non riesce, o se uno dei due confronti non torna.
pub(super) fn riapri_accertato(numero: i32, atteso: Verso) -> Result<std::fs::File> {
    riapri_accertato_con(numero, atteso, osservazione_vera)
}

/// Cio' che si osserva del descrittore **dopo** averlo riaperto.
///
/// Separa l'osservazione dal giudizio: il giudizio diventa una funzione pura,
/// e le divergenze, che su una macchina sana non si producono, si scrivono.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Adozione {
    impronta: Impronta,
    /// La riga `flags:` del `fdinfo` del **nuovo** descrittore, non del
    /// vecchio: e' l'unica che dice da che lato guarda quello che si e' appena
    /// aperto.
    flag: String,
}

/// L'osservazione vera, quella che la produzione passa.
///
/// # Errors
///
/// [`PlenoraError::IsolationUnavailable`] se il descrittore riaperto non si
/// interroga.
fn osservazione_vera(riaperto: &std::fs::File) -> Result<Adozione> {
    use std::os::fd::AsRawFd as _;
    let numero = riaperto.as_raw_fd();
    let dove = |motivo: &str| non_disponibile(&format!("canale, fd riaperto {numero}"), motivo);

    let dati = riaperto
        .metadata()
        .map_err(|errore| dove(&format!("non si interroga: {errore}")))?;
    let flag = flag_di(numero).map_err(|motivo| dove(&motivo))?;
    Ok(Adozione {
        impronta: Impronta {
            dispositivo: dati.dev(),
            inode: dati.ino(),
        },
        flag,
    })
}

/// Che il descrittore riaperto sia **lo stesso oggetto, dallo stesso lato**.
///
/// Tre condizioni, ciascuna col suo messaggio: l'**inode** dice quale
/// oggetto, il **dispositivo** su quale filesystem (gli inode sono unici solo
/// dentro un filesystem), il **verso** da che lato, che l'impronta non dice.
/// Sta a se' perche' le divergenze non si producono su una macchina sana: cosi'
/// si scrivono, e il controllo si misura.
///
/// # Errors
///
/// [`PlenoraError::IsolationUnavailable`], che nomina quale delle tre non torna.
fn accerta_adozione(numero: i32, prima: &Impronta, dopo: &Adozione, atteso: Verso) -> Result<()> {
    let dove = |motivo: &str| non_disponibile(&format!("canale, fd {numero}"), motivo);

    if dopo.impronta.inode != prima.inode {
        return Err(dove(&format!(
            "il descrittore riaperto ha inode {} invece di {}: fra il controllo e l'apertura non e' piu' lo stesso oggetto",
            dopo.impronta.inode, prima.inode
        )));
    }
    if dopo.impronta.dispositivo != prima.dispositivo {
        return Err(dove(&format!(
            "il descrittore riaperto sta sul dispositivo {} invece di {}: stesso numero di inode, un altro filesystem",
            dopo.impronta.dispositivo, prima.dispositivo
        )));
    }
    verso_dai_flag(&dopo.flag, atteso)
        .map_err(|motivo| dove(&format!("il descrittore riaperto: {motivo}")))?;
    Ok(())
}

/// [`riapri_accertato`] con l'osservazione in mano al chiamante.
///
/// Esiste perche' il controllo dopo la riapertura si provi **al suo posto**:
/// togliere la chiamata da qui deve far fallire un caso. E' privata, e il suo
/// solo chiamante di produzione le passa [`osservazione_vera`]; un
/// osservatore puo' variare che cosa si dichiara di aver visto, mai che cosa
/// si controlla, perche' il giudizio resta [`accerta_adozione`].
///
/// # Errors
///
/// [`PlenoraError::IsolationUnavailable`] se il descrittore non supera i
/// controlli, se la riapertura non riesce, o se l'adozione non torna.
fn riapri_accertato_con(
    numero: i32,
    atteso: Verso,
    osserva: impl FnOnce(&std::fs::File) -> Result<Adozione>,
) -> Result<std::fs::File> {
    let dove = |motivo: &str| non_disponibile(&format!("canale, fd {numero}"), motivo);

    // 1. Cio' che c'e', prima di toccarlo.
    let prima = accerta(numero, atteso)?;

    // 2. La riapertura, nel verso chiesto.
    let percorso = format!("/proc/self/fd/{numero}");
    let mut modo = std::fs::OpenOptions::new();
    match atteso {
        Verso::Lettura => modo.read(true),
        Verso::Scrittura => modo.write(true),
    };
    let riaperto = modo
        .open(&percorso)
        .map_err(|errore| dove(&format!("{percorso} non si riapre: {errore}")))?;

    // 3. Che cosa si e' aperto davvero, e se e' cio' che si e' guardato.
    let dopo = osserva(&riaperto)?;
    accerta_adozione(numero, &prima.impronta, &dopo, atteso)?;

    Ok(riaperto)
}

/// I due estremi destinati al worker, mentre stanno ancora nel supervisore.
///
/// Li possiede perche' fra la rimozione di `CLOEXEC` e lo `spawn` ogni
/// ritorno anticipato potrebbe lasciare nel supervisore un descrittore
/// **ereditabile**: il prossimo `spawn` se lo porterebbe dietro, e l'EOF del
/// canale non arriverebbe mai. `Drop` li chiude su **ogni** uscita.
///
/// Non c'e' modo di estrarli: si hanno solo i **numeri**, che non tengono
/// niente aperto.
pub(super) struct EstremiDelWorker {
    legge: std::io::PipeReader,
    scrive: std::io::PipeWriter,
}

impl EstremiDelWorker {
    /// I due numeri, per la richiesta — **riguardandoli**.
    ///
    /// Passa da [`accerta_coppia`], l'unica che li rende: altrimenti questo
    /// sarebbe un secondo posto in cui due interi diventano «i numeri del
    /// canale» senza che nessuno li abbia guardati.
    ///
    /// # Errors
    ///
    /// [`PlenoraError::IsolationUnavailable`] se i due estremi non sono quelli
    /// dichiarati, anche se `apri` li ha appena creati.
    pub(super) fn numeri(&self) -> Result<super::NumeriDelCanale> {
        accerta_coppia(self.legge.as_raw_fd(), self.scrive.as_raw_fd())
    }
}

/// Il canale: quattro estremi, due per lato.
///
/// Nasce **prima** della richiesta, che ne porta i numeri: una richiesta
/// senza canale non deve essere rappresentabile.
///
/// # Errors
///
/// [`PlenoraError::IsolationUnavailable`] se le pipe non si creano, o se uno
/// dei quattro estremi non e' quello che dev'essere.
pub(super) fn apri() -> Result<(std::io::PipeReader, std::io::PipeWriter, EstremiDelWorker)> {
    fn apertura(quale: &str, errore: &std::io::Error) -> plenora_core::error::PlenoraError {
        non_disponibile("canale", &format!("{quale}: {errore}"))
    }
    // W -> S: il supervisore legge, il worker scrive.
    let (sup_legge, worker_scrive) =
        std::io::pipe().map_err(|errore| apertura("pipe worker -> supervisore", &errore))?;
    // S -> W: il supervisore scrive, il worker legge.
    let (worker_legge, sup_scrive) =
        std::io::pipe().map_err(|errore| apertura("pipe supervisore -> worker", &errore))?;

    // Si guardano **tutti e quattro**, mentre sono ancora `CLOEXEC`. Verificare
    // solo i due che partono lascerebbe fuori proprio quelli che restano, e un
    // supervisore che legge da un estremo sbagliato non se ne accorgerebbe mai.
    let sl = accerta(sup_legge.as_raw_fd(), Verso::Lettura)?;
    let ss = accerta(sup_scrive.as_raw_fd(), Verso::Scrittura)?;
    let estremi = EstremiDelWorker {
        legge: worker_legge,
        scrive: worker_scrive,
    };
    let numeri = estremi.numeri()?;
    let wl = accerta(numeri.legge, Verso::Lettura)?;
    let ws = accerta(numeri.scrive, Verso::Scrittura)?;

    // E poi la **topologia**, che nessuna verifica individuale coglie: quattro
    // estremi tutti validi possono essere accoppiati male, e un canale accoppiato
    // male non da' nessun errore — da' silenzio, perche' ognuno parla con
    // qualcuno che non ascolta.
    accerta_topologia(&sl, &ws, &ss, &wl)?;
    Ok((sup_legge, sup_scrive, estremi))
}

/// Che i quattro estremi formino **due** pipe, accoppiate come dicono i nomi.
///
/// Quattro estremi validi uno per uno possono essere accoppiati male (per
/// esempio `sup_legge` sulla pipe di `sup_scrive`), e un canale accoppiato
/// male non da' errore: da' **silenzio**, finche' un timeout dice la cosa
/// sbagliata.
///
/// # Errors
///
/// [`PlenoraError::IsolationUnavailable`], che nomina quale accoppiamento
/// manca.
fn accerta_topologia(
    sup_legge: &Estremo,
    worker_scrive: &Estremo,
    sup_scrive: &Estremo,
    worker_legge: &Estremo,
) -> Result<()> {
    if sup_legge.impronta != worker_scrive.impronta {
        return Err(non_disponibile(
            "canale",
            "l'estremo da cui il supervisore legge non appartiene alla pipe su cui il worker scrive",
        ));
    }
    if sup_scrive.impronta != worker_legge.impronta {
        return Err(non_disponibile(
            "canale",
            "l'estremo su cui il supervisore scrive non appartiene alla pipe da cui il worker legge",
        ));
    }
    if sup_legge.impronta == sup_scrive.impronta {
        return Err(non_disponibile(
            "canale",
            "le due direzioni sono la stessa pipe: un canale con un verso solo non ha due lati",
        ));
    }
    Ok(())
}

impl EstremiDelWorker {
    /// Toglie `CLOEXEC` ai due estremi, e non a nient'altro.
    ///
    /// Da qui allo `spawn` i due descrittori sono **ereditabili**, quindi la
    /// finestra contiene **solo** lo `spawn`: richiesta, argomenti e `Command`
    /// sono gia' costruiti, e in mezzo non c'e' niente che possa fallire a
    /// lungo o creare processi.
    ///
    /// # Errors
    ///
    /// [`PlenoraError::IsolationUnavailable`] se `fcntl` non riesce. Il
    /// chiamante lascia allora cadere la guardia, che chiude **entrambi** gli
    /// estremi, anche il primo gia' reso ereditabile.
    pub(super) fn rendi_ereditabili(&self) -> Result<()> {
        use std::os::fd::AsFd as _;
        let togli = |quale: &str, fd: std::os::fd::BorrowedFd<'_>| {
            rustix::io::fcntl_setfd(fd, rustix::io::FdFlags::empty()).map_err(|errore| {
                non_disponibile(
                    "canale",
                    &format!("{quale}: CLOEXEC non si toglie: {errore}"),
                )
            })
        };
        #[cfg(qualificazione_isolamento)]
        guasto_richiesto("primo-fcntl")?;
        togli("estremo di lettura del worker", self.legge.as_fd())?;
        // Qui il primo estremo e' **gia' ereditabile**: e' l'unico momento in
        // cui la guardia tiene uno stato misto, ed e' quello che il braccio
        // «secondo-fcntl» misura.
        #[cfg(qualificazione_isolamento)]
        guasto_richiesto("secondo-fcntl")?;
        togli("estremo di scrittura del worker", self.scrive.as_fd())?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Il terzo descrittore: l'artefatto del verificatore
// ---------------------------------------------------------------------------
//
// Un file regolare, aperto in sola lettura dal coordinatore prima che il
// verificatore nasca
// (`isolamento.md#2-ter-la-verifica-non-può-stare-fuori-dal-limite`,
// `#2-quater-topologia-chi-osserva-chi`). Lo schema e' quello delle pipe, in
// due tempi, meno la topologia (un file non ha un altro lato); il verso e'
// sempre `Lettura`, perche' il coordinatore non cede mai un handle di
// scrittura sull'artefatto.

/// Guarda il descrittore ereditato dell'artefatto e dice che cos'e', o
/// perche' non va.
///
/// Le prove di [`accerta`], con un **file regolare** al posto della FIFO.
///
/// # Errors
///
/// [`PlenoraError::IsolationUnavailable`], col numero e la ragione.
pub(super) fn accerta_artefatto(numero: i32) -> Result<Estremo> {
    let dove = |motivo: &str| non_disponibile(&format!("artefatto, fd {numero}"), motivo);

    numero_ammissibile(numero).map_err(|motivo| dove(&motivo))?;

    let percorso = format!("/proc/self/fd/{numero}");
    let dati =
        std::fs::metadata(&percorso).map_err(|errore| dove(&format!("{percorso}: {errore}")))?;
    if !dati.is_file() {
        return Err(dove(
            "non e' un file regolare: l'artefatto del verificatore non e' una pipe",
        ));
    }

    let flag = flag_di(numero).map_err(|motivo| dove(&motivo))?;
    verso_dai_flag(&flag, Verso::Lettura).map_err(|motivo| dove(&motivo))?;

    Ok(Estremo {
        numero,
        verso: Verso::Lettura,
        impronta: Impronta {
            dispositivo: dati.dev(),
            inode: dati.ino(),
        },
    })
}

/// Riapre il terzo descrittore e **accerta** che sia lo stesso file, dal
/// verso giusto.
///
/// Stesse ragioni di [`riapri_accertato`]. La riapertura crea una *open file
/// description* indipendente, con un offset non condiviso: chi consuma
/// l'handle (`verifica::verifica_artefatto_handle`,
/// `pubblicazione::copia_accertando`) legge sempre per **posizione**
/// (`SeekSource::read_at` / `ArtefattoConvalidato::leggi_a`).
///
/// # Errors
///
/// [`PlenoraError::IsolationUnavailable`] se il descrittore non supera i
/// controlli, se la riapertura non riesce, o se l'adozione non torna.
pub(super) fn riapri_accertato_artefatto(numero: i32) -> Result<std::fs::File> {
    riapri_accertato_artefatto_con(numero, osservazione_vera)
}

/// [`riapri_accertato_artefatto`] con l'osservazione in mano al chiamante.
///
/// Stessa ragione di [`riapri_accertato_con`]: il controllo dopo la
/// riapertura va provato al suo posto, con un solo chiamante di produzione.
fn riapri_accertato_artefatto_con(
    numero: i32,
    osserva: impl FnOnce(&std::fs::File) -> Result<Adozione>,
) -> Result<std::fs::File> {
    let dove = |motivo: &str| non_disponibile(&format!("artefatto, fd riaperto {numero}"), motivo);

    // 1. Cio' che c'e', prima di toccarlo.
    let prima = accerta_artefatto(numero)?;

    // 2. La riapertura, sempre in lettura: e' l'unico verso che un artefatto
    //    del verificatore puo' avere.
    let percorso = format!("/proc/self/fd/{numero}");
    let riaperto = std::fs::OpenOptions::new()
        .read(true)
        .open(&percorso)
        .map_err(|errore| dove(&format!("{percorso} non si riapre: {errore}")))?;

    // 3. Che cosa si e' aperto davvero, e se e' cio' che si e' guardato.
    let dopo = osserva(&riaperto)?;
    accerta_adozione(numero, &prima.impronta, &dopo, Verso::Lettura)?;

    Ok(riaperto)
}

/// Dove il verificatore legge il numero del terzo descrittore, l'artefatto.
///
/// Stessa disciplina di [`VARIABILE_DEL_CANALE`]: la scrive lo spawner, dalla
/// richiesta gia' rivalidata, e la **impone** — mai un valore ereditato — e
/// il verificatore la riguarda con [`riapri_accertato_artefatto`] invece di
/// crederci. Assente per ogni worker ordinario: quella modalita' non ha un
/// terzo descrittore da nominare.
pub(super) const VARIABILE_ARTEFATTO: &str = "PLENORA_ARTEFATTO_LETTURA";

/// Toglie `CLOEXEC` al descrittore dell'artefatto, e a nient'altro.
///
/// Stessa finestra di [`EstremiDelWorker::rendi_ereditabili`]: la chiamata
/// sta immediatamente prima dello `spawn`.
///
/// # Errors
///
/// [`PlenoraError::IsolationUnavailable`] se `fcntl` non riesce.
pub(super) fn rendi_ereditabile_artefatto(file: &std::fs::File) -> Result<()> {
    use std::os::fd::AsFd as _;
    rustix::io::fcntl_setfd(file.as_fd(), rustix::io::FdFlags::empty()).map_err(|errore| {
        non_disponibile(
            "artefatto",
            &format!("CLOEXEC non si toglie dal descrittore dell'artefatto: {errore}"),
        )
    })
}

/// I punti in cui la qualificazione puo' chiedere un guasto.
///
/// L'invariante da misurare e' **che cosa resta** quando un passo fallisce
/// (nessun descrittore ereditabile nel supervisore, nessun processo, nessuno
/// zombie), e su una macchina sana quei fallimenti non si producono a
/// comando. Vivono sotto `qualificazione_isolamento`, un `cfg` di `rustc` e
/// non una feature, che l'unificazione propagherebbe. Il punto si legge da
/// una variabile d'ambiente, mai scritta: non c'e' stato da ripristinare.
///
/// «dopo-lo-spawn» non ha un punto di chiamata qui, ma sta nell'elenco perche'
/// e' la lista di cio' che si riconosce: senza, il braccio che lo chiede
/// verrebbe rifiutato dagli altri punti.
#[cfg(qualificazione_isolamento)]
pub(super) const PUNTI_DI_GUASTO: [&str; 4] =
    ["primo-fcntl", "secondo-fcntl", "spawn", "dopo-lo-spawn"];

/// La variabile che nomina il punto.
#[cfg(qualificazione_isolamento)]
pub(super) const VARIABILE_DI_GUASTO: &str = "PLENORA_QUALIFICAZIONE_GUASTO";

/// Se la qualificazione ha chiesto un guasto **qui**.
///
/// Un nome sconosciuto e' un rifiuto: altrimenti il braccio misurerebbe il
/// cammino ordinario e riporterebbe verde.
///
/// # Errors
///
/// [`PlenoraError::IsolationUnavailable`] col punto richiesto, se e' questo o
/// se non e' nessuno.
#[cfg(qualificazione_isolamento)]
pub(super) fn guasto_richiesto(qui: &str) -> Result<()> {
    let Ok(chiesto) = std::env::var(VARIABILE_DI_GUASTO) else {
        return Ok(());
    };
    if chiesto == qui {
        return Err(non_disponibile(
            "guasto di qualificazione",
            &format!("guasto richiesto in «{qui}»"),
        ));
    }
    if PUNTI_DI_GUASTO.contains(&chiesto.as_str()) {
        return Ok(());
    }
    Err(non_disponibile(
        "guasto di qualificazione",
        &format!(
            "«{chiesto}» non e' un punto di guasto: il braccio non misurerebbe cio' che crede"
        ),
    ))
}

/// Che il processo abbia **un task solo**, adesso.
///
/// Adesso, non all'avvio: fra l'avvio e la finestra in cui i descrittori
/// diventano ereditabili puo' essere nato un thread.
///
/// # Errors
///
/// [`PlenoraError::IsolationUnavailable`] col numero di task trovati.
pub(super) fn accerta_monothread() -> Result<()> {
    let voci = std::fs::read_dir("/proc/self/task")
        .map_err(|errore| non_disponibile("canale", &format!("/proc/self/task: {errore}")))?;
    let mut quanti = 0_usize;
    for voce in voci {
        voce.map_err(|errore| non_disponibile("canale", &format!("/proc/self/task: {errore}")))?;
        quanti += 1;
    }
    if quanti == 1 {
        return Ok(());
    }
    Err(non_disponibile(
        "canale",
        &format!(
            "il supervisore ha {quanti} task: rendere ereditabili i descrittori qui vorrebbe dire \
             che un altro thread puo' avviare un processo che se li porta via"
        ),
    ))
}

#[cfg(test)]
mod tests {
    use super::{forma_di_pipe_anonima, numero_ammissibile, verso_dai_flag, Verso};

    /// I canali standard non trasportano il protocollo, e i numeri negativi non
    /// sono descrittori.
    #[test]
    fn i_numeri_ammissibili_cominciano_da_tre() {
        for numero in [-1, -100, 0, 1, 2] {
            assert!(
                numero_ammissibile(numero).is_err(),
                "{numero} non puo' essere un estremo"
            );
        }
        for numero in [3, 4, 1024] {
            assert!(
                numero_ammissibile(numero).is_ok(),
                "{numero} e' ammissibile"
            );
        }
    }

    /// La forma della pipe anonima e' esatta, e una FIFO del filesystem non ce
    /// l'ha.
    #[test]
    fn solo_la_forma_esatta_e_una_pipe_anonima() {
        assert!(forma_di_pipe_anonima("pipe:[1]"));
        assert!(forma_di_pipe_anonima("pipe:[162733]"));

        for finta in [
            "/tmp/pipe:[162733]",   // una FIFO nominata: readlink rende un percorso
            "/run/la-mia-fifo",     // idem, senza travestimento
            "pipe:[]",              // senza cifre
            "pipe:[12x]",           // non tutte cifre
            "pipe:[12]/altro",      // con una coda
            "pipe:162733",          // senza parentesi
            "socket:[162733]",      // un altro tipo di oggetto anonimo
            "anon_inode:[eventfd]", // idem
            "",
        ] {
            assert!(
                !forma_di_pipe_anonima(finta),
                "«{finta}» non e' una pipe anonima"
            );
        }
    }

    /// Il verso si legge dai flag ottali, e `O_RDWR` e' un rifiuto.
    #[test]
    fn il_verso_si_legge_e_lettura_scrittura_si_rifiuta() {
        assert!(verso_dai_flag("00", Verso::Lettura).is_ok());
        assert!(verso_dai_flag("01", Verso::Scrittura).is_ok());
        // I flag veri portano altri bit: solo i due bassi contano.
        assert!(verso_dai_flag("0100000", Verso::Lettura).is_ok());
        assert!(verso_dai_flag("0100001", Verso::Scrittura).is_ok());

        // Il verso sbagliato.
        assert!(verso_dai_flag("00", Verso::Scrittura).is_err());
        assert!(verso_dai_flag("01", Verso::Lettura).is_err());

        // `O_RDWR`, in entrambi i versi attesi.
        for atteso in [Verso::Lettura, Verso::Scrittura] {
            let motivo = verso_dai_flag("02", atteso).expect_err("O_RDWR si rifiuta");
            assert!(
                motivo.contains("lettura e scrittura"),
                "il rifiuto non nomina la ragione: {motivo}"
            );
        }

        // Flag illeggibili.
        assert!(verso_dai_flag("", Verso::Lettura).is_err());
        assert!(verso_dai_flag("non-un-numero", Verso::Lettura).is_err());
        assert!(verso_dai_flag("09", Verso::Lettura).is_err());
    }

    // --- su pipe vere ------------------------------------------------------
    //
    // Da qui in giu' i casi aprono pipe vere. Non serve ne' un cgroup ne' un
    // privilegio: cio' che si misura e' come il kernel presenta una pipe in
    // `/proc`, e quello si vede da qualunque processo.

    /// **Il fatto su cui poggia il confronto delle impronte**: i due lati di
    /// una pipe condividono l'inode.
    ///
    /// Per questo due estremi con impronta uguale **non** possono essere i due
    /// lati di un canale. Si misura perche' l'intuizione dice il contrario.
    #[test]
    #[cfg(target_os = "linux")]
    fn i_due_lati_di_una_pipe_condividono_l_inode() {
        use std::os::fd::AsRawFd as _;
        let (legge, scrive) = std::io::pipe().expect("la pipe si crea");
        let ingresso = super::accerta(legge.as_raw_fd(), Verso::Lettura).expect("lato di lettura");
        let uscita =
            super::accerta(scrive.as_raw_fd(), Verso::Scrittura).expect("lato di scrittura");
        assert_eq!(
            ingresso.impronta, uscita.impronta,
            "i due lati di una pipe sono la stessa pipe"
        );
    }

    /// Un canale sono **due** pipe, ed e' cosi' che si riconosce.
    #[test]
    #[cfg(target_os = "linux")]
    fn due_pipe_fanno_un_canale() {
        use std::os::fd::AsRawFd as _;
        // Come le apre il supervisore: una per verso.
        let (_sup_legge, worker_scrive) = std::io::pipe().expect("la pipe si crea");
        let (worker_legge, _sup_scrive) = std::io::pipe().expect("la pipe si crea");
        let (nl, ns) = (worker_legge.as_raw_fd(), worker_scrive.as_raw_fd());

        let numeri = super::accerta_coppia(nl, ns).expect("i due estremi reggono");
        assert_eq!(
            (numeri.legge, numeri.scrive),
            (nl, ns),
            "i numeri resi sono quelli osservati"
        );
        // I versi e le impronte si riguardano da qui: `accerta_coppia` rende i
        // numeri verificati, e cio' che ha verificato resta osservabile
        // chiedendolo di nuovo ai singoli estremi.
        let ingresso = super::accerta(nl, Verso::Lettura).expect("il lato di lettura regge");
        let uscita = super::accerta(ns, Verso::Scrittura).expect("il lato di scrittura regge");
        assert_eq!(ingresso.verso, Verso::Lettura);
        assert_eq!(uscita.verso, Verso::Scrittura);
        assert_ne!(
            ingresso.impronta, uscita.impronta,
            "i due versi del canale sono due pipe distinte"
        );

        // Scambiati, non reggono: e' il controllo del verso a dirlo.
        assert!(
            super::accerta_coppia(ns, nl).is_err(),
            "invertire i due estremi deve essere un rifiuto"
        );
    }

    /// Due estremi che appartengono alla **stessa** pipe non sono un canale.
    #[test]
    #[cfg(target_os = "linux")]
    fn una_pipe_sola_non_fa_un_canale() {
        use std::os::fd::AsRawFd as _;
        let (legge_a, scrive_a) = std::io::pipe().expect("la pipe si crea");
        // Si riapre l'estremo di lettura **in scrittura**: e' un descrittore
        // valido, con il verso giusto per il posto in cui lo si mette, e
        // appartiene alla stessa pipe dell'altro. Solo il confronto delle
        // impronte lo smaschera.
        let scrive_b = std::fs::OpenOptions::new()
            .write(true)
            .open(format!("/proc/self/fd/{}", legge_a.as_raw_fd()))
            .expect("l'estremo di lettura si riapre in scrittura");
        let _ = &scrive_a;

        let esito = super::accerta_coppia(legge_a.as_raw_fd(), scrive_b.as_raw_fd());
        let motivo = esito.expect_err("un canale che parla con se stesso non e' un canale");
        assert!(
            motivo.to_string().contains("stessa pipe"),
            "il rifiuto non nomina la ragione: {motivo}"
        );
    }

    /// **Il fatto che rende necessario il secondo confronto**: l'impronta non
    /// dice il verso.
    ///
    /// Riaprire l'estremo di *lettura* **in scrittura** riesce e rende
    /// un'impronta identica. Il caso misura il fatto, non il nostro controllo.
    #[test]
    #[cfg(target_os = "linux")]
    fn l_impronta_non_distingue_il_verso() {
        use std::os::fd::AsRawFd as _;
        use std::os::unix::fs::MetadataExt as _;

        let (legge, _scrive) = std::io::pipe().expect("la pipe si crea");
        let numero = legge.as_raw_fd();
        let originale = std::fs::metadata(format!("/proc/self/fd/{numero}")).expect("metadata");

        let al_contrario = std::fs::OpenOptions::new()
            .write(true)
            .open(format!("/proc/self/fd/{numero}"))
            .expect("riaprire in scrittura riesce: e' il punto");
        let riaperto = al_contrario.metadata().expect("metadata del riaperto");

        assert_eq!(
            (originale.dev(), originale.ino()),
            (riaperto.dev(), riaperto.ino()),
            "l'impronta e' identica: e' per questo che da sola non basta"
        );

        // Cio' che invece distingue: i flag del nuovo descrittore.
        let flag = super::flag_di(al_contrario.as_raw_fd()).expect("i flag si leggono");
        assert!(
            verso_dai_flag(&flag, Verso::Lettura).is_err(),
            "il riaperto non e' in lettura, e i flag lo dicono"
        );
        assert!(verso_dai_flag(&flag, Verso::Scrittura).is_ok());
    }

    /// La riapertura rende un descrittore che **funziona**, sulla stessa pipe.
    ///
    /// Il confronto delle impronte dice che l'inode e' lo stesso; questo dice
    /// che ci passano davvero i byte. Sono due cose diverse, e la seconda e'
    /// quella che al worker interessa.
    #[test]
    #[cfg(target_os = "linux")]
    fn il_descrittore_riaperto_porta_i_byte() {
        use std::io::{Read as _, Write as _};
        use std::os::fd::AsRawFd as _;

        let (legge, mut scrive) = std::io::pipe().expect("la pipe si crea");
        let mut riaperto = super::riapri_accertato(legge.as_raw_fd(), Verso::Lettura)
            .expect("l'estremo di lettura si riapre");

        scrive.write_all(b"plenora").expect("si scrive");
        drop(scrive);

        let mut letto = Vec::new();
        riaperto.read_to_end(&mut letto).expect("si legge");
        assert_eq!(letto, b"plenora");
    }

    /// Un verso sbagliato viene rifiutato **prima** della riapertura.
    ///
    /// L'ordine conta: chiedere di riaprire in scrittura un estremo di lettura
    /// riuscirebbe, e il rifiuto arriverebbe dopo aver aperto qualcosa. Qui il
    /// controllo sul descrittore ereditato viene prima, e non si apre niente.
    #[test]
    #[cfg(target_os = "linux")]
    fn il_verso_sbagliato_si_rifiuta_prima_di_aprire() {
        use std::os::fd::AsRawFd as _;
        let (legge, scrive) = std::io::pipe().expect("la pipe si crea");
        assert!(super::riapri_accertato(legge.as_raw_fd(), Verso::Scrittura).is_err());
        assert!(super::riapri_accertato(scrive.as_raw_fd(), Verso::Lettura).is_err());
    }

    /// **La non-garanzia, misurata**: riaprire non adotta e non chiude
    /// l'ereditato.
    ///
    /// Dopo che il descrittore riaperto e' caduto, il numero originale nomina
    /// ancora la stessa pipe: per questo «il worker non avvia altri processi»
    /// e' un'invariante operativa e non una garanzia.
    #[test]
    #[cfg(target_os = "linux")]
    fn riaprire_non_chiude_l_ereditato() {
        use std::os::fd::AsRawFd as _;
        let (legge, _scrive) = std::io::pipe().expect("la pipe si crea");
        let numero = legge.as_raw_fd();
        let prima = std::fs::read_link(format!("/proc/self/fd/{numero}")).expect("readlink");

        let riaperto =
            super::riapri_accertato(numero, Verso::Lettura).expect("si riapre in lettura");
        assert_ne!(
            riaperto.as_raw_fd(),
            numero,
            "la riapertura rende un descrittore nuovo, non lo stesso"
        );
        drop(riaperto);

        let dopo = std::fs::read_link(format!("/proc/self/fd/{numero}")).expect("readlink");
        assert_eq!(
            prima, dopo,
            "l'ereditato e' ancora li', e nomina ancora la stessa pipe"
        );
    }

    /// Una FIFO del filesystem non passa per una pipe anonima.
    ///
    /// E' il caso che il solo `is_fifo()` non distinguerebbe: una FIFO nominata
    /// **e'** una FIFO, e senza il controllo sulla forma del bersaglio un
    /// worker potrebbe essere fatto parlare con un file che chiunque puo'
    /// aprire.
    #[test]
    #[cfg(target_os = "linux")]
    fn una_fifo_nominata_non_e_il_canale() {
        use std::os::fd::AsRawFd as _;
        use std::os::unix::fs::OpenOptionsExt as _;

        let cartella = std::env::temp_dir().join(format!("plenora-fifo-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&cartella);
        std::fs::create_dir_all(&cartella).expect("la cartella si crea");
        let fifo = cartella.join("canale");

        let fatto = std::process::Command::new("/usr/bin/mkfifo")
            .arg(&fifo)
            .status();
        let Ok(stato) = fatto else {
            let _ = std::fs::remove_dir_all(&cartella);
            return; // Senza `mkfifo` non c'e' niente da misurare.
        };
        if !stato.success() {
            let _ = std::fs::remove_dir_all(&cartella);
            return;
        }

        // Si apre in lettura senza bloccare: una FIFO senza scrittori
        // bloccherebbe l'apertura, e il caso resterebbe fermo invece di
        // misurare.
        let aperta = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(o_nonblock())
            .open(&fifo);
        if let Ok(aperta) = aperta {
            let esito = super::accerta(aperta.as_raw_fd(), Verso::Lettura);
            let motivo = esito.expect_err("una FIFO nominata non e' il canale del supervisore");
            assert!(
                motivo.to_string().contains("pipe anonima"),
                "il rifiuto non nomina la ragione: {motivo}"
            );
        }
        let _ = std::fs::remove_dir_all(&cartella);
    }

    /// `O_NONBLOCK`, scritto qui invece di dipendere da `libc`.
    ///
    /// Il valore e' 0o4000 su Linux e non cambia: e' parte dell'ABI. Prenderlo
    /// da una dipendenza nuova per una costante di quattro cifre non vale il
    /// prezzo.
    #[cfg(target_os = "linux")]
    const fn o_nonblock() -> i32 {
        0o4000
    }

    // --- il controllo dopo la riapertura -----------------------------------
    //
    // Quattro combinazioni sulla funzione pura, e due sulla **chiamata**. Le
    // prime provano che il giudizio sia giusto; le seconde che venga
    // esercitato, che e' una cosa diversa: un controllo corretto e mai chiamato
    // e' un controllo che non c'e'.

    /// Le impronte dei casi, costruite invece che osservate.
    #[cfg(target_os = "linux")]
    const fn impronta(dispositivo: u64, inode: u64) -> super::Impronta {
        super::Impronta { dispositivo, inode }
    }

    /// Un'osservazione del riaperto, costruita.
    #[cfg(target_os = "linux")]
    fn adozione(dispositivo: u64, inode: u64, flag: &str) -> super::Adozione {
        super::Adozione {
            impronta: impronta(dispositivo, inode),
            flag: flag.to_owned(),
        }
    }

    /// I flag di un descrittore aperto in sola lettura, e in sola scrittura.
    ///
    /// Sono i valori che `fdinfo` riporta in ottale: `O_RDONLY` vale zero e
    /// `O_WRONLY` uno, e il resto dei bit non riguarda il verso.
    #[cfg(target_os = "linux")]
    const IN_LETTURA: &str = "0100000";
    #[cfg(target_os = "linux")]
    const IN_SCRITTURA: &str = "0100001";

    /// **Impronta e verso giusti**: l'adozione regge.
    ///
    /// E' la meta' che non va dimenticata. Senza, un controllo che rifiutasse
    /// *tutto* passerebbe i tre casi negativi e romperebbe la produzione.
    #[test]
    #[cfg(target_os = "linux")]
    fn un_adozione_intatta_regge() {
        let prima = impronta(27, 4242);
        let dopo = adozione(27, 4242, IN_LETTURA);
        assert!(super::accerta_adozione(3, &prima, &dopo, Verso::Lettura).is_ok());
    }

    /// **Stessa impronta, verso sbagliato**: e' il caso che l'impronta non
    /// coglie, ed e' misurato altrove che accade davvero.
    #[test]
    #[cfg(target_os = "linux")]
    fn stessa_impronta_e_verso_sbagliato_si_rifiuta() {
        let prima = impronta(27, 4242);
        let dopo = adozione(27, 4242, IN_SCRITTURA);
        let motivo = super::accerta_adozione(3, &prima, &dopo, Verso::Lettura)
            .expect_err("un estremo riaperto al contrario non e' lo stesso estremo");
        assert!(
            motivo.to_string().contains("aperto in scrittura"),
            "il rifiuto non nomina il verso: {motivo}"
        );
    }

    /// **Stesso inode, dispositivo diverso**: due oggetti che condividono un
    /// numero per caso, su due filesystem.
    #[test]
    #[cfg(target_os = "linux")]
    fn stesso_inode_su_un_altro_dispositivo_si_rifiuta() {
        let prima = impronta(27, 4242);
        let dopo = adozione(28, 4242, IN_LETTURA);
        let motivo = super::accerta_adozione(3, &prima, &dopo, Verso::Lettura)
            .expect_err("un inode uguale su un altro filesystem non e' lo stesso oggetto");
        assert!(
            motivo.to_string().contains("un altro filesystem"),
            "il rifiuto non viene dal confronto sul dispositivo: {motivo}"
        );
    }

    /// **Stesso dispositivo, inode diverso**: il numero e' stato riusato.
    #[test]
    #[cfg(target_os = "linux")]
    fn stesso_dispositivo_e_inode_diverso_si_rifiuta() {
        let prima = impronta(27, 4242);
        let dopo = adozione(27, 4243, IN_LETTURA);
        let motivo = super::accerta_adozione(3, &prima, &dopo, Verso::Lettura)
            .expect_err("un altro inode non e' lo stesso oggetto");
        // La frase e' quella che **solo** questo ramo produce: cercare la parola
        // «inode» non basterebbe, perche' anche il messaggio del dispositivo la
        // contiene, e il caso resterebbe verde con il confronto sbagliato.
        assert!(
            motivo.to_string().contains("non e' piu' lo stesso oggetto"),
            "il rifiuto non viene dal confronto sull'inode: {motivo}"
        );
    }

    /// **La chiamata, al suo posto**: un inode divergente ferma la riapertura.
    ///
    /// Su una pipe vera, con un osservatore che dichiara un altro oggetto:
    /// togliere la chiamata da `riapri_accertato_con` lascerebbe verdi i casi
    /// sulla funzione pura. Diverge **solo** l'inode, perche' il caso misuri
    /// quel confronto e non il primo dei tre che incontra.
    #[test]
    #[cfg(target_os = "linux")]
    fn un_inode_divergente_ferma_la_riapertura() {
        use std::os::fd::AsRawFd as _;
        use std::os::unix::fs::MetadataExt as _;

        let (legge, _scrive) = std::io::pipe().expect("la pipe si crea");
        let esito = super::riapri_accertato_con(legge.as_raw_fd(), Verso::Lettura, |riaperto| {
            let dati = riaperto.metadata().expect("metadata del riaperto");
            Ok(super::Adozione {
                impronta: impronta(dati.dev(), dati.ino().wrapping_add(1)),
                flag: IN_LETTURA.to_owned(),
            })
        });
        let motivo = esito.expect_err("un altro oggetto non si adotta");
        assert!(
            motivo.to_string().contains("non e' piu' lo stesso oggetto"),
            "il rifiuto non viene dal confronto sull'inode: {motivo}"
        );
    }

    /// **La chiamata, al suo posto**: un verso divergente ferma la riapertura.
    ///
    /// L'impronta qui e' quella giusta — la si legge dal descrittore vero — e
    /// diverge **solo** il verso. E' il caso che distingue il controllo del
    /// verso da quello dell'impronta: senza il primo, questo resterebbe verde.
    #[test]
    #[cfg(target_os = "linux")]
    fn un_verso_divergente_ferma_la_riapertura() {
        use std::os::fd::AsRawFd as _;
        use std::os::unix::fs::MetadataExt as _;

        let (legge, _scrive) = std::io::pipe().expect("la pipe si crea");
        let esito = super::riapri_accertato_con(legge.as_raw_fd(), Verso::Lettura, |riaperto| {
            let dati = riaperto.metadata().expect("metadata del riaperto");
            Ok(super::Adozione {
                impronta: impronta(dati.dev(), dati.ino()),
                flag: IN_SCRITTURA.to_owned(),
            })
        });
        let motivo = esito.expect_err("un estremo riaperto al contrario non si adotta");
        assert!(
            motivo.to_string().contains("aperto in scrittura"),
            "il rifiuto non nomina il verso: {motivo}"
        );
    }

    /// L'osservatore vero legge cio' che c'e', e l'adozione passa.
    ///
    /// Chiude il giro: la giuntura non e' una scorciatoia che evita la lettura
    /// vera, e cio' che la produzione passa supera i suoi stessi controlli.
    #[test]
    #[cfg(target_os = "linux")]
    fn l_osservazione_vera_regge_su_una_pipe_vera() {
        use std::os::fd::AsRawFd as _;
        let (legge, _scrive) = std::io::pipe().expect("la pipe si crea");
        let riaperto = super::riapri_accertato_con(
            legge.as_raw_fd(),
            Verso::Lettura,
            super::osservazione_vera,
        )
        .expect("l'osservazione vera regge");
        assert_ne!(riaperto.as_raw_fd(), legge.as_raw_fd());
    }

    // --- accerta_artefatto: il terzo descrittore, un file regolare ----------
    //
    // Stessa disciplina di `accerta`, su un file regolare vero di questo
    // processo, senza privilegi, spawn o dominio. Le funzioni pure sono
    // provate sopra; qui si prova la **chiamata**.

    /// Un file regolare vero, con contenuto qualunque — serve solo un inode
    /// reale su cui `/proc/self/fd/<n>` possa risolversi.
    #[cfg(target_os = "linux")]
    fn file_di_prova(dir: &std::path::Path, nome: &str) -> std::path::PathBuf {
        let percorso = dir.join(nome);
        std::fs::write(&percorso, b"artefatto di prova").expect("scrittura del file di prova");
        percorso
    }

    /// Un numero non ammissibile si rifiuta prima ancora di guardare
    /// `/proc/self/fd`: stesso rifiuto di [`numero_ammissibile`], ma
    /// esercitato dalla chiamata vera.
    #[test]
    #[cfg(target_os = "linux")]
    fn accerta_artefatto_rifiuta_un_numero_non_ammissibile() {
        let motivo =
            super::accerta_artefatto(0).expect_err("uno standard stream non e' ammissibile");
        assert!(
            motivo.to_string().contains("fd 0"),
            "il rifiuto deve nominare il numero rifiutato: {motivo}"
        );
    }

    /// Una pipe vera non e' un file regolare: e' esattamente cio' che
    /// l'artefatto del verificatore non puo' essere.
    #[test]
    #[cfg(target_os = "linux")]
    fn accerta_artefatto_rifiuta_cio_che_non_e_un_file_regolare() {
        use std::os::fd::AsRawFd as _;
        let (legge, _scrive) = std::io::pipe().expect("la pipe si crea");
        let motivo = super::accerta_artefatto(legge.as_raw_fd())
            .expect_err("una pipe non e' un file regolare");
        assert!(
            motivo.to_string().contains("non e' un file regolare"),
            "il rifiuto non viene dal controllo sul tipo di oggetto: {motivo}"
        );
    }

    /// Un file regolare vero, aperto in scrittura: il verso e' sbagliato, non
    /// il tipo di oggetto — un controllo che confondesse i due lascerebbe
    /// questo caso verde per la ragione sbagliata.
    #[test]
    #[cfg(target_os = "linux")]
    fn accerta_artefatto_rifiuta_il_verso_sbagliato() {
        use std::os::fd::AsRawFd as _;
        let dir = tempfile::tempdir().expect("cartella temporanea");
        let percorso = file_di_prova(dir.path(), "in-scrittura.bin");
        let file = std::fs::OpenOptions::new()
            .write(true)
            .open(&percorso)
            .expect("apertura in scrittura");
        let motivo = super::accerta_artefatto(file.as_raw_fd())
            .expect_err("un file aperto in scrittura non e' l'artefatto in lettura atteso");
        assert!(
            motivo.to_string().contains("scrittura"),
            "il rifiuto non nomina il verso: {motivo}"
        );
    }

    /// Un file regolare vero, aperto in lettura: l'unico caso che deve
    /// riuscire, con l'impronta che coincide con quella osservata per
    /// davvero sul filesystem.
    #[test]
    #[cfg(target_os = "linux")]
    fn accerta_artefatto_accetta_un_file_regolare_in_lettura() {
        use std::os::fd::AsRawFd as _;
        use std::os::unix::fs::MetadataExt as _;
        let dir = tempfile::tempdir().expect("cartella temporanea");
        let percorso = file_di_prova(dir.path(), "in-lettura.bin");
        let file = std::fs::File::open(&percorso).expect("apertura in lettura");
        let dati = file.metadata().expect("metadata del file vero");

        let estremo = super::accerta_artefatto(file.as_raw_fd())
            .expect("un file regolare in lettura e' esattamente l'artefatto atteso");
        assert_eq!(estremo.verso, Verso::Lettura);
        assert_eq!(estremo.impronta.dispositivo, dati.dev());
        assert_eq!(estremo.impronta.inode, dati.ino());
    }

    // --- riapri_accertato_artefatto_con: la chiamata, su un file vero -------

    /// Un inode divergente ferma la riapertura dell'artefatto, sullo stesso
    /// principio di `un_inode_divergente_ferma_la_riapertura` per la pipe del
    /// worker: la funzione pura non basta, serve la chiamata.
    #[test]
    #[cfg(target_os = "linux")]
    fn un_inode_divergente_ferma_la_riapertura_dell_artefatto() {
        use std::os::fd::AsRawFd as _;
        use std::os::unix::fs::MetadataExt as _;
        let dir = tempfile::tempdir().expect("cartella temporanea");
        let percorso = file_di_prova(dir.path(), "artefatto.bin");
        let file = std::fs::File::open(&percorso).expect("apertura in lettura");

        let esito = super::riapri_accertato_artefatto_con(file.as_raw_fd(), |riaperto| {
            let dati = riaperto.metadata().expect("metadata del riaperto");
            Ok(super::Adozione {
                impronta: impronta(dati.dev(), dati.ino().wrapping_add(1)),
                flag: IN_LETTURA.to_owned(),
            })
        });
        let motivo = esito.expect_err("un altro inode non e' lo stesso artefatto");
        assert!(
            motivo.to_string().contains("non e' piu' lo stesso oggetto"),
            "il rifiuto non viene dal confronto sull'inode: {motivo}"
        );
    }

    /// Un verso divergente ferma la riapertura dell'artefatto: l'impronta e'
    /// quella vera, e diverge solo il verso che l'osservatore dichiara.
    #[test]
    #[cfg(target_os = "linux")]
    fn un_verso_divergente_ferma_la_riapertura_dell_artefatto() {
        use std::os::fd::AsRawFd as _;
        use std::os::unix::fs::MetadataExt as _;
        let dir = tempfile::tempdir().expect("cartella temporanea");
        let percorso = file_di_prova(dir.path(), "artefatto.bin");
        let file = std::fs::File::open(&percorso).expect("apertura in lettura");

        let esito = super::riapri_accertato_artefatto_con(file.as_raw_fd(), |riaperto| {
            let dati = riaperto.metadata().expect("metadata del riaperto");
            Ok(super::Adozione {
                impronta: impronta(dati.dev(), dati.ino()),
                flag: IN_SCRITTURA.to_owned(),
            })
        });
        let motivo = esito.expect_err("un artefatto riaperto al contrario non si adotta");
        assert!(
            motivo.to_string().contains("aperto in scrittura"),
            "il rifiuto non nomina il verso: {motivo}"
        );
    }

    /// L'osservatore vero, su un artefatto vero: chiude il giro esattamente
    /// come [`l_osservazione_vera_regge_su_una_pipe_vera`] lo chiude per la
    /// pipe del worker.
    #[test]
    #[cfg(target_os = "linux")]
    fn l_osservazione_vera_regge_sull_artefatto_vero() {
        use std::os::fd::AsRawFd as _;
        let dir = tempfile::tempdir().expect("cartella temporanea");
        let percorso = file_di_prova(dir.path(), "artefatto.bin");
        let file = std::fs::File::open(&percorso).expect("apertura in lettura");

        let riaperto =
            super::riapri_accertato_artefatto_con(file.as_raw_fd(), super::osservazione_vera)
                .expect("l'osservazione vera regge sull'artefatto");
        assert_ne!(riaperto.as_raw_fd(), file.as_raw_fd());
    }
}
