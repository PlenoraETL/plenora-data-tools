//! Vettori di conformità del contratto Arrow (Arrow Metadata Vocabulary
//! 1.0, `vectors/arrow-v1` di `plenora-contracts`), attraverso il confine
//! pubblico del componente: schema Arrow dal vettore, file Arrow IPC,
//! `esegui_da_file` con un piano identità, file d'uscita riletto.
//!
//! I vettori sono copie byte per byte del commit dei contratti registrato in
//! `tests/fixtures/contratti/arrow-v1/provenienza.json`, con il loro
//! SHA-256: il primo test verifica che nessuno li abbia toccati.
//!
//! I vettori non dicono la categoria dell'errore atteso; il vocabolario
//! (sezione 4) vuole `schema` o `crs` per i metadati incoerenti, e qui ogni
//! vettore invalido ha la sua, scritta accanto.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::{BTreeMap, HashMap};
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use plenora_core::arrow::array::{
    Array, ArrayRef, BinaryArray, Int64Array, LargeBinaryArray, RecordBatch,
};
use plenora_core::arrow::schema::{DataType, Field, Schema};
use plenora_core::contract::arrow_schema::contract_from_arrow_schema;
use plenora_core::crs::resolve_crs;
use plenora_core::PlenoraError;
use plenora_io::{
    esegui_da_file, leggi_tabella, scrivi_tabella, FileIngresso, FileUscita, OpzioniScrittura,
};
use plenora_pipeline::Pipeline;
use serde_json::Value;
use sha2::{Digest, Sha256};

/// Chiavi che il vocabolario vuole su ogni campo `geoarrow.wkb`
/// (sezione 4).
const CHIAVI_GEOMETRIA: &[&str] = &[
    "plenora.field_id",
    "ARROW:extension:name",
    "plenora.geometry.encoding",
    "plenora.geometry.dimensions",
    "plenora.geometry.spatial_semantics",
    "plenora.geometry.precision",
    "plenora.geometry.types_declaration",
    "plenora.geometry.crs_resolution",
];

fn cartella_vettori() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("contratti")
        .join("arrow-v1")
}

fn vettore(nome: &str) -> Value {
    let testo = std::fs::read_to_string(cartella_vettori().join(nome)).expect("vettore");
    serde_json::from_str(&testo).expect("JSON del vettore")
}

fn mappa(valore: &Value) -> HashMap<String, String> {
    valore
        .as_object()
        .expect("oggetto di metadati")
        .iter()
        .map(|(chiave, valore)| {
            (
                chiave.clone(),
                valore.as_str().expect("metadato testuale").to_owned(),
            )
        })
        .collect()
}

/// `POINT(12.5 41.9)` in WKB ISO little-endian: valido anche come EWKB
/// (senza SRID incorporato), l'encoding del vettore `resolved-point`.
fn punto_wkb() -> Vec<u8> {
    let mut wkb = vec![1, 1, 0, 0, 0];
    wkb.extend_from_slice(&12.5_f64.to_le_bytes());
    wkb.extend_from_slice(&41.9_f64.to_le_bytes());
    wkb
}

/// Lo schema Arrow nativo del vettore e una tabella di una riga.
fn tabella_del_vettore(vettore: &Value) -> RecordBatch {
    let mut campi = Vec::new();
    let mut array: Vec<ArrayRef> = Vec::new();
    for campo in vettore["fields"].as_array().expect("fields") {
        let nome = campo["name"].as_str().expect("name");
        let nullable = campo["nullable"].as_bool().expect("nullable");
        let (tipo, colonna): (DataType, ArrayRef) = match campo["type"].as_str().expect("type") {
            "int64" => (DataType::Int64, Arc::new(Int64Array::from(vec![7_i64]))),
            "binary" => (
                DataType::Binary,
                Arc::new(BinaryArray::from(vec![Some(punto_wkb().as_slice())])),
            ),
            "large_binary" => (
                DataType::LargeBinary,
                Arc::new(LargeBinaryArray::from(vec![Some(punto_wkb().as_slice())])),
            ),
            altro => panic!("tipo del vettore non mappato nel test: {altro}"),
        };
        campi.push(Field::new(nome, tipo, nullable).with_metadata(mappa(&campo["metadata"])));
        array.push(colonna);
    }
    let schema = Schema::new_with_metadata(campi, mappa(&vettore["schema_metadata"]));
    RecordBatch::try_new(Arc::new(schema), array).expect("tabella del vettore")
}

fn piano(json: &str) -> Pipeline {
    Pipeline::from_json(json).expect("piano")
}

/// Piano identità: l'input è l'output, nessun passo.
fn identita() -> Pipeline {
    piano(r#"{"version": 1, "inputs": ["t"], "steps": [], "outputs": ["t"]}"#)
}

fn esegui(pipeline: &Pipeline, ingresso: &Path, uscita: &Path) -> Result<(), PlenoraError> {
    esegui_da_file(
        pipeline,
        &[FileIngresso {
            nome: "t".to_owned(),
            percorso: ingresso.to_path_buf(),
            formato: None,
        }],
        &[FileUscita {
            nome: pipeline.outputs[0].clone(),
            percorso: uscita.to_path_buf(),
            formato: None,
        }],
        &OpzioniScrittura::default(),
    )
    .map(|_| ())
}

/// Scrive il vettore in Arrow IPC senza le verifiche di `scrivi_tabella`
/// (un produttore qualunque), per provare la lettura.
fn scrivi_grezzo(tabella: &RecordBatch, percorso: &Path) {
    let file = File::create(percorso).expect("file");
    plenora_io::ipc::scrivi(tabella, file).expect("scrittura IPC grezza");
}

#[test]
fn i_vettori_sono_le_copie_registrate() {
    let provenienza = vettore("provenienza.json");
    assert_eq!(
        provenienza["commit"],
        "3c395a8db96df739024e203b22794af340dc8a7f"
    );
    let registrati: BTreeMap<String, String> = provenienza["file"]
        .as_object()
        .expect("file")
        .iter()
        .map(|(nome, hash)| (nome.clone(), hash.as_str().expect("hash").to_owned()))
        .collect();
    let mut presenti = BTreeMap::new();
    for voce in std::fs::read_dir(cartella_vettori()).expect("cartella") {
        let percorso = voce.expect("voce").path();
        let nome = percorso.file_name().unwrap().to_str().unwrap().to_owned();
        if nome == "provenienza.json" {
            continue;
        }
        let byte = std::fs::read(&percorso).expect("byte");
        let hash = Sha256::digest(&byte)
            .iter()
            .fold(String::new(), |mut testo, b| {
                use std::fmt::Write as _;
                let _ = write!(testo, "{b:02x}");
                testo
            });
        presenti.insert(nome, hash);
    }
    assert_eq!(presenti, registrati);
}

/// Un vettore valido attraversa il confine: si scrive, il piano identità lo
/// legge e lo riscrive, e l'uscita porta lo stesso contratto.
fn valido_attraversa_il_confine(nome: &str) -> RecordBatch {
    let vettore = vettore(nome);
    assert_eq!(vettore["expect"], "valid");
    let tabella = tabella_del_vettore(&vettore);
    let dir = tempfile::tempdir().expect("cartella");
    let ingresso = dir.path().join("in.arrow");
    let uscita = dir.path().join("out.arrow");
    scrivi_tabella(&tabella, &ingresso, &OpzioniScrittura::default())
        .unwrap_or_else(|errore| panic!("{nome}: scrittura rifiutata: {errore}"));
    esegui(&identita(), &ingresso, &uscita)
        .unwrap_or_else(|errore| panic!("{nome}: rifiutato: {errore}"));
    let letta = leggi_tabella(&uscita, None, u64::MAX).expect("uscita");
    let schema = letta.schema();

    assert_eq!(
        schema
            .metadata()
            .get("plenora.contract.version")
            .map(String::as_str),
        Some("1"),
        "{nome}: versione del contratto"
    );
    for (campo_atteso, campo) in vettore["fields"]
        .as_array()
        .unwrap()
        .iter()
        .zip(schema.fields())
    {
        assert_eq!(campo.name(), campo_atteso["name"].as_str().unwrap());
        let attesi = mappa(&campo_atteso["metadata"]);
        let geometria = attesi.contains_key("ARROW:extension:name");
        // Ogni chiave del vettore resta byte per byte, identità e
        // precisione comprese: il piano identità è un passaggio senza
        // perdite (ARROW-008, ARROW-010).
        for (chiave, valore) in &attesi {
            let ottenuto = campo.metadata().get(chiave).map(String::as_str);
            assert_eq!(ottenuto, Some(valore.as_str()), "{nome}: chiave {chiave}");
        }
        if geometria {
            for chiave in CHIAVI_GEOMETRIA {
                assert!(
                    campo.metadata().contains_key(chiave),
                    "{nome}: chiave {chiave} assente dall'uscita"
                );
            }
            // `binary` o `large_binary` in ingresso, `binary` in uscita:
            // entrambe ammesse dal vocabolario.
            assert_eq!(campo.data_type(), &DataType::Binary, "{nome}: storage");
        }
    }
    // L'uscita è a sua volta un ingresso valido.
    contract_from_arrow_schema(schema, resolve_crs)
        .unwrap_or_else(|errore| panic!("{nome}: uscita non rileggibile: {errore}"));
    letta
}

#[test]
fn resolved_point_e_valido() {
    let letta = valido_attraversa_il_confine("resolved-point.json");
    let geometria = letta
        .column(1)
        .as_any()
        .downcast_ref::<BinaryArray>()
        .expect("geometria");
    assert_eq!(geometria.value(0), punto_wkb().as_slice());
}

#[test]
fn missing_crs_con_large_binary_e_valido() {
    let letta = valido_attraversa_il_confine("missing-crs.json");
    let geometria = letta
        .column(0)
        .as_any()
        .downcast_ref::<BinaryArray>()
        .expect("geometria convertita in Binary");
    assert_eq!(geometria.len(), 1);
    assert_eq!(geometria.value(0), punto_wkb().as_slice());
}

/// Un vettore invalido si rifiuta alla lettura, con la categoria
/// prescritta, e non produce un file d'uscita.
fn invalido_rifiutato(nome: &str, categoria: &str) {
    let vettore = vettore(nome);
    assert_eq!(vettore["expect"], "invalid");
    let tabella = tabella_del_vettore(&vettore);
    let dir = tempfile::tempdir().expect("cartella");
    let ingresso = dir.path().join("in.arrow");
    let uscita = dir.path().join("out.arrow");
    scrivi_grezzo(&tabella, &ingresso);
    let errore =
        esegui(&identita(), &ingresso, &uscita).expect_err(&format!("{nome}: atteso un rifiuto"));
    assert_eq!(
        errore.category().as_str(),
        categoria,
        "{nome}: categoria, errore {errore}"
    );
    assert!(!uscita.exists(), "{nome}: nessuna uscita");
}

/// `crs_resolution = missing` con un `crs_id`: lo stato del CRS si
/// contraddice, categoria `crs`.
#[test]
fn invalid_missing_crs_with_id_e_rifiutato() {
    invalido_rifiutato("invalid-missing-crs-with-id.json", "crs");
}

/// Chiavi `plenora.*` senza `plenora.contract.version` (ARROW-001):
/// categoria `schema`. Anche la scrittura lo rifiuta: il componente non
/// produce uno schema Plenora senza versione.
#[test]
fn invalid_unversioned_geometry_e_rifiutato() {
    invalido_rifiutato("invalid-unversioned-geometry.json", "schema");
    let tabella = tabella_del_vettore(&vettore("invalid-unversioned-geometry.json"));
    let dir = tempfile::tempdir().expect("cartella");
    let errore = scrivi_tabella(
        &tabella,
        &dir.path().join("x.arrow"),
        &OpzioniScrittura::default(),
    )
    .expect_err("scrittura senza versione");
    assert_eq!(errore.category().as_str(), "schema");
}

/// Il vettore `resolved-point` dichiara `lat_lon` per `EPSG:4326`: il piano
/// identità lo attraversa intatto (sopra), un'operazione geo lo rifiuta
/// perché i kernel leggono x come longitudine (docs/metadati-arrow.md, «Metadati Arrow»).
#[test]
fn assi_scambiati_rifiutati_dalle_operazioni_geo() {
    let tabella = tabella_del_vettore(&vettore("resolved-point.json"));
    let dir = tempfile::tempdir().expect("cartella");
    let ingresso = dir.path().join("in.arrow");
    scrivi_tabella(&tabella, &ingresso, &OpzioniScrittura::default()).expect("scrittura");
    let geo = piano(
        r#"{"version": 1, "inputs": ["t"],
            "steps": [{"out": "c", "op": "geo.centroid", "in": ["t"], "config": {}}],
            "outputs": ["c"]}"#,
    );
    let errore = esegui(&geo, &ingresso, &dir.path().join("out.arrow"))
        .expect_err("assi scambiati in un'operazione geo");
    assert_eq!(errore.category().as_str(), "crs", "{errore}");
}

/// La precisione dichiarata attraversa intatta un'operazione tabellare;
/// dopo un'operazione geo esce `float64`, anche da una che restituisce la
/// geometria com'era come `geo.vertex_count` (limite dichiarato, docs/metadati-arrow.md,
/// «Metadati Arrow»).
#[test]
fn precisione_conservata_dalle_tabellari_normalizzata_dalle_geo() {
    let tabella = tabella_del_vettore(&vettore("resolved-point.json"));
    let schema = tabella.schema();
    let campi: Vec<Field> = schema
        .fields()
        .iter()
        .map(|campo| {
            let mut metadati = campo.metadata().clone();
            if metadati.contains_key("ARROW:extension:name") {
                metadati.insert(
                    "plenora.geometry.axis_order".to_owned(),
                    "lon_lat".to_owned(),
                );
                metadati.insert(
                    "plenora.geometry.precision".to_owned(),
                    "float32".to_owned(),
                );
            }
            campo.as_ref().clone().with_metadata(metadati)
        })
        .collect();
    let tabella = RecordBatch::try_new(
        Arc::new(Schema::new_with_metadata(campi, schema.metadata().clone())),
        tabella.columns().to_vec(),
    )
    .expect("schema");
    let dir = tempfile::tempdir().expect("cartella");
    let ingresso = dir.path().join("in.arrow");
    scrivi_tabella(&tabella, &ingresso, &OpzioniScrittura::default()).expect("scrittura");
    let precisione = |piano_json: &str, uscita: &str| {
        let percorso = dir.path().join(uscita);
        esegui(&piano(piano_json), &ingresso, &percorso).expect("piano");
        let letta = leggi_tabella(&percorso, None, u64::MAX).expect("uscita");
        let schema = letta.schema();
        schema
            .field_with_name("geometry")
            .expect("geometria")
            .metadata()
            .get("plenora.geometry.precision")
            .cloned()
    };
    assert_eq!(
        precisione(
            r#"{"version": 1, "inputs": ["t"],
                "steps": [{"out": "f", "op": "table.filter", "in": ["t"],
                           "config": {"column": "id", "operator": ">", "value": 0}}],
                "outputs": ["f"]}"#,
            "tabellare.arrow"
        )
        .as_deref(),
        Some("float32")
    );
    assert_eq!(
        precisione(
            r#"{"version": 1, "inputs": ["t"],
                "steps": [{"out": "c", "op": "geo.vertex_count", "in": ["t"], "config": {}}],
                "outputs": ["c"]}"#,
            "geo.arrow"
        )
        .as_deref(),
        Some("float64")
    );
}

/// `resolved-point` con un CRS proiettato (le operazioni geo che producono
/// geometrie lo vogliono) e, se dato, un'altra cella geometria.
fn punto_ewkb_proiettato(cella: Option<&[u8]>) -> RecordBatch {
    let tabella = tabella_del_vettore(&vettore("resolved-point.json"));
    let schema = tabella.schema();
    let campi: Vec<Field> = schema
        .fields()
        .iter()
        .map(|campo| {
            let mut metadati = campo.metadata().clone();
            if metadati.contains_key("ARROW:extension:name") {
                for (chiave, valore) in [
                    ("plenora.geometry.axis_order", "easting_northing"),
                    ("plenora.geometry.srid", "32632"),
                    ("plenora.geometry.crs_id", "EPSG:32632"),
                ] {
                    metadati.insert(chiave.to_owned(), valore.to_owned());
                }
            }
            campo.as_ref().clone().with_metadata(metadati)
        })
        .collect();
    let mut colonne = tabella.columns().to_vec();
    if let Some(cella) = cella {
        colonne[1] = Arc::new(BinaryArray::from(vec![Some(cella)]));
    }
    RecordBatch::try_new(
        Arc::new(Schema::new_with_metadata(campi, schema.metadata().clone())),
        colonne,
    )
    .expect("schema")
}

/// Esegue un passo `o` su `t` e rilegge encoding e prima cella geometria.
fn encoding_e_cella(
    tabella: &RecordBatch,
    passo: &str,
) -> Result<(Option<String>, Vec<u8>), PlenoraError> {
    let dir = tempfile::tempdir().expect("cartella");
    let ingresso = dir.path().join("in.arrow");
    scrivi_tabella(tabella, &ingresso, &OpzioniScrittura::default()).expect("scrittura");
    let uscita = dir.path().join("out.arrow");
    let piano_json =
        format!(r#"{{"version": 1, "inputs": ["t"], "steps": [{passo}], "outputs": ["o"]}}"#);
    esegui(&piano(&piano_json), &ingresso, &uscita)?;
    let letta = leggi_tabella(&uscita, None, u64::MAX).expect("uscita");
    let encoding = letta
        .schema()
        .field_with_name("geometry")
        .expect("geometria")
        .metadata()
        .get("plenora.geometry.encoding")
        .cloned();
    let colonna = letta.column_by_name("geometry").expect("colonna");
    let celle = colonna
        .as_any()
        .downcast_ref::<BinaryArray>()
        .expect("binary");
    Ok((encoding, celle.value(0).to_vec()))
}

const FILTRO: &str = r#"{"out": "o", "op": "table.filter", "in": ["t"],
    "config": {"column": "id", "operator": ">", "value": 0}}"#;
const CENTROIDE: &str = r#"{"out": "o", "op": "geo.centroid", "in": ["t"], "config": {}}"#;
const BUFFER: &str =
    r#"{"out": "o", "op": "geo.buffer", "in": ["t"], "config": {"distance": 1.0}}"#;

/// La colonna geometrica d'ingresso `ewkb` esce `ewkb`, dopo
/// un'operazione tabellare come dopo una geo che la riscrive al suo posto
/// (docs/metadati-arrow.md, «In uscita»). I byte di un'operazione geo sono
/// WKB ISO senza SRID incorporato, che è anche EWKB valido: la
/// dichiarazione resta vera. Una colonna geometrica nuova
/// (`geo.coverage_validate`, su un `geo.buffer` che resta `ewkb`) non
/// eredita l'encoding ed esce `wkb`.
#[test]
fn un_ingresso_ewkb_esce_ewkb() {
    let tabella = punto_ewkb_proiettato(None);
    let (encoding, cella) = encoding_e_cella(&tabella, FILTRO).expect("filtro");
    assert_eq!(encoding.as_deref(), Some("ewkb"));
    assert_eq!(cella, punto_wkb(), "una tabellare copia i byte");
    for passo in [CENTROIDE, BUFFER] {
        let (encoding, cella) = encoding_e_cella(&tabella, passo).expect(passo);
        assert_eq!(encoding.as_deref(), Some("ewkb"), "{passo}");
        // Little-endian, tipo senza il bit SRID di EWKB (0x20000000).
        assert_eq!(cella[0], 1, "{passo}");
        assert_eq!(cella[4] & 0x20, 0, "{passo}: niente SRID incorporato");
    }
    let dir = tempfile::tempdir().expect("cartella");
    let ingresso = dir.path().join("in.arrow");
    scrivi_tabella(&tabella, &ingresso, &OpzioniScrittura::default()).expect("scrittura");
    let uscita = dir.path().join("out.arrow");
    let copertura = piano(
        r#"{"version": 1, "inputs": ["t"],
            "steps": [{"out": "b", "op": "geo.buffer", "in": ["t"], "config": {"distance": 1.0}},
                      {"out": "o", "op": "geo.coverage_validate", "in": ["b"], "config": {}}],
            "outputs": ["o"]}"#,
    );
    esegui(&copertura, &ingresso, &uscita).expect("copertura");
    let letta = leggi_tabella(&uscita, None, u64::MAX).expect("uscita");
    let schema = letta.schema();
    let nuove: Vec<_> = schema
        .fields()
        .iter()
        .filter(|campo| campo.metadata().contains_key("ARROW:extension:name"))
        .map(|campo| campo.metadata().get("plenora.geometry.encoding").cloned())
        .collect();
    assert_eq!(nuove, vec![Some("wkb".to_owned())]);
}

/// Un EWKB con SRID incorporato attraversa intatto un'operazione tabellare
/// (le celle non si leggono) e un'operazione geo lo rifiuta con un errore
/// esplicito: non si scarta lo SRID in silenzio.
#[test]
fn un_ewkb_con_srid_incorporato_passa_le_tabellari_e_si_rifiuta_nelle_geo() {
    let mut ewkb = vec![1_u8, 1, 0, 0, 0x20];
    ewkb.extend_from_slice(&32632_u32.to_le_bytes());
    ewkb.extend_from_slice(&12.5_f64.to_le_bytes());
    ewkb.extend_from_slice(&41.9_f64.to_le_bytes());
    let tabella = punto_ewkb_proiettato(Some(&ewkb));
    let (encoding, cella) = encoding_e_cella(&tabella, FILTRO).expect("filtro");
    assert_eq!(encoding.as_deref(), Some("ewkb"));
    assert_eq!(cella, ewkb);
    for passo in [CENTROIDE, BUFFER] {
        let errore = encoding_e_cella(&tabella, passo).expect_err(passo);
        assert_eq!(
            errore.category().as_str(),
            "unsupported",
            "{passo}: {errore}"
        );
    }
}
