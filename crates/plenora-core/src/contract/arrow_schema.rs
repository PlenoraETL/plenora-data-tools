//! Autorita' unica della conversione fra schemi Arrow e contratti dati.
//!
//! Due operazioni **non speculari**: [`contract_from_arrow_schema`] preserva
//! cio' che lo schema dichiara (un CRS assente resta [`ContractCrs::Missing`],
//! una dichiarazione incoerente resta `DeclaredUnresolved`), mentre
//! `arrow_schema_from_contract` emette i metadati canonici e rifiuta i
//! conflitti. Renderle simmetriche vorrebbe dire normalizzare in lettura o
//! accettare l'ambiguita' in scrittura.
//!
//! Le due direzioni stanno qui, in un punto solo, perché ogni chiamante
//! (`plenora-io` per `GeoParquet` e IPC, i test dei kernel) deve interpretare
//! uno schema allo stesso modo. Il modulo lavora solo sullo schema, senza
//! leggere dati; il contesto di file e input resta al chiamante.

use std::sync::Arc;

use crate::arrow::schema::Schema;
use crate::arrow::schema::{DataType, SchemaRef};
use crate::contract::arrow_metadata::{
    canonical_geometry_metadata, canonical_schema_version_metadata, read_contract_version,
    read_geometry_contract_keys, strip_decided_crs_declarations, CanonicalGeometryKeys,
    GeometryMetadataDetails, GEOARROW_EXTENSION_KEY, GEOARROW_WKB_EXTENSION, GEO_METADATA_KEY,
    PLENORA_GEOMETRY_AXIS_ORDER_KEY, PLENORA_GEOMETRY_CRS_RESOLUTION_KEY,
    PLENORA_GEOMETRY_NAMESPACE_PREFIX, PLENORA_GEOMETRY_SRID_KEY,
};
use crate::contract::{
    ContractCrs, ContractProperties, ContractProperty, CrsDefinitionFormat, CrsResolution,
    DataContract, FieldId, GeometryColumnContract, GeometryDimensions, PropertyConfidence,
    PropertyScope,
};
use crate::crs::ResolvedCrs;
use crate::PlenoraError;

/// Come si risolve una definizione di CRS in un CRS risolto.
///
/// È un parametro perché la risoluzione è una scelta del chiamante: qui
/// l'implementazione è [`crate::crs::resolve_crs`] (la tabella dei CRS
/// integrati), e un chiamante che risolve i CRS altrove passa la propria.
/// Così chi legge e chi scrive una tabella usano esplicitamente lo stesso
/// risolutore, invece di dipendere dalla build.
pub type CrsResolver = fn(&str, &'static str) -> Result<ResolvedCrs, crate::crs::CrsError>;

/// Errore di contratto: schema e metadati non dicono cio' che devono dire.
///
/// `InvalidPlan` e non `Schema`, e non è una svista: è la variante che
/// questo percorso dichiara, e chi la osserva la osserva da fuori. Cambiarla
/// sarebbe una modifica di semantica (la categoria cambia); se `Schema`
/// fosse la categoria giusta, va cambiata come rottura dichiarata, non di
/// straforo.
fn errore_di_contratto(messaggio: impl Into<String>) -> PlenoraError {
    PlenoraError::InvalidPlan(messaggio.into())
}

/// Definizione CRS dal metadato `geo` di una colonna `GeoArrow`: stringa
/// `authority:code` oppure PROJJSON come oggetto (serializzato compatto).
///
/// Un metadato assente o senza chiave `crs` restituisce `None`, cioè
/// [`ContractCrs::Missing`]; un metadato malformato resta un errore
/// («illeggibile» non è «assente»).
///
/// # Errors
///
/// [`PlenoraError::DataMapping`] (`json error`) se il metadato `geo` non è
/// JSON valido; [`PlenoraError::InvalidPlan`] se la chiave `crs` non è né
/// una stringa né un oggetto.
pub fn crs_definition_from_metadata(
    field_name: &str,
    geo_metadata: Option<&String>,
) -> Result<Option<String>, PlenoraError> {
    let Some(raw) = geo_metadata else {
        return Ok(None);
    };
    let value: serde_json::Value = serde_json::from_str(raw)?;
    match value.get("crs") {
        None => Ok(None),
        Some(serde_json::Value::String(definition)) => Ok(Some(definition.clone())),
        Some(object @ serde_json::Value::Object(_)) => Ok(Some(serde_json::to_string(object)?)),
        Some(_) => Err(errore_di_contratto(format!(
            "colonna geometria `{field_name}`: metadato `{GEO_METADATA_KEY}` senza \
             chiave `crs` valida"
        ))),
    }
}

/// Il contratto dichiarato da uno schema Arrow già letto, senza leggere
/// dati.
///
/// Una colonna è geometrica se porta l'estensione `geoarrow.wkb` o almeno
/// una chiave canonica `plenora.geometry.*` (le chiavi canoniche bastano da
/// sole). Il suo CRS si deduce con [`contract_crs_from_keys`]; la colonna
/// geometrica, se c'è, è la geometria attiva, con `FieldId` provvisorio 0
/// (il chiamante lo rimappa con il proprio [`crate::contract::FieldAllocator`]).
///
/// # Errors
///
/// [`PlenoraError::InvalidPlan`] se i metadati sono incoerenti (estensione
/// non supportata, metadato `geo` senza estensione, colonna geometria non
/// `Binary`) o se le chiavi canoniche si contraddicono;
/// [`PlenoraError::Unsupported`] per una versione di contratto successiva a
/// quella supportata; [`PlenoraError::Crs`] per un CRS dichiarato che il
/// risolutore non risolve; [`PlenoraError::Schema`] se il contratto che ne
/// risulta non supera [`DataContract::validate`] (per esempio più di una
/// colonna geometrica).
pub fn contract_from_arrow_schema(
    schema: SchemaRef,
    resolve_crs: CrsResolver,
) -> Result<DataContract, PlenoraError> {
    // La versione del contratto vive nei metadati dello schema: una versione
    // successiva a quella supportata si rifiuta prima di leggere i campi.
    read_contract_version(&schema)?;
    let mut geometries = Vec::new();
    for field in schema.fields() {
        let extension = field.metadata().get(GEOARROW_EXTENSION_KEY);
        let geo_metadata = field.metadata().get(GEO_METADATA_KEY);
        if let Some(extension) = extension {
            if extension != GEOARROW_WKB_EXTENSION {
                return Err(errore_di_contratto(format!(
                    "colonna `{}`: estensione `{extension}` non supportata \
                     (attesa `{GEOARROW_WKB_EXTENSION}`)",
                    field.name()
                )));
            }
        } else {
            if geo_metadata.is_some() {
                return Err(errore_di_contratto(format!(
                    "colonna `{}`: metadato `{GEO_METADATA_KEY}` senza estensione \
                     `{GEOARROW_EXTENSION_KEY}`: metadati incoerenti",
                    field.name()
                )));
            }
            // Le chiavi canoniche sono autosufficienti: il campo si dichiara
            // colonna geometrica da solo.
            let canonical = field
                .metadata()
                .keys()
                .any(|key| key.starts_with(PLENORA_GEOMETRY_NAMESPACE_PREFIX));
            if !canonical {
                continue;
            }
        }
        if field.data_type() != &DataType::Binary {
            return Err(errore_di_contratto(format!(
                "colonna geometria `{}` di tipo {}, atteso Binary",
                field.name(),
                field.data_type()
            )));
        }
        let keys = read_geometry_contract_keys(field)?;
        let crs = contract_crs_from_keys(field.name(), geo_metadata, &keys, resolve_crs)?;
        geometries.push(geometry_contract_from_field(field, crs, &keys));
    }
    let active_geometry = if geometries.is_empty() {
        None
    } else {
        Some(FieldId(0))
    };
    DataContract::new(
        schema,
        geometries,
        active_geometry,
        ContractProperties::default(),
    )
}

/// Lo stato CRS del contratto, dedotto dalle chiavi canoniche.
///
/// Le chiavi arrivano già completate da [`read_geometry_contract_keys`].
/// Un'incoerenza dichiarata non si risolve: si preserva come
/// [`ContractCrs::DeclaredUnresolved`] con le dichiarazioni originali.
///
/// Regole, in ordine:
///
/// 1. `crs_resolution = declared_unresolved` con almeno una rappresentazione:
///    preservato com'è, senza chiamare il risolutore. Basta anche il solo
///    `srid` (l'autorità non si inventa): `crs_id`/`definition` restano
///    assenti, mai sintetizzati;
/// 2. conflitti decidibili senza risolutore, che danno `DeclaredUnresolved`:
///    (2a) solo per input **non dichiarati** (`crs_resolution` assente),
///    `crs_id` e `crs_definition` co-presenti, perché il loro accordo non è
///    decidibile testualmente; (2b) sempre, anche con `resolved`, un
///    `crs_id` `authority:code` con codice numerico diverso da `srid`. Il
///    limite della (2a) evita di rovesciare la dichiarazione del produttore;
/// 3. una rappresentazione (canonica o legacy `geo.crs`), o `resolved`
///    dichiarato: risoluzione con il risolutore dato (con
///    [`crate::crs::resolve_crs`], la tabella dei CRS integrati), e un
///    fallimento resta un errore `Crs` (limite dichiarato: chi non
///    garantisce la risoluzione dichiara `declared_unresolved`). Con
///    `resolved` e sia `crs_id` sia `crs_definition` segue
///    [`verify_declared_coherence`]. Con la tabella integrata una
///    definizione WKT o PROJJSON non si risolve, e quell'input fallisce con
///    errore `Crs`, come uno a rappresentazione singola;
/// 4. nessuna rappresentazione: [`ContractCrs::Missing`], salvo la
///    contraddizione `resolved`/`declared_unresolved` senza
///    rappresentazioni, che resta errore.
///
/// # Errors
///
/// [`PlenoraError::InvalidPlan`] se la dichiarazione è contraddittoria
/// (`crs_resolution` valorizzata senza alcuna rappresentazione) o il
/// metadato `geo` ha una chiave `crs` malformata;
/// [`PlenoraError::Crs`] se il risolutore non risolve la definizione.
pub fn contract_crs_from_keys(
    field_name: &str,
    geo_metadata: Option<&String>,
    keys: &CanonicalGeometryKeys,
    resolve_crs: CrsResolver,
) -> Result<ContractCrs, PlenoraError> {
    let crs_id = keys.crs_id.clone();
    let definition = keys.crs_definition.clone();
    // (1) Incoerenza dichiarata dal produttore: preservata, mai risolta.
    // Anche il solo SRID numerico è una rappresentazione (dopo definizione e
    // identificatore): senza `crs_id`/`definition` lo stato li porta
    // assenti, mai sintetizzati.
    if keys.crs_resolution == Some(CrsResolution::DeclaredUnresolved)
        && (crs_id.is_some() || definition.is_some() || keys.srid.is_some())
    {
        return Ok(ContractCrs::DeclaredUnresolved {
            crs_id,
            definition,
            definition_format: keys.crs_definition_format,
        });
    }
    // (2) Un conflitto numerico decidibile fra identificatore e SRID non puo'
    // essere nascosto da una dichiarazione `resolved`: si preservano tutte
    // le rappresentazioni originali e non si invoca il backend CRS.
    if let (Some(id), Some(srid)) = (&crs_id, keys.srid) {
        if authority_code(id).is_some_and(|code| code != srid) {
            return Ok(ContractCrs::DeclaredUnresolved {
                crs_id,
                definition,
                definition_format: keys.crs_definition_format,
            });
        }
    }
    // (3) La co-presenza di due rappresentazioni risolvibili resta
    // indecidibile per gli input che non dichiarano uno stato.
    if keys.crs_resolution.is_none() {
        // Due rappresentazioni risolvibili co-presenti: accordo non
        // decidibile, il centro non sceglie.
        if crs_id.is_some() && definition.is_some() {
            return Ok(ContractCrs::DeclaredUnresolved {
                crs_id,
                definition,
                definition_format: keys.crs_definition_format,
            });
        }
    }
    // (4) La rappresentazione completata (canonica o legacy) alimenta la
    // stessa risoluzione di sempre; la verifica di coerenza post-risoluzione
    // riguarda il solo caso `resolved` dichiarato con doppia
    // rappresentazione.
    if let Some(definition_text) = definition.as_deref().or(crs_id.as_deref()) {
        let resolved = resolve_crs(definition_text, "crs")?;
        if keys.crs_resolution == Some(CrsResolution::Resolved) {
            if let (Some(id), Some(text)) = (crs_id.as_deref(), definition.as_deref()) {
                return Ok(verify_declared_coherence(
                    resolved,
                    id,
                    text,
                    keys.crs_definition_format,
                    resolve_crs,
                ));
            }
        }
        return Ok(ContractCrs::Resolved(resolved));
    }
    if let Some(definition) = crs_definition_from_metadata(field_name, geo_metadata)? {
        return Ok(ContractCrs::Resolved(resolve_crs(&definition, "crs")?));
    }
    // (5) Mai collassare una dichiarazione esplicita su `missing`:
    // `resolved`/`declared_unresolved` senza alcuna rappresentazione è una
    // contraddizione, non un'assenza.
    if let Some(resolution) = keys.crs_resolution {
        if resolution != CrsResolution::Missing {
            return Err(errore_di_contratto(format!(
                "colonna geometria `{field_name}`: chiave \
                 `{PLENORA_GEOMETRY_CRS_RESOLUTION_KEY}` dichiara `{resolution}` ma \
                 nessun CRS e' dichiarato in alcuna rappresentazione accettata"
            )));
        }
    }
    Ok(ContractCrs::Missing)
}

/// Verifica di coerenza decidibile dopo la risoluzione.
///
/// Per un input `resolved` con doppia rappresentazione (`crs_id` e
/// `crs_definition`) risolve anche `crs_id` e confronta la coppia
/// autorità+codice dei due CRS risolti: uguali danno `Resolved` (per
/// esempio un WKT Monte Mario che risolve a EPSG:3003); diversi, o
/// confronto non decidibile (mai arbitrato), danno `DeclaredUnresolved`
/// con le dichiarazioni originali.
pub fn verify_declared_coherence(
    resolved: ResolvedCrs,
    crs_id: &str,
    definition: &str,
    definition_format: Option<CrsDefinitionFormat>,
    resolve_crs: CrsResolver,
) -> ContractCrs {
    let resolved_identifier = resolved.authority_identifier();
    let simple_identifier = crate::crs::authority_code_identifier(crs_id);
    let coherent = simple_identifier.map_or_else(
        || {
            resolve_crs(crs_id, "crs").is_ok_and(|declared| {
                matches!(
                    (declared.authority_identifier(), resolved_identifier),
                    (Some(left), Some(right))
                        if left.0.eq_ignore_ascii_case(right.0) && left.1 == right.1
                )
            })
        },
        |declared| {
            resolved_identifier.is_some_and(|canonical| {
                declared.0.eq_ignore_ascii_case(canonical.0) && declared.1 == canonical.1
            })
        },
    );
    if coherent {
        return ContractCrs::Resolved(resolved);
    }
    ContractCrs::DeclaredUnresolved {
        crs_id: Some(crs_id.to_owned()),
        definition: Some(definition.to_owned()),
        definition_format,
    }
}

/// Codice numerico di un identificatore `authority:code` (es. `EPSG:4326`).
///
/// `None` per ogni altra forma: il confronto con `srid` non è decidibile.
/// Delega a [`crate::crs::authority_code_srid`], l'unica fonte del
/// parsing.
#[must_use]
pub fn authority_code(crs_id: &str) -> Option<u32> {
    crate::crs::authority_code_srid(crs_id)
}

/// Il contratto di una colonna geometria, dalle chiavi gia' lette.
///
/// Le chiavi arrivano da [`read_geometry_contract_keys`], che ha già
/// rifiutato le chiavi malformate o in conflitto e completato quelle
/// deducibili. Dimensionalità ed encoding assenti danno `Unknown`/`None`,
/// mai `Xy`. Una coppia `types_declaration`/`types` entra con confidence
/// `Declared`; assente, vale [`GeometryColumnContract::undeclared_types`].
/// Il `FieldId` è provvisorio (0): lo rimappa il chiamante.
pub fn geometry_contract_from_field(
    field: &crate::arrow::schema::Field,
    crs: ContractCrs,
    keys: &CanonicalGeometryKeys,
) -> GeometryColumnContract {
    let types =
        keys.types
            .as_ref()
            .map_or_else(GeometryColumnContract::undeclared_types, |types| {
                ContractProperty::new(
                    PropertyConfidence::Declared(types.clone()),
                    PropertyScope::Schema,
                )
            });
    GeometryColumnContract {
        field_id: FieldId(0),
        name: field.name().clone(),
        crs,
        dimensions: keys.dimensions.unwrap_or(GeometryDimensions::Unknown),
        encoding: keys.encoding,
        nullable: field.is_nullable(),
        types,
    }
}

// ---------------------------------------------------------------------------
// Contratto -> Arrow: si emette il canone e si rifiutano i conflitti, con la
// regola opposta alla lettura (vedi il doc del modulo).
// ---------------------------------------------------------------------------

/// Lo schema Arrow che un contratto dichiara, in forma canonica.
///
/// Aggiunge allo schema il blocco canonico `plenora.geometry.*` di ogni
/// colonna geometrica e la versione del contratto
/// (`plenora.contract.version`). Le analisi costruiscono i campi con le sole
/// chiavi `GeoArrow`, che restano; il blocco canonico si aggiunge qui, in un
/// punto solo.
///
/// - Le chiavi di [`canonical_geometry_metadata`] si fondono nel campo
///   omonimo. Con `GeometryMetadataDetails::default()` si completa l'assente:
///   `axis_order` e `srid` sono dedotti dalla definizione d'autorità
///   ([`ResolvedCrs::authority_axis_order`]/[`ResolvedCrs::authority_srid`]),
///   e `axis_order` vale `unknown` solo se la definizione non determina gli
///   assi. `geo.reproject` fa eccezione per `axis_order`: lo emette già
///   l'analisi, con l'ordine normalizzato della riproiezione.
/// - Una chiave canonica già presente con valore diverso è un errore, mai
///   una sovrascrittura; uguale è idempotente. Le chiavi che un'operazione
///   riscrive per mestiere sono già rimosse a monte, nel contratto
///   dell'analisi. Eccezioni: su `axis_order` e `srid` una chiave di
///   lineage presente vince sempre, qualunque valore emetta il contratto;
///   `crs_resolution = resolved` diventa `declared_unresolved` quando il
///   contratto porta un'incoerenza (unica sovrascrittura, in una sola
///   direzione).
/// - `plenora.contract.version` si aggiunge solo se almeno un campo porta
///   chiavi canoniche; uno schema senza geometrie resta invariato.
/// - Una colonna geometrica del contratto assente dallo schema è un errore.
///
/// # Errors
///
/// `PlenoraError::InvalidPlan` per chiave canonica preesistente divergente
/// o colonna geometrica del contratto assente nello schema.
pub fn arrow_schema_from_contract(contract: &DataContract) -> Result<SchemaRef, PlenoraError> {
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
        // Con un CRS deciso dal piano (`ResolvedByDecision`) le
        // dichiarazioni della sorgente sono SOSTITUITE, non fuse: il
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
                    // `axis_order` e `srid` si completano solo se assenti:
                    // una chiave di lineage presente vince con qualunque
                    // valore emesso, anche dedotto dall'autorità, così la
                    // deduzione non diventa un falso conflitto su un
                    // passaggio che non tocca il CRS.
                    if key == PLENORA_GEOMETRY_AXIS_ORDER_KEY || key == PLENORA_GEOMETRY_SRID_KEY {
                        continue;
                    }
                    // Un'incoerenza CRS rilevata si dichiara
                    // (`declared_unresolved`) invece di propagare il
                    // `resolved` del produttore. Unica sovrascrittura ammessa
                    // su una chiave canonica, in una sola direzione; con una
                    // decisione del piano le dichiarazioni della sorgente
                    // sono già rimosse (`strip_decided_crs_declarations`).
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
    // La versione accompagna le chiavi canoniche; qui almeno un campo le
    // porta (guardia in testa e conteggio sopra).
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
