//! `GeoParquet` 1.1: il metadato di file `geo` e il contratto geometrico.
//!
//! **Lettura** ([`applica`]): il metadato `geo` del file è l'autorità. Per
//! ogni colonna dichiarata:
//!
//! - `encoding` deve essere `WKB` (le codifiche `GeoArrow` native si
//!   rifiutano, `Unsupported`);
//! - `edges` assente o `planar` (`spherical` si rifiuta: i kernel sono
//!   planari); `epoch` si rifiuta (il contratto non ha epoche);
//!   `orientation` può valere solo `counterclockwise`, `bbox` deve avere 4 o
//!   6 numeri finiti e `covering` deve essere un oggetto: sono dichiarazioni
//!   che il contratto non porta e si lasciano cadere;
//! - `crs` assente vale `OGC:CRS84` (la specifica), `null` è un CRS
//!   assente ([`ContractCrs::Missing`]), un `PROJJSON` si riconduce alla
//!   tabella integrata per `id` ([`crate::crs_projjson`]);
//! - `geometry_types` dà i tipi dichiarati (`exact`) e la dimensionalità
//!   (tutti ` Z` → `xyz`, nessuno → `xy`, misti o elenco vuoto →
//!   `unknown`); ogni cella si cammina ([`crate::wkb`]) e un tipo non
//!   dichiarato è un errore;
//! - il campo diventa il campo `GeoArrow`-WKB che il contratto accetta
//!   (`Binary`, `ARROW:extension:name = geoarrow.wkb`, metadato di campo
//!   `geo` con `crs`, `dimensions`, `encoding`), e lo schema passa da
//!   `contract_from_arrow_schema` e `arrow_schema_from_contract`, che
//!   aggiungono il blocco canonico `plenora.geometry.*` e rifiutano le
//!   chiavi canoniche già presenti in conflitto.
//!
//! Il metadato `geo` di schema si toglie: vive nei campi. `primary_column`
//! deve essere la prima colonna geometrica dello schema, perché il
//! contratto elegge quella come geometria attiva.
//!
//! **Scrittura** ([`prepara`]): dal contratto della tabella; `crs` è il
//! `PROJJSON` completo del CRS integrato (o `null` per un CRS assente),
//! `geometry_types` i tipi dichiarati dal contratto (verificati sui dati) o,
//! senza dichiarazione, quelli trovati nei dati, `bbox` il riquadro XY delle
//! coordinate quando tutte le geometrie sono 2D. Il metadato `geo` di campo
//! e `ARROW:extension:metadata` si tolgono dallo schema scritto: li porta il
//! metadato di file.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use plenora_core::arrow::array::{Array, ArrayRef, BinaryArray, LargeBinaryArray, RecordBatch};
use plenora_core::arrow::schema::{DataType, Field, Schema};
use plenora_core::contract::arrow_metadata::{
    canonical_geometry_axis_order, field_declares_wkb_geometry, GEOARROW_EXTENSION_KEY,
    GEOARROW_WKB_EXTENSION, GEO_METADATA_KEY,
};
use plenora_core::contract::arrow_schema::{
    arrow_schema_from_contract, contract_from_arrow_schema,
};
use plenora_core::contract::{
    AxisOrder, ContractCrs, ContractProperty, GeometryDimensions, GeometryEncoding, GeometryType,
    GeometryTypesProperty, PropertyConfidence, PropertyScope, TypesDeclaration,
};
use plenora_core::crs::resolve_crs;
use plenora_core::{PlenoraError, Result};
use serde_json::{Map, Value};

use crate::crs_projjson::{identificativo_da_projjson, identificativo_di, projjson_di};
use crate::wkb::{nome_geoparquet, scansiona_cella, tipo_da_nome, Sommario};

/// Versione scritta.
pub const VERSIONE_SCRITTA: &str = "1.1.0";
/// Versioni lette.
const VERSIONI_LETTE: [&str; 2] = ["1.0.0", "1.1.0"];
/// Chiave del metadato di estensione `GeoArrow`, sostituito in lettura.
const EXTENSION_METADATA_KEY: &str = "ARROW:extension:metadata";

/// Chiavi di colonna della specifica 1.1.
const CHIAVI_COLONNA: [&str; 8] = [
    "encoding",
    "geometry_types",
    "crs",
    "epoch",
    "edges",
    "orientation",
    "bbox",
    "covering",
];

fn invalido(motivo: &str) -> PlenoraError {
    PlenoraError::InvalidPlan(format!("metadato GeoParquet `geo`: {motivo}"))
}

fn di_colonna(nome: &str, errore: PlenoraError) -> PlenoraError {
    errore.con_contesto(&format!("colonna geometrica `{nome}`"))
}

/// Il CRS di una colonna letta.
#[derive(Clone, Debug, PartialEq, Eq)]
enum CrsColonna {
    Integrato(String),
    Assente,
}

/// Una colonna del metadato `geo`, già validata.
#[derive(Clone, Debug)]
struct ColonnaGeo {
    crs: CrsColonna,
    /// Tipi dichiarati, con Z; vuoto = sconosciuti.
    tipi: BTreeSet<(GeometryType, bool)>,
}

impl ColonnaGeo {
    fn dimensioni(&self) -> GeometryDimensions {
        let z: BTreeSet<bool> = self.tipi.iter().map(|(_, z)| *z).collect();
        match (z.contains(&false), z.contains(&true)) {
            (true, false) => GeometryDimensions::Xy,
            (false, true) => GeometryDimensions::Xyz,
            _ => GeometryDimensions::Unknown,
        }
    }
}

/// Il metadato `geo` letto.
#[derive(Clone, Debug)]
struct MetadatoGeo {
    primaria: String,
    colonne: BTreeMap<String, ColonnaGeo>,
}

fn analizza(testo: &str) -> Result<MetadatoGeo> {
    plenora_core::json::ensure_no_duplicate_keys(testo)
        .map_err(|_| invalido("chiavi duplicate, documento ambiguo"))?;
    let Value::Object(radice) =
        serde_json::from_str::<Value>(testo).map_err(|_| invalido("JSON non valido"))?
    else {
        return Err(invalido("non e' un oggetto JSON"));
    };
    let versione = radice
        .get("version")
        .and_then(Value::as_str)
        .ok_or_else(|| invalido("`version` assente o non testuale"))?;
    if !VERSIONI_LETTE.contains(&versione) {
        // Il valore letto non entra nel messaggio («errori senza dati»).
        return Err(PlenoraError::Unsupported(
            "GeoParquet: versione (`version`) non supportata (lette: 1.0.0, 1.1.0)".to_owned(),
        ));
    }
    let primaria = radice
        .get("primary_column")
        .and_then(Value::as_str)
        .ok_or_else(|| invalido("`primary_column` assente o non testuale"))?
        .to_owned();
    let Some(Value::Object(colonne_json)) = radice.get("columns") else {
        return Err(invalido("`columns` assente o non oggetto"));
    };
    let mut colonne = BTreeMap::new();
    for (nome, valore) in colonne_json {
        let analizzata = analizza_colonna(valore).map_err(|errore| di_colonna(nome, errore))?;
        colonne.insert(nome.clone(), analizzata);
    }
    if !colonne.contains_key(&primaria) {
        return Err(invalido("`primary_column` non e' tra le `columns`"));
    }
    Ok(MetadatoGeo { primaria, colonne })
}

fn analizza_colonna(valore: &Value) -> Result<ColonnaGeo> {
    let Value::Object(colonna) = valore else {
        return Err(invalido("colonna non oggetto"));
    };
    if let Some(chiave) = colonna
        .keys()
        .find(|chiave| !CHIAVI_COLONNA.contains(&chiave.as_str()))
    {
        return Err(PlenoraError::Unsupported(format!(
            "metadato GeoParquet `geo`: chiave di colonna `{chiave}` fuori dalla specifica 1.1"
        )));
    }
    match colonna.get("encoding").and_then(Value::as_str) {
        Some("WKB") => {}
        Some(
            "point" | "linestring" | "polygon" | "multipoint" | "multilinestring" | "multipolygon",
        ) => {
            return Err(PlenoraError::Unsupported(
                "codifica GeoArrow nativa: solo WKB e' supportata".to_owned(),
            ))
        }
        _ => return Err(invalido("`encoding` assente o non riconosciuta")),
    }
    match colonna.get("edges") {
        None => {}
        Some(Value::String(edges)) if edges == "planar" => {}
        Some(Value::String(edges)) if edges == "spherical" => {
            return Err(PlenoraError::Unsupported(
                "`edges` spherical: i kernel sono planari, i lati geodetici non si \
                 rappresentano"
                    .to_owned(),
            ))
        }
        Some(_) => return Err(invalido("`edges` non riconosciuto")),
    }
    if colonna.contains_key("epoch") {
        return Err(PlenoraError::Unsupported(
            "`epoch` di un CRS dinamico: il contratto non la rappresenta".to_owned(),
        ));
    }
    match colonna.get("orientation") {
        None => {}
        Some(Value::String(verso)) if verso == "counterclockwise" => {}
        Some(_) => return Err(invalido("`orientation` non riconosciuta")),
    }
    if let Some(bbox) = colonna.get("bbox") {
        let valido = bbox.as_array().is_some_and(|valori| {
            (valori.len() == 4 || valori.len() == 6)
                && valori
                    .iter()
                    .all(|v| v.as_f64().is_some_and(f64::is_finite))
        });
        if !valido {
            return Err(invalido("`bbox` non e' un elenco di 4 o 6 numeri finiti"));
        }
    }
    if colonna.get("covering").is_some_and(|c| !c.is_object()) {
        return Err(invalido("`covering` non e' un oggetto"));
    }
    let Some(Value::Array(nomi)) = colonna.get("geometry_types") else {
        return Err(invalido("`geometry_types` assente o non elenco"));
    };
    let mut tipi = BTreeSet::new();
    for nome in nomi {
        let coppia = nome
            .as_str()
            .and_then(tipo_da_nome)
            .ok_or_else(|| invalido("`geometry_types` con un tipo fuori dalla specifica"))?;
        if !tipi.insert(coppia) {
            return Err(invalido("`geometry_types` con un tipo ripetuto"));
        }
    }
    let crs = match colonna.get("crs") {
        None => CrsColonna::Integrato("OGC:CRS84".to_owned()),
        Some(Value::Null) => CrsColonna::Assente,
        Some(documento) => CrsColonna::Integrato(identificativo_da_projjson(documento)?),
    };
    Ok(ColonnaGeo { crs, tipi })
}

/// La colonna come `BinaryArray`: `LargeBinary` si converte (la specifica
/// chiede ai lettori di accettarlo), con offset che devono stare in `i32`.
fn come_binary(colonna: &ArrayRef) -> Result<Arc<BinaryArray>> {
    if let Some(binary) = colonna.as_any().downcast_ref::<BinaryArray>() {
        return Ok(Arc::new(binary.clone()));
    }
    if let Some(large) = colonna.as_any().downcast_ref::<LargeBinaryArray>() {
        let byte = large.value_data().len();
        if i32::try_from(byte).is_err() {
            return Err(PlenoraError::ResourceLimit(
                "colonna LargeBinary oltre i 2 GiB di un Binary".to_owned(),
            ));
        }
        return Ok(Arc::new(large.iter().collect::<BinaryArray>()));
    }
    Err(PlenoraError::Unsupported(format!(
        "tipo Arrow {} non ammesso per una colonna WKB (attesi Binary o LargeBinary)",
        plenora_core::tipo_arrow::descrivi_tipo(colonna.data_type())
    )))
}

fn sommario(celle: &BinaryArray) -> Result<Sommario> {
    let mut sommario = Sommario::default();
    for cella in celle.iter().flatten() {
        scansiona_cella(cella, &mut sommario)?;
    }
    Ok(sommario)
}

/// Il metadato di campo `geo` letto: `crs` (se c'è), dimensionalità,
/// encoding.
///
/// `dimensions` c'è solo se `geometry_types` la decide (`xy` o `xyz`): un
/// elenco vuoto non dice nulla, e una dimensionalità già dichiarata dalle
/// chiavi canoniche del campo resta quella (nessun conflitto con un
/// `unknown` inventato qui).
fn geo_di_campo(crs: &CrsColonna, dimensioni: GeometryDimensions) -> Result<String> {
    let mut mappa = Map::new();
    if let CrsColonna::Integrato(identificativo) = crs {
        mappa.insert("crs".to_owned(), Value::String(identificativo.clone()));
    }
    if dimensioni != GeometryDimensions::Unknown {
        mappa.insert(
            "dimensions".to_owned(),
            Value::String(dimensioni.as_str().to_owned()),
        );
    }
    mappa.insert(
        "encoding".to_owned(),
        Value::String(GeometryEncoding::Wkb.as_str().to_owned()),
    );
    Ok(serde_json::to_string(&Value::Object(mappa))?)
}

/// Applica il metadato di file `geo` a una tabella letta da Parquet.
///
/// # Errors
///
/// `InvalidPlan` per un metadato malformato o incoerente con lo schema;
/// `Unsupported` per ciò che la specifica ammette e il contratto no; `Crs`
/// per un CRS fuori tabella; `DataMapping` per celle WKB non ammesse o tipi
/// non dichiarati.
pub fn applica(tabella: &RecordBatch, testo_geo: &str) -> Result<RecordBatch> {
    let geo = analizza(testo_geo)?;
    let schema = tabella.schema();
    for nome in geo.colonne.keys() {
        if schema.column_with_name(nome).is_none() {
            return Err(invalido("colonna dichiarata assente dallo schema"));
        }
    }
    let mut campi = Vec::with_capacity(schema.fields().len());
    let mut colonne = Vec::with_capacity(schema.fields().len());
    let mut prima_geometrica: Option<&str> = None;
    for (campo, colonna) in schema.fields().iter().zip(tabella.columns()) {
        let Some(dichiarata) = geo.colonne.get(campo.name()) else {
            if field_declares_wkb_geometry(campo) {
                return Err(di_colonna(
                    campo.name(),
                    invalido("colonna geometrica non dichiarata nel metadato di file"),
                ));
            }
            campi.push(campo.as_ref().clone());
            colonne.push(colonna.clone());
            continue;
        };
        prima_geometrica.get_or_insert(campo.name());
        let errore = |e| di_colonna(campo.name(), e);
        if let Some(estensione) = campo.metadata().get(GEOARROW_EXTENSION_KEY) {
            if estensione != GEOARROW_WKB_EXTENSION {
                // Il valore letto non entra nel messaggio («errori senza dati»).
                return Err(errore(PlenoraError::Unsupported(format!(
                    "`{GEOARROW_EXTENSION_KEY}` diversa da `{GEOARROW_WKB_EXTENSION}`: \
                     incoerente con encoding WKB"
                ))));
            }
        }
        let celle = come_binary(colonna).map_err(errore)?;
        let trovati = sommario(&celle).map_err(errore)?;
        if !dichiarata.tipi.is_empty() && !trovati.tipi.is_subset(&dichiarata.tipi) {
            return Err(errore(PlenoraError::DataMapping(
                "`geometry_types` dichiarati non coerenti con le geometrie".to_owned(),
            )));
        }
        let mut metadati = campo.metadata().clone();
        metadati.remove(EXTENSION_METADATA_KEY);
        metadati.insert(
            GEOARROW_EXTENSION_KEY.to_owned(),
            GEOARROW_WKB_EXTENSION.to_owned(),
        );
        metadati.insert(
            GEO_METADATA_KEY.to_owned(),
            geo_di_campo(&dichiarata.crs, dichiarata.dimensioni())?,
        );
        campi.push(
            Field::new(campo.name(), DataType::Binary, campo.is_nullable()).with_metadata(metadati),
        );
        colonne.push(celle as ArrayRef);
    }
    if prima_geometrica != Some(geo.primaria.as_str()) {
        return Err(PlenoraError::Unsupported(
            "`primary_column` non e' la prima colonna geometrica dello schema: il contratto \
             elegge la prima come geometria attiva"
                .to_owned(),
        ));
    }
    let mut metadati_schema = schema.metadata().clone();
    metadati_schema.remove(GEO_METADATA_KEY);
    let schema_letto = Arc::new(Schema::new_with_metadata(campi, metadati_schema));
    let mut contratto = contract_from_arrow_schema(schema_letto, resolve_crs)?;
    for geometria in &mut contratto.geometries {
        let dichiarata = geo.colonne.get(&geometria.name).ok_or_else(|| {
            invalido("colonna geometrica del contratto non dichiarata nel metadato di file")
        })?;
        if dichiarata.tipi.is_empty() {
            continue;
        }
        let tipi: Vec<GeometryType> = dichiarata.tipi.iter().map(|(tipo, _)| *tipo).collect();
        let proprieta = GeometryTypesProperty::new(TypesDeclaration::Exact, tipi)
            .map_err(|errore| PlenoraError::Internal(errore.to_string()))?;
        match geometria.types.value() {
            None => {
                geometria.types = ContractProperty::new(
                    PropertyConfidence::Declared(proprieta),
                    PropertyScope::Schema,
                );
            }
            Some(esistente) if esistente.types() == proprieta.types() => {}
            Some(_) => {
                return Err(di_colonna(
                    &geometria.name,
                    invalido("`geometry_types` diversi dai tipi canonici del campo"),
                ))
            }
        }
    }
    if contratto.geometries.len() != geo.colonne.len() {
        return Err(invalido(
            "colonne dichiarate e colonne geometriche dello schema diverse",
        ));
    }
    contratto.validate()?;
    let schema_finale = arrow_schema_from_contract(&contratto)?;
    plenora_core::batch_with_rows(schema_finale, colonne, tabella.num_rows())
}

/// Tabella pronta da scrivere come `GeoParquet`: schema senza i metadati che
/// il file porta altrove, e il testo del metadato di file `geo`.
pub struct Preparata {
    pub tabella: RecordBatch,
    pub geo: String,
}

/// Prepara la scrittura `GeoParquet`; `None` se la tabella non ha colonne
/// geometriche.
///
/// # Errors
///
/// `InvalidPlan` per un metadato di schema `geo` senza colonne geometriche
/// o un contratto non valido; `Unsupported` per EWKB, coordinate M, ordine
/// degli assi diverso da x = est/longitudine; `Crs` per un CRS non integrato
/// o dichiarato ma non risolto; `DataMapping` per celle non ammesse o tipi
/// fuori dalla dichiarazione del contratto.
pub fn prepara(tabella: &RecordBatch) -> Result<Option<Preparata>> {
    let schema = tabella.schema();
    let geometrica = schema.fields().iter().any(|campo| {
        field_declares_wkb_geometry(campo) || campo.metadata().contains_key(GEO_METADATA_KEY)
    });
    if !geometrica {
        if schema.metadata().contains_key(GEO_METADATA_KEY) {
            return Err(PlenoraError::InvalidPlan(
                "metadato di schema `geo` senza colonne geometriche: e' riservato a GeoParquet"
                    .to_owned(),
            ));
        }
        return Ok(None);
    }
    let contratto = contract_from_arrow_schema(schema.clone(), resolve_crs)?;
    let mut colonne_geo = Map::new();
    let mut nomi_geometrici = BTreeSet::new();
    for geometria in &contratto.geometries {
        let (indice, campo) = schema
            .column_with_name(&geometria.name)
            .ok_or_else(|| PlenoraError::Internal("geometria senza campo".to_owned()))?;
        let voce = colonna_da_scrivere(geometria, campo, tabella.column(indice))
            .map_err(|errore| di_colonna(&geometria.name, errore))?;
        colonne_geo.insert(geometria.name.clone(), voce);
        nomi_geometrici.insert(geometria.name.clone());
    }
    let primaria = contratto
        .active_geometry_column()
        .map(|geometria| geometria.name.clone())
        .ok_or_else(|| PlenoraError::Internal("contratto geometrico senza attiva".to_owned()))?;
    let mut radice = Map::new();
    radice.insert(
        "version".to_owned(),
        Value::String(VERSIONE_SCRITTA.to_owned()),
    );
    radice.insert("primary_column".to_owned(), Value::String(primaria));
    radice.insert("columns".to_owned(), Value::Object(colonne_geo));
    let testo = serde_json::to_string(&Value::Object(radice))?;

    let campi: Vec<Field> = schema
        .fields()
        .iter()
        .map(|campo| {
            if nomi_geometrici.contains(campo.name()) {
                let mut metadati = campo.metadata().clone();
                metadati.remove(GEO_METADATA_KEY);
                metadati.remove(EXTENSION_METADATA_KEY);
                campo.as_ref().clone().with_metadata(metadati)
            } else {
                campo.as_ref().clone()
            }
        })
        .collect();
    let mut metadati_schema = schema.metadata().clone();
    metadati_schema.remove(GEO_METADATA_KEY);
    let schema_scritto = Arc::new(Schema::new_with_metadata(campi, metadati_schema));
    let tabella = plenora_core::batch_with_rows(
        schema_scritto,
        tabella.columns().to_vec(),
        tabella.num_rows(),
    )?;
    Ok(Some(Preparata {
        tabella,
        geo: testo,
    }))
}

fn colonna_da_scrivere(
    geometria: &plenora_core::contract::GeometryColumnContract,
    campo: &Field,
    colonna: &ArrayRef,
) -> Result<Value> {
    if geometria.encoding == Some(GeometryEncoding::Ewkb) {
        return Err(PlenoraError::Unsupported(
            "encoding EWKB: GeoParquet vuole WKB ISO".to_owned(),
        ));
    }
    match canonical_geometry_axis_order(campo)? {
        None | Some(AxisOrder::LonLat | AxisOrder::EastingNorthing) => {}
        Some(_) => {
            return Err(PlenoraError::Unsupported(
                "ordine degli assi diverso da x = est/longitudine: GeoParquet lo impone e le \
                 coordinate non si scambiano"
                    .to_owned(),
            ))
        }
    }
    let crs = match &geometria.crs {
        ContractCrs::Resolved(crs) | ContractCrs::ResolvedByDecision(crs) => {
            projjson_di(&identificativo_di(crs)?)?
        }
        ContractCrs::Missing => Value::Null,
        ContractCrs::DeclaredUnresolved { .. } => {
            return Err(PlenoraError::Crs(
                "CRS dichiarato ma non risolto: GeoParquet vuole un PROJJSON".to_owned(),
            ))
        }
    };
    let celle = colonna
        .as_any()
        .downcast_ref::<BinaryArray>()
        .ok_or_else(|| PlenoraError::Internal("colonna geometrica non Binary".to_owned()))?;
    let trovati = sommario(celle)?;
    let z_dati: BTreeSet<bool> = trovati.tipi.iter().map(|(_, z)| *z).collect();
    let z_colonna = match geometria.dimensions {
        GeometryDimensions::Xy => Some(false),
        GeometryDimensions::Xyz => Some(true),
        GeometryDimensions::Xym | GeometryDimensions::Xyzm => {
            return Err(PlenoraError::Unsupported(
                "coordinate M: GeoParquet 1.1 non le ammette".to_owned(),
            ))
        }
        GeometryDimensions::Unknown => None,
    };
    if let Some(z) = z_colonna {
        if z_dati.iter().any(|trovata| *trovata != z) {
            return Err(PlenoraError::DataMapping(
                "dimensionalita' delle geometrie diversa da quella del contratto".to_owned(),
            ));
        }
    }
    let dichiarazione = geometria.types.value();
    if let Some(dichiarati) = dichiarazione.map(GeometryTypesProperty::types) {
        if !dichiarati.is_empty()
            && trovati
                .tipi
                .iter()
                .any(|(tipo, _)| !dichiarati.contains(tipo))
        {
            return Err(PlenoraError::DataMapping(
                "geometrie di un tipo non dichiarato dal contratto".to_owned(),
            ));
        }
    }
    // `geometry_types` dice tipi e dimensionalita' insieme: si scrive solo
    // cio' che il contratto decide, cosi' la rilettura ridà lo stesso
    // contratto. Tipi dichiarati con dimensionalita' nota: l'elenco
    // dichiarato. Tipi non dichiarati con dimensionalita' nota: i tipi dei
    // dati (tutti della dimensionalita' del contratto, verificato sopra).
    // Altrimenti (dimensionalita' `unknown`, dichiarazione senza elenco):
    // elenco vuoto, «sconosciuti».
    let coppie: Vec<(GeometryType, bool)> = match (z_colonna, dichiarazione) {
        (Some(z), Some(dichiarati)) if !dichiarati.types().is_empty() => {
            dichiarati.types().iter().map(|tipo| (*tipo, z)).collect()
        }
        (Some(_), None) => trovati.tipi.iter().copied().collect(),
        _ => Vec::new(),
    };
    let mut nomi = Vec::with_capacity(coppie.len());
    for (tipo, z) in coppie {
        // Il tipo viene dai metadati o dalle celle: non entra nel messaggio.
        let etichetta = nome_geoparquet(tipo, z).ok_or_else(|| {
            PlenoraError::Unsupported(
                "tipo geometrico fuori dai sette tipi di GeoParquet 1.1".to_owned(),
            )
        })?;
        nomi.push(Value::String(etichetta));
    }
    let mut voce = Map::new();
    voce.insert("encoding".to_owned(), Value::String("WKB".to_owned()));
    voce.insert("geometry_types".to_owned(), Value::Array(nomi));
    voce.insert("crs".to_owned(), crs);
    if let (Some(riquadro), false) = (trovati.riquadro, z_dati.contains(&true)) {
        voce.insert(
            "bbox".to_owned(),
            serde_json::json!([riquadro.xmin, riquadro.ymin, riquadro.xmax, riquadro.ymax]),
        );
    }
    Ok(Value::Object(voce))
}
