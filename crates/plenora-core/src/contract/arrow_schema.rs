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

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use crate::arrow::schema::Schema;
use crate::arrow::schema::{DataType, Field, SchemaRef};
use crate::contract::arrow_metadata::{
    canonical_field_id, canonical_geometry_metadata, canonical_geometry_spatial_semantics,
    canonical_schema_version_metadata, errore_di_metadato, figlio_con_chiave,
    read_contract_version, read_geometry_contract_keys, strip_decided_crs_declarations,
    CanonicalGeometryKeys, GeometryMetadataDetails, GEOARROW_EXTENSION_KEY, GEOARROW_WKB_EXTENSION,
    GEO_METADATA_KEY, PLENORA_FIELD_ID_KEY, PLENORA_GEOMETRY_AXIS_ORDER_KEY,
    PLENORA_GEOMETRY_CRS_RESOLUTION_KEY, PLENORA_GEOMETRY_NAMESPACE_PREFIX,
    PLENORA_GEOMETRY_PRECISION_KEY, PLENORA_GEOMETRY_SRID_KEY,
    PLENORA_GEOMETRY_TYPES_DECLARATION_KEY, PLENORA_GEOMETRY_TYPES_KEY,
};
use crate::contract::{
    ContractCrs, ContractProperties, ContractProperty, CrsDefinitionFormat, CrsResolution,
    DataContract, FieldId, GeometryColumnContract, GeometryDimensions, PropertyConfidence,
    PropertyScope, SpatialSemantics,
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
/// Categoria `schema`, come chiede il vocabolario Arrow 1.0 (sezione 4: «Contradictory
/// metadata fails with category `schema` or `crs`»); gli errori che
/// riguardano il CRS usano `PlenoraError::Crs`. Fino a questo ciclo era
/// `InvalidPlan`: il cambio di categoria è una rottura dichiarata (README,
/// «Metadati Arrow»), non fatta di straforo.
fn errore_di_contratto(messaggio: impl Into<String>) -> PlenoraError {
    PlenoraError::Schema(messaggio.into())
}

/// Rifiuta gli schemi con tipi che i kernel non trattano: `RunEndEncoded`
/// e `Union`, a qualunque profondita' (valori di dictionary, figli di
/// liste, struct e mappe).
///
/// Arrow 60.0.0 li gestisce ancora male in punti su cui i kernel poggiano
/// (`concat` di run-end trabocca sulle fini `Int16`, `logical_nulls`
/// sbaglia sulle union dense a un campo con id diverso da 0; `take` su
/// run-end con indici nulli, sbagliato in 59.2.0, in 60.0.0 e' corretto):
/// un caso che non si garantisce si rifiuta al confine,
/// una volta, e nessun kernel li vede. README, «Limiti dichiarati del
/// runner».
///
/// # Errors
///
/// [`PlenoraError::Unsupported`] con il nome della colonna di primo livello.
pub fn verifica_tipi_supportati(schema: &Schema) -> Result<(), PlenoraError> {
    fn ammesso(tipo: &DataType) -> bool {
        match tipo {
            DataType::RunEndEncoded(_, _) | DataType::Union(_, _) => false,
            DataType::Dictionary(chiave, valore) => ammesso(chiave) && ammesso(valore),
            DataType::List(figlio)
            | DataType::LargeList(figlio)
            | DataType::ListView(figlio)
            | DataType::LargeListView(figlio)
            | DataType::FixedSizeList(figlio, _)
            | DataType::Map(figlio, _) => ammesso(figlio.data_type()),
            DataType::Struct(figli) => figli.iter().all(|figlio| ammesso(figlio.data_type())),
            _ => true,
        }
    }
    for campo in schema.fields() {
        if !ammesso(campo.data_type()) {
            return Err(PlenoraError::Unsupported(format!(
                "colonna `{}`: i tipi RunEndEncoded e Union (anche annidati) non sono \
                 supportati dai kernel",
                campo.name()
            )));
        }
    }
    Ok(())
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
/// JSON valido; [`PlenoraError::Crs`] se la chiave `crs` non è né
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
        Some(_) => Err(PlenoraError::Crs(format!(
            "colonna geometria `{field_name}`: metadato `{GEO_METADATA_KEY}` senza \
             chiave `crs` valida"
        ))),
    }
}

/// Il contratto dichiarato da uno schema Arrow già letto, senza leggere
/// dati.
///
/// Una colonna è geometrica se porta l'estensione `geoarrow.wkb`. Le chiavi
/// `plenora.geometry.*` su un campo senza quell'estensione si rifiutano
/// (vocabolario Arrow 1.0, sezione 4: «Geometry keys on a field without the
/// `geoarrow.wkb` extension are invalid»; prima bastavano da sole). Il CRS si
/// deduce con [`contract_crs_from_keys`]; la colonna geometrica, se c'è, è
/// la geometria attiva, con `FieldId` provvisorio 0 (il chiamante lo
/// rimappa con il proprio [`crate::contract::FieldAllocator`]).
///
/// Si leggono e si verificano anche le identità dei campi
/// ([`verifica_identita_campi`]) e il metadato d'estensione `GeoArrow`
/// ([`verifica_metadato_estensione`]). La semantica `geography` e lo
/// storage `LargeBinary` non arrivano qui: la prima si rifiuta (i kernel sono
/// planari), il secondo lo converte in `Binary` chi riceve la tabella
/// (runner, lettore `GeoParquet`), perché la conversione tocca i dati.
///
/// # Errors
///
/// [`PlenoraError::Schema`] se i metadati sono incoerenti (estensione
/// non supportata, metadato `geo` o chiavi geometriche senza estensione,
/// colonna geometria non `Binary`, identità ripetute) o se le chiavi
/// canoniche si contraddicono; [`PlenoraError::Crs`] se si contraddicono le
/// chiavi del CRS o il risolutore non risolve un CRS dichiarato;
/// [`PlenoraError::Unsupported`] per una versione di contratto successiva a
/// quella supportata, la semantica `geography`, un `edges` non planare o un
/// CRS nel metadato d'estensione `GeoArrow`; [`PlenoraError::Schema`] anche
/// se il contratto che ne risulta non supera [`DataContract::validate`]
/// (per esempio più di una colonna geometrica).
pub fn contract_from_arrow_schema(
    schema: SchemaRef,
    resolve_crs: CrsResolver,
) -> Result<DataContract, PlenoraError> {
    verifica_tipi_supportati(&schema)?;
    // La versione del contratto vive nei metadati dello schema: una versione
    // successiva a quella supportata si rifiuta prima di leggere i campi.
    read_contract_version(&schema)?;
    verifica_identita_campi(&schema)?;
    let mut geometries = Vec::new();
    for field in schema.fields() {
        let extension = field.metadata().get(GEOARROW_EXTENSION_KEY);
        let geo_metadata = field.metadata().get(GEO_METADATA_KEY);
        if let Some(extension) = extension {
            if extension != GEOARROW_WKB_EXTENSION {
                // Il valore ricevuto non entra nel messaggio («errori senza
                // dati»): si nominano la chiave e la violazione.
                return Err(errore_di_contratto(format!(
                    "colonna `{}`: `{GEOARROW_EXTENSION_KEY}` diversa da \
                     `{GEOARROW_WKB_EXTENSION}`: estensione non supportata",
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
            if field
                .metadata()
                .keys()
                .any(|key| key.starts_with(PLENORA_GEOMETRY_NAMESPACE_PREFIX))
            {
                return Err(errore_di_contratto(format!(
                    "colonna `{}`: chiavi `{PLENORA_GEOMETRY_NAMESPACE_PREFIX}*` senza \
                     `{GEOARROW_EXTENSION_KEY}` = `{GEOARROW_WKB_EXTENSION}`",
                    field.name()
                )));
            }
            continue;
        }
        if field.data_type() != &DataType::Binary {
            return Err(errore_di_contratto(format!(
                "colonna geometria `{}` di tipo {}, atteso Binary (LargeBinary lo \
                 converte chi riceve la tabella)",
                field.name(),
                crate::tipo_arrow::descrivi_tipo(field.data_type())
            )));
        }
        verifica_metadato_estensione(field)?;
        if canonical_geometry_spatial_semantics(field)? == Some(SpatialSemantics::Geography) {
            return Err(PlenoraError::Unsupported(format!(
                "colonna geometria `{}`: semantica `geography` non supportata: i kernel \
                 sono planari e la leggerebbero come `geometry`",
                field.name()
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
/// 1. `crs_resolution = declared_unresolved` con `crs_id` o
///    `crs_definition`: preservato com'è, senza chiamare il risolutore. Il
///    solo `srid` non basta: il vocabolario Arrow 1.0 (sezione 4) vuole
///    per `declared_unresolved` un identificatore o una definizione, e uno
///    stato emesso senza l'uno e l'altra sarebbe uno schema non conforme;
///    si rifiuta con la regola 5 (prima si accettava);
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
/// [`PlenoraError::Crs`] se la dichiarazione è contraddittoria
/// (`crs_resolution` valorizzata senza alcuna rappresentazione), se il
/// metadato `geo` ha una chiave `crs` malformata o se il risolutore non
/// risolve la definizione.
pub fn contract_crs_from_keys(
    field_name: &str,
    geo_metadata: Option<&String>,
    keys: &CanonicalGeometryKeys,
    resolve_crs: CrsResolver,
) -> Result<ContractCrs, PlenoraError> {
    let crs_id = keys.crs_id.clone();
    let definition = keys.crs_definition.clone();
    // (1) Incoerenza dichiarata dal produttore: preservata, mai risolta.
    // Il solo SRID numerico non e' una rappresentazione ammessa per
    // `declared_unresolved` (vocabolario Arrow 1.0, sezione 4): finisce
    // nella contraddizione della regola 5.
    if keys.crs_resolution == Some(CrsResolution::DeclaredUnresolved)
        && (crs_id.is_some() || definition.is_some())
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
        if authority_code(id).is_some_and(|code| i64::from(code) != i64::from(srid)) {
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
            return Err(PlenoraError::Crs(format!(
                "colonna geometria `{field_name}`: chiave \
                 `{PLENORA_GEOMETRY_CRS_RESOLUTION_KEY}` dichiara un CRS (risolto o non \
                 risolto) ma nessun CRS e' dichiarato in alcuna rappresentazione accettata"
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
/// `Declared`; assente o `unresolved`, vale
/// [`GeometryColumnContract::undeclared_types`]: le due forme dicono la
/// stessa cosa (nessuna pretesa sui tipi), l'emissione le scrive entrambe
/// `unresolved` perché il vocabolario vuole la chiave sempre, e la lettura
/// le riporta alla proprietà non dichiarata, così il contratto sopravvive
/// al giro. Il `FieldId` è provvisorio (0): lo rimappa il chiamante.
pub fn geometry_contract_from_field(
    field: &crate::arrow::schema::Field,
    crs: ContractCrs,
    keys: &CanonicalGeometryKeys,
) -> GeometryColumnContract {
    let types = keys
        .types
        .as_ref()
        .filter(|types| types.declaration() != crate::contract::TypesDeclaration::Unresolved)
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
///   come loro `precision` (le operazioni geo la tolgono a monte, perché
///   ricodificano le coordinate in `f64`); sovrascritture in una sola
///   direzione: `crs_resolution = resolved` diventa `declared_unresolved`
///   quando il contratto porta un'incoerenza, `types_declaration =
///   unresolved` diventa la dichiarazione del contratto quando questo ne ha
///   una (README, «Metadati Arrow»).
/// - Il campo porta sempre `ARROW:extension:name = geoarrow.wkb`.
/// - `plenora.contract.version` si aggiunge solo se almeno un campo porta
///   chiavi canoniche; uno schema senza geometrie resta invariato. È la
///   forma interna del piano: allo schema che esce dal componente la
///   versione e le identità dei campi le dà [`pubblica_schema`].
/// - Una colonna geometrica del contratto assente dallo schema è un errore.
///
/// # Errors
///
/// `PlenoraError::Schema` (o `Crs` per le chiavi del CRS) per chiave
/// canonica preesistente divergente; `PlenoraError::Schema` per colonna
/// geometrica del contratto assente nello schema o con un'altra estensione.
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
        match metadata.get(GEOARROW_EXTENSION_KEY) {
            Some(extension) if extension != GEOARROW_WKB_EXTENSION => {
                return Err(errore_di_contratto(format!(
                    "campo geometria `{}`: estensione diversa da `{GEOARROW_WKB_EXTENSION}`",
                    geometry.name
                )));
            }
            Some(_) => {}
            None => {
                metadata.insert(
                    GEOARROW_EXTENSION_KEY.to_owned(),
                    GEOARROW_WKB_EXTENSION.to_owned(),
                );
            }
        }
        // Una dichiarazione dei tipi `unresolved` della lineage cede a
        // quella del contratto: se ne va con il suo elenco (assente per
        // costruzione), e il blocco canonico ri-emette la nuova.
        let dichiarazione = canonical.get(PLENORA_GEOMETRY_TYPES_DECLARATION_KEY);
        if metadata
            .get(PLENORA_GEOMETRY_TYPES_DECLARATION_KEY)
            .is_some_and(|existing| existing == "unresolved")
            && dichiarazione.is_some_and(|value| value != "unresolved")
            && !metadata.contains_key(PLENORA_GEOMETRY_TYPES_KEY)
        {
            metadata.remove(PLENORA_GEOMETRY_TYPES_DECLARATION_KEY);
        }
        // In ordine di chiave: con piu' chiavi in conflitto l'errore (e la
        // sua categoria) e' sempre lo stesso, non quello che l'ordine della
        // `HashMap` sceglie.
        let mut canonical: Vec<(String, String)> = canonical.into_iter().collect();
        canonical.sort();
        for (key, value) in &canonical {
            match metadata.get(key) {
                Some(existing) if existing != value => {
                    // `axis_order` e `srid` si completano solo se assenti:
                    // una chiave di lineage presente vince con qualunque
                    // valore emesso, anche dedotto dall'autorità, così la
                    // deduzione non diventa un falso conflitto su un
                    // passaggio che non tocca il CRS.
                    // La precisione segue la stessa regola: una dichiarazione
                    // ereditata attraversa intatta le operazioni che non
                    // toccano le coordinate, e le operazioni geo la tolgono
                    // a monte (`analyze_geo_contract`), perche' ricodificano
                    // in `f64`.
                    if key == PLENORA_GEOMETRY_AXIS_ORDER_KEY
                        || key == PLENORA_GEOMETRY_SRID_KEY
                        || key == PLENORA_GEOMETRY_PRECISION_KEY
                    {
                        continue;
                    }
                    // Un'incoerenza CRS rilevata si dichiara
                    // (`declared_unresolved`) invece di propagare il
                    // `resolved` del produttore; con una decisione del
                    // piano le dichiarazioni della sorgente sono già rimosse
                    // (`strip_decided_crs_declarations`).
                    if key == PLENORA_GEOMETRY_CRS_RESOLUTION_KEY
                        && existing == "resolved"
                        && value == "declared_unresolved"
                    {
                        metadata.insert(key.clone(), value.clone());
                        continue;
                    }
                    return Err(errore_di_metadato(
                        key,
                        format!(
                            "campo geometria `{}`: chiave `{key}` gia' presente con un valore \
                             diverso da quello del contratto (il componente fallisce, \
                             non sovrascrive)",
                            geometry.name
                        ),
                    ));
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
        return Err(errore_di_contratto(
            "colonna geometrica del contratto assente nello schema di output",
        ));
    }
    // La versione accompagna le chiavi canoniche; qui almeno un campo le
    // porta (guardia in testa e conteggio sopra).
    let mut metadata = contract.schema.metadata().clone();
    aggiungi_versione(&mut metadata)?;
    Ok(Arc::new(Schema::new_with_metadata(fields, metadata)))
}

/// `plenora.contract.version` nei metadati di schema: aggiunta se manca,
/// idempotente se uguale, errore se diversa (il componente fallisce, non
/// sovrascrive).
fn aggiungi_versione(metadata: &mut crate::arrow::Metadata) -> Result<(), PlenoraError> {
    for (key, value) in canonical_schema_version_metadata() {
        match metadata.get(&key) {
            Some(existing) if existing != &value => {
                return Err(errore_di_contratto(format!(
                    "chiave `{key}` dello schema gia' presente con un valore diverso \
                     (il componente fallisce, non sovrascrive)"
                )));
            }
            Some(_) => {}
            None => {
                metadata.insert(key, value);
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Identità dei campi (ARROW-003/004, vocabolario, sezione 2) e pubblicazione dello
// schema al confine (ARROW-001).
// ---------------------------------------------------------------------------

/// Le identità `plenora.field_id` dei campi di primo livello, verificate.
///
/// Ogni valore è un intero decimale non negativo entro `u32`
/// ([`canonical_field_id`]) e nessun valore si ripete nello schema
/// (vocabolario, sezione 2: «Unique within a schema»). Una chiave
/// `plenora.field_id` su un campo annidato (figlio di struct, lista o
/// mappa) si rifiuta: il contratto la definisce sui campi pubblici, e
/// un'operazione che porta i figli al primo livello (`table.unnest`) la
/// trasformerebbe in un'identità di primo livello che nessuno ha
/// dichiarato.
///
/// # Errors
///
/// `PlenoraError::Schema` per un valore malformato o ripetuto;
/// `PlenoraError::Unsupported` per un valore oltre `u32::MAX` o una chiave
/// su un campo annidato.
pub fn verifica_identita_campi(schema: &Schema) -> Result<BTreeSet<u32>, PlenoraError> {
    let mut identita = BTreeSet::new();
    for field in schema.fields() {
        if let Some(FieldId(id)) = canonical_field_id(field)? {
            if !identita.insert(id) {
                return Err(PlenoraError::Schema(format!(
                    "colonna `{}`: `{PLENORA_FIELD_ID_KEY}` ripetuto nello schema \
                     (l'identita' dei campi e' unica)",
                    field.name()
                )));
            }
        }
        if figlio_con_chiave(field.data_type(), &|key| key == PLENORA_FIELD_ID_KEY) {
            return Err(PlenoraError::Unsupported(format!(
                "colonna `{}`: `{PLENORA_FIELD_ID_KEY}` su un campo annidato non e' \
                 interpretata (l'identita' e' dei campi di primo livello)",
                field.name()
            )));
        }
    }
    Ok(identita)
}

/// Lo schema che attraversa il confine del componente (ARROW-001, ARROW-003,
/// ARROW-004): `plenora.contract.version = 1` nei metadati di schema e
/// `plenora.field_id` su ogni campo di primo livello.
///
/// L'identità segue la lineage dei metadati di campo: un'operazione che
/// propaga una colonna, la rinomina o la riscrive al suo posto (README,
/// «Metadati Arrow») ne clona i metadati, e con loro `plenora.field_id`;
/// una colonna nuova nasce senza. Qui:
///
/// - un'identità portata da **un solo** campo dello schema e assente da
///   `ambigue` resta, byte per byte;
/// - un'identità portata da più campi (una colonna duplicata, un self-join)
///   o in `ambigue` (dichiarata da più di un ingresso: i namespace degli
///   ingressi sono indipendenti, e la stessa cifra direbbe due campi
///   diversi) si toglie a tutti i campi che la portano. L'identità si
///   perde, non si trasferisce: un consumatore non trova il campo, invece
///   di trovarne un altro;
/// - ogni campo senza identità ne riceve una nuova, in ordine di colonna,
///   dalla prima sopra il massimo di `osservate` e delle identità dello
///   schema: un'identità nuova non coincide mai con una che un ingresso
///   usava per un altro campo.
///
/// Deterministica: dipende solo dallo schema e dagli insiemi dati.
///
/// # Errors
///
/// Quelli di [`canonical_field_id`]; `PlenoraError::Schema` per una
/// versione diversa da `1` già nei metadati; `PlenoraError::Unsupported`
/// se le identità nuove superano `u32::MAX`.
pub fn pubblica_schema(
    schema: &Schema,
    osservate: &BTreeSet<u32>,
    ambigue: &BTreeSet<u32>,
) -> Result<SchemaRef, PlenoraError> {
    let mut metadata = schema.metadata().clone();
    aggiungi_versione(&mut metadata)?;
    let mut portate = Vec::with_capacity(schema.fields().len());
    let mut conteggi: BTreeMap<u32, usize> = BTreeMap::new();
    for field in schema.fields() {
        let id = canonical_field_id(field)?.map(|FieldId(id)| id);
        if let Some(id) = id {
            *conteggi.entry(id).or_insert(0) += 1;
        }
        portate.push(id);
    }
    let massimo = osservate.iter().chain(conteggi.keys()).max().copied();
    let mut prossima = massimo.map_or(0, |massimo| u64::from(massimo) + 1);
    let mut fields = Vec::with_capacity(schema.fields().len());
    for (field, id) in schema.fields().iter().zip(portate) {
        let tenuta = id.filter(|id| conteggi.get(id) == Some(&1) && !ambigue.contains(id));
        if tenuta.is_some() {
            fields.push(field.as_ref().clone());
            continue;
        }
        let nuova = u32::try_from(prossima).map_err(|_| {
            PlenoraError::Unsupported(format!(
                "`{PLENORA_FIELD_ID_KEY}`: identita' nuove oltre {} (limite di questo \
                 componente)",
                u32::MAX
            ))
        })?;
        prossima += 1;
        let mut campo_metadata = field.metadata().clone();
        campo_metadata.insert(PLENORA_FIELD_ID_KEY.to_owned(), nuova.to_string());
        fields.push(field.as_ref().clone().with_metadata(campo_metadata));
    }
    Ok(Arc::new(Schema::new_with_metadata(fields, metadata)))
}

/// Chiave del metadato d'estensione `GeoArrow` (JSON con `crs` ed `edges`).
pub const GEOARROW_EXTENSION_METADATA_KEY: &str = "ARROW:extension:metadata";

/// Il metadato d'estensione `GeoArrow` di una colonna `geoarrow.wkb` non dice
/// nulla che i kernel ignorerebbero in silenzio.
///
/// Assente, vuoto o `{}`: niente da dire. `edges` diverso da `planar`
/// (archi geodetici, `spherical` e simili) si rifiuta come la semantica
/// `geography`: i kernel sono planari. Un `crs` (non `null`) si rifiuta:
/// questo componente legge il CRS dalle chiavi `plenora.geometry.*` o dal
/// metadato `geo`, e ignorarlo lo farebbe passare per assente (ARROW-007).
///
/// # Errors
///
/// `PlenoraError::Schema` per un metadato che non è un oggetto JSON senza
/// chiavi ripetute; `PlenoraError::Unsupported` per `edges` non planare o
/// un `crs` dichiarato.
pub fn verifica_metadato_estensione(field: &Field) -> Result<(), PlenoraError> {
    let Some(raw) = field.metadata().get(GEOARROW_EXTENSION_METADATA_KEY) else {
        return Ok(());
    };
    if raw.trim().is_empty() {
        return Ok(());
    }
    let illeggibile = || {
        errore_di_contratto(format!(
            "colonna geometria `{}`: `{GEOARROW_EXTENSION_METADATA_KEY}` non e' un oggetto \
             JSON leggibile",
            field.name()
        ))
    };
    crate::json::ensure_no_duplicate_keys(raw).map_err(|_| illeggibile())?;
    let value: serde_json::Value = serde_json::from_str(raw).map_err(|_| illeggibile())?;
    let oggetto = value.as_object().ok_or_else(illeggibile)?;
    match oggetto.get("edges") {
        None => {}
        Some(serde_json::Value::String(edges)) if edges == "planar" => {}
        Some(_) => {
            return Err(PlenoraError::Unsupported(format!(
                "colonna geometria `{}`: `edges` non planare nel metadato d'estensione: i \
                 kernel sono planari",
                field.name()
            )));
        }
    }
    match oggetto.get("crs") {
        None | Some(serde_json::Value::Null) => Ok(()),
        Some(_) => Err(PlenoraError::Unsupported(format!(
            "colonna geometria `{}`: CRS nel metadato d'estensione GeoArrow non \
             interpretato da questo componente (dichiararlo con le chiavi \
             `plenora.geometry.*`): ignorarlo lo farebbe passare per assente",
            field.name()
        ))),
    }
}

/// Versione e identità dei campi di uno schema da scrivere, verificate.
///
/// Le verifiche di [`read_contract_version`] e [`verifica_identita_campi`],
/// per chi scrive uno schema senza leggerlo come contratto (`plenora-io`).
///
/// # Errors
///
/// Quelli delle due verifiche.
pub fn verifica_metadati_di_confine(schema: &Schema) -> Result<(), PlenoraError> {
    read_contract_version(schema)?;
    verifica_identita_campi(schema)?;
    Ok(())
}
