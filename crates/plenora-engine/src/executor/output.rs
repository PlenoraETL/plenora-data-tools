//! L'uscita di un'esecuzione: lo stream dei batch, e la pubblicazione.
//!
//! [`Output`] e' un iteratore di batch piu' le metriche, e il punto in cui la
//! pubblicazione avviene in modo **atomico**: nessun output parziale, mai.
//!
//! Lo schema pubblicato e' quello che il contratto DICHIARA: i metadati geo
//! canonici sono riscritti qui, in un punto solo, e le dichiarazioni di CRS del
//! piano sostituiscono quelle della sorgente invece di sommarsi.
//! `PublishedButDurabilityUnconfirmed` dichiara i casi in cui il `fsync` della
//! directory non e' disponibile o non e' significativo.

use std::io::Write;
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;

use plenora_core::arrow::array::RecordBatch;
use plenora_core::arrow::ipc::writer::FileWriter;
use plenora_core::arrow::schema::SchemaRef;
use plenora_core::contract::DataContract;
use plenora_core::error::ErrorPhase;
use plenora_core::{PlenoraError, Result};

use crate::commit_token::CommitToken;
use crate::geo_transport::publish::{
    publish_with_profile, EsitoDellaPubblicazione, PublishProfile,
};
use crate::governor::{GovernedBatch, MemoryGovernor};
use crate::protocollo::messaggi::ConteggiDichiarati;

use super::input::BatchStream;
use super::metrics::ExecutionMetrics;
use super::state::ExecState;

// L'emissione dello schema canonico e' autorita' di `plenora-core`: la stessa
// che legge in ingresso, cosi' supervisore e worker non possono divergere.
pub(super) use plenora_core::contract::arrow_schema::arrow_schema_from_contract as canonical_output_schema;

/// Output di un'esecuzione: stream lazy dei batch finali + metriche.
///
/// Iterare l'`Output` guida l'esecuzione: l'input e' consumato
/// batch-per-batch (streaming reale). Non e' `Send` nella v1 seriale (parallelismo solo dove conviene).
pub struct Output {
    pub(super) contract: DataContract,
    /// Schema IPC emesso: quello del contratto piu' il blocco canonico
    /// R2.2/R2.5, calcolato una sola volta alla costruzione
    /// ([`canonical_output_schema`], fail-fast su divergenze R2.6).
    pub(super) schema: SchemaRef,
    pub(super) stream: BatchStream,
    pub(super) state: Rc<ExecState>,
    /// Stato terminale del consumo per iteratore: dopo che lo stream si e'
    /// esaurito, il controllo di salute finale corre **una sola volta** e il
    /// suo esito non si ripete. Senza, un iteratore riavviato produrrebbe lo
    /// stesso errore all'infinito, e chi lo consuma in un `for` lo vedrebbe
    /// come un ciclo che non finisce.
    pub(super) esaurito: bool,
}

impl Output {
    /// Schema Arrow dell'output: quello del contratto inferito in
    /// validazione arricchito del blocco canonico R2.2 e della versione
    /// R2.5 — lo stesso schema scritto nell'header IPC da
    /// [`Output::write_ipc_file_with_profile`].
    #[must_use]
    pub fn schema(&self) -> SchemaRef {
        self.schema.clone()
    }

    /// Contratto dell'arco di output del piano.
    ///
    /// Il suo `schema` e' quello inferito in validazione, SENZA il blocco
    /// canonico R2.2/R2.5: lo schema effettivamente emesso in IPC e'
    /// [`Output::schema`].
    #[must_use]
    pub const fn output_contract(&self) -> &DataContract {
        &self.contract
    }

    /// Identita' dell'esecuzione (errori-e-limiti.md, errori arricchiti): la stessa riportata negli
    /// errori `Execution`/`Cancelled` e nel lock del `TempStore`.
    #[must_use]
    pub fn execution_id(&self) -> &str {
        &self.state.execution_id
    }

    /// Snapshot delle metriche correnti (parziali finche' lo stream non e'
    /// esaurito).
    #[must_use]
    pub fn metrics(&self) -> ExecutionMetrics {
        self.state.metrics()
    }

    /// Drena lo stream raccogliendo tutti i batch finali.
    ///
    /// Il wrapper governato si spacca al confine pubblico: il lease di ogni
    /// batch e' rilasciato alla consegna (la memoria passa al chiamante).
    ///
    /// # Errors
    ///
    /// Propaga il primo errore dello stream (nessun output parziale viene
    /// restituito).
    pub fn collect_batches(self) -> Result<(Vec<RecordBatch>, ExecutionMetrics)> {
        let batches = self
            .stream
            .map(|item| item.map(GovernedBatch::into_batch))
            .collect::<Result<Vec<_>>>()?;
        // Controllo di salute PRIMA di dichiarare conclusa l'esecuzione: una
        // corruzione della contabilita' rilevata dentro un `Drop` non puo'
        // propagare un errore da li', e senza questo l'ultimo output verrebbe
        // consegnato da un governor che ha gia' perso il conto.
        self.state.governor.verifica_salute("output")?;
        // Stesso criterio per il temp store: consegnare l'output di
        // un'esecuzione il cui lock e' fermo da oltre la tolleranza
        // significherebbe dichiararla riuscita mentre la sua directory e'
        // gia' raccoglibile da un altro avvio.
        self.state.verifica_heartbeat()?;
        Ok((batches, self.state.metrics()))
    }

    /// Drena lo stream conservando i wrapper governati (lease + sequenza).
    ///
    /// Seam interno per i test del governor (architettura.md#determinismo e
    /// #memoria): nessun consumatore pubblico riordina per `BatchSequence`,
    /// quindi la sequenza logica si osserva solo da qui.
    #[cfg(test)]
    pub(crate) fn collect_governed(self) -> Result<(Vec<GovernedBatch>, ExecutionMetrics)> {
        let batches = self.stream.collect::<Result<Vec<_>>>()?;
        Ok((batches, self.state.metrics()))
    }

    /// Scrive l'output in Arrow IPC file format con publish atomico
    /// (D22, errori-e-limiti.md#publish-e-cleanup).
    ///
    /// Tempfile nella directory di destinazione, persist no-clobber solo a
    /// stream completato con successo. L'header IPC porta lo schema di
    /// [`Output::schema`]: contratto, blocco canonico R2.2 per ogni colonna
    /// geometrica, versione R2.5, chiavi `GeoArrow` legacy (R2.6).
    ///
    /// L'esito si rende sempre: una forma senza esito scarterebbe in silenzio
    /// la durabilita' non confermata e il temporaneo rimasto. Chi non lo vuole
    /// lo lascia cadere a vista.
    ///
    /// # Errors
    ///
    /// Propaga errori di stream e di I/O; [`PlenoraError::Conflict`] se la
    /// destinazione esiste gia'; `PlenoraError::InvalidPlan` se la directory
    /// non esiste; `PlenoraError::Unsupported` se il filesystem di
    /// destinazione e' di rete o non identificabile
    /// (errori-e-limiti.md#publish-e-cleanup).
    pub fn write_ipc_file_with_profile(
        self,
        path: &Path,
        profile: PublishProfile,
    ) -> Result<(ExecutionMetrics, EsitoDellaPubblicazione)> {
        let schema = self.schema.clone();
        let governor = self.state.governor.clone();
        let stato = Rc::clone(&self.state);
        let mut stream = self.stream;
        let (_conteggi, esito) = publish_with_profile(path, profile, move |writer| {
            // Nessun token: il percorso in-process non ha un tentativo da
            // identificare. Nessun osservatore: non c'e' un supervisore che
            // aspetti il progresso, e i conteggi che il ciclo rende comunque
            // qui non hanno un destinatario.
            scrivi_e_conta(
                writer,
                &schema,
                &mut stream,
                None,
                &mut |_| Ok(()),
                &governor,
                &stato,
            )
        })?;
        Ok((self.state.metrics(), esito))
    }

    /// Come [`Output::write_ipc_file_with_profile`], ma **solo per i casi**:
    /// pubblica col profilo atomico e lascia cadere le avvertenze.
    ///
    /// Esiste solo sotto `cfg(test)` perche' scarta cio' che chi pubblica deve
    /// sapere; nei casi che giudicano altro ignorarle e' legittimo, e il nome
    /// lo dichiara.
    #[cfg(test)]
    pub(crate) fn write_ipc_file_ignorando_le_avvertenze(
        self,
        path: &Path,
    ) -> Result<ExecutionMetrics> {
        self.write_ipc_file_with_profile(path, PublishProfile::Atomic)
            .map(|(metriche, _avvertenze)| metriche)
    }

    /// Scrive l'artefatto di un'esecuzione **isolata** sul solo percorso
    /// temporaneo, e ne rende i conteggi.
    ///
    /// Non pubblica e non conosce la destinazione finale: la pubblicazione e'
    /// il passo 9 di `isolamento.md`, di chi ha osservato la verifica, non di
    /// chi ha prodotto i byte. Per questo non usa [`publish_with_profile`], che
    /// rinomina; il no-clobber resta, con l'apertura in `create_new`.
    ///
    /// Token e osservatore non sono `Option`: un artefatto senza token non e'
    /// attribuibile al tentativo e il passo 8-bis lo rifiuterebbe.
    ///
    /// # Errors
    ///
    /// [`PlenoraError::InvalidPlan`] se il percorso temporaneo esiste gia' o la
    /// sua directory no; propaga gli errori dello stream, della scrittura IPC,
    /// dell'osservatore e dei due controlli terminali.
    pub(crate) fn scrivi_artefatto_isolato(
        self,
        temporaneo: &Path,
        token: &CommitToken,
        osservatore: &mut dyn FnMut(ConteggiDichiarati) -> Result<()>,
    ) -> Result<(ExecutionMetrics, ConteggiDichiarati)> {
        let schema = self.schema.clone();
        let governor = self.state.governor.clone();
        let stato = Rc::clone(&self.state);
        let mut stream = self.stream;
        let mut file = std::fs::File::options()
            .write(true)
            // No-clobber al confine: `create_new` fallisce se il percorso
            // esiste, e la decisione non passa da un `exists()` seguito da una
            // `open()` — fra i due ci sarebbe una finestra in cui qualcuno crea
            // il file, e la seconda chiamata lo sovrascriverebbe.
            .create_new(true)
            .open(temporaneo)
            .map_err(|causa| non_apribile(temporaneo, &causa))?;
        let conteggi = scrivi_e_conta(
            &mut file,
            &schema,
            &mut stream,
            Some(token),
            osservatore,
            &governor,
            &stato,
        )?;
        Ok((self.state.metrics(), conteggi))
    }
}

/// I generi d'errore che parlano dell'**incarico**, con la fase e cio' che si
/// dice a chi legge.
///
/// Riguardano la destinazione (percorso occupato, directory assente) o la
/// forma del percorso (rifiutata dal sistema, componente intermedio che e' un
/// file, directory dove serve un file): il protocollo limita la lunghezza del
/// percorso, non la forma. Tutto il resto ricade su `Io`.
///
/// Quale genere arrivi lo decide il sistema: una directory esistente da'
/// `AlreadyExists` su Unix e `PermissionDenied` su Windows, che qui non c'e' e
/// resta `Io`, la diagnosi conservativa. Deviazione dichiarata in
/// errori-e-limiti.md#lapertura-dellartefatto-temporaneo-quali-generi-sono-dellincarico,
/// e due casi pretendono il genere reale su ciascuna piattaforma.
///
/// Fasi: `Probe` quando il difetto si vede guardando il percorso, `Commit`
/// quando si vede solo provando a occupare la destinazione.
const GENERI_DELL_INCARICO: &[(std::io::ErrorKind, ErrorPhase, &str)] = &[
    (
        std::io::ErrorKind::AlreadyExists,
        ErrorPhase::Commit,
        "artefatto temporaneo gia' esistente, e non lo si sovrascrive",
    ),
    (
        std::io::ErrorKind::NotFound,
        ErrorPhase::Probe,
        "la directory dell'artefatto temporaneo non esiste",
    ),
    (
        std::io::ErrorKind::InvalidInput,
        ErrorPhase::Probe,
        "il percorso dell'artefatto temporaneo non e' un percorso che il sistema accetti",
    ),
    (
        std::io::ErrorKind::NotADirectory,
        ErrorPhase::Probe,
        "un componente del percorso dell'artefatto temporaneo non e' una directory",
    ),
    (
        std::io::ErrorKind::IsADirectory,
        ErrorPhase::Commit,
        "il percorso dell'artefatto temporaneo nomina una directory, non un file",
    ),
];

/// Perche' l'artefatto temporaneo non si e' aperto.
///
/// I generi di [`GENERI_DELL_INCARICO`] parlano di cio' che il chiamante ha
/// chiesto e diventano `InvalidPlan`; il resto parla dell'ambiente (permessi,
/// disco, sola lettura) e resta `Io`. La distinzione passa dal genere, non dal
/// testo di `io::Error`, che dipende da piattaforma e lingua.
fn non_apribile(temporaneo: &Path, causa: &std::io::Error) -> PlenoraError {
    let dove = temporaneo.display();
    for (genere, fase, detto) in GENERI_DELL_INCARICO {
        if causa.kind() == *genere {
            return PlenoraError::InvalidPlan(format!("{detto}: {dove}")).with_phase(*fase);
        }
    }
    // L'ambiente che risponde di no. Resta `Io`, che porta con se' il genere
    // vero, e la fase dice **quando**: si stava aprendo per scrivere.
    PlenoraError::Io(std::io::Error::new(
        causa.kind(),
        format!("artefatto temporaneo non apribile in esclusiva: {dove}: {causa}"),
    ))
    .with_phase(ErrorPhase::Write)
}

/// Il ciclo che scrive un artefatto: **l'unica autorita'** che lo fa.
///
/// Rivestimento dello schema, scrittura dei batch, conteggi, footer prima di
/// `finish` e controlli terminali devono valere insieme: un secondo ciclo
/// sarebbe una seconda autorita' sul formato. I due chiamanti differiscono
/// solo per destinazione dei byte, token e osservatore.
///
/// I conteggi sono esatti e cumulativi, con aritmetica **controllata**: ne'
/// `wrapping` ne' `saturating`. [`crate::verifica::conta_in_streaming`] applica
/// lo stesso contratto in modo indipendente, e il passo 8 confronta i due.
///
/// L'osservatore riceve lo snapshot dopo ogni batch scritto con successo, e il
/// suo errore interrompe la scrittura.
///
/// # Errors
///
/// Propaga gli errori dello stream, del rivestimento, della scrittura IPC,
/// dell'osservatore e dei due controlli terminali.
fn scrivi_e_conta(
    destinazione: &mut dyn Write,
    schema: &SchemaRef,
    stream: &mut BatchStream,
    // L'unico chiamante che puo' passare `None` e' quello in-process, che non
    // ha un tentativo da identificare. Il fratello isolato non ha questa
    // scelta: il suo parametro non e' un `Option`.
    token: Option<&CommitToken>,
    osservatore: &mut dyn FnMut(ConteggiDichiarati) -> Result<()>,
    governor: &MemoryGovernor,
    stato: &ExecState,
) -> Result<ConteggiDichiarati> {
    let mut ipc = FileWriter::try_new(destinazione, schema)?;
    // Cache della decisione di rivestimento per Arc di schema: i batch di uno
    // stream condividono lo stesso Arc (lo schema del contratto del kernel),
    // quindi il confronto profondo di `Schema` (campi + mappe metadata) si
    // esegue solo al primo batch di ogni schema distinto (hot path minimale:
    // lavoro hoistable fuori dal loop). Il rivestimento resta fail-closed:
    // `try_new` rivalida ogni batch rivestito.
    let mut schema_decision: Option<(SchemaRef, bool)> = None;
    let mut conteggi = ConteggiDichiarati { righe: 0, batch: 0 };
    for item in &mut *stream {
        let batch = item?.into_batch();
        // Lo schema emesso (blocco canonico R2.2/R2.5 fuso dal contratto) puo'
        // differire da quello del batch solo nei metadati: rivestimento a costo
        // zero sui buffer (colonne condivise via Arc), fail-closed su qualunque
        // altra divergenza (tipo, numero di colonne).
        let batch_schema = batch.schema();
        let rewrap = match &schema_decision {
            Some((seen, decision)) if Arc::ptr_eq(seen, &batch_schema) => *decision,
            _ => {
                let decision = batch_schema != *schema;
                schema_decision = Some((batch_schema, decision));
                decision
            }
        };
        let batch = if rewrap {
            // Rivestimento dello schema prima della pubblicazione: le colonne
            // sono quelle dell'input, quindi possono essere zero e la
            // cardinalita' va dichiarata.
            let righe = batch.num_rows();
            plenora_core::batch_with_rows(schema.clone(), batch.columns().to_vec(), righe)?
        } else {
            batch
        };
        ipc.write(&batch)?;
        // I conteggi si aggiornano **dopo** la scrittura, e per la stessa
        // ragione per cui l'osservatore parla dopo: contano cio' che
        // nell'artefatto c'e', non cio' che si stava per scriverci.
        conteggi = avanza(conteggi, batch.num_rows())?;
        osservatore(conteggi)?;
    }
    // Il `commit_token` va scritto **prima** di `finish`: dopo, il footer e'
    // gia' stato emesso e la chiamata non avrebbe effetto, in silenzio. Con
    // `None` non si scrive nulla, quindi i byte di un artefatto in-process
    // restano identici a quelli di prima che questo ciclo fosse condiviso.
    crate::commit_footer::scrivi_commit_token(&mut ipc, token);
    ipc.finish()?;
    // Controllo di salute PRIMA che l'artefatto conti come scritto: una
    // corruzione della contabilita' rilevata dentro un `Drop` non puo'
    // propagare un errore da li'. Per il percorso pubblico questo e' anche
    // prima del publish atomico, che e' irreversibile: fallire ora significa
    // non pubblicare nulla. Per quello isolato e' prima dell'`Esito`: fallire
    // ora significa non dichiarare un successo.
    governor.verifica_salute("output")?;
    // Stesso cancello per il temp store: dichiarare riuscita un'esecuzione il
    // cui lock e' fermo da oltre la tolleranza significherebbe dichiarare
    // riuscita un'esecuzione la cui directory e' gia' raccoglibile.
    stato.verifica_heartbeat()?;
    Ok(conteggi)
}

/// Un batch in piu', e le sue righe, con l'aritmetica controllata.
fn avanza(finora: ConteggiDichiarati, righe_del_batch: usize) -> Result<ConteggiDichiarati> {
    let di_questo = u64::try_from(righe_del_batch).map_err(|_| {
        PlenoraError::Internal(
            "scrittura dell'artefatto: righe di un batch fuori intervallo".to_owned(),
        )
    })?;
    Ok(ConteggiDichiarati {
        righe: finora.righe.checked_add(di_questo).ok_or_else(|| {
            PlenoraError::Internal(
                "scrittura dell'artefatto: somma delle righe fuori intervallo".to_owned(),
            )
        })?,
        batch: finora.batch.checked_add(1).ok_or_else(|| {
            PlenoraError::Internal(
                "scrittura dell'artefatto: numero di batch fuori intervallo".to_owned(),
            )
        })?,
    })
}

impl Iterator for Output {
    type Item = Result<RecordBatch>;

    /// Consumo batch per batch dell'output.
    ///
    /// A stream esaurito corre il **controllo di salute terminale**: se la
    /// contabilita' del governor e' stata marcata incoerente (per esempio nel
    /// `Drop` dell'ultimo lease, dove un errore non si propaga) l'iteratore
    /// produce **una volta** `Some(Err(...))` e poi `None`. Senza, un `for`
    /// sull'output renderebbe la corruzione un successo silenzioso.
    fn next(&mut self) -> Option<Self::Item> {
        if self.esaurito {
            return None;
        }
        if let Some(item) = self.stream.next() {
            return Some(item.map(GovernedBatch::into_batch));
        }
        self.esaurito = true;
        // Terminale dell'iteratore: come il cancello di consegna e quello di
        // publish, riporta sia una contabilita' corrotta sia un heartbeat
        // fermo da troppo tempo. Chiudere lo stream in silenzio su un lock
        // stantio sarebbe dichiarare riuscita un'esecuzione la cui directory
        // e' gia' raccoglibile.
        self.state
            .governor
            .verifica_salute("output")
            .err()
            .or_else(|| self.state.verifica_heartbeat().err())
            .map(Err)
    }
}

// ---------------------------------------------------------------------------
// execute
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;
