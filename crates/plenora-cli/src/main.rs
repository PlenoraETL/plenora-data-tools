//! Interfaccia a riga di comando di plenora-data-tools (architettura.md, cli.md).
//!
//! Un solo eseguibile per tre gruppi di comandi che non condividono il
//! percorso di esecuzione: tabellari (`run`, `self-test`), geospaziali
//! (`capabilities`, `transform`, `spatial-join`, `transform-arrow`,
//! `pair-arrow`) e sul catalogo unificato (`catalog`, `validate`).
//!
//! **Superficie a compatibilita' congelata**: i comandi geospaziali e `run`
//! sui piani `schema_version <= 3` (modulo `cli::commands::legacy`). `validate`
//! e `run` sui piani DAG passano da `plenora_engine::planner::validate` ed
//! `execute`: la v4 migra al canonico v5, la v6 ha parser e dominio di
//! `plan_hash` propri.
//!
//! Fail-closed: nessun output parziale, publish atomico no-clobber, messaggi
//! senza dati, mai un'uscita a zero dopo un errore. L'exit code dipende dalla
//! categoria ([`cli::error_envelope::exit_code_di`], tabella in `docs/cli.md`).
//! Ctrl-C cancella l'esecuzione DAG in modo cooperativo senza pubblicare
//! nulla, con exit 130; il secondo forza l'uscita
//! (errori-e-limiti.md#cancellazione).

use std::env;
use std::error::Error;
use std::path::Path;

use plenora_core::PlenoraError;

mod cli;

use cli::args::{help_text, reject_unknown_flags, subcommand_help_text, superficie};
use cli::commands::catalog::{capabilities_command, catalog_command};
use cli::commands::describe::describe_command;
use cli::commands::legacy::{
    execute_pair_arrow, execute_spatial_join, execute_transform, execute_transform_arrow,
    run_pipeline, self_test_command,
};
use cli::commands::run::{run_command, DagInputs};
use cli::commands::validate::validate_command;
use cli::error_envelope::{emetti_e_codice, error_envelope};
// Il dispatch delle modalita' riservate emette da se': solo su Linux, e nei
// casi.
#[cfg(any(test, target_os = "linux"))]
use cli::error_envelope::emit_error_envelope;
use cli::process::esegui_processo;
use cli::rendering::{hex_digest, version_json};

// I nomi che i moduli di `cli` raggiungono come `crate::...`: restano qui
// come riesportazioni, cosi' lo spostamento non tocca i chiamanti.
pub(crate) use cli::args::{
    argument_value, arrow_output_format, has_flag, optional_value_after, value_after,
};
pub(crate) use cli::formato::{strip_output_format, OutputFormat};
pub(crate) use cli::piano::{
    contract_error_missing, da_testo_di_controllo, read_control_json, read_control_plan_text,
    testo_piano_dag, PlanInputsProbe,
};
pub(crate) use cli::pubblicazione::{campi_della_pubblicazione, frammento_della_pubblicazione};
pub(crate) use cli::rendering::{graph_summary_json, metrics_json};
pub(crate) use cli::segnali::install_ctrlc_handler;
#[cfg(target_os = "linux")]
pub(crate) use cli::segnali::installa_gestore_segnale_isolato;

// Quello che serve SOLO ai test di questo file, che raggiungono i nomi di
// main.rs con `use super::*`. Tenerli fra gli import normali lascerebbe un
// warning permanente nella build del binario, ed e' cosi' che i warning
// smettono di volere dire qualcosa.
#[cfg(test)]
use cli::commands::legacy::{read_geometry_stream, transform_stream, TransformSchema};
#[cfg(test)]
use cli::commands::run::{is_named_input, reject_legacy_row_diagnostics_plan, v4_inputs};
#[cfg(test)]
use cli::contract_discovery::{
    apply_crs_decisions, at_input, discover_input_contract_from_schema, ipc_header_schema,
    open_input, pair_v4_inputs,
};
#[cfg(test)]
use plenora_core::contract::arrow_schema::{
    contract_crs_from_keys, crs_definition_from_metadata, geometry_contract_from_field,
};
// Serve ai casi **e** al dispatch dello spawner, che deve tradurre il proprio
// rifiuto in un exit code senza passare da `esegui_processo`.
#[cfg(any(test, target_os = "linux"))]
use cli::error_envelope::error_exit_code;
#[cfg(test)]
use plenora_core::arrow::array::RecordBatch;
#[cfg(test)]
use plenora_core::arrow::ipc::writer::FileWriter;
#[cfg(test)]
use plenora_core::arrow::schema::{DataType, SchemaRef};
#[cfg(test)]
use plenora_core::contract::{ContractCrs, DataContract};
#[cfg(test)]
use plenora_core::contract::{
    ContractProperties, CrsResolution, FieldId, GeometryColumnContract, PropertyConfidence,
    PropertyScope,
};
#[cfg(test)]
use plenora_engine::geo_transport::protocol::{Frame, FrameReader, FrameWriter};
#[cfg(test)]
use plenora_engine::{ipc_boundary, IpcFormat};
#[cfg(test)]
use plenora_engine::{Input, IpcLimits};
#[cfg(test)]
use plenora_kernels_geo::arrow_adapter::{
    read_geometry_contract_keys, CanonicalGeometryKeys, GEOARROW_EXTENSION_KEY,
    GEOARROW_WKB_EXTENSION, GEO_METADATA_KEY,
};
#[cfg(test)]
use plenora_kernels_geo::Operation;

#[cfg(all(test, not(feature = "proj-backend")))]
use plenora_core::crs::resolve_crs;
#[cfg(all(test, feature = "proj-backend"))]
use plenora_kernels_geo::crs::resolve_crs;

// ---------------------------------------------------------------------------
// Helper comuni
// ---------------------------------------------------------------------------

pub(crate) fn contract(message: impl Into<String>) -> PlenoraError {
    PlenoraError::InvalidPlan(message.into())
}

/// Costruttore esplicito dei limiti di RISORSA della CLI.
///
/// Come `contract`, rende la categoria visibile e cercabile nel punto d'uso:
/// passando da `contract` tetti e traboccamenti uscirebbero `invalid_plan`.
pub(crate) fn limite_risorsa(message: impl Into<String>) -> PlenoraError {
    PlenoraError::ResourceLimit(message.into())
}

// Dispatch unico dei sottocomandi: la lunghezza e' data dalla sequenza
// lineare dei casi, non da complessita' logica, e spezzarla in funzioni
// artificiali peggiorerebbe solo la leggibilita'.
#[allow(clippy::too_many_lines)]
pub(crate) fn run_with_args(args: &[String]) -> Result<(), Box<dyn Error>> {
    // La validazione precede QUALUNQUE uscita anticipata, help compreso.
    // Trattando `--help` per primo, `run --help junk` stamperebbe l'aiuto e
    // uscirebbe con successo ignorando `junk`: un parser che risponde «va
    // bene» a un'invocazione che non ha capito e' fail-open anche quando non
    // pubblica nulla.
    if let Some(comando) = args.first() {
        reject_unknown_flags(comando, args)?;
        // Il formato si controlla qui, in un punto solo, per ogni comando
        // della superficie: un comando senza resa markdown che dimenticasse
        // il controllo accetterebbe il flag e lo disattenderebbe.
        if superficie(comando).is_some_and(|comando| !comando.ha_resa_markdown()) {
            OutputFormat::require_json(comando)?;
        }
    }
    if args
        .get(1)
        .is_some_and(|argument| matches!(argument.as_str(), "--help" | "-h"))
    {
        if let Some(help) = args
            .first()
            .and_then(|command| subcommand_help_text(command))
        {
            println!("{help}");
            return Ok(());
        }
    }
    match args.first().map(String::as_str) {
        Some("--help" | "-h") => {
            println!("{}", help_text());
            Ok(())
        }
        Some("--version" | "-V") => {
            // `--version --json` (o `--format json` esplicito) per gli
            // orchestratori: una versione che si legge solo a occhio non e'
            // verificabile da uno script.
            if has_flag(args, "--json") || OutputFormat::active() == OutputFormat::Json {
                println!("{}", serde_json::to_string_pretty(&version_json())?);
            } else {
                println!("plenora-data-tools {}", env!("CARGO_PKG_VERSION"));
            }
            Ok(())
        }
        Some("catalog") => catalog_command(args),
        Some("describe" | "inspect-dataset") => describe_command(args),
        Some("validate") => validate_command(args),
        Some("run") => run_command(args),
        Some("capabilities") => capabilities_command(),
        Some("transform") => {
            let input = argument_value(args, "--input")?;
            let schema = argument_value(args, "--schema")?;
            let output = argument_value(args, "--output")?;
            let (summary, esito) = execute_transform(&input, Path::new(&schema), &output)?;
            println!(
                "{{\"status\":\"ok\",\"rows\":{},\"sha256\":\"{}\",{}}}",
                summary.rows,
                hex_digest(&summary.checksum),
                frammento_della_pubblicazione(&esito)
            );
            Ok(())
        }
        Some("transform-arrow") => {
            let output_format = arrow_output_format(args)?;
            let input = argument_value(args, "--input")?;
            let schema = argument_value(args, "--schema")?;
            let output = argument_value(args, "--output")?;
            let (summary, esito) =
                execute_transform_arrow(&input, Path::new(&schema), &output, output_format)?;
            println!(
                "{{\"status\":\"ok\",\"rows\":{},\"output_rows\":{},\"sha256\":\"{}\",{}}}",
                summary.rows,
                summary.output_rows,
                hex_digest(&summary.checksum),
                frammento_della_pubblicazione(&esito)
            );
            Ok(())
        }
        Some("pair-arrow") => {
            let output_format = arrow_output_format(args)?;
            let left = argument_value(args, "--left")?;
            let right = argument_value(args, "--right")?;
            let schema = argument_value(args, "--schema")?;
            let output = argument_value(args, "--output")?;
            if left == "-" || right == "-" || output == "-" {
                return Err(contract(
                    "pair-arrow richiede percorsi file per due input e output transazionale",
                )
                .into());
            }
            let (summary, esito) = execute_pair_arrow(
                Path::new(&left),
                Path::new(&right),
                Path::new(&schema),
                Path::new(&output),
                output_format,
            )?;
            println!(
                "{{\"status\":\"ok\",\"left_rows\":{},\"right_rows\":{},\"output_rows\":{},\"sha256\":\"{}\",{}}}",
                summary.left_rows,
                summary.right_rows,
                summary.output_rows,
                hex_digest(&summary.checksum),
                frammento_della_pubblicazione(&esito)
            );
            Ok(())
        }
        Some("spatial-join") => {
            let left = argument_value(args, "--left")?;
            let right = argument_value(args, "--right")?;
            let schema = argument_value(args, "--schema")?;
            let output = argument_value(args, "--output")?;
            if left == "-" || right == "-" || output == "-" {
                return Err(contract(
                    "spatial-join richiede percorsi file per due input e output transazionale",
                )
                .into());
            }
            let (summary, esito) = execute_spatial_join(
                Path::new(&left),
                Path::new(&right),
                Path::new(&schema),
                Path::new(&output),
            )?;
            println!(
                "{{\"status\":\"ok\",\"pairs\":{},\"sha256\":\"{}\",{}}}",
                summary.pairs,
                hex_digest(&summary.checksum),
                frammento_della_pubblicazione(&esito)
            );
            Ok(())
        }
        Some("self-test") => self_test_command(args),
        _ => Err(contract(
            "comando non valido: `plenora-data-tools --help` elenca i comandi disponibili",
        )
        .into()),
    }
}

fn main() {
    // Ultima barriera del processo: un panico che sfugge, anche di una
    // dipendenza, diventa un envelope su stdout. L'hook di default scriverebbe
    // su stderr, quindi si silenzia (`plenora_core::panic_policy`).
    //
    // L'esito di `install` si conserva: `false` significa che un hook era gia'
    // installato prima di `main`, e se poi un panico arriva va dichiarato che
    // «stderr vuoto» non e' garantito. Nemmeno `true` impedisce un
    // `set_hook` successivo (vedi `panic_policy`).
    let politica_nostra =
        plenora_core::panic_policy::install(plenora_core::panic_policy::PanicPolicy::Silent);

    // I dispatch delle modalita' riservate stanno qui, prima di tutto.
    //
    // Questo eseguibile e' anche lo spawner del profilo isolato, riconosciuto
    // da `argv[1]`. Lo spawner pretende un processo monothread (le
    // credenziali si cambiano per thread): fino a qui non nasce nessun
    // thread, mentre `esegui_processo` puo' costruire il pool di rayon. Il
    // caso riuscito non torna: la `exec` ha sostituito l'immagine.
    #[cfg(target_os = "linux")]
    {
        let argomenti: Vec<std::ffi::OsString> = std::env::args_os().collect();
        // Le tre modalita' si interrogano in fila, e **prima** del parser della
        // CLI. I namespace sono disgiunti, quindi l'ordine fra loro non
        // sposta niente; cio' che conta e' che vengano tutte prima, perche'
        // una riga del namespace riservato che arrivasse al parser si
        // sentirebbe rispondere «comando sconosciuto» invece della diagnosi
        // vera.
        for modalita in [
            plenora_engine::spawner_dal_confine,
            plenora_engine::worker_dal_confine,
            plenora_engine::verificatore_dal_confine,
        ] {
            match modalita(&argomenti) {
                // Non e' questa modalita': si prova la prossima, e poi il
                // parser.
                plenora_engine::DalConfine::AltroComando => {}
                // La modalita' ha finito il proprio lavoro. Si esce **qui**:
                // proseguire porterebbe la riga riservata al parser della CLI,
                // che risponderebbe «comando sconosciuto» dopo un'esecuzione
                // riuscita.
                plenora_engine::DalConfine::Conclusa => std::process::exit(0),
                plenora_engine::DalConfine::Fallita(errore) => {
                    let envelope = error_envelope(&errore, false);
                    let _ = emit_error_envelope(std::io::stdout().lock(), &envelope);
                    std::process::exit(error_exit_code(&envelope));
                }
            }
        }
    }

    let esito = std::panic::catch_unwind(esegui_processo);
    let codice = match esito {
        Ok(codice) => codice,
        Err(panico) => {
            let avvertenza = if politica_nostra {
                ""
            } else {
                "; hook di panico non installato da questo processo: \
                 il contenuto su stderr non e' sotto il nostro controllo"
            };
            let envelope = error_envelope(
                &PlenoraError::Internal(format!(
                    "panico non gestito: {}{avvertenza}",
                    // La forma del payload, mai il testo: un `assert_eq!`
                    // di una dipendenza puo' contenere i dati della riga.
                    plenora_core::panic_policy::forma_payload(panico.as_ref())
                )),
                false,
            );
            // `internal`: 70 anche dalla categoria, e 70 se l'envelope non
            // esce.
            emetti_e_codice(&envelope)
        }
    };
    std::process::exit(codice);
}

// ---------------------------------------------------------------------------
// Test
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::fs::File;
    use std::path::PathBuf;

    use plenora_core::arrow::schema::{Field, Schema};
    use plenora_core::ErrorPhase;

    use crate::cli::error_envelope::{retry_json, EXIT_CANCELLED, EXIT_INTERNO};
    use crate::cli::rendering::contract_json;
    use plenora_core::contract::{GeometryDimensions, GeometryEncoding};
    #[cfg(feature = "proj-backend")]
    use plenora_core::contract::{GeometryType, TypesDeclaration};
    use plenora_core::crs::{CrsKind, ResolvedCrs};
    use plenora_core::RetryDisposition;
    use plenora_kernels_geo::arrow_adapter::{
        PLENORA_CONTRACT_VERSION_KEY, PLENORA_GEOMETRY_AXIS_ORDER_KEY,
        PLENORA_GEOMETRY_CRS_DEFINITION_FORMAT_KEY, PLENORA_GEOMETRY_CRS_DEFINITION_KEY,
        PLENORA_GEOMETRY_CRS_ID_KEY, PLENORA_GEOMETRY_CRS_RESOLUTION_KEY,
        PLENORA_GEOMETRY_DIMENSIONS_KEY, PLENORA_GEOMETRY_ENCODING_KEY, PLENORA_GEOMETRY_SRID_KEY,
        PLENORA_GEOMETRY_TYPES_DECLARATION_KEY, PLENORA_GEOMETRY_TYPES_KEY,
    };

    #[test]
    fn error_envelope_is_capturable_on_declared_stdout_channel() {
        let envelope = error_envelope(&PlenoraError::InvalidPlan("piano invalido".into()), false);
        let mut stdout = Vec::new();
        emit_error_envelope(&mut stdout, &envelope).expect("stdout catturabile");
        let decoded: serde_json::Value = serde_json::from_slice(&stdout).expect("envelope JSON");
        assert_eq!(decoded, envelope);
    }

    /// Ogni categoria di `plenora_core::ErrorCategory` ha un exit code, e i
    /// codici sono quelli dichiarati: la tabella e' un contratto, non una
    /// tendenza. Il caso di default e' `70`, quindi una categoria nuova non
    /// resta senza codice in silenzio.
    #[test]
    fn un_errore_interno_di_passo_esce_come_interno_non_come_esecuzione() {
        // Ultimo anello della catena: una categoria `Internal` dentro un
        // `Replayed` diventa `internal`/exit 70, non `execution`/exit 6. Gli
        // anelli precedenti sono provati in `plenora_engine::error_propagation`;
        // nessun `Internal` dei kernel e' raggiungibile da un piano valido.
        let replayed = PlenoraError::Replayed(Box::new(plenora_core::error::ReplayedError {
            category: plenora_core::ErrorCategory::Internal,
            phase: ErrorPhase::Write,
            remote_effect: plenora_core::RemoteEffect::None,
            retry: plenora_core::RetryDisposition::Never,
            message: "invariante nostra violata al passo".into(),
            node: Some("n".into()),
            operation: Some("table.sort".into()),
            execution_id: None,
            execution_reason: None,
        }));
        let envelope = error_envelope(&replayed, false);
        assert_eq!(
            envelope["error"]["category"], "internal",
            "la categoria non si perde nell'involucro: {envelope}"
        );
        assert_eq!(
            error_exit_code(&envelope),
            EXIT_INTERNO,
            "e proietta su 70, non su 6: {envelope}"
        );
        // Il contesto del passo resta: preservare la categoria non deve
        // costare l'attribuzione.
        let testo = envelope.to_string();
        assert!(
            testo.contains("table.sort"),
            "nodo e operazione restano nella diagnostica: {envelope}"
        );
    }

    #[test]
    fn ogni_categoria_ha_l_exit_code_dichiarato() {
        // La tabella e' scritta a mano: e' la seconda opinione rispetto a
        // `exit_code_di`. Si itera `ErrorCategory::ALL` e si pretende che la
        // tabella nomini ogni categoria, cosi' una nuova non resta scoperta.
        let atteso: [(&str, i32); 20] = [
            ("invalid_plan", 2),
            ("invalid_configuration", 2),
            ("schema", 3),
            ("data_mapping", 3),
            ("crs", 3),
            ("unsupported", 3),
            ("resource_limit", 4),
            ("io", 5),
            ("not_found", 5),
            ("conflict", 5),
            ("protocol", 5),
            ("authentication", 5),
            ("authorization", 5),
            ("timeout", 5),
            ("transient", 5),
            ("isolation_unavailable", 5),
            ("unattributed_memory_pressure", 5),
            ("execution", 6),
            ("internal", 70),
            ("cancelled", 130),
        ];
        for &categoria in plenora_core::ErrorCategory::ALL {
            let nome = categoria.as_str();
            let codice = atteso
                .iter()
                .find_map(|(atteso, codice)| (*atteso == nome).then_some(*codice))
                .unwrap_or_else(|| {
                    panic!("categoria `{nome}` senza exit code dichiarato in questa tabella")
                });
            // Il giro completo, come lo fa la CLI: envelope -> stringa ->
            // categoria -> numero.
            let envelope = serde_json::json!({"error": {"category": nome}});
            assert_eq!(error_exit_code(&envelope), codice, "categoria `{nome}`");
            // E il ramo tipizzato da solo, senza passare per il JSON: e'
            // quello che il compilatore presidia.
            assert_eq!(
                cli::error_envelope::exit_code_di(categoria),
                codice,
                "categoria `{nome}`"
            );
        }
        // Una stringa che non e' una categoria non passa per «successo», e
        // nemmeno per una categoria vicina di nome.
        for ignota in ["categoria-nuova", "", "INTERNAL", "internal "] {
            let envelope = serde_json::json!({"error": {"category": ignota}});
            assert_eq!(
                error_exit_code(&envelope),
                EXIT_INTERNO,
                "stringa non riconosciuta `{ignota}`"
            );
        }
        // Un envelope senza il campo affatto: stesso ripiego, nessun panic.
        assert_eq!(error_exit_code(&serde_json::json!({})), EXIT_INTERNO);
    }

    use super::*;

    /// Envelope §9: gli assi di un `PlenoraError` arrivano in uscita
    /// espliciti (R9.2), il contesto DAG solo quando presente.
    #[test]
    fn error_envelope_carries_the_four_axes_and_dag_context() {
        let error = PlenoraError::Execution {
            node: "t".to_owned(),
            operation: "geo.centroid".to_owned(),
            execution_id: "exec-1".to_owned(),
            reason: "boom".to_owned(),
        };
        let envelope = error_envelope(&error, false);
        assert_eq!(envelope["status"], "error");
        assert_eq!(envelope["protocol_version"], 1);
        assert_eq!(envelope["error"]["category"], "execution");
        assert_eq!(envelope["error"]["phase"], "write");
        assert_eq!(envelope["error"]["remote_effect"], "none");
        assert_eq!(
            envelope["error"]["retry"],
            serde_json::json!({"kind": "never"})
        );
        assert_eq!(envelope["error"]["context"]["node"], "t");
        assert_eq!(envelope["error"]["context"]["operation"], "geo.centroid");
        assert_eq!(envelope["error"]["context"]["execution_id"], "exec-1");
        assert!(
            envelope["error"]["message"]
                .as_str()
                .expect("message")
                .contains("step failed at node `t`"),
            "il testo Display resta in message: {envelope}"
        );
    }

    #[test]
    fn error_envelope_omits_context_for_non_execution_and_omits_empty_execution_id() {
        let contract = PlenoraError::Unsupported("operazione sconosciuta".to_owned());
        let envelope = error_envelope(&contract, false);
        assert_eq!(envelope["error"]["category"], "unsupported");
        assert_eq!(envelope["error"]["phase"], "validate");
        assert_eq!(
            envelope["error"]["retry"],
            serde_json::json!({"kind": "never"})
        );
        assert!(envelope["error"].get("context").is_none());

        let legacy_step = PlenoraError::Execution {
            node: "n".to_owned(),
            operation: "table.filter".to_owned(),
            execution_id: String::new(),
            reason: "boom".to_owned(),
        };
        let legacy = error_envelope(&legacy_step, false);
        assert!(legacy["error"]["context"].get("execution_id").is_none());
    }

    #[test]
    fn legacy_date32_coerce_requires_dag_v4() {
        let plan = serde_json::json!({
            "schema_version": 1,
            "steps": [{
                "operation": "type_cast",
                "config": {"column": "effective_date", "target_type": "date32", "errors": "coerce"}
            }]
        });
        let error = reject_legacy_row_diagnostics_plan(&plan.to_string())
            .expect_err("legacy non può dichiarare completezza cross-batch");
        assert!(matches!(error, PlenoraError::Unsupported(_)));
    }

    #[test]
    fn every_catalog_row_diagnostic_operation_requires_dag_v4_in_legacy_plans() {
        // Catalog-driven (anti-drift): l'universo delle op arriva dal
        // catalogo, NON da una lista duplicata nel test. Per ogni
        // (descrittore, config) che l'autorita' dichiara diagnostica, ogni
        // nome risolvibile (id canonico + alias legacy) in un piano
        // sort -> op deve essere rifiutato DAL GATE (errore Unsupported, mai
        // da una validazione incidentale).
        let mut probes = vec![serde_json::json!({})];
        for target in [
            "int",
            "float",
            "bool",
            "uint64",
            "date",
            "datetime",
            "date32",
            "timestamp_millis",
            "decimal128",
            "str",
        ] {
            probes.push(serde_json::json!({"target_type": target}));
            probes.push(serde_json::json!({"target_type": target, "errors": "coerce"}));
            probes.push(serde_json::json!({"target_type": target, "errors": "raise"}));
        }
        for policy in ["error", "empty", "literal"] {
            probes.push(serde_json::json!({"null_policy": policy}));
        }
        let mut gated = std::collections::BTreeSet::new();
        for descriptor in plenora_core::catalog::CATALOG {
            let mut names = vec![descriptor.id.to_owned()];
            names.extend(
                plenora_core::catalog::ALIASES
                    .iter()
                    .filter(|(_, _, canonical)| *canonical == descriptor.id)
                    .map(|(_, alias, _)| (*alias).to_owned()),
            );
            for config in &probes {
                if !descriptor.emits_row_diagnostics(config) {
                    continue;
                }
                for name in &names {
                    let plan = serde_json::json!({
                        "schema_version": 1,
                        "steps": [
                            {"operation": "sort", "config": {"columns": ["id"]}},
                            {"operation": name, "config": config}
                        ]
                    });
                    let error = reject_legacy_row_diagnostics_plan(&plan.to_string())
                        .expect_err("op diagnostica legacy non bloccata dal gate");
                    assert!(
                        matches!(error, PlenoraError::Unsupported(_)),
                        "{} (nome `{name}`, config {config}): rifiuto atteso dal gate \
                         (Unsupported), ottenuto {error:?}",
                        descriptor.id
                    );
                    gated.insert(descriptor.id);
                }
            }
        }
        // Lock espliciti del perimetro: `formula` ed `expression` sono le due
        // che una lista locale tende a dimenticare, `md5`/`sha256` valgono
        // solo con null_policy=error, `type_cast` e' fallibile.
        for expected in [
            "table.formula",
            "table.expression",
            "table.type_cast",
            "table.md5_hash",
            "table.sha256_hash",
            "table.flatten_json",
            "table.assert_not_null",
            "geo.from_wkt",
            "geo.centroid",
        ] {
            assert!(
                gated.contains(expected),
                "{expected}: op diagnostica non coperta dal gate legacy"
            );
        }
    }

    #[test]
    fn legacy_gate_passes_operations_that_do_not_emit_row_diagnostics() {
        // md5/sha256 senza null_policy=error, type_cast verso `str`
        // e hmac_sha256 non emettono diagnostica row-scoped: il gate li
        // lascia passare (resta la validazione legacy, qui con config
        // valide). Lock anti-regressione sull'autorita' config-sensitive.
        for plan in [
            serde_json::json!({"schema_version": 1, "steps": [
                {"operation": "md5_hash", "config": {"columns": ["id"]}}
            ]}),
            serde_json::json!({"schema_version": 1, "steps": [
                {"operation": "md5_hash", "config": {"columns": ["id"], "null_policy": "empty"}}
            ]}),
            serde_json::json!({"schema_version": 1, "steps": [
                {"operation": "sha256_hash", "config": {
                    "columns": ["id"], "null_policy": "literal", "null_literal": "<null>"
                }}
            ]}),
            serde_json::json!({"schema_version": 1, "steps": [
                {"operation": "type_cast", "config": {
                    "column": "id", "target_type": "str", "date_format": "", "errors": "coerce"
                }}
            ]}),
            serde_json::json!({"schema_version": 1, "steps": [
                {"operation": "table.hmac_sha256", "config": {
                    "columns": ["id"], "key_env": "PLENORA_GATE_TEST_KEY"
                }}
            ]}),
        ] {
            reject_legacy_row_diagnostics_plan(&plan.to_string())
                .expect("op non diagnostica bloccata dal gate legacy");
        }
    }

    #[test]
    fn legacy_diagnostic_plans_are_rejected_even_when_blocking() {
        // Nel percorso legacy non esiste gate provenance (solo DAG):
        // qualsiasi op row-diagnostics, anche come primo step o dopo op
        // blocking row-preserving, richiede DAG — la materializzazione
        // completa non attesta la provenance `source_row_zero_based`.
        for plan in [
            serde_json::json!({
                "schema_version": 1,
                "steps": [{
                    "operation": "assert_unique",
                    "config": {"columns": ["id"], "nulls_equal": true}
                }]
            }),
            serde_json::json!({
                "schema_version": 1,
                "steps": [{
                    "operation": "assert_foreign_key",
                    "config": {"left_keys": ["id"], "right_keys": ["id"]}
                }]
            }),
            serde_json::json!({
                "schema_version": 1,
                "steps": [
                    {"operation": "bin", "config": {"column": "value", "bins": 2}},
                    {"operation": "assert_regex", "config": {"column": "name", "pattern": ".*"}}
                ]
            }),
            serde_json::json!({
                "schema_version": 1,
                "steps": [
                    {"operation": "add_row_number", "config": {}},
                    {"operation": "type_cast", "config": {"column": "value", "target_type": "date32"}}
                ]
            }),
        ] {
            let error = reject_legacy_row_diagnostics_plan(&plan.to_string())
                .expect_err("piano legacy diagnostico: provenance non attestabile");
            assert!(matches!(error, PlenoraError::Unsupported(_)));
        }
    }

    #[test]
    fn error_envelope_preserves_optional_row_diagnostics() {
        let mut counts = std::collections::BTreeMap::new();
        counts.insert("conversion.invalid_date".to_owned(), 1);
        let report = plenora_core::diagnostics::RowDiagnostics {
            contract: plenora_core::diagnostics::ROW_DIAGNOSTICS_CONTRACT.to_owned(),
            scope: plenora_core::diagnostics::RowDiagnosticScope::Read,
            index_basis: plenora_core::diagnostics::ROW_DIAGNOSTICS_INDEX_BASIS.to_owned(),
            completeness: plenora_core::diagnostics::RowDiagnosticsCompleteness::Complete,
            knowledge_limits: None,
            total: Some(1),
            observed_total: 1,
            counts,
            examples_limit: 10,
            examples_truncated: false,
            examples: vec![plenora_core::diagnostics::RowDiagnosticExample {
                source_index: 4,
                cause: "conversion.invalid_date".to_owned(),
                column: Some("effective_date".to_owned()),
                key: None,
                write_state: None,
            }],
            input_total: None,
            diagnostic_state_counts: None,
            write_outcome: None,
        };
        let error = PlenoraError::DataMapping("conversione data rifiutata".to_owned())
            .with_phase(ErrorPhase::Read)
            .with_row_diagnostics(report.clone());
        let envelope = error_envelope(&error, false);

        assert_eq!(
            envelope["error"]["row_diagnostics"],
            serde_json::to_value(&report).expect("serializzazione report")
        );
        assert_eq!(envelope["error"]["category"], "data_mapping");
        assert_eq!(envelope["error"]["phase"], "read");

        let ordinary = error_envelope(
            &PlenoraError::DataMapping("errore ordinario".to_owned()),
            false,
        );
        assert!(ordinary["error"].get("row_diagnostics").is_none());

        let mut invalid = report;
        invalid.examples_limit = 0;
        let direct = PlenoraError::RowDiagnostics {
            source: Box::new(PlenoraError::DataMapping("righe non conformi".to_owned())),
            diagnostics: Box::new(invalid.clone()),
        };
        let fail_closed = error_envelope(&direct, false);
        assert!(fail_closed["error"].get("row_diagnostics").is_none());
        assert_eq!(fail_closed["error"]["category"], "internal");
        assert_eq!(
            fail_closed["error"]["message"],
            "row diagnostics interne non valide"
        );

        let cancelled_direct = PlenoraError::RowDiagnostics {
            source: Box::new(PlenoraError::Cancelled {
                node: "cast".to_owned(),
                operation: "table.type_cast".to_owned(),
                execution_id: "exec-invalid".to_owned(),
                reason: "cancellazione richiesta".to_owned(),
            }),
            diagnostics: Box::new(invalid.clone()),
        };
        let cancelled_fail_closed = error_envelope(&cancelled_direct, true);
        assert_eq!(cancelled_fail_closed["error"]["category"], "internal");
        // Il declassamento fail-closed cambia la categoria in `internal`, e
        // con essa il codice: 70, non 130. Il codice segue la categoria
        // pubblicata, mai l'intenzione originale.
        assert_eq!(error_exit_code(&cancelled_fail_closed), 70);

        let rejected = PlenoraError::DataMapping("righe non conformi".to_owned())
            .with_row_diagnostics(invalid);
        assert_eq!(rejected.category(), plenora_core::ErrorCategory::Internal);
        assert!(rejected.row_diagnostics().is_none());
    }

    #[test]
    fn error_envelope_maps_io_json_and_unknown_errors() {
        let io = std::io::Error::other("disco pieno");
        let envelope = error_envelope(&io, false);
        assert_eq!(envelope["error"]["category"], "io");
        assert_eq!(envelope["error"]["phase"], "read");
        assert_eq!(
            envelope["error"]["retry"],
            serde_json::json!({"kind": "safe"})
        );

        let json = serde_json::from_str::<u32>("\"non-un-numero\"").expect_err("json invalido");
        let envelope = error_envelope(&json, false);
        assert_eq!(envelope["error"]["category"], "data_mapping");
        assert_eq!(envelope["error"]["phase"], "validate");
        assert_eq!(
            envelope["error"]["retry"],
            serde_json::json!({"kind": "never"})
        );
    }

    #[test]
    fn legacy_execution_without_id_keeps_node_and_operation_context() {
        let error = PlenoraError::Execution {
            node: "0".to_owned(),
            operation: "table.transpose".to_owned(),
            execution_id: String::new(),
            reason: "transpose supera i limiti".to_owned(),
        };

        let envelope = error_envelope(&error, false);

        assert_eq!(envelope["error"]["context"]["node"], "0");
        assert_eq!(envelope["error"]["context"]["operation"], "table.transpose");
        assert!(envelope["error"]["context"].get("execution_id").is_none());
    }

    #[test]
    fn pair_arrow_invalid_public_parameter_is_invalid_plan() {
        let error =
            plenora_engine::geo_transport::transport::ArrowTransportError::InvalidParameter {
                operation: "haversine_distance",
                name: "max_output_rows",
                reason: "oltre il limite righe del trasporto",
            };

        let envelope = error_envelope(&error, false);

        assert_eq!(envelope["error"]["category"], "invalid_plan");
        assert_eq!(envelope["error"]["phase"], "validate");
        assert_eq!(
            envelope["error"]["retry"],
            serde_json::json!({"kind": "never"})
        );
    }

    #[test]
    fn error_envelope_retry_after_carries_delay_ms() {
        // La forma taggata di conformance/components.json: senza delay_ms
        // il chiamante saprebbe DI riprovare piu' tardi, non QUANDO. Si
        // prova la funzione che `error_envelope` usa, non una sua copia:
        // nessun errore della CLI rende oggi `After`, e l'envelope intero
        // non lo puo' raggiungere.
        assert_eq!(
            retry_json(RetryDisposition::After(std::time::Duration::from_millis(
                5000
            ))),
            serde_json::json!({"kind": "after", "delay_ms": 5000})
        );
        // Una durata oltre `u64` millisecondi satura, non si tronca.
        assert_eq!(
            retry_json(RetryDisposition::After(std::time::Duration::MAX)),
            serde_json::json!({"kind": "after", "delay_ms": u64::MAX})
        );
        // Le altre disposizioni non portano `delay_ms`.
        for (disposizione, kind) in [
            (RetryDisposition::Never, "never"),
            (RetryDisposition::Safe, "safe"),
            (
                RetryDisposition::RequiresIdempotencyKey,
                "requires_idempotency_key",
            ),
            (RetryDisposition::RequiresRecovery, "requires_recovery"),
        ] {
            assert_eq!(
                retry_json(disposizione),
                serde_json::json!({ "kind": kind })
            );
        }
    }

    #[test]
    fn error_envelope_cancelled_keeps_dedicated_message_and_axes() {
        let error = PlenoraError::Cancelled {
            node: "t".to_owned(),
            operation: "table.filter".to_owned(),
            execution_id: "exec-9".to_owned(),
            reason: "cancellazione richiesta".to_owned(),
        };
        let envelope = error_envelope(&error, true);
        assert_eq!(envelope["error"]["category"], "cancelled");
        assert_eq!(error_exit_code(&envelope), EXIT_CANCELLED);
        assert_eq!(
            envelope["error"]["retry"],
            serde_json::json!({"kind": "never"})
        );
        assert!(
            envelope["error"]["message"]
                .as_str()
                .expect("message")
                .starts_with("esecuzione annullata: "),
            "messaggio dedicato preservato: {envelope}"
        );
        assert_eq!(envelope["error"]["context"]["execution_id"], "exec-9");

        let wrapped = error.with_row_diagnostics(plenora_core::diagnostics::RowDiagnostics {
            contract: plenora_core::diagnostics::ROW_DIAGNOSTICS_CONTRACT.to_owned(),
            scope: plenora_core::diagnostics::RowDiagnosticScope::Read,
            index_basis: plenora_core::diagnostics::ROW_DIAGNOSTICS_INDEX_BASIS.to_owned(),
            completeness: plenora_core::diagnostics::RowDiagnosticsCompleteness::Partial,
            observed_total: 1,
            total: None,
            input_total: None,
            counts: std::collections::BTreeMap::from([("conversion.invalid_date".to_owned(), 1)]),
            examples_limit: 10,
            examples_truncated: false,
            examples: vec![plenora_core::diagnostics::RowDiagnosticExample {
                source_index: 4,
                cause: "conversion.invalid_date".to_owned(),
                column: Some("effective_date".to_owned()),
                key: None,
                write_state: None,
            }],
            knowledge_limits: Some(vec!["data_tools.processing_interrupted".to_owned()]),
            diagnostic_state_counts: None,
            write_outcome: None,
        });
        assert!(wrapped.is_cancelled(), "exit code deve restare 130");
        let wrapped_envelope = error_envelope(&wrapped, wrapped.is_cancelled());
        assert_eq!(wrapped_envelope["error"]["category"], "cancelled");
        assert_eq!(error_exit_code(&wrapped_envelope), EXIT_CANCELLED);
        assert_eq!(
            wrapped_envelope["error"]["context"]["execution_id"],
            "exec-9"
        );
        assert!(wrapped_envelope["error"].get("row_diagnostics").is_some());
    }

    fn projected_crs() -> ResolvedCrs {
        ResolvedCrs::from_resolved_parts(
            "EPSG:32632".to_owned(),
            serde_json::json!({"type": "ProjectedCRS", "name": "WGS 84 / UTM zone 32N"}),
            CrsKind::Projected,
            Some(1.0),
        )
    }

    /// Campo geometria GeoArrow-WKB con il metadato `geo` dato (o senza).
    fn geometry_field(geo_json: Option<&str>) -> Field {
        let mut metadata = std::collections::HashMap::new();
        metadata.insert(
            GEOARROW_EXTENSION_KEY.to_owned(),
            GEOARROW_WKB_EXTENSION.to_owned(),
        );
        if let Some(geo) = geo_json {
            metadata.insert(GEO_METADATA_KEY.to_owned(), geo.to_owned());
        }
        Field::new("geometry", DataType::Binary, true).with_metadata(metadata)
    }

    /// Campo geometria con SOLE chiavi canoniche (niente `GeoArrow` legacy).
    fn canonical_geometry_field(data_type: DataType) -> Field {
        let metadata = std::collections::HashMap::from([
            (PLENORA_GEOMETRY_ENCODING_KEY.to_owned(), "wkb".to_owned()),
            (PLENORA_GEOMETRY_DIMENSIONS_KEY.to_owned(), "xyz".to_owned()),
            (
                PLENORA_GEOMETRY_TYPES_DECLARATION_KEY.to_owned(),
                "exact".to_owned(),
            ),
            (PLENORA_GEOMETRY_TYPES_KEY.to_owned(), "point".to_owned()),
            (
                PLENORA_GEOMETRY_CRS_RESOLUTION_KEY.to_owned(),
                "resolved".to_owned(),
            ),
            (
                PLENORA_GEOMETRY_CRS_ID_KEY.to_owned(),
                "EPSG:32632".to_owned(),
            ),
            (
                PLENORA_GEOMETRY_AXIS_ORDER_KEY.to_owned(),
                "unknown".to_owned(),
            ),
        ]);
        Field::new("geometry", data_type, true).with_metadata(metadata)
    }

    /// Schema con la versione di protocollo R2.5 nei metadati di schema.
    fn schema_v1(fields: Vec<Field>) -> SchemaRef {
        std::sync::Arc::new(Schema::new_with_metadata(
            fields,
            std::collections::HashMap::from([(
                PLENORA_CONTRACT_VERSION_KEY.to_owned(),
                "1".to_owned(),
            )]),
        ))
    }

    /// Lettura di contratto del campo + costruzione del contratto, come nel
    /// loop di discovery (la risoluzione CRS resta iniettata dai test).
    fn contract_from_field(field: &Field) -> Result<GeometryColumnContract, PlenoraError> {
        let keys = read_geometry_contract_keys(field)?;
        Ok(geometry_contract_from_field(
            field,
            ContractCrs::Resolved(projected_crs()),
            &keys,
        ))
    }

    #[test]
    fn discovery_reads_dimensions_and_encoding_from_metadata() {
        let contract = contract_from_field(&geometry_field(Some(
            r#"{"crs":"EPSG:32632","dimensions":"xyz","encoding":"ewkb"}"#,
        )))
        .expect("discovery");
        assert_eq!(contract.dimensions, GeometryDimensions::Xyz);
        assert_eq!(contract.encoding, Some(GeometryEncoding::Ewkb));

        // Forma scritta dai writer correnti (dimensions xy, niente encoding).
        let written =
            plenora_kernels_geo::arrow_adapter::geometry_output_field("geometry", "EPSG:32632")
                .expect("field");
        let contract = contract_from_field(&written).expect("discovery");
        assert_eq!(contract.dimensions, GeometryDimensions::Xy);
        // R2.7: il nome di estensione `geoarrow.wkb` dichiara
        // la famiglia WKB e completa l'encoding assente altrove (ultimo
        // rango della precedenza): non piu' `None`.
        assert_eq!(contract.encoding, Some(GeometryEncoding::Wkb));
    }

    #[test]
    fn discovery_without_dimensions_metadata_propagates_unknown_never_xy() {
        // (b) R3.4: chiave `dimensions` assente -> Unknown propagato nel
        // contratto, MAI un default silenzioso Xy.
        let contract = contract_from_field(&geometry_field(Some(r#"{"crs":"EPSG:32632"}"#)))
            .expect("discovery");
        assert_eq!(contract.dimensions, GeometryDimensions::Unknown);
        // Come sopra: encoding completato dal nome di estensione (R2.7).
        assert_eq!(contract.encoding, Some(GeometryEncoding::Wkb));
    }

    #[test]
    fn discovery_rejects_unreadable_dimensions_never_ignores_them() {
        // Reader strict (R5.1): un valore `dimensions` non canonico o non
        // testuale e' un errore esplicito, mai ignorato ne' mappato a
        // `Unknown` — «illeggibile» non e' «assente».
        for geo_json in [
            r#"{"crs":"EPSG:32632","dimensions":"2d"}"#,
            r#"{"crs":"EPSG:32632","dimensions":42}"#,
        ] {
            let result = read_geometry_contract_keys(&geometry_field(Some(geo_json)));
            assert!(result.is_err(), "geo: {geo_json}");
        }
    }

    #[test]
    fn discovery_rejects_unrepresentable_encoding() {
        // (d) R3.5: framing fuori dall'enum chiuso -> rifiuto esplicito
        // (Unsupported), mai mappato a un encoding noto.
        for geo_json in [
            r#"{"crs":"EPSG:32632","encoding":"gpkg"}"#,
            r#"{"crs":"EPSG:32632","encoding":"twkb"}"#,
            r#"{"crs":"EPSG:32632","encoding":42}"#,
        ] {
            let result = read_geometry_contract_keys(&geometry_field(Some(geo_json)));
            assert!(
                matches!(result, Err(PlenoraError::Unsupported(_))),
                "geo: {geo_json}"
            );
        }

        // Encoding rappresentabile -> propagato nel contratto.
        let contract = contract_from_field(&geometry_field(Some(
            r#"{"crs":"EPSG:32632","encoding":"wkb"}"#,
        )))
        .expect("discovery");
        assert_eq!(contract.encoding, Some(GeometryEncoding::Wkb));
    }

    #[test]
    fn discovery_legacy_field_leaves_types_undeclared() {
        // R3.4.1: ingresso legacy senza la coppia types_declaration/types ->
        // «proprieta' non dichiarata» (confidence Unknown), MAI unresolved.
        let contract = contract_from_field(&geometry_field(Some(r#"{"crs":"EPSG:32632"}"#)))
            .expect("discovery");
        assert!(contract.types.value().is_none());
    }

    #[test]
    fn discovery_recognizes_canonical_only_geometry_field() {
        // (a) tabella §2: le chiavi canoniche sono autosufficienti — il campo
        // e' riconosciuto come geometria anche senza estensione `geoarrow.wkb`
        // e metadato `geo`, con types Declared/Schema dalla coppia canonica.
        let schema = schema_v1(vec![
            Field::new("id", DataType::Int64, false),
            canonical_geometry_field(DataType::Binary),
        ]);
        let result = discover_input_contract_from_schema(schema, resolve_crs);
        #[cfg(feature = "proj-backend")]
        {
            let contract = result.expect("discovery canonica");
            assert_eq!(contract.geometries.len(), 1);
            let geometry = &contract.geometries[0];
            assert_eq!(geometry.dimensions, GeometryDimensions::Xyz);
            assert_eq!(geometry.encoding, Some(GeometryEncoding::Wkb));
            assert!(
                matches!(geometry.types.confidence, PropertyConfidence::Declared(_)),
                "types dichiarati dalla coppia canonica"
            );
            assert_eq!(geometry.types.scope, PropertyScope::Schema);
            let types = geometry.types.value().expect("types");
            assert_eq!(types.declaration(), TypesDeclaration::Exact);
            assert_eq!(types.types(), &[GeometryType::Point]);
        }
        #[cfg(not(feature = "proj-backend"))]
        {
            // Senza backend PROJ la risoluzione CRS fallisce chiusa DOPO il
            // riconoscimento: un errore `Crs` (non `InvalidPlan`) dimostra che
            // il campo canonico-only e' stato riconosciuto come geometria e
            // le chiavi lette senza errori.
            assert!(
                matches!(result, Err(PlenoraError::Crs(_))),
                "atteso fallimento di risoluzione CRS, ottenuto {result:?}"
            );
        }
    }

    #[test]
    fn discovery_rejects_canonical_geometry_field_of_non_binary_type() {
        // (1c) chiavi canoniche coerenti ma tipo non Binary -> errore.
        let schema = schema_v1(vec![canonical_geometry_field(DataType::Utf8)]);
        let result = discover_input_contract_from_schema(schema, resolve_crs);
        assert!(matches!(result, Err(PlenoraError::InvalidPlan(_))));
    }

    #[test]
    fn discovery_rejects_contract_version_newer_than_supported() {
        // (b) R2.5: versione successiva a quella nota -> fallimento
        // esplicito (Unsupported), mai interpretazione parziale.
        let schema = std::sync::Arc::new(Schema::new_with_metadata(
            vec![Field::new("id", DataType::Int64, false)],
            std::collections::HashMap::from([(
                PLENORA_CONTRACT_VERSION_KEY.to_owned(),
                "2".to_owned(),
            )]),
        ));
        let result = discover_input_contract_from_schema(schema, resolve_crs);
        assert!(matches!(result, Err(PlenoraError::Unsupported(_))));
    }

    #[test]
    fn discovery_rejects_canonical_keys_without_contract_version() {
        // R2.5: chiavi canoniche senza `plenora.contract.version` nei
        // metadati dello schema -> errore esplicito.
        let schema = std::sync::Arc::new(Schema::new(vec![canonical_geometry_field(
            DataType::Binary,
        )]));
        let result = discover_input_contract_from_schema(schema, resolve_crs);
        assert!(matches!(result, Err(PlenoraError::InvalidPlan(_))));
    }

    #[test]
    fn discovery_rejects_canonical_legacy_divergence() {
        // (c) R2.6: nozione divergente fra chiavi canoniche e metadato legacy
        // -> il componente fallisce, non sceglie.
        let field = geometry_field(Some(r#"{"crs":"EPSG:32632","dimensions":"xy"}"#));
        let mut metadata = field.metadata().clone();
        metadata.insert(PLENORA_GEOMETRY_DIMENSIONS_KEY.to_owned(), "xyz".to_owned());
        let field = field.with_metadata(metadata);
        let result = discover_input_contract_from_schema(schema_v1(vec![field]), resolve_crs);
        match result {
            Err(PlenoraError::InvalidPlan(message)) => {
                assert!(message.contains("divergente"), "{message}");
            }
            other => panic!("attesa divergenza R2.6, ottenuto {other:?}"),
        }
    }

    // -------------------------------------------------------------------
    // R4.6.3 (contratti trasversali v2.0-rc9/rc10): la discovery non
    // pretende un CRS risolvibile — lo stato `missing` entra nel contratto.
    // -------------------------------------------------------------------

    #[test]
    fn discovery_geometry_without_crs_is_missing_not_an_error() {
        // Colonna GeoArrow-WKB senza metadato `geo` e senza chiavi
        // canoniche: nessun CRS dichiarato in alcuna rappresentazione
        // accettata -> `ContractCrs::Missing` (R4.4: mai un CRS inventato),
        // non un errore. La discovery non chiede la risoluzione, quindi il
        // test vale con e senza backend PROJ.
        let schema = std::sync::Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int64, false),
            geometry_field(None),
        ]));
        let contract = discover_input_contract_from_schema(schema, resolve_crs).expect("discovery");
        assert_eq!(contract.geometries.len(), 1);
        assert!(
            matches!(contract.geometries[0].crs, ContractCrs::Missing),
            "CRS assente -> stato missing"
        );
    }

    #[test]
    fn discovery_geometry_with_geo_metadata_without_crs_is_missing() {
        // Metadato `geo` presente ma senza chiave `crs` (dimensions sola):
        // anche qui nessun CRS dichiarato -> `missing`, mai errore.
        let schema = std::sync::Arc::new(Schema::new(vec![geometry_field(Some(
            r#"{"dimensions":"xy"}"#,
        ))]));
        let contract = discover_input_contract_from_schema(schema, resolve_crs).expect("discovery");
        assert!(matches!(contract.geometries[0].crs, ContractCrs::Missing));
        assert_eq!(contract.geometries[0].dimensions, GeometryDimensions::Xy);
    }

    #[test]
    fn discovery_canonical_missing_resolution_is_carried() {
        // `crs_resolution = missing` dichiarato canonicamente (senza chiavi
        // CRS, come impone la coerenza R2.2) -> stato missing nel contratto.
        let metadata = std::collections::HashMap::from([
            (PLENORA_GEOMETRY_ENCODING_KEY.to_owned(), "wkb".to_owned()),
            (PLENORA_GEOMETRY_DIMENSIONS_KEY.to_owned(), "xy".to_owned()),
            (
                PLENORA_GEOMETRY_CRS_RESOLUTION_KEY.to_owned(),
                "missing".to_owned(),
            ),
        ]);
        let field = Field::new("geometry", DataType::Binary, true).with_metadata(metadata);
        let contract = discover_input_contract_from_schema(schema_v1(vec![field]), resolve_crs)
            .expect("discovery");
        assert!(matches!(contract.geometries[0].crs, ContractCrs::Missing));
    }

    #[test]
    fn discovery_rejects_resolution_declaration_without_any_crs() {
        // R4.1: una dichiarazione `resolved` (o `declared_unresolved`) senza
        // alcuna rappresentazione CRS e' una contraddizione — MAI collassata
        // su `missing`: errore esplicito che nomina la chiave.
        for resolution in ["resolved", "declared_unresolved"] {
            let metadata = std::collections::HashMap::from([
                (PLENORA_GEOMETRY_DIMENSIONS_KEY.to_owned(), "xy".to_owned()),
                (
                    PLENORA_GEOMETRY_CRS_RESOLUTION_KEY.to_owned(),
                    resolution.to_owned(),
                ),
            ]);
            let field = Field::new("geometry", DataType::Binary, true).with_metadata(metadata);
            let result = discover_input_contract_from_schema(schema_v1(vec![field]), resolve_crs);
            match result {
                Err(PlenoraError::InvalidPlan(message)) => {
                    assert!(
                        message.contains("nessun CRS e' dichiarato in alcuna rappresentazione"),
                        "{resolution}: {message}"
                    );
                }
                other => panic!("{resolution}: attesa contraddizione, ottenuto {other:?}"),
            }
        }
    }

    #[test]
    fn discovery_rejects_malformed_geo_metadata_never_treats_it_as_missing() {
        // R5.1: un metadato `geo` illeggibile non diventa «CRS assente» —
        // «illeggibile» non e' «assente»: e' un errore, e R4.6.3 non lo
        // trasforma in un CRS mancante.
        let schema = std::sync::Arc::new(Schema::new(vec![geometry_field(Some("not json"))]));
        let result = discover_input_contract_from_schema(schema, resolve_crs);
        assert!(result.is_err(), "metadato geo malformato -> errore");
    }

    // -------------------------------------------------------------------
    // R4.6.3 (BLOCK-08): `declared_unresolved` — preservato, mai risolto
    // in assenza di una decisione esplicita nel piano.
    // -------------------------------------------------------------------

    /// Campo geometria canonico con le chiavi date (helper delle fixture
    /// CRS: schema con versione R2.5, colonna `id` + `geometry`).
    fn canonical_crs_field(pairs: &[(&str, &str)]) -> Field {
        let mut metadata = std::collections::HashMap::from([
            (PLENORA_GEOMETRY_ENCODING_KEY.to_owned(), "wkb".to_owned()),
            (PLENORA_GEOMETRY_DIMENSIONS_KEY.to_owned(), "xy".to_owned()),
        ]);
        for (key, value) in pairs {
            metadata.insert((*key).to_owned(), (*value).to_owned());
        }
        Field::new("geometry", DataType::Binary, true).with_metadata(metadata)
    }

    #[test]
    fn discovery_declared_unresolved_is_preserved_never_auto_resolved() {
        // Cambio di comportamento dichiarato (R4.6.3): una dichiarazione
        // `declared_unresolved` con crs_id RISOLVIBILE (EPSG:32632) non e'
        // piu' risolta ed emessa come `resolved` — il centro preserva lo
        // stato dichiarato. Nessun backend coinvolto: il test vale con e
        // senza `proj-backend`.
        let field = canonical_crs_field(&[
            (PLENORA_GEOMETRY_CRS_RESOLUTION_KEY, "declared_unresolved"),
            (PLENORA_GEOMETRY_CRS_ID_KEY, "EPSG:32632"),
            (PLENORA_GEOMETRY_AXIS_ORDER_KEY, "unknown"),
        ]);
        let contract = discover_input_contract_from_schema(schema_v1(vec![field]), resolve_crs)
            .expect("discovery");
        let ContractCrs::DeclaredUnresolved {
            crs_id, definition, ..
        } = &contract.geometries[0].crs
        else {
            panic!(
                "atteso DeclaredUnresolved: {:?}",
                contract.geometries[0].crs
            );
        };
        assert_eq!(crs_id.as_deref(), Some("EPSG:32632"));
        assert_eq!(definition, &None);
        assert_eq!(
            contract.geometries[0].crs.resolution(),
            CrsResolution::DeclaredUnresolved
        );
    }

    #[test]
    fn discovery_conflicting_crs_id_and_srid_become_declared_unresolved() {
        // Il caso `conflicting_crs` del corpus di conformita': crs_id=EPSG:4326
        // con srid=3003 (R4.3.1). Il centro preserva: lo stato diventa
        // DeclaredUnresolved con la dichiarazione originale. Resta preservato
        // anche con `resolved` dichiarato (test successivo, H-06).
        let field = canonical_crs_field(&[
            (PLENORA_GEOMETRY_CRS_ID_KEY, "EPSG:4326"),
            (PLENORA_GEOMETRY_AXIS_ORDER_KEY, "lon_lat"),
            (PLENORA_GEOMETRY_SRID_KEY, "3003"),
        ]);
        let contract = discover_input_contract_from_schema(schema_v1(vec![field]), resolve_crs)
            .expect("discovery");
        let ContractCrs::DeclaredUnresolved {
            crs_id, definition, ..
        } = &contract.geometries[0].crs
        else {
            panic!(
                "atteso DeclaredUnresolved: {:?}",
                contract.geometries[0].crs
            );
        };
        assert_eq!(crs_id.as_deref(), Some("EPSG:4326"));
        assert_eq!(definition, &None);
    }

    #[test]
    fn discovery_declared_resolved_with_conflicting_crs_id_and_srid_stays_unresolved() {
        // Una dichiarazione `resolved` non puo' nascondere un conflitto
        // numerico decidibile fra identificatore e SRID (H-06/R4.1).
        let field = canonical_crs_field(&[
            (PLENORA_GEOMETRY_CRS_RESOLUTION_KEY, "resolved"),
            (PLENORA_GEOMETRY_CRS_ID_KEY, "EPSG:4326"),
            (PLENORA_GEOMETRY_AXIS_ORDER_KEY, "lon_lat"),
            (PLENORA_GEOMETRY_SRID_KEY, "3003"),
        ]);
        let contract = discover_input_contract_from_schema(schema_v1(vec![field]), resolve_crs)
            .expect("discovery");
        let ContractCrs::DeclaredUnresolved {
            crs_id, definition, ..
        } = &contract.geometries[0].crs
        else {
            panic!(
                "atteso DeclaredUnresolved: {:?}",
                contract.geometries[0].crs
            );
        };
        assert_eq!(crs_id.as_deref(), Some("EPSG:4326"));
        assert_eq!(definition, &None);
    }

    #[test]
    fn discovery_crs_id_and_definition_copresent_become_declared_unresolved() {
        // Due rappresentazioni risolvibili co-presenti: l'accordo non e'
        // decidibile testualmente (R2.7), quindi `DeclaredUnresolved` con
        // entrambe le dichiarazioni. Emendamento 2026-07-31 (classe A): la
        // regola (2a) vale solo per input non dichiarati, per questo la fixture
        // non porta `crs_resolution`.
        let field = canonical_crs_field(&[
            (PLENORA_GEOMETRY_CRS_ID_KEY, "EPSG:4326"),
            (
                PLENORA_GEOMETRY_CRS_DEFINITION_KEY,
                r#"{"type":"GeographicCRS"}"#,
            ),
            (PLENORA_GEOMETRY_CRS_DEFINITION_FORMAT_KEY, "projjson"),
            (PLENORA_GEOMETRY_AXIS_ORDER_KEY, "lat_lon"),
        ]);
        let contract = discover_input_contract_from_schema(schema_v1(vec![field]), resolve_crs)
            .expect("discovery");
        let ContractCrs::DeclaredUnresolved {
            crs_id,
            definition,
            definition_format,
        } = &contract.geometries[0].crs
        else {
            panic!(
                "atteso DeclaredUnresolved: {:?}",
                contract.geometries[0].crs
            );
        };
        assert_eq!(crs_id.as_deref(), Some("EPSG:4326"));
        assert_eq!(definition.as_deref(), Some(r#"{"type":"GeographicCRS"}"#));
        assert_eq!(
            definition_format.map(plenora_core::contract::CrsDefinitionFormat::as_str),
            Some("projjson")
        );
    }

    // -------------------------------------------------------------------
    // Emendamento 2026-07-31 (classe A): `resolved` dichiarato con doppia
    // rappresentazione — risoluzione + verifica di coerenza decidibile.
    // -------------------------------------------------------------------

    /// WKT1 realistico di Monte Mario / Italy zone 1 con `AUTHORITY` e
    /// `TOWGS84` (EPSG:3003): la forma dello shapefile catastale owner.
    const MONTE_MARIO_WKT: &str = concat!(
        r#"PROJCS["Monte Mario / Italy zone 1",GEOGCS["Monte Mario","#,
        r#"DATUM["Monte_Mario",SPHEROID["International 1924",6378388,297],"#,
        r#"TOWGS84[-104.1,-49.1,-9.9,0.971,-2.917,0.714,-11.68]],"#,
        r#"PRIMEM["Greenwich",0],UNIT["degree",0.0174532925199433]],"#,
        r#"PROJECTION["Transverse_Mercator"],PARAMETER["latitude_of_origin",0],"#,
        r#"PARAMETER["central_meridian",9],PARAMETER["scale_factor",0.9996],"#,
        r#"PARAMETER["false_easting",1500000],PARAMETER["false_northing",0],"#,
        r#"UNIT["metre",1],AXIS["Easting",EAST],AXIS["Northing",NORTH],"#,
        r#"AUTHORITY["EPSG","3003"]]"#
    );

    /// Coppie canoniche del caso owner: `resolved` dichiarato, doppia
    /// rappresentazione (`crs_id` + definizione WKT) con formato `wkt`.
    fn monte_mario_resolved_pairs(crs_id: &str) -> Vec<(&'static str, String)> {
        vec![
            (PLENORA_GEOMETRY_CRS_RESOLUTION_KEY, "resolved".to_owned()),
            (PLENORA_GEOMETRY_CRS_ID_KEY, crs_id.to_owned()),
            (
                PLENORA_GEOMETRY_CRS_DEFINITION_KEY,
                MONTE_MARIO_WKT.to_owned(),
            ),
            (PLENORA_GEOMETRY_CRS_DEFINITION_FORMAT_KEY, "wkt".to_owned()),
            (
                PLENORA_GEOMETRY_AXIS_ORDER_KEY,
                "easting_northing".to_owned(),
            ),
        ]
    }

    #[cfg(feature = "proj-backend")]
    #[test]
    fn discovery_resolved_with_coherent_wkt_resolves_with_authority_srid() {
        // Il caso owner: `resolved` + crs_id=EPSG:3003 + WKT Monte Mario
        // coerente. La (2a) NON rovescia la dichiarazione: il WKT risolve
        // contro PROJ e la verifica di coerenza (crs_id 3003 == srid del
        // canonical) conferma — `Resolved`, con `authority_srid` 3003.
        let pairs = monte_mario_resolved_pairs("EPSG:3003");
        let pairs_ref: Vec<(&str, &str)> = pairs
            .iter()
            .map(|(key, value)| (*key, value.as_str()))
            .collect();
        let field = canonical_crs_field(&pairs_ref);
        let contract = discover_input_contract_from_schema(schema_v1(vec![field]), resolve_crs)
            .expect("discovery");
        let ContractCrs::Resolved(resolved) = &contract.geometries[0].crs else {
            panic!("atteso Resolved: {:?}", contract.geometries[0].crs);
        };
        assert_eq!(resolved.authority_srid(), Some(3003));
    }

    #[cfg(feature = "proj-backend")]
    #[test]
    fn discovery_resolved_with_divergent_crs_id_becomes_declared_unresolved() {
        // Stessa fixture ma crs_id=EPSG:4326: il WKT risolve a 3003, il
        // confronto decidibile smentisce il `resolved` dichiarato —
        // `DeclaredUnresolved` con le dichiarazioni ORIGINALI preservate
        // (non passa e nulla si perde).
        let pairs = monte_mario_resolved_pairs("EPSG:4326");
        let pairs_ref: Vec<(&str, &str)> = pairs
            .iter()
            .map(|(key, value)| (*key, value.as_str()))
            .collect();
        let field = canonical_crs_field(&pairs_ref);
        let contract = discover_input_contract_from_schema(schema_v1(vec![field]), resolve_crs)
            .expect("discovery");
        let ContractCrs::DeclaredUnresolved {
            crs_id,
            definition,
            definition_format,
        } = &contract.geometries[0].crs
        else {
            panic!(
                "atteso DeclaredUnresolved: {:?}",
                contract.geometries[0].crs
            );
        };
        assert_eq!(crs_id.as_deref(), Some("EPSG:4326"));
        assert_eq!(definition.as_deref(), Some(MONTE_MARIO_WKT));
        assert_eq!(
            definition_format.map(plenora_core::contract::CrsDefinitionFormat::as_str),
            Some("wkt")
        );
    }

    #[cfg(feature = "proj-backend")]
    #[test]
    fn discovery_resolved_with_same_code_but_different_authority_stays_unresolved() {
        let pairs = monte_mario_resolved_pairs("FOO:3003");
        let pairs_ref: Vec<(&str, &str)> = pairs
            .iter()
            .map(|(key, value)| (*key, value.as_str()))
            .collect();
        let field = canonical_crs_field(&pairs_ref);
        let contract = discover_input_contract_from_schema(schema_v1(vec![field]), resolve_crs)
            .expect("discovery");
        assert!(
            matches!(
                contract.geometries[0].crs,
                ContractCrs::DeclaredUnresolved { .. }
            ),
            "un'autorita' diversa non puo' essere certificata dal solo codice numerico"
        );
    }

    #[cfg(not(feature = "proj-backend"))]
    #[test]
    fn discovery_resolved_with_double_representation_needs_the_backend() {
        // Conseguenza DICHIARATA dell'emendamento 2026-07-31 (classe A):
        // senza `proj-backend` un input `resolved` con doppia
        // rappresentazione non degrada a `DeclaredUnresolved`. La
        // dichiarazione si onora con la regola (3), quindi la risoluzione
        // impossibile fallisce con errore `Crs` — coerente con quel che fa un
        // `resolved` a rappresentazione singola.
        let pairs = monte_mario_resolved_pairs("EPSG:3003");
        let pairs_ref: Vec<(&str, &str)> = pairs
            .iter()
            .map(|(key, value)| (*key, value.as_str()))
            .collect();
        let field = canonical_crs_field(&pairs_ref);
        let result = discover_input_contract_from_schema(schema_v1(vec![field]), resolve_crs);
        assert!(
            matches!(result, Err(PlenoraError::Crs(_))),
            "atteso errore Crs senza backend: {result:?}"
        );
    }

    #[cfg(feature = "proj-backend")]
    #[test]
    fn discovery_coherent_crs_id_and_srid_still_resolves() {
        // srid coerente con il codice dell'identificatore (come il caso
        // `multipolygon_xyzm_srid` del corpus): nessun conflitto, la
        // risoluzione avviene come sempre.
        let field = canonical_crs_field(&[
            (PLENORA_GEOMETRY_CRS_RESOLUTION_KEY, "resolved"),
            (PLENORA_GEOMETRY_CRS_ID_KEY, "EPSG:32632"),
            (PLENORA_GEOMETRY_AXIS_ORDER_KEY, "easting_northing"),
            (PLENORA_GEOMETRY_SRID_KEY, "32632"),
        ]);
        let contract = discover_input_contract_from_schema(schema_v1(vec![field]), resolve_crs)
            .expect("discovery");
        assert!(
            matches!(contract.geometries[0].crs, ContractCrs::Resolved(_)),
            "srid coerente -> risoluzione"
        );
    }

    // -------------------------------------------------------------------
    // Decisione esplicita nel piano (R4.6.3, campo v4 `crs_decisions`).
    // -------------------------------------------------------------------

    /// Contratto di input con geometria nello stato dato, per i test di
    /// `apply_crs_decisions` (schema con le chiavi canoniche CRS dichiarate
    /// e metadato legacy `geo.crs`).
    fn contract_with_crs_state(crs: ContractCrs) -> DataContract {
        let mut metadata = std::collections::HashMap::from([
            (PLENORA_GEOMETRY_ENCODING_KEY.to_owned(), "wkb".to_owned()),
            (
                PLENORA_GEOMETRY_CRS_RESOLUTION_KEY.to_owned(),
                crs.resolution().as_str().to_owned(),
            ),
            (
                GEO_METADATA_KEY.to_owned(),
                r#"{"crs":"EPSG:99999","encoding":"wkb"}"#.to_owned(),
            ),
        ]);
        if let ContractCrs::DeclaredUnresolved {
            crs_id: Some(id), ..
        } = &crs
        {
            metadata.insert(PLENORA_GEOMETRY_CRS_ID_KEY.to_owned(), id.clone());
            metadata.insert(
                PLENORA_GEOMETRY_AXIS_ORDER_KEY.to_owned(),
                "unknown".to_owned(),
            );
            metadata.insert(PLENORA_GEOMETRY_SRID_KEY.to_owned(), "99999".to_owned());
        }
        let field = Field::new("geometry", DataType::Binary, true).with_metadata(metadata);
        let geometry = GeometryColumnContract {
            field_id: FieldId(0),
            name: "geometry".to_owned(),
            crs,
            dimensions: GeometryDimensions::Xy,
            encoding: Some(GeometryEncoding::Wkb),
            nullable: true,
            types: GeometryColumnContract::undeclared_types(),
        };
        DataContract::new(
            schema_v1(vec![field]),
            vec![geometry],
            Some(FieldId(0)),
            ContractProperties::default(),
        )
        .expect("contratto fixture valido")
    }

    fn declared_unresolved_state() -> ContractCrs {
        ContractCrs::DeclaredUnresolved {
            crs_id: Some("EPSG:99999".to_owned()),
            definition: None,
            definition_format: None,
        }
    }

    fn decisions_probe(definition: &str) -> PlanInputsProbe {
        PlanInputsProbe {
            inputs: vec!["main".to_owned()],
            crs_decisions: std::collections::BTreeMap::from([(
                "main".to_owned(),
                definition.to_owned(),
            )]),
            limits: None,
        }
    }

    #[test]
    fn crs_decisions_on_unknown_input_is_an_error() {
        let mut contracts = vec![(
            "main".to_owned(),
            contract_with_crs_state(declared_unresolved_state()),
        )];
        let probe = PlanInputsProbe {
            inputs: vec!["main".to_owned()],
            crs_decisions: std::collections::BTreeMap::from([(
                "other".to_owned(),
                "EPSG:32632".to_owned(),
            )]),
            limits: None,
        };
        let error = apply_crs_decisions(&probe, &mut contracts).expect_err("input ignoto");
        assert!(error.to_string().contains("crs_decisions"), "{error}");
    }

    #[test]
    fn crs_decisions_on_missing_or_resolved_state_is_an_error() {
        // Una decisione su `missing` inventerebbe un CRS per un ingresso che
        // non ne dichiara (R4.4); su `resolved` e' una contraddizione del
        // piano. Mai ignorata in silenzio: errore esplicito in entrambi i
        // casi, prima di toccare il backend (il test vale senza PROJ).
        for state in [ContractCrs::Missing, ContractCrs::Resolved(projected_crs())] {
            let mut contracts = vec![("main".to_owned(), contract_with_crs_state(state))];
            let error = apply_crs_decisions(&decisions_probe("EPSG:32632"), &mut contracts)
                .expect_err("stato non decidibile");
            assert!(error.to_string().contains("non e' applicabile"), "{error}");
        }
    }

    #[cfg(feature = "proj-backend")]
    #[test]
    fn crs_decisions_resolve_declared_unresolved_keeping_the_schema_intact() {
        // La decisione esplicita del piano risolve l'incoerenza: il
        // contratto diventa `ResolvedByDecision` con la definizione decisa
        // (risolta contro PROJ). Lo schema di input NON e' toccato: il
        // check fail-closed dell'executor confronta i campi del file con il
        // contratto validato — la sostituzione delle dichiarazioni avviene
        // solo in emissione (strip nella fusione dello schema di output).
        let mut contracts = vec![(
            "main".to_owned(),
            contract_with_crs_state(declared_unresolved_state()),
        )];
        apply_crs_decisions(&decisions_probe("EPSG:32632"), &mut contracts).expect("decisione");
        let contract = &contracts[0].1;
        let ContractCrs::ResolvedByDecision(crs) = &contract.geometries[0].crs else {
            panic!(
                "atteso ResolvedByDecision: {:?}",
                contract.geometries[0].crs
            );
        };
        assert_eq!(crs.definition(), "EPSG:32632");
        assert_eq!(
            contract.geometries[0].crs.resolution(),
            CrsResolution::Resolved
        );
        let metadata = contract
            .schema
            .field_with_name("geometry")
            .expect("campo")
            .metadata();
        assert_eq!(
            metadata
                .get(PLENORA_GEOMETRY_CRS_RESOLUTION_KEY)
                .map(String::as_str),
            Some("declared_unresolved"),
            "lo schema di input resta quello scoperto"
        );
    }

    // -------------------------------------------------------------------
    // Tagging di fase al confine di lettura (BLOCK-03, piano-v5.md#contratti-di-input)
    // -------------------------------------------------------------------

    #[test]
    fn ipc_probes_tag_read_errors_at_the_input_boundary() {
        // File assente: Io dello sniffing -> fase Read; testo invariato.
        let missing = Path::new("input-che-non-esiste.arrow");
        let error = ipc_boundary::sniff_format(missing).expect_err("file assente");
        assert_eq!(error.phase(), ErrorPhase::Read);
        assert_eq!(error.phase_tag(), Some(ErrorPhase::Read));
        assert!(error.to_string().starts_with("io error: "), "{error}");
        // Stesso tag dal lato dell'header.
        let error = ipc_header_schema(missing).expect_err("file assente");
        assert_eq!(error.phase(), ErrorPhase::Read);

        // Header malformato: DataMapping nato leggendo la sorgente -> Read.
        let directory = tempfile::tempdir().expect("tempdir");
        let garbage = directory.path().join("garbage.arrow");
        std::fs::write(&garbage, b"non-e-un-flusso-ipc").expect("fixture");
        let error = ipc_header_schema(&garbage).expect_err("header malformato");
        assert_eq!(error.phase(), ErrorPhase::Read);
        // Diciannove byte di spazzatura: il file e' ROTTO, non troppo
        // grande. La lunghezza dichiarata dai primi quattro byte non e'
        // contenuta nel file, quindi il confine la tratta come troncamento
        // (`data_mapping`) e non come tetto superato (`resource_limit`).
        let senza_tag = error.untag();
        assert!(
            matches!(senza_tag, PlenoraError::DataMapping(_)),
            "{senza_tag:?}"
        );
    }

    #[test]
    fn discovery_contract_errors_keep_the_derived_validate_phase() {
        // Regressione: gli errori della discovery del contratto (coerenza
        // dei metadati, R2.6) NON sono taggati — restano validazione
        // derivata per variante. Il tagging copre solo la lettura fisica.
        let field = geometry_field(Some(r#"{"crs":"EPSG:32632","dimensions":"xy"}"#));
        let mut metadata = field.metadata().clone();
        metadata.insert(PLENORA_GEOMETRY_DIMENSIONS_KEY.to_owned(), "xyz".to_owned());
        let field = field.with_metadata(metadata);
        let error = discover_input_contract_from_schema(schema_v1(vec![field]), resolve_crs)
            .expect_err("divergenza R2.6");
        assert_eq!(error.phase(), ErrorPhase::Validate);
        assert_eq!(error.phase_tag(), None, "nessun tag: fase derivata");
    }

    // -------------------------------------------------------------------
    // Helper di presentazione e parsing argomenti
    // -------------------------------------------------------------------

    #[test]
    fn hex_digest_renders_every_byte_as_two_lowercase_hex_digits() {
        let mut digest = [0_u8; 32];
        digest[1] = 0x0f;
        digest[2] = 0xa5;
        digest[31] = 0xff;
        let hex = hex_digest(&digest);
        assert_eq!(hex.len(), 64);
        assert!(hex.starts_with("000fa5"), "{hex}");
        assert!(hex.ends_with("ff"), "{hex}");
        assert!(hex.bytes().all(|byte| byte.is_ascii_hexdigit()));
    }

    #[test]
    fn argument_value_requires_flag_and_value() {
        let args: Vec<String> = ["transform", "--input", "in.bin", "--schema", "s.json"]
            .iter()
            .map(ToString::to_string)
            .collect();
        assert_eq!(
            argument_value(&args, "--schema").expect("presente"),
            "s.json"
        );
        let missing = argument_value(&args, "--output").expect_err("flag assente");
        assert!(missing.to_string().contains("--output"), "{missing}");
        let dangling: Vec<String> = vec!["--input".to_string()];
        let error = argument_value(&dangling, "--input").expect_err("valore mancante");
        assert!(error.to_string().contains("--input"), "{error}");
    }

    #[test]
    fn at_input_prefixes_context_and_preserves_the_variant() {
        let path = Path::new("dati.arrow");
        let prefixed = at_input("main", path, PlenoraError::InvalidPlan("boom".into()));
        match prefixed {
            PlenoraError::InvalidPlan(message) => {
                assert_eq!(message, "input `main` (dati.arrow): boom");
            }
            other => panic!("variante non preservata: {other:?}"),
        }
        for make in [
            PlenoraError::Unsupported as fn(String) -> PlenoraError,
            PlenoraError::Schema,
            PlenoraError::Crs,
        ] {
            let prefixed = at_input("main", path, make("boom".to_owned()));
            let message = match &prefixed {
                PlenoraError::Unsupported(message)
                | PlenoraError::Schema(message)
                | PlenoraError::Crs(message) => message,
                other => panic!("variante non preservata: {other:?}"),
            };
            assert_eq!(message, "input `main` (dati.arrow): boom");
        }
        // Le altre varianti passano inalterate (testo e tipo).
        let io = at_input(
            "main",
            path,
            PlenoraError::Io(std::io::Error::other("disco")),
        );
        assert!(matches!(io, PlenoraError::Io(_)));
        assert_eq!(io.to_string(), "io error: disco");
    }

    // -------------------------------------------------------------------
    // Definizione CRS dalle rappresentazioni accettate (R4.x)
    // -------------------------------------------------------------------

    #[test]
    fn crs_definition_from_metadata_accepts_objects_and_rejects_other_types() {
        // PROJJSON come oggetto: serializzato compatto, mai perso (chiavi in
        // ordine canonico: `serde_json::Value` le riordina alfabeticamente).
        let object = r#"{"crs":{"type":"GeographicCRS","name":"WGS 84"}}"#.to_owned();
        let definition =
            crs_definition_from_metadata("geometry", Some(&object)).expect("oggetto PROJJSON");
        assert_eq!(
            definition.as_deref(),
            Some(r#"{"name":"WGS 84","type":"GeographicCRS"}"#)
        );
        // Tipo non stringa/oggetto: metadato malformato -> errore (R5.1).
        let invalid = r#"{"crs":32632}"#.to_owned();
        let result = crs_definition_from_metadata("geometry", Some(&invalid));
        assert!(
            matches!(result, Err(PlenoraError::InvalidPlan(_))),
            "{result:?}"
        );
        // Senza chiave `crs` e senza metadato: assenza, non errore (R4.6.3).
        let bare = "{}".to_owned();
        assert_eq!(
            crs_definition_from_metadata("geometry", Some(&bare)).expect("nessun crs"),
            None
        );
        assert_eq!(
            crs_definition_from_metadata("geometry", None).expect("nessun metadato"),
            None
        );
    }

    #[test]
    fn contract_crs_from_keys_co_presence_is_declared_unresolved_not_a_choice() {
        // `crs_id` + `crs_definition` co-presenti: l'accordo non e'
        // decidibile testualmente (R2.7: mai arbitrato), quindi nessuna
        // precedenza silenziosa fra i due. Lo stato e' `DeclaredUnresolved`
        // con entrambe le dichiarazioni.
        let keys = CanonicalGeometryKeys {
            crs_definition: Some(r#"{"type":"ProjectedCRS"}"#.to_owned()),
            crs_id: Some("EPSG:32632".to_owned()),
            ..CanonicalGeometryKeys::default()
        };
        let ContractCrs::DeclaredUnresolved {
            crs_id, definition, ..
        } = contract_crs_from_keys("geometry", None, &keys, resolve_crs).expect("stato")
        else {
            panic!("atteso DeclaredUnresolved");
        };
        assert_eq!(crs_id.as_deref(), Some("EPSG:32632"));
        assert_eq!(definition.as_deref(), Some(r#"{"type":"ProjectedCRS"}"#));
    }

    #[test]
    fn contract_crs_from_keys_srid_only_declared_unresolved_is_a_representation() {
        // Catena MySQL TLS Database→Data: il provider dichiara
        // `declared_unresolved` con solo `srid` (R4.4). Lo SRID e' la terza
        // rappresentazione CRS (R4.3.1), quindi non e' la contraddizione R4.1:
        // `DeclaredUnresolved` con crs_id/definition/format assenti, mai
        // sintetizzati.
        let keys = CanonicalGeometryKeys {
            srid: Some(4326),
            crs_resolution: Some(CrsResolution::DeclaredUnresolved),
            ..CanonicalGeometryKeys::default()
        };
        let ContractCrs::DeclaredUnresolved {
            crs_id,
            definition,
            definition_format,
        } = contract_crs_from_keys("geometry", None, &keys, resolve_crs).expect("stato")
        else {
            panic!("atteso DeclaredUnresolved");
        };
        assert_eq!(crs_id, None, "crs_id mai sintetizzato");
        assert_eq!(definition, None, "definizione mai sintetizzata");
        assert_eq!(definition_format, None, "formato mai sintetizzato");
    }

    #[test]
    fn contract_crs_from_keys_declared_unresolved_without_any_representation_is_an_error() {
        // Fail-closed (R4.1): `declared_unresolved` senza crs_id, definition
        // E srid resta una contraddizione — errore esplicito, mai collasso
        // su `missing`.
        let keys = CanonicalGeometryKeys {
            crs_resolution: Some(CrsResolution::DeclaredUnresolved),
            ..CanonicalGeometryKeys::default()
        };
        let result = contract_crs_from_keys("geometry", None, &keys, resolve_crs);
        match result {
            Err(PlenoraError::InvalidPlan(message)) => {
                assert!(
                    message.contains("nessun CRS e' dichiarato in alcuna rappresentazione"),
                    "{message}"
                );
            }
            other => panic!("attesa contraddizione R4.1, ottenuto {other:?}"),
        }
    }

    #[test]
    fn contract_crs_from_keys_resolved_with_srid_only_is_never_promoted() {
        // Fail-closed: lo SRID numerico da solo non identifica un'autorita'
        // risolvibile e il centro non la inventa (R4.4) — un `resolved`
        // dichiarato con SOLO `srid` non e' promosso ne' risolto
        // implicitamente: resta la contraddizione R4.1 di sempre (errore).
        let keys = CanonicalGeometryKeys {
            srid: Some(4326),
            crs_resolution: Some(CrsResolution::Resolved),
            ..CanonicalGeometryKeys::default()
        };
        let result = contract_crs_from_keys("geometry", None, &keys, resolve_crs);
        assert!(
            matches!(result, Err(PlenoraError::InvalidPlan(_))),
            "`resolved` srid-only non promosso: {result:?}"
        );
    }

    #[test]
    fn contract_crs_from_keys_legacy_fallback_feeds_the_resolution() {
        // Nessuna forma canonica: il legacy `geo.crs` alimenta la
        // risoluzione (con backend -> Resolved; senza -> errore `Crs` di
        // backend, mai `Missing` inventato).
        let legacy = r#"{"crs":"EPSG:32632"}"#.to_owned();
        let keys = CanonicalGeometryKeys::default();
        let result = contract_crs_from_keys("geometry", Some(&legacy), &keys, resolve_crs);
        #[cfg(feature = "proj-backend")]
        assert!(matches!(result, Ok(ContractCrs::Resolved(_))), "{result:?}");
        #[cfg(not(feature = "proj-backend"))]
        assert!(matches!(result, Err(PlenoraError::Crs(_))), "{result:?}");
        // Nessuna rappresentazione: `Missing`, mai errore (R4.6.3).
        let missing =
            contract_crs_from_keys("geometry", None, &keys, resolve_crs).expect("assente");
        assert!(matches!(missing, ContractCrs::Missing));
    }

    #[test]
    fn discovery_rejects_incoherent_geometry_metadata() {
        // Estensione diversa da `geoarrow.wkb`: rifiuto esplicito.
        let unknown_extension = Field::new("geometry", DataType::Binary, true).with_metadata(
            std::collections::HashMap::from([(
                GEOARROW_EXTENSION_KEY.to_owned(),
                "geoarrow.point".to_owned(),
            )]),
        );
        let result = discover_input_contract_from_schema(
            std::sync::Arc::new(Schema::new(vec![unknown_extension])),
            resolve_crs,
        );
        match result {
            Err(PlenoraError::InvalidPlan(message)) => {
                assert!(message.contains("non supportata"), "{message}");
            }
            other => panic!("atteso rifiuto estensione, ottenuto {other:?}"),
        }
        // Metadato `geo` senza estensione: metadati incoerenti.
        let orphan = Field::new("geometry", DataType::Binary, true).with_metadata(
            std::collections::HashMap::from([(GEO_METADATA_KEY.to_owned(), "{}".to_owned())]),
        );
        let result = discover_input_contract_from_schema(
            std::sync::Arc::new(Schema::new(vec![orphan])),
            resolve_crs,
        );
        match result {
            Err(PlenoraError::InvalidPlan(message)) => {
                assert!(message.contains("incoerenti"), "{message}");
            }
            other => panic!("attesi metadati incoerenti, ottenuto {other:?}"),
        }
    }

    #[test]
    fn canonical_types_enter_the_contract_as_declared_with_schema_scope() {
        // Variante proj-indipendente del riconoscimento canonico: la coppia
        // types_declaration/types entra nel contratto come Declared/Schema.
        let contract =
            contract_from_field(&canonical_geometry_field(DataType::Binary)).expect("lettura");
        assert!(
            matches!(contract.types.confidence, PropertyConfidence::Declared(_)),
            "types dichiarati dalla coppia canonica"
        );
        assert_eq!(contract.types.scope, PropertyScope::Schema);
    }

    #[test]
    fn contract_json_renders_a_resolved_crs_with_kind_and_resolution() {
        let schema = std::sync::Arc::new(Schema::new(vec![Field::new(
            "geometry",
            DataType::Binary,
            true,
        )]));
        let contract = DataContract::new(
            schema,
            vec![GeometryColumnContract {
                field_id: FieldId(0),
                name: "geometry".to_owned(),
                crs: ContractCrs::Resolved(projected_crs()),
                dimensions: GeometryDimensions::Xy,
                encoding: None,
                nullable: true,
                types: GeometryColumnContract::undeclared_types(),
            }],
            Some(FieldId(0)),
            ContractProperties::default(),
        )
        .expect("contratto");
        let summary = contract_json(&contract);
        assert_eq!(summary["geometry"]["crs"], "EPSG:32632");
        assert_eq!(summary["geometry"]["crs_kind"], "Projected");
        assert_eq!(summary["geometry"]["crs_resolution"], "resolved");
    }

    #[test]
    fn contract_json_renders_a_missing_crs_as_null() {
        let schema = std::sync::Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int64, false),
            Field::new("geometry", DataType::Binary, true),
        ]));
        let contract = DataContract::new(
            schema,
            vec![GeometryColumnContract {
                field_id: FieldId(0),
                name: "geometry".to_owned(),
                crs: ContractCrs::Missing,
                dimensions: GeometryDimensions::Unknown,
                encoding: None,
                nullable: true,
                types: GeometryColumnContract::undeclared_types(),
            }],
            Some(FieldId(0)),
            ContractProperties::default(),
        )
        .expect("contratto");
        let summary = contract_json(&contract);
        assert_eq!(summary["geometry"]["crs"], serde_json::Value::Null);
        assert_eq!(summary["geometry"]["crs_kind"], serde_json::Value::Null);
        assert_eq!(summary["geometry"]["crs_resolution"], "missing");
        assert_eq!(summary["fields"][0]["name"], "id");
        assert_eq!(summary["fields"][0]["data_type"], "Int64");
        assert_eq!(summary["fields"][0]["nullable"], false);
        assert_eq!(summary["fields"][1]["nullable"], true);
    }

    // -------------------------------------------------------------------
    // Sniffing del framing IPC e input lazy
    // -------------------------------------------------------------------

    #[test]
    fn ipc_sniffing_treats_short_and_non_magic_files_as_streams() {
        let directory = tempfile::tempdir().expect("tempdir");
        // Piu' corto del magic: lettura parziale, nessun errore, non-file.
        let short = directory.path().join("short.bin");
        std::fs::write(&short, b"ARR").expect("fixture");
        assert_eq!(
            ipc_boundary::sniff_format(&short).expect("sniffing"),
            IpcFormat::Stream
        );
        // Sei byte ma magic diverso: non e' IPC file format.
        let other = directory.path().join("other.bin");
        std::fs::write(&other, b"ARROW2").expect("fixture");
        assert_eq!(
            ipc_boundary::sniff_format(&other).expect("sniffing"),
            IpcFormat::Stream
        );
    }

    #[test]
    fn open_input_accepts_file_and_stream_ipc_framings() {
        let directory = tempfile::tempdir().expect("tempdir");
        let schema: SchemaRef =
            std::sync::Arc::new(Schema::new(vec![Field::new("id", DataType::Int64, false)]));
        let batch = RecordBatch::try_new(
            std::sync::Arc::clone(&schema),
            vec![std::sync::Arc::new(
                plenora_core::arrow::array::Int64Array::from(vec![1, 2]),
            )],
        )
        .expect("batch");
        // IPC file format.
        let file_path = directory.path().join("in.arrow");
        let mut writer = FileWriter::try_new(File::create(&file_path).expect("create"), &schema)
            .expect("writer");
        writer.write(&batch).expect("write");
        writer.finish().expect("finish");
        assert_eq!(
            ipc_boundary::sniff_format(&file_path).expect("sniff"),
            IpcFormat::File
        );
        // IPC stream format.
        let stream_path = directory.path().join("in.stream");
        let mut writer = plenora_core::arrow::ipc::writer::StreamWriter::try_new(
            File::create(&stream_path).expect("create"),
            &schema,
        )
        .expect("writer");
        writer.write(&batch).expect("write");
        writer.finish().expect("finish");
        assert_eq!(
            ipc_boundary::sniff_format(&stream_path).expect("sniff"),
            IpcFormat::Stream
        );
        // Entrambi si aprono come input lazy con lo schema dichiarato.
        for path in [&file_path, &stream_path] {
            let input = open_input(path, &IpcLimits::default()).expect("open_input");
            match input {
                Input::Stream {
                    schema: declared, ..
                } => assert_eq!(declared, schema),
                Input::Batches { .. } => panic!("gli input da percorso sono lazy"),
            }
        }
    }

    #[test]
    fn v4_input_paths_combines_single_and_multiple_flags() {
        let argv =
            |args: &[&str]| -> Vec<String> { args.iter().map(ToString::to_string).collect() };
        let posizionali = |args: &[&str]| match v4_inputs(&argv(args)).expect("inputs") {
            DagInputs::Positional(paths) => paths,
            DagInputs::Named(_) => panic!("attesa forma posizionale"),
        };
        assert_eq!(
            posizionali(&["run", "--input", "a.arrow"]),
            vec![PathBuf::from("a.arrow")]
        );
        assert_eq!(
            posizionali(&["run", "--inputs", "b.arrow", "c.arrow", "--output", "o.arrow"]),
            vec![PathBuf::from("b.arrow"), PathBuf::from("c.arrow")],
            "i valori si fermano al prossimo flag"
        );
        assert_eq!(
            posizionali(&["run", "--input", "a.arrow", "--inputs", "b.arrow"]).len(),
            2
        );
        assert!(v4_inputs(&argv(&["--input"])).is_err());
    }

    #[test]
    fn la_forma_nominale_lega_ogni_input_al_suo_nome() {
        let argv =
            |args: &[&str]| -> Vec<String> { args.iter().map(ToString::to_string).collect() };
        let inputs = v4_inputs(&argv(&[
            "run",
            "--input",
            "destra=b.arrow",
            "--input",
            "sinistra=a.arrow",
            "--output",
            "o.arrow",
        ]))
        .expect("inputs");
        assert_eq!(
            inputs,
            DagInputs::Named(vec![
                ("destra".to_owned(), PathBuf::from("b.arrow")),
                ("sinistra".to_owned(), PathBuf::from("a.arrow")),
            ])
        );

        // L'ordine restituito e' quello del PIANO, non della riga di comando:
        // e' il punto del difetto — con la forma posizionale questi due file
        // sarebbero finiti sugli input sbagliati.
        let probe = PlanInputsProbe {
            inputs: vec!["sinistra".to_owned(), "destra".to_owned()],
            crs_decisions: std::collections::BTreeMap::new(),
            limits: None,
        };
        assert_eq!(
            pair_v4_inputs(&probe, &inputs).expect("accoppiamento"),
            vec![
                ("sinistra".to_owned(), PathBuf::from("a.arrow")),
                ("destra".to_owned(), PathBuf::from("b.arrow")),
            ]
        );

        // Un nome che il piano non dichiara e' un errore, e il messaggio dice
        // quali sono i nomi buoni.
        let sconosciuto = v4_inputs(&argv(&["run", "--input", "centro=c.arrow"])).expect("inputs");
        let error = pair_v4_inputs(&probe, &sconosciuto).expect_err("nome non dichiarato");
        assert!(
            error.to_string().contains("non dichiarato dal piano"),
            "{error}"
        );
        assert!(error.to_string().contains("sinistra, destra"), "{error}");

        // Un input dichiarato ma non fornito e' un errore, con il rimedio.
        let parziale = v4_inputs(&argv(&["run", "--input", "sinistra=a.arrow"])).expect("inputs");
        let error = pair_v4_inputs(&probe, &parziale).expect_err("input mancante");
        assert!(
            error.to_string().contains("--input destra=PERCORSO"),
            "{error}"
        );

        // Nome ripetuto, percorso vuoto, forme mescolate: tutti errori.
        assert!(v4_inputs(&argv(&[
            "run",
            "--input",
            "sinistra=a.arrow",
            "--input",
            "sinistra=b.arrow"
        ]))
        .is_err());
        assert!(v4_inputs(&argv(&["run", "--input", "sinistra="])).is_err());
        assert!(v4_inputs(&argv(&[
            "run",
            "--input",
            "sinistra=a.arrow",
            "--inputs",
            "b.arrow"
        ]))
        .is_err());
    }

    #[test]
    fn un_percorso_non_e_scambiato_per_una_coppia_nominale() {
        // Un percorso assoluto o relativo non contiene `=` prima di un
        // separatore: resta un percorso. La regola e' quella documentata su
        // `is_named_input`, e vale su entrambi gli stili di separatore.
        assert!(!is_named_input("/dati/x.arrow"));
        assert!(!is_named_input(r"C:\dati\x.arrow"));
        assert!(!is_named_input("x.arrow"));
        assert!(!is_named_input("=x.arrow"));
        assert!(!is_named_input("/dati/a=b.arrow"));
        assert!(is_named_input("main=x.arrow"));
        assert!(is_named_input("main=/dati/x.arrow"));
        assert!(is_named_input(r"main=C:\dati\x.arrow"));
    }

    // -------------------------------------------------------------------
    // Trasporto WKB v2: transform_stream e read_geometry_stream
    // (i comandi che li invocano richiedono la risoluzione CRS — feature
    // `proj-backend`; il framing e la trasformazione no)
    // -------------------------------------------------------------------

    /// POINT (2 3), little-endian OGC WKB.
    const POINT_WKB: [u8; 21] = [
        1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 64, 0, 0, 0, 0, 0, 0, 8, 64,
    ];

    /// Scrive `frames` (con null) in framing WKB v2 e restituisce i byte.
    fn framed_v2(frames: &[Option<&[u8]>]) -> Vec<u8> {
        let mut writer = FrameWriter::new(Vec::new(), frames.len() as u64).expect("writer");
        for frame in frames {
            writer.write_frame(*frame).expect("frame");
        }
        writer.finish().expect("finish").0
    }

    fn centroid_schema(row_count: u64) -> TransformSchema {
        TransformSchema {
            schema_version: 2,
            operation: Operation::Centroid,
            row_count,
            crs: None,
        }
    }

    #[test]
    fn transform_stream_rechecks_the_schema_version_fail_closed() {
        // Difesa in profondita': `execute_transform` verifica la versione
        // prima della validazione CRS, ma il motore di stream non si fida
        // del chiamante.
        let input = framed_v2(&[Some(&POINT_WKB)]);
        let mut output = Vec::new();
        let mut schema = centroid_schema(1);
        schema.schema_version = 3;
        let result = transform_stream(&mut input.as_slice(), &mut output, &schema);
        let error = result.expect_err("versione non supportata");
        assert!(error.to_string().contains("schema_version"), "{error}");
    }

    #[test]
    fn transform_stream_transforms_frames_and_preserves_nulls() {
        let input = framed_v2(&[Some(&POINT_WKB), None, Some(&POINT_WKB)]);
        let mut output = Vec::new();
        let summary = transform_stream(&mut input.as_slice(), &mut output, &centroid_schema(3))
            .expect("transform");
        assert_eq!(summary.rows, 3);
        // Il checksum e' quello dello stream prodotto: rileggendolo i frame
        // sono coerenti (il reader verifica il footer).
        let mut reader = FrameReader::new(output.as_slice(), 3).expect("reader");
        let first = reader.next_frame().expect("frame").expect("riga 1");
        let Frame::Wkb(payload) = first else {
            panic!("atteso WKB");
        };
        // Il centroide di un punto e' il punto stesso.
        assert_eq!(payload.as_slice(), &POINT_WKB);
        assert!(
            matches!(reader.next_frame().expect("frame"), Some(Frame::Null)),
            "il null e' preservato in posizione"
        );
        assert!(reader.next_frame().expect("frame").is_some());
        assert!(reader.next_frame().expect("fine").is_none());
    }

    #[test]
    fn transform_stream_rejects_a_short_stream_against_row_count() {
        // Il flusso dichiara 3 righe ma ne arrivano 2: errore esplicito, mai
        // un output pubblicato come completo.
        let input = framed_v2(&[Some(&POINT_WKB), Some(&POINT_WKB)]);
        let mut output = Vec::new();
        let result = transform_stream(&mut input.as_slice(), &mut output, &centroid_schema(3));
        let error = result.expect_err("row_count non coerente");
        assert!(
            error.to_string().contains("row_count non coerente"),
            "{error}"
        );
    }

    #[test]
    fn transform_stream_fails_closed_on_undecodable_wkb() {
        // Payload WKB corrotto: la trasformazione fallisce, nessun frame
        // inventato attraversa il confine.
        let garbage = [0x01, 0x01, 0x00, 0xFF];
        let input = framed_v2(&[Some(&garbage)]);
        let mut output = Vec::new();
        let result = transform_stream(&mut input.as_slice(), &mut output, &centroid_schema(1));
        assert!(result.is_err(), "WKB corrotto deve fallire");
    }

    #[test]
    fn transform_stream_flushes_full_batches_mid_stream() {
        // Oltre BATCH_ROWS (4096) righe: il flush intermedio e' esercitato e
        // il conteggio finale resta esatto.
        const ROWS: u64 = 4097;
        let frames: Vec<Option<&[u8]>> = (0..ROWS).map(|_| Some(&POINT_WKB[..])).collect();
        let input = framed_v2(&frames);
        let mut output = Vec::new();
        let summary = transform_stream(&mut input.as_slice(), &mut output, &centroid_schema(ROWS))
            .expect("transform");
        assert_eq!(summary.rows, ROWS);
        let mut reader = FrameReader::new(output.as_slice(), ROWS).expect("reader");
        let mut count = 0_u64;
        while reader.next_frame().expect("frame").is_some() {
            count += 1;
        }
        assert_eq!(count, ROWS);
    }

    #[test]
    fn read_geometry_stream_decodes_payloads_and_nulls() {
        let directory = tempfile::tempdir().expect("tempdir");
        let path = directory.path().join("geometries.bin");
        std::fs::write(&path, framed_v2(&[Some(&POINT_WKB), None])).expect("fixture");
        let geometries = read_geometry_stream(&path, 2).expect("lettura");
        assert_eq!(geometries.len(), 2);
        match &geometries[0] {
            Some(geo::Geometry::Point(point)) => {
                assert!((point.x() - 2.0).abs() < f64::EPSILON);
                assert!((point.y() - 3.0).abs() < f64::EPSILON);
            }
            other => panic!("atteso Point, ottenuto {other:?}"),
        }
        assert!(geometries[1].is_none(), "null preservato");
        // Payload non decodificabile: errore, mai geometria inventata.
        let garbage = [0xDE, 0xAD];
        let bad_path = directory.path().join("garbage.bin");
        std::fs::write(&bad_path, framed_v2(&[Some(&garbage)])).expect("fixture");
        assert!(read_geometry_stream(&bad_path, 1).is_err());
    }
}
