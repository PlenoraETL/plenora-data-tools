//! Che cosa il worker fa dell'incarico: lo **rivalida**, lo esegue, e dichiara.
//!
//! Rivalida perche' riceve un piano, non una prova. La rivalidazione e' la
//! **stessa** `planner::validate`, e il `plan_hash` che ne esce si confronta
//! con quello dichiarato: se divergono, i due lati non parlano dello stesso
//! piano.
//!
//! Prima gli ingressi, poi il piano: `planner::validate` usa i contratti
//! d'ingresso, e un contratto sbagliato va respinto nominando l'ingresso, non
//! il piano.
//!
//! Non pubblica e non verifica il proprio artefatto: i passi da 3 a 8-bis e il
//! passo 9 appartengono a chi osserva il worker.

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
/// L'osservatore del progresso riceve i totali dopo ogni batch scritto, e il
/// suo errore **interrompe**: e' il canale verso chi aspetta.
///
/// # Errors
///
/// Qualunque errore della rivalidazione, dell'apertura degli ingressi,
/// dell'esecuzione, della scrittura o del digest. Il chiamante lo porta
/// sull'`Esito` come errore dichiarato, senza sanificarlo ne' riclassificarlo.
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
        // Il worker esegue QUI, dentro il dominio che il supervisore ha gia'
        // preparato e confinato: la richiesta di isolamento che il piano
        // porta ancora e' quella che questa stessa esecuzione sta servendo,
        // non una nuova richiesta da respingere (`PR-12`).
        gia_confinato: Some(crate::prepare::Confinamento::interno()),
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
/// Il contratto completo non viaggia (servirebbe un codec reversibile del
/// `DataContract`, e il worker si fiderebbe di una descrizione altrui):
/// viaggia il **fingerprint**. Copre schema e contratto, non il contenuto:
/// file con righe diverse e lo stesso schema hanno lo stesso fingerprint.
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
/// `Input::read_ipc_with_limits` riconosce il formato dal magic: senza questo
/// controllo il campo `formato` dell'incarico non lo leggerebbe nessuno.
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
/// Si confronta il `plan_hash`, identita' del piano **migrato e canonico**,
/// non il testo: due testi diversi possono essere lo stesso piano.
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
/// Zero: nello stream i nodi restano attivi fino all'ultimo batch, e le
/// metriche per nodo esistono tutte dalla nascita dello stato, quindi
/// contarle direbbe quanti nodi ha il piano. Zero dice cio' che si osserva.
///
/// Il limite e' registrato in `errori-e-limiti.md`. Rientra quando l'executor
/// osserva il completamento per nodo — che e' una contabilita' nuova sul
/// percorso caldo, non una lettura di cio' che c'e'.
const fn nodi_completati_osservabili() -> u64 {
    0
}

/// Lo SHA-256 dell'intero artefatto finalizzato, footer compreso.
///
/// Si rilegge il file finito perche' il footer lo scrive `finish` alla fine,
/// e un digest sui byte in transito non coprirebbe il sigillo. E' lo stesso
/// valore che ricalcola il passo 5-bis.
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
/// Il **nome** che il piano dichiara, non il percorso, che il supervisore
/// conosce gia'. `con_contesto` lo attacca solo alle varianti con un messaggio
/// nostro (`InvalidPlan`, `Schema`, `Crs`, `Unsupported`), non a `Io` o
/// `DataMapping`: forzarlo cambierebbe la categoria dell'errore. I rifiuti
/// costruiti qui (formato, fingerprint) nominano l'ingresso da se'.
fn all_ingresso(descrittore: &DescrittoreIngresso, causa: PlenoraError) -> PlenoraError {
    causa.con_contesto(&format!("ingresso `{}`", descrittore.nome))
}
