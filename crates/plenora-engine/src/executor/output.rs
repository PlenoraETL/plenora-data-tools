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

/// Schema IPC dell'output: lo schema del contratto arricchito del blocco
/// canonico R2.2 per ogni colonna geometrica e della versione di protocollo
/// R2.5 nei metadati dello schema (milestone C — post-processo CENTRALE: i
/// campi continuano a essere costruiti dagli `analyze_contract` con le sole
/// chiavi `GeoArrow` legacy, che RESTANO — R2.6 ammette la coesistenza se
/// coerente; il cablaggio dei singoli analyze e' milestone successiva).
///
/// Regole:
///
/// - per ogni `GeometryColumnContract` del contratto, le chiavi di
///   [`canonical_geometry_metadata`] sono fuse nel metadata del campo
///   omonimo. `GeometryMetadataDetails::default()` (nessun dettaglio
///   opzionale modellato dal contratto) attiva la cascata di completamento
///   DELL'ASSENTE (R2.7, piano-v5.md#contratti-di-input emendamento 2026-07-31): normalmente
///   `axis_order` e `srid` sono DEDOTTI dalla definizione canonica d'autorita'
///   ([`ResolvedCrs::authority_axis_order`]/[`ResolvedCrs::authority_srid`]
///   — lo stesso oggetto con cui il kernel ha operato; deduzione da
///   autorita', non invenzione) e `axis_order` vale `unknown` solo quando
///   neanche la definizione determina gli assi — `unknown` resta l'onesta',
///   non il default pigro (R5.2 riguarda le chiavi opzionali, che restano
///   assenti). `geo.reproject` fa eccezione esplicita per `axis_order`: lo
///   inserisce gia' nell'output dell'analisi con l'ordine GIS normalizzato
///   realmente prodotto dal backend (`lon_lat`/`easting_northing`), distinto
///   dall'ordine nativo dell'autorita'; lo `srid` resta d'autorita';
/// - R2.6: una chiave canonica gia' presente sul campo (o la versione sullo
///   schema) con valore DIVERSO da quello imposto dal contratto e' un
///   errore, mai una sovrascrittura silenziosa; valore uguale e'
///   idempotente. Le chiavi che l'operazione RISCRIVE di mestiere (piano-v5.md#contratti-di-input,
///   decisione 8 — il blocco CRS per `reproject`, `types`/
///   `types_declaration` per le trasformazioni che cambiano il tipo
///   geometrico) non passano MAI di qui come divergenze: la sostituzione
///   avviene a monte, nel contratto prodotto dall'analisi
///   (`analyze_reproject` / `with_geometry_types` rimuovono le chiavi
///   ereditate), e qui sono ri-emesse dal contratto come ogni altra. Per
///   tutte le chiavi non riscritte il guard resta intatto. Eccezioni
///   dichiarate: `axis_order` e `srid` sono
///   per completamento dell'assente (R2.7, mai arbitrato) — una chiave
///   di lineage PRESENTE vince sempre, qualunque sia il valore emesso dal
///   contratto (anche un valore dedotto dall'autorita': la deduzione non
///   deve mai trasformarsi in conflitto R2.6 su un passthrough; prima
///   dell'emendamento 2026-07-31 lo skip copriva solo `axis_order =
///   unknown`, l'unico valore emesso possibile allora);
///   `crs_resolution = resolved` preesistente e' corretta in
///   `declared_unresolved` quando il contratto porta un'incoerenza rilevata
///   (R4.6.4: mai silenziarla propagando la dichiarazione `resolved` che
///   l'incoerenza smentisce — unica sovrascrittura ammessa, in una sola
///   direzione);
/// - R2.5: `plenora.contract.version` e' aggiunta ai metadati dello schema
///   SOLO se almeno un campo porta chiavi canoniche (sempre vero quando il
///   contratto dichiara geometrie: [`canonical_geometry_metadata`] emette
///   comunque `dimensions`/`crs_resolution`/`field_id`); uno schema senza
///   geometrie e' restituito invariato;
/// - una colonna geometrica del contratto assente dallo schema e' un errore
///   (invariante violata a monte: fail-closed, mai silenziosa).
///
/// # Errors
///
/// `PlenoraError::InvalidPlan` per chiave canonica preesistente divergente
/// (R2.6) o colonna geometrica del contratto assente nello schema.
pub(super) fn canonical_output_schema(contract: &DataContract) -> Result<SchemaRef> {
    if contract.geometries.is_empty() {
        return Ok(contract.schema.clone());
    }
    let mut matched = 0_usize;
    let mut fields = Vec::with_capacity(contract.schema.fields().len());
    for field in contract.schema.fields() {
        let Some(geometry) = contract
            .geometries
            .iter()
            .find(|geometry| geometry.name.as_str() == field.name().as_str())
        else {
            fields.push(field.as_ref().clone());
            continue;
        };
        matched += 1;
        let canonical = canonical_geometry_metadata(geometry, &GeometryMetadataDetails::default());
        let mut metadata = field.metadata().clone();
        // R4.6.3: con un CRS deciso dal piano (`ResolvedByDecision`) le
        // dichiarazioni della sorgente sono SOSTITUITE, non fuse — il
        // blocco canonico ri-emette il CRS deciso e la lineage non deve
        // riproporre il conflitto a valle. Lo schema del contratto di
        // input resta intatto (il check fail-closed input/contratto
        // confronta i campi, metadati inclusi): la sostituzione vive solo
        // qui, all'emissione.
        if matches!(geometry.crs, ContractCrs::ResolvedByDecision(_)) {
            strip_decided_crs_declarations(&mut metadata);
        }
        for (key, value) in &canonical {
            match metadata.get(key) {
                Some(existing) if existing != value => {
                    // `axis_order` e `srid` sono per completamento DELL'ASSENTE
                    // (R2.7), mai per arbitrato: una chiave di lineage
                    // PRESENTE vince sempre, qualunque sia il valore emesso —
                    // anche un valore dedotto dalla definizione d'autorita'
                    // (piano-v5.md#contratti-di-input, emendamento 2026-07-31): la deduzione riempie
                    // solo le chiavi assenti e non deve mai trasformarsi in
                    // un falso conflitto R2.6 su un passthrough (R2.4: la
                    // dichiarazione del produttore resta). Prima
                    // dell'emendamento lo skip copriva solo
                    // `axis_order = unknown`, allora unico valore possibile.
                    if key == PLENORA_GEOMETRY_AXIS_ORDER_KEY || key == PLENORA_GEOMETRY_SRID_KEY {
                        continue;
                    }
                    // R4.6.4: un centro che ha rilevato un'incoerenza CRS la
                    // DICHIARA (`declared_unresolved`) invece di propagare la
                    // dichiarazione `resolved` del produttore, che
                    // l'incoerenza stessa smentisce — silenziarla e'
                    // vietato. E' l'unica sovrascrittura ammessa su una
                    // chiave canonica: una sola chiave, una sola direzione
                    // (`resolved` -> `declared_unresolved`), mai il
                    // contrario (piano-v5.md#contratti-di-input, decisione 7). La direzione
                    // opposta (`declared_unresolved` -> `resolved`) non
                    // passa di qui: con una decisione del piano le
                    // dichiarazioni della sorgente sono gia' state rimosse
                    // sopra (`strip_decided_crs_declarations`).
                    if key == PLENORA_GEOMETRY_CRS_RESOLUTION_KEY
                        && existing == "resolved"
                        && value == "declared_unresolved"
                    {
                        metadata.insert(key.clone(), value.clone());
                        continue;
                    }
                    return Err(PlenoraError::InvalidPlan(format!(
                        "campo geometria `{}`: chiave `{key}` gia' presente con un valore \
                         diverso da quello del contratto (R2.6: il componente fallisce, \
                         non sovrascrive)",
                        geometry.name
                    )));
                }
                Some(_) => {}
                None => {
                    metadata.insert(key.clone(), value.clone());
                }
            }
        }
        fields.push(field.as_ref().clone().with_metadata(metadata));
    }
    if matched != contract.geometries.len() {
        return Err(PlenoraError::InvalidPlan(
            "colonna geometrica del contratto assente nello schema di output".to_owned(),
        ));
    }
    // R2.5: la versione accompagna le chiavi canoniche; qui almeno un campo
    // le porta (guardia in testa e conteggio sopra).
    let mut metadata = contract.schema.metadata().clone();
    for (key, value) in canonical_schema_version_metadata() {
        match metadata.get(&key) {
            Some(existing) if existing != &value => {
                return Err(PlenoraError::InvalidPlan(format!(
                    "chiave `{key}` dello schema gia' presente con un valore diverso \
                     (R2.6: il componente fallisce, non sovrascrive)"
                )));
            }
            Some(_) => {}
            None => {
                metadata.insert(key, value);
            }
        }
    }
    Ok(Arc::new(Schema::new_with_metadata(fields, metadata)))
}

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
