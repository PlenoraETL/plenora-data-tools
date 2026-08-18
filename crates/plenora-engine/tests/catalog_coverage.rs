//! Gate di completezza: ogni operazione del catalogo ha il proprio analyzer
//! di contratto.
//!
//! # Perche' esiste
//!
//! Decode, validazione, `prepare` e dispatch delle operazioni vivono in
//! grandi `match` separati, uno per percorso. Nessun meccanismo del
//! linguaggio lega quei `match` al catalogo: aggiungere un descrittore e
//! dimenticare uno dei percorsi compila, passa i test esistenti — che
//! elencano i casi a mano — e fallisce solo quando qualcuno usa davvero
//! l'operazione nuova.
//!
//! Questo test chiude proprio quel varco per il percorso piu' esposto,
//! l'analisi a secco del contratto: e' quello che decide se un piano e'
//! valido, e un'operazione senza analyzer viene accettata dal parser del
//! piano per poi fallire in `validate`. Il test itera il CATALOGO, non un
//! elenco scritto a mano: un'op nuova senza analyzer fa fallire la CI il
//! giorno stesso in cui entra nel catalogo.
//!
//! Non sostituisce la rappresentazione tipizzata unica delle operazioni —
//! quella resta il modo strutturale di eliminare il problema — ma ne
//! fornisce la garanzia osservabile a costo nullo.

use std::collections::HashMap;
use std::sync::Arc;

use plenora_core::arrow::schema::{DataType, Field, Schema};
use plenora_core::catalog::{Arity, Family, Maturity, CATALOG};
use plenora_core::contract::{
    ContractCrs, ContractProperties, DataContract, FieldAllocator, FieldId, GeometryColumnContract,
    GeometryDimensions,
};
use plenora_core::PlenoraError;
use plenora_kernels_geo::analyze::analyze_geo_contract;
use plenora_kernels_geo::arrow_adapter::{GEOARROW_EXTENSION_KEY, GEOARROW_WKB_EXTENSION};
use plenora_kernels_table::analyze::analyze_table_contract;
use serde_json::json;

fn tabular_contract() -> DataContract {
    DataContract::tabular(Arc::new(Schema::new(vec![
        Field::new("id", DataType::Int64, false),
        Field::new("text", DataType::Utf8, true),
        Field::new("num", DataType::Float64, true),
    ])))
}

/// Contratto geo la cui colonna e' IDENTIFICABILE dal trasporto.
///
/// Senza i metadati dell'estensione `GeoArrow` l'analyzer geo fallisce in
/// `require_identifiable_geometry`, cioe' PRIMA del `match` sull'operazione:
/// il gate non raggiungerebbe il punto che vuole verificare, e ogni op
/// risulterebbe coperta. E' il difetto che la prima versione di questo test
/// aveva.
fn geo_contract() -> DataContract {
    let mut metadata = HashMap::new();
    metadata.insert(
        GEOARROW_EXTENSION_KEY.to_owned(),
        GEOARROW_WKB_EXTENSION.to_owned(),
    );
    let schema = Arc::new(Schema::new(vec![
        Field::new("id", DataType::Int64, false),
        Field::new("geom", DataType::Binary, true).with_metadata(metadata),
    ]));
    DataContract {
        schema,
        geometries: vec![GeometryColumnContract {
            field_id: FieldId(0),
            name: "geom".to_owned(),
            crs: ContractCrs::Missing,
            dimensions: GeometryDimensions::Xy,
            encoding: None,
            nullable: true,
            types: GeometryColumnContract::undeclared_types(),
        }],
        active_geometry: Some(FieldId(0)),
        properties: ContractProperties::default(),
    }
}

/// Esegue l'analyzer della famiglia giusta con una config vuota.
fn analyze(
    descriptor: &plenora_core::catalog::OperationDescriptor,
) -> Result<DataContract, PlenoraError> {
    // L'arieta' va rispettata: il controllo di arieta' precede il match del
    // dispatch, e sbagliarla farebbe uscire prima del punto che interessa.
    let contract = match descriptor.family {
        Family::Table => tabular_contract(),
        Family::Geo => geo_contract(),
    };
    // Per le N-arie due input sono la forma minima accettata dal dispatch.
    let inputs: Vec<DataContract> = match descriptor.arity {
        Arity::Unary => vec![contract],
        Arity::BinaryOrdered | Arity::NAry => vec![contract.clone(), contract],
    };
    let mut fields = FieldAllocator::default();
    // La config vuota fa quasi sempre fallire la validazione dell'op: e'
    // voluto. Interessa solo NON incontrare il ramo `_ =>` del dispatch.
    match descriptor.family {
        Family::Table => analyze_table_contract(descriptor.id, &inputs, &json!({}), &mut fields),
        Family::Geo => analyze_geo_contract(descriptor.id, &inputs, &json!({}), None, &mut fields),
    }
}

/// Op che dichiarano capability non compilate in questa build: il loro
/// analyzer esiste, ma l'assenza del backend e' anch'essa un `Unsupported` e
/// va distinta dal ramo `_ =>` del dispatch.
fn richiede_backend_assente(descriptor: &plenora_core::catalog::OperationDescriptor) -> bool {
    descriptor.maturity == Maturity::BackendPending
        || descriptor.required_capabilities.iter().any(|capability| {
            (*capability == "geos" && !cfg!(feature = "geos-backend"))
                || (*capability == "proj" && !cfg!(feature = "proj-backend"))
        })
}

/// Op il cui analyzer esiste ed e' RAGGIUNTO, ma risponde `Unsupported` per
/// una ragione semantica dichiarata.
///
/// E' un elenco scritto a mano, ma minuscolo e verificato: il test fallisce
/// sia se un'op nuova finisce fuori dal dispatch, sia se una voce qui dentro
/// diventa stantia. Non e' la matrice di 150 casi che questo gate sostituisce.
const UNSUPPORTED_DICHIARATO: [&str; 1] = [
    // Il numero di colonne di output dipende dal numero di RIGHE dell'input:
    // lo schema non e' inferibile a secco, ed e' esattamente cio' che
    // l'analyzer dichiara.
    "table.transpose",
];

#[test]
fn le_op_con_unsupported_dichiarato_sono_davvero_tali() {
    // Una voce stantia nell'elenco renderebbe il gate cieco su quell'op.
    for id in UNSUPPORTED_DICHIARATO {
        let descriptor = CATALOG
            .iter()
            .find(|descriptor| descriptor.id == id)
            .unwrap_or_else(|| panic!("`{id}` non e' piu' in catalogo: voce stantia"));
        assert!(
            matches!(analyze(descriptor), Err(PlenoraError::Unsupported(_))),
            "`{id}` non risponde piu' Unsupported: va tolta dall'elenco"
        );
    }
}

#[test]
fn ogni_operazione_del_catalogo_ha_un_analyzer_di_contratto() {
    // Il criterio e' STRUTTURALE, non testuale: il ramo `_ =>` di entrambi i
    // dispatch restituisce `Unsupported`, mentre un analyzer che esiste,
    // messo davanti a una config vuota, fallisce sulla config o sul contratto
    // — `InvalidPlan` o `Schema`. Cercare due stringhe precise avrebbe reso
    // il gate verde per sempre al primo messaggio riscritto.
    //
    // Le op `Planned` sono dichiarate come non ancora implementate: il
    // catalogo le espone apposta. Quelle che richiedono un backend non
    // compilato producono anch'esse `Unsupported`, per un motivo diverso.
    let mut senza_analyzer: Vec<(&str, String)> = Vec::new();
    let mut verificate = 0_usize;
    for descriptor in CATALOG {
        if descriptor.maturity == Maturity::Planned
            || richiede_backend_assente(descriptor)
            || UNSUPPORTED_DICHIARATO.contains(&descriptor.id)
        {
            continue;
        }
        verificate += 1;
        if let Err(PlenoraError::Unsupported(message)) = analyze(descriptor) {
            senza_analyzer.push((descriptor.id, message));
        }
    }
    assert!(
        verificate > 0,
        "nessuna operazione verificata: il gate non sta guardando niente"
    );
    assert!(
        senza_analyzer.is_empty(),
        "operazioni nel catalogo il cui analyzer di contratto non e' raggiungibile \
         ({verificate} verificate): {senza_analyzer:?}"
    );
}

#[test]
fn il_gate_riconosce_davvero_un_analyzer_irraggiungibile() {
    // Controllo del controllo. Il ramo `_ =>` del dispatch restituisce
    // `Unsupported`: qui si produce deliberatamente quella stessa classe di
    // errore — un'op del catalogo instradata all'analyzer della famiglia
    // sbagliata — e si verifica che il criterio del gate la riconosca. Senza
    // questo, un criterio che non discrimina renderebbe il gate sempre verde.
    let geo = CATALOG
        .iter()
        .find(|descriptor| descriptor.family == Family::Geo)
        .expect("almeno un'operazione geo in catalogo");
    let mut fields = FieldAllocator::default();
    let error = analyze_table_contract(geo.id, &[tabular_contract()], &json!({}), &mut fields)
        .expect_err("famiglia sbagliata");
    assert!(
        matches!(error, PlenoraError::Unsupported(_)),
        "il criterio del gate non riconoscerebbe un analyzer irraggiungibile: {error:?}"
    );

    // E un'op che l'analyzer copre davvero NON produce `Unsupported`: senza
    // questa meta' il criterio potrebbe essere vero per costruzione.
    let filtro = CATALOG
        .iter()
        .find(|descriptor| descriptor.id == "table.filter")
        .expect("table.filter in catalogo");
    assert!(
        !matches!(analyze(filtro), Err(PlenoraError::Unsupported(_))),
        "un'op coperta non deve risultare irraggiungibile"
    );
}
