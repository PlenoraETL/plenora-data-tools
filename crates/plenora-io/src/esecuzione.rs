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

use std::path::{Path, PathBuf};

use plenora_core::arrow::array::RecordBatch;
use plenora_core::limits::Limits;
use plenora_core::memoria::byte_vivi;
use plenora_core::{PlenoraError, RemoteEffect, Result};
use plenora_pipeline::{Pipeline, Report};

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

fn assoluto(percorso: &Path) -> Result<PathBuf> {
    Ok(std::path::absolute(percorso)?)
}

/// Controlli sui percorsi, prima di leggere qualunque dato.
fn verifica_percorsi(
    piano: &Pipeline,
    ingressi: &[FileIngresso],
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
    for ingresso in ingressi {
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
        Formato::ArrowIpc => ipc::transitorio_scrittura(tabella),
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
    verifica_percorsi(piano, ingressi, uscite, *opzioni)?;
    let limiti = piano.limits.as_ref().map_or_else(
        || Ok(Limits::default()),
        plenora_pipeline::LimitiParziali::applica,
    )?;
    let budget = limiti.max_governed_memory_bytes;

    let mut caricate: Vec<(String, RecordBatch)> = Vec::with_capacity(ingressi.len());
    for ingresso in ingressi {
        let contesto = format!("input `{}`", ingresso.nome);
        let vivi = byte_vivi(caricate.iter().map(|(_, tabella)| tabella))?;
        let residuo = budget.saturating_sub(vivi);
        let letta = leggi_tabella(&ingresso.percorso, ingresso.formato, residuo)
            .map_err(|errore| con_nome(&contesto, errore))?;
        caricate.push((ingresso.nome.clone(), letta));
        let vivi = byte_vivi(caricate.iter().map(|(_, tabella)| tabella))?;
        if vivi > budget {
            return Err(con_nome(&contesto, oltre_il_budget(vivi, budget)));
        }
    }

    let schemi: Vec<(&str, _)> = caricate
        .iter()
        .map(|(nome, tabella)| (nome.as_str(), tabella.schema()))
        .collect();
    let validata = piano.validate(&schemi)?;
    drop(schemi);
    let esito = validata.run(caricate)?;

    let mut pubblicati = 0_usize;
    scrivi_uscite(esito.outputs, uscite, *opzioni, budget, &mut pubblicati).map_err(|errore| {
        if pubblicati == 0 {
            errore
        } else {
            errore.with_remote_effect(RemoteEffect::Partial)
        }
    })?;
    Ok(esito.report)
}

/// Scrive gli output nell'ordine del piano; `pubblicati` conta quelli già
/// alla destinazione, anche quando la funzione poi fallisce.
fn scrivi_uscite(
    mut restanti: Vec<(String, RecordBatch)>,
    uscite: &[FileUscita],
    opzioni: OpzioniScrittura,
    budget: u64,
    pubblicati: &mut usize,
) -> Result<()> {
    let mut scritti: Vec<PathBuf> = Vec::new();
    while !restanti.is_empty() {
        let (nome, tabella) = restanti.remove(0);
        let contesto = format!("output `{nome}`");
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
        *pubblicati += 1;
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
}
