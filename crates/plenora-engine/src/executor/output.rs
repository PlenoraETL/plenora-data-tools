//! L'uscita di un'esecuzione: lo stream dei batch, e la pubblicazione.
//!
//! [`Output`] e' un iteratore di batch piu' le metriche. Non e' solo un
//! contenitore: e' il punto in cui il contratto dichiarato diventa uno schema
//! Arrow vero, con i metadati canonici della geometria, e in cui la
//! pubblicazione avviene in modo **atomico** — nessun output parziale, mai.
//!
//! # Il rivestimento dello schema
//!
//! Lo schema pubblicato non e' quello che i kernel producono: e' quello che
//! il contratto DICHIARA. I metadati geo canonici vengono riscritti qui, in un
//! punto solo, e le dichiarazioni di CRS gia' decise dal piano vengono
//! sostituite invece di sommarsi a quelle della sorgente. Farlo altrove
//! significherebbe avere due verita' sullo schema di uscita.
//!
//! # Perche' la durabilita' e' un esito, non un booleano
//!
//! `PublishedButDurabilityUnconfirmed` esiste perche' su alcune piattaforme il
//! `fsync` della directory non e' disponibile o non e' significativo: il file
//! c'e', ma nessuno puo' promettere che sopravviva a un'interruzione
//! dell'alimentazione. Dirlo e' diverso dal tacerlo.

use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;

use plenora_core::arrow::array::RecordBatch;
use plenora_core::arrow::ipc::writer::FileWriter;
use plenora_core::arrow::schema::{Schema, SchemaRef};
use plenora_core::contract::{ContractCrs, DataContract};
use plenora_core::{PlenoraError, Result};
use plenora_kernels_geo::arrow_adapter::{
    canonical_geometry_metadata, canonical_schema_version_metadata, strip_decided_crs_declarations,
    GeometryMetadataDetails, PLENORA_GEOMETRY_AXIS_ORDER_KEY, PLENORA_GEOMETRY_CRS_RESOLUTION_KEY,
    PLENORA_GEOMETRY_SRID_KEY,
};

use crate::geo_transport::publish::{publish_with_profile, PublishOutcome, PublishProfile};
use crate::governor::GovernedBatch;

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
    /// R2.2/R2.5 (milestone C), calcolato una sola volta alla costruzione
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
    /// R2.5 (milestone C) — lo stesso schema scritto nell'header IPC da
    /// [`Output::write_ipc_file`].
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
    /// Seam interno per i test del governor (architettura.md#determinismo e #memoria): in questa
    /// milestone nessun consumatore pubblico riordina per `BatchSequence`.
    #[cfg(test)]
    pub(crate) fn collect_governed(self) -> Result<(Vec<GovernedBatch>, ExecutionMetrics)> {
        let batches = self.stream.collect::<Result<Vec<_>>>()?;
        Ok((batches, self.state.metrics()))
    }

    /// Scrive l'output in Arrow IPC file format con publish atomico
    /// (decisione D22/errori-e-limiti.md#publish-e-cleanup): tempfile nella directory di destinazione,
    /// persist no-clobber solo a stream completato con successo — nessun
    /// output parziale e' mai visibile. Profilo [`PublishProfile::Atomic`]:
    /// wrapper su [`Output::write_ipc_file_with_profile`], l'esito tipizzato
    /// (sempre `Published` a publish riuscito) e' scartato.
    ///
    /// L'header IPC porta lo schema di [`Output::schema`]: quello del
    /// contratto piu' il blocco canonico R2.2 per ogni colonna geometrica e
    /// la versione R2.5 nei metadati dello schema (milestone C); le chiavi
    /// `GeoArrow` legacy restano (coesistenza coerente, R2.6).
    ///
    /// # Errors
    ///
    /// Propaga errori di stream e di I/O; `PlenoraError::InvalidPlan` se la
    /// destinazione esiste gia' o la directory non esiste;
    /// `PlenoraError::Unsupported` se il filesystem di
    /// destinazione e' di rete o non identificabile (errori-e-limiti.md#publish-e-cleanup).
    pub fn write_ipc_file(self, path: &Path) -> Result<ExecutionMetrics> {
        let (metrics, _outcome) = self.write_ipc_file_with_profile(path, PublishProfile::Atomic)?;
        Ok(metrics)
    }

    /// Come [`Output::write_ipc_file`], ma con profilo di publish
    /// selezionabile (errori-e-limiti.md#publish-e-cleanup) ed esito tipizzato restituito al chiamante:
    /// [`PublishOutcome::PublishedButDurabilityUnconfirmed`] se il publish e'
    /// riuscito ma la durabilita' non e' confermata (es. `fsync` di directory
    /// non supportato dalla piattaforma).
    ///
    /// # Errors
    ///
    /// Come [`Output::write_ipc_file`].
    pub fn write_ipc_file_with_profile(
        self,
        path: &Path,
        profile: PublishProfile,
    ) -> Result<(ExecutionMetrics, PublishOutcome)> {
        let schema = self.schema.clone();
        let governor = self.state.governor.clone();
        let stato_publish = Rc::clone(&self.state);
        let mut stream = self.stream;
        let ((), outcome) = publish_with_profile(path, profile, move |writer| {
            let mut ipc = FileWriter::try_new(writer, &schema)?;
            // Cache della decisione di rivestimento per Arc di schema: i
            // batch di uno stream condividono lo stesso Arc (lo schema del
            // contratto del kernel), quindi il confronto profondo di
            // `Schema` (campi + mappe metadata) si esegue solo al primo
            // batch di ogni schema distinto (hot path minimale: lavoro hoistable fuori
            // dal loop). Il rivestimento resta fail-closed: `try_new`
            // rivalida ogni batch rivestito.
            let mut schema_decision: Option<(SchemaRef, bool)> = None;
            for item in &mut stream {
                let batch = item?.into_batch();
                // Lo schema emesso (blocco canonico R2.2/R2.5 fuso dal
                // contratto) puo' differire da quello del batch solo nei
                // metadati: rivestimento a costo zero sui buffer (colonne
                // condivise via Arc), fail-closed su qualunque altra
                // divergenza (tipo, numero di colonne).
                let batch_schema = batch.schema();
                let rewrap = match &schema_decision {
                    Some((seen, decision)) if Arc::ptr_eq(seen, &batch_schema) => *decision,
                    _ => {
                        let decision = batch_schema != schema;
                        schema_decision = Some((batch_schema, decision));
                        decision
                    }
                };
                let batch = if rewrap {
                    // Rivestimento dello schema prima della pubblicazione: le
                    // colonne sono quelle dell'input, quindi possono essere
                    // zero e la cardinalita' va dichiarata.
                    let righe = batch.num_rows();
                    plenora_core::batch_with_rows(schema.clone(), batch.columns().to_vec(), righe)?
                } else {
                    batch
                };
                ipc.write(&batch)?;
            }
            ipc.finish()?;
            // Controllo di salute PRIMA del publish atomico: una corruzione
            // della contabilita' rilevata dentro un `Drop` non puo' propagare
            // un errore da li', e il publish e' irreversibile. Qui il file
            // temporaneo non e' ancora stato reso visibile (errori-e-limiti.md#publish-e-cleanup), quindi
            // fallire ora significa non pubblicare nulla.
            governor.verifica_salute("output")?;
            // Stesso cancello per il temp store: il publish e' irreversibile,
            // e pubblicare mentre il lock e' fermo da oltre la tolleranza
            // significherebbe dichiarare riuscita un'esecuzione la cui
            // directory e' gia' raccoglibile.
            stato_publish.verifica_heartbeat()?;
            Ok(())
        })?;
        Ok((self.state.metrics(), outcome))
    }
}

impl Iterator for Output {
    type Item = Result<RecordBatch>;

    /// Consumo batch per batch dell'output.
    ///
    /// A stream esaurito corre il **controllo di salute terminale**: se la
    /// contabilita' del governor e' stata marcata incoerente — cosa che puo'
    /// accadere dentro il `Drop` dell'ultimo lease, dove un errore non puo'
    /// essere propagato — l'iteratore produce **una volta** `Some(Err(...))` e
    /// poi `None`.
    ///
    /// Senza questo controllo chi consuma con `for batch in output` non
    /// passerebbe ne' da [`Output::collect_batches`] ne' dal publish atomico,
    /// e una corruzione rilevata all'ultimo rilascio diventerebbe un successo
    /// silenzioso: lo stream finirebbe e basta.
    ///
    /// L'errore e' emesso una sola volta (`esaurito`): ripeterlo a ogni
    /// chiamata trasformerebbe un `for` in un ciclo che non termina.
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
