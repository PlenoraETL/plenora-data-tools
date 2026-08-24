//! Autorita' unica della conversione fra schemi Arrow e contratti dati.
//!
//! Due operazioni, **non speculari**:
//!
//! - [`contract_from_arrow_schema`] legge uno schema e ne ricava il contratto.
//!   In ingresso si PRESERVA cio' che c'e': un CRS assente resta
//!   [`ContractCrs::Missing`], una dichiarazione incoerente resta
//!   `DeclaredUnresolved`. Non si arbitra e non si inventa.
//! - `arrow_schema_from_contract` (nel percorso di pubblicazione) emette i
//!   metadati canonici e RIFIUTA i conflitti.
//!
//! L'asimmetria e' voluta: leggere e scrivere sono due responsabilita'
//! diverse, e imporre la simmetria significherebbe o normalizzare in lettura
//! (perdendo cio' che la sorgente dichiarava) o accettare l'ambiguita' in
//! scrittura.
//!
//! # Perche' l'autorita' e' qui
//!
//! Prima la direzione Arrow -> contratto viveva nella CLI e la direzione
//! contratto -> Arrow nell'executor: due componenti che decidevano
//! separatamente come interpretare ed emettere uno schema. Con l'esecuzione
//! isolata in un processo worker questo diventa un protocollo — supervisore e
//! worker devono leggere lo stesso schema allo stesso modo, o la verifica
//! della pubblicazione confronta due interpretazioni invece di due risultati.
//!
//! # Che cosa resta fuori
//!
//! Il contesto di file e input (quale percorso, quale nome di ingresso) resta
//! nella CLI: qui non si sa da dove venga uno schema, e non deve saperlo. La
//! lettura IPC, l'esecuzione e la pubblicazione restano fuori: **questo
//! modulo lavora solo sullo schema, senza leggere una riga di dati**.

use std::collections::HashMap;

use crate::arrow::schema::{DataType, Field, SchemaRef};
use crate::contract::arrow_metadata::{
    canonical_geometry_srid, read_contract_version, read_geometry_contract_keys,
    CanonicalGeometryKeys, GEOARROW_EXTENSION_KEY, GEOARROW_WKB_EXTENSION, GEO_METADATA_KEY,
    PLENORA_GEOMETRY_CRS_RESOLUTION_KEY, PLENORA_GEOMETRY_NAMESPACE_PREFIX,
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
/// # Perche' e' un parametro e non una scelta di questo modulo
///
/// Esistono due implementazioni: quella di base in [`crate::crs`] e quella
/// con backend PROJ in `plenora-kernels-geo`, selezionata da una feature. Se
/// l'autorita' ne scegliesse una, il comportamento cambierebbe a seconda di
/// dove il codice viene compilato — e `plenora-core` non puo' dipendere dai
/// kernel.
///
/// Passarlo esplicitamente rende la dipendenza visibile invece che nascosta
/// dietro un `cfg`, ed e' cio' che serve al worker isolato: supervisore e
/// worker devono usare lo STESSO risolutore, e ora e' un argomento che
/// entrambi passano invece di una proprieta' della build.
pub type CrsResolver = fn(&str, &'static str) -> Result<ResolvedCrs, crate::crs::CrsError>;

/// Errore di contratto: schema e metadati non dicono cio' che devono dire.
///
/// `InvalidPlan` e non `Schema`, e non e' una svista: la variante e' quella
/// che questo percorso produceva prima dello spostamento. Cambiarla sarebbe
/// una modifica di semantica — l'exit code della CLI e' una proiezione della
/// categoria, quindi 2 invece di 3 — dentro un refactor che dichiara di non
/// cambiare comportamento. Se un giorno `Schema` fosse la categoria giusta,
/// va cambiata come rottura dichiarata, non di straforo.
fn errore_di_contratto(messaggio: impl Into<String>) -> PlenoraError {
    PlenoraError::InvalidPlan(messaggio.into())
}

/// Definizione CRS dal metadato `geo` di una colonna `GeoArrow`: stringa
/// `authority:code` oppure PROJJSON come oggetto (serializzato compatto).
///
/// R4.6.3: il metadato mancante o privo della chiave `crs` NON e' piu' un
/// errore (restituisce `None` → [`ContractCrs::Missing`]): il centro non puo'
/// pretendere un CRS risolvibile per operazioni che non lo richiedono. Un
/// metadato MALFORMATO resta un errore (R5.1: «illeggibile» non e'
/// «assente»).
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
/// Lettura di contratto completata (R2.7),
/// con la collocazione di R4.6.3 (v2.0-rc9/rc10): il centro NON risolve
/// un'incoerenza dichiarata in assenza di una decisione esplicita nel piano
/// — la preserva come [`ContractCrs::DeclaredUnresolved`] con le
/// dichiarazioni originali, mai un errore (non e' il bordo di scrittura) e
/// mai una scelta silenziosa.
///
/// Regole, in ordine (emendamento 2026-07-31 — classe A: la co-presenza
/// `crs_id` + `crs_definition` della regola (2a) vale SOLO per input NON
/// dichiarati; il conflitto numerico `crs_id`/`srid` della regola (2b) resta
/// sempre bloccante; un `resolved` dichiarato con doppia rappresentazione si
/// onora con risoluzione + verifica di coerenza):
///
/// 1. `crs_resolution = declared_unresolved` con almeno una
///    rappresentazione: il produttore dichiara l'incoerenza — preservata
///    cosi' com'e', NESSUNA risoluzione tentata (cambio di comportamento
///    dichiarato: prima una definizione risolvibile era risolta ed emessa
///    come `resolved`; nessuna chiamata al backend, quindi nessun
///    `BackendUnavailable`). Le rappresentazioni contano per precedenza
///    R4.3.1: definizione, identificatore, poi SRID numerico — un
///    `declared_unresolved` con SOLO `srid` (il produttore conosce il
///    codice dal catalogo ma non puo' inventare l'autorita', R4.4) e'
///    legittimo: lo stato e' `DeclaredUnresolved` con
///    `crs_id`/`definition` assenti (mai sintetizzati) e lo SRID resta
///    custodito dallo schema Arrow originale;
/// 2. conflitti DECIDIBILI senza backend: (2a) SOLO per input NON dichiarati
///    (`crs_resolution` assente — il caso per cui la regola e' nata, la
///    doppia rappresentazione `GeoArrow` legacy), `crs_id` e
///    `crs_definition` co-presenti (l'accordo non e' decidibile
///    testualmente — R2.7: mai arbitrato sul dato; prima vinceva
///    `crs_definition`, scelta silenziosa); (2b) SEMPRE, anche con
///    `crs_resolution = resolved`, `crs_id` nella forma `authority:code` con
///    codice numerico discordante da `srid` (R4.3.1; prima lo `srid` era
///    ignorato e l'identificatore risolto — conciliazione silenziosa). Lo
///    stato diventa `DeclaredUnresolved` con le dichiarazioni. Prima
///    dell'emendamento la sola (2a) scattava anche con `crs_resolution`
///    esplicitamente dichiarato, rovesciando la dichiarazione del produttore
///    (bug del caso owner: shapefile EPSG:3003 con WKT coerente degradato a
///    `declared_unresolved`);
/// 3. una rappresentazione (canonica o legacy `geo.crs`), o `resolved`
///    dichiarato: risoluzione contro il backend PROJ, come sempre — un
///    fallimento di risoluzione resta un errore `Crs`, NON diventa
///    `DeclaredUnresolved` (limite dichiarato: il produttore che sa di non
///    poter garantire la risoluzione dichiara `declared_unresolved`
///    esplicitamente, come nel corpus di conformita'). Con `resolved`
///    dichiarato ED ENTRAMBE `crs_id` e `crs_definition`, alla risoluzione
///    riuscita segue la verifica di coerenza decidibile
///    ([`verify_declared_coherence`]): coerenza → `Resolved`; mismatch o
///    confronto non decidibile → `DeclaredUnresolved` con le dichiarazioni
///    originali (mai un rovesciamento silenzioso). Effetto collaterale
///    DICHIARATO: senza `proj-backend`, un input `resolved` con doppia
///    rappresentazione prima passava come `DeclaredUnresolved` (la (2a)
///    scattava senza backend), ora fallisce con errore `Crs` (risoluzione
///    impossibile) — coerente col comportamento per `resolved` a
///    rappresentazione singola di questa regola: era la (2a) l'anomalia;
/// 4. nessuna rappresentazione: [`ContractCrs::Missing`] (R4.4: mai un CRS
///    inventato), salvo la contraddizione R4.1 — `resolved`/
///    `declared_unresolved` senza alcuna rappresentazione — che resta
///    errore di discovery.
///
/// # Errors
///
/// [`PlenoraError::InvalidPlan`] se la dichiarazione e' contraddittoria:
/// `crs_resolution` valorizzata senza alcuna rappresentazione, oppure una
/// definizione che non si risolve dove il produttore la dichiarava risolta.
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

/// Verifica di coerenza DECIDIBILE dopo la risoluzione.
///
/// Per un input
/// `resolved` con doppia rappresentazione (piano-v5.md#contratti-di-input, emendamento 2026-07-31
/// — classe A): risolve anche `crs_id` e confronta l'intera coppia
/// autorita'+codice dedotta dai due canonical.
///
/// - entrambi decidibili e UGUALI: la doppia dichiarazione e' coerente →
///   `Resolved` (il caso owner: WKT Monte Mario risolve a id EPSG:3003);
/// - entrambi decidibili e DIVERSI: la dichiarazione `resolved` e'
///   dimostrabilmente falsa → `DeclaredUnresolved` con le dichiarazioni
///   originali (forma identica al braccio (1)): non passa e nulla si perde;
/// - confronto NON decidibile (identificatore non risolvibile, codice non
///   numerico o canonical senza `id`): mai arbitrato (R2.7) — la co-presenza
///   non verificabile resta un'incoerenza dichiarabile →
///   `DeclaredUnresolved`.
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

/// Codice numerico di un identificatore `authority:code` (es.
///
/// `EPSG:4326`
/// -> 4326); `None` per ogni altra forma — il confronto con `srid` non e'
/// decidibile e l'identificatore resta intero alla risoluzione. Il parsing
/// vive in `plenora-core` (unica fonte condivisa, piano-v5.md#contratti-di-input emendamento
/// 2026-07-31: lo stesso helper alimenta la deduzione `srid` del percorso
/// legacy in `arrow_adapter`).
#[must_use]
pub fn authority_code(crs_id: &str) -> Option<u32> {
    crate::crs::authority_code_srid(crs_id)
}

/// Il contratto di una colonna geometria, dalle chiavi gia' lette.
///
/// Lettura di contratto completata
/// (milestone C: [`read_geometry_contract_keys`] come sorgente primaria —
/// fail-closed R2.6 e completamento R2.7 gia' applicati dal reader).
///
/// Dimensionalita' ed encoding arrivano dalle chiavi completate: assenti ->
/// `Unknown` / `None` (R3.4: MAI un default silenzioso `Xy`). `types`: la
/// coppia `types_declaration`/`types`, se presente, entra nel contratto con
/// confidence `Declared` e scope `Schema`; assente (ingresso legacy) ->
/// [`GeometryColumnContract::undeclared_types`] (R3.4.1: «proprieta' non
/// dichiarata», MAI interpretata come `unresolved`). Il `FieldId` e'
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
