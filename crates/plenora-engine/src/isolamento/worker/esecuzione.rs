//! Che cosa il worker fa dell'incarico: lo **rivalida**, lo esegue, e dichiara.
//!
//! # Perche' rivalida invece di fidarsi
//!
//! Perche' il supervisore gli manda un piano, non una prova. Il piano e' gia'
//! stato validato dall'altro lato, ma «e' stato validato» e' un'affermazione
//! altrui: il worker che la accettasse eseguirebbe cio' che gli e' arrivato, e
//! non cio' su cui i due si sono accordati. Un incarico intercettato o
//! costruito male sarebbe eseguito senza che nessun passo lo neghi.
//!
//! La rivalidazione non e' una copia della validazione del supervisore: e' la
//! **stessa**, chiamata di nuovo. `planner::validate` e' un'autorita' unica, e
//! il `plan_hash` che ne esce si confronta con quello dichiarato. Se
//! divergono, i due lati non stanno parlando dello stesso piano — ed e' l'unica
//! cosa che conta sapere.
//!
//! # L'ordine dei rifiuti
//!
//! Prima gli ingressi, poi il piano. Non e' indifferente: `planner::validate`
//! ha bisogno dei contratti d'ingresso per validare il grafo, quindi un
//! contratto che non e' quello atteso va respinto **prima** che il piano venga
//! validato contro di lui. L'ordine opposto validerebbe un grafo su contratti
//! che poi si scoprono sbagliati, e il rifiuto arriverebbe nominando il piano
//! invece dell'ingresso che lo ha causato.
//!
//! # Che cosa questo modulo non fa
//!
//! Non pubblica, e non verifica il proprio artefatto. La verifica e' dei passi
//! da 3 a 8-bis e la pubblicazione e' il passo 9: appartengono a chi ha
//! osservato il worker, non al worker. Un processo che verificasse se stesso
//! confronterebbe la propria affermazione con la propria affermazione.

use std::path::Path;

use plenora_core::contract::arrow_schema::contract_from_arrow_schema;
use plenora_core::contract::DataContract;
use plenora_core::error::{ErrorPhase, PlenoraError};
use sha2::{Digest as _, Sha256};

use crate::cancellation::CancellationToken;
use crate::commit_token::CommitToken;
use crate::esadecimale32::Esadecimale32;
use crate::executor::{execute, Input, Inputs};
use crate::ipc_boundary::{self, IpcFormat, IpcLimits};
use crate::planner;
use crate::prepare::RuntimeContext;
use crate::protocollo::digest::ALGORITMO_DIGEST;
use crate::protocollo::messaggi::{
    ConteggiDichiarati, DescrittoreIngresso, DigestArtefatto, FormatoIngresso, Incarico, Progresso,
};

use super::Result;

/// Byte letti per volta nel digest dell'artefatto.
///
/// Costante e piccola, per la stessa ragione del passo 5-bis: e' cio' che rende
/// il digest a memoria costante invece che proporzionale alla dimensione
/// dell'artefatto.
const BLOCCO_DIGEST: usize = 64 * 1024;

/// Esegue l'incarico e rende cio' che l'`Esito` dichiara.
///
/// # Il progresso
///
/// L'osservatore riceve i totali dopo ogni batch scritto. Il suo errore
/// **interrompe**: e' il canale verso chi aspetta, e continuare a scrivere
/// senza di lui produrrebbe un artefatto che nessuno sa di dover verificare.
///
/// # Errors
///
/// Qualunque errore della rivalidazione, dell'apertura degli ingressi,
/// dell'esecuzione, della scrittura o del digest. Il chiamante lo porta
/// sull'`Esito` come errore dichiarato: qui non si sanifica e non si
/// riclassifica nulla, perche' gli assi dell'errore sono gia' quelli giusti.
pub(super) fn esegui(
    incarico: &Incarico,
    token: &CommitToken,
    annullamento: &CancellationToken,
    progresso: &mut dyn FnMut(Progresso) -> Result<()>,
) -> Result<(DigestArtefatto, ConteggiDichiarati)> {
    let contratti = contratti_degli_ingressi(&incarico.ingressi)?;
    let grafo = planner::validate(incarico.piano_canonico.get(), &contratti)?;
    accerta_il_piano(&grafo, incarico)?;

    // `max_parallelism` si applica **prima** di aprire gli ingressi e prima di
    // qualunque uso di Rayon, come sul percorso in-process: dimensiona il pool
    // del processo, che e' l'unica leva che vincola tutti i percorsi paralleli
    // dei kernel. Applicarlo dopo lo renderebbe una promessa di risorsa.
    crate::parallelism::configure(grafo.effective_limits().max_parallelism)?;
    let runtime = RuntimeContext {
        max_parallelism: grafo.effective_limits().max_parallelism,
        // Lo **stesso** token che l'ascolto puo' cancellare. Un token nuovo
        // qui sarebbe una seconda leva, e l'annullamento tirerebbe quella che
        // l'executor non guarda: il lavoro proseguirebbe fino in fondo e il
        // supervisore dovrebbe forzare la terminazione.
        cancellation: annullamento.clone(),
        ..RuntimeContext::default()
    };
    // I tetti del confine derivano dai limiti **effettivi** del piano appena
    // rivalidato, non da quelli dichiarati dal supervisore: il worker applica
    // cio' che ha verificato.
    let limiti = ipc_boundary::limits_from_plan(
        grafo.effective_limits(),
        runtime.batch_target.max_batch_bytes,
    );

    let mut ingressi = Inputs::strict();
    for (descrittore, (nome, contratto)) in incarico.ingressi.iter().zip(contratti) {
        let percorso = Path::new(&descrittore.percorso);
        ingressi.add_with_contract(
            nome,
            Input::read_ipc_with_limits(percorso, &limiti)
                .map_err(|causa| all_ingresso(descrittore, causa))?,
            contratto,
        )?;
    }

    let uscita = execute(&grafo, ingressi, runtime)?;
    // I nodi completati non li osserva nessuno, e il perche' sta su
    // `nodi_completati_osservabili`.
    let (_metriche, conteggi) = uscita.scrivi_artefatto_isolato(
        Path::new(&incarico.artefatto_temporaneo),
        token,
        &mut |scritti| {
            progresso(Progresso {
                righe: scritti.righe,
                batch: scritti.batch,
                nodi_completati: nodi_completati_osservabili(),
            })
        },
    )?;

    let digest = digest_dell_artefatto(Path::new(&incarico.artefatto_temporaneo))?;
    Ok((digest, conteggi))
}

/// I contratti degli ingressi, **riletti dai file** e riconosciuti.
///
/// # Perche' riletti e non trasportati
///
/// Perche' il contratto completo non viaggia, e la ragione sta sul tipo del
/// filo: trasportarlo richiederebbe un codec reversibile del `DataContract`
/// che oggi non esiste, e farebbe fidare il worker di una descrizione altrui
/// invece del file che sta per leggere. Cio' che viaggia e' il **fingerprint**,
/// cioe' quanto basta per dire «e' quello» senza dire che cos'e'.
///
/// Il fingerprint copre schema e contratto, non l'identita' dei dati: due file
/// con righe diverse e lo stesso schema hanno lo stesso fingerprint, e va detto
/// perche' non si prenda questo passo per una verifica del contenuto.
///
/// # Errors
///
/// [`PlenoraError::Protocol`] se il formato dichiarato non e' quello del file o
/// se il fingerprint non e' quello atteso; propaga gli errori di lettura
/// dell'header.
fn contratti_degli_ingressi(
    descrittori: &[DescrittoreIngresso],
) -> Result<Vec<(String, DataContract)>> {
    let mut contratti = Vec::with_capacity(descrittori.len());
    for descrittore in descrittori {
        let percorso = Path::new(&descrittore.percorso);
        accerta_il_formato(descrittore, percorso)?;
        // I tetti dell'header sono quelli di default: il piano non e' ancora
        // validato, quindi i suoi limiti effettivi non si conoscono. Leggere
        // l'header col tetto del piano richiederebbe il piano, e validare il
        // piano richiede i contratti: l'unico ordine possibile passa dal
        // default, che e' quello del confine e non del piano.
        let schema = ipc_boundary::header_schema(percorso, &IpcLimits::default())
            .map_err(|causa| all_ingresso(descrittore, causa))?;
        // Il risolutore e' quello del selettore tipizzato, lo stesso che il
        // worker ha **dichiarato** nell'handshake e su cui il supervisore ha
        // convenuto: leggere i contratti con un altro renderebbe l'accordo una
        // formalita'.
        let letto = contract_from_arrow_schema(schema, crate::risolutore::risolvi)
            .map_err(|causa| all_ingresso(descrittore, causa))?;
        let impronta = planner::contract_fingerprint(&letto)
            .map_err(|causa| all_ingresso(descrittore, causa))?;
        if impronta.to_hex() != descrittore.contract_fingerprint_atteso.in_esadecimale() {
            // Nessuno dei due fingerprint entra nel messaggio: sono funzioni
            // dello schema, e uno schema descrive i dati. Chi legge deve sapere
            // **quale** ingresso non e' quello atteso, non che aspetto ha.
            return Err(all_ingresso(
                descrittore,
                PlenoraError::Protocol(
                    "il contratto letto dal file non e' quello che l'incarico dichiara".to_owned(),
                ),
            ));
        }
        contratti.push((descrittore.nome.clone(), letto));
    }
    Ok(contratti)
}

/// Il formato dichiarato e' quello del file.
///
/// # Perche' non basta lo sniffing
///
/// Perche' `Input::read_ipc_with_limits` riconosce il formato dal magic e apre
/// di conseguenza: senza questo controllo, il campo `formato` dell'incarico
/// sarebbe un campo che nessuno legge, e un ingresso dichiarato `file` e
/// consegnato come `stream` verrebbe eseguito comunque. Un accordo su un valore
/// che non si verifica non e' un accordo.
///
/// # Errors
///
/// [`PlenoraError::Protocol`] se i due non coincidono; propaga l'errore dello
/// sniffing se il file non e' ne' l'uno ne' l'altro.
fn accerta_il_formato(descrittore: &DescrittoreIngresso, percorso: &Path) -> Result<()> {
    let riconosciuto =
        ipc_boundary::sniff_format(percorso).map_err(|causa| all_ingresso(descrittore, causa))?;
    let atteso = match descrittore.formato {
        FormatoIngresso::File => IpcFormat::File,
        FormatoIngresso::Stream => IpcFormat::Stream,
    };
    if riconosciuto == atteso {
        return Ok(());
    }
    Err(all_ingresso(
        descrittore,
        PlenoraError::Protocol(format!(
            "l'incarico lo dichiara in formato {}, il file e' in formato {}",
            nome_del_formato(atteso),
            nome_del_formato(riconosciuto)
        )),
    ))
}

/// Il nome di un formato, per i messaggi.
const fn nome_del_formato(quale: IpcFormat) -> &'static str {
    match quale {
        IpcFormat::File => "file",
        IpcFormat::Stream => "stream",
    }
}

/// Il piano rivalidato e' quello che l'incarico dichiara.
///
/// # Perche' il confronto e' sull'hash e non sul testo
///
/// Perche' il `plan_hash` e' l'identita' del piano **migrato e canonico**, ed e'
/// l'unica nozione di «stesso piano» che il programma ha. Confrontare i testi
/// direbbe «stessi byte», che e' una domanda diversa: due testi diversi possono
/// essere lo stesso piano, e il supervisore ne manda la forma canonica proprio
/// perche' quella differenza non conti.
///
/// # Errors
///
/// [`PlenoraError::Protocol`] se gli hash divergono.
fn accerta_il_piano(grafo: &planner::ValidatedGraph, incarico: &Incarico) -> Result<()> {
    if grafo.plan_hash().to_hex() == incarico.plan_hash_atteso.in_esadecimale() {
        return Ok(());
    }
    // Nessuno dei due hash entra nel messaggio. Sono l'identita' di un piano, e
    // il piano e' l'unica cosa che il supervisore e il worker hanno in comune:
    // dirne uno in un log non aggiunge niente a chi legge, che sa gia' quale
    // tentativo sta guardando.
    Err(PlenoraError::Protocol(
        "il piano rivalidato non e' quello che l'incarico dichiara: i due lati non stanno \
         eseguendo lo stesso piano"
            .to_owned(),
    )
    .with_phase(ErrorPhase::Validate))
}

/// Quanti nodi risultano completati **mentre** l'artefatto si scrive.
///
/// # Perche' zero, e perche' non e' una svista
///
/// Perche' questa esecuzione e' uno stream: i nodi non finiscono uno dopo
/// l'altro, restano attivi finche' l'ultimo batch non e' passato. Le metriche
/// per nodo esistono dal primo istante — sono create tutte in una volta alla
/// nascita dello stato — quindi contarle direbbe **quanti nodi ha il piano**,
/// non quanti ne hanno finito il lavoro.
///
/// Dichiarare quel numero come «completati» sarebbe un progresso che parte al
/// massimo e non si muove: peggio di zero, perche' sembrerebbe
/// un'informazione. Zero dice cio' che si osserva, e cio' che si osserva e'
/// niente.
///
/// Il limite e' registrato in `errori-e-limiti.md`. Rientra quando l'executor
/// osserva il completamento per nodo — che e' una contabilita' nuova sul
/// percorso caldo, non una lettura di cio' che c'e'.
const fn nodi_completati_osservabili() -> u64 {
    0
}

/// Lo SHA-256 dell'intero artefatto finalizzato, footer compreso.
///
/// # Perche' rileggendolo, e non mentre si scrive
///
/// Perche' il digest copre **il file finito**, footer compreso, e il footer lo
/// scrive `finish` alla fine: un digest calcolato sui byte che passano
/// coprirebbe tutto tranne la coda, cioe' non coprirebbe il sigillo. E'
/// deliberatamente lo stesso valore che il passo 5-bis ricalcola: se le due
/// letture divergessero, l'artefatto sarebbe cambiato fra la scrittura e la
/// verifica, e il rifiuto arriverebbe da chi lo verifica.
///
/// # Errors
///
/// [`PlenoraError::Io`] se l'artefatto non si rilegge.
fn digest_dell_artefatto(percorso: &Path) -> Result<DigestArtefatto> {
    use std::io::Read as _;

    let mut artefatto = std::fs::File::open(percorso).map_err(|causa| {
        PlenoraError::Io(causa)
            .con_contesto("l'artefatto appena scritto non si rilegge per il digest")
            .with_phase(ErrorPhase::Read)
    })?;
    let mut digestore = Sha256::new();
    let mut blocco = vec![0_u8; BLOCCO_DIGEST];
    loop {
        let quanti = artefatto.read(&mut blocco).map_err(|causa| {
            PlenoraError::Io(causa)
                .con_contesto("l'artefatto non si legge per il digest")
                .with_phase(ErrorPhase::Read)
        })?;
        if quanti == 0 {
            break;
        }
        digestore.update(&blocco[..quanti]);
    }
    Ok(DigestArtefatto {
        algoritmo: ALGORITMO_DIGEST.to_owned(),
        valore: Esadecimale32::dai_byte(digestore.finalize().into()).in_esadecimale(),
    })
}

/// L'errore di un ingresso, col nome che lo riguarda.
///
/// Il **nome** e non il percorso: il nome e' quello che il piano dichiara, ed e'
/// cio' che permette a chi legge di sapere quale arco del grafo non regge. Il
/// percorso lo ha scelto il supervisore, che quindi lo sa gia'.
///
/// # Dove il nome si attacca, e dove no
///
/// `con_contesto` tocca soltanto le varianti che portano un **messaggio
/// nostro** — `InvalidPlan`, `Schema`, `Crs`, `Unsupported` — e lascia intatte
/// quelle che portano un errore di sistema o un'attribuzione propria, come `Io`
/// e `DataMapping`. Su quelle il nome non compare, e va detto invece di
/// lasciarlo credere.
///
/// Non lo si forza: anteporre il nome ricostruendo una variante diversa
/// cambierebbe la **categoria** dell'errore, e un limite di risorsa che
/// diventasse un errore di mappatura direbbe a chi classifica una cosa falsa —
/// che e' peggio di un nome mancante. I rifiuti che questo modulo costruisce da
/// se' nominano l'ingresso nel proprio testo, e sono quelli che riguardano
/// l'incarico: formato dichiarato e fingerprint.
fn all_ingresso(descrittore: &DescrittoreIngresso, causa: PlenoraError) -> PlenoraError {
    causa.con_contesto(&format!("ingresso `{}`", descrittore.nome))
}
