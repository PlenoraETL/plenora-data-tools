//! Validazione del piano, prima di qualunque esecuzione.
//!
//! Tutto ciò che si può dire senza i dati si dice qui: nomi SSA, operazione
//! e arietà dal catalogo, dispatch del runner, config tipizzate, contratti
//! passo per passo con `analyze_table_contract` e i limiti dei kernel, o con
//! `analyze_geo_contract` e il CRS di piano per le operazioni geo (ogni
//! regola sulla config sta lì, una volta sola, per ogni chiamante dei
//! kernel). Qui resta solo ciò che non è config: la chiave HMAC
//! nell'ambiente ([`verifica_ambiente`]). Un piano che passa non fallisce in
//! esecuzione per un motivo che la validazione poteva vedere.
//!
//! I passi 2 e 5 di `validate` di `plenora-engine/src/planner.rs` a
//! `190c493` sono portati quasi alla lettera: contratti di input con un solo
//! `FieldAllocator` e rimappatura dei `FieldId` geometrici, `sorted_by`
//! rifiutato sugli input, analisi per passo. Il controllo di provenance delle
//! diagnostiche per riga non rifiuta più: decide la base degli indici del
//! passo ([`BaseIndici`]). Restano fuori versioni e migrazioni del piano,
//! isolamento, capability, profilo di publish, `plan_hash` e
//! `catalog_fingerprint`.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use plenora_core::arrow::schema::{DataType, Schema, SchemaRef};
use plenora_core::catalog::{
    find_operation, Arity, Family, OperationDescriptor, SourceRowProvenance,
};
use plenora_core::contract::arrow_metadata::{GEOARROW_EXTENSION_KEY, GEOARROW_WKB_EXTENSION};
use plenora_core::contract::arrow_schema::{
    arrow_schema_from_contract, contract_from_arrow_schema, pubblica_schema,
    verifica_identita_campi,
};
use plenora_core::contract::{DataContract, FieldAllocator};
use plenora_core::crs::{resolve_crs, ResolvedCrs};
use plenora_core::limits::{Limits, PlanLimits};
use plenora_core::{PlenoraError, Result};
use plenora_kernels_geo::analyze::analyze_geo_contract;
use plenora_kernels_table::analyze::analyze_table_contract;

use crate::budget::{costo_di, CostoOperazione};
use crate::dispatch::PassoPreparato;
use crate::geo::PassoGeo;
use crate::piano::{Pipeline, VERSIONE_PIANO};

/// Chiave dei metadati di schema che pandas usa come secondo schema opaco.
pub const METADATI_PANDAS: &str = "pandas";

/// Il kernel di un passo, con la config gia' letta.
#[derive(Debug)]
pub enum KernelPasso {
    /// Operazione tabellare.
    Tabellare(PassoPreparato),
    /// Operazione geo ([`crate::geo`]).
    Geo(Box<PassoGeo>),
}

/// Se gli indici di riga della diagnostica per riga di un passo
/// (`plenora-row-diagnostics-v1`) sono righe della sorgente.
///
/// Si decide in validazione, dal catalogo
/// ([`OperationDescriptor::source_row_provenance`]), e l'esecuzione la
/// applica senza guardare i dati: stesso piano, stessa base.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BaseIndici {
    /// Righe della tabella d'ingresso del piano da cui il primo ingresso del
    /// passo discende attraverso passi che conservano numero e ordine delle
    /// righe: l'indice del kernel è quello della sorgente, e il payload resta
    /// quello del kernel (`source_row_zero_based`).
    Sorgente,
    /// A monte un passo cambia numero o ordine delle righe (`filter`,
    /// `sort`, `join`, `aggregate`…) e la riga della sorgente non si
    /// ricostruisce. Il runner toglie gli esempi dal payload e lascia i
    /// conteggi con il limite di conoscenza
    /// `read.row_attribution_unavailable`
    /// ([`plenora_core::diagnostics::RowDiagnostics::senza_attribuzione`]);
    /// il testo dell'errore nomina passo e ingresso.
    SenzaAttribuzione,
}

/// Un passo che ha superato la validazione.
#[derive(Debug)]
pub struct PassoValidato {
    pub out: String,
    pub descrittore: &'static OperationDescriptor,
    pub inputs: Vec<String>,
    pub kernel: KernelPasso,
    /// Modello di costo dell'operazione, per il budget.
    pub costo: &'static CostoOperazione,
    /// Righe dell'uscita note a secco (le celle di `geo.generate_grid`):
    /// il modello di costo le usa come righe quando superano quelle degli
    /// ingressi.
    pub righe_previste: u64,
    /// Base degli indici della diagnostica per riga del passo.
    pub base_indici: BaseIndici,
    /// Colonne dell'uscita, dal contratto: con le righe in ingresso danno le
    /// celle d'uscita del modello di costo ([`crate::budget::Ingresso`]).
    pub colonne_uscita: u64,
    /// Espansione fissata dalla config (`melt`): fuori dal fattore di
    /// espansione, verificata esatta dopo il passo
    /// ([`PassoPreparato::moltiplicatore_dichiarato`]).
    pub moltiplicatore_dichiarato: Option<u64>,
}

/// Piano validato contro gli schemi degli input: pronto per l'esecuzione.
#[derive(Debug)]
pub struct PipelineValidata {
    pub(crate) inputs: Vec<String>,
    /// Schemi degli input dopo la normalizzazione (`LargeUtf8` → `Utf8`,
    /// metadati `pandas` tolti): le tabelle date a `run` devono avere
    /// esattamente questi, normalizzate allo stesso modo.
    pub(crate) schemi_input: BTreeMap<String, SchemaRef>,
    pub(crate) passi: Vec<PassoValidato>,
    pub(crate) outputs: Vec<String>,
    pub(crate) contratti: BTreeMap<String, DataContract>,
    /// Schema pubblicato di ogni output del piano
    /// ([`pubblica_schema`]): versione del contratto e identità dei campi.
    /// `run` lo applica alle tabelle d'uscita.
    pub(crate) schemi_uscita: BTreeMap<String, SchemaRef>,
    pub(crate) limiti: Limits,
    pub(crate) limiti_kernel: plenora_kernels_table::Limits,
    pub(crate) crs_piano: Option<ResolvedCrs>,
}

impl PipelineValidata {
    /// Contratto inferito per un nome del piano (input o `out` di un passo).
    ///
    /// È la forma interna: lo schema che un output porta fuori dal runner è
    /// [`PipelineValidata::schema_uscita`].
    #[must_use]
    pub fn contratto(&self, nome: &str) -> Option<&DataContract> {
        self.contratti.get(nome)
    }

    /// Schema con cui `run` restituisce l'output `nome`: lo schema del
    /// contratto con `plenora.contract.version` e `plenora.field_id` su
    /// ogni campo (README, «Metadati Arrow»). `None` se `nome` non è un
    /// output del piano.
    #[must_use]
    pub fn schema_uscita(&self, nome: &str) -> Option<&SchemaRef> {
        self.schemi_uscita.get(nome)
    }

    /// Limiti effettivi del piano.
    #[must_use]
    pub const fn limiti(&self) -> &Limits {
        &self.limiti
    }

    /// CRS di piano risolto, se dichiarato.
    #[must_use]
    pub const fn crs_piano(&self) -> Option<&ResolvedCrs> {
        self.crs_piano.as_ref()
    }

    /// Base degli indici della diagnostica per riga del passo che produce
    /// `out`: dice se un rifiuto del passo avrà esempi con l'indice della
    /// sorgente o solo conteggi. `None` se nessun passo produce `out`.
    #[must_use]
    pub fn base_indici(&self, out: &str) -> Option<BaseIndici> {
        self.passi
            .iter()
            .find(|passo| passo.out == out)
            .map(|passo| passo.base_indici)
    }
}

/// Aggiunge il nome del passo all'errore, senza cambiarne la categoria.
///
/// `con_contesto` antepone il contesto solo a piano, supporto, schema e CRS;
/// qui lo riceve anche un errore di risorsa, interno o di conversione, che
/// altrimenti non direbbe a quale passo appartiene. I nomi dei passi sono
/// del piano, non dei dati.
pub fn nel_passo(out: &str, errore: PlenoraError) -> PlenoraError {
    nel_passo_o_input(&format!("passo `{out}`"), errore)
}

pub fn nel_passo_o_input(contesto: &str, errore: PlenoraError) -> PlenoraError {
    match errore {
        PlenoraError::ResourceLimit(messaggio) => {
            PlenoraError::ResourceLimit(format!("{contesto}: {messaggio}"))
        }
        PlenoraError::Internal(messaggio) => {
            PlenoraError::Internal(format!("{contesto}: {messaggio}"))
        }
        PlenoraError::DataMapping(messaggio) => {
            PlenoraError::DataMapping(format!("{contesto}: {messaggio}"))
        }
        altro => altro.con_contesto(contesto),
    }
}

/// Normalizzazione degli schemi di input, la stessa che `run` applica ai
/// batch.
///
/// `LargeUtf8` di primo livello diventa `Utf8`, una colonna `geoarrow.wkb`
/// `LargeBinary` diventa `Binary` (il contratto Arrow ammette entrambe, i
/// kernel geo leggono `Binary`), la voce `pandas` dei metadati di schema si
/// toglie (è un secondo schema che una trasformazione renderebbe falso).
/// Pubblica perché `data.describe` della CLI legga un input come lo
/// leggerebbe un piano.
#[must_use]
pub fn normalizza_schema(schema: &Schema) -> SchemaRef {
    let mut metadati = schema.metadata().clone();
    let _ = metadati.remove(METADATI_PANDAS);
    let campi: Vec<_> = schema
        .fields()
        .iter()
        .map(|campo| {
            if campo.data_type() == &DataType::LargeUtf8 {
                campo.as_ref().clone().with_data_type(DataType::Utf8)
            } else if geometria_large_binary(campo) {
                campo.as_ref().clone().with_data_type(DataType::Binary)
            } else {
                campo.as_ref().clone()
            }
        })
        .collect();
    Arc::new(Schema::new_with_metadata(campi, metadati))
}

/// Colonna `geoarrow.wkb` con storage `LargeBinary`, che la normalizzazione
/// converte in `Binary`.
pub fn geometria_large_binary(campo: &plenora_core::arrow::schema::Field) -> bool {
    campo.data_type() == &DataType::LargeBinary
        && campo
            .metadata()
            .get(GEOARROW_EXTENSION_KEY)
            .is_some_and(|estensione| estensione == GEOARROW_WKB_EXTENSION)
}

/// Limiti dei kernel tabellari derivati dai limiti effettivi del piano.
///
/// Porting di `limiti_dei_kernel_tabellari` di `plenora-engine/src/prepare.rs`
/// a `190c493`: `max_rows` è un limite per tabella, ancorato a
/// `max_input_rows`; `max_columns` e `max_split_columns` sono limiti interni
/// dei kernel, non del piano. Le conversioni verso `usize` sono fail-closed:
/// un limite non rappresentabile si rifiuta, non si allarga.
fn limiti_dei_kernel_tabellari(limiti: &Limits) -> Result<plenora_kernels_table::Limits> {
    let stretto = |valore: u64, nome: &str| -> Result<usize> {
        usize::try_from(valore).map_err(|_| {
            PlenoraError::ResourceLimit(format!(
                "{nome} dichiarato oltre quanto questa piattaforma sa rappresentare"
            ))
        })
    };
    Ok(plenora_kernels_table::Limits {
        max_rows: stretto(limiti.rows.max_input_rows, "max_input_rows")?,
        max_columns: plenora_kernels_table::limiti_interni::MAX_COLUMNS,
        max_split_columns: plenora_kernels_table::limiti_interni::MAX_SPLIT_COLUMNS,
        max_string_bytes: limiti.max_string_bytes,
        max_regex_bytes: limiti.max_regex_bytes,
        max_governed_memory_bytes: stretto(
            limiti.max_governed_memory_bytes,
            "max_governed_memory_bytes",
        )?,
    })
}

/// Controlli sull'ambiente del processo, non sulla config: l'analisi dei
/// kernel non li vede, e un errore qui non arriva dopo i passi a monte.
///
/// Solo `table.hmac_sha256`: la variabile `key_env` deve esistere, non
/// essere vuota ed essere UTF-8, con la stessa funzione del kernel
/// (`security::carica_chiave_hmac`). Può ancora cambiare fra `validate` e
/// `run`; in quel caso l'errore arriva al passo (README, «Runner»).
fn verifica_ambiente(preparato: &PassoPreparato) -> Result<()> {
    if let PassoPreparato::HmacSha256(config) = preparato {
        // La stessa lettura del kernel: variabile assente, vuota o non UTF-8
        // si rifiutano qui come li' (la chiave letta si scarta subito).
        plenora_kernels_table::security::carica_chiave_hmac(&config.key_env)?;
    }
    Ok(())
}

/// Numero di input che il runner accetta per l'arietà del catalogo.
///
/// `NAry` (`table.concat`) si esegue col dispatch binario, quindi con
/// esattamente due input: oltre si rifiuta come faceva `prepare_table` a
/// `190c493`, prima dell'esecuzione.
fn verifica_arieta(descrittore: &OperationDescriptor, ricevuti: usize) -> Result<()> {
    let id = descrittore.id;
    match descrittore.arity {
        Arity::Unary if ricevuti != 1 => Err(PlenoraError::InvalidPlan(format!(
            "{id}: atteso 1 input, ricevuti {ricevuti}"
        ))),
        Arity::BinaryOrdered if ricevuti != 2 => Err(PlenoraError::InvalidPlan(format!(
            "{id}: attesi 2 input (left, right), ricevuti {ricevuti}"
        ))),
        Arity::NAry if ricevuti < 2 => Err(PlenoraError::InvalidPlan(format!(
            "{id}: attesi almeno 2 input, ricevuti {ricevuti}"
        ))),
        Arity::NAry if ricevuti > 2 => Err(PlenoraError::Unsupported(format!(
            "{id} con {ricevuti} input: sono supportati al massimo 2 input, \
             l'esecuzione N-aria non e' implementata"
        ))),
        _ => Ok(()),
    }
}

fn verifica_nome(nome: &str, ruolo: &str, limiti: &PlanLimits) -> Result<()> {
    if nome.is_empty() {
        return Err(PlenoraError::InvalidPlan(format!("{ruolo}: nome vuoto")));
    }
    if nome.len() > limiti.max_identifier_bytes {
        return Err(PlenoraError::InvalidPlan(format!(
            "{ruolo}: nome di {} byte oltre max_identifier_bytes {}",
            nome.len(),
            limiti.max_identifier_bytes
        )));
    }
    Ok(())
}

/// Limiti di complessità del piano (`PlanLimits::default()`): passi, input,
/// archi, fan-out, profondità, byte di config per passo. Il piano non li
/// dichiara: sono i default del motore d'origine.
fn verifica_limiti_del_piano(piano: &Pipeline, limiti: &PlanLimits) -> Result<()> {
    let oltre = |nome: &str, valore: usize, massimo: usize| {
        if valore > massimo {
            Err(PlenoraError::InvalidPlan(format!(
                "{nome}: {valore} oltre il limite {massimo}"
            )))
        } else {
            Ok(())
        }
    };
    oltre("max_inputs", piano.inputs.len(), limiti.max_inputs)?;
    oltre("max_plan_nodes", piano.steps.len(), limiti.max_plan_nodes)?;
    let archi = piano.steps.iter().map(|passo| passo.inputs.len()).sum();
    oltre("max_plan_edges", archi, limiti.max_plan_edges)?;
    let mut consumatori: BTreeMap<&str, usize> = BTreeMap::new();
    let mut profondita: BTreeMap<&str, usize> = BTreeMap::new();
    for passo in &piano.steps {
        let byte_config = serde_json::to_vec(&passo.config)
            .map_err(|_| PlenoraError::InvalidPlan("config non serializzabile".to_owned()))?
            .len();
        oltre(
            &format!("passo `{}`: max_config_bytes_per_node", passo.out),
            byte_config,
            limiti.max_config_bytes_per_node,
        )?;
        let mut livello = 0;
        for sorgente in &passo.inputs {
            let uso = consumatori.entry(sorgente).or_insert(0);
            *uso += 1;
            oltre(
                &format!("`{sorgente}`: max_fan_out"),
                *uso,
                limiti.max_fan_out,
            )?;
            livello = livello.max(profondita.get(sorgente.as_str()).copied().unwrap_or(0));
        }
        let livello = livello + 1;
        oltre(
            &format!("passo `{}`: max_plan_depth", passo.out),
            livello,
            limiti.max_plan_depth,
        )?;
        profondita.insert(&passo.out, livello);
    }
    Ok(())
}

/// Il contratto con lo schema che il runner emette: il blocco canonico
/// `plenora.geometry.*` e `plenora.contract.version` dal contratto
/// (`arrow_schema_from_contract`), come dopo ogni passo in esecuzione.
///
/// Ogni tabella del piano (input e uscite dei passi) porta questo schema,
/// e il passo seguente si analizza su di esso: altrimenti le chiavi che
/// l'emissione aggiunge (per esempio i tipi che un'operazione geo
/// ridichiara) sarebbero nel batch e non nel contratto, e il controllo
/// dopo il passo seguente li vedrebbe divergere. Uno schema senza
/// geometrie resta invariato.
fn canonico(mut contratto: DataContract) -> Result<DataContract> {
    contratto.schema = arrow_schema_from_contract(&contratto)?;
    contratto.validate()?;
    Ok(contratto)
}

/// Colonne di un contratto contro `max_columns` dei kernel e regole del
/// contratto (nomi unici): prevedibile dallo schema, quindi in validazione.
fn verifica_colonne(contratto: &DataContract, massimo: usize) -> Result<()> {
    let colonne = contratto.schema.fields().len();
    if colonne > massimo {
        return Err(PlenoraError::ResourceLimit(format!(
            "schema con {colonne} colonne oltre il limite {massimo}"
        )));
    }
    contratto.validate()
}

impl Pipeline {
    /// Valida il piano contro gli schemi degli input.
    ///
    /// `schemas` associa a ogni nome di `inputs` lo schema Arrow della tabella
    /// che `run` riceverà: nomi duplicati, mancanti o in più sono rifiutati.
    ///
    /// # Errors
    ///
    /// - `InvalidPlan`: versione diversa da 1, limiti non validi, piano oltre
    ///   `PlanLimits::default()` (passi, input, archi, fan-out, profondità,
    ///   byte di config, lunghezza dei nomi), nomi non
    ///   SSA (ridefiniti, usati prima della definizione, output inesistenti o
    ///   ripetuti), alias legacy al posto dell'id canonico, arietà errata,
    ///   config non valida, `sorted_by` su un input;
    /// - `Unsupported`: operazione sconosciuta o senza dispatch nel runner,
    ///   `table.concat` con più di due input, schema di output non
    ///   inferibile senza i dati;
    /// - `Schema`: contratti di input o inferiti che violano le regole;
    /// - `ResourceLimit`: schema di un input o di un passo oltre le colonne
    ///   ammesse dai kernel;
    /// - `Crs`: CRS di piano non risolvibile, CRS delle operazioni geo
    ///   (requisito del catalogo, dominio delle geometrie della config).
    #[allow(clippy::too_many_lines)] // Passi sequenziali della validazione: spezzarli nuocerebbe alla lettura.
    pub fn validate(&self, schemas: &[(&str, SchemaRef)]) -> Result<PipelineValidata> {
        if self.version != VERSIONE_PIANO {
            return Err(PlenoraError::InvalidPlan(format!(
                "versione del piano {} non supportata: attesa {VERSIONE_PIANO}",
                self.version
            )));
        }
        // Senza sostituzioni restano i default, validati allo stesso modo.
        let limiti = self.limits.clone().unwrap_or_default().applica()?;
        let limiti_kernel = limiti_dei_kernel_tabellari(&limiti)?;
        let limiti_piano = PlanLimits::default();

        // Nomi SSA: ogni nome definito una volta, fra input e `out` dei
        // passi; un passo usa solo nomi definiti prima.
        let mut definiti: BTreeSet<&str> = BTreeSet::new();
        for nome in &self.inputs {
            verifica_nome(nome, "input", &limiti_piano)?;
            if !definiti.insert(nome.as_str()) {
                return Err(PlenoraError::InvalidPlan(format!(
                    "input `{nome}` dichiarato due volte"
                )));
            }
        }
        for passo in &self.steps {
            verifica_nome(&passo.out, "passo", &limiti_piano)?;
            for sorgente in &passo.inputs {
                if !definiti.contains(sorgente.as_str()) {
                    return Err(PlenoraError::InvalidPlan(format!(
                        "passo `{}`: `{sorgente}` non e' definito prima del passo",
                        passo.out
                    )));
                }
            }
            if !definiti.insert(passo.out.as_str()) {
                return Err(PlenoraError::InvalidPlan(format!(
                    "`{}` e' gia' definito: ogni nome si definisce una volta sola",
                    passo.out
                )));
            }
        }
        if self.outputs.is_empty() {
            return Err(PlenoraError::InvalidPlan(
                "il piano non dichiara output".to_owned(),
            ));
        }
        let mut dichiarati: BTreeSet<&str> = BTreeSet::new();
        for nome in &self.outputs {
            if !definiti.contains(nome.as_str()) {
                return Err(PlenoraError::InvalidPlan(format!(
                    "output `{nome}` non definito dal piano"
                )));
            }
            if !dichiarati.insert(nome.as_str()) {
                return Err(PlenoraError::InvalidPlan(format!(
                    "output `{nome}` dichiarato due volte"
                )));
            }
        }

        verifica_limiti_del_piano(self, &limiti_piano)?;

        // Schemi di input: corrispondenza esatta con i nomi dichiarati.
        let mut forniti: BTreeMap<&str, &SchemaRef> = BTreeMap::new();
        for (nome, schema) in schemas {
            if forniti.insert(nome, schema).is_some() {
                return Err(PlenoraError::InvalidPlan(format!(
                    "schema di input duplicato per `{nome}`"
                )));
            }
        }
        for dichiarato in &self.inputs {
            if !forniti.contains_key(dichiarato.as_str()) {
                return Err(PlenoraError::InvalidPlan(format!(
                    "manca lo schema dell'input `{dichiarato}`"
                )));
            }
        }
        if let Some(extra) = forniti
            .keys()
            .find(|nome| !self.inputs.iter().any(|input| input.as_str() == **nome))
        {
            return Err(PlenoraError::InvalidPlan(format!(
                "schema fornito per `{extra}`, non dichiarato tra gli input del piano"
            )));
        }

        // CRS di piano: risolto qui, fail-closed, anche se nessun passo lo usa.
        let crs_piano = self
            .crs
            .as_deref()
            .map(|definizione| resolve_crs(definizione, "crs").map_err(PlenoraError::from))
            .transpose()?;

        // Contratti di input (passo 2 di `planner.rs::validate`). Un solo
        // FieldAllocator per piano; i FieldId delle geometrie di input sono
        // rimappati all'ingresso con un'allocazione fresca SENZA legare il
        // nome: input diversi possono avere colonne omonime.
        let mut campi = FieldAllocator::default();
        // Identità pubbliche dei campi di input (`plenora.field_id`): tutte
        // osservate, e ambigue quelle che più di un input dichiara.
        let mut identita_osservate: BTreeSet<u32> = BTreeSet::new();
        let mut identita_ambigue: BTreeSet<u32> = BTreeSet::new();
        let mut schemi_input: BTreeMap<String, SchemaRef> = BTreeMap::new();
        let mut contratti: BTreeMap<String, DataContract> = BTreeMap::new();
        let mut provenance: BTreeMap<String, bool> = BTreeMap::new();
        for nome in &self.inputs {
            let schema = forniti.get(nome.as_str()).ok_or_else(|| {
                PlenoraError::Internal("schema di input non risolto dopo il controllo".to_owned())
            })?;
            let normalizzato = normalizza_schema(schema);
            let mut letto = contract_from_arrow_schema(normalizzato.clone(), resolve_crs)
                .map_err(|errore| errore.con_contesto(&format!("input `{nome}`")))?;
            letto
                .validate()
                .map_err(|errore| errore.con_contesto(&format!("input `{nome}`")))?;
            for id in verifica_identita_campi(&normalizzato)
                .map_err(|errore| errore.con_contesto(&format!("input `{nome}`")))?
            {
                if !identita_osservate.insert(id) {
                    identita_ambigue.insert(id);
                }
            }
            if let Some(ordinamento) = &letto.properties.sorted_by {
                if ordinamento.confidence.value().is_some() {
                    return Err(PlenoraError::InvalidPlan(format!(
                        "l'input `{nome}` dichiara sorted_by con chiavi FieldId: il namespace \
                         dei FieldId e' assegnato dal runner e le chiavi non sono riferibili \
                         a colonne"
                    )));
                }
            }
            for geometria in &mut letto.geometries {
                let rimappato = campi.alloc()?;
                if letto.active_geometry == Some(geometria.field_id) {
                    letto.active_geometry = Some(rimappato);
                }
                geometria.field_id = rimappato;
            }
            verifica_colonne(&letto, limiti_kernel.max_columns)
                .map_err(|errore| nel_passo_o_input(&format!("input `{nome}`"), errore))?;
            letto = canonico(letto)
                .map_err(|errore| nel_passo_o_input(&format!("input `{nome}`"), errore))?;
            schemi_input.insert(nome.clone(), normalizzato);
            contratti.insert(nome.clone(), letto);
            provenance.insert(nome.clone(), true);
        }

        // Passi (passo 5 di `planner.rs::validate`), nell'ordine del piano,
        // che è già topologico per la regola SSA.
        let mut passi = Vec::with_capacity(self.steps.len());
        for passo in &self.steps {
            let descrittore = find_operation(&passo.op).ok_or_else(|| {
                nel_passo(
                    &passo.out,
                    PlenoraError::Unsupported(format!("{}: operazione sconosciuta", passo.op)),
                )
            })?;
            if descrittore.id != passo.op {
                return Err(nel_passo(
                    &passo.out,
                    PlenoraError::InvalidPlan(format!(
                        "`{}` e' un alias legacy: il piano usa l'id canonico `{}`",
                        passo.op, descrittore.id
                    )),
                ));
            }
            let geo = descrittore.family == Family::Geo;
            verifica_arieta(descrittore, passo.inputs.len())
                .map_err(|errore| nel_passo(&passo.out, errore))?;
            let preparato = if geo {
                None
            } else {
                Some(
                    PassoPreparato::prepara(descrittore.id, &passo.config)
                        .map_err(|errore| nel_passo(&passo.out, errore))?,
                )
            };
            // Il budget si applica a ogni passo: un'operazione senza modello
            // di costo non ha una previsione, e non si esegue.
            let costo = costo_di(descrittore.id).ok_or_else(|| {
                nel_passo(
                    &passo.out,
                    PlenoraError::Unsupported(format!(
                        "{}: operazione senza modello di costo per il budget di memoria",
                        descrittore.id
                    )),
                )
            })?;

            // Diagnostiche per riga: gli indici del kernel sono righe del suo
            // primo ingresso (per `assert_foreign_key` il lato left, per le
            // unarie l'unico). Sono righe della sorgente solo se nessun passo
            // a monte ha cambiato numero o ordine delle righe; altrimenti il
            // runner pubblica i soli conteggi. Nessuna catena si rifiuta per
            // questo.
            let righe_della_sorgente = passo
                .inputs
                .first()
                .is_some_and(|sorgente| provenance.get(sorgente).copied().unwrap_or(false));
            let base_indici = if righe_della_sorgente {
                BaseIndici::Sorgente
            } else {
                BaseIndici::SenzaAttribuzione
            };
            let ingressi: Vec<DataContract> = passo
                .inputs
                .iter()
                .map(|sorgente| {
                    contratti.get(sorgente).cloned().ok_or_else(|| {
                        PlenoraError::Internal(format!(
                            "contratto di `{sorgente}` non risolto dalla validazione"
                        ))
                    })
                })
                .collect::<Result<_>>()?;
            let moltiplicatore_dichiarato = match (&preparato, ingressi.first()) {
                (Some(preparato), Some(primo)) => {
                    preparato.moltiplicatore_dichiarato(&primo.schema)
                }
                _ => None,
            };
            let (uscita, kernel, righe_previste) = if let Some(preparato) = preparato {
                let uscita = analyze_table_contract(
                    descrittore.id,
                    &ingressi,
                    &passo.config,
                    &mut campi,
                    &limiti_kernel,
                )
                .map_err(|errore| nel_passo(&passo.out, errore))?;
                verifica_ambiente(&preparato).map_err(|errore| nel_passo(&passo.out, errore))?;
                (uscita, KernelPasso::Tabellare(preparato), 0)
            } else {
                let uscita = analyze_geo_contract(
                    descrittore.id,
                    &ingressi,
                    &passo.config,
                    crs_piano.as_ref(),
                    &mut campi,
                )
                .map_err(|errore| nel_passo(&passo.out, errore))?;
                let geo = PassoGeo::prepara(
                    descrittore,
                    &passo.config,
                    &ingressi,
                    &uscita,
                    &limiti,
                    self.outputs.contains(&passo.out),
                )
                .map_err(|errore| nel_passo(&passo.out, errore))?;
                let righe = PassoGeo::righe_previste(&uscita);
                (uscita, KernelPasso::Geo(Box::new(geo)), righe)
            };
            verifica_colonne(&uscita, limiti_kernel.max_columns)
                .map_err(|errore| nel_passo(&passo.out, errore))?;
            let uscita = canonico(uscita).map_err(|errore| nel_passo(&passo.out, errore))?;
            let colonne_uscita = u64::try_from(uscita.schema.fields().len()).unwrap_or(u64::MAX);
            contratti.insert(passo.out.clone(), uscita);
            // Un'operazione che conserva le righe conserva quelle del primo
            // ingresso: `assert_foreign_key` rende il lato left invariato.
            provenance.insert(
                passo.out.clone(),
                righe_della_sorgente
                    && descrittore.source_row_provenance() == SourceRowProvenance::Preserved,
            );
            passi.push(PassoValidato {
                out: passo.out.clone(),
                descrittore,
                inputs: passo.inputs.clone(),
                kernel,
                costo,
                righe_previste,
                base_indici,
                colonne_uscita,
                moltiplicatore_dichiarato,
            });
        }

        // Lo schema che ogni output porta fuori dal runner: versione e
        // identità dei campi, deciso qui una volta (`run` lo applica).
        let mut schemi_uscita = BTreeMap::new();
        for nome in &self.outputs {
            let interno = contratti.get(nome).ok_or_else(|| {
                PlenoraError::Internal(format!("contratto dell'output `{nome}` assente"))
            })?;
            let pubblicato =
                pubblica_schema(&interno.schema, &identita_osservate, &identita_ambigue)
                    .map_err(|errore| nel_passo_o_input(&format!("output `{nome}`"), errore))?;
            schemi_uscita.insert(nome.clone(), pubblicato);
        }

        Ok(PipelineValidata {
            inputs: self.inputs.clone(),
            schemi_input,
            passi,
            outputs: self.outputs.clone(),
            contratti,
            schemi_uscita,
            limiti,
            limiti_kernel,
            crs_piano,
        })
    }
}
