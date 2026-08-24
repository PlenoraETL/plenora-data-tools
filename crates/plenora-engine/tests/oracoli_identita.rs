//! Oracolo dell'identita' dei piani: `plan_hash`, fingerprint del catalogo e
//! dei contratti di input, per un insieme di piani rappresentativi.
//!
//! # Perche' esiste
//!
//! Il `plan_hash` e' `SHA256(dominio ‖ canonical_json)`: dipende dalla forma
//! canonica del piano, cioe' dall'ordine delle chiavi, dalla formattazione dei
//! numeri e da quali campi entrano nella canonicalizzazione. Nessuna di queste
//! cose e' visibile leggendo un diff.
//!
//! Fino al 2026-08-24 i test verificavano che `plan_hash` fosse **lungo 64
//! caratteri esadecimali** e che un piano v4 e il suo equivalente v5 dessero lo
//! stesso valore. Entrambe le proprieta' sono vere anche se il valore cambia:
//! una riorganizzazione che alterasse l'ordine delle chiavi canoniche avrebbe
//! cambiato l'identita' di **ogni piano gia' emesso** senza far fallire nulla.
//! Un consumatore che conserva `plan_hash` per riconoscere un piano gia' visto
//! avrebbe smesso di riconoscerlo, in silenzio.
//!
//! Questo oracolo chiude quel varco: fissa i valori, non la loro forma.
//!
//! # Perche' registra anche il JSON canonico
//!
//! Un oracolo che confronta solo hash dice *che* qualcosa e' cambiato, non
//! *cosa*. Il JSON canonico e' esattamente il testo su cui l'hash e' calcolato:
//! averlo accanto rende il diff diagnostico invece che allarmante. Costa
//! qualche riga di snapshot e risparmia un'indagine.
//!
//! # Rigenerazione
//!
//! Dopo un cambiamento **intenzionale e versionato** della forma canonica:
//!
//! ```sh
//! PLENORA_UPDATE_SNAPSHOT=1 cargo test -p plenora-engine identita_dei_piani
//! ```
//!
//! e commit dell'oracolo aggiornato insieme alla modifica. Un cambiamento non
//! intenzionale fallisce qui, che e' il punto.

use std::sync::Arc;

use plenora_core::arrow::schema::{DataType, Field, Schema};
use plenora_core::contract::{
    ContractCrs, ContractProperties, DataContract, FieldId, GeometryColumnContract,
    GeometryDimensions,
};
use plenora_core::crs::{CrsKind, ResolvedCrs};
use plenora_engine::planner::validate;
use serde_json::{json, Value};

/// Percorso dell'oracolo committato.
const ORACOLO_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/oracoli_identita.snap");

// ---------------------------------------------------------------------------
// Contratti di input
// ---------------------------------------------------------------------------

/// Contratto tabellare fisso: cambiarlo cambia i fingerprint dei contratti e
/// quindi l'oracolo. E' voluto — il fingerprint di input fa parte
/// dell'identita' che stiamo sorvegliando.
fn contratto_tabellare() -> DataContract {
    DataContract::tabular(Arc::new(Schema::new(vec![
        Field::new("id", DataType::Int64, false),
        Field::new("gruppo", DataType::Utf8, true),
        Field::new("valore", DataType::Float64, true),
    ])))
}

/// Contratto geo con la colonna identificabile dal trasporto **e** un CRS
/// proiettato risolto.
///
/// Servono entrambe le cose: senza i metadati dell'estensione `GeoArrow`
/// l'analyzer non riconosce la colonna, e senza un CRS proiettato le op che
/// lo richiedono — `geo.centroid` fra queste — rifiutano il piano
/// fail-closed. La prima stesura di questo contratto dichiarava
/// `ContractCrs::Missing` e il piano veniva respinto, che e' il
/// comportamento corretto.
fn contratto_geo() -> DataContract {
    let schema = Arc::new(Schema::new(vec![
        Field::new("id", DataType::Int64, false),
        plenora_kernels_geo::arrow_adapter::geometry_output_field("geom", "EPSG:32632")
            .expect("campo geometria"),
    ]));
    DataContract::new(
        schema,
        vec![GeometryColumnContract {
            field_id: FieldId(0),
            name: "geom".to_owned(),
            crs: ContractCrs::Resolved(ResolvedCrs::from_resolved_parts(
                "EPSG:32632".to_owned(),
                json!({"type": "ProjectedCRS", "name": "WGS 84 / UTM zone 32N"}),
                CrsKind::Projected,
                Some(1.0),
            )),
            dimensions: GeometryDimensions::Xy,
            encoding: None,
            nullable: true,
            types: GeometryColumnContract::undeclared_types(),
        }],
        None,
        ContractProperties::default(),
    )
    .expect("contratto geo coerente con lo schema")
}

// ---------------------------------------------------------------------------
// I piani rappresentativi
// ---------------------------------------------------------------------------

/// Quali input dichiara un piano, e con quale contratto.
enum Ingressi {
    /// Un solo input tabellare chiamato `main`.
    Tabellare,
    /// Due input tabellari, per le operazioni binarie.
    DueTabellari,
    /// Un solo input geo chiamato `main`.
    Geo,
}

struct Piano {
    nome: &'static str,
    /// Che cosa questo piano copre, e perche' vale la pena sorvegliarlo.
    perche: &'static str,
    ingressi: Ingressi,
    json: Value,
}

/// L'insieme non e' esaustivo — 146 operazioni non ci starebbero — ma copre le
/// **classi di esecuzione** e i punti in cui la forma canonica e' fragile.
///
/// I tre gruppi rispondono a domande diverse: le classi di esecuzione, la
/// fragilita' della forma canonica, e la famiglia geo che ha analyzer e
/// contratto propri.
fn piani() -> Vec<Piano> {
    let mut tutti = classi_di_esecuzione();
    tutti.extend(forma_canonica_fragile());
    tutti.extend(famiglia_geo());
    tutti
}

/// Una per classe di esecuzione: streaming, blocking unaria, blocking con
/// schema di uscita diverso, binaria ordinata, e una catena di piu' nodi.
fn classi_di_esecuzione() -> Vec<Piano> {
    vec![
        Piano {
            nome: "table_filtro",
            perche: "streaming 1:1, il piano piu' semplice possibile",
            ingressi: Ingressi::Tabellare,
            json: json!({
                "schema_version": 5,
                "inputs": ["main"],
                "nodes": [
                    {"id": "f", "op": "table.filter", "in": ["main"],
                     "config": {"column": "valore", "operator": ">", "value": 10}}
                ],
                "output": "f"
            }),
        },
        Piano {
            nome: "table_ordinamento",
            perche: "blocking unaria: materializza prima di emettere",
            ingressi: Ingressi::Tabellare,
            json: json!({
                "schema_version": 5,
                "inputs": ["main"],
                "nodes": [
                    {"id": "o", "op": "table.sort", "in": ["main"],
                     "config": {"columns": ["valore"], "ascending": true}}
                ],
                "output": "o"
            }),
        },
        Piano {
            nome: "table_aggregazione",
            perche: "blocking con colonne nuove: lo schema di uscita non e' quello di ingresso",
            ingressi: Ingressi::Tabellare,
            json: json!({
                "schema_version": 5,
                "inputs": ["main"],
                "nodes": [
                    {"id": "a", "op": "table.aggregate", "in": ["main"],
                     "config": {"group_by": ["gruppo"],
                                "aggregations": [{"column": "valore", "function": "sum"},
                                                 {"column": "id", "function": "count"}]}}
                ],
                "output": "a"
            }),
        },
        Piano {
            nome: "table_join",
            perche: "binaria ordinata: due ingressi, e l'ordine fra loro conta",
            ingressi: Ingressi::DueTabellari,
            json: json!({
                "schema_version": 5,
                "inputs": ["sinistra", "destra"],
                "nodes": [
                    {"id": "j", "op": "table.join", "in": ["sinistra", "destra"],
                     "config": {"left_keys": ["id"], "right_keys": ["id"], "how": "inner"}}
                ],
                "output": "j"
            }),
        },
        Piano {
            nome: "table_catena",
            perche: "piu' nodi: l'ordine topologico entra nella forma canonica",
            ingressi: Ingressi::Tabellare,
            json: json!({
                "schema_version": 5,
                "inputs": ["main"],
                "nodes": [
                    {"id": "f", "op": "table.filter", "in": ["main"],
                     "config": {"column": "valore", "operator": ">=", "value": 0}},
                    {"id": "o", "op": "table.sort", "in": ["f"],
                     "config": {"columns": ["id"], "ascending": false}}
                ],
                "output": "o"
            }),
        },
    ]
}

/// I punti in cui la forma canonica e' fragile: i numeri grandi, i limiti
/// dichiarati, e la migrazione da v4 che non deve cambiare l'identita'.
fn forma_canonica_fragile() -> Vec<Piano> {
    vec![
        Piano {
            nome: "table_intero_oltre_2_53",
            perche: "gli interi oltre 2^53 non devono collassare nel canonico (regressione nota)",
            ingressi: Ingressi::Tabellare,
            json: json!({
                "schema_version": 5,
                "inputs": ["main"],
                "nodes": [
                    {"id": "f", "op": "table.filter", "in": ["main"],
                     "config": {"column": "id", "operator": ">",
                                "value": 9_007_199_254_740_993_u64}}
                ],
                "output": "f"
            }),
        },
        Piano {
            nome: "table_limiti_dichiarati",
            perche: "i limiti dichiarati restano nella forma canonica e quindi nell'identita'",
            ingressi: Ingressi::Tabellare,
            json: json!({
                "schema_version": 5,
                "inputs": ["main"],
                "limits": {"max_governed_memory_bytes": 4_194_304},
                "nodes": [
                    {"id": "f", "op": "table.filter", "in": ["main"],
                     "config": {"column": "valore", "operator": "<", "value": 100}}
                ],
                "output": "f"
            }),
        },
        Piano {
            nome: "migrazione_da_v4",
            perche: "un piano v4 migrato deve dare la STESSA identita' del suo equivalente v5",
            ingressi: Ingressi::Tabellare,
            json: json!({
                "schema_version": 4,
                "inputs": ["main"],
                "limits": {"max_memory_bytes": 4_194_304},
                "nodes": [
                    {"id": "f", "op": "table.filter", "in": ["main"],
                     "config": {"column": "valore", "operator": "<", "value": 100}}
                ],
                "output": "f"
            }),
        },
    ]
}

/// La famiglia geo: analyzer, contratto e requisito di CRS diversi da quelli
/// tabellari.
fn famiglia_geo() -> Vec<Piano> {
    vec![Piano {
        nome: "geo_centroide",
        perche: "famiglia geo: analyzer e contratto diversi da quelli tabellari",
        ingressi: Ingressi::Geo,
        json: json!({
            "schema_version": 5,
            "inputs": ["main"],
            "nodes": [
                {"id": "c", "op": "geo.centroid", "in": ["main"], "config": {}}
            ],
            "output": "c"
        }),
    }]
}

// ---------------------------------------------------------------------------
// Costruzione dell'oracolo
// ---------------------------------------------------------------------------

fn contratti(ingressi: &Ingressi) -> Vec<(String, DataContract)> {
    match ingressi {
        Ingressi::Tabellare => vec![("main".to_owned(), contratto_tabellare())],
        Ingressi::DueTabellari => vec![
            ("sinistra".to_owned(), contratto_tabellare()),
            ("destra".to_owned(), contratto_tabellare()),
        ],
        Ingressi::Geo => vec![("main".to_owned(), contratto_geo())],
    }
}

fn oracolo_content() -> String {
    let voci: Vec<Value> = piani()
        .iter()
        .map(|piano| {
            let grafo = validate(&piano.json.to_string(), &contratti(&piano.ingressi))
                .unwrap_or_else(|error| {
                    panic!("il piano `{}` deve essere valido: {error}", piano.nome)
                });
            json!({
                "nome": piano.nome,
                "perche": piano.perche,
                "plan_format_version": grafo.plan_format_version(),
                "plan_hash": grafo.plan_hash().to_hex(),
                "catalog_fingerprint": grafo.catalog_fingerprint().to_hex(),
                "input_contract_fingerprints": grafo
                    .input_contract_fingerprints()
                    .iter()
                    .map(plenora_engine::planner::ContractFingerprint::to_hex)
                    .collect::<Vec<_>>(),
                "canonical_json": grafo.plan().canonical_json(),
            })
        })
        .collect();
    let mut content = serde_json::to_string_pretty(&voci).expect("la serializzazione non fallisce");
    content.push('\n');
    content
}

// ---------------------------------------------------------------------------
// Il test
// ---------------------------------------------------------------------------

/// L'identita' dei piani rappresentativi deve coincidere con l'oracolo
/// committato. Vedi la doc di modulo per il perche' e per la rigenerazione.
#[test]
fn identita_dei_piani_coincide_con_l_oracolo_committato() {
    let attuale = oracolo_content();
    let path = std::path::Path::new(ORACOLO_PATH);
    if std::env::var_os("PLENORA_UPDATE_SNAPSHOT").is_some() {
        std::fs::write(path, &attuale).expect("rigenerazione dell'oracolo di identita'");
        eprintln!("oracolo di identita' rigenerato in {ORACOLO_PATH}");
        return;
    }
    let atteso = std::fs::read_to_string(path).unwrap_or_else(|error| {
        panic!("oracolo non leggibile ({error}): generarlo con PLENORA_UPDATE_SNAPSHOT=1")
    });
    // Confronto insensibile agli a-capo, come per lo snapshot del catalogo:
    // un checkout su Windows puo' produrre CRLF.
    let atteso = atteso.replace("\r\n", "\n");
    assert!(
        attuale == atteso,
        "l'identita' dei piani diverge dall'oracolo committato {ORACOLO_PATH}: \
         il `canonical_json` accanto a ogni hash dice quale campo e' cambiato. \
         Se il cambiamento e' INTENZIONALE e versionato, rigenerare con \
         `PLENORA_UPDATE_SNAPSHOT=1 cargo test -p plenora-engine identita_dei_piani` \
         e committare l'oracolo aggiornato insieme alla modifica"
    );
}

/// Un piano v4 e il suo equivalente v5 devono avere la STESSA identita': la
/// migrazione e' una riscrittura di forma, non di significato.
///
/// L'oracolo lo registra gia' come valore; questo test lo dice come
/// **relazione**, cosi' resta vero anche dopo una rigenerazione intenzionale.
/// Le due difese sono indipendenti di proposito: rigenerare l'oracolo non deve
/// poter far passare in silenzio una migrazione che cambia il piano.
#[test]
fn la_migrazione_da_v4_conserva_l_identita() {
    let tutti = piani();
    let v4 = tutti
        .iter()
        .find(|piano| piano.nome == "migrazione_da_v4")
        .expect("il piano v4 e' nell'insieme");
    let v5 = tutti
        .iter()
        .find(|piano| piano.nome == "table_limiti_dichiarati")
        .expect("l'equivalente v5 e' nell'insieme");

    let da_v4 = validate(&v4.json.to_string(), &contratti(&v4.ingressi)).expect("piano v4 valido");
    let da_v5 = validate(&v5.json.to_string(), &contratti(&v5.ingressi)).expect("piano v5 valido");

    assert_eq!(
        da_v4.plan_hash().to_hex(),
        da_v5.plan_hash().to_hex(),
        "un piano v4 migrato deve avere lo stesso plan_hash del suo equivalente v5"
    );
    assert_eq!(
        da_v4.plan_format_version(),
        da_v5.plan_format_version(),
        "dopo la migrazione la versione di formato e' quella canonica"
    );
}
