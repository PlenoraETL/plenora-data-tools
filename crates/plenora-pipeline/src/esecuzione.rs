//! Esecuzione di un piano validato: tabelle intere in memoria, vivibilità
//! per nome, byte vivi contati per allocazione, budget di memoria.
//!
//! Stato: una mappa nome → `RecordBatch` delle tabelle residenti. L'ultimo
//! passo che usa ogni nome si calcola una volta, all'indietro sui passi; gli
//! output del piano non muoiono mai. Dopo ogni passo si liberano le tabelle
//! il cui ultimo consumatore ha girato, e un'uscita che nessuno usa si
//! libera subito.
//!
//! **Budget** (`max_governed_memory_bytes` del piano). Prima di ogni passo
//! il picco previsto del kernel ([`crate::budget`]) più i byte vivi delle
//! tabelle residenti deve stare nel budget; altrimenti `ResourceLimit`
//! prima di eseguire, con il nome del passo e dell'operazione. Niente va su
//! disco: non c'è ripiego per un passo che non sta.
//!
//! Il kernel riceve come `max_governed_memory_bytes` il margine vero,
//! budget meno byte vivi. Dopo il passo, byte vivi con l'output oltre il
//! budget sono un `ResourceLimit` esplicito.
//!
//! Dopo ogni passo l'output del kernel deve avere nomi e tipi del contratto
//! inferito in validazione (altrimenti `Internal`: analisi e kernel
//! divergono) e riceve lo schema del contratto
//! (`arrow_schema_from_contract`). I controlli sui dati sono quelli di
//! `validate_batch` di `table_engine/executor.rs` e di `check_edge_counts`,
//! `check_expansion`, `check_join_expansion` di `executor/validation.rs` a
//! `190c493`, per un solo batch per arco.

use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

use plenora_core::arrow::array::{Array, ArrayRef, LargeStringArray, RecordBatch, StringArray};
use plenora_core::arrow::schema::{DataType, Schema};
use plenora_core::catalog::Arity;
use plenora_core::contract::arrow_schema::arrow_schema_from_contract;
use plenora_core::contract::DataContract;
use plenora_core::limits::expansion_exceeded;
use plenora_core::memoria::{byte_dati, byte_vivi};
use plenora_core::{PlenoraError, Result};
use plenora_kernels_table::EffettiKernel;

use crate::budget::Ingresso;
use crate::validazione::{
    nel_passo, nel_passo_o_input, BaseIndici, KernelPasso, PassoValidato, PipelineValidata,
    METADATI_PANDAS,
};

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
    /// Picco previsto del kernel oltre le tabelle residenti, fattore di
    /// sicurezza compreso.
    pub byte_previsti: u64,
    /// `max_governed_memory_bytes` passato al kernel: budget meno byte vivi.
    pub margine_kernel: u64,
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
    /// Righe in cui una divisione di `table.formula` o `table.expression`
    /// ha trovato un divisore zero e, con `on_division_by_zero = "null"`
    /// (il default), ha dato null: un conteggio, mai i valori. Zero per le
    /// altre operazioni; con `"error"` un passo riuscito ne ha zero (una
    /// divisione per zero lo fa fallire con la diagnostica per riga).
    pub righe_divisione_per_zero: u64,
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
///
/// Un passo con un'espansione fissata dalla config (`melt`,
/// `PassoValidato::moltiplicatore_dichiarato`) non espande i dati: al posto
/// del fattore si verifica che l'uscita abbia esattamente le righe
/// d'ingresso per quel fattore, e una differenza e' `Internal`.
fn check_expansion(
    passo: &PassoValidato,
    limiti: &plenora_core::limits::Limits,
    righe_in: &[u64],
    righe_out: u64,
) -> Result<()> {
    let descrittore = passo.descrittore;
    if let Some(fattore) = passo.moltiplicatore_dichiarato {
        let attese = match righe_in {
            [righe] => righe.checked_mul(fattore),
            _ => None,
        };
        return if attese == Some(righe_out) {
            Ok(())
        } else {
            Err(PlenoraError::Internal(
                "righe d'uscita diverse da quelle fissate dalla config".to_owned(),
            ))
        };
    }
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
    contratto: &DataContract,
) -> Result<(RecordBatch, EffettiKernel)> {
    #[cfg(test)]
    CHIAMATE_KERNEL.with(|chiamate| chiamate.set(chiamate.get() + 1));
    let nessuno = |uscita| (uscita, EffettiKernel::default());
    let chiamata = || match (&passo.kernel, ingressi) {
        (KernelPasso::Geo(geo), _) => geo
            .esegui(ingressi, contratto, limiti.max_governed_memory_bytes)
            .map(nessuno),
        (KernelPasso::Tabellare(preparato), [unico]) => {
            preparato.esegui_unario_con_effetti(unico, limiti)
        }
        (KernelPasso::Tabellare(preparato), [sinistra, destra]) => preparato
            .esegui_binario(sinistra, destra, limiti)
            .map(nessuno),
        (KernelPasso::Tabellare(_), _) => Err(PlenoraError::Internal(
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

/// La diagnostica per riga di un passo nella base decisa in validazione.
///
/// Con [`BaseIndici::Sorgente`] l'errore del kernel resta com'è. Con
/// [`BaseIndici::IngressoDelPasso`] gli indici del kernel, che sono righe del
/// primo ingresso del passo, si dichiarano tali: `index_basis` diventa
/// `step_input_row_zero_based` e il testo nomina passo e ingresso (nomi del
/// piano, mai valori). Gli indici non si toccano: sono già quelli giusti per
/// quella base. Un errore senza payload resta com'è.
fn diagnostica_nella_base(errore: PlenoraError, passo: &PassoValidato) -> PlenoraError {
    if passo.base_indici == BaseIndici::Sorgente {
        return errore;
    }
    match errore {
        PlenoraError::RowDiagnostics {
            source,
            mut diagnostics,
        } => {
            passo
                .base_indici
                .index_basis()
                .clone_into(&mut diagnostics.index_basis);
            if diagnostics.validate_for_emission().is_err() {
                return PlenoraError::Internal(format!(
                    "passo `{}`: diagnostica per riga non valida dopo il cambio di base",
                    passo.out
                ));
            }
            let ingresso = passo.inputs.first().map_or("", String::as_str);
            let contesto = format!(
                "passo `{}`: indici di riga riferiti all'ingresso `{ingresso}` del passo, \
                 non alla sorgente",
                passo.out
            );
            PlenoraError::RowDiagnostics {
                source: Box::new(con_contesto_sotto_la_fase(&contesto, *source)),
                diagnostics,
            }
        }
        PlenoraError::Tagged { phase, source } => PlenoraError::Tagged {
            phase,
            source: Box::new(diagnostica_nella_base(*source, passo)),
        },
        altro => altro,
    }
}

/// Il contesto sul testo dell'errore, anche sotto un eventuale wrapper di
/// fase (oggi i kernel non ne mettono: la diagnostica sta su un
/// `DataMapping` con la fase derivata).
fn con_contesto_sotto_la_fase(contesto: &str, errore: PlenoraError) -> PlenoraError {
    match errore {
        PlenoraError::Tagged { phase, source } => PlenoraError::Tagged {
            phase,
            source: Box::new(con_contesto_sotto_la_fase(contesto, *source)),
        },
        altro => nel_passo_o_input(contesto, altro),
    }
}

/// Byte in ingresso per il modello: il maggiore fra la memoria tenuta viva
/// (ogni allocazione una volta) e il costo di una copia (colonne che sono lo
/// stesso array contano ciascuna: i kernel le copiano ciascuna).
fn byte_ingresso(ingressi: &[&RecordBatch]) -> Result<u64> {
    let copia = ingressi.iter().fold(0_u64, |totale, tabella| {
        totale.saturating_add(u64::try_from(byte_dati(tabella)).unwrap_or(u64::MAX))
    });
    Ok(byte_vivi(ingressi.iter().copied())?.max(copia))
}

/// Grandezze del modello di costo di un passo: righe (almeno quelle note a
/// secco dell'uscita), byte, coppie e celle d'uscita (righe per colonne del
/// contratto d'uscita).
fn ingresso_del_passo(passo: &PassoValidato, righe_in: &[u64], byte: u64) -> Ingresso {
    let righe = somma(righe_in).max(passo.righe_previste);
    Ingresso {
        righe,
        byte,
        coppie: coppie(righe_in),
        celle: righe.saturating_mul(passo.colonne_uscita),
    }
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
    /// ultimo consumatore ha girato. Un clone tenuto dal chiamante tiene vive
    /// le stesse allocazioni e vanifica il rilascio: i byte vivi del
    /// resoconto contano solo le tabelle del runner.
    ///
    /// # Errors
    ///
    /// - `InvalidPlan`: tabelle duplicate, mancanti o non dichiarate;
    /// - `Schema`: schema di un input diverso da quello dato in validazione,
    ///   nomi di colonna ripetuti;
    /// - `ResourceLimit`: righe o colonne oltre i limiti, fattore di
    ///   espansione superato, passo che non sta nel budget (prima di
    ///   eseguirlo) o che lo supera (dopo);
    /// - `Internal`: output di un kernel che diverge dal contratto, panico
    ///   di un kernel;
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
            // Lo schema del contratto dell'input (blocco canonico delle
            // geometrie, `validazione::canonico`): stesse colonne, solo
            // metadati in piu'.
            let canonico = self.contratti.get(&nome).ok_or_else(|| {
                PlenoraError::Internal(format!("contratto dell'input `{nome}` assente"))
            })?;
            let righe_tabella = tabella.num_rows();
            let tabella = plenora_core::batch_with_rows(
                canonico.schema.clone(),
                tabella.columns().to_vec(),
                righe_tabella,
            )
            .map_err(|errore| errore.con_contesto(&format!("input `{nome}`")))?;
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
        // Lo stato iniziale e' il primo confine: gli input residenti stanno
        // nel budget, anche in un piano senza passi.
        if byte_vivi_iniziali > budget {
            return Err(PlenoraError::ResourceLimit(format!(
                "input: byte vivi iniziali {byte_vivi_iniziali} oltre il budget {budget} \
                 (max_governed_memory_bytes)"
            )));
        }

        let mut passi = Vec::with_capacity(self.passi.len());
        for (indice, passo) in self.passi.iter().enumerate() {
            let op = passo.descrittore.id;
            let nel = |errore: PlenoraError| nel_passo(&passo.out, errore);
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

            // Prima di eseguire: byte vivi delle residenti piu' il picco
            // previsto nel budget, o il passo si rifiuta senza girare.
            let residenti = byte_vivi(vivi.values()).map_err(nel)?;
            let byte_previsti = passo.costo.in_memoria.picco(ingresso_del_passo(
                passo,
                &righe_in,
                byte_ingresso(&ingressi).map_err(nel)?,
            ));
            if residenti
                .checked_add(byte_previsti)
                .is_none_or(|totale| totale > budget)
            {
                return Err(nel(PlenoraError::ResourceLimit(format!(
                    "{op}: previsti {byte_previsti} byte oltre i {residenti} residenti, \
                     oltre il budget {budget} (max_governed_memory_bytes)"
                ))));
            }
            // Al kernel tutto il margine: i suoi preflight usano lo spazio
            // che c'e'.
            let margine = budget - residenti;
            let limiti_kernel = plenora_kernels_table::Limits {
                max_governed_memory_bytes: usize::try_from(margine).unwrap_or(usize::MAX),
                ..self.limiti_kernel.clone()
            };

            let (uscita, effetti) = esegui_kernel(passo, &ingressi, &limiti_kernel, contratto)
                .map_err(|errore| diagnostica_nella_base(errore, passo))
                .and_then(|(uscita, effetti)| {
                    validate_batch(&uscita, &self.limiti_kernel)?;
                    let righe_out = righe(&uscita)?;
                    // Come a `190c493` (`blocking.rs`), l'arco d'uscita del
                    // piano ha solo `max_output_rows`, controllato alla fine.
                    if !self.outputs.contains(&passo.out) {
                        check_edge_counts(&self.limiti, righe_out)?;
                    }
                    check_expansion(passo, &self.limiti, &righe_in, righe_out)?;
                    conforma(&uscita, contratto).map(|uscita| (uscita, effetti))
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
                byte_previsti,
                margine_kernel: margine,
                byte_output_esclusivi,
                byte_vivi_con_uscita: con_uscita,
                liberati,
                byte_vivi: byte_vivi(vivi.values())?,
                righe_divisione_per_zero: effetti.righe_divisione_per_zero,
            });
        }

        // Ultimo confine: tutti gli output residenti insieme.
        let finali = byte_vivi(vivi.values())?;
        if finali > budget {
            return Err(PlenoraError::ResourceLimit(format!(
                "output: byte vivi finali {finali} oltre il budget {budget} \
                 (max_governed_memory_bytes)"
            )));
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
            },
        })
    }
}

#[cfg(test)]
mod tests {
    //! Rifiuto prima di eseguire e conformità al contratto.

    use std::sync::Arc;

    use plenora_core::arrow::array::{Float64Array, Int64Array, RecordBatch, StringArray};
    use plenora_core::arrow::schema::{DataType, Field, Schema};
    use serde_json::json;

    use super::{conforma, CHIAMATE_KERNEL};
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
                .with_metadata(plenora_core::arrow::Metadata::from([("chiave", "valore")])),
        );
        let contratto = DataContract::tabular(con_metadati);
        assert!(matches!(
            conforma(&batch, &contratto),
            Err(PlenoraError::Internal(_))
        ));
        assert!(conforma(&batch, &DataContract::tabular(batch.schema())).is_ok());
    }
}
