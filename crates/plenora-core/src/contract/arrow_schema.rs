//! Autorita' unica della conversione fra schemi Arrow e contratti dati.
//!
//! Due operazioni **non speculari**: [`contract_from_arrow_schema`] preserva
//! cio' che lo schema dichiara (un CRS assente resta [`ContractCrs::Missing`],
//! una dichiarazione incoerente resta `DeclaredUnresolved`), mentre
//! `arrow_schema_from_contract` emette i metadati canonici e rifiuta i
//! conflitti. Renderle simmetriche vorrebbe dire normalizzare in lettura o
//! accettare l'ambiguita' in scrittura.
//!
//! Le due direzioni stanno qui, e non presso la CLI e l'executor, perche'
//! supervisore e worker isolato devono interpretare uno schema allo stesso
//! modo. Il modulo lavora solo sullo schema, senza leggere dati; il contesto
//! di file e input resta nella CLI.

use std::collections::HashMap;
use std::sync::Arc;

use crate::arrow::schema::Schema;
use crate::arrow::schema::{DataType, Field, SchemaRef};
use crate::contract::arrow_metadata::{
    canonical_geometry_metadata, canonical_geometry_srid, canonical_schema_version_metadata,
    read_contract_version, read_geometry_contract_keys, strip_decided_crs_declarations,
    CanonicalGeometryKeys, GeometryMetadataDetails, GEOARROW_EXTENSION_KEY, GEOARROW_WKB_EXTENSION,
    GEO_METADATA_KEY, PLENORA_GEOMETRY_AXIS_ORDER_KEY, PLENORA_GEOMETRY_CRS_RESOLUTION_KEY,
    PLENORA_GEOMETRY_NAMESPACE_PREFIX, PLENORA_GEOMETRY_SRID_KEY,
};
use crate::contract::{
    ContractCrs, ContractProperties, ContractProperty, CrsDefinitionFormat, CrsResolution,
    DataContract, FieldId, GeometryColumnContract, GeometryDimensions, GeometryTypesProperty,
    PropertyConfidence, PropertyScope,
};
use crate::crs::{required_definition, validate_requirement, ResolvedCrs};
use crate::PlenoraError;

/// Come si risolve una definizione di CRS in un CRS risolto.
///
/// E' un parametro perche' le implementazioni sono due ([`crate::crs`] e
/// quella con backend PROJ in `plenora-kernels-geo`, dietro feature) e
/// `plenora-core` non dipende dai kernel. Cosi' supervisore e worker passano
/// esplicitamente lo stesso risolutore, invece di dipendere dalla build.
pub type CrsResolver = fn(&str, &'static str) -> Result<ResolvedCrs, crate::crs::CrsError>;

/// Errore di contratto: schema e metadati non dicono cio' che devono dire.
///
/// `InvalidPlan` e non `Schema`, e non e' una svista: e' la variante che
/// questo percorso dichiara, e chi la osserva la osserva da fuori. Cambiarla
/// sarebbe una modifica di semantica — l'exit code della CLI e' una
/// proiezione della categoria, quindi 2 invece di 3. Se `Schema` fosse la
/// categoria giusta, va cambiata come rottura dichiarata, non di straforo.
fn errore_di_contratto(messaggio: impl Into<String>) -> PlenoraError {
    PlenoraError::InvalidPlan(messaggio.into())
}

/// Definizione CRS dal metadato `geo` di una colonna `GeoArrow`: stringa
/// `authority:code` oppure PROJJSON come oggetto (serializzato compatto).
///
/// Un metadato assente o senza chiave `crs` restituisce `None`, cioe'
/// [`ContractCrs::Missing`] (R4.6.3); un metadato malformato resta un errore
/// (R5.1: «illeggibile» non e' «assente»).
///
/// # Errors
///
/// [`PlenoraError::InvalidPlan`] se il metadato `geo` non e' JSON valido, se
/// la chiave `crs` non e' un oggetto, o se la definizione supera il tetto
/// dichiarato.
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

/// Scoperta da schema Arrow gia' letto (seam di test: nessun file toccato).
/// Le regole sono quelle di [`discover_input_contract`].
///
/// # Errors
///
/// [`PlenoraError::InvalidPlan`] se i metadati sono incoerenti — estensione
/// non supportata, metadato `geo` senza estensione, colonna geometria non
/// `Binary` — o se le chiavi canoniche si contraddicono.
pub fn contract_from_arrow_schema(
    schema: SchemaRef,
    resolve_crs: CrsResolver,
) -> Result<DataContract, PlenoraError> {
    // Gate R2.5: la versione del protocollo vive nei metadati dello schema.
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
            // (1c) le chiavi canoniche sono autosufficienti (tabella §2):
            // il campo si dichiara colonna geometrica da solo.
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
/// Lettura di contratto completata (R2.7). Senza una decisione esplicita del
/// piano un'incoerenza dichiarata non si risolve: si preserva come
/// [`ContractCrs::DeclaredUnresolved`] con le dichiarazioni originali (R4.6.3).
///
/// Regole, in ordine (piano-v5.md#contratti-di-input, emendamento 2026-07-31,
/// classe A):
///
/// 1. `crs_resolution = declared_unresolved` con almeno una rappresentazione:
///    preservato com'e', senza chiamare il backend (quindi mai
///    `BackendUnavailable`). Basta anche il solo `srid` (R4.4: l'autorita' non
///    si inventa): `crs_id`/`definition` restano assenti, mai sintetizzati;
/// 2. conflitti decidibili senza backend, che danno `DeclaredUnresolved`:
///    (2a) solo per input **non dichiarati** (`crs_resolution` assente),
///    `crs_id` e `crs_definition` co-presenti, perche' il loro accordo non e'
///    decidibile testualmente (R2.7); (2b) sempre, anche con `resolved`, un
///    `crs_id` `authority:code` con codice numerico diverso da `srid`
///    (R4.3.1). Il limite della (2a) evita di rovesciare la dichiarazione del
///    produttore;
/// 3. una rappresentazione (canonica o legacy `geo.crs`), o `resolved`
///    dichiarato: risoluzione contro PROJ, e un fallimento resta un errore
///    `Crs` (limite dichiarato: chi non garantisce la risoluzione dichiara
///    `declared_unresolved`). Con `resolved` e sia `crs_id` sia
///    `crs_definition` segue [`verify_declared_coherence`]. Senza
///    `proj-backend` quell'input fallisce con errore `Crs`, come uno a
///    rappresentazione singola;
/// 4. nessuna rappresentazione: [`ContractCrs::Missing`] (R4.4), salvo la
///    contraddizione R4.1 (`resolved`/`declared_unresolved` senza
///    rappresentazioni), che resta errore.
///
/// # Errors
///
/// [`PlenoraError::InvalidPlan`] se la dichiarazione e' contraddittoria:
/// `crs_resolution` valorizzata senza alcuna rappresentazione, oppure una
/// definizione che non si risolve dove il produttore la dichiara risolta.
pub fn contract_crs_from_keys(
    field_name: &str,
    geo_metadata: Option<&String>,
    keys: &CanonicalGeometryKeys,
    resolve_crs: CrsResolver,
) -> Result<ContractCrs, PlenoraError> {
    let crs_id = keys.crs_id.clone();
    let definition = keys.crs_definition.clone();
    // (1) Incoerenza dichiarata dal produttore: preservata, mai risolta.
    // R4.3.1: anche il solo SRID numerico e' una rappresentazione (dopo
    // definizione e identificatore) — senza `crs_id`/`definition` lo stato
    // li porta assenti (R4.4: mai sintetizzarli).
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
    // (5) R4.1: mai collassare una dichiarazione esplicita su `missing` —
    // `resolved`/`declared_unresolved` senza alcuna rappresentazione e' una
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
/// Per un input `resolved` con doppia rappresentazione
/// (piano-v5.md#contratti-di-input, emendamento 2026-07-31, classe A) risolve
/// anche `crs_id` e confronta la coppia autorita'+codice dei due canonical:
/// uguali danno `Resolved` (per esempio un WKT Monte Mario che risolve a
/// EPSG:3003); diversi, o confronto non decidibile (R2.7, mai arbitrato),
/// danno `DeclaredUnresolved` con le dichiarazioni originali.
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
/// `None` per ogni altra forma: il confronto con `srid` non e' decidibile.
/// Unica fonte condivisa del parsing (piano-v5.md#contratti-di-input,
/// emendamento 2026-07-31), usata anche dalla deduzione `srid` del percorso
/// legacy in `arrow_adapter`.
#[must_use]
pub fn authority_code(crs_id: &str) -> Option<u32> {
    crate::crs::authority_code_srid(crs_id)
}

/// Il contratto di una colonna geometria, dalle chiavi gia' lette.
///
/// Le chiavi arrivano da [`read_geometry_contract_keys`], che ha gia'
/// applicato il fail-closed R2.6 e il completamento R2.7. Dimensionalita' ed
/// encoding assenti danno `Unknown`/`None`, mai `Xy` (R3.4). Una coppia
/// `types_declaration`/`types` entra con confidence `Declared`; assente, vale
/// [`GeometryColumnContract::undeclared_types`] (R3.4.1). Il `FieldId` e'
/// provvisorio (rimappato dal planner, D16).
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
/// Aggiunge allo schema il blocco canonico R2.2 di ogni colonna geometrica e
/// la versione di protocollo R2.5. Gli `analyze_contract` costruiscono i
/// campi con le sole chiavi `GeoArrow` legacy, che restano (R2.6); il blocco
/// canonico si aggiunge qui, in un punto solo.
///
/// - Le chiavi di [`canonical_geometry_metadata`] si fondono nel campo
///   omonimo. Con `GeometryMetadataDetails::default()` si completa l'assente
///   (R2.7, piano-v5.md#contratti-di-input, emendamento 2026-07-31):
///   `axis_order` e `srid` sono dedotti dalla definizione d'autorita'
///   ([`ResolvedCrs::authority_axis_order`]/[`ResolvedCrs::authority_srid`]),
///   e `axis_order` vale `unknown` solo se la definizione non determina gli
///   assi. `geo.reproject` fa eccezione per `axis_order`: lo emette gia'
///   l'analisi, con l'ordine normalizzato prodotto dal backend.
/// - R2.6: una chiave canonica gia' presente con valore diverso e' un
///   errore, mai una sovrascrittura; uguale e' idempotente. Le chiavi che
///   un'operazione riscrive di mestiere (piano-v5.md#contratti-di-input,
///   decisione 8) sono gia' rimosse a monte, nel contratto dell'analisi.
///   Eccezioni: su `axis_order` e `srid` una chiave di lineage presente vince
///   sempre, qualunque valore emetta il contratto; `crs_resolution =
///   resolved` diventa `declared_unresolved` quando il contratto porta
///   un'incoerenza (R4.6.4, unica sovrascrittura, in una sola direzione).
/// - R2.5: `plenora.contract.version` si aggiunge solo se almeno un campo
///   porta chiavi canoniche; uno schema senza geometrie resta invariato.
/// - Una colonna geometrica del contratto assente dallo schema e' un errore.
///
/// # Errors
///
/// `PlenoraError::InvalidPlan` per chiave canonica preesistente divergente
/// (R2.6) o colonna geometrica del contratto assente nello schema.
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
                    // `axis_order` e `srid` si completano solo se assenti
                    // (R2.7): una chiave di lineage presente vince con
                    // qualunque valore emesso, anche dedotto dall'autorita',
                    // cosi' la deduzione non diventa un falso conflitto R2.6
                    // su un passthrough (R2.4).
                    if key == PLENORA_GEOMETRY_AXIS_ORDER_KEY || key == PLENORA_GEOMETRY_SRID_KEY {
                        continue;
                    }
                    // R4.6.4: un'incoerenza CRS rilevata si dichiara
                    // (`declared_unresolved`) invece di propagare il
                    // `resolved` del produttore. Unica sovrascrittura ammessa
                    // su una chiave canonica, in una sola direzione
                    // (piano-v5.md#contratti-di-input, decisione 7); con una
                    // decisione del piano le dichiarazioni della sorgente
                    // sono gia' rimosse (`strip_decided_crs_declarations`).
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
