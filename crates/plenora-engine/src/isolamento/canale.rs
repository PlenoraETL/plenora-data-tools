//! Gli estremi delle pipe: che cosa sono, e come si accerta che lo siano.
//!
//! # Perche' una verifica composta e non un controllo solo
//!
//! Perche' nessuno dei controlli disponibili, da solo, dice cio' che serve.
//!
//! `is_fifo()` dimostra che l'oggetto e' una FIFO, e accetta allo stesso modo
//! una **pipe anonima** e una **FIFO nominata** del filesystem. Il contratto
//! vuole solo le prime: una FIFO nominata puo' essere stata creata da chiunque
//! abbia scrittura su una directory, e chiunque puo' aprirla dall'altro capo.
//!
//! Il verso non si vede dal tipo: un descrittore su una pipe puo' essere in
//! lettura, in scrittura, o — se qualcuno riapre `/proc/self/fd` — in entrambi.
//! `O_RDWR` non e' un caso limite da tollerare: e' un estremo che puo' fare
//! cio' che l'altro dovrebbe fare, e il canale smette di avere due lati.
//!
//! E il numero non dice niente di suo: `0`, `1` e `2` sono descrittori
//! legittimi, ma sono **stdin, stdout e stderr**, che per contratto non
//! trasportano il protocollo.
//!
//! Quindi quattro prove, ciascuna per la cosa che sa:
//!
//! | prova | che cosa dimostra |
//! |---|---|
//! | il numero e' `>= 3` e i due sono distinti | non e' uno dei canali standard, e non e' lo stesso estremo due volte |
//! | `metadata` dice `S_IFIFO` | e' una FIFO |
//! | `readlink` ha la forma **esatta** `pipe:[cifre]` | e' **anonima**: il bersaglio di una FIFO del filesystem non ha quella forma |
//! | `fdinfo` dice `O_RDONLY` o `O_WRONLY` | il verso e' quello atteso, e non e' `O_RDWR` |
//!
//! Piu' due prove **dopo la riapertura**, e servono entrambe. Riaprire
//! `/proc/self/fd/N` rende un descrittore **nuovo**, quindi:
//!
//! - `(st_dev, st_ino)` uguali a quelli osservati prima, o niente direbbe che
//!   punta ancora alla stessa pipe;
//! - il **verso** riletto sul nuovo handle. Su Linux `/proc/self/fd/N` per una
//!   pipe riapre **la pipe**, non l'estremo: la si puo' riaprire nel verso
//!   opposto, e l'impronta resta identica. E' misurato, non temuto — quindi un
//!   confronto sulla sola impronta accetterebbe un handle che legge dove
//!   deve scrivere.
//!
//! # Che cosa questa verifica **non** dimostra
//!
//! Chi c'e' dall'altro capo. Una pipe anonima non porta l'identita' di chi la
//! ha creata, e nessuna di queste prove la ricostruisce. Cio' che discrimina
//! un supervisore da un estraneo e' l'handshake del protocollo, non il
//! descrittore — e va detto, perche' la tentazione opposta e' forte: si e'
//! appena finito di controllare quattro cose, e sembra di sapere piu' di
//! quanto si sa.

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
/// # Perche' la forma esatta e non un prefisso
///
/// Perche' `pipe:[` come prefisso lascerebbe passare `pipe:[12x]`, `pipe:[]` e
/// `pipe:[1]/qualcosa`. La forma e' fissa: `pipe:`, una parentesi quadra,
/// almeno una cifra decimale, la chiusura, e **nient'altro**.
///
/// La proprieta' che serve e' stretta quanto basta: il bersaglio di una FIFO
/// del filesystem **non coincide** con quella forma. Non serve promettere che
/// cosa sia — un percorso, e con quale aspetto — perche' ogni promessa in piu'
/// e' una cosa in piu' che puo' smettere di essere vera senza che nessuno se
/// ne accorga.
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
/// `0` sola lettura, `1` sola scrittura, `2` lettura e scrittura.
///
/// # Perche' `O_RDWR` e' un rifiuto e non un caso piu' generoso
///
/// Perche' un estremo che puo' fare entrambe le cose non e' un estremo: e' il
/// canale intero in mano a un lato solo. Il worker potrebbe leggere cio' che
/// ha scritto per il supervisore, o scrivere su cio' da cui dovrebbe leggere,
/// e il protocollo smetterebbe di avere due direzioni.
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
/// # Perche' assenza e duplicazione sono due errori
///
/// Perche' dicono due cose diverse. Senza la riga, il descrittore non e' cio'
/// che crediamo — o `/proc` non e' quello vero — e non c'e' verso da leggere.
/// Con due righe, il file ne porta due possibili, e prenderne una e' una
/// convenzione che nessuno ha dichiarato: su un file di kernel una
/// duplicazione dice che quel file non e' quello che crediamo.
///
/// Un solo messaggio per i due casi manderebbe a cercare la cosa sbagliata.
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
/// # Perche' l'ambiente e non la riga di comando
///
/// Perche' la riga di comando del worker e' **sua**: gli argomenti dopo `--`
/// arrivano da chi ha chiesto l'esecuzione, e infilarci due numeri vorrebbe dire
/// che il canale occupa posizioni che appartengono a un altro. Un worker che
/// legge i propri argomenti troverebbe due voci che non ha chiesto, e un worker
/// che non se le aspetta le passerebbe oltre.
///
/// # Che cosa questa variabile **non** e'
///
/// Una prova. Chi legge da qui non deve credere ai numeri: deve riguardarli, ed
/// e' esattamente cio' che fa [`riapri_accertato`]. Un numero sbagliato — per
/// errore o perche' qualcuno ha riscritto l'ambiente — porta a un descrittore
/// che non supera i controlli, non a un canale diverso accettato per buono.
///
/// # Chi la scrive e chi la legge
///
/// La scrive lo **spawner**, dalla coppia che ha appena rivalidato, e la
/// **impone**: un valore ereditato indicherebbe descrittori veri di un altro
/// canale, e il worker li troverebbe buoni. La legge il **worker**, che e' il
/// primo a riaprire davvero i propri estremi.
pub(super) const VARIABILE_DEL_CANALE: &str = "PLENORA_CANALE";

/// Lo stadio del worker: riguarda il descrittore ereditato, lo **riapre**, e
/// confronta.
///
/// # Perche' riaprire, invece di usare quello che c'e'
///
/// Perche' un descrittore ereditato e' un numero, e un numero non si puo'
/// adottare in Rust senza `unsafe`: `OwnedFd::from_raw_fd` e
/// `BorrowedFd::borrow_raw` lo sono entrambi, e questo progetto non ne ammette.
/// Cio' che si puo' fare senza e' **aprirne uno nuovo** attraverso
/// `/proc/self/fd/<n>`, che rende un `File` posseduto — chiuso dal suo `Drop`,
/// e nato `CLOEXEC` come ogni apertura di Rust.
///
/// # I due confronti dopo la riapertura, e perche' nessuno dei due basta
///
/// **L'impronta** `(dispositivo, inode)` dice che il nuovo descrittore guarda la
/// **stessa pipe** di quello ereditato. Senza, la riapertura potrebbe finire
/// altrove — il numero puo' essere stato riusato fra il controllo e l'apertura —
/// e il worker parlerebbe con qualcosa che non e' il supervisore.
///
/// **Il verso** dice che ci guarda dal lato giusto. E serve, perche' l'impronta
/// da sola non lo dice: riaprire l'estremo di lettura di una pipe *in
/// scrittura* riesce, e rende un descrittore con **impronta identica**. E' una
/// cosa misurata, non dedotta: sono due aperture della stessa pipe, e la pipe e'
/// una sola. Un controllo che si fermasse all'impronta accetterebbe un worker
/// che scrive dove deve leggere, e il difetto si vedrebbe come un silenzio.
///
/// # Che cosa resta aperto, e va detto
///
/// Il descrittore **ereditato** resta li'. Riaprire non lo adotta e non lo
/// chiude — anche questo e' misurato: dopo aver lasciato cadere il descrittore
/// riaperto, il numero originale continua a nominare la stessa pipe — e
/// chiuderlo richiederebbe di adottarlo, cioe' `unsafe`. Non e' `CLOEXEC`,
/// perche' il supervisore glielo ha tolto apposta per farglielo ereditare.
///
/// La conseguenza e' un'**invariante operativa, non una garanzia**: finche' il
/// worker non avvia altri processi, quel descrittore non va da nessuna parte. Se
/// un domani il worker eseguisse qualcosa, se lo porterebbe dietro. Sta scritto
/// fra le non-garanzie, dove le cose di questo genere devono stare.
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
/// # Perche' un tipo, e non due letture in mezzo al codice
///
/// Perche' e' cio' che separa l'osservazione dal giudizio. Con le letture
/// sparse nella funzione, il giudizio si puo' provare solo producendo davvero
/// la divergenza — e le divergenze che contano non si producono su una macchina
/// sana. Raccolte in un valore, il giudizio diventa una funzione pura, e le
/// divergenze si scrivono.
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
/// # Perche' tre condizioni e non una
///
/// Perche' rispondono a tre domande diverse, e un solo messaggio per tutte e
/// tre manderebbe a cercare la cosa sbagliata.
///
/// **L'inode** dice quale oggetto. **Il dispositivo** dice su quale filesystem:
/// gli inode sono unici dentro un filesystem, non fra filesystem diversi, e
/// confrontare il solo inode accetterebbe come «la stessa pipe» due oggetti che
/// condividono un numero per caso. **Il verso** dice da che lato, e non lo dice
/// nessuna delle altre due: riaprire in scrittura l'estremo di lettura di una
/// pipe riesce e rende un'impronta **identica**, perche' sono due aperture della
/// stessa pipe.
///
/// # Perche' e' una funzione a se'
///
/// Perche' altrimenti non si potrebbe provare. Le divergenze che questo
/// controllo esiste per fermare non si producono su una macchina sana: fra
/// l'accertamento e l'apertura non passa niente che possa cambiare il
/// descrittore. Descrivere quell'assenza — «non e' raggiungibile» — racconta
/// l'esecuzione sana, non prova il codice. Estratta, la divergenza si scrive, e
/// il controllo si misura.
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
/// # Perche' esiste, e perche' non e' una porta aperta
///
/// Esiste perche' il controllo dopo la riapertura vada **provato al suo posto**,
/// e non solo come funzione isolata: togliere la chiamata da qui deve far
/// diventare rosso qualcosa, altrimenti il controllo c'e' ma nessuno verifica
/// che venga esercitato.
///
/// E' privata e ha **un solo chiamante di produzione**, che le passa
/// [`osservazione_vera`]. Cio' che un osservatore puo' variare e' che cosa si
/// dichiara di aver visto, mai che cosa si controlla: il giudizio resta
/// [`accerta_adozione`], che questa funzione chiama sempre.
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
/// # Perche' un tipo che possiede, e non due variabili
///
/// Perche' fra la rimozione di `CLOEXEC` e lo `spawn` ogni ritorno anticipato
/// e' un cammino che potrebbe lasciare vivo, nel supervisore, un descrittore
/// **ereditabile**: il prossimo `spawn` di chiunque se lo porterebbe dietro, e
/// l'EOF del canale non arriverebbe mai perche' un estraneo terrebbe aperto
/// l'altro capo.
///
/// Possedendoli, `Drop` corre su **ogni** uscita — il `?` che fallisce sul
/// primo `fcntl`, quello che fallisce sul secondo, lo `spawn` che non riesce —
/// e li chiude entrambi. Non e' una disciplina da ricordare: e' il tipo che non
/// lascia scelta.
///
/// # Perche' non c'e' un modo di estrarli
///
/// Perche' un accessore che ne rendesse la proprieta' rimetterebbe in piedi
/// esattamente i cammini che questo tipo esiste per chiudere. Cio' che si puo'
/// avere sono i **numeri**, che servono alla richiesta e non tengono niente
/// aperto.
#[cfg(any(test, feature = "internals"))]
pub(super) struct EstremiDelWorker {
    legge: std::io::PipeReader,
    scrive: std::io::PipeWriter,
}

#[cfg(any(test, feature = "internals"))]
impl EstremiDelWorker {
    /// I due numeri, per la richiesta — **riguardandoli**.
    ///
    /// Non li si legge e basta: si passa da [`accerta_coppia`], che e' l'unica
    /// che li rende. Costa due letture di `fdinfo` e toglie un modo di
    /// sbagliare: senza, questo sarebbe un secondo posto in cui due interi
    /// diventano «i numeri del canale» senza che nessuno li abbia guardati.
    ///
    /// # Errors
    ///
    /// [`PlenoraError::IsolationUnavailable`] se i due estremi non sono quelli
    /// dichiarati. Qui non dovrebbe capitare — `apri` li ha appena creati — e
    /// «non dovrebbe» e' precisamente cio' che questa chiamata smette di dare
    /// per scontato.
    pub(super) fn numeri(&self) -> Result<super::NumeriDelCanale> {
        accerta_coppia(self.legge.as_raw_fd(), self.scrive.as_raw_fd())
    }
}

/// Il canale: quattro estremi, due per lato.
///
/// Nasce **prima** della richiesta, e non e' un dettaglio d'ordine: la
/// richiesta porta i numeri dei due estremi del worker, quindi non puo'
/// esistere prima di loro. Costruirla vuota e riempirla dopo renderebbe
/// rappresentabile una richiesta senza canale, che e' uno stato che non deve
/// esistere.
///
/// # Errors
///
/// [`PlenoraError::IsolationUnavailable`] se le pipe non si creano, o se uno
/// dei quattro estremi non e' quello che dev'essere.
#[cfg(any(test, feature = "internals"))]
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
/// # Perche' la validita' individuale non basta
///
/// Perche' quattro estremi ciascuno valido possono essere accoppiati male.
/// Se `sup_legge` appartenesse alla pipe su cui scrive `sup_scrive`, il
/// supervisore parlerebbe con se stesso e il worker con nessuno: nessuna delle
/// verifiche precedenti se ne accorge, perche' ciascuna guarda un estremo alla
/// volta.
///
/// E un canale accoppiato male non produce un errore. Produce **silenzio**:
/// ognuno scrive e nessuno legge, finche' un timeout non chiude la faccenda
/// dicendo la cosa sbagliata — «il worker non risponde» invece di «il canale
/// non e' un canale».
///
/// # Errors
///
/// [`PlenoraError::IsolationUnavailable`], che nomina quale accoppiamento
/// manca.
#[cfg(any(test, feature = "internals"))]
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

#[cfg(any(test, feature = "internals"))]
impl EstremiDelWorker {
    /// Toglie `CLOEXEC` ai due estremi, e non a nient'altro.
    ///
    /// # La finestra che questo apre, e perche' e' strettissima
    ///
    /// Da qui allo `spawn` i due descrittori sono **ereditabili**: qualunque
    /// altro `spawn` del processo se li porterebbe dietro, e un estraneo che
    /// tenesse aperto l'altro capo impedirebbe per sempre l'EOF del canale.
    ///
    /// Per questo la finestra contiene **solo** lo `spawn`: richiesta,
    /// argomenti e `Command` sono gia' costruiti quando questa funzione viene
    /// chiamata, e fra lei e il `.spawn()` non c'e' niente che possa fallire a
    /// lungo o creare processi.
    ///
    /// # Errors
    ///
    /// [`PlenoraError::IsolationUnavailable`] se `fcntl` non riesce. Il
    /// chiamante lascia allora cadere la guardia, che chiude **entrambi** gli
    /// estremi — compreso il primo, se a fallire e' stato il secondo dopo che
    /// il primo e' gia' diventato ereditabile.
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

/// I quattro punti in cui la qualificazione puo' chiedere un guasto.
///
/// # Perche' esistono, e perche' non in produzione
///
/// Perche' l'invariante da misurare non e' *come* un passo fallisce ma **che
/// cosa resta** quando fallisce: nessun descrittore ereditabile vivo nel
/// supervisore, nessun processo, nessuno zombie. Quei fallimenti non si
/// producono a comando su una macchina sana — un `fcntl` su un descrittore
/// valido riesce sempre — e senza un punto in cui chiederli l'invariante
/// resterebbe non misurata.
///
/// Vivono sotto `qualificazione_isolamento`, che e' un `cfg` passato a `rustc`
/// e **non** una feature: una feature l'unificazione la propaga a chi non l'ha
/// chiesta, un `cfg` no.
///
/// # Perche' da una variabile d'ambiente e non da uno stato modificabile
///
/// Perche' uno stato che si accende e si spegne va anche rimesso a posto, e un
/// ripristino dimenticato lascerebbe verde un braccio che non ha misurato
/// niente. La variabile e' letta e mai scritta: ogni braccio del gate e' un
/// processo suo, con il suo ambiente, e non c'e' niente da ripristinare.
/// I quattro nomi ammessi. «dopo-lo-spawn» non ha un punto di chiamata qui —
/// e' una giuntura del binario di qualificazione — ma sta nell'elenco lo stesso,
/// perche' questa e' la lista di cio' che si riconosce, non di cio' che si
/// esegue: senza, un braccio che lo chiedesse verrebbe rifiutato da tutti e tre
/// i punti proprio mentre sta misurando il suo.
#[cfg(qualificazione_isolamento)]
pub(super) const PUNTI_DI_GUASTO: [&str; 4] =
    ["primo-fcntl", "secondo-fcntl", "spawn", "dopo-lo-spawn"];

/// La variabile che nomina il punto.
#[cfg(qualificazione_isolamento)]
pub(super) const VARIABILE_DI_GUASTO: &str = "PLENORA_QUALIFICAZIONE_GUASTO";

/// Se la qualificazione ha chiesto un guasto **qui**.
///
/// # Perche' un nome sconosciuto e' un rifiuto e non un'assenza
///
/// Perche' un valore che nessun punto riconosce vorrebbe dire che il gate ha
/// chiesto un guasto che non arrivera' mai: il braccio proseguirebbe fino in
/// fondo e riporterebbe verde, avendo misurato il cammino ordinario invece di
/// quello che vuole. Un braccio che passa per non aver fatto niente e' peggio
/// di un braccio rosso.
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
/// # Perche' adesso e non una volta sola all'avvio
///
/// Perche' fra l'avvio e questo punto un thread puo' essere nato, e la finestra
/// in cui i descrittori sono ereditabili e' esattamente quella che sta per
/// aprirsi. Un accertamento fatto prima direbbe una cosa vera su un momento che
/// non e' questo.
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
    /// E' cio' che rende l'impronta capace di dire «stessa pipe», ed e' anche
    /// il motivo per cui due estremi con impronta uguale **non** possono essere
    /// i due lati di un canale: sarebbero i due lati della stessa pipe, cioe' un
    /// canale in cui il worker parla con se stesso.
    ///
    /// Va misurato e non dedotto, perche' l'intuizione dice il contrario — due
    /// estremi, due oggetti — e su quell'intuizione il confronto sarebbe stato
    /// scritto al rovescio.
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
    /// Riaprire l'estremo di *lettura* di una pipe **in scrittura** riesce, e
    /// rende un descrittore con impronta identica — sono due aperture della
    /// stessa pipe, e la pipe e' una sola. Un controllo che si fermasse
    /// all'impronta accetterebbe un worker che scrive dove deve leggere.
    ///
    /// Questo caso misura il fatto, non il nostro controllo: e' il fatto a
    /// dire perche' il controllo del verso non e' ridondante.
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
    /// ancora la stessa pipe. E' la ragione per cui «il worker non avvia altri
    /// processi» sta fra le invarianti operative e non fra le garanzie: quel
    /// descrittore non e' `CLOEXEC`, e chiuderlo richiederebbe di adottarlo,
    /// cioe' `unsafe`.
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
    /// Su una pipe vera, con un osservatore che dichiara di aver visto un altro
    /// oggetto. E' cio' che i casi sulla funzione pura non coprono: togliere la
    /// chiamata da `riapri_accertato_con` li lascerebbe tutti verdi.
    ///
    /// # Perche' diverge **solo** l'inode
    ///
    /// Perche' un'osservazione tutta sbagliata — dispositivo zero e inode zero —
    /// verrebbe fermata dal primo dei tre confronti che la incontra, e il caso
    /// resterebbe verde anche togliendo gli altri due. Facendo divergere una
    /// cosa sola, il caso misura **quel** confronto e non il gruppo.
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
}
