//! Scoperta del contratto di un input Arrow, e le decisioni sul CRS.
//!
//! Che cosa un file dichiara di se' — campi, colonna geometrica, dimensioni,
//! encoding, CRS — letto dai soli metadati dello schema, senza toccare una
//! riga di dati. E' la superficie da cui nascono `describe`, la validazione
//! del piano e la verifica di compatibilita' degli input.
//!
//! Le regole sul CRS sono qui e non sparse: una dichiarazione incoerente
//! resta incoerente (`DeclaredUnresolved`) invece di essere risolta in
//! silenzio a favore di una delle due forme, e la risoluzione per decisione
//! del piano ([`apply_crs_decisions`]) sostituisce le dichiarazioni della
//! sorgente in un punto solo.

use std::path::{Path, PathBuf};

use plenora_core::arrow::schema::{DataType, SchemaRef};
use plenora_core::contract::{
    ContractCrs, ContractProperties, ContractProperty, CrsDefinitionFormat, CrsResolution,
    DataContract, FieldId, GeometryColumnContract, GeometryDimensions, PropertyConfidence,
    PropertyScope,
};
use plenora_core::crs::ResolvedCrs;
use plenora_core::PlenoraError;
use plenora_engine::{ipc_boundary, Input, IpcLimits};
use plenora_kernels_geo::arrow_adapter::{
    read_contract_version, read_geometry_contract_keys, CanonicalGeometryKeys,
    GEOARROW_EXTENSION_KEY, GEOARROW_WKB_EXTENSION, GEO_METADATA_KEY,
    PLENORA_GEOMETRY_CRS_RESOLUTION_KEY, PLENORA_GEOMETRY_NAMESPACE_PREFIX,
};

#[cfg(not(feature = "proj-backend"))]
use plenora_core::crs::resolve_crs;
#[cfg(feature = "proj-backend")]
use plenora_kernels_geo::crs::resolve_crs;

use crate::{contract, PlanInputsProbe, V4Inputs};

/// Schema Arrow dell'header IPC di un input (file o stream format): nessuna
/// riga di dati letta.
///
/// Passa dal lettore di confine condiviso ([`plenora_engine::ipc_boundary`]):
/// framing e limiti pre-validati prima che arrow allochi, panico di
/// `fb_to_schema` convertito in errore. La CLI non apre piu'
/// `FileReader`/`StreamReader` per conto proprio su input non fidati.
///
/// Confine di lettura (BLOCK-03): gli errori `Io`/`DataMapping` di apertura
/// e parse dell'header nascono leggendo la sorgente — tag
/// [`ErrorPhase::Read`].
pub fn ipc_header_schema(path: &Path) -> Result<SchemaRef, PlenoraError> {
    ipc_boundary::header_schema(path, &IpcLimits::default())
}

/// Input lazy per l'executor: IPC file o stream format, sniffato dal magic.
pub fn open_input(path: &Path, limits: &IpcLimits) -> Result<Input, PlenoraError> {
    Input::read_ipc_with_limits(path, limits)
}

/// Definizione CRS dal metadato `geo` di una colonna `GeoArrow`: stringa
/// `authority:code` oppure PROJJSON come oggetto (serializzato compatto).
///
/// R4.6.3: il metadato mancante o privo della chiave `crs` NON e' piu' un
/// errore (restituisce `None` → [`ContractCrs::Missing`]): il centro non puo'
/// pretendere un CRS risolvibile per operazioni che non lo richiedono. Un
/// metadato MALFORMATO resta un errore (R5.1: «illeggibile» non e'
/// «assente»).
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
        Some(_) => Err(contract(format!(
            "colonna geometria `{field_name}`: metadato `{GEO_METADATA_KEY}` senza \
             chiave `crs` valida"
        ))),
    }
}

/// Scoperta del `DataContract` di un input dal solo header IPC: schema Arrow
/// e colonne geometria se presenti, CRS risolto SE dichiarato. Fail-closed
/// su metadati incoerenti. Il `FieldId` e' provvisorio: il planner lo rimappa
/// nel namespace globale del grafo (D16).
///
/// Milestone C (protocollo chiavi canoniche, contratti trasversali §2):
///
/// - gate R2.5 all'ingresso: [`read_contract_version`] — una versione
///   successiva a quella nota, una chiave di versione malformata o chiavi
///   canoniche senza versione sono errori propagati, mai ignorati;
/// - sorgente primaria per ogni campo geometria:
///   [`read_geometry_contract_keys`] — chiavi canoniche fail-closed,
///   coerenza canonica/legacy (R2.6: divergenza -> errore) e completamento
///   per precedenza (R2.7) sono applicati dal reader;
/// - riconoscimento (1c): le chiavi canoniche sono autosufficienti (tabella
///   §2), quindi un campo con chiavi `plenora.geometry.*` e' riconosciuto
///   come colonna geometrica anche SENZA l'estensione `geoarrow.wkb` e il
///   metadato `geo`; in entrambi i percorsi il tipo DEVE essere `Binary`,
///   altrimenti errore esplicito;
/// - CRS: lo stato e' deciso da [`contract_crs_from_keys`] sulla
///   rappresentazione completata (canonica o legacy) e sulla
///   `crs_resolution` dichiarata. R4.6.3 (v2.0-rc9/rc10): un campo
///   geometrico SENZA CRS dichiarato NON e' un errore — il centro non puo'
///   pretendere un CRS risolvibile per operazioni che non lo richiedono;
///   lo stato entra nel contratto come [`ContractCrs::Missing`] (R4.4: mai
///   un CRS inventato) e ferma solo le op che dichiarano un
///   `CrsRequirement`, in analyze. Un'incoerenza dichiarata
///   (`declared_unresolved`) o un conflitto decidibile fra rappresentazioni
///   diventano [`ContractCrs::DeclaredUnresolved`], preservati e mai
///   risolti in assenza di una decisione esplicita nel piano
///   (`crs_decisions`). Resta errore la dichiarazione contraddittoria
///   (`crs_resolution` valorizzata ma nessuna rappresentazione: R4.1 vieta
///   di collassarla su `missing`).
pub fn discover_input_contract(path: &Path) -> Result<DataContract, PlenoraError> {
    discover_input_contract_from_schema(ipc_header_schema(path)?)
}

/// Scoperta da schema Arrow gia' letto (seam di test: nessun file toccato).
/// Le regole sono quelle di [`discover_input_contract`].
pub fn discover_input_contract_from_schema(
    schema: SchemaRef,
) -> Result<DataContract, PlenoraError> {
    // Gate R2.5: la versione del protocollo vive nei metadati dello schema.
    read_contract_version(&schema)?;
    let mut geometries = Vec::new();
    for field in schema.fields() {
        let extension = field.metadata().get(GEOARROW_EXTENSION_KEY);
        let geo_metadata = field.metadata().get(GEO_METADATA_KEY);
        if let Some(extension) = extension {
            if extension != GEOARROW_WKB_EXTENSION {
                return Err(contract(format!(
                    "colonna `{}`: estensione `{extension}` non supportata \
                     (attesa `{GEOARROW_WKB_EXTENSION}`)",
                    field.name()
                )));
            }
        } else {
            if geo_metadata.is_some() {
                return Err(contract(format!(
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
            return Err(contract(format!(
                "colonna geometria `{}` di tipo {}, atteso Binary",
                field.name(),
                field.data_type()
            )));
        }
        let keys = read_geometry_contract_keys(field)?;
        let crs = contract_crs_from_keys(field.name(), geo_metadata, &keys)?;
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

/// Stato CRS del contratto dalla lettura di contratto completata (R2.7),
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
pub fn contract_crs_from_keys(
    field_name: &str,
    geo_metadata: Option<&String>,
    keys: &CanonicalGeometryKeys,
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
            return Err(contract(format!(
                "colonna geometria `{field_name}`: chiave \
                 `{PLENORA_GEOMETRY_CRS_RESOLUTION_KEY}` dichiara `{resolution}` ma \
                 nessun CRS e' dichiarato in alcuna rappresentazione accettata"
            )));
        }
    }
    Ok(ContractCrs::Missing)
}

/// Verifica di coerenza DECIDIBILE dopo la risoluzione, per un input
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
) -> ContractCrs {
    let resolved_identifier = resolved.authority_identifier();
    let simple_identifier = plenora_core::crs::authority_code_identifier(crs_id);
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

/// Codice numerico di un identificatore `authority:code` (es. `EPSG:4326`
/// -> 4326); `None` per ogni altra forma — il confronto con `srid` non e'
/// decidibile e l'identificatore resta intero alla risoluzione. Il parsing
/// vive in `plenora-core` (unica fonte condivisa, piano-v5.md#contratti-di-input emendamento
/// 2026-07-31: lo stesso helper alimenta la deduzione `srid` del percorso
/// legacy in `arrow_adapter`).
pub fn authority_code(crs_id: &str) -> Option<u32> {
    plenora_core::crs::authority_code_srid(crs_id)
}

/// Contesto "input `nome` (percorso)" sull'errore, preservando la variante.
pub fn at_input(name: &str, path: &Path, error: PlenoraError) -> PlenoraError {
    let prefix = |message: &String| format!("input `{name}` ({}): {message}", path.display());
    match error {
        PlenoraError::InvalidPlan(message) => PlenoraError::InvalidPlan(prefix(&message)),
        PlenoraError::Unsupported(message) => PlenoraError::Unsupported(prefix(&message)),
        PlenoraError::Schema(message) => PlenoraError::Schema(prefix(&message)),
        PlenoraError::Crs(message) => PlenoraError::Crs(prefix(&message)),
        other => other,
    }
}

/// Contratto della colonna geometria dalla lettura di contratto completata
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
    field: &plenora_core::arrow::schema::Field,
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

/// Accoppia gli input della riga di comando a quelli dichiarati dal piano DAG.
///
/// Nella forma NOMINALE l'accoppiamento e' quello scritto: ogni nome dev'essere
/// dichiarato dal piano e ogni input dichiarato dev'essere fornito, una volta
/// sola.
///
/// La forma POSIZIONALE e' ammessa **solo con un input dichiarato**. Con due o
/// piu' input non e' verificabile: due file scambiati con lo stesso schema
/// producono un risultato sbagliato invece di un errore — il piano gira, i
/// contratti combaciano, e nessuno se ne accorge. Era l'unico posto della CLI
/// in cui uno scambio dell'utente non era intercettabile dal componente; ora
/// quella forma non arriva all'esecuzione.
///
/// # Errors
///
/// `PlenoraError::InvalidPlan` su nome non dichiarato, input dichiarato ma non
/// fornito, forma posizionale con piu' di un input dichiarato, o conteggio
/// diverso nella forma posizionale.
pub fn pair_v4_inputs(
    probe: &PlanInputsProbe,
    inputs: &V4Inputs,
) -> Result<Vec<(String, PathBuf)>, PlenoraError> {
    let paths = match inputs {
        V4Inputs::Named(named) => {
            for (name, _) in named {
                if !probe.inputs.iter().any(|declared| declared == name) {
                    return Err(contract(format!(
                        "input `{name}` non dichiarato dal piano (dichiarati: {})",
                        probe.inputs.join(", ")
                    )));
                }
            }
            // L'ordine restituito e' quello del PIANO, non quello della riga
            // di comando: cosi' l'ordine degli argomenti non e' osservabile a
            // valle e non puo' diventare una dipendenza implicita.
            return probe
                .inputs
                .iter()
                .map(|declared| {
                    named
                        .iter()
                        .find(|(name, _)| name == declared)
                        .map(|(name, path)| (name.clone(), path.clone()))
                        .ok_or_else(|| {
                            contract(format!(
                                "input `{declared}` dichiarato dal piano ma non fornito: \
                                 aggiungere `--input {declared}=PERCORSO`"
                            ))
                        })
                })
                .collect();
        }
        V4Inputs::Positional(paths) => paths,
    };
    if probe.inputs.len() > 1 {
        // Con piu' di un input la forma posizionale non e' VERIFICABILE: due
        // percorsi scambiati sono indistinguibili da due percorsi giusti, e
        // se gli schemi coincidono il piano gira producendo il risultato
        // sbagliato. Un avviso non basta — nei log di una pipeline non lo
        // legge nessuno — quindi si rifiuta prima di toccare i file,
        // indicando la forma che chiude il problema. Resta ammessa con un
        // input solo, dove non c'e' niente da scambiare.
        return Err(contract(format!(
            "`--inputs` accoppia i percorsi per POSIZIONE e non e' ammesso con {} input \
             dichiarati: usare la forma nominale `{}`",
            probe.inputs.len(),
            probe
                .inputs
                .iter()
                .map(|name| format!("--input {name}=PERCORSO"))
                .collect::<Vec<_>>()
                .join(" ")
        )));
    }
    if probe.inputs.len() != paths.len() {
        return Err(contract(format!(
            "il piano dichiara {} input ({}) ma ne sono stati forniti {}",
            probe.inputs.len(),
            probe.inputs.join(", "),
            paths.len()
        )));
    }
    Ok(probe
        .inputs
        .iter()
        .cloned()
        .zip(paths.iter().cloned())
        .collect())
}

/// Contratti degli input di un piano DAG, scoperti dagli header IPC.
pub fn discover_contracts(
    pairs: &[(String, PathBuf)],
) -> Result<Vec<(String, DataContract)>, PlenoraError> {
    pairs
        .iter()
        .map(|(name, path)| {
            discover_input_contract(path)
                .map(|contract| (name.clone(), contract))
                .map_err(|error| at_input(name, path, error))
        })
        .collect()
}

/// Applica le decisioni CRS esplicite del piano DAG (`crs_decisions`,
/// R4.6.3) ai contratti scoperti: per ogni input nominato, la definizione
/// decisa e' risolta contro il backend (senza `proj-backend`:
/// `BackendUnavailable`, come il CRS di piano) e sostituisce lo stato
/// [`ContractCrs::DeclaredUnresolved`] con
/// [`ContractCrs::ResolvedByDecision`] — un CRS risolto a tutti gli effetti
/// per le op a valle, marcato perche' l'emissione SOSTITUISCA le
/// dichiarazioni della sorgente con il CRS deciso
/// (`strip_decided_crs_declarations` nella fusione dello schema di
/// output). Lo schema del contratto di input NON e' toccato: il check
/// fail-closed dell'executor confronta i campi del file con quelli del
/// contratto validato (metadati inclusi). La decisione resta esplicita nel
/// piano e coperta dal `plan_hash` (piano-v5.md#identita-e-fingerprint); il fingerprint del contratto
/// di input cambia di conseguenza (un piano con decisione non accetta in
/// riesecuzione l'input non deciso senza rivalidazione).
///
/// Errori espliciti (mai una decisione ignorata in silenzio): input non
/// fornito o senza colonna geometrica; stato diverso da
/// `DeclaredUnresolved` (su `Missing` sarebbe un CRS inventato — R4.4; su
/// `Resolved` una contraddizione del piano); definizione non risolvibile.
/// I messaggi non riportano valori di dichiarazioni (regola «errori senza
/// dati»).
pub fn apply_crs_decisions(
    probe: &PlanInputsProbe,
    contracts: &mut [(String, DataContract)],
) -> Result<(), PlenoraError> {
    for (input, definition) in &probe.crs_decisions {
        let Some((_, input_contract)) = contracts.iter_mut().find(|(name, _)| name == input) else {
            return Err(contract(format!(
                "crs_decisions: l'input `{input}` non e' tra i contratti scoperti"
            )));
        };
        if input_contract.geometries.len() != 1 {
            return Err(contract(format!(
                "crs_decisions: l'input `{input}` non dichiara esattamente una colonna \
                 geometrica: la decisione non e' applicabile"
            )));
        }
        let geometry = &mut input_contract.geometries[0];
        if !matches!(geometry.crs, ContractCrs::DeclaredUnresolved { .. }) {
            return Err(contract(format!(
                "crs_decisions: l'input `{input}` dichiara il CRS come `{}`, non come \
                 `declared_unresolved`: la decisione non e' applicabile",
                geometry.crs.resolution()
            )));
        }
        geometry.crs = ContractCrs::ResolvedByDecision(resolve_crs(definition, "crs")?);
    }
    Ok(())
}
