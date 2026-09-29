//! Oracolo del round-trip semantico fra schema Arrow e contratto dati.
//!
//! # Perche' esiste
//!
//! [`contract_from_arrow_schema`] e [`arrow_schema_from_contract`] sono la
//! stessa autorità vista dai due lati. Appena uno schema esce dal processo
//! (un file scritto da `plenora-io` e riletto, da questo o da un altro
//! processo) questo smette di essere una comodità e diventa un contratto:
//! chi rilegge deve trovare ciò che chi ha scritto intendeva, e se le due
//! direzioni non compongono, un confronto fra scritto e riletto confronta
//! due interpretazioni invece di due risultati.
//!
//! # Gli invarianti, e perche' NON sono la simmetria
//!
//! Le due direzioni non sono speculari, di proposito:
//!
//! - in **lettura** si preserva cio' che la sorgente dichiara: un CRS assente
//!   resta `Missing`, un'incoerenza resta `DeclaredUnresolved`;
//! - in **scrittura** si emette il canone e si rifiutano i conflitti.
//!
//! Quello che deve valere non e' `schema == round_trip(schema)` — falso e
//! giustamente falso, perche' la scrittura AGGIUNGE le chiavi canoniche a uno
//! schema che non le aveva. E' che il **contratto** sopravviva:
//!
//! > `contratto -> schema -> contratto` restituisce lo stesso contratto.
//!
//! È l'unica formulazione che dice ciò che serve a chi scrive: che scrivere
//! e rileggere non perda né inventi nulla.

use std::collections::HashMap;
use std::sync::Arc;

use plenora_core::arrow::schema::{DataType, Field, Schema};
use plenora_core::contract::arrow_metadata::{
    GEOARROW_EXTENSION_KEY, GEOARROW_WKB_EXTENSION, PLENORA_GEOMETRY_NAMESPACE_PREFIX,
};
use plenora_core::contract::arrow_schema::{
    arrow_schema_from_contract, contract_from_arrow_schema,
};
use plenora_core::contract::{
    ContractCrs, ContractProperties, DataContract, FieldId, GeometryColumnContract,
    GeometryDimensions,
};
use plenora_core::crs::resolve_crs;

/// Forma confrontabile di un [`ContractCrs`].
///
/// Il tipo non implementa `PartialEq`, e non e' il caso di aggiungerglielo
/// per un test: l'uguaglianza di due CRS risolti e' una domanda semantica
/// (due definizioni diverse possono descrivere lo stesso sistema) e un
/// `derive` la banalizzerebbe. Qui serve solo sapere se il round-trip ha
/// restituito la STESSA cosa, e il `Debug` lo dice senza pretendere di piu'.
fn forma_del_crs(crs: &ContractCrs) -> String {
    format!("{crs:?}")
}

/// Campo geometria riconoscibile: estensione `GeoArrow` e tipo `Binary`.
fn campo_geometria(nome: &str) -> Field {
    let mut metadata = HashMap::new();
    metadata.insert(
        GEOARROW_EXTENSION_KEY.to_owned(),
        GEOARROW_WKB_EXTENSION.to_owned(),
    );
    Field::new(nome, DataType::Binary, true).with_metadata(metadata)
}

fn contratto(crs: ContractCrs, dimensioni: GeometryDimensions) -> DataContract {
    let schema = Arc::new(Schema::new(vec![
        Field::new("id", DataType::Int64, false),
        campo_geometria("geom"),
    ]));
    DataContract::new(
        schema,
        vec![GeometryColumnContract {
            field_id: FieldId(0),
            name: "geom".to_owned(),
            crs,
            dimensions: dimensioni,
            encoding: None,
            nullable: true,
            types: GeometryColumnContract::undeclared_types(),
        }],
        Some(FieldId(0)),
        ContractProperties::default(),
    )
    .expect("contratto coerente con lo schema")
}

/// I casi che il round-trip può attraversare **senza PROJ**.
///
/// Fra i CRS risolti ci sono solo quelli della tabella integrata: per ogni
/// altra definizione il risolutore di base rifiuta la validazione
/// (`CRS_BACKEND_UNAVAILABLE`) invece di fidarsi, quindi rileggere uno schema
/// che la dichiara risolta FALLISCE. È fail-closed corretto, e il round-trip
/// di quei CRS resta possibile solo con un risolutore del chiamante che li
/// risolva.
fn casi() -> Vec<(&'static str, DataContract)> {
    let integrato = |definizione: &str| {
        ContractCrs::Resolved(resolve_crs(definizione, "crs").expect("CRS integrato"))
    };
    vec![
        (
            "crs_integrato_utm",
            contratto(integrato("EPSG:32632"), GeometryDimensions::Xy),
        ),
        (
            "crs_integrato_geografico_lat_lon",
            contratto(integrato("EPSG:4326"), GeometryDimensions::Xy),
        ),
        (
            "crs_integrato_crs84",
            contratto(integrato("OGC:CRS84"), GeometryDimensions::Xy),
        ),
        (
            "crs_integrato_northing_first",
            contratto(integrato("EPSG:6707"), GeometryDimensions::Xy),
        ),
        (
            "crs_assente",
            contratto(ContractCrs::Missing, GeometryDimensions::Xy),
        ),
        (
            "dimensioni_xyz_senza_crs",
            contratto(ContractCrs::Missing, GeometryDimensions::Xyz),
        ),
        (
            "senza_geometrie",
            DataContract::tabular(Arc::new(Schema::new(vec![
                Field::new("id", DataType::Int64, false),
                Field::new("valore", DataType::Float64, true),
            ]))),
        ),
    ]
}

/// **Invariante 1.** `contratto -> schema -> contratto` conserva il contratto.
///
/// Non lo schema: la scrittura aggiunge le chiavi canoniche, ed e' cio' che
/// deve fare. Il contratto invece deve tornare identico, altrimenti chi
/// rilegge leggerebbe qualcosa di diverso da ciò che chi scrive intende.
#[test]
fn il_contratto_sopravvive_al_round_trip() {
    for (nome, originale) in casi() {
        let schema = arrow_schema_from_contract(&originale)
            .unwrap_or_else(|errore| panic!("{nome}: emissione fallita: {errore}"));
        let riletto = contract_from_arrow_schema(schema, resolve_crs)
            .unwrap_or_else(|errore| panic!("{nome}: rilettura fallita: {errore}"));

        assert_eq!(
            riletto.geometries.len(),
            originale.geometries.len(),
            "{nome}: numero di colonne geometriche"
        );
        for (dopo, prima) in riletto.geometries.iter().zip(&originale.geometries) {
            assert_eq!(dopo.name, prima.name, "{nome}: nome della colonna");
            assert_eq!(
                forma_del_crs(&dopo.crs),
                forma_del_crs(&prima.crs),
                "{nome}: CRS"
            );
            assert_eq!(dopo.dimensions, prima.dimensions, "{nome}: dimensionalita'");
            assert_eq!(dopo.nullable, prima.nullable, "{nome}: nullabilita'");
            assert_eq!(dopo.types, prima.types, "{nome}: tipi dichiarati");
        }
        assert_eq!(
            riletto.active_geometry, originale.active_geometry,
            "{nome}: geometria attiva"
        );
    }
}

/// **Invariante 2.** Il round-trip converge, e converge in un giro.
///
/// # Che cosa NON vale, e perche'
///
/// La prima emissione **non** e' un punto fisso, e scoprirlo e' stato il
/// motivo per cui questo test esiste nella forma che ha. Un contratto
/// costruito a mano con `encoding: None` produce uno schema senza la chiave
/// `plenora.geometry.encoding`; rileggendolo, il lettore la COMPLETA a `wkb`
/// deducendola dall'estensione `GeoArrow` (completamento per precedenza); la
/// seconda
/// emissione la scrive. Prima e seconda emissione differiscono quindi di una
/// chiave.
///
/// Non e' un difetto: il completamento e' esattamente cio' che il lettore
/// deve fare, e l'emissione scrive fedelmente cio' che il contratto dice
/// DOPO il completamento. Sarebbe un difetto il contrario — un lettore che
/// tace su un encoding deducibile, o un'emissione che lo omette.
///
/// # Che cosa vale
///
/// Dal secondo giro in poi lo schema è stabile. È la proprietà che serve a
/// chi scrive e rilegge: due letture successive devono coincidere. Se il
/// round-trip non convergesse, ogni passaggio sposterebbe i metadati e un
/// confronto fra scritto e riletto confronterebbe cose diverse a ogni giro.
#[test]
fn il_round_trip_converge_in_un_giro() {
    for (nome, contratto) in casi() {
        let primo = arrow_schema_from_contract(&contratto).expect("prima emissione");
        let dopo_uno = contract_from_arrow_schema(primo, resolve_crs).expect("prima rilettura");
        let secondo = arrow_schema_from_contract(&dopo_uno).expect("seconda emissione");
        let dopo_due =
            contract_from_arrow_schema(secondo.clone(), resolve_crs).expect("seconda rilettura");
        let terzo = arrow_schema_from_contract(&dopo_due).expect("terza emissione");

        assert_eq!(
            secondo, terzo,
            "{nome}: lo schema non e' stabile dal secondo giro"
        );
    }
}

/// **Invariante 3.** Nessun CRS viene inventato.
///
/// Uno schema che non dichiara CRS deve produrre un contratto con
/// [`ContractCrs::Missing`]. Un default silenzioso qui sarebbe il difetto
/// peggiore possibile: darebbe coordinate a un sistema di riferimento che
/// nessuno ha scelto.
#[test]
fn un_crs_assente_resta_assente() {
    let schema = Arc::new(Schema::new(vec![
        Field::new("id", DataType::Int64, false),
        campo_geometria("geom"),
    ]));
    let contratto = contract_from_arrow_schema(schema, resolve_crs).expect("lettura");
    assert_eq!(contratto.geometries.len(), 1);
    assert!(
        matches!(contratto.geometries[0].crs, ContractCrs::Missing),
        "un CRS assente non va inventato, ottenuto {:?}",
        contratto.geometries[0].crs
    );
}

/// **Invariante 4.** La conversione non legge dati.
///
/// Lavora su uno schema, e uno schema non ha righe. Il test lo dice
/// costruendo il contratto da uno schema che non ha alcun batch associato:
/// se un giorno la conversione volesse dei dati, non compilerebbe.
#[test]
fn la_conversione_lavora_solo_sullo_schema() {
    let schema = Arc::new(Schema::new(vec![Field::new("id", DataType::Int64, false)]));
    let contratto = contract_from_arrow_schema(schema.clone(), resolve_crs).expect("lettura");
    assert_eq!(contratto.schema, schema);
    assert!(contratto.geometries.is_empty());
}

/// **Invariante 5.** Un metadato canonico divergente non viene sovrascritto.
///
/// Se il campo dichiara gia' una chiave canonica con un valore diverso da
/// quello del contratto, il conflitto e' un ERRORE invece di un arbitrato.
/// Arbitrare significherebbe che lo schema pubblicato non dice quale delle
/// due versioni vale.
///
/// La garanzia sta piu' a monte di dove la si cercherebbe: e'
/// [`DataContract::new`] a rifiutare la coppia, prima ancora che l'emissione
/// venga invocata. Il test lo dice esplicitamente perche' e' una scoperta:
/// un contratto incoerente col proprio schema non arriva mai al confine.
#[test]
fn un_metadato_divergente_e_un_errore_non_un_arbitrato() {
    let mut metadata = HashMap::new();
    metadata.insert(
        GEOARROW_EXTENSION_KEY.to_owned(),
        GEOARROW_WKB_EXTENSION.to_owned(),
    );
    // Dichiara dimensioni diverse da quelle del contratto.
    metadata.insert(
        format!("{PLENORA_GEOMETRY_NAMESPACE_PREFIX}dimensions"),
        "xyz".to_owned(),
    );
    let schema = Arc::new(Schema::new(vec![
        Field::new("id", DataType::Int64, false),
        Field::new("geom", DataType::Binary, true).with_metadata(metadata),
    ]));
    let esito = DataContract::new(
        schema,
        vec![GeometryColumnContract {
            field_id: FieldId(0),
            name: "geom".to_owned(),
            crs: ContractCrs::Missing,
            // ...mentre il contratto dice `xy`.
            dimensions: GeometryDimensions::Xy,
            encoding: None,
            nullable: true,
            types: GeometryColumnContract::undeclared_types(),
        }],
        Some(FieldId(0)),
        ContractProperties::default(),
    );

    assert!(
        esito.is_err(),
        "una chiave canonica divergente deve fallire, non essere sovrascritta"
    );
}
