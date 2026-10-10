//! I punti di `plenora_pipeline::sonda` nel lavoro di `plenora-io`: fra due
//! passi del piano e fra la scrittura di due output, mai altrove, e
//! nell'ordine del lavoro. La sonda è del processo: questo binario di prova
//! ha **un solo test**, così nessun'altra prova la vede.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::{Arc, Mutex};

use plenora_core::arrow::array::{Int64Array, RecordBatch};
use plenora_core::arrow::schema::{DataType, Field, Schema};
use plenora_core::PlenoraError;
use plenora_io::{esegui_ingressi, FileUscita, Ingresso, OpzioniScrittura};
use plenora_pipeline::sonda::{self, Azione, Punto};
use plenora_pipeline::{Interruzione, Pipeline};

fn piano() -> Pipeline {
    Pipeline::from_json(
        r#"{"version": 1, "inputs": ["t"],
          "steps": [
            {"out": "a", "op": "table.sort", "in": ["t"], "config": {"columns": ["id"]}},
            {"out": "b", "op": "table.sort", "in": ["a"],
             "config": {"columns": ["id"], "ascending": false}},
            {"out": "c", "op": "table.limit", "in": ["b"], "config": {"n": 2}}],
          "outputs": ["a", "c"]}"#,
    )
    .unwrap()
}

fn esegui(cartella: &std::path::Path) -> plenora_core::Result<()> {
    let tabella = RecordBatch::try_new(
        Arc::new(Schema::new(vec![Field::new("id", DataType::Int64, false)])),
        vec![Arc::new(Int64Array::from(vec![3, 1, 2]))],
    )
    .unwrap();
    let uscite: Vec<FileUscita> = ["a", "c"]
        .iter()
        .map(|nome| FileUscita {
            nome: (*nome).to_owned(),
            percorso: cartella.join(format!("{nome}.arrow")),
            formato: None,
        })
        .collect();
    esegui_ingressi(
        &piano(),
        vec![Ingresso::Tabella {
            nome: "t".to_owned(),
            tabella,
        }],
        &uscite,
        &OpzioniScrittura::default(),
        &Interruzione::default(),
    )
    .map(|_| ())
}

#[test]
fn la_sonda_passa_fra_i_passi_e_fra_le_scritture() {
    let punti = Arc::new(Mutex::new(Vec::new()));
    let visti = Arc::clone(&punti);
    sonda::registra(Some(Arc::new(move |punto, _: &Interruzione| {
        visti.lock().unwrap().push(punto);
        Ok(Azione::Prosegui)
    })));
    let cartella = tempfile::tempdir().unwrap();
    esegui(cartella.path()).unwrap();
    assert_eq!(
        *punti.lock().unwrap(),
        [Punto::FraPassi, Punto::FraPassi, Punto::FraScritture]
    );

    // Anticipare la scadenza fuori da `Prima` non si fa: `internal`.
    sonda::registra(Some(Arc::new(|_, _: &Interruzione| {
        Ok(Azione::AnticipaScadenza)
    })));
    let cartella = tempfile::tempdir().unwrap();
    assert!(matches!(
        esegui(cartella.path()).unwrap_err().untag(),
        PlenoraError::Internal(_)
    ));

    // Tolta, nessun punto la chiama più.
    punti.lock().unwrap().clear();
    sonda::registra(None);
    let cartella = tempfile::tempdir().unwrap();
    esegui(cartella.path()).unwrap();
    assert!(punti.lock().unwrap().is_empty());

    // `Prima` anticipa una scadenza che c'è, e solo quella.
    sonda::registra(Some(Arc::new(|_, _: &Interruzione| {
        Ok(Azione::AnticipaScadenza)
    })));
    let lontana = std::time::Instant::now() + std::time::Duration::from_secs(3600);
    let mut con_scadenza = Interruzione {
        scadenza: Some(lontana),
        annullamento: None,
    };
    sonda::chiama_prima(&mut con_scadenza).unwrap();
    assert!(con_scadenza.scadenza.unwrap() < lontana);
    assert!(con_scadenza.verifica("").is_err());
    let mut senza = Interruzione::default();
    sonda::chiama_prima(&mut senza).unwrap();
    assert!(senza.scadenza.is_none());
    sonda::registra(None);
}
