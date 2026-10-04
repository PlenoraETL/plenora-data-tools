//! La tabella delle operazioni pubbliche: l'unica fonte di comandi CLI,
//! aiuto, Capability Discovery 2.0 (della CLI e dell'SDK Python) e mappe
//! degli export Rust e dei simboli Python.
//!
//! Le operazioni sono le cinque del catalogo pubblico di `plenora-contracts`
//! (`catalogs/data-tools-v2.json`, profilo `plenora-data-tools-profile-v2`):
//! `data.catalog` 2, `data.describe` 1, `data.validate` 2, `data.run` 2 e
//! `data.run` 3. Quest'ultima sta solo sulla superficie Rust (e sul runtime,
//! che l'applicazione costruisce sopra): niente comando CLI, niente simbolo
//! Python, niente voce nei documenti delle capacità della CLI e dell'SDK.
//! Identificatori, versioni, contratti e tipi di contenuto vengono da lì; gli
//! attributi dicono ciò che i campi comuni non dicono.
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
/// sono descritti in docs/cli.md («CLI `plenora-data`», «Attributi»).
pub const CONTRATTO_ATTRIBUTI: &str = "plenora-data-capability-attributes-v1";
/// Il registro dei kernel restituito da `data.catalog`.
pub const REGISTRO_KERNEL: &str = "plenora-data-kernel-catalog-v2";

/// Classe d'effetto collaterale pubblica (Public Surfaces 1.0, SURF-006).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effetto {
    /// Nessun effetto osservabile fuori dal processo.
    Nessuno,
    /// Effetti locali (file scritti), mai remoti.
    Locale,
    /// Effetti che possono essere remoti (artefatti pubblicati da un
    /// risolutore dell'applicazione): la classe prudente.
    Remoto,
}

impl Effetto {
    /// Il nome dello schema `capabilities-v2` (`side_effect`).
    #[must_use]
    pub const fn come_testo(self) -> &'static str {
        match self {
            Self::Nessuno => "none",
            Self::Locale => "local",
            Self::Remoto => "remote",
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
    /// L'operazione legge un piano `plenora-data-plan-v1` (attributo
    /// `plan_contract`).
    pub usa_piano: bool,
    /// Solo sulla superficie Rust: niente comando CLI, niente simbolo
    /// Python, assente dai documenti delle capacità della CLI e dell'SDK.
    pub solo_rust: bool,
}

/// Le operazioni pubbliche dell'artefatto, nell'ordine del catalogo.
pub const OPERAZIONI: &[OperazionePubblica] = &[
    OperazionePubblica {
        id: "data.catalog",
        versione: 2,
        comando: "catalog",
        sintassi: "catalog",
        riassunto: "registro dei kernel che un piano puo' usare",
        ingresso: "plenora-data-catalog-query-v1",
        tipi_ingresso: &[JSON],
        uscita: "plenora-data-catalog-result-v2",
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
        usa_piano: false,
        solo_rust: false,
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
        usa_piano: false,
        solo_rust: false,
    },
    OperazionePubblica {
        id: "data.validate",
        versione: 2,
        comando: "validate",
        sintassi: "validate --plan PLAN.json [--input NAME=INPUT.arrow]... \
                   [--deadline RFC3339 | --timeout-ms MS]",
        riassunto: "valida un piano contro gli schemi degli input, senza eseguirlo",
        ingresso: "plenora-data-plan-validation-input-v2",
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
        usa_piano: true,
        solo_rust: false,
    },
    OperazionePubblica {
        id: "data.run",
        versione: 2,
        comando: "run",
        sintassi: "run --plan PLAN.json [--input NAME=INPUT.arrow]... \
                   --output [NAME=]OUTPUT.arrow... [--overwrite] \
                   [--deadline RFC3339 | --timeout-ms MS]",
        riassunto: "esegue un piano e scrive i suoi output (.arrow file, .arrows stream, .parquet)",
        ingresso: "plenora-data-execution-input-v2",
        tipi_ingresso: &[JSON, ARROW_STREAM, ARROW_FILE],
        uscita: "plenora-data-execution-result-v2",
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
        usa_piano: true,
        solo_rust: false,
    },
    OperazionePubblica {
        id: "data.run",
        versione: 3,
        comando: "",
        sintassi: "",
        riassunto: "esegue un piano da sorgenti a destinazioni artefatto (runtime)",
        ingresso: "plenora-data-execution-input-v3",
        tipi_ingresso: &[JSON],
        uscita: "plenora-data-execution-result-v3",
        tipi_uscita: &[JSON],
        effetto: Effetto::Remoto,
        annullamento: true,
        scadenza: true,
        estensioni_ingresso: &[PARQUET],
        estensioni_uscita: &[PARQUET],
        materializzazione_limitata: true,
        usa_registro: true,
        export_rust: &["plenora_cli::api::esegui_artefatti"],
        export_python: &[],
        usa_piano: true,
        solo_rust: true,
    },
];

/// L'operazione del comando CLI canonico.
#[must_use]
pub fn per_comando(comando: &str) -> Option<&'static OperazionePubblica> {
    OPERAZIONI
        .iter()
        .find(|operazione| !operazione.solo_rust && operazione.comando == comando)
}

/// L'operazione dal suo identificatore, sulle superfici CLI e Python
/// (`data.run` 2: la 3 è solo Rust).
#[must_use]
pub fn per_id(id: &str) -> Option<&'static OperazionePubblica> {
    OPERAZIONI
        .iter()
        .find(|operazione| !operazione.solo_rust && operazione.id == id)
}
