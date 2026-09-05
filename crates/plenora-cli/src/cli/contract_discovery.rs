//! Scoperta del contratto di un input Arrow, e le decisioni sul CRS.
//!
//! Che cosa un file dichiara di se' — campi, colonna geometrica, dimensioni,
//! encoding, CRS — letto dai soli metadati dello schema, senza toccare una
//! riga di dati. E' la superficie da cui nascono `describe`, la validazione
//! del piano e la verifica di compatibilita' degli input.
//!
//! Le regole sul CRS sono qui e non sparse: una dichiarazione incoerente
//! resta incoerente (`DeclaredUnresolved`) invece di essere risolta in
//! silenzio a favore di una delle due forme, e la risoluzione per decisione
//! del piano ([`apply_crs_decisions`]) sostituisce le dichiarazioni della
//! sorgente in un punto solo.

use std::path::{Path, PathBuf};

use plenora_core::arrow::schema::{DataType, SchemaRef};
use plenora_core::contract::{
    ContractCrs, ContractProperties, ContractProperty, CrsDefinitionFormat, CrsResolution,
    DataContract, FieldId, GeometryColumnContract, GeometryDimensions, PropertyConfidence,
    PropertyScope,
};
use plenora_core::crs::ResolvedCrs;
use plenora_core::PlenoraError;
use plenora_engine::{ipc_boundary, Input, IpcLimits};
use plenora_kernels_geo::arrow_adapter::{
    read_contract_version, read_geometry_contract_keys, CanonicalGeometryKeys,
    GEOARROW_EXTENSION_KEY, GEOARROW_WKB_EXTENSION, GEO_METADATA_KEY,
    PLENORA_GEOMETRY_CRS_RESOLUTION_KEY, PLENORA_GEOMETRY_NAMESPACE_PREFIX,
};

#[cfg(not(feature = "proj-backend"))]
use plenora_core::crs::resolve_crs;
#[cfg(feature = "proj-backend")]
use plenora_kernels_geo::crs::resolve_crs;

use crate::{contract, DagInputs, PlanInputsProbe};

// La conversione schema -> contratto e' autorita' di `plenora-core`: qui
// resta il contesto di file e input, che core non deve conoscere.
pub use plenora_core::contract::arrow_schema::{
    contract_crs_from_keys, contract_from_arrow_schema as discover_input_contract_from_schema,
    crs_definition_from_metadata, geometry_contract_from_field,
};

/// Schema Arrow dell'header IPC di un input (file o stream format): nessuna
/// riga di dati letta.
///
/// Passa dal lettore di confine condiviso ([`plenora_engine::ipc_boundary`]):
/// framing e limiti pre-validati prima che arrow allochi, panico di
/// `fb_to_schema` convertito in errore. La CLI non apre piu'
/// `FileReader`/`StreamReader` per conto proprio su input non fidati.
///
/// Confine di lettura (BLOCK-03): gli errori `Io`/`DataMapping` di apertura
/// e parse dell'header nascono leggendo la sorgente — tag
/// [`ErrorPhase::Read`].
pub fn ipc_header_schema(path: &Path) -> Result<SchemaRef, PlenoraError> {
    ipc_boundary::header_schema(path, &IpcLimits::default())
}

/// Input lazy per l'executor: IPC file o stream format, sniffato dal magic.
pub fn open_input(path: &Path, limits: &IpcLimits) -> Result<Input, PlenoraError> {
    Input::read_ipc_with_limits(path, limits)
}

/// Scoperta del `DataContract` di un input dal solo header IPC: schema Arrow
/// e colonne geometria se presenti, CRS risolto SE dichiarato. Fail-closed
/// su metadati incoerenti. Il `FieldId` e' provvisorio: il planner lo rimappa
/// nel namespace globale del grafo (D16).
///
/// Protocollo delle chiavi canoniche (contratti trasversali §2):
///
/// - gate R2.5 all'ingresso: [`read_contract_version`] — una versione
///   successiva a quella nota, una chiave di versione malformata o chiavi
///   canoniche senza versione sono errori propagati, mai ignorati;
/// - sorgente primaria per ogni campo geometria:
///   [`read_geometry_contract_keys`] — chiavi canoniche fail-closed,
///   coerenza canonica/legacy (R2.6: divergenza -> errore) e completamento
///   per precedenza (R2.7) sono applicati dal reader;
/// - riconoscimento (1c): le chiavi canoniche sono autosufficienti (tabella
///   §2), quindi un campo con chiavi `plenora.geometry.*` e' riconosciuto
///   come colonna geometrica anche SENZA l'estensione `geoarrow.wkb` e il
///   metadato `geo`; in entrambi i percorsi il tipo DEVE essere `Binary`,
///   altrimenti errore esplicito;
/// - CRS: lo stato e' deciso da [`contract_crs_from_keys`] sulla
///   rappresentazione completata (canonica o legacy) e sulla
///   `crs_resolution` dichiarata. R4.6.3 (v2.0-rc9/rc10): un campo
///   geometrico SENZA CRS dichiarato NON e' un errore — il centro non puo'
///   pretendere un CRS risolvibile per operazioni che non lo richiedono;
///   lo stato entra nel contratto come [`ContractCrs::Missing`] (R4.4: mai
///   un CRS inventato) e ferma solo le op che dichiarano un
///   `CrsRequirement`, in analyze. Un'incoerenza dichiarata
///   (`declared_unresolved`) o un conflitto decidibile fra rappresentazioni
///   diventano [`ContractCrs::DeclaredUnresolved`], preservati e mai
///   risolti in assenza di una decisione esplicita nel piano
///   (`crs_decisions`). Resta errore la dichiarazione contraddittoria
///   (`crs_resolution` valorizzata ma nessuna rappresentazione: R4.1 vieta
///   di collassarla su `missing`).
pub fn discover_input_contract(path: &Path) -> Result<DataContract, PlenoraError> {
    // Il risolutore e' quello scelto dalla build (base oppure PROJ): l'autorita'
    // in core interpreta lo schema, il chiamante fornisce il backend. Non e'
    // una scelta di questo file — e' la stessa che il worker isolato dovra'
    // ricevere dal supervisore, invece di dedurla dalla propria compilazione.
    discover_input_contract_from_schema(ipc_header_schema(path)?, resolve_crs)
}

/// Contesto "input `nome` (percorso)" sull'errore, preservando la variante.
pub fn at_input(name: &str, path: &Path, error: PlenoraError) -> PlenoraError {
    error.con_contesto(&format!("input `{name}` ({})", path.display()))
}

/// Accoppia gli input della riga di comando a quelli dichiarati dal piano DAG.
///
/// Nella forma NOMINALE l'accoppiamento e' quello scritto: ogni nome dev'essere
/// dichiarato dal piano e ogni input dichiarato dev'essere fornito, una volta
/// sola.
///
/// La forma POSIZIONALE e' ammessa **solo con un input dichiarato**. Con due o
/// piu' input non e' verificabile: due file scambiati con lo stesso schema
/// producono un risultato sbagliato invece di un errore — il piano gira, i
/// contratti combaciano, e nessuno se ne accorge. E' l'unico punto della CLI
/// in cui uno scambio dell'utente non sarebbe intercettabile dal componente,
/// e per questo quella forma non arriva all'esecuzione.
///
/// # Errors
///
/// `PlenoraError::InvalidPlan` su nome non dichiarato, input dichiarato ma non
/// fornito, forma posizionale con piu' di un input dichiarato, o conteggio
/// diverso nella forma posizionale.
pub fn pair_v4_inputs(
    probe: &PlanInputsProbe,
    inputs: &DagInputs,
) -> Result<Vec<(String, PathBuf)>, PlenoraError> {
    let paths = match inputs {
        DagInputs::Named(named) => {
            for (name, _) in named {
                if !probe.inputs.iter().any(|declared| declared == name) {
                    return Err(contract(format!(
                        "input `{name}` non dichiarato dal piano (dichiarati: {})",
                        probe.inputs.join(", ")
                    )));
                }
            }
            // L'ordine restituito e' quello del PIANO, non quello della riga
            // di comando: cosi' l'ordine degli argomenti non e' osservabile a
            // valle e non puo' diventare una dipendenza implicita.
            return probe
                .inputs
                .iter()
                .map(|declared| {
                    named
                        .iter()
                        .find(|(name, _)| name == declared)
                        .map(|(name, path)| (name.clone(), path.clone()))
                        .ok_or_else(|| {
                            contract(format!(
                                "input `{declared}` dichiarato dal piano ma non fornito: \
                                 aggiungere `--input {declared}=PERCORSO`"
                            ))
                        })
                })
                .collect();
        }
        DagInputs::Positional(paths) => paths,
    };
    if probe.inputs.len() > 1 {
        // Con piu' di un input la forma posizionale non e' VERIFICABILE: due
        // percorsi scambiati sono indistinguibili da due percorsi giusti, e
        // se gli schemi coincidono il piano gira producendo il risultato
        // sbagliato. Un avviso non basta — nei log di una pipeline non lo
        // legge nessuno — quindi si rifiuta prima di toccare i file,
        // indicando la forma che chiude il problema. Resta ammessa con un
        // input solo, dove non c'e' niente da scambiare.
        return Err(contract(format!(
            "`--inputs` accoppia i percorsi per POSIZIONE e non e' ammesso con {} input \
             dichiarati: usare la forma nominale `{}`",
            probe.inputs.len(),
            probe
                .inputs
                .iter()
                .map(|name| format!("--input {name}=PERCORSO"))
                .collect::<Vec<_>>()
                .join(" ")
        )));
    }
    if probe.inputs.len() != paths.len() {
        return Err(contract(format!(
            "il piano dichiara {} input ({}) ma ne sono stati forniti {}",
            probe.inputs.len(),
            probe.inputs.join(", "),
            paths.len()
        )));
    }
    Ok(probe
        .inputs
        .iter()
        .cloned()
        .zip(paths.iter().cloned())
        .collect())
}

/// Contratti degli input di un piano DAG, scoperti dagli header IPC.
pub fn discover_contracts(
    pairs: &[(String, PathBuf)],
) -> Result<Vec<(String, DataContract)>, PlenoraError> {
    pairs
        .iter()
        .map(|(name, path)| {
            discover_input_contract(path)
                .map(|contract| (name.clone(), contract))
                .map_err(|error| at_input(name, path, error))
        })
        .collect()
}

/// Applica le decisioni CRS esplicite del piano DAG (`crs_decisions`,
/// R4.6.3) ai contratti scoperti: per ogni input nominato, la definizione
/// decisa e' risolta contro il backend (senza `proj-backend`:
/// `BackendUnavailable`, come il CRS di piano) e sostituisce lo stato
/// [`ContractCrs::DeclaredUnresolved`] con
/// [`ContractCrs::ResolvedByDecision`] — un CRS risolto a tutti gli effetti
/// per le op a valle, marcato perche' l'emissione SOSTITUISCA le
/// dichiarazioni della sorgente con il CRS deciso
/// (`strip_decided_crs_declarations` nella fusione dello schema di
/// output). Lo schema del contratto di input NON e' toccato: il check
/// fail-closed dell'executor confronta i campi del file con quelli del
/// contratto validato (metadati inclusi). La decisione resta esplicita nel
/// piano e coperta dal `plan_hash` (piano-v5.md#identita-e-fingerprint); il fingerprint del contratto
/// di input cambia di conseguenza (un piano con decisione non accetta in
/// riesecuzione l'input non deciso senza rivalidazione).
///
/// Errori espliciti (mai una decisione ignorata in silenzio): input non
/// fornito o senza colonna geometrica; stato diverso da
/// `DeclaredUnresolved` (su `Missing` sarebbe un CRS inventato — R4.4; su
/// `Resolved` una contraddizione del piano); definizione non risolvibile.
/// I messaggi non riportano valori di dichiarazioni (regola «errori senza
/// dati»).
pub fn apply_crs_decisions(
    probe: &PlanInputsProbe,
    contracts: &mut [(String, DataContract)],
) -> Result<(), PlenoraError> {
    for (input, definition) in &probe.crs_decisions {
        let Some((_, input_contract)) = contracts.iter_mut().find(|(name, _)| name == input) else {
            return Err(contract(format!(
                "crs_decisions: l'input `{input}` non e' tra i contratti scoperti"
            )));
        };
        if input_contract.geometries.len() != 1 {
            return Err(contract(format!(
                "crs_decisions: l'input `{input}` non dichiara esattamente una colonna \
                 geometrica: la decisione non e' applicabile"
            )));
        }
        let geometry = &mut input_contract.geometries[0];
        if !matches!(geometry.crs, ContractCrs::DeclaredUnresolved { .. }) {
            return Err(contract(format!(
                "crs_decisions: l'input `{input}` dichiara il CRS come `{}`, non come \
                 `declared_unresolved`: la decisione non e' applicabile",
                geometry.crs.resolution()
            )));
        }
        geometry.crs = ContractCrs::ResolvedByDecision(resolve_crs(definition, "crs")?);
    }
    Ok(())
}
