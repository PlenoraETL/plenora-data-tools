//! Esecuzione di un piano validato: tabelle intere in memoria, vivibilità
//! per nome, byte vivi contati per allocazione.
//!
//! Stato: una mappa nome → `RecordBatch`. L'ultimo passo che usa ogni nome
//! si calcola una volta, all'indietro sui passi; gli output del piano non
//! muoiono mai. Dopo ogni passo si liberano le tabelle il cui ultimo
//! consumatore ha girato, e un'uscita che nessuno usa si libera subito.
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
use plenora_core::{PlenoraError, Result};

use crate::byte::byte_vivi;
use crate::dispatch::Instradamento;
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
    /// Byte dell'output in allocazioni che nessuna tabella residente
    /// raggiungeva prima del passo: la memoria nuova del passo. Una
    /// rinomina ne ha zero.
    pub byte_output_esclusivi: u64,
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

/// L'output del kernel ha nomi e tipi del contratto; riceve lo schema del
/// contratto. La ricostruzione verifica anche che una colonna dichiarata
/// non nulla non contenga null.
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

/// Chiama il kernel. Un panico diventa `Internal` con la sola forma del
/// payload, mai il testo (può contenere valori di riga).
fn esegui_kernel(
    passo: &PassoValidato,
    ingressi: &[&RecordBatch],
    limiti: &plenora_kernels_table::Limits,
    instradamento: Instradamento,
) -> Result<RecordBatch> {
    let chiamata = || match ingressi {
        [unico] => passo.preparato.esegui_unario(unico, limiti, instradamento),
        [sinistra, destra] => {
            passo
                .preparato
                .esegui_binario(sinistra, destra, limiti, instradamento)
        }
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
    ///   espansione superato;
    /// - `Internal`: output di un kernel che diverge dal contratto, panico
    ///   di un kernel;
    /// - gli errori dei kernel, con il nome del passo dove la variante porta
    ///   un messaggio.
    pub fn run(self, tables: Vec<(String, RecordBatch)>) -> Result<Esito> {
        self.esegui(tables, Instradamento::InMemoria)
    }

    #[allow(clippy::too_many_lines)] // Il ciclo dei passi in un punto solo.
    pub(crate) fn esegui(
        self,
        tables: Vec<(String, RecordBatch)>,
        instradamento: Instradamento,
    ) -> Result<Esito> {
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

        let mut passi = Vec::with_capacity(self.passi.len());
        for (indice, passo) in self.passi.iter().enumerate() {
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

            let uscita = esegui_kernel(passo, &ingressi, &self.limiti_kernel, instradamento)
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
                .map_err(|errore| nel_passo(&passo.out, errore))?;
            let righe_out = righe(&uscita)?;

            let prima = byte_vivi(vivi.values())?;
            let con_uscita = byte_vivi(vivi.values().chain(std::iter::once(&uscita)))?;
            let byte_output_esclusivi = con_uscita.checked_sub(prima).ok_or_else(|| {
                PlenoraError::Internal("byte vivi diminuiti aggiungendo una tabella".to_owned())
            })?;
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
                op: passo.descrittore.id,
                righe_in,
                righe_out,
                byte_output_esclusivi,
                liberati,
                byte_vivi: byte_vivi(vivi.values())?,
            });
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
                liberati_all_avvio,
                byte_vivi_iniziali,
                passi,
            },
        })
    }
}

#[cfg(test)]
mod tests {
    //! Oracolo del percorso con spill: stesso output del percorso in
    //! memoria, operazione per operazione.

    use std::sync::Arc;

    use plenora_core::arrow::array::{Float64Array, Int64Array, RecordBatch, StringArray};
    use plenora_core::arrow::schema::{DataType, Field, Schema};
    use serde_json::json;

    use super::Instradamento;
    use crate::{LimitiParziali, Passo, Pipeline};

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

    fn esegui(op: &str, config: serde_json::Value, instradamento: Instradamento) -> RecordBatch {
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
            // Budget sotto la stima dei byte di ogni input (circa 24 KiB),
            // sopra il working set delle chiavi (50 distinte).
            limits: Some(LimitiParziali {
                max_governed_memory_bytes: Some(16 * 1024),
                ..LimitiParziali::default()
            }),
            steps: vec![Passo {
                out: "x".into(),
                op: op.into(),
                inputs: ingressi,
                config,
            }],
            outputs: vec!["x".into()],
        };
        let tabelle = [("a", tabella(50)), ("b", tabella(40))];
        let usate = if binaria { &tabelle[..] } else { &tabelle[..1] };
        let schemi: Vec<_> = usate.iter().map(|(nome, t)| (*nome, t.schema())).collect();
        let validata = pipeline.validate(&schemi).expect("piano valido");
        let esito = validata
            .esegui(
                usate
                    .iter()
                    .map(|(nome, t)| ((*nome).to_owned(), t.clone()))
                    .collect(),
                instradamento,
            )
            .unwrap_or_else(|errore| panic!("{op} {instradamento:?}: {errore}"));
        esito.outputs.into_iter().next().expect("output").1
    }

    #[test]
    fn lo_spill_sopra_il_budget_da_lo_stesso_output_della_memoria() {
        // Il percorso con spill scatta davvero: ogni input supera il budget.
        let limiti = plenora_kernels_table::Limits {
            max_governed_memory_bytes: 16 * 1024,
            ..plenora_kernels_table::Limits::default()
        };
        assert!(plenora_kernels_table::spill::should_spill_unary(
            &tabella(40),
            &limiti
        ));
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
            let memoria = esegui(op, config.clone(), Instradamento::InMemoria);
            let spill = esegui(op, config, Instradamento::SpillSopraBudget);
            assert_eq!(memoria, spill, "{op}");
            assert!(memoria.num_rows() > 0, "{op}");
        }
    }
}
