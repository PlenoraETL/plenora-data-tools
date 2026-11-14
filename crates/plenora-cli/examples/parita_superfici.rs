//! Verifica Rust/CLI/runtime su tutto il corpus delle schede ed esporta l'oracolo SDK.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use plenora_cli::{
    api::{Destinazione, PubblicazioneFallita, RifiutoDestinazioni, RisolutoreArtefatti},
    runtime_binding::{RuntimeBinding, RuntimeInvocation},
};
use plenora_io::{leggi_tabella, Ingresso};
use plenora_pipeline::{Interruzione, Pipeline};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::{Read, Write},
    path::{Path, PathBuf},
};
struct Host {
    sources: BTreeMap<String, PathBuf>,
    output: PathBuf,
}
impl RisolutoreArtefatti for Host {
    fn leggi(&self, reference: &str, out: &mut dyn Write) -> std::io::Result<()> {
        std::io::copy(&mut std::fs::File::open(&self.sources[reference])?, out)?;
        Ok(())
    }
    fn prepara(&self, _: &[Destinazione<'_>]) -> Result<(), RifiutoDestinazioni> {
        Ok(())
    }
    fn pubblica(
        &self,
        _: &Destinazione<'_>,
        content: &mut dyn Read,
        _: u64,
    ) -> Result<(), PubblicazioneFallita> {
        let mut file =
            std::fs::File::create(&self.output).map_err(PubblicazioneFallita::NienteScritto)?;
        std::io::copy(content, &mut file).map_err(PubblicazioneFallita::Parziale)?;
        Ok(())
    }
}
fn invocation(payload: &Value) -> RuntimeInvocation {
    RuntimeInvocation {
        content_type: "application/json".into(),
        payload: payload.to_string().into_bytes(),
        metadata: BTreeMap::from([
            (
                "plenora.message.id".into(),
                json!("018f3d84-7b2c-7f00-8000-000000000130"),
            ),
            (
                "plenora.trace.correlation_id".into(),
                json!("018f3d84-7b2c-7f00-8000-000000000030"),
            ),
            (
                "plenora.capability.name".into(),
                json!("plenora.data-tools"),
            ),
            ("plenora.capability.version".into(), json!("1")),
            ("plenora.capability.operation".into(), json!("data.run")),
            ("plenora.operation.version".into(), json!("3")),
            (
                "plenora.input.contract".into(),
                json!("plenora-data-execution-input-v3"),
            ),
        ]),
    }
}
// Orchestrazione sequenziale delle tre superfici e dello stesso oracolo.
#[allow(clippy::too_many_lines)]
fn main() {
    std::env::set_var("PLENORA_ESEMPIO_CHIAVE_HMAC", "chiave-di-esempio");
    let args: Vec<_> = std::env::args_os().collect();
    let root = Path::new(&args[1]);
    let cli = Path::new(&args[2]);
    let cases: Vec<Value> =
        serde_json::from_slice(&std::fs::read(root.join("cases.json")).unwrap()).unwrap();
    let listed: BTreeSet<_> = cases
        .iter()
        .map(|c| c["operation"].as_str().unwrap())
        .collect();
    let catalog: BTreeSet<_> = plenora_core::catalog::CATALOG
        .iter()
        .map(|c| c.id)
        .collect();
    assert_eq!(listed, catalog, "nessun kernel escluso");
    for case in cases {
        let op = case["operation"].as_str().unwrap();
        let folder = root.join(op);
        let plan = Pipeline::from_json(&case["plan"].to_string()).unwrap();
        let mut sources = BTreeMap::new();
        let mut inputs = serde_json::Map::new();
        let mut tables = Vec::new();
        let plan_path = folder.join("plan.json");
        std::fs::write(&plan_path, case["plan"].to_string()).unwrap();
        let mut command = std::process::Command::new(cli);
        command
            .arg("run")
            .arg("--plan")
            .arg(&plan_path)
            .arg("--output")
            .arg(folder.join("cli.arrow"));
        for (name, file) in case["inputs"].as_object().unwrap() {
            let path = folder.join(file.as_str().unwrap());
            let reference = format!("artifact://input/{name}");
            sources.insert(reference.clone(), path.clone());
            inputs.insert(
                name.clone(),
                json!({"reference":reference,"content_type":"application/vnd.apache.arrow.file"}),
            );
            command
                .arg("--input")
                .arg(format!("{name}={}", path.display()));
            tables.push(Ingresso::Tabella {
                nome: name.clone(),
                tabella: leggi_tabella(&path, None, u64::MAX).unwrap(),
            });
        }
        let host = Host {
            sources,
            output: folder.join("runtime.arrow"),
        };
        let output = case["plan"]["outputs"][0].as_str().unwrap();
        let request = invocation(
            &json!({"schema_version":1,"plan":case["plan"],"inputs":inputs,
            "outputs":{output:{"reference":"artifact://output/result","content_type":"application/vnd.apache.arrow.file","overwrite":false}}}),
        );
        let runtime = RuntimeBinding { resolver: &host }.invoke(&request, &Interruzione::default());
        let runtime_doc: Value = serde_json::from_slice(&runtime.payload).unwrap();
        let cli = command.output().unwrap();
        assert!(cli.stderr.is_empty(), "{op}: stderr");
        let cli_doc: Value = serde_json::from_slice(&cli.stdout).unwrap();
        let rust = plenora_cli::api::esegui_in_memoria(&plan, tables, &Interruzione::default());
        let expected = match rust {
            Err(error) => {
                let error = serde_json::to_value(error.public_projection()).unwrap();
                assert!(!cli.status.success(), "{op}");
                for key in ["category", "phase", "remote_effect", "retry"] {
                    assert_eq!(runtime_doc[key], error[key], "runtime {op} {key}");
                    assert_eq!(cli_doc["error"][key], error[key], "CLI {op} {key}");
                }
                json!({"error":error})
            }
            Ok((report, outputs)) => {
                assert!(cli.status.success(), "{op}: {cli_doc}");
                assert_eq!(
                    runtime.metadata["plenora.output.contract"], "plenora-data-execution-result-v3",
                    "{op}: {runtime_doc}"
                );
                assert_eq!(cli_doc["result"]["steps"], report["steps"], "{op}");
                let effects: Value =
                    serde_json::from_str(&runtime.metadata["plenora.data.localized_effects"])
                        .unwrap();
                assert_eq!(
                    effects["steps"].as_array().unwrap().len(),
                    report["steps"].as_array().unwrap().len(),
                    "{op}"
                );
                let mut manifest_steps = report["steps"].clone();
                for step in manifest_steps.as_array_mut().unwrap() {
                    step.as_object_mut().unwrap().remove("effects");
                }
                assert_eq!(runtime_doc["steps"], manifest_steps, "{op}");
                for (step, original) in effects["steps"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .zip(report["steps"].as_array().unwrap())
                {
                    assert_eq!(step["out"], original["out"], "{op}");
                    assert_eq!(step["op"], original["op"], "{op}");
                    assert_eq!(step["effects"], original["effects"], "{op}");
                }
                let expected = &outputs[0].1;
                for path in [folder.join("cli.arrow"), host.output] {
                    let actual = leggi_tabella(&path, None, u64::MAX).unwrap();
                    assert_eq!(actual.schema(), expected.schema(), "{op}");
                    assert_eq!(actual.num_rows(), expected.num_rows(), "{op}");
                    if case["compare_values"] == true {
                        assert_eq!(&actual, expected, "{op}");
                    } else {
                        assert_eq!(op, "table.uuid_generator");
                        let generated = case["plan"]["steps"][0]["config"]["output_column"]
                            .as_str()
                            .unwrap();
                        for (index, field) in expected.schema().fields().iter().enumerate() {
                            if field.name() == generated {
                                for batch in [&actual, expected] {
                                    let column = batch
                                        .column(index)
                                        .as_any()
                                        .downcast_ref::<plenora_core::arrow::array::StringArray>()
                                        .unwrap();
                                    for value in column {
                                        let value = value.unwrap();
                                        assert_eq!(value.len(), 36);
                                        assert_eq!(&value[14..15], "4");
                                        assert!(matches!(&value[19..20], "8" | "9" | "a" | "b"));
                                        assert!(value.bytes().enumerate().all(|(i, b)| {
                                            if [8, 13, 18, 23].contains(&i) {
                                                b == b'-'
                                            } else {
                                                b.is_ascii_digit() || (b'a'..=b'f').contains(&b)
                                            }
                                        }));
                                    }
                                }
                            } else {
                                assert_eq!(actual.column(index), expected.column(index), "{op}");
                            }
                        }
                    }
                }
                let mut file = std::fs::File::create(folder.join("expected.arrow")).unwrap();
                plenora_io::ipc::scrivi(expected, &mut file).unwrap();
                json!({"report":report})
            }
        };
        std::fs::write(folder.join("expected.json"), expected.to_string()).unwrap();
        println!("{op}: Rust CLI runtime OK");
    }
}
