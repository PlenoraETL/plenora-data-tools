//! `data.run` versione 3 (`plenora-contracts`, profilo data-tools v2,
//! DT-RUN-001..DT-RUN-008): un piano eseguito da sorgenti a destinazioni
//! che sono artefatti, con il manifesto degli artefatti pubblicati come
//! risultato. È la rappresentazione runtime di un piano con output nominati
//! (Runtime Binding 1.0 porta un payload per risposta e vieta i percorsi
//! locali, RT-013): questo artefatto la espone sulla superficie Rust;
//! trasporto, autorizzazione e risoluzione dei riferimenti spettano
//! all'applicazione, che fornisce un [`RisolutoreArtefatti`] (RT-015).
//!
//! Il flusso, in ordine:
//!
//! 1. la richiesta (`plenora-data-execution-input-v3`) si legge chiusa:
//!    chiavi ripetute, campi sconosciuti, riferimenti con la forma di un
//!    percorso (`file:`, segmenti `.` e `..`, punti codificati, spazi) e
//!    tipi di contenuto fuori dall'elenco sono `InvalidConfiguration`; il
//!    piano si legge dal suo testo originale ([`Pipeline::from_json`], con
//!    le stesse regole di un file di piano);
//! 2. i nomi delle sorgenti e delle destinazioni sono esattamente gli input
//!    e gli output del piano, due destinazioni non condividono un
//!    riferimento (DT-RUN-001, DT-RUN-002), e il risolutore prepara le
//!    destinazioni senza creare nulla ([`RisolutoreArtefatti::prepara`]);
//! 3. ogni sorgente si copia in un file temporaneo privato contando i byte e
//!    calcolando lo SHA-256: dimensione e digest attesi, se dichiarati, e la
//!    firma del formato dichiarato (`ARROW1` per il file Arrow, `PAR1` per
//!    Parquet, nessuna delle due per lo stream) si verificano prima di
//!    eseguire (DT-RUN-003);
//! 4. il piano gira con l'esecuzione da file di `plenora-io` (la semantica di
//!    `data.run` 2, DT-RUN-004) e scrive ogni output in un file temporaneo:
//!    un errore fin qui non ha pubblicato nulla (`remote_effect: none`,
//!    DT-RUN-005);
//! 5. gli output si pubblicano uno alla volta nell'ordine del piano; il
//!    fallimento di una pubblicazione porta l'effetto che il risolutore sa
//!    provare (DT-RUN-006): nessuno se nulla è stato scritto e nulla prima,
//!    `partial` se qualcosa è stato scritto, `unknown` se l'esito non è
//!    provato;
//! 6. il risultato (`plenora-data-execution-result-v3`) elenca ogni output
//!    nell'ordine del piano con riferimento, righe, colonne, tipo, byte e
//!    SHA-256 dei byte pubblicati, e i conteggi per passo di `data.run` 2.
//!
//! Né il risultato né un errore portano ciò a cui un riferimento si è
//! risolto, un percorso locale o un valore di riga (DT-RUN-008): i
//! riferimenti non entrano nei messaggi.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};

use plenora_core::limits::PlanLimits;
use plenora_core::{ErrorPhase, PlenoraError, RemoteEffect, Result};
use plenora_io::{
    CompressioneParquet, FileIngresso, FileUscita, Formato, Ingresso, OpzioniScrittura,
};
use plenora_pipeline::{Interruzione, Pipeline};
use serde::Deserialize;
use serde_json::value::RawValue;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::api::verifica_nomi;
use crate::operazioni::{ARROW_FILE, ARROW_STREAM, PARQUET};

/// Contratto della richiesta di `data.run` 3.
pub const CONTRATTO_RICHIESTA: &str = "plenora-data-execution-input-v3";
/// Contratto del risultato di `data.run` 3.
pub const CONTRATTO_RISULTATO: &str = "plenora-data-execution-result-v3";

/// Byte di una richiesta oltre il piano: le sorgenti e le destinazioni
/// (riferimenti di al più 2048 byte ciascuno) per un piano qualunque stanno
/// ampiamente qui.
const MARGINE_RICHIESTA: usize = 4 * 1024 * 1024;

/// Una destinazione come la vede il risolutore: nome dell'output nel piano,
/// riferimento opaco, tipo di contenuto e `overwrite`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Destinazione<'a> {
    /// Nome dell'output nel piano.
    pub nome: &'a str,
    /// Riferimento opaco, così come la richiesta lo dà (RT-013).
    pub riferimento: &'a str,
    /// Tipo di contenuto dell'artefatto.
    pub tipo: &'a str,
    /// `overwrite` della richiesta: senza, un artefatto esistente è un
    /// conflitto, verificato dalla destinazione insieme alla pubblicazione.
    pub sovrascrivi: bool,
}

/// Il fallimento di una pubblicazione (DT-RUN-006).
///
/// Porta ciò che il risolutore sa provare. Della causa conta solo
/// l'`ErrorKind`, che dà la categoria pubblica (`AlreadyExists` per un
/// artefatto esistente con `overwrite: false`, `PermissionDenied`,
/// `NotFound`, ...): il suo testo non entra mai nell'errore restituito,
/// perché può dire ciò a cui il riferimento si è risolto (DT-RUN-008).
#[derive(Debug)]
pub enum PubblicazioneFallita {
    /// Provato: la destinazione non ha ricevuto nulla.
    NienteScritto(std::io::Error),
    /// Provato: una parte dell'artefatto è stata scritta o sostituita.
    Parziale(std::io::Error),
    /// Non provato: non si sa che cosa la destinazione abbia ricevuto.
    Ignoto(std::io::Error),
}

/// Perché il risolutore rifiuta le destinazioni prima dell'esecuzione.
///
/// Ogni variante ha una categoria e un testo fissi: niente testo libero del
/// risolutore nell'errore restituito (DT-RUN-008).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RifiutoDestinazioni {
    /// Due destinazioni sono lo stesso artefatto sotto riferimenti diversi
    /// (`invalid_configuration`, DT-RUN-002).
    StessoArtefatto,
    /// Una destinazione non si risolve (`not_found`).
    NonTrovata,
    /// Una destinazione non è autorizzata (`authorization`).
    NonAutorizzata,
    /// Una destinazione con `overwrite: false` non sa rifiutare in modo
    /// atomico un artefatto esistente (`unsupported`, DT-RUN-006).
    SovrascritturaNonAtomica,
    /// Un altro errore di I/O della risoluzione, per tipo.
    Io(std::io::ErrorKind),
}

impl RifiutoDestinazioni {
    fn errore(self) -> PlenoraError {
        let errore = match self {
            Self::StessoArtefatto => PlenoraError::InvalidConfiguration(
                "richiesta data.run 3: due destinazioni sono lo stesso artefatto".to_owned(),
            ),
            Self::NonTrovata => solo_tipo("destinazione", std::io::ErrorKind::NotFound),
            Self::NonAutorizzata => solo_tipo("destinazione", std::io::ErrorKind::PermissionDenied),
            Self::SovrascritturaNonAtomica => PlenoraError::Unsupported(
                "una destinazione senza overwrite non rifiuta in modo atomico un artefatto \
                 esistente"
                    .to_owned(),
            ),
            Self::Io(tipo) => solo_tipo("destinazione", tipo),
        };
        errore.with_phase(ErrorPhase::Validate)
    }
}

/// Un errore di I/O del risolutore ridotto al suo tipo: il testo, che può
/// citare ciò a cui un riferimento si è risolto, non passa (DT-RUN-008).
fn solo_tipo(contesto: &str, tipo: std::io::ErrorKind) -> PlenoraError {
    PlenoraError::io_con_contesto(contesto, std::io::Error::from(tipo))
}

/// Il risolutore dei riferimenti, fornito dall'applicazione (RT-015).
///
/// Risolve i riferimenti solo nei namespace che autorizza e mai come
/// percorsi relativi al processo; questo artefatto non li decodifica, non li
/// normalizza e non li unisce a un percorso.
pub trait RisolutoreArtefatti {
    /// Scrive in `destinazione` i byte della sorgente `riferimento`. Un
    /// riferimento che non si risolve è `NotFound`, uno non autorizzato
    /// `PermissionDenied`. Risolvere una sorgente la legge e basta.
    ///
    /// # Errors
    ///
    /// L'errore di I/O della risoluzione o della lettura.
    fn leggi(&self, riferimento: &str, destinazione: &mut dyn Write) -> std::io::Result<()>;

    /// Prima dell'esecuzione, senza creare nulla: ogni destinazione si
    /// risolve ed è autorizzata, due destinazioni non sono lo stesso
    /// artefatto sotto riferimenti diversi, e una destinazione con
    /// `overwrite: false` sa rifiutare in modo atomico un artefatto esistente.
    ///
    /// # Errors
    ///
    /// [`RifiutoDestinazioni`].
    fn prepara(
        &self,
        destinazioni: &[Destinazione<'_>],
    ) -> std::result::Result<(), RifiutoDestinazioni>;

    /// Pubblica l'artefatto: i byte di `contenuto`, `byte` in tutto.
    ///
    /// # Errors
    ///
    /// [`PubblicazioneFallita`], con ciò che il risolutore sa provare.
    fn pubblica(
        &self,
        destinazione: &Destinazione<'_>,
        contenuto: &mut dyn Read,
        byte: u64,
    ) -> std::result::Result<(), PubblicazioneFallita>;
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Richiesta<'a> {
    #[serde(
        rename = "$schema",
        default,
        deserialize_with = "plenora_core::json::presente"
    )]
    _schema: Option<String>,
    schema_version: u64,
    #[serde(borrow)]
    plan: &'a RawValue,
    inputs: BTreeMap<String, Sorgente>,
    outputs: BTreeMap<String, Uscita>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Sorgente {
    reference: String,
    content_type: String,
    // `null` non è l'assenza: un'attesa scritta `null` non si verificherebbe.
    #[serde(default, deserialize_with = "plenora_core::json::presente")]
    expected: Option<Attesa>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Attesa {
    #[serde(default, deserialize_with = "plenora_core::json::presente")]
    size: Option<u64>,
    #[serde(default, deserialize_with = "plenora_core::json::presente")]
    sha256: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Uscita {
    reference: String,
    content_type: String,
    overwrite: bool,
}

fn configurazione(messaggio: &str) -> PlenoraError {
    PlenoraError::InvalidConfiguration(format!("richiesta data.run 3: {messaggio}"))
        .with_phase(ErrorPhase::Validate)
}

/// `\s` di ECMAScript (la semantica dei `pattern` di JSON Schema): gli
/// spazi `WhiteSpace` e i terminatori di riga, U+FEFF compreso e U+0085
/// escluso, diversamente da `char::is_whitespace`.
const fn spazio_ecma(c: char) -> bool {
    matches!(
        c,
        '\t' | '\n' | '\u{b}' | '\u{c}' | '\r' | ' ' | '\u{a0}' | '\u{1680}' | '\u{2000}'
            ..='\u{200a}'
                | '\u{2028}'
                | '\u{2029}'
                | '\u{202f}'
                | '\u{205f}'
                | '\u{3000}'
                | '\u{feff}'
    )
}

/// Un riferimento opaco come lo vuole lo schema `data-execution-input-v3`,
/// regola per regola.
///
/// Lunghezza da 4 a 2048 caratteri (punti di codice, come `minLength` e
/// `maxLength`); `^[a-z][a-z0-9+.-]{1,31}:(//)?[^\s\\]+$`, cioè uno schema e
/// poi almeno un carattere senza spazi né barre rovesciate (le `//` dopo lo
/// schema sono a loro volta caratteri ammessi); e nessuna delle forme
/// escluse da `not`: `file:` all'inizio senza distinzione di maiuscole, un
/// segmento `.` o `..` (dopo l'inizio, `/` o `:`, prima di `/` o della
/// fine), `%2e` o `%2E`, uno spazio.
fn riferimento_valido(riferimento: &str) -> bool {
    let caratteri: Vec<char> = riferimento.chars().collect();
    if !(4..=2048).contains(&caratteri.len()) {
        return false;
    }
    let Some(due_punti) = caratteri.iter().position(|c| *c == ':') else {
        return false;
    };
    let (schema, resto) = (&caratteri[..due_punti], &caratteri[due_punti + 1..]);
    let schema_valido = schema.first().is_some_and(char::is_ascii_lowercase)
        && (2..=32).contains(&schema.len())
        && schema[1..]
            .iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "+.-".contains(*c));
    let resto_valido = !resto.is_empty() && !resto.iter().any(|c| spazio_ecma(*c) || *c == '\\');
    let file = caratteri.len() >= 5
        && caratteri[..4]
            .iter()
            .collect::<String>()
            .eq_ignore_ascii_case("file")
        && caratteri[4] == ':';
    // `(^|[/:])\.{1,2}(/|$)`: un punto o due, preceduti dall'inizio, da `/`
    // o da `:`, seguiti da `/` o dalla fine.
    let segmento_punto = (0..caratteri.len()).any(|inizio| {
        caratteri[inizio] == '.'
            && (inizio == 0 || matches!(caratteri[inizio - 1], '/' | ':'))
            && [1, 2].iter().any(|punti| {
                let termine = inizio + punti;
                termine <= caratteri.len()
                    && caratteri[inizio..termine].iter().all(|c| *c == '.')
                    && (termine == caratteri.len() || caratteri[termine] == '/')
            })
    });
    let punti_codificati = caratteri
        .windows(3)
        .any(|tre| tre[0] == '%' && tre[1] == '2' && matches!(tre[2], 'e' | 'E'));
    schema_valido
        && resto_valido
        && !file
        && !segmento_punto
        && !punti_codificati
        && !caratteri.iter().any(|c| spazio_ecma(*c))
}

fn formato_del_tipo(tipo: &str) -> Option<Formato> {
    match tipo {
        ARROW_FILE => Some(Formato::ArrowIpc),
        ARROW_STREAM => Some(Formato::ArrowIpcStream),
        PARQUET => Some(Formato::Parquet),
        _ => None,
    }
}

/// La firma dei primi byte contro il formato dichiarato (DT-RUN-003): la
/// lettura di `plenora-io` riconosce file e stream Arrow dal contenuto, e
/// una sorgente si legge come il tipo che dichiara.
fn firma_coerente(formato: Formato, inizio: &[u8]) -> bool {
    let arrow_file = inizio.starts_with(b"ARROW1");
    let parquet = inizio.starts_with(b"PAR1");
    match formato {
        Formato::ArrowIpc => arrow_file,
        Formato::ArrowIpcStream => !arrow_file && !parquet,
        Formato::Parquet => parquet,
    }
}

/// Uno scrittore che conta i byte, calcola lo SHA-256 e tiene i primi byte
/// per la firma.
struct Impronta<W> {
    interno: W,
    byte: u64,
    hash: Sha256,
    inizio: Vec<u8>,
}

impl<W: Write> Write for Impronta<W> {
    fn write(&mut self, dati: &[u8]) -> std::io::Result<usize> {
        let scritti = self.interno.write(dati)?;
        let scritti_qui = &dati[..scritti];
        self.hash.update(scritti_qui);
        self.byte = self.byte.saturating_add(scritti as u64);
        let mancano = 8_usize.saturating_sub(self.inizio.len());
        self.inizio
            .extend_from_slice(&scritti_qui[..scritti_qui.len().min(mancano)]);
        Ok(scritti)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.interno.flush()
    }
}

fn esadecimale(digest: &[u8]) -> String {
    use std::fmt::Write as _;
    digest
        .iter()
        .fold(String::with_capacity(64), |mut testo, byte| {
            let _ = write!(testo, "{byte:02x}");
            testo
        })
}

/// Byte e SHA-256 di un file temporaneo d'uscita.
fn impronta_del_file(percorso: &Path) -> Result<(u64, String)> {
    let mut lettore = BufReader::new(File::open(percorso)?);
    let mut hash = Sha256::new();
    let mut byte = 0_u64;
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        let letti = lettore.read(&mut buffer)?;
        if letti == 0 {
            break;
        }
        hash.update(&buffer[..letti]);
        byte = byte.saturating_add(letti as u64);
    }
    Ok((byte, esadecimale(&hash.finalize())))
}

/// Una sorgente letta in un file temporaneo e verificata (DT-RUN-003).
fn leggi_sorgente(
    risolutore: &dyn RisolutoreArtefatti,
    sorgente: &Sorgente,
    formato: Formato,
    percorso: &Path,
) -> Result<()> {
    let errore_io =
        |errore| PlenoraError::io_con_contesto("sorgente", errore).with_phase(ErrorPhase::Read);
    let file = File::create(percorso).map_err(errore_io)?;
    let mut impronta = Impronta {
        interno: BufWriter::new(file),
        byte: 0,
        hash: Sha256::new(),
        inizio: Vec::with_capacity(8),
    };
    risolutore
        .leggi(&sorgente.reference, &mut impronta)
        .map_err(|errore| solo_tipo("sorgente", errore.kind()).with_phase(ErrorPhase::Read))?;
    impronta.flush().map_err(errore_io)?;
    let incoerente = |cosa: &str| {
        PlenoraError::DataMapping(format!("sorgente con {cosa} diverso da quello atteso"))
            .with_phase(ErrorPhase::Read)
    };
    if let Some(attesa) = &sorgente.expected {
        if attesa.size.is_some_and(|attesi| attesi != impronta.byte) {
            return Err(incoerente("numero di byte"));
        }
        let digest = esadecimale(&impronta.hash.clone().finalize());
        if attesa
            .sha256
            .as_ref()
            .is_some_and(|atteso| *atteso != digest)
        {
            return Err(incoerente("SHA-256"));
        }
    }
    if !firma_coerente(formato, &impronta.inizio) {
        return Err(PlenoraError::DataMapping(
            "sorgente il cui contenuto non e' del tipo dichiarato".to_owned(),
        )
        .with_phase(ErrorPhase::Read));
    }
    Ok(())
}

/// Richiesta letta e piano: forma chiusa, riferimenti opachi, tipi
/// accettati, nomi uguali a quelli del piano, destinazioni distinte.
fn leggi_richiesta(richiesta: &str) -> Result<(Richiesta<'_>, Pipeline)> {
    let massimo = PlanLimits::default()
        .max_plan_json_bytes
        .saturating_add(MARGINE_RICHIESTA);
    if richiesta.len() > massimo {
        return Err(configurazione("oltre la dimensione massima"));
    }
    plenora_core::json::ensure_no_duplicate_keys(richiesta)
        .map_err(|_| configurazione("chiave ripetuta"))?;
    let letta: Richiesta<'_> = serde_json::from_str(richiesta)
        .map_err(|_| configurazione("forma non valida (campi, tipi o valori)"))?;
    if letta.schema_version != 1 {
        return Err(configurazione("schema_version diversa da 1"));
    }
    for sorgente in letta.inputs.values() {
        if !riferimento_valido(&sorgente.reference) {
            return Err(configurazione("riferimento di una sorgente non opaco"));
        }
        if formato_del_tipo(&sorgente.content_type).is_none() {
            return Err(configurazione(
                "tipo di contenuto di una sorgente non accettato",
            ));
        }
        if let Some(attesa) = &sorgente.expected {
            if attesa.size.is_none() && attesa.sha256.is_none() {
                return Err(configurazione("`expected` vuoto"));
            }
            if attesa.sha256.as_ref().is_some_and(|digest| {
                digest.len() != 64
                    || !digest
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            }) {
                return Err(configurazione("SHA-256 atteso non valido"));
            }
        }
    }
    for uscita in letta.outputs.values() {
        if !riferimento_valido(&uscita.reference) {
            return Err(configurazione("riferimento di una destinazione non opaco"));
        }
        if formato_del_tipo(&uscita.content_type).is_none() {
            return Err(configurazione(
                "tipo di contenuto di una destinazione non accettato",
            ));
        }
    }
    let piano = Pipeline::from_json(letta.plan.get())?;
    verifica_nomi(
        &piano.inputs,
        letta.inputs.keys().map(String::as_str),
        "input",
    )?;
    verifica_nomi(
        &piano.outputs,
        letta.outputs.keys().map(String::as_str),
        "output",
    )?;
    let mut riferimenti: Vec<&str> = letta
        .outputs
        .values()
        .map(|uscita| uscita.reference.as_str())
        .collect();
    riferimenti.sort_unstable();
    if riferimenti.windows(2).any(|coppia| coppia[0] == coppia[1]) {
        return Err(configurazione("due destinazioni con lo stesso riferimento"));
    }
    Ok((letta, piano))
}

/// Le sorgenti nei file temporanei di `cartella`, nell'ordine degli input
/// del piano (DT-RUN-003).
fn leggi_sorgenti(
    letta: &Richiesta<'_>,
    piano: &Pipeline,
    risolutore: &dyn RisolutoreArtefatti,
    interruzione: &Interruzione,
    cartella: &Path,
) -> Result<Vec<Ingresso>> {
    let mut ingressi = Vec::with_capacity(piano.inputs.len());
    for (indice, nome) in piano.inputs.iter().enumerate() {
        let sorgente = letta
            .inputs
            .get(nome)
            .ok_or_else(|| configurazione("input del piano senza sorgente"))?;
        let formato = formato_del_tipo(&sorgente.content_type)
            .ok_or_else(|| configurazione("tipo di contenuto di una sorgente non accettato"))?;
        interruzione
            .verifica("prima di leggere una sorgente")
            .map_err(|errore| errore.with_phase(ErrorPhase::Read))?;
        let percorso = cartella.join(format!("sorgente-{indice}"));
        leggi_sorgente(risolutore, sorgente, formato, &percorso)?;
        ingressi.push(Ingresso::File(FileIngresso {
            nome: nome.clone(),
            percorso,
            formato: Some(formato),
        }));
    }
    Ok(ingressi)
}

/// Un output pronto da pubblicare: destinazione, file temporaneo, byte,
/// SHA-256, righe e colonne.
struct Pronto<'a> {
    destinazione: Destinazione<'a>,
    percorso: PathBuf,
    byte: u64,
    sha256: String,
    righe: u64,
    colonne: u64,
}

/// Pubblica gli output uno alla volta, nell'ordine del piano (DT-RUN-006,
/// DT-RUN-007).
fn pubblica(
    pronti: &[Pronto<'_>],
    risolutore: &dyn RisolutoreArtefatti,
    interruzione: &Interruzione,
) -> Result<()> {
    for (pubblicati, pronto) in pronti.iter().enumerate() {
        let effetto_finora = if pubblicati == 0 {
            RemoteEffect::None
        } else {
            RemoteEffect::Partial
        };
        interruzione
            .verifica("prima di pubblicare un output")
            .map_err(|errore| {
                errore
                    .with_phase(ErrorPhase::Write)
                    .with_remote_effect(effetto_finora)
            })?;
        let mut contenuto = BufReader::new(File::open(&pronto.percorso).map_err(|errore| {
            PlenoraError::io_con_contesto("uscita temporanea", errore)
                .with_phase(ErrorPhase::Write)
                .with_remote_effect(effetto_finora)
        })?);
        if let Err(fallimento) =
            risolutore.pubblica(&pronto.destinazione, &mut contenuto, pronto.byte)
        {
            let (causa, effetto) = match fallimento {
                PubblicazioneFallita::NienteScritto(causa) => (causa, effetto_finora),
                PubblicazioneFallita::Parziale(causa) => (causa, RemoteEffect::Partial),
                PubblicazioneFallita::Ignoto(causa) => (causa, RemoteEffect::Unknown),
            };
            return Err(solo_tipo("destinazione", causa.kind())
                .with_phase(ErrorPhase::Write)
                .with_remote_effect(effetto));
        }
    }
    // Un'interruzione arrivata durante l'ultima pubblicazione non diventa un
    // successo: tutto è pubblicato, l'effetto è `committed` e il ritentativo
    // non è automatico (come `data.run` 2 dopo l'ultima scrittura).
    interruzione
        .verifica("dopo aver pubblicato gli output")
        .map_err(|errore| {
            errore
                .with_phase(ErrorPhase::Finalize)
                .with_remote_effect(RemoteEffect::Committed)
        })
}

/// `data.run` 3: un piano da sorgenti a destinazioni artefatto.
///
/// Esegue la richiesta `richiesta` (il testo JSON di
/// `plenora-data-execution-input-v3`) leggendo le sorgenti e pubblicando le
/// destinazioni con `risolutore`; restituisce il risultato
/// `plenora-data-execution-result-v3`. Scadenza e annullamento si
/// controllano come in `data.run` 2 fino alla pubblicazione, e prima di ogni
/// pubblicazione: dopo la prima, un'interruzione è `partial` (DT-RUN-007).
///
/// # Errors
///
/// `InvalidConfiguration` per una richiesta che non è il contratto o i cui
/// nomi non sono quelli del piano; `InvalidPlan` per il piano; quelli di
/// [`RisolutoreArtefatti::prepara`]; l'I/O delle sorgenti (fase `read`),
/// `DataMapping` per una sorgente diversa dall'attesa o dal tipo dichiarato;
/// quelli dell'esecuzione di `data.run` 2, tutti senza effetto; il
/// fallimento di una pubblicazione con l'effetto di DT-RUN-006.
pub fn esegui_artefatti(
    richiesta: &str,
    risolutore: &dyn RisolutoreArtefatti,
    interruzione: &Interruzione,
) -> Result<Value> {
    let (letta, piano) = leggi_richiesta(richiesta)?;
    // Le destinazioni nell'ordine del piano: l'ordine di pubblicazione.
    let destinazioni: Vec<Destinazione<'_>> = piano
        .outputs
        .iter()
        .filter_map(|nome| {
            letta.outputs.get(nome).map(|uscita| Destinazione {
                nome,
                riferimento: &uscita.reference,
                tipo: &uscita.content_type,
                sovrascrivi: uscita.overwrite,
            })
        })
        .collect();
    risolutore
        .prepara(&destinazioni)
        .map_err(RifiutoDestinazioni::errore)?;

    let cartella = tempfile::tempdir()
        .map_err(|errore| PlenoraError::io_con_contesto("cartella temporanea", errore))?;
    let ingressi = leggi_sorgenti(&letta, &piano, risolutore, interruzione, cartella.path())?;
    let uscite: Vec<FileUscita> = destinazioni
        .iter()
        .enumerate()
        .map(|(indice, destinazione)| FileUscita {
            nome: destinazione.nome.to_owned(),
            percorso: cartella.path().join(format!("uscita-{indice}")),
            formato: formato_del_tipo(destinazione.tipo),
        })
        .collect();
    let opzioni = OpzioniScrittura {
        formato: None,
        sovrascrivi: false,
        compressione: CompressioneParquet::default(),
    };
    // I file d'uscita sono temporanei e privati: un errore fin qui non ha
    // pubblicato nulla (DT-RUN-005), anche quando `plenora-io` lo segna
    // `partial` per i suoi file.
    let esito = plenora_io::esegui_ingressi(&piano, ingressi, &uscite, &opzioni, interruzione)
        .map_err(|errore| errore.override_remote_effect(RemoteEffect::None))?;

    // Tutto ciò che il manifesto dice si conosce prima di pubblicare: un
    // output senza resoconto è un errore nostro, senza effetto.
    let righe: BTreeMap<&str, (u64, u64)> = esito
        .uscite
        .iter()
        .map(|uscita| (uscita.nome.as_str(), (uscita.righe, uscita.colonne)))
        .collect();
    let pronti = destinazioni
        .iter()
        .zip(&uscite)
        .map(|(destinazione, uscita)| {
            let (righe, colonne) = righe
                .get(destinazione.nome)
                .copied()
                .ok_or_else(|| PlenoraError::Internal("output senza resoconto".to_owned()))?;
            let (byte, sha256) = impronta_del_file(&uscita.percorso)?;
            Ok(Pronto {
                destinazione: *destinazione,
                percorso: uscita.percorso.clone(),
                byte,
                sha256,
                righe,
                colonne,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    pubblica(&pronti, risolutore, interruzione)?;
    // La cartella temporanea si toglie e la rimozione si verifica: un
    // fallimento lascerebbe sul disco copie delle sorgenti e delle uscite.
    // Gli output sono già pubblicati, quindi l'effetto è `committed`.
    cartella.close().map_err(|errore| {
        solo_tipo("cartella temporanea", errore.kind())
            .with_phase(ErrorPhase::Finalize)
            .with_remote_effect(RemoteEffect::Committed)
    })?;

    let outputs: Vec<Value> = pronti
        .iter()
        .map(|pronto| {
            json!({
                "name": pronto.destinazione.nome,
                "reference": pronto.destinazione.riferimento,
                "rows": pronto.righe,
                "columns": pronto.colonne,
                "artifact": {
                    "content_type": pronto.destinazione.tipo,
                    "size": pronto.byte,
                    "sha256": pronto.sha256,
                },
            })
        })
        .collect();
    let steps: Vec<Value> = esito
        .report
        .passi
        .iter()
        .map(|passo| {
            json!({
                "out": passo.out,
                "op": passo.op,
                "rows_in": passo.righe_in,
                "rows_out": passo.righe_out,
                "division_by_zero_rows": passo.righe_divisione_per_zero,
            })
        })
        .collect();
    Ok(json!({
        "schema_version": 1,
        "outputs": outputs,
        "steps": steps,
    }))
}
