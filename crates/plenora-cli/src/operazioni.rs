//! La tabella delle operazioni pubbliche: l'unica fonte di comandi CLI,
//! aiuto, Capability Discovery 2.0 (della CLI e dell'SDK Python) e mappe
//! degli export Rust e dei simboli Python.
//!
//! Le operazioni sono le quattro del catalogo pubblico di `plenora-contracts`
//! (`catalogs/data-tools-v1.json`): `data.catalog`, `data.describe`,
//! `data.validate`, `data.run`. Identificatori, versioni, contratti e tipi di
//! contenuto vengono da lì; dove questo artefatto se ne scosta lo dice la
//! voce stessa (attributi) e il README («CLI `plenora-data`», deviazioni).
//!
//! I kernel che un piano può usare non sono comandi: stanno nel registro
//! che `data.catalog` restituisce ([`crate::catalogo`]), derivato dal
//! catalogo di `plenora-core`.

/// `application/json`.
pub const JSON: &str = "application/json";
/// Arrow IPC, formato stream.
pub const ARROW_STREAM: &str = "application/vnd.apache.arrow.stream";
/// Arrow IPC, formato file.
pub const ARROW_FILE: &str = "application/vnd.apache.arrow.file";
/// Parquet: estensione di questo artefatto, fuori dai tipi del catalogo
/// pubblico (dichiarata negli attributi, mai in `content_types`).
pub const PARQUET: &str = "application/vnd.apache.parquet";

/// Contratto degli attributi tipizzati delle operazioni (CAP-013): i campi
/// sono descritti nel README («CLI `plenora-data`», «Attributi»).
pub const CONTRATTO_ATTRIBUTI: &str = "plenora-data-capability-attributes-v1";
/// Il registro dei kernel restituito da `data.catalog`.
pub const REGISTRO_KERNEL: &str = "plenora-data-kernel-catalog-v1";

/// Classe d'effetto collaterale pubblica (Public Surfaces 1.0, SURF-006).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effetto {
    /// Nessun effetto osservabile fuori dal processo.
    Nessuno,
    /// Effetti locali (file scritti), mai remoti.
    Locale,
}

impl Effetto {
    /// Il nome dello schema `capabilities-v2` (`side_effect`).
    #[must_use]
    pub const fn come_testo(self) -> &'static str {
        match self {
            Self::Nessuno => "none",
            Self::Locale => "local",
        }
    }
}

/// Un'operazione pubblica e le sue spellature.
// I booleani sono campi indipendenti del descrittore pubblico (controlli e
// dichiarazioni di `capabilities-v2`), non stati di una macchina.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Copy)]
pub struct OperazionePubblica {
    /// Identificatore stabile (`data.*`).
    pub id: &'static str,
    /// Versione dell'operazione.
    pub versione: u32,
    /// Comando CLI canonico (`bindings/cli-v1.json`).
    pub comando: &'static str,
    /// Sintassi del comando per l'aiuto, senza `--format json`.
    pub sintassi: &'static str,
    /// Riassunto di una riga per l'aiuto.
    pub riassunto: &'static str,
    /// Contratto d'ingresso.
    pub ingresso: &'static str,
    /// Tipi di contenuto accettati, come nel catalogo pubblico.
    pub tipi_ingresso: &'static [&'static str],
    /// Contratto d'uscita: anche il `contract` dell'inviluppo del comando.
    pub uscita: &'static str,
    /// Tipi di contenuto prodotti, come nel catalogo pubblico.
    pub tipi_uscita: &'static [&'static str],
    /// Effetto collaterale di questo artefatto.
    pub effetto: Effetto,
    /// Annullamento cooperativo accettato.
    pub annullamento: bool,
    /// Scadenza accettata (`--deadline`, `--timeout-ms`).
    pub scadenza: bool,
    /// Tipi di contenuto in più rispetto al catalogo, in ingresso e in
    /// uscita (Parquet): solo negli attributi.
    pub estensioni_ingresso: &'static [&'static str],
    pub estensioni_uscita: &'static [&'static str],
    /// L'operazione materializza le tabelle intere (ARROW-011: dichiarato).
    pub materializzazione_limitata: bool,
    /// L'operazione usa il registro dei kernel.
    pub usa_registro: bool,
    /// Gli export Rust pubblici che la implementano (mappa della superficie
    /// Rust, Surface Bindings 1.0, sezione 2).
    pub export_rust: &'static [&'static str],
    /// I simboli dell'SDK Python `plenora_data` che la chiamano, sincroni e
    /// asincroni con la stessa semantica (Surface Bindings 1.0, sezione 4;
    /// Python SDK 1.0, sezioni 4 e 12).
    pub export_python: &'static [&'static str],
}

/// Le operazioni pubbliche dell'artefatto, nell'ordine del catalogo.
pub const OPERAZIONI: &[OperazionePubblica] = &[
    OperazionePubblica {
        id: "data.catalog",
        versione: 1,
        comando: "catalog",
        sintassi: "catalog",
        riassunto: "registro dei kernel che un piano puo' usare",
        ingresso: "plenora-data-catalog-query-v1",
        tipi_ingresso: &[JSON],
        uscita: "plenora-data-kernel-catalog-v1",
        tipi_uscita: &[JSON],
        effetto: Effetto::Nessuno,
        annullamento: false,
        scadenza: false,
        estensioni_ingresso: &[],
        estensioni_uscita: &[],
        materializzazione_limitata: false,
        usa_registro: true,
        export_rust: &["plenora_cli::api::catalogo"],
        export_python: &["plenora_data.catalog", "plenora_data.acatalog"],
    },
    OperazionePubblica {
        id: "data.describe",
        versione: 1,
        comando: "describe",
        sintassi: "describe --input INPUT.arrow [--deadline RFC3339 | --timeout-ms MS]",
        riassunto: "descrive una tabella Arrow (o Parquet) senza modificarla",
        ingresso: "plenora-data-describe-input-v1",
        tipi_ingresso: &[ARROW_STREAM, ARROW_FILE],
        uscita: "plenora-data-description-v1",
        tipi_uscita: &[JSON],
        effetto: Effetto::Nessuno,
        annullamento: true,
        scadenza: true,
        estensioni_ingresso: &[PARQUET],
        estensioni_uscita: &[],
        materializzazione_limitata: true,
        usa_registro: false,
        export_rust: &[
            "plenora_cli::api::descrivi",
            "plenora_cli::api::descrivi_tabella",
        ],
        export_python: &["plenora_data.describe", "plenora_data.adescribe"],
    },
    OperazionePubblica {
        id: "data.validate",
        versione: 1,
        comando: "validate",
        sintassi: "validate --plan PLAN.json [--input NAME=INPUT.arrow]... \
                   [--deadline RFC3339 | --timeout-ms MS]",
        riassunto: "valida un piano contro gli schemi degli input, senza eseguirlo",
        ingresso: "plenora-data-plan-validation-input-v1",
        tipi_ingresso: &[JSON, ARROW_STREAM, ARROW_FILE],
        uscita: "plenora-data-plan-validation-result-v1",
        tipi_uscita: &[JSON],
        effetto: Effetto::Nessuno,
        annullamento: true,
        scadenza: true,
        estensioni_ingresso: &[PARQUET],
        estensioni_uscita: &[],
        materializzazione_limitata: true,
        usa_registro: true,
        export_rust: &[
            "plenora_cli::api::valida",
            "plenora_cli::api::valida_ingressi",
        ],
        export_python: &["plenora_data.validate", "plenora_data.avalidate"],
    },
    OperazionePubblica {
        id: "data.run",
        versione: 1,
        comando: "run",
        sintassi: "run --plan PLAN.json [--input NAME=INPUT.arrow]... \
                   --output [NAME=]OUTPUT.arrow... [--overwrite] \
                   [--deadline RFC3339 | --timeout-ms MS]",
        riassunto: "esegue un piano e scrive i suoi output (.arrow file, .arrows stream, .parquet)",
        ingresso: "plenora-data-execution-input-v1",
        tipi_ingresso: &[JSON, ARROW_STREAM, ARROW_FILE],
        uscita: "plenora-data-execution-result-v1",
        tipi_uscita: &[ARROW_STREAM, ARROW_FILE],
        effetto: Effetto::Locale,
        annullamento: true,
        scadenza: true,
        estensioni_ingresso: &[PARQUET],
        estensioni_uscita: &[PARQUET],
        materializzazione_limitata: true,
        usa_registro: true,
        export_rust: &[
            "plenora_cli::api::esegui",
            "plenora_cli::api::esegui_ingressi",
            "plenora_cli::api::esegui_in_memoria",
        ],
        export_python: &["plenora_data.run", "plenora_data.arun"],
    },
];

/// L'operazione del comando CLI canonico.
#[must_use]
pub fn per_comando(comando: &str) -> Option<&'static OperazionePubblica> {
    OPERAZIONI
        .iter()
        .find(|operazione| operazione.comando == comando)
}

/// L'operazione dal suo identificatore.
#[must_use]
pub fn per_id(id: &str) -> Option<&'static OperazionePubblica> {
    OPERAZIONI.iter().find(|operazione| operazione.id == id)
}
