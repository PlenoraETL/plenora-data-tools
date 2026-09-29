//! Esecuzione di un piano validato: tabelle intere in memoria, vivibilità
//! per nome, byte vivi contati per allocazione, budget di memoria.
//!
//! Stato: una mappa nome → `RecordBatch` delle tabelle residenti, e l'area
//! delle tabelle sfrattate su disco ([`crate::sfratto`]). L'ultimo passo che
//! usa ogni nome si calcola una volta, all'indietro sui passi; gli output
//! del piano non muoiono mai. Dopo ogni passo si liberano le tabelle il cui
//! ultimo consumatore ha girato, e un'uscita che nessuno usa si libera
//! subito.
//!
//! **Budget** (`max_governed_memory_bytes` del piano). Prima di ogni passo:
//!
//! 1. il picco previsto del kernel ([`crate::budget`]) più i byte vivi delle
//!    tabelle residenti, più le tabelle sfrattate da rileggere per il passo,
//!    deve stare nel budget;
//! 2. se non sta, si sfrattano su disco le tabelle residenti che il passo
//!    non usa, prima quella il cui prossimo uso è più lontano (Belady; a
//!    parità, per nome), fino al più corto prefisso di quell'ordine che fa
//!    stare il passo, poi si tengono in memoria quelle del prefisso che non
//!    servono. Il guadagno di uno sfratto sono i byte vivi che spariscono
//!    davvero, non la dimensione della tabella: le allocazioni condivise con
//!    tabelle residenti restano;
//! 3. se nemmeno così sta, la variante spilled del kernel dove c'è
//!    (`sort`, `distinct`, `aggregate`, set operation), con lo stesso
//!    procedimento: lo spill riduce il transitorio, non l'output, che il
//!    modello della variante comprende;
//! 4. altrimenti `ResourceLimit` prima di eseguire, con il nome del passo e
//!    dell'operazione.
//!
//! Il kernel riceve come `max_governed_memory_bytes` il margine vero,
//! budget meno byte vivi (per la variante spilled, al più il budget con cui
//! è stata misurata), e come `max_temp_bytes` la quota che gli sfratti non
//! occupano. Dopo il passo, byte vivi con l'output oltre il budget sono un
//! `ResourceLimit` esplicito.
//!
//! Dopo ogni passo l'output del kernel deve avere nomi e tipi del contratto
//! inferito in validazione (altrimenti `Internal`: analisi e kernel
//! divergono) e riceve lo schema del contratto
//! (`arrow_schema_from_contract`). I controlli sui dati sono quelli di
//! `validate_batch` di `table_engine/executor.rs` e di `check_edge_counts`,
//! `check_expansion`, `check_join_expansion` di `executor/validation.rs` a
//! `190c493`, per un solo batch per arco.

use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::sync::Arc;

use plenora_core::arrow::array::{Array, ArrayRef, LargeStringArray, RecordBatch, StringArray};
use plenora_core::arrow::schema::{DataType, Schema};
use plenora_core::catalog::Arity;
use plenora_core::contract::arrow_schema::arrow_schema_from_contract;
use plenora_core::contract::DataContract;
use plenora_core::limits::expansion_exceeded;
use plenora_core::memoria::{byte_dati, byte_vivi};
use plenora_core::{PlenoraError, Result};

use crate::budget::{riserva_spill, Costo, Ingresso};
use crate::costi_operazioni::BUDGET_SPILL_MISURATO;
use crate::dispatch::Variante;
use crate::sfratto::AreaSfratti;
use crate::validazione::{nel_passo, PassoValidato, PipelineValidata, METADATI_PANDAS};

/// Esito di un'esecuzione: le tabelle d'uscita e il resoconto.
#[derive(Debug)]
pub struct Esito {
    /// Output nell'ordine di `outputs` del piano.
    pub outputs: Vec<(String, RecordBatch)>,
    pub report: Report,
}

/// Resoconto dell'esecuzione.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Report {
    /// Budget applicato: `max_governed_memory_bytes` del piano.
    pub budget: u64,
    /// Input dichiarati e mai usati, liberati prima del primo passo.
    pub liberati_all_avvio: Vec<String>,
    /// Byte vivi prima del primo passo (dopo quei rilasci).
    pub byte_vivi_iniziali: u64,
    /// Un elemento per passo, nell'ordine del piano.
    pub passi: Vec<ReportPasso>,
    /// Output del piano sfrattati e riletti alla fine, in ordine di nome.
    pub ricaricati_alla_fine: Vec<String>,
    /// Massimo dei byte su disco degli sfratti.
    pub byte_su_disco_massimi: u64,
}

/// Resoconto di un passo.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReportPasso {
    /// Nome della tabella prodotta.
    pub out: String,
    /// Id canonico dell'operazione.
    pub op: &'static str,
    /// Righe di ogni input, nell'ordine del passo.
    pub righe_in: Vec<u64>,
    pub righe_out: u64,
    /// Variante del kernel eseguita.
    pub variante: Variante,
    /// Picco previsto del kernel oltre le tabelle residenti, fattore di
    /// sicurezza compreso (per la variante spilled, con la riserva per le
    /// partizioni rilette, `budget::riserva_spill`).
    pub byte_previsti: u64,
    /// `max_governed_memory_bytes` passato al kernel.
    pub margine_kernel: u64,
    /// Tabelle sfrattate su disco prima del passo, nell'ordine di sfratto.
    pub sfrattati: Vec<String>,
    /// Input del passo riletti dal disco prima del passo, in ordine di nome.
    pub ricaricati: Vec<String>,
    /// Byte dell'output in allocazioni che nessuna tabella residente
    /// raggiungeva prima del passo: la memoria nuova del passo. Una
    /// rinomina ne ha zero.
    pub byte_output_esclusivi: u64,
    /// Byte vivi delle tabelle residenti con l'output, prima dei rilasci:
    /// il confine controllato contro il budget.
    pub byte_vivi_con_uscita: u64,
    /// Tabelle liberate dopo il passo, in ordine di nome.
    pub liberati: Vec<String>,
    /// Byte vivi di tutte le tabelle residenti dopo il passo e i rilasci.
    pub byte_vivi: u64,
}

/// Ultimo uso degli output del piano: nessun passo li libera.
const USO_SENZA_FINE: usize = usize::MAX;

fn righe(batch: &RecordBatch) -> Result<u64> {
    u64::try_from(batch.num_rows())
        .map_err(|_| PlenoraError::Internal("numero di righe non rappresentabile".to_owned()))
}

/// Limiti del piano su un batch (porting di `validate_batch`): righe,
/// colonne, nomi di colonna ripetuti.
fn validate_batch(batch: &RecordBatch, limits: &plenora_kernels_table::Limits) -> Result<()> {
    if batch.num_rows() > limits.max_rows {
        return Err(PlenoraError::ResourceLimit(format!(
            "batch con {} righe oltre il limite {}",
            batch.num_rows(),
            limits.max_rows
        )));
    }
    if batch.num_columns() > limits.max_columns {
        return Err(PlenoraError::ResourceLimit(format!(
            "schema con {} colonne oltre il limite {}",
            batch.num_columns(),
            limits.max_columns
        )));
    }
    let mut names = HashSet::new();
    for field in batch.schema().fields() {
        if !names.insert(field.name()) {
            return Err(PlenoraError::Schema(format!(
                "nome colonna duplicato: {}",
                field.name()
            )));
        }
    }
    Ok(())
}

/// `LargeUtf8` di primo livello in `Utf8`, voce `pandas` tolta dai metadati
/// di schema (porting di `normalize_large_utf8`). Stessa trasformazione di
/// [`crate::validazione::normalizza_schema`] sugli schemi.
fn normalize_large_utf8(batch: RecordBatch) -> Result<RecordBatch> {
    let has_large_utf8 = batch
        .schema()
        .fields()
        .iter()
        .any(|field| field.data_type() == &DataType::LargeUtf8);
    let has_pandas_metadata = batch.schema().metadata().contains_key(METADATI_PANDAS);
    if !has_large_utf8 && !has_pandas_metadata {
        return Ok(batch);
    }
    let mut metadata = batch.schema().metadata().clone();
    // Pandas tiene in questa voce un secondo schema, indipendente: una
    // trasformazione qualsiasi lo rende falso, e PyArrow reinterpreterebbe
    // colonne Arrow corrette alla lettura.
    let _ = metadata.remove(METADATI_PANDAS);
    let mut fields = Vec::with_capacity(batch.num_columns());
    let mut columns: Vec<ArrayRef> = Vec::with_capacity(batch.num_columns());
    for (field, column) in batch.schema().fields().iter().zip(batch.columns()) {
        if field.data_type() == &DataType::LargeUtf8 {
            let strings = column
                .as_any()
                .downcast_ref::<LargeStringArray>()
                .ok_or_else(|| PlenoraError::Schema("downcast LargeUtf8 fallito".into()))?;
            let bytes = strings.iter().flatten().try_fold(0_usize, |total, value| {
                total.checked_add(value.len()).ok_or_else(|| {
                    PlenoraError::ResourceLimit("overflow dimensione colonna LargeUtf8".into())
                })
            })?;
            if bytes > i32::MAX as usize {
                return Err(PlenoraError::ResourceLimit(
                    "colonna LargeUtf8 oltre il limite sicuro Utf8 di Arrow".into(),
                ));
            }
            fields.push(field.as_ref().clone().with_data_type(DataType::Utf8));
            columns.push(Arc::new(strings.iter().collect::<StringArray>()));
        } else {
            fields.push(field.as_ref().clone());
            columns.push(column.clone());
        }
    }
    plenora_core::batch_with_rows(
        Arc::new(Schema::new_with_metadata(fields, metadata)),
        columns,
        batch.num_rows(),
    )
}

/// L'output del kernel ha nomi, tipi e metadati (di campo e di schema) del
/// contratto; riceve lo schema del contratto. Kernel e analisi concordano
/// sui metadati: una divergenza è un difetto, non un dato. La ricostruzione
/// verifica anche che una colonna dichiarata non nulla non contenga null.
fn conforma(batch: &RecordBatch, contratto: &DataContract) -> Result<RecordBatch> {
    let attesi = contratto.schema.fields();
    let ottenuti = batch.schema();
    let divergente = attesi.len() != ottenuti.fields().len()
        || attesi
            .iter()
            .zip(ottenuti.fields())
            .any(|(atteso, ottenuto)| {
                atteso.name() != ottenuto.name() || atteso.data_type() != ottenuto.data_type()
            });
    if divergente {
        return Err(PlenoraError::Internal(
            "l'output del kernel diverge dal contratto inferito in validazione \
             (nomi o tipi delle colonne)"
                .to_owned(),
        ));
    }
    let metadati_divergenti = contratto.schema.metadata() != ottenuti.metadata()
        || attesi
            .iter()
            .zip(ottenuti.fields())
            .any(|(atteso, ottenuto)| atteso.metadata() != ottenuto.metadata());
    if metadati_divergenti {
        return Err(PlenoraError::Internal(
            "l'output del kernel diverge dal contratto inferito in validazione \
             (metadati di campo o di schema)"
                .to_owned(),
        ));
    }
    let schema = arrow_schema_from_contract(contratto)?;
    let righe = batch.num_rows();
    plenora_core::batch_with_rows(schema, batch.columns().to_vec(), righe).map_err(|_| {
        PlenoraError::Internal(
            "l'output del kernel non rispetta lo schema del contratto \
             (nullabilita' o metadati)"
                .to_owned(),
        )
    })
}

/// Righe di un arco contro `max_rows_per_edge` (porting di
/// `check_edge_counts` per un batch per arco: `max_batches` vale 1).
fn check_edge_counts(limiti: &plenora_core::limits::Limits, righe: u64) -> Result<()> {
    if righe > limiti.rows.max_rows_per_edge {
        return Err(PlenoraError::ResourceLimit(format!(
            "max_rows_per_edge superato: {righe} righe > {}",
            limiti.rows.max_rows_per_edge
        )));
    }
    Ok(())
}

/// Fattore di espansione (porting di `check_expansion` e
/// `check_join_expansion`): base le righe di input per le unarie, il vincolo
/// del catalogo per le binarie; le operazioni esenti non si controllano.
fn check_expansion(
    passo: &PassoValidato,
    limiti: &plenora_core::limits::Limits,
    righe_in: &[u64],
    righe_out: u64,
) -> Result<()> {
    let descrittore = passo.descrittore;
    if descrittore.expansion_factor_exempt {
        return Ok(());
    }
    let fattore = limiti.rows.max_expansion_factor;
    let superato = match (descrittore.arity, righe_in) {
        (Arity::Unary, [base]) => expansion_exceeded(righe_out, *base, fattore),
        (Arity::BinaryOrdered | Arity::NAry, [sinistra, destra]) => descrittore
            .expansion_constraint
            .exceeded(righe_out, *sinistra, *destra, fattore),
        _ => {
            return Err(PlenoraError::Internal(
                "arieta' del passo diversa da quella validata".to_owned(),
            ))
        }
    };
    if superato {
        return Err(PlenoraError::ResourceLimit(format!(
            "max_expansion_factor superato (vincolo {:?}): soglia {}, output={righe_out}, \
             input={righe_in:?}",
            descrittore.expansion_constraint,
            descrittore.expansion_constraint.binding_threshold(fattore),
        )));
    }
    Ok(())
}

#[cfg(test)]
thread_local! {
    /// Chiamate ai kernel del thread: i test verificano che un passo
    /// rifiutato dal budget non abbia girato.
    pub static CHIAMATE_KERNEL: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Chiama il kernel. Un panico diventa `Internal` con la sola forma del
/// payload, mai il testo (può contenere valori di riga).
fn esegui_kernel(
    passo: &PassoValidato,
    ingressi: &[&RecordBatch],
    limiti: &plenora_kernels_table::Limits,
    variante: Variante,
) -> Result<RecordBatch> {
    #[cfg(test)]
    CHIAMATE_KERNEL.with(|chiamate| chiamate.set(chiamate.get() + 1));
    let chiamata = || match ingressi {
        [unico] => passo.preparato.esegui_unario(unico, limiti, variante),
        [sinistra, destra] => passo
            .preparato
            .esegui_binario(sinistra, destra, limiti, variante),
        _ => Err(PlenoraError::Internal(
            "numero di input diverso da quello validato".to_owned(),
        )),
    };
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(chiamata)).unwrap_or_else(|payload| {
        Err(PlenoraError::Internal(format!(
            "panico nel kernel {}: {}",
            passo.descrittore.id,
            plenora_core::panic_policy::forma_payload(payload.as_ref())
        )))
    })
}

/// Byte vivi delle tabelle residenti meno quelle escluse.
fn byte_vivi_senza(vivi: &BTreeMap<String, RecordBatch>, esclusi: &[&str]) -> Result<u64> {
    byte_vivi(
        vivi.iter()
            .filter(|(nome, _)| !esclusi.contains(&nome.as_str()))
            .map(|(_, tabella)| tabella),
    )
}

/// Il più corto prefisso dell'ordine di sfratto che fa stare `extra` nel
/// budget, poi ridotto: si tiene in memoria, dalla tabella usata prima,
/// ogni tabella del prefisso senza la quale il passo sta comunque. `None`
/// se nemmeno sfrattando tutti i candidati il passo sta.
fn scegli_sfratti<'a>(
    vivi: &BTreeMap<String, RecordBatch>,
    candidati: &[&'a str],
    extra: u64,
    budget: u64,
    disco: u64,
) -> Result<Option<Vec<&'a str>>> {
    // Memoria: i byte vivi che restano piu' il passo. Disco: i byte di una
    // copia delle tabelle sfrattate (i dati del file IPC, senza i metadati
    // dei blocchi) nella quota rimasta; la scrittura la verifica comunque.
    let sta = |esclusi: &[&str]| -> Result<bool> {
        let su_disco =
            esclusi
                .iter()
                .filter_map(|nome| vivi.get(*nome))
                .fold(0_u64, |totale, tabella| {
                    totale.saturating_add(u64::try_from(byte_dati(tabella)).unwrap_or(u64::MAX))
                });
        Ok(su_disco <= disco
            && byte_vivi_senza(vivi, esclusi)?
                .checked_add(extra)
                .is_some_and(|totale| totale <= budget))
    };
    for lunghezza in 0..=candidati.len() {
        let mut scelti: Vec<&str> = candidati[..lunghezza].to_vec();
        if !sta(&scelti)? {
            continue;
        }
        for indice in (0..scelti.len()).rev() {
            let mut senza = scelti.clone();
            senza.remove(indice);
            if sta(&senza)? {
                scelti = senza;
            }
        }
        return Ok(Some(scelti));
    }
    Ok(None)
}

/// Variante scelta per un passo, con le tabelle da sfrattare.
struct Scelta<'a> {
    variante: Variante,
    costo: Costo,
    sfrattare: Vec<&'a str>,
}

/// Righe sinistra per righe destra di un passo binario; zero per gli altri.
/// Byte in ingresso per il modello: il maggiore fra la memoria tenuta viva
/// (ogni allocazione una volta) e il costo di una copia (colonne che sono lo
/// stesso array contano ciascuna: i kernel le copiano ciascuna).
fn byte_ingresso(ingressi: &[&RecordBatch]) -> Result<u64> {
    let copia = ingressi.iter().fold(0_u64, |totale, tabella| {
        totale.saturating_add(u64::try_from(byte_dati(tabella)).unwrap_or(u64::MAX))
    });
    Ok(byte_vivi(ingressi.iter().copied())?.max(copia))
}

const fn coppie(righe_in: &[u64]) -> u64 {
    match righe_in {
        [sinistra, destra] => sinistra.saturating_mul(*destra),
        _ => 0,
    }
}

fn somma(valori: &[u64]) -> u64 {
    valori
        .iter()
        .fold(0_u64, |totale, valore| totale.saturating_add(*valore))
}

impl PipelineValidata {
    /// Esegue il piano sulle tabelle date.
    ///
    /// Le tabelle passano **per valore**: il runner le libera appena il loro
    /// ultimo consumatore ha girato, e le sfratta su disco quando un passo
    /// non sta nel budget. Un clone tenuto dal chiamante tiene vive le
    /// stesse allocazioni e vanifica rilascio e sfratto: i byte vivi del
    /// resoconto contano solo le tabelle del runner.
    ///
    /// # Errors
    ///
    /// - `InvalidPlan`: tabelle duplicate, mancanti o non dichiarate;
    /// - `Schema`: schema di un input diverso da quello dato in validazione,
    ///   nomi di colonna ripetuti;
    /// - `ResourceLimit`: righe o colonne oltre i limiti, fattore di
    ///   espansione superato, passo che non sta nel budget (prima di
    ///   eseguirlo) o che lo supera (dopo), sfratti oltre `max_temp_bytes`;
    /// - `Internal`: output di un kernel che diverge dal contratto, panico
    ///   di un kernel;
    /// - `Io`, `Arrow`: scrittura o rilettura di una tabella sfrattata;
    /// - gli errori dei kernel, con il nome del passo dove la variante porta
    ///   un messaggio.
    #[allow(clippy::too_many_lines)] // Il ciclo dei passi in un punto solo.
    pub fn run(self, tables: Vec<(String, RecordBatch)>) -> Result<Esito> {
        let budget = self.limiti.max_governed_memory_bytes;
        // Input: esattamente quelli dichiarati, con lo schema validato.
        let mut vivi: BTreeMap<String, RecordBatch> = BTreeMap::new();
        for (nome, tabella) in tables {
            let Some(atteso) = self.schemi_input.get(&nome) else {
                return Err(PlenoraError::InvalidPlan(format!(
                    "tabella `{nome}` non dichiarata tra gli input del piano"
                )));
            };
            let tabella = normalize_large_utf8(tabella)
                .map_err(|errore| errore.con_contesto(&format!("input `{nome}`")))?;
            if tabella.schema().as_ref() != atteso.as_ref() {
                return Err(PlenoraError::Schema(format!(
                    "input `{nome}`: schema diverso da quello dato in validazione"
                )));
            }
            validate_batch(&tabella, &self.limiti_kernel)
                .map_err(|errore| errore.con_contesto(&format!("input `{nome}`")))?;
            let righe_input = righe(&tabella)?;
            if righe_input > self.limiti.rows.max_input_rows {
                return Err(PlenoraError::ResourceLimit(format!(
                    "input `{nome}`: {righe_input} righe oltre max_input_rows {}",
                    self.limiti.rows.max_input_rows
                )));
            }
            if vivi.insert(nome.clone(), tabella).is_some() {
                return Err(PlenoraError::InvalidPlan(format!(
                    "tabella `{nome}` fornita due volte"
                )));
            }
        }
        if let Some(mancante) = self.inputs.iter().find(|nome| !vivi.contains_key(*nome)) {
            return Err(PlenoraError::InvalidPlan(format!(
                "manca la tabella dell'input `{mancante}`"
            )));
        }

        // Ultimo uso, all'indietro: il primo consumatore incontrato è
        // l'ultimo. Un'uscita senza consumatori muore al suo passo; gli
        // output del piano non muoiono.
        let mut ultimo_uso: BTreeMap<&str, usize> = BTreeMap::new();
        for nome in &self.outputs {
            ultimo_uso.insert(nome, USO_SENZA_FINE);
        }
        for (indice, passo) in self.passi.iter().enumerate().rev() {
            for sorgente in &passo.inputs {
                ultimo_uso.entry(sorgente).or_insert(indice);
            }
            ultimo_uso.entry(&passo.out).or_insert(indice);
        }
        // Passi che usano ogni nome, in ordine: il prossimo uso per Belady.
        let mut usi: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
        for (indice, passo) in self.passi.iter().enumerate() {
            for sorgente in &passo.inputs {
                usi.entry(sorgente).or_default().push(indice);
            }
        }

        let liberati_all_avvio: Vec<String> = self
            .inputs
            .iter()
            .filter(|nome| !ultimo_uso.contains_key(nome.as_str()))
            .cloned()
            .collect();
        for nome in &liberati_all_avvio {
            vivi.remove(nome);
        }
        let byte_vivi_iniziali = byte_vivi(vivi.values())?;
        let mut area = AreaSfratti::new(self.limiti.max_temp_bytes);

        let mut passi = Vec::with_capacity(self.passi.len());
        for (indice, passo) in self.passi.iter().enumerate() {
            let op = passo.descrittore.id;
            let nel = |errore: PlenoraError| nel_passo(&passo.out, errore);
            let scelta = self
                .scegli(indice, passo, &vivi, &area, &usi)
                .map_err(nel)?;
            let variante = scelta.variante;
            let costo = scelta.costo;
            let da_sfrattare: Vec<String> = scelta
                .sfrattare
                .iter()
                .map(|nome| (*nome).to_owned())
                .collect();

            // Sfratti, poi riletture: i byte liberati fanno posto a quelli
            // riletti.
            for nome in &da_sfrattare {
                let tabella = vivi.remove(nome).ok_or_else(|| {
                    nel(PlenoraError::Internal(format!(
                        "`{nome}` da sfrattare non residente"
                    )))
                })?;
                let byte_tabella = byte_vivi(std::iter::once(&tabella)).map_err(nel)?;
                area.sfratta(nome, &tabella, byte_tabella).map_err(nel)?;
            }
            let da_rileggere: BTreeSet<&str> = passo
                .inputs
                .iter()
                .map(String::as_str)
                .filter(|nome| area.sfrattata(nome).is_some())
                .collect();
            let mut ricaricati = Vec::with_capacity(da_rileggere.len());
            for nome in da_rileggere {
                let tabella = area.ricarica(nome).map_err(nel)?;
                vivi.insert(nome.to_owned(), tabella);
                ricaricati.push(nome.to_owned());
            }

            let ingressi: Vec<&RecordBatch> = passo
                .inputs
                .iter()
                .map(|sorgente| {
                    vivi.get(sorgente).ok_or_else(|| {
                        PlenoraError::Internal(format!(
                            "`{sorgente}` non residente al passo che lo usa"
                        ))
                    })
                })
                .collect::<Result<_>>()?;
            let righe_in: Vec<u64> = ingressi.iter().map(|t| righe(t)).collect::<Result<_>>()?;
            let contratto = self.contratti.get(&passo.out).ok_or_else(|| {
                PlenoraError::Internal(format!("contratto di `{}` assente", passo.out))
            })?;

            // Controllo con i byte veri, dopo sfratti e riletture.
            let residenti = byte_vivi(vivi.values()).map_err(nel)?;
            let (byte_previsti, riserva) = self.previsione(
                variante,
                costo,
                Ingresso {
                    righe: somma(&righe_in),
                    byte: byte_ingresso(&ingressi).map_err(nel)?,
                    coppie: coppie(&righe_in),
                },
            );
            if residenti
                .checked_add(byte_previsti)
                .is_none_or(|totale| totale > budget)
            {
                return Err(nel(PlenoraError::ResourceLimit(format!(
                    "{op}: previsti {byte_previsti} byte oltre i {residenti} residenti, \
                     oltre il budget {budget} (max_governed_memory_bytes)"
                ))));
            }
            // In memoria: tutto il margine. Spilled: meta' di cio' che resta
            // oltre il picco del modello (almeno meta' della riserva), al
            // piu' il budget delle misure: il kernel tiene fino al suo budget
            // di batch riletti e `concat_batches` li copia ([`riserva_spill`]).
            let margine = match variante {
                Variante::InMemoria => budget - residenti,
                Variante::Spill => ((budget - residenti - (byte_previsti - riserva)) / 2)
                    .min(BUDGET_SPILL_MISURATO),
            };
            let limiti_kernel = plenora_kernels_table::Limits {
                max_governed_memory_bytes: usize::try_from(margine).unwrap_or(usize::MAX),
                max_temp_bytes: area.quota_rimasta(),
                ..self.limiti_kernel.clone()
            };

            let uscita = esegui_kernel(passo, &ingressi, &limiti_kernel, variante)
                .and_then(|uscita| {
                    validate_batch(&uscita, &self.limiti_kernel)?;
                    let righe_out = righe(&uscita)?;
                    // Come a `190c493` (`blocking.rs`), l'arco d'uscita del
                    // piano ha solo `max_output_rows`, controllato alla fine.
                    if !self.outputs.contains(&passo.out) {
                        check_edge_counts(&self.limiti, righe_out)?;
                    }
                    check_expansion(passo, &self.limiti, &righe_in, righe_out)?;
                    conforma(&uscita, contratto)
                })
                .map_err(nel)?;
            let righe_out = righe(&uscita)?;

            let con_uscita = byte_vivi(vivi.values().chain(std::iter::once(&uscita)))?;
            let byte_output_esclusivi = con_uscita.checked_sub(residenti).ok_or_else(|| {
                PlenoraError::Internal("byte vivi diminuiti aggiungendo una tabella".to_owned())
            })?;
            if con_uscita > budget {
                return Err(nel(PlenoraError::ResourceLimit(format!(
                    "{op}: byte vivi con l'output {con_uscita} oltre il budget {budget} \
                     (max_governed_memory_bytes)"
                ))));
            }
            vivi.insert(passo.out.clone(), uscita);

            let liberati: Vec<String> = ultimo_uso
                .iter()
                .filter(|(_, ultimo)| **ultimo == indice)
                .map(|(nome, _)| (*nome).to_owned())
                .collect();
            for nome in &liberati {
                vivi.remove(nome);
            }
            passi.push(ReportPasso {
                out: passo.out.clone(),
                op,
                righe_in,
                righe_out,
                variante,
                byte_previsti,
                margine_kernel: margine,
                sfrattati: da_sfrattare,
                ricaricati,
                byte_output_esclusivi,
                byte_vivi_con_uscita: con_uscita,
                liberati,
                byte_vivi: byte_vivi(vivi.values())?,
            });
        }

        // Output sfrattati: si rileggono uno alla volta, nel budget.
        let mut ricaricati_alla_fine = Vec::new();
        let sfrattati_finali: Vec<String> = area.nomi().map(str::to_owned).collect();
        for nome in sfrattati_finali {
            let residenti = byte_vivi(vivi.values())?;
            let stimati = area
                .sfrattata(&nome)
                .map(crate::sfratto::Sfrattata::byte_stimati)
                .ok_or_else(|| {
                    PlenoraError::Internal(format!("output `{nome}` non piu' sfrattato"))
                })?;
            // Lettura: i blocchi e la tabella ricomposta insieme.
            let lettura = stimati.saturating_mul(2);
            if residenti
                .checked_add(lettura)
                .is_none_or(|totale| totale > budget)
            {
                return Err(PlenoraError::ResourceLimit(format!(
                    "output `{nome}`: rileggerlo dal disco richiede {lettura} byte oltre i \
                     {residenti} residenti, oltre il budget {budget} \
                     (max_governed_memory_bytes)"
                )));
            }
            let tabella = area.ricarica(&nome)?;
            vivi.insert(nome.clone(), tabella);
            let dopo = byte_vivi(vivi.values())?;
            if dopo > budget {
                return Err(PlenoraError::ResourceLimit(format!(
                    "output `{nome}`: riletto dal disco, byte vivi {dopo} oltre il budget \
                     {budget} (max_governed_memory_bytes)"
                )));
            }
            ricaricati_alla_fine.push(nome);
        }

        let mut outputs = Vec::with_capacity(self.outputs.len());
        for nome in &self.outputs {
            let tabella = vivi.remove(nome).ok_or_else(|| {
                PlenoraError::Internal(format!("output `{nome}` non residente a fine piano"))
            })?;
            let righe_output = righe(&tabella)?;
            if righe_output > self.limiti.rows.max_output_rows {
                return Err(PlenoraError::ResourceLimit(format!(
                    "output `{nome}`: {righe_output} righe oltre max_output_rows {}",
                    self.limiti.rows.max_output_rows
                )));
            }
            outputs.push((nome.clone(), tabella));
        }
        Ok(Esito {
            outputs,
            report: Report {
                budget,
                liberati_all_avvio,
                byte_vivi_iniziali,
                passi,
                ricaricati_alla_fine,
                byte_su_disco_massimi: area.massimo_su_disco(),
            },
        })
    }

    /// Picco previsto di un passo, riserva della variante spilled compresa,
    /// e quella riserva.
    fn previsione(&self, variante: Variante, costo: Costo, ingresso: Ingresso) -> (u64, u64) {
        let riserva = match variante {
            Variante::InMemoria => 0,
            Variante::Spill => riserva_spill(ingresso.byte, self.limiti.spill_partitions),
        };
        (costo.picco(ingresso).saturating_add(riserva), riserva)
    }

    /// Variante e sfratti di un passo, prima di toccare qualunque tabella.
    fn scegli<'a>(
        &self,
        indice: usize,
        passo: &PassoValidato,
        vivi: &'a BTreeMap<String, RecordBatch>,
        area: &AreaSfratti,
        usi: &BTreeMap<&str, Vec<usize>>,
    ) -> Result<Scelta<'a>> {
        let budget = self.limiti.max_governed_memory_bytes;
        let op = passo.descrittore.id;
        // Grandezze dell'ingresso: le tabelle sfrattate contano per la
        // stima di quando saranno rilette.
        let mut righe_in = Vec::with_capacity(passo.inputs.len());
        let mut residenti_del_passo = Vec::new();
        let mut riletti = 0_u64;
        let mut visti: BTreeSet<&str> = BTreeSet::new();
        for sorgente in &passo.inputs {
            if let Some(tabella) = vivi.get(sorgente) {
                righe_in.push(righe(tabella)?);
                residenti_del_passo.push(tabella);
            } else if let Some(sfrattata) = area.sfrattata(sorgente) {
                righe_in.push(sfrattata.righe);
                // Uno stesso input a sinistra e a destra si rilegge una volta.
                if visti.insert(sorgente) {
                    riletti = riletti.saturating_add(sfrattata.byte_stimati());
                }
            } else {
                return Err(PlenoraError::Internal(format!(
                    "`{sorgente}` ne' residente ne' sfrattato al passo che lo usa"
                )));
            }
        }
        let ingresso = Ingresso {
            righe: somma(&righe_in),
            byte: byte_ingresso(&residenti_del_passo)?.saturating_add(riletti),
            coppie: coppie(&righe_in),
        };

        // Candidati allo sfratto: le residenti che il passo non usa, prima
        // quella usata più tardi (Belady), a parità per nome. Un output del
        // piano senza altri consumatori ha il prossimo uso più lontano.
        let prossimo_uso = |nome: &str| -> usize {
            usi.get(nome)
                .and_then(|indici| indici.iter().copied().find(|uso| *uso > indice))
                .unwrap_or(USO_SENZA_FINE)
        };
        let mut candidati: Vec<&str> = vivi
            .keys()
            .map(String::as_str)
            .filter(|nome| !passo.inputs.iter().any(|sorgente| sorgente == nome))
            .collect();
        candidati.sort_by_key(|nome| (Reverse(prossimo_uso(nome)), *nome));

        let mut varianti = vec![(Variante::InMemoria, passo.costo.in_memoria)];
        if passo.preparato.ha_spill() {
            if let Some(costo) = passo.costo.spill {
                varianti.push((Variante::Spill, costo));
            }
        }
        for (variante, costo) in varianti {
            let (picco, _) = self.previsione(variante, costo, ingresso);
            // Rilettura (blocchi e tabella ricomposta), poi il kernel con le
            // tabelle rilette residenti.
            let extra = riletti.saturating_add(riletti.max(picco));
            if let Some(sfrattare) =
                scegli_sfratti(vivi, &candidati, extra, budget, area.quota_rimasta())?
            {
                return Ok(Scelta {
                    variante,
                    costo,
                    sfrattare,
                });
            }
        }
        let previsti = passo.costo.in_memoria.picco(ingresso);
        let residenti = byte_vivi(vivi.values())?;
        Err(PlenoraError::ResourceLimit(format!(
            "{op}: previsti {previsti} byte oltre i {residenti} residenti (piu' {riletti} \
             da rileggere dal disco), oltre il budget {budget} (max_governed_memory_bytes) \
             anche sfrattando le tabelle che il passo non usa e con la variante spilled \
             dove c'e'"
        )))
    }
}

#[cfg(test)]
mod tests {
    //! Oracolo del percorso con spill (stesso output del percorso in
    //! memoria, operazione per operazione) e rifiuto prima di eseguire.

    use std::sync::Arc;

    use plenora_core::arrow::array::{Float64Array, Int64Array, RecordBatch, StringArray};
    use plenora_core::arrow::schema::{DataType, Field, Schema};
    use serde_json::json;

    use super::{conforma, Variante, CHIAMATE_KERNEL};
    use crate::{LimitiParziali, Passo, Pipeline};
    use plenora_core::contract::DataContract;
    use plenora_core::PlenoraError;

    fn tabella(modulo: i64) -> RecordBatch {
        let righe: Vec<i64> = (0..1000).map(|indice| (indice * 7) % modulo).collect();
        RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("k", DataType::Int64, false),
                Field::new("g", DataType::Utf8, false),
                Field::new("v", DataType::Float64, false),
            ])),
            vec![
                Arc::new(Int64Array::from(righe.clone())),
                Arc::new(StringArray::from(
                    righe
                        .iter()
                        .map(|valore| format!("g{}", valore % 5))
                        .collect::<Vec<_>>(),
                )),
                Arc::new(Float64Array::from(
                    righe
                        .iter()
                        .map(|valore| f64::from(i32::try_from(*valore).expect("valore")))
                        .collect::<Vec<_>>(),
                )),
            ],
        )
        .expect("tabella")
    }

    /// Il kernel del passo, chiamato con la variante data e un budget sotto
    /// la stima dei byte di ogni input (circa 24 KiB), sopra il working set
    /// delle chiavi (50 distinte).
    fn esegui(op: &str, config: serde_json::Value, variante: Variante) -> RecordBatch {
        let binaria = matches!(
            op,
            "table.union_distinct" | "table.intersect" | "table.except"
        );
        let ingressi: Vec<String> = if binaria {
            vec!["a".into(), "b".into()]
        } else {
            vec!["a".into()]
        };
        let pipeline = Pipeline {
            version: 1,
            inputs: ingressi.clone(),
            crs: None,
            limits: None,
            steps: vec![Passo {
                out: "x".into(),
                op: op.into(),
                inputs: ingressi,
                config,
            }],
            outputs: vec!["x".into()],
        };
        let (a, b) = (tabella(50), tabella(40));
        let schemi = [("a", a.schema()), ("b", b.schema())];
        let validata = pipeline
            .validate(if binaria { &schemi[..] } else { &schemi[..1] })
            .expect("piano valido");
        let limiti = plenora_kernels_table::Limits {
            max_governed_memory_bytes: 16 * 1024,
            ..plenora_kernels_table::Limits::default()
        };
        let passo = &validata.passi[0];
        let uscita = if binaria {
            passo.preparato.esegui_binario(&a, &b, &limiti, variante)
        } else {
            passo.preparato.esegui_unario(&a, &limiti, variante)
        };
        uscita.unwrap_or_else(|errore| panic!("{op} {variante:?}: {errore}"))
    }

    #[test]
    fn lo_spill_sopra_il_budget_da_lo_stesso_output_della_memoria() {
        let casi = [
            ("table.sort", json!({"columns": ["k", "v"]})),
            ("table.distinct", json!({"subset": ["k"]})),
            (
                "table.aggregate",
                json!({"group_by": ["k"],
                       "aggregations": [{"column": "v", "function": "sum"}]}),
            ),
            ("table.union_distinct", json!({})),
            ("table.intersect", json!({})),
            ("table.except", json!({})),
        ];
        for (op, config) in casi {
            let memoria = esegui(op, config.clone(), Variante::InMemoria);
            let spill = esegui(op, config, Variante::Spill);
            assert_eq!(memoria, spill, "{op}");
            assert!(memoria.num_rows() > 0, "{op}");
        }
    }

    #[test]
    fn un_passo_fuori_budget_si_rifiuta_senza_chiamare_il_kernel() {
        // Due tabelle di 1000 righe: un milione di coppie, oltre 100 MB
        // previsti, contro un budget di 8 MiB.
        let pipeline = Pipeline {
            version: 1,
            inputs: vec!["a".into(), "b".into()],
            crs: None,
            limits: Some(LimitiParziali {
                max_governed_memory_bytes: Some(8 * 1024 * 1024),
                max_expansion_factor: Some(1.0e7),
                ..LimitiParziali::default()
            }),
            steps: vec![
                Passo {
                    out: "b2".into(),
                    op: "table.rename".into(),
                    inputs: vec!["b".into()],
                    config: json!({"renames": [
                        {"old_name": "k", "new_name": "k2"},
                        {"old_name": "g", "new_name": "g2"},
                        {"old_name": "v", "new_name": "v2"}
                    ]}),
                },
                Passo {
                    out: "x".into(),
                    op: "table.cross_join".into(),
                    inputs: vec!["a".into(), "b2".into()],
                    config: json!({}),
                },
            ],
            outputs: vec!["x".into()],
        };
        let (a, b) = (tabella(50), tabella(40));
        let validata = pipeline
            .validate(&[("a", a.schema()), ("b", b.schema())])
            .expect("piano valido");
        CHIAMATE_KERNEL.with(|chiamate| chiamate.set(0));
        let errore = validata
            .run(vec![("a".into(), a), ("b".into(), b)])
            .expect_err("oltre il budget");
        let PlenoraError::ResourceLimit(messaggio) = errore else {
            panic!("atteso ResourceLimit: {errore}");
        };
        assert!(messaggio.contains("passo `x`"), "{messaggio}");
        assert!(messaggio.contains("table.cross_join"), "{messaggio}");
        assert!(messaggio.contains("previsti"), "{messaggio}");
        // Solo la rinomina ha girato.
        assert_eq!(CHIAMATE_KERNEL.with(std::cell::Cell::get), 1);
    }

    #[test]
    fn metadati_diversi_dal_contratto_sono_un_errore_interno() {
        let batch = tabella(10);
        let con_metadati = Arc::new(
            batch
                .schema()
                .as_ref()
                .clone()
                .with_metadata([("chiave".to_owned(), "valore".to_owned())].into()),
        );
        let contratto = DataContract::tabular(con_metadati);
        assert!(matches!(
            conforma(&batch, &contratto),
            Err(PlenoraError::Internal(_))
        ));
        assert!(conforma(&batch, &DataContract::tabular(batch.schema())).is_ok());
    }
}
