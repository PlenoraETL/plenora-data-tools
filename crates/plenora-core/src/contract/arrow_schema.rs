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
//! Le due direzioni — Arrow -> contratto e contratto -> Arrow — servono a
//! componenti diversi, la CLI in ingresso e l'executor in uscita. Ospitate
//! ciascuna presso il proprio chiamante, deciderebbero separatamente come
//! interpretare ed emettere uno schema. Con l'esecuzione isolata in un
//! processo worker questo e' un protocollo — supervisore e
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
///    cosi' com'e', NESSUNA risoluzione tentata — nemmeno di una definizione
///    che sarebbe risolvibile: nessuna chiamata al backend, quindi nessun
///    `BackendUnavailable`. Le rappresentazioni contano per precedenza
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
///    testualmente — R2.7: mai arbitrato sul dato, quindi non se ne elegge
///    una); (2b) SEMPRE, anche con `crs_resolution = resolved`, `crs_id`
///    nella forma `authority:code` con codice numerico discordante da `srid`
///    (R4.3.1: lo `srid` non si ignora per risolvere l'identificatore, che
///    sarebbe una conciliazione silenziosa). Lo stato diventa
///    `DeclaredUnresolved` con le dichiarazioni. Il limite della (2a) agli
///    input NON dichiarati e' quel che impedisce di rovesciare la
///    dichiarazione del produttore — uno shapefile EPSG:3003 con WKT
///    coerente resta `resolved`, non degrada a `declared_unresolved`;
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
///    originali (mai un rovesciamento silenzioso). Conseguenza DICHIARATA:
///    senza `proj-backend` un input `resolved` con doppia rappresentazione
///    non degrada a `DeclaredUnresolved`, ma fallisce con errore `Crs`
///    (risoluzione impossibile) — lo stesso che questa regola fa a un
///    `resolved` a rappresentazione singola;
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
/// Lettura di contratto completata: la sorgente primaria e'
/// [`read_geometry_contract_keys`], che ha gia' applicato il fail-closed
/// R2.6 e il completamento R2.7.
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

// ---------------------------------------------------------------------------
// Contratto -> Arrow: si emette il canone e si rifiutano i conflitti
//
// La direzione opposta a quella qui sopra, e con la regola opposta. In lettura
// si preserva cio' che la sorgente dichiara, anche se incoerente; in scrittura
// si emette la forma canonica e una divergenza preesistente e' un errore.
//
// L'asimmetria e' voluta. Renderle speculari costringerebbe a scegliere: o
// normalizzare in lettura, perdendo cio' che la sorgente diceva, o accettare
// l'ambiguita' in scrittura, pubblicando uno schema che non dice quale delle
// due versioni vale.
// ---------------------------------------------------------------------------

/// Lo schema Arrow che un contratto dichiara, in forma canonica.
///
/// Lo schema del contratto arricchito del blocco
/// canonico R2.2 per ogni colonna geometrica e della versione di protocollo
/// R2.5 nei metadati dello schema. E' un post-processo CENTRALE: i campi
/// sono costruiti dagli `analyze_contract` con le sole chiavi `GeoArrow`
/// legacy, che RESTANO (R2.6 ammette la coesistenza se coerente), e il
/// blocco canonico si aggiunge qui, in un punto solo, invece che in ciascun
/// analyze.
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
///   deve mai trasformarsi in conflitto R2.6 su un passthrough, e per questo
///   lo skip copre qualunque valore emesso, non il solo `axis_order =
///   unknown`);
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
                    // `axis_order` e `srid` sono per completamento DELL'ASSENTE
                    // (R2.7), mai per arbitrato: una chiave di lineage
                    // PRESENTE vince sempre, qualunque sia il valore emesso —
                    // anche un valore dedotto dalla definizione d'autorita'
                    // (piano-v5.md#contratti-di-input, emendamento 2026-07-31): la deduzione riempie
                    // solo le chiavi assenti e non deve mai trasformarsi in
                    // un falso conflitto R2.6 su un passthrough (R2.4: la
                    // dichiarazione del produttore resta). Lo skip vale per
                    // QUALUNQUE valore emesso, non per il solo
                    // `axis_order = unknown`.
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
