//! Esecuzione di un piano da file a file.
//!
//! 1. Controlli che non richiedono dati: ogni output del piano ha un solo
//!    percorso e ogni percorso d'uscita nomina un output; percorsi d'uscita
//!    distinti fra loro e dagli ingressi (per testo e, per i file che
//!    esistono, per percorso canonico); destinazioni scrivibili (assenti, o
//!    sovrascrittura esplicita).
//! 2. Caricamento degli input nell'ordine dato, ciascuno con il budget
//!    residuo (`max_governed_memory_bytes` del piano meno i byte vivi delle
//!    tabelle già caricate): le tabelle caricate contano nel budget come
//!    in `run`, che ricontrolla lo stato iniziale.
//! 3. `validate` con gli schemi letti, poi `run`.
//! 4. Scrittura degli output nell'ordine del piano: prima di ciascuno i
//!    byte vivi degli output non ancora scritti più il transitorio previsto
//!    della scrittura devono stare nel budget; ogni output si libera appena
//!    scritto. Un errore a metà lascia scritti gli output precedenti (ogni
//!    file è atomico, l'insieme no): dopo il primo output scritto l'errore
//!    dichiara effetto `partial` (`PlenoraError::with_remote_effect`), e il
//!    ritentativo non è più automatico.
//!
//! Gli input possono essere anche tabelle già in memoria ([`Ingresso`]),
//! per le superfici che le ricevono dal chiamante (l'SDK Python): passano
//! dagli stessi controlli di budget e d'interruzione dei file, e
//! [`esegui_in_memoria`] rende gli output invece di scriverli.

use std::path::{Path, PathBuf};

use plenora_core::arrow::array::RecordBatch;
use plenora_core::limits::Limits;
use plenora_core::memoria::byte_vivi;
use plenora_core::{ErrorPhase, PlenoraError, RemoteEffect, Result};
use plenora_pipeline::{Esito, Interruzione, Pipeline, PipelineValidata, Report};

use crate::formato::{Formato, OpzioniScrittura};
use crate::memoria::oltre_il_budget;
use crate::{atomico, ipc, leggi_tabella, parquet_io, scrivi_tabella};

/// Un input del piano letto da file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileIngresso {
    /// Nome dell'input nel piano.
    pub nome: String,
    pub percorso: PathBuf,
    /// Formato; `None` dall'estensione.
    pub formato: Option<Formato>,
}

/// Un output del piano scritto su file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileUscita {
    /// Nome dell'output nel piano.
    pub nome: String,
    pub percorso: PathBuf,
    /// Formato; `None` dall'estensione. Prevale su `OpzioniScrittura::formato`.
    pub formato: Option<Formato>,
}

/// Un input del piano: un file da leggere o una tabella già in memoria.
#[derive(Clone, Debug)]
pub enum Ingresso {
    File(FileIngresso),
    /// Una tabella data dal chiamante: non si legge, ma conta nel budget
    /// come una tabella letta.
    Tabella {
        nome: String,
        tabella: RecordBatch,
    },
}

impl Ingresso {
    /// Nome dell'input nel piano.
    #[must_use]
    pub fn nome(&self) -> &str {
        match self {
            Self::File(file) => &file.nome,
            Self::Tabella { nome, .. } => nome,
        }
    }
}

impl From<FileIngresso> for Ingresso {
    fn from(file: FileIngresso) -> Self {
        Self::File(file)
    }
}

fn da_file(ingressi: &[FileIngresso]) -> Vec<Ingresso> {
    ingressi.iter().cloned().map(Ingresso::File).collect()
}

fn assoluto(percorso: &Path) -> Result<PathBuf> {
    Ok(std::path::absolute(percorso)?)
}

/// Controlli sui percorsi, prima di leggere qualunque dato. Gli input in
/// memoria non hanno percorso e non entrano nei confronti.
fn verifica_percorsi(
    piano: &Pipeline,
    ingressi: &[Ingresso],
    uscite: &[FileUscita],
    opzioni: OpzioniScrittura,
) -> Result<()> {
    for (indice, uscita) in uscite.iter().enumerate() {
        if !piano.outputs.contains(&uscita.nome) {
            return Err(PlenoraError::InvalidPlan(format!(
                "`{}`: percorso d'uscita per un nome che non e' un output del piano",
                uscita.nome
            )));
        }
        if uscite[..indice]
            .iter()
            .any(|altra| altra.nome == uscita.nome)
        {
            return Err(PlenoraError::InvalidPlan(format!(
                "`{}`: due percorsi d'uscita per lo stesso output",
                uscita.nome
            )));
        }
    }
    if let Some(mancante) = piano
        .outputs
        .iter()
        .find(|nome| !uscite.iter().any(|uscita| &uscita.nome == *nome))
    {
        return Err(PlenoraError::InvalidPlan(format!(
            "`{mancante}`: output del piano senza percorso d'uscita"
        )));
    }
    let mut visti: Vec<PathBuf> = Vec::new();
    let mut canonici_ingressi: Vec<PathBuf> = Vec::new();
    for ingresso in ingressi.iter().filter_map(|ingresso| match ingresso {
        Ingresso::File(file) => Some(file),
        Ingresso::Tabella { .. } => None,
    }) {
        Formato::risolvi(ingresso.formato, &ingresso.percorso)?;
        visti.push(assoluto(&ingresso.percorso)?);
        canonici_ingressi
            .push(std::fs::canonicalize(&ingresso.percorso).map_err(|errore| {
                con_nome(&format!("input `{}`", ingresso.nome), errore.into())
            })?);
    }
    let numero_ingressi = visti.len();
    for uscita in uscite {
        Formato::risolvi(uscita.formato.or(opzioni.formato), &uscita.percorso)?;
        let percorso = assoluto(&uscita.percorso)?;
        if let Some(posizione) = visti.iter().position(|visto| *visto == percorso) {
            let cosa = if posizione < numero_ingressi {
                "e' anche un ingresso"
            } else {
                "e' gia' un'altra uscita"
            };
            return Err(PlenoraError::InvalidPlan(format!(
                "`{}`: il percorso d'uscita {cosa}",
                uscita.nome
            )));
        }
        visti.push(percorso);
        atomico::verifica_destinazione(&uscita.percorso, opzioni.sovrascrivi)
            .map_err(|errore| con_nome(&format!("output `{}`", uscita.nome), errore))?;
        // Stesso file sotto un altro nome (maiuscole su Windows, link): il
        // confronto per testo sopra non lo vede, quello canonico si'.
        if let Some(canonico) = canonico_se_esiste(&uscita.percorso)? {
            if canonici_ingressi.contains(&canonico) {
                return Err(PlenoraError::InvalidPlan(format!(
                    "`{}`: il percorso d'uscita e' anche un ingresso",
                    uscita.nome
                )));
            }
        }
    }
    Ok(())
}

/// Il percorso canonico di un file che esiste; `None` se non esiste.
fn canonico_se_esiste(percorso: &Path) -> Result<Option<PathBuf>> {
    match std::fs::canonicalize(percorso) {
        Ok(canonico) => Ok(Some(canonico)),
        Err(errore) if errore.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(errore) => Err(errore.into()),
    }
}

/// Antepone il nome dell'input o dell'output al messaggio, senza cambiare
/// la categoria dell'errore (nomi del piano, mai dati).
fn con_nome(contesto: &str, errore: PlenoraError) -> PlenoraError {
    let anteponi = |messaggio: String| format!("{contesto}: {messaggio}");
    match errore {
        PlenoraError::ResourceLimit(m) => PlenoraError::ResourceLimit(anteponi(m)),
        PlenoraError::DataMapping(m) => PlenoraError::DataMapping(anteponi(m)),
        PlenoraError::Internal(m) => PlenoraError::Internal(anteponi(m)),
        PlenoraError::Conflict(m) => PlenoraError::Conflict(anteponi(m)),
        PlenoraError::Io(e) => PlenoraError::io_con_contesto(contesto, e),
        altro => altro.con_contesto(contesto),
    }
}

/// Transitorio previsto della scrittura di una tabella nel formato dato.
fn transitorio(tabella: &RecordBatch, formato: Formato) -> u64 {
    match formato {
        Formato::ArrowIpc | Formato::ArrowIpcStream => ipc::transitorio_scrittura(tabella),
        Formato::Parquet => parquet_io::transitorio_scrittura(tabella),
    }
}

/// Carica gli input, esegue il piano con il suo budget e scrive gli output.
///
/// Rende il resoconto dell'esecuzione.
///
/// # Errors
///
/// - `InvalidPlan`: percorsi d'uscita che non corrispondono agli output,
///   ripetuti o uguali a un ingresso; gli errori di validazione del piano;
/// - `Conflict`: una destinazione esiste senza sovrascrittura;
/// - `ResourceLimit`: input, esecuzione o scrittura oltre il budget;
/// - gli errori di lettura, esecuzione e scrittura, con il nome
///   dell'input o dell'output.
///
/// Un errore dopo che almeno un output è stato scritto ha effetto
/// `partial`: gli output precedenti restano alla destinazione.
pub fn esegui_da_file(
    piano: &Pipeline,
    ingressi: &[FileIngresso],
    uscite: &[FileUscita],
    opzioni: &OpzioniScrittura,
) -> Result<Report> {
    esegui_da_file_interrompibile(piano, ingressi, uscite, opzioni, &Interruzione::default())
        .map(|esito| esito.report)
}

/// Esito di [`esegui_da_file_interrompibile`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EsitoFile {
    /// Resoconto del runner.
    pub report: Report,
    /// Gli output scritti, nell'ordine del piano.
    pub uscite: Vec<UscitaScritta>,
}

/// Un output scritto: nome, forma e formato, senza il percorso.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UscitaScritta {
    /// Nome dell'output nel piano.
    pub nome: String,
    pub righe: u64,
    pub colonne: u64,
    /// Formato effettivo del file scritto.
    pub formato: Formato,
}

/// [`esegui_da_file`] con una scadenza e un segnale di annullamento
/// ([`Interruzione`]).
///
/// Oltre ai controlli del runner (prima di ogni passo e prima di consegnare
/// gli output) l'interruzione si controlla prima di leggere ogni input (fase
/// `read`), prima di scrivere ogni output (fase `write`) e dopo l'ultimo
/// (fase `finalize`). Mai durante la lettura o la scrittura di un file: un
/// file grande finisce anche oltre la scadenza, e l'errore arriva al
/// controllo successivo, anche quello dopo l'ultima scrittura (un
/// annullamento arrivato durante l'ultima scrittura non diventa un
/// successo). Un'interruzione dopo il primo output scritto ha effetto
/// `partial`, come ogni errore della scrittura; dopo l'ultimo `committed`.
///
/// # Errors
///
/// Quelli di [`esegui_da_file`]; `Cancelled` o `Timeout` a un controllo.
pub fn esegui_da_file_interrompibile(
    piano: &Pipeline,
    ingressi: &[FileIngresso],
    uscite: &[FileUscita],
    opzioni: &OpzioniScrittura,
    interruzione: &Interruzione,
) -> Result<EsitoFile> {
    esegui_ingressi(piano, da_file(ingressi), uscite, opzioni, interruzione)
}

/// Il budget del piano: `max_governed_memory_bytes` dei suoi limiti.
///
/// Il default se il piano non li dà. Lo usa anche chi importa tabelle in
/// memoria prima di chiamare queste funzioni (l'SDK Python), per fermarsi
/// allo stesso confine.
///
/// # Errors
///
/// Quelli di `LimitiParziali::applica` per limiti non validi.
pub fn budget_del_piano(piano: &Pipeline) -> Result<u64> {
    let limiti = piano.limits.as_ref().map_or_else(
        || Ok(Limits::default()),
        plenora_pipeline::LimitiParziali::applica,
    )?;
    Ok(limiti.max_governed_memory_bytes)
}

/// Carica gli input, valida il piano con i loro schemi e lo esegue.
fn carica_e_esegui(
    piano: &Pipeline,
    ingressi: Vec<Ingresso>,
    budget: u64,
    interruzione: &Interruzione,
) -> Result<Esito> {
    let caricate = carica_ingressi(ingressi, budget, interruzione)?;
    let schemi: Vec<(&str, _)> = caricate
        .iter()
        .map(|(nome, tabella)| (nome.as_str(), tabella.schema()))
        .collect();
    let validata = piano.validate(&schemi)?;
    drop(schemi);
    validata.run_interrompibile(caricate, interruzione)
}

/// [`esegui_da_file_interrompibile`] con input da file o in memoria
/// ([`Ingresso`]). Stessi controlli, stesso budget, stessi effetti.
///
/// # Errors
///
/// Quelli di [`esegui_da_file_interrompibile`].
pub fn esegui_ingressi(
    piano: &Pipeline,
    ingressi: Vec<Ingresso>,
    uscite: &[FileUscita],
    opzioni: &OpzioniScrittura,
    interruzione: &Interruzione,
) -> Result<EsitoFile> {
    verifica_percorsi(piano, &ingressi, uscite, *opzioni)?;
    let budget = budget_del_piano(piano)?;
    let esito = carica_e_esegui(piano, ingressi, budget, interruzione)?;

    let mut pubblicati = Vec::with_capacity(esito.outputs.len());
    scrivi_uscite(
        esito.outputs,
        uscite,
        *opzioni,
        budget,
        interruzione,
        &mut pubblicati,
    )
    .map_err(|errore| {
        if pubblicati.is_empty() {
            errore
        } else {
            errore.with_remote_effect(RemoteEffect::Partial)
        }
    })?;
    verifica_finale(interruzione, &pubblicati)?;
    Ok(EsitoFile {
        report: esito.report,
        uscite: pubblicati,
    })
}

/// Esegue un piano e ne rende gli output in memoria, senza scriverli:
/// caricamento, validazione, budget e interruzione come in
/// [`esegui_ingressi`]. Nessun effetto fuori dal processo.
///
/// # Errors
///
/// Quelli della lettura degli input da file, di [`Pipeline::validate`] e
/// di [`PipelineValidata::run_interrompibile`].
pub fn esegui_in_memoria(
    piano: &Pipeline,
    ingressi: Vec<Ingresso>,
    interruzione: &Interruzione,
) -> Result<Esito> {
    let budget = budget_del_piano(piano)?;
    carica_e_esegui(piano, ingressi, budget, interruzione)
}

/// L'interruzione dopo l'ultima scrittura (fase `finalize`): un annullamento
/// o una scadenza arrivati mentre si scriveva l'ultimo output non diventano
/// un successo. Gli output sono tutti alla destinazione, quindi l'effetto è
/// `committed` (con almeno un output; `none` senza) e il ritentativo non è
/// automatico: il chiamante sa che cosa è stato pubblicato e decide.
fn verifica_finale(interruzione: &Interruzione, pubblicati: &[UscitaScritta]) -> Result<()> {
    interruzione
        .verifica("dopo aver scritto gli output")
        .map_err(|errore| {
            let errore = errore.with_phase(ErrorPhase::Finalize);
            if pubblicati.is_empty() {
                errore
            } else {
                errore.with_remote_effect(RemoteEffect::Committed)
            }
        })
}

/// Valida un piano contro gli schemi dei suoi input letti da file, senza
/// eseguirlo.
///
/// Gli input si caricano come in [`esegui_da_file_interrompibile`] (stesso
/// budget, stessa lettura, stessi controlli dell'interruzione), poi si
/// liberano: la validazione guarda solo gli schemi. Leggere le tabelle
/// intere per averne lo schema è un costo dichiarato (docs/cli.md, «CLI
/// `plenora-data`»), limitato dal budget del piano.
///
/// # Errors
///
/// Gli errori di lettura degli input, con il loro nome, e quelli di
/// [`Pipeline::validate`]; `Cancelled` o `Timeout` a un controllo.
pub fn valida_da_file(
    piano: &Pipeline,
    ingressi: &[FileIngresso],
    interruzione: &Interruzione,
) -> Result<PipelineValidata> {
    valida_ingressi(piano, da_file(ingressi), interruzione)
}

/// [`valida_da_file`] con input da file o in memoria ([`Ingresso`]): anche
/// le tabelle in memoria passano dal budget del piano, come in `run`.
///
/// # Errors
///
/// Quelli di [`valida_da_file`].
pub fn valida_ingressi(
    piano: &Pipeline,
    ingressi: Vec<Ingresso>,
    interruzione: &Interruzione,
) -> Result<PipelineValidata> {
    let budget = budget_del_piano(piano)?;
    let caricate = carica_ingressi(ingressi, budget, interruzione)?;
    let schemi: Vec<(&str, _)> = caricate
        .iter()
        .map(|(nome, tabella)| (nome.as_str(), tabella.schema()))
        .collect();
    piano.validate(&schemi)
}

/// Carica gli input nell'ordine dato, ciascuno con il budget residuo; dopo
/// ogni lettura (o tabella in memoria) i byte vivi esatti devono stare nel
/// budget. L'interruzione si controlla prima di ogni input (fase `read`).
///
/// Le tabelle in memoria sono già residenti prima di qualunque lettura: si
/// riservano nel budget dall'inizio, in qualunque posizione stiano, così un
/// file letto prima di loro ha come residuo il budget meno quelle tabelle
/// (non il budget intero, che farebbe stare in memoria file e tabelle oltre
/// il budget prima del rifiuto). I byte vivi contano ogni allocazione una
/// volta: una tabella in memoria già caricata non conta due volte.
fn carica_ingressi(
    ingressi: Vec<Ingresso>,
    budget: u64,
    interruzione: &Interruzione,
) -> Result<Vec<(String, RecordBatch)>> {
    let mut residenti: Vec<RecordBatch> = Vec::new();
    for ingresso in &ingressi {
        if let Ingresso::Tabella { nome, tabella } = ingresso {
            residenti.push(tabella.clone());
            let riservati = byte_vivi(&residenti)?;
            if riservati > budget {
                return Err(con_nome(
                    &format!("input `{nome}`"),
                    oltre_il_budget(riservati, budget),
                )
                .with_phase(ErrorPhase::Read));
            }
        }
    }
    let mut caricate: Vec<(String, RecordBatch)> = Vec::with_capacity(ingressi.len());
    let vivi_con_residenti = |caricate: &[(String, RecordBatch)]| {
        byte_vivi(
            caricate
                .iter()
                .map(|(_, tabella)| tabella)
                .chain(residenti.iter()),
        )
    };
    for ingresso in ingressi {
        let contesto = format!("input `{}`", ingresso.nome());
        interruzione
            .verifica(&format!("prima di leggere l'{contesto}"))
            .map_err(|errore| errore.with_phase(ErrorPhase::Read))?;
        let (nome, letta) = match ingresso {
            Ingresso::File(file) => {
                let vivi = vivi_con_residenti(&caricate)?;
                let residuo = budget.saturating_sub(vivi);
                // Fase `read`: l'errore nasce leggendo l'input (la
                // derivazione per variante darebbe `write` a I/O e limiti).
                let letta = leggi_tabella(&file.percorso, file.formato, residuo)
                    .map_err(|errore| con_nome(&contesto, errore).with_phase(ErrorPhase::Read))?;
                (file.nome, letta)
            }
            Ingresso::Tabella { nome, tabella } => (nome, tabella),
        };
        caricate.push((nome, letta));
        let vivi = vivi_con_residenti(&caricate)?;
        if vivi > budget {
            return Err(
                con_nome(&contesto, oltre_il_budget(vivi, budget)).with_phase(ErrorPhase::Read)
            );
        }
    }
    Ok(caricate)
}

/// Scrive gli output nell'ordine del piano; `pubblicati` tiene quelli già
/// alla destinazione, anche quando la funzione poi fallisce.
fn scrivi_uscite(
    mut restanti: Vec<(String, RecordBatch)>,
    uscite: &[FileUscita],
    opzioni: OpzioniScrittura,
    budget: u64,
    interruzione: &Interruzione,
    pubblicati: &mut Vec<UscitaScritta>,
) -> Result<()> {
    let mut scritti: Vec<PathBuf> = Vec::new();
    while !restanti.is_empty() {
        let (nome, tabella) = restanti.remove(0);
        let contesto = format!("output `{nome}`");
        #[cfg(feature = "sonde-di-prova")]
        if !scritti.is_empty() {
            plenora_pipeline::sonda::chiama(
                plenora_pipeline::sonda::Punto::FraScritture,
                interruzione,
            )
            .map_err(|errore| errore.with_phase(ErrorPhase::Write))?;
        }
        interruzione
            .verifica(&format!("prima di scrivere l'{contesto}"))
            .map_err(|errore| errore.with_phase(ErrorPhase::Write))?;
        let destinazione = uscite
            .iter()
            .find(|uscita| uscita.nome == nome)
            .ok_or_else(|| PlenoraError::Internal(format!("{contesto}: percorso assente")))?;
        let formato = Formato::risolvi(
            destinazione.formato.or(opzioni.formato),
            &destinazione.percorso,
        )?;
        let vivi = byte_vivi(
            std::iter::once(&tabella).chain(restanti.iter().map(|(_, tabella)| tabella)),
        )?;
        let servono = vivi.saturating_add(transitorio(&tabella, formato));
        if servono > budget {
            return Err(con_nome(&contesto, oltre_il_budget(servono, budget)));
        }
        if let Some(canonico) = canonico_se_esiste(&destinazione.percorso)? {
            if scritti.contains(&canonico) {
                return Err(con_nome(
                    &contesto,
                    PlenoraError::Conflict(
                        "la destinazione e' il file di un altro output appena scritto".to_owned(),
                    ),
                ));
            }
        }
        let opzioni_uscita = OpzioniScrittura {
            formato: Some(formato),
            ..opzioni
        };
        scrivi_tabella(&tabella, &destinazione.percorso, &opzioni_uscita)
            .map_err(|errore| con_nome(&contesto, errore))?;
        let conta = |n: usize| {
            u64::try_from(n).map_err(|_| {
                PlenoraError::Internal(format!("{contesto}: conteggio non rappresentabile"))
            })
        };
        pubblicati.push(UscitaScritta {
            nome,
            righe: conta(tabella.num_rows())?,
            colonne: conta(tabella.num_columns())?,
            formato,
        });
        drop(tabella);
        scritti.push(std::fs::canonicalize(&destinazione.percorso)?);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    //! Effetto di un errore a metà della scrittura degli output.

    use std::sync::Arc;

    use plenora_core::arrow::array::{Int64Array, RecordBatch};
    use plenora_core::arrow::schema::{DataType, Field, Schema};
    use plenora_core::{ErrorCategory, RemoteEffect, RetryDisposition};
    use plenora_pipeline::Pipeline;

    use super::{esegui_da_file, FileIngresso, FileUscita};
    use crate::formato::OpzioniScrittura;
    use crate::{parquet_io, scrivi_tabella};

    /// Due output, `primo` e `secondo`, con un budget che basta per
    /// scrivere in Arrow IPC ma non per il transitorio fisso di Parquet.
    fn esegui(primo: &str, secondo: &str) -> (tempfile::TempDir, plenora_core::PlenoraError) {
        let dir = tempfile::tempdir().expect("directory temporanea");
        let tabella = RecordBatch::try_new(
            Arc::new(Schema::new(vec![Field::new("id", DataType::Int64, false)])),
            vec![Arc::new(Int64Array::from(vec![1, 2, 3]))],
        )
        .expect("tabella");
        let sorgente = dir.path().join("t.arrow");
        scrivi_tabella(&tabella, &sorgente, &OpzioniScrittura::default()).expect("sorgente");
        let budget = parquet_io::MARGINE_SCRITTURA / 2;
        let piano = Pipeline::from_json(&format!(
            r#"{{"version": 1, "inputs": ["t"], "limits": {{"max_governed_memory_bytes": {budget}}},
                "steps": [
                  {{"out": "a", "op": "table.filter", "in": ["t"],
                    "config": {{"column": "id", "operator": ">", "value": 0}}}},
                  {{"out": "b", "op": "table.filter", "in": ["t"],
                    "config": {{"column": "id", "operator": ">", "value": 1}}}}],
                "outputs": ["a", "b"]}}"#
        ))
        .expect("piano");
        let uscita = |nome: &str, file: &str| FileUscita {
            nome: nome.to_owned(),
            percorso: dir.path().join(file),
            formato: None,
        };
        let errore = esegui_da_file(
            &piano,
            &[FileIngresso {
                nome: "t".to_owned(),
                percorso: sorgente,
                formato: None,
            }],
            &[uscita("a", primo), uscita("b", secondo)],
            &OpzioniScrittura::default(),
        )
        .expect_err("Parquet oltre il budget");
        assert_eq!(errore.category(), ErrorCategory::ResourceLimit, "{errore}");
        (dir, errore)
    }

    #[test]
    fn errore_dopo_il_primo_output_ha_effetto_parziale() {
        let (dir, errore) = esegui("a.arrow", "b.parquet");
        assert!(
            dir.path().join("a.arrow").exists(),
            "il primo resta scritto"
        );
        assert!(!dir.path().join("b.parquet").exists());
        assert_eq!(errore.remote_effect(), RemoteEffect::Partial);
        // La causa (budget) non si ritenta mai: l'effetto non la rende
        // ritentabile; una causa ritentabile diventerebbe `requires_recovery`.
        assert_eq!(errore.retry_disposition(), RetryDisposition::Never);
        let pubblico = errore.public_projection();
        assert_eq!(pubblico.remote_effect(), RemoteEffect::Partial);
        assert_eq!(pubblico.category(), ErrorCategory::ResourceLimit);
        assert!(errore.to_string().contains("output `b`"), "{errore}");
    }

    #[test]
    fn errore_sul_primo_output_non_ha_effetto() {
        let (dir, errore) = esegui("a.parquet", "b.arrow");
        assert!(!dir.path().join("a.parquet").exists());
        assert!(!dir.path().join("b.arrow").exists());
        assert_eq!(errore.remote_effect(), RemoteEffect::None);
        assert_eq!(errore.retry_disposition(), RetryDisposition::Never);
    }

    /// Un'interruzione arrivata durante l'ultima scrittura si vede al
    /// controllo finale: `cancelled` o `timeout`, fase `finalize`, effetto
    /// `committed` con gli output alla destinazione, mai un successo.
    #[test]
    fn interruzione_dopo_l_ultima_scrittura() {
        use std::sync::atomic::AtomicBool;
        use std::time::Instant;

        use plenora_core::ErrorPhase;
        use plenora_pipeline::Interruzione;

        use super::{verifica_finale, UscitaScritta};
        use crate::Formato;

        let scritta = UscitaScritta {
            nome: "a".to_owned(),
            righe: 1,
            colonne: 1,
            formato: Formato::ArrowIpc,
        };
        let annullata = Interruzione {
            scadenza: None,
            annullamento: Some(Arc::new(AtomicBool::new(true))),
        };
        let scaduta = Interruzione {
            scadenza: Some(Instant::now()),
            annullamento: None,
        };
        for (interruzione, categoria) in [
            (&annullata, ErrorCategory::Cancelled),
            (&scaduta, ErrorCategory::Timeout),
        ] {
            let errore = verifica_finale(interruzione, std::slice::from_ref(&scritta))
                .expect_err("interrotta");
            assert_eq!(errore.category(), categoria);
            assert_eq!(errore.phase(), ErrorPhase::Finalize);
            assert_eq!(errore.remote_effect(), RemoteEffect::Committed);
            assert_eq!(
                errore.retry_disposition(),
                RetryDisposition::RequiresRecovery
            );
            let senza_output = verifica_finale(interruzione, &[]).expect_err("interrotta");
            assert_eq!(senza_output.remote_effect(), RemoteEffect::None);
        }
        assert!(verifica_finale(&Interruzione::default(), &[scritta]).is_ok());
    }
}
