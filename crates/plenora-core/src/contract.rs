//! Contratti dati del grafo (Architetture.md par. 4.2/4.3, decisioni D6, D16,
//! D25; ADR 1, ADR 5).
//!
//! Fondamenta della Fase 2A: tipi NUOVI (non trasloco) che descrivono ciò che
//! scorre sugli archi del DAG (`DataContract`), l'identità logica stabile delle
//! colonne (`FieldId`), la provenienza e lo scope delle proprietà
//! (`PropertyConfidence`/`PropertyScope`/`ContractProperty`), le statistiche di
//! runtime (`RuntimeStatistic`) e la sequenza logica dei batch
//! (`BatchSequence`).
//!
//! Struttura aperta, comportamento chiuso: il modello ammette più colonne
//! geometriche e una geometria attiva, ma in v1 [`DataContract::validate`]
//! rifiuta contratti con più di una geometria (decisione D16).
//!
//! La type-safety avanzata sulle combinazioni confidence/scope prive di senso
//! (es. `Proven` + scope `Schema` per proprietà dataset-wide, ADR 5) è
//! deliberatamente rimandata: la rappresentazione v1 è la coppia semplice
//! `ContractProperty<T> { confidence, scope }`.

use std::collections::HashMap;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::arrow::SchemaRef;
use crate::crs::ResolvedCrs;
use crate::error::{PlenoraError, Result};

/// Identità logica stabile di una colonna nel grafo (decisione D16).
///
/// Namespace globale del grafo: gli ID sono assegnati dal planner dopo aver
/// letto tutti gli input (oppure rimappati all'ingresso), così due input non
/// possono collidere. Una rinomina preserva il `FieldId`; una colonna
/// calcolata o derivata ne riceve uno nuovo; un join eredita i `FieldId`
/// globali dei rispettivi rami.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct FieldId(pub u32);

impl fmt::Display for FieldId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "field#{}", self.0)
    }
}

/// Dimensionalità delle geometrie di una colonna.
///
/// v1: solo `XY` (decisione D16). Il modello è aperto a future varianti
/// (XYZ/XYM/XYZM) senza cambiare la forma del contratto.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GeometryDimensions {
    Xy,
}

/// Contratto di una colonna geometrica (Architetture.md par. 4.3).
#[derive(Clone, Debug)]
pub struct GeometryColumnContract {
    /// Identità logica stabile nel grafo: le rinomine cambiano `name`,
    /// non `field_id`.
    pub field_id: FieldId,
    /// Nome visibile della colonna nello schema Arrow.
    pub name: String,
    /// CRS risolto (solo `geo.reproject` lo modifica; ogni altro step lo
    /// preserva).
    pub crs: ResolvedCrs,
    pub dimensions: GeometryDimensions,
    pub nullable: bool,
}

/// Assegnatore di [`FieldId`] nel namespace globale del grafo (decisione D16).
///
/// Unico allocatore condiviso dal planner e dai moduli `analyze_contract`
/// delle famiglie di kernel (unificazione Fase 2A-3: in precedenza
/// `plenora-kernels-table` ne definiva uno locale, doppione di questo).
/// Il planner crea un unico allocatore per l'analisi dell'intero grafo:
/// [`FieldAllocator::observe`] registra gli ID già assegnati (contratti di
/// input) per evitare collisioni con i futuri [`FieldAllocator::alloc`];
/// le colonne propagate mantengono l'ID di input; quelle derivate
/// ([`FieldAllocator::derive`]) ne ricevono uno nuovo; le rinomine spostano
/// l'identità ([`FieldAllocator::rename`]). L'interning per nome
/// ([`FieldAllocator::intern`]) rende stabile l'ID di una colonna visibile
/// tra nodi analizzati con lo stesso allocatore (usato per le chiavi
/// `sorted_by`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FieldAllocator {
    next: u32,
    by_name: HashMap<String, FieldId>,
}

impl FieldAllocator {
    /// Allocatore che parte da `next` (il planner usa il primo ID libero dopo
    /// gli input del grafo).
    #[must_use]
    pub fn new(next: u32) -> Self {
        Self {
            next,
            by_name: HashMap::new(),
        }
    }

    /// Assegna un nuovo `FieldId` garantito fresco e avanza il cursore.
    pub const fn alloc(&mut self) -> FieldId {
        let id = FieldId(self.next);
        self.next = self.next.saturating_add(1);
        id
    }

    /// Il prossimo ID che verrà assegnato (ispezione, non consuma).
    #[must_use]
    pub const fn peek(&self) -> FieldId {
        FieldId(self.next)
    }

    /// Registra un ID già assegnato (contratti di input) per evitare
    /// collisioni con i futuri [`FieldAllocator::alloc`].
    pub fn observe(&mut self, id: FieldId) {
        self.next = self.next.max(id.0.saturating_add(1));
    }

    /// ID stabile di una colonna propagata: stesso nome → stesso ID.
    pub fn intern(&mut self, name: &str) -> FieldId {
        if let Some(id) = self.by_name.get(name) {
            return *id;
        }
        let id = self.alloc();
        self.by_name.insert(name.to_owned(), id);
        id
    }

    /// Rinomina: il `FieldId` segue la colonna (D16).
    pub fn rename(&mut self, old: &str, new: &str) {
        if let Some(id) = self.by_name.remove(old) {
            self.by_name.insert(new.to_owned(), id);
        }
    }

    /// Colonna derivata: riceve un `FieldId` nuovo, sostituendo l'eventuale
    /// identità precedente associata al nome (D16).
    pub fn derive(&mut self, name: &str) -> FieldId {
        let id = self.alloc();
        self.by_name.insert(name.to_owned(), id);
        id
    }
}

/// Provenienza di una proprietà del contratto (decisione D25).
///
/// Il planner usa come precondizioni semantiche solo proprietà `Proven`;
/// le `Estimated` guidano esclusivamente scelte prestazionali correggibili a
/// runtime. Le proprietà possono cambiare livello nel tempo: una `Declared`
/// può diventare `Proven` tramite validazione dinamica, una `Estimated` può
/// essere aggiornata dal runtime.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PropertyConfidence<T> {
    /// Dichiarata da una fonte esterna (piano, utente), non verificata.
    Declared(T),
    /// Dimostrata: usabile come precondizione semantica.
    Proven(T),
    /// Stimata: solo scelte prestazionali correggibili a runtime.
    Estimated(T),
    /// Assente.
    Unknown,
}

impl<T> PropertyConfidence<T> {
    /// Il valore, se presente a qualunque livello di fiducia.
    pub fn value(&self) -> Option<&T> {
        match self {
            Self::Declared(value) | Self::Proven(value) | Self::Estimated(value) => Some(value),
            Self::Unknown => None,
        }
    }

    /// Il valore solo se `Proven` (unica precondizione semantica ammessa).
    pub fn proven_value(&self) -> Option<&T> {
        match self {
            Self::Proven(value) => Some(value),
            _ => None,
        }
    }

    /// `true` solo per `Proven`.
    pub fn is_proven(&self) -> bool {
        matches!(self, Self::Proven(_))
    }
}

/// Ambito di validità di una proprietà (decisione D25).
///
/// Lo scope conta: "ogni batch è ordinato" non implica "lo stream è
/// ordinato". Una proprietà dataset-wide diventa `Proven(Dataset)` solo al
/// completamento dell'intero input — mai inferenze premature a metà stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PropertyScope {
    /// Deducibile dallo schema (statica, indipendente dai dati).
    Schema,
    /// Vale per il singolo batch.
    Batch,
    /// Vale lungo lo stream di batch di un arco.
    Stream,
    /// Vale sull'intero dataset.
    Dataset,
}

/// Una proprietà tipizzata con provenienza e scope (Architetture.md par. 4.3).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContractProperty<T> {
    pub confidence: PropertyConfidence<T>,
    pub scope: PropertyScope,
}

impl<T> ContractProperty<T> {
    pub fn new(confidence: PropertyConfidence<T>, scope: PropertyScope) -> Self {
        Self { confidence, scope }
    }

    /// `true` solo se la proprietà è `Proven` (precondizione semantica).
    pub fn is_proven(&self) -> bool {
        self.confidence.is_proven()
    }

    /// Il valore, se presente a qualunque livello di fiducia.
    pub fn value(&self) -> Option<&T> {
        self.confidence.value()
    }
}

/// Proprietà tipizzate del contratto, v1 minimale (non un framework generico:
/// nuove proprietà si aggiungono come campi tipizzati).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ContractProperties {
    /// Chiavi di ordinamento dichiarate/dimostrate, come `FieldId` nel
    /// namespace globale del grafo.
    pub sorted_by: Option<ContractProperty<Vec<FieldId>>>,
    /// Cardinalità nota o stimata dell'arco (mai `Proven` in fase 1: non è
    /// dimostrabile dagli header, Architetture.md par. 4.3).
    pub row_count: Option<ContractProperty<u64>>,
}

/// Contratto di un arco del DAG (decisioni D6, D16).
///
/// Ogni arco trasporta `RecordBatch` conformi a questo contratto, inferito a
/// secco dal planner (`analyze_contract` di ogni operazione, Fase 2A-2).
#[derive(Clone, Debug)]
pub struct DataContract {
    pub schema: SchemaRef,
    /// v1: al massimo una colonna geometrica (validato, decisione D16).
    pub geometries: Vec<GeometryColumnContract>,
    pub active_geometry: Option<FieldId>,
    pub properties: ContractProperties,
}

impl DataContract {
    /// Contratto tabellare: nessuna colonna geometrica.
    pub fn tabular(schema: SchemaRef) -> Self {
        Self {
            schema,
            geometries: Vec::new(),
            active_geometry: None,
            properties: ContractProperties::default(),
        }
    }

    /// Costruisce e valida subito il contratto (fail-closed).
    ///
    /// # Errors
    ///
    /// Restituisce `PlenoraError::Schema` se il contratto viola le regole di
    /// [`DataContract::validate`].
    pub fn new(
        schema: SchemaRef,
        geometries: Vec<GeometryColumnContract>,
        active_geometry: Option<FieldId>,
        properties: ContractProperties,
    ) -> Result<Self> {
        let contract = Self {
            schema,
            geometries,
            active_geometry,
            properties,
        };
        contract.validate()?;
        Ok(contract)
    }

    /// Validazione strutturale del contratto.
    ///
    /// Regole v1: al massimo una colonna geometrica (D16); ogni colonna
    /// geometrica deve esistere nello schema con lo stesso nome e la stessa
    /// nullability; `active_geometry`, se presente, deve riferire una delle
    /// colonne geometriche dichiarate.
    ///
    /// # Errors
    ///
    /// Restituisce `PlenoraError::Schema` descrivendo la prima violazione.
    pub fn validate(&self) -> Result<()> {
        if self.geometries.len() > 1 {
            return Err(PlenoraError::Schema(format!(
                "contratto con {} colonne geometriche: la v1 ne ammette al massimo una (D16)",
                self.geometries.len()
            )));
        }
        for geometry in &self.geometries {
            let field = self
                .schema
                .field_with_name(&geometry.name)
                .map_err(|_| {
                    PlenoraError::Schema(format!(
                        "colonna geometrica `{}` ({}) assente dallo schema",
                        geometry.name, geometry.field_id
                    ))
                })?;
            if field.is_nullable() != geometry.nullable {
                return Err(PlenoraError::Schema(format!(
                    "colonna geometrica `{}`: nullability del contratto ({}) diversa dallo schema ({})",
                    geometry.name,
                    geometry.nullable,
                    field.is_nullable()
                )));
            }
        }
        if let Some(active) = self.active_geometry {
            if !self.geometries.iter().any(|g| g.field_id == active) {
                return Err(PlenoraError::Schema(format!(
                    "active_geometry {active} non e' tra le colonne geometriche dichiarate"
                )));
            }
        }
        Ok(())
    }

    /// La colonna geometrica attiva, se presente: `active_geometry` se
    /// dichiarata, altrimenti l'unica geometria del contratto.
    pub fn active_geometry_column(&self) -> Option<&GeometryColumnContract> {
        match self.active_geometry {
            Some(active) => self.geometries.iter().find(|g| g.field_id == active),
            None => self.geometries.first(),
        }
    }
}

/// Statistica di runtime (ADR 5).
///
/// Regola: `prepare` produce sempre un piano valido anche con statistiche
/// completamente assenti (`Unknown` → scelta conservativa); le statistiche
/// `Known`/`Estimated` possono solo migliorare scelte fisiche correggibili;
/// nessuna scelta semantica può dipendere da una statistica.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuntimeStatistic<T> {
    /// Misurata (es. numero di righe da header Arrow IPC file format).
    Known(T),
    /// Stimata: solo scelte migliorative.
    Estimated(T),
    /// Assente: scelta conservativa obbligatoria.
    Unknown,
}

impl<T> RuntimeStatistic<T> {
    /// Il valore, se noto o stimato.
    pub fn value(&self) -> Option<&T> {
        match self {
            Self::Known(value) | Self::Estimated(value) => Some(value),
            Self::Unknown => None,
        }
    }

    /// Il valore solo se misurato (`Known`).
    pub fn known_value(&self) -> Option<&T> {
        match self {
            Self::Known(value) => Some(value),
            _ => None,
        }
    }

    /// `true` solo per `Known`.
    pub fn is_known(&self) -> bool {
        matches!(self, Self::Known(_))
    }
}

impl<T> Default for RuntimeStatistic<T> {
    fn default() -> Self {
        Self::Unknown
    }
}

/// Sequenza logica di un batch (ADR 1).
///
/// Le operazioni parallele ricompongono l'output secondo l'ordine logico
/// assegnato dal piano, mai secondo l'ordine temporale di completamento: la
/// politica `InputOrder` del catalogo significa "ordine logico delle
/// `BatchSequence` in ingresso".
///
/// `source_node` usa l'id testuale del nodo del piano (formato v4);
/// l'eventuale interning in indici compatti è una decisione fisica del
/// planner e non appartiene a questo contratto.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct BatchSequence {
    /// Nodo del DAG che ha prodotto il batch.
    pub source_node: String,
    /// Partizione di input (rami paralleli della stessa sorgente).
    pub input_partition: u32,
    /// Numero di sequenza all'interno della partizione.
    pub sequence_number: u64,
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use serde_json::json;

    use super::*;
    use crate::arrow::schema::{DataType, Field, Schema};
    use crate::crs::CrsKind;

    fn schema(fields: Vec<Field>) -> SchemaRef {
        Arc::new(Schema::new(fields))
    }

    fn projected_crs() -> ResolvedCrs {
        ResolvedCrs::from_resolved_parts(
            "EPSG:32632".to_owned(),
            json!({"type": "ProjectedCRS", "name": "WGS 84 / UTM zone 32N"}),
            CrsKind::Projected,
            Some(1.0),
        )
    }

    fn geometry(field_id: u32, name: &str, nullable: bool) -> GeometryColumnContract {
        GeometryColumnContract {
            field_id: FieldId(field_id),
            name: name.to_owned(),
            crs: projected_crs(),
            dimensions: GeometryDimensions::Xy,
            nullable,
        }
    }

    #[test]
    fn field_allocator_assigns_monotonic_ids_without_consuming_on_peek() {
        let mut allocator = FieldAllocator::new(41);
        assert_eq!(allocator.peek(), FieldId(41));
        assert_eq!(allocator.alloc(), FieldId(41));
        assert_eq!(allocator.alloc(), FieldId(42));
        assert_eq!(allocator.peek(), FieldId(43));
        assert_eq!(FieldAllocator::default().peek(), FieldId(0));
    }

    #[test]
    fn field_allocator_observe_avoids_collisions_with_assigned_ids() {
        let mut allocator = FieldAllocator::default();
        allocator.observe(FieldId(7));
        assert_eq!(allocator.peek(), FieldId(8));
        assert_eq!(allocator.alloc(), FieldId(8));
        // Osservare un id gia' coperto non fa arretrare il cursore.
        allocator.observe(FieldId(3));
        assert_eq!(allocator.peek(), FieldId(9));
    }

    #[test]
    fn field_allocator_intern_rename_and_derive_follow_column_identity() {
        let mut allocator = FieldAllocator::default();
        // Interning: stesso nome -> stesso id, nomi diversi -> id diversi.
        let id = allocator.intern("geom");
        assert_eq!(allocator.intern("geom"), id);
        assert_ne!(allocator.intern("other"), id);

        // Rinomina: l'id segue la colonna (D16).
        allocator.rename("geom", "geometry");
        assert_eq!(allocator.intern("geometry"), id);

        // Derivazione: nuovo id, sostituisce il binding del nome.
        let derived = allocator.derive("geometry");
        assert_ne!(derived, id);
        assert_eq!(allocator.intern("geometry"), derived);
        // Gli id internati non collidono con quelli osservati.
        allocator.observe(FieldId(100));
        assert!(allocator.intern("fresh").0 > 100);
    }

    #[test]
    fn tabular_contract_is_valid_without_geometries() {
        let contract = DataContract::tabular(schema(vec![Field::new("a", DataType::Int64, false)]));
        contract.validate().unwrap();
        assert!(contract.active_geometry_column().is_none());
        assert!(contract.properties.sorted_by.is_none());
    }

    #[test]
    fn single_geometry_contract_is_valid() {
        let contract = DataContract::new(
            schema(vec![
                Field::new("a", DataType::Int64, false),
                Field::new("geom", DataType::Binary, true),
            ]),
            vec![geometry(7, "geom", true)],
            None,
            ContractProperties::default(),
        )
        .unwrap();
        let active = contract.active_geometry_column().unwrap();
        assert_eq!(active.field_id, FieldId(7));
        assert_eq!(active.name, "geom");
    }

    #[test]
    fn more_than_one_geometry_is_rejected_in_v1() {
        let result = DataContract::new(
            schema(vec![
                Field::new("g1", DataType::Binary, true),
                Field::new("g2", DataType::Binary, true),
            ]),
            vec![geometry(1, "g1", true), geometry(2, "g2", true)],
            None,
            ContractProperties::default(),
        );
        assert!(matches!(result, Err(PlenoraError::Schema(_))));
    }

    #[test]
    fn geometry_column_must_exist_with_matching_nullability() {
        let missing = DataContract::new(
            schema(vec![Field::new("a", DataType::Int64, false)]),
            vec![geometry(1, "geom", true)],
            None,
            ContractProperties::default(),
        );
        assert!(matches!(missing, Err(PlenoraError::Schema(_))));

        let nullability_mismatch = DataContract::new(
            schema(vec![Field::new("geom", DataType::Binary, false)]),
            vec![geometry(1, "geom", true)],
            None,
            ContractProperties::default(),
        );
        assert!(matches!(nullability_mismatch, Err(PlenoraError::Schema(_))));
    }

    #[test]
    fn active_geometry_must_reference_a_declared_geometry() {
        let result = DataContract::new(
            schema(vec![Field::new("geom", DataType::Binary, true)]),
            vec![geometry(1, "geom", true)],
            Some(FieldId(99)),
            ContractProperties::default(),
        );
        assert!(matches!(result, Err(PlenoraError::Schema(_))));

        let ok = DataContract::new(
            schema(vec![Field::new("geom", DataType::Binary, true)]),
            vec![geometry(1, "geom", true)],
            Some(FieldId(1)),
            ContractProperties::default(),
        )
        .unwrap();
        assert_eq!(ok.active_geometry_column().unwrap().field_id, FieldId(1));
    }

    #[test]
    fn property_confidence_exposes_proven_values_only_for_proven() {
        let declared: PropertyConfidence<u64> = PropertyConfidence::Declared(10);
        let proven = PropertyConfidence::Proven(10);
        let estimated = PropertyConfidence::Estimated(10);
        let unknown: PropertyConfidence<u64> = PropertyConfidence::Unknown;

        assert_eq!(declared.value(), Some(&10));
        assert!(!declared.is_proven());
        assert_eq!(declared.proven_value(), None);
        assert!(proven.is_proven());
        assert_eq!(proven.proven_value(), Some(&10));
        assert_eq!(estimated.proven_value(), None);
        assert_eq!(unknown.value(), None);
    }

    #[test]
    fn contract_property_combines_confidence_and_scope() {
        let property = ContractProperty::new(
            PropertyConfidence::Proven(vec![FieldId(0), FieldId(3)]),
            PropertyScope::Stream,
        );
        assert!(property.is_proven());
        assert_eq!(property.scope, PropertyScope::Stream);
        assert_eq!(property.value().unwrap().len(), 2);

        let unknown: ContractProperty<Vec<FieldId>> =
            ContractProperty::new(PropertyConfidence::Unknown, PropertyScope::Dataset);
        assert!(!unknown.is_proven());
        assert_eq!(unknown.value(), None);
    }

    #[test]
    fn runtime_statistic_distinguishes_known_estimated_unknown() {
        let known = RuntimeStatistic::Known(42_u64);
        let estimated = RuntimeStatistic::Estimated(42_u64);
        let unknown: RuntimeStatistic<u64> = RuntimeStatistic::Unknown;

        assert!(known.is_known());
        assert_eq!(known.known_value(), Some(&42));
        assert!(!estimated.is_known());
        assert_eq!(estimated.value(), Some(&42));
        assert_eq!(estimated.known_value(), None);
        assert_eq!(unknown.value(), None);
        assert_eq!(RuntimeStatistic::<u64>::default(), RuntimeStatistic::Unknown);
    }

    #[test]
    fn batch_sequence_carries_logical_order() {
        let sequence = BatchSequence {
            source_node: "n0".to_owned(),
            input_partition: 2,
            sequence_number: 17,
        };
        assert_eq!(sequence.source_node, "n0");
        assert_eq!(sequence.input_partition, 2);
        assert_eq!(sequence.sequence_number, 17);
    }
}
