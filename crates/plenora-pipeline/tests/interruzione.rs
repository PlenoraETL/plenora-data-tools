//! Scadenza e annullamento cooperativi di `run_interrompibile`: controllati
//! prima di ogni passo e prima di consegnare gli output, con gli assi e il
//! codice di `plenora-error-v1`.

use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::{Duration, Instant};

use plenora_core::arrow::array::{Int64Array, RecordBatch};
use plenora_core::arrow::schema::{DataType, Field, Schema};
use plenora_core::error::{CODE_CANCELLED, CODE_DEADLINE_EXCEEDED};
use plenora_core::{
    ErrorCategory, ErrorPhase, PlenoraError, RemoteEffect, Result, RetryDisposition,
};
use plenora_pipeline::{Esito, Interruzione, Passo, Pipeline};
use serde_json::json;

fn tabella() -> RecordBatch {
    RecordBatch::try_new(
        Arc::new(Schema::new(vec![Field::new("k", DataType::Int64, false)])),
        vec![Arc::new(Int64Array::from(vec![3, 1, 2]))],
    )
    .expect("tabella")
}

/// Un passo (`sort`), o nessuno: l'output e' allora l'input stesso.
fn piano(con_passo: bool) -> Pipeline {
    Pipeline {
        version: 1,
        inputs: vec!["a".into()],
        crs: None,
        limits: None,
        steps: if con_passo {
            vec![Passo {
                out: "x".into(),
                op: "table.sort".into(),
                inputs: vec!["a".into()],
                config: json!({"columns": ["k"]}),
            }]
        } else {
            Vec::new()
        },
        outputs: vec![if con_passo { "x" } else { "a" }.into()],
    }
}

fn esegui(con_passo: bool, interruzione: &Interruzione) -> Result<Esito> {
    let a = tabella();
    piano(con_passo)
        .validate(&[("a", a.schema())])?
        .run_interrompibile(vec![("a".into(), a)], interruzione)
}

fn scaduta() -> Interruzione {
    Interruzione {
        scadenza: Some(Instant::now()),
        annullamento: None,
    }
}

fn annullata() -> Interruzione {
    Interruzione {
        scadenza: None,
        annullamento: Some(Arc::new(AtomicBool::new(true))),
    }
}

#[test]
fn senza_interruzione_l_esito_e_quello_di_run() {
    let a = tabella();
    let atteso = piano(true)
        .validate(&[("a", a.schema())])
        .and_then(|validata| validata.run(vec![("a".into(), a)]))
        .expect("run");
    let lontana = Interruzione {
        scadenza: Some(Instant::now() + Duration::from_secs(3_600)),
        annullamento: Some(Arc::new(AtomicBool::new(false))),
    };
    for interruzione in [Interruzione::default(), lontana] {
        let esito = esegui(true, &interruzione).expect("esecuzione");
        assert_eq!(esito.outputs, atteso.outputs);
        assert_eq!(esito.report, atteso.report);
    }
}

#[test]
fn la_scadenza_passata_ferma_prima_del_passo_con_gli_assi_del_contratto() {
    let errore = esegui(true, &scaduta()).expect_err("scaduta");
    assert!(matches!(errore, PlenoraError::Timeout(_)), "{errore}");
    assert_eq!(
        errore.to_string(),
        "timeout: scadenza dell'esecuzione superata prima del passo `x` (table.sort)"
    );
    let pubblico = errore.public_projection();
    assert_eq!(pubblico.phase, ErrorPhase::Write);
    assert_eq!(pubblico.category, ErrorCategory::Timeout);
    assert_eq!(pubblico.code.as_deref(), Some(CODE_DEADLINE_EXCEEDED));
    assert_eq!(pubblico.remote_effect, RemoteEffect::None);
    assert_eq!(pubblico.retry, RetryDisposition::Safe);
}

#[test]
fn l_annullamento_ferma_prima_del_passo() {
    let errore = esegui(true, &annullata()).expect_err("annullata");
    assert_eq!(
        errore.to_string(),
        "cancelled: esecuzione annullata prima del passo `x` (table.sort)"
    );
    let pubblico = errore.public_projection();
    assert_eq!(pubblico.category, ErrorCategory::Cancelled);
    assert_eq!(pubblico.code.as_deref(), Some(CODE_CANCELLED));
    assert_eq!(pubblico.remote_effect, RemoteEffect::None);
}

#[test]
fn l_annullamento_prevale_sulla_scadenza() {
    let entrambe = Interruzione {
        scadenza: Some(Instant::now()),
        annullamento: Some(Arc::new(AtomicBool::new(true))),
    };
    let errore = esegui(true, &entrambe).expect_err("interrotta");
    assert_eq!(errore.category(), ErrorCategory::Cancelled, "{errore}");
}

#[test]
fn senza_passi_il_controllo_e_prima_della_consegna_degli_output() {
    for (interruzione, categoria) in [
        (scaduta(), ErrorCategory::Timeout),
        (annullata(), ErrorCategory::Cancelled),
    ] {
        let errore = esegui(false, &interruzione).expect_err("interrotta");
        assert_eq!(errore.category(), categoria, "{errore}");
        assert_eq!(errore.phase(), ErrorPhase::Finalize, "{errore}");
        assert!(
            errore
                .to_string()
                .ends_with("prima di consegnare gli output"),
            "{errore}"
        );
    }
}
