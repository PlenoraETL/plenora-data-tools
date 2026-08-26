//! Codifica e decodifica dei frame: **solo forma**, nessuna semantica.
//!
//! # Il frame
//!
//! | offset | byte | campo |
//! |---|---|---|
//! | 0 | 4 | lunghezza del payload, `u32` **big-endian**, prefisso escluso |
//! | 4 | N | payload: JSON UTF-8 **compatto** |
//!
//! # Il tetto si applica prima di allocare
//!
//! In lettura si prendono i quattro byte del prefisso, si confronta con
//! [`MAX_PROTOCOL_FRAME_BYTES`], e **solo dopo** si guarda il payload. Un
//! frame che dichiara un gigabyte non ne fa allocare nemmeno uno.
//!
//! In scrittura non si costruisce un buffer illimitato per poi misurarlo:
//! [`ScrittoreLimitato`] fallisce al primo byte oltre il tetto. La differenza
//! conta quando il frame e' grande — misurare dopo significa aver gia'
//! allocato cio' che si voleva rifiutare.

use std::io::{self, Write};

use plenora_core::{PlenoraError, Result};

use super::limiti::{
    ESPANSIONE_ESCAPE, MAX_BACKEND_DINAMICI, MAX_CAPABILITY, MAX_CHIAVE_CONTEGGIO_BYTES,
    MAX_CONTEGGI_DIAGNOSTICA, MAX_DIGEST_BYTES, MAX_ESEMPIO_BYTES, MAX_ESEMPI_DIAGNOSTICA,
    MAX_IDENTIFICATORE_BYTES, MAX_INGRESSI, MAX_MESSAGGIO_BYTES, MAX_MOTIVO_BYTES,
    MAX_PERCORSO_BYTES, MAX_PIANO_CANONICO_BYTES, MAX_RISORSE, MAX_VERSIONE_BYTES,
};
use super::messaggi::{Corpo, Frame, TipoMessaggio, VERSIONE_PROTOCOLLO};

/// Byte del prefisso di lunghezza.
pub const BYTE_PREFISSO: usize = 4;

// ---------------------------------------------------------------------------
// La derivazione del tetto del frame
// ---------------------------------------------------------------------------

/// Byte dell'involucro JSON, **contati** e non stimati.
///
/// `{"protocol_version":65535,"tipo":"progresso","corpo":}` e' la forma piu'
/// lunga: `u16` a cinque cifre e il nome di tipo piu' lungo.
///
/// «Piu' lungo» fra i **sei tipi di messaggio** — `progresso`, nove caratteri
/// — non fra i nomi degli assi dell'errore, che sono ben piu' lunghi ma non
/// compaiono mai qui: `tipo` non li puo' contenere.
///
/// Il conto e' sui caratteri letterali, quindi e' verificabile leggendolo.
const INVOLUCRO_BYTES: usize = {
    // {"protocol_version": = 20, valore u16 = 5, , = 1
    let versione = 20 + 5 + 1;
    // "tipo":"" = 9, nome del tipo piu' lungo ("progresso" = 9), , = 1
    let tipo = 9 + 9 + 1;
    // "corpo": = 8, } di chiusura = 1
    let corpo = 8 + 1;
    versione + tipo + corpo
};

/// Byte di un campo stringa **codificato**, dal suo tetto decodificato.
///
/// Il tetto vale sui byte UTF-8 decodificati; qui si aggiunge l'espansione
/// degli escape e le due virgolette.
const fn stringa_codificata(tetto_decodificato: usize) -> usize {
    tetto_decodificato * ESPANSIONE_ESCAPE + 2
}

/// Byte massimi di un descrittore d'ingresso, **contando le chiavi**.
const DESCRITTORE_BYTES: usize = {
    // {"nome": = 8, "percorso": = 12, "formato": = 11, "stream" = 8,
    // "contract_fingerprint_atteso": = 30, tre virgole = 3, } = 1
    let chiavi = 8 + 12 + 11 + 8 + 30 + 3 + 1;
    chiavi
        + stringa_codificata(MAX_IDENTIFICATORE_BYTES)
        + stringa_codificata(MAX_PERCORSO_BYTES)
        + stringa_codificata(MAX_DIGEST_BYTES)
};

/// Byte massimi del corpo di un `Incarico`, il messaggio piu' grande.
const INCARICO_BYTES: usize = {
    // {"piano_canonico": = 18, "plan_hash_atteso": = 20, "ingressi": = 12,
    // "artefatto_temporaneo": = 24, virgole = 3, [] = 2, } = 1,
    // separatori fra i descrittori = MAX_INGRESSI - 1
    let chiavi = 18 + 20 + 12 + 24 + 3 + 2 + 1 + (MAX_INGRESSI - 1);
    chiavi
        + MAX_PIANO_CANONICO_BYTES
        + stringa_codificata(MAX_DIGEST_BYTES)
        + DESCRITTORE_BYTES * MAX_INGRESSI
        + stringa_codificata(MAX_PERCORSO_BYTES)
};

/// Tetto del payload di un frame, **prefisso escluso**.
///
/// Derivato con aritmetica `checked` dai tetti dei campi e dall'involucro
/// contato. Non e' un numero tondo scelto: e' una somma, e i test verificano
/// che i sei messaggi massimi ci stiano dentro e che l'`Incarico` sia
/// davvero il piu' grande.
pub const MAX_PROTOCOL_FRAME_BYTES: usize = match INVOLUCRO_BYTES.checked_add(INCARICO_BYTES) {
    Some(totale) => totale,
    // Irraggiungibile: la somma di due costanti note. `panic!` in contesto
    // costante e' un errore di compilazione, non un panico a runtime.
    None => panic!("traboccamento nella derivazione di MAX_PROTOCOL_FRAME_BYTES"),
};

// ---------------------------------------------------------------------------
// Il writer limitato
// ---------------------------------------------------------------------------

/// Buffer che **si ferma** appena supera il tetto.
///
/// Non «scrivi tutto e poi controlla»: la differenza e' che qui il byte
/// oltre il tetto non viene mai allocato.
pub struct ScrittoreLimitato {
    buffer: Vec<u8>,
    tetto: usize,
}

impl ScrittoreLimitato {
    /// Un writer che accetta al piu' `tetto` byte.
    #[must_use]
    pub const fn nuovo(tetto: usize) -> Self {
        Self {
            buffer: Vec::new(),
            tetto,
        }
    }

    /// I byte scritti.
    #[must_use]
    pub fn in_byte(self) -> Vec<u8> {
        self.buffer
    }
}

impl Write for ScrittoreLimitato {
    fn write(&mut self, blocco: &[u8]) -> io::Result<usize> {
        let dopo = self
            .buffer
            .len()
            .checked_add(blocco.len())
            .ok_or_else(|| io::Error::other("traboccamento nella lunghezza del frame"))?;
        if dopo > self.tetto {
            return Err(io::Error::other(format!(
                "frame oltre il tetto: {dopo} byte > {}",
                self.tetto
            )));
        }
        self.buffer.extend_from_slice(blocco);
        Ok(blocco.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Codifica
// ---------------------------------------------------------------------------

const fn errore(messaggio: String) -> PlenoraError {
    PlenoraError::Protocol(messaggio)
}

/// Serializza un frame nella forma con prefisso di lunghezza.
///
/// # Errors
///
/// `PlenoraError::Protocol` se il frame viola un tetto strutturale o se la
/// serializzazione supera [`MAX_PROTOCOL_FRAME_BYTES`].
pub fn codifica(frame: &Frame) -> Result<Vec<u8>> {
    verifica_forma(frame)?;
    let mut scrittore = ScrittoreLimitato::nuovo(MAX_PROTOCOL_FRAME_BYTES);
    serde_json::to_writer(&mut scrittore, frame)
        .map_err(|origine| errore(format!("serializzazione del frame fallita: {origine}")))?;
    let payload = scrittore.in_byte();
    let lunghezza = u32::try_from(payload.len())
        .map_err(|_| errore("lunghezza del frame fuori da u32".to_owned()))?;
    let mut fuori = Vec::with_capacity(BYTE_PREFISSO + payload.len());
    fuori.extend_from_slice(&lunghezza.to_be_bytes());
    fuori.extend_from_slice(&payload);
    Ok(fuori)
}

// ---------------------------------------------------------------------------
// Decodifica
// ---------------------------------------------------------------------------

/// La lunghezza dichiarata dal prefisso, **se sta sotto il tetto**.
///
/// Esiste separata da [`decodifica`] per una ragione precisa, e va detta senza
/// abbellirla: `decodifica` riceve una slice, quindi quando viene chiamata i
/// byte del payload **qualcuno li ha gia' letti**. Il suo rifiuto e' sincero
/// su di se' — non alloca e non guarda oltre i quattro byte — ma non impedisce
/// da solo che un frame ostile faccia leggere un gigabyte da un pipe.
///
/// Quella difesa appartiene al lettore, ed e' questa funzione che gliela da':
/// prende i quattro byte, decide, e solo se dice `Ok` il lettore ne legge
/// altri. Il lettore vero e' di `PR-5`; qui c'e' il predicato su cui si
/// appoggera', gia' provato e gia' unico — `decodifica` usa **questo**
/// confronto, non una copia.
///
/// # Errors
///
/// `PlenoraError::Protocol` se la lunghezza dichiarata supera
/// [`MAX_PROTOCOL_FRAME_BYTES`].
pub fn lunghezza_dichiarata(prefisso: [u8; BYTE_PREFISSO]) -> Result<usize> {
    let dichiarata = u32::from_be_bytes(prefisso) as usize;
    if dichiarata > MAX_PROTOCOL_FRAME_BYTES {
        return Err(errore(format!(
            "frame dichiarato oltre il tetto: {dichiarata} byte > {MAX_PROTOCOL_FRAME_BYTES}, \
             rifiutato senza leggere il payload"
        )));
    }
    Ok(dichiarata)
}

/// Legge un frame.
///
/// # Errors
///
/// `PlenoraError::Protocol` per prefisso troncato, lunghezza incoerente,
/// lunghezza oltre il tetto, payload troncato, byte in eccesso, UTF-8 o JSON
/// invalido, chiavi duplicate, campi mancanti o sconosciuti, versione o tipo
/// non riconosciuti, valori fuori dominio.
pub fn decodifica(byte: &[u8]) -> Result<Frame> {
    let Some(prefisso) = byte.get(..BYTE_PREFISSO) else {
        return Err(errore(format!(
            "prefisso troncato: {} byte, ne servono {BYTE_PREFISSO}",
            byte.len()
        )));
    };
    let mut quattro = [0_u8; BYTE_PREFISSO];
    quattro.copy_from_slice(prefisso);
    // Il tetto PRIMA di guardare il payload, dalla stessa funzione che
    // usera' il lettore del pipe: una seconda copia del confronto sarebbe un
    // secondo confronto, e potrebbe divergere.
    let dichiarata = lunghezza_dichiarata(quattro)?;

    let disponibili = byte.len() - BYTE_PREFISSO;
    if disponibili < dichiarata {
        return Err(errore(format!(
            "payload troncato: dichiarati {dichiarata} byte, presenti {disponibili}"
        )));
    }
    if disponibili > dichiarata {
        return Err(errore(format!(
            "byte in eccesso dopo il frame: {} oltre i {dichiarata} dichiarati",
            disponibili - dichiarata
        )));
    }
    let payload = &byte[BYTE_PREFISSO..];
    let testo = std::str::from_utf8(payload)
        .map_err(|origine| errore(format!("payload non UTF-8: {origine}")))?;
    // Le chiavi duplicate si rifiutano PRIMA della deserializzazione:
    // `serde_json` le risolverebbe con «vince l'ultima», e due frame diversi
    // diventerebbero lo stesso messaggio.
    plenora_core::json::ensure_no_duplicate_keys(testo)
        .map_err(|origine| errore(format!("chiavi duplicate nel frame: {origine}")))?;
    let involucro: Involucro = serde_json::from_str(testo)
        .map_err(|origine| errore(format!("involucro malformato: {origine}")))?;

    // La versione **prima** del corpo: se non e' la nostra, il corpo non ha
    // una forma di cui possiamo dire nulla, e provare a leggerlo produrrebbe
    // un errore che parla del campo sbagliato.
    if involucro.protocol_version != VERSIONE_PROTOCOLLO {
        return Err(errore(format!(
            "versione del protocollo {} non riconosciuta; questa e' {VERSIONE_PROTOCOLLO}",
            involucro.protocol_version
        )));
    }

    let grezzo = involucro.corpo.get();
    let corpo = match involucro.tipo {
        TipoMessaggio::Saluto => Corpo::Saluto(Box::new(corpo_di(grezzo, "saluto")?)),
        TipoMessaggio::Incarico => Corpo::Incarico(Box::new(corpo_di(grezzo, "incarico")?)),
        TipoMessaggio::Annulla => Corpo::Annulla(corpo_di(grezzo, "annulla")?),
        TipoMessaggio::Risposta => Corpo::Risposta(Box::new(corpo_di(grezzo, "risposta")?)),
        TipoMessaggio::Progresso => Corpo::Progresso(corpo_di(grezzo, "progresso")?),
        TipoMessaggio::Esito => Corpo::Esito(Box::new(corpo_di(grezzo, "esito")?)),
    };

    let frame = Frame {
        protocol_version: involucro.protocol_version,
        tipo: involucro.tipo,
        corpo,
    };
    verifica_forma(&frame)?;
    Ok(frame)
}

/// L'involucro letto per primo, col corpo ancora **non interpretato**.
///
/// `deny_unknown_fields` vale gia' qui: un quarto campo accanto a
/// `protocol_version`, `tipo` e `corpo` e' un errore prima ancora che si sappia
/// che messaggio sia.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Involucro<'a> {
    protocol_version: u16,
    tipo: TipoMessaggio,
    #[serde(borrow)]
    corpo: &'a serde_json::value::RawValue,
}

fn corpo_di<T: serde::de::DeserializeOwned>(grezzo: &str, tipo: &str) -> Result<T> {
    serde_json::from_str(grezzo)
        .map_err(|origine| errore(format!("corpo di `{tipo}` malformato: {origine}")))
}

// ---------------------------------------------------------------------------
// Verifica dei tetti strutturali
// ---------------------------------------------------------------------------

fn limita(valore: &str, tetto: usize, campo: &str) -> Result<()> {
    if valore.len() > tetto {
        return Err(errore(format!(
            "`{campo}` oltre il tetto: {} byte > {tetto}",
            valore.len()
        )));
    }
    Ok(())
}

fn limita_elementi<T>(elementi: &[T], tetto: usize, campo: &str) -> Result<()> {
    if elementi.len() > tetto {
        return Err(errore(format!(
            "`{campo}` oltre il tetto: {} elementi > {tetto}",
            elementi.len()
        )));
    }
    Ok(())
}

/// I tetti per campo, applicati in entrambi i versi.
///
/// In scrittura impediscono di produrre un frame che il ricevente
/// rifiuterebbe; in lettura sono la difesa. Applicarli da un lato solo
/// avrebbe reso il protocollo asimmetrico proprio dove i due lati devono
/// concordare.
fn verifica_forma(frame: &Frame) -> Result<()> {
    match &frame.corpo {
        Corpo::Saluto(saluto) => {
            limita(
                &saluto.artefatto.digest,
                MAX_DIGEST_BYTES,
                "artefatto.digest",
            )?;
            limita(
                &saluto.artefatto.versione,
                MAX_VERSIONE_BYTES,
                "artefatto.versione",
            )?;
            limita(
                &saluto.resolver.identita,
                MAX_IDENTIFICATORE_BYTES,
                "resolver.identita",
            )?;
            limita(
                &saluto.resolver.versione,
                MAX_VERSIONE_BYTES,
                "resolver.versione",
            )?;
            limita(&saluto.commit_token, MAX_DIGEST_BYTES, "commit_token")?;
            verifica_ambiente(&saluto.ambiente)
        }
        Corpo::Incarico(incarico) => {
            limita(
                &incarico.plan_hash_atteso,
                MAX_DIGEST_BYTES,
                "plan_hash_atteso",
            )?;
            limita(
                &incarico.artefatto_temporaneo,
                MAX_PERCORSO_BYTES,
                "artefatto_temporaneo",
            )?;
            limita_elementi(&incarico.ingressi, MAX_INGRESSI, "ingressi")?;
            for ingresso in &incarico.ingressi {
                limita(&ingresso.nome, MAX_IDENTIFICATORE_BYTES, "ingresso.nome")?;
                limita(&ingresso.percorso, MAX_PERCORSO_BYTES, "ingresso.percorso")?;
                limita(
                    &ingresso.contract_fingerprint_atteso,
                    MAX_DIGEST_BYTES,
                    "ingresso.contract_fingerprint_atteso",
                )?;
            }
            // Il piano e' JSON grezzo: i byte che si misurano qui sono
            // esattamente quelli che il writer emettera'. Con un `Value` si
            // sarebbe misurata una serializzazione e spedita un'altra.
            let byte_piano = incarico.piano_canonico.get().len();
            if byte_piano > MAX_PIANO_CANONICO_BYTES {
                return Err(errore(format!(
                    "`piano_canonico` oltre il tetto: {byte_piano} byte > \
                     {MAX_PIANO_CANONICO_BYTES}"
                )));
            }
            Ok(())
        }
        Corpo::Annulla(annulla) => limita(&annulla.motivo, MAX_MOTIVO_BYTES, "motivo"),
        Corpo::Risposta(risposta) => {
            limita(
                &risposta.artefatto.digest,
                MAX_DIGEST_BYTES,
                "artefatto.digest",
            )?;
            limita(
                &risposta.artefatto.versione,
                MAX_VERSIONE_BYTES,
                "artefatto.versione",
            )?;
            limita(
                &risposta.resolver.identita,
                MAX_IDENTIFICATORE_BYTES,
                "resolver.identita",
            )?;
            limita(
                &risposta.resolver.versione,
                MAX_VERSIONE_BYTES,
                "resolver.versione",
            )?;
            limita_elementi(&risposta.capability, MAX_CAPABILITY, "capability")?;
            for capability in &risposta.capability {
                // Il nome dice **elemento**: con la sola parola «capability»
                // l'errore non distinguerebbe «troppe capability» da «una
                // capability troppo lunga», e sono due difetti diversi.
                limita(
                    capability,
                    MAX_IDENTIFICATORE_BYTES,
                    "capability (elemento)",
                )?;
            }
            verifica_ambiente(&risposta.ambiente)
        }
        // Contatori: nessun tetto oltre al tipo.
        Corpo::Progresso(_) => Ok(()),
        Corpo::Esito(esito) => verifica_esito(esito),
    }
}

fn verifica_ambiente(ambiente: &super::messaggi::Ambiente) -> Result<()> {
    limita(&ambiente.digest_insieme, MAX_DIGEST_BYTES, "digest_insieme")?;
    limita_elementi(&ambiente.risorse, MAX_RISORSE, "risorse")?;
    for risorsa in &ambiente.risorse {
        limita(&risorsa.nome, MAX_IDENTIFICATORE_BYTES, "risorsa.nome")?;
        limita(&risorsa.versione, MAX_VERSIONE_BYTES, "risorsa.versione")?;
        limita(&risorsa.percorso, MAX_PERCORSO_BYTES, "risorsa.percorso")?;
    }
    limita_elementi(
        &ambiente.backend_dinamici,
        MAX_BACKEND_DINAMICI,
        "backend_dinamici",
    )?;
    for backend in &ambiente.backend_dinamici {
        limita(&backend.nome, MAX_IDENTIFICATORE_BYTES, "backend.nome")?;
        limita(&backend.versione, MAX_VERSIONE_BYTES, "backend.versione")?;
        limita(&backend.percorso, MAX_PERCORSO_BYTES, "backend.percorso")?;
    }
    Ok(())
}

fn verifica_esito(esito: &super::messaggi::EsitoWorkerSulFilo) -> Result<()> {
    match esito {
        super::messaggi::EsitoWorkerSulFilo::Successo { digest_artefatto } => {
            limita(
                &digest_artefatto.algoritmo,
                MAX_IDENTIFICATORE_BYTES,
                "digest_artefatto.algoritmo",
            )?;
            limita(
                &digest_artefatto.valore,
                MAX_DIGEST_BYTES,
                "digest_artefatto.valore",
            )
        }
        super::messaggi::EsitoWorkerSulFilo::Errore { errore: dentro } => {
            limita(&dentro.messaggio, MAX_MESSAGGIO_BYTES, "messaggio")?;
            for (campo, valore) in [
                ("nodo", &dentro.nodo),
                ("operazione", &dentro.operazione),
                ("execution_id", &dentro.execution_id),
            ] {
                if let Some(valore) = valore {
                    limita(valore, MAX_IDENTIFICATORE_BYTES, campo)?;
                }
            }
            dentro
                .diagnostica
                .as_ref()
                .map_or(Ok(()), verifica_diagnostica)
        }
        // La forma del panico e' un enum chiuso: non c'e' lunghezza da
        // limitare, ed e' precisamente il punto.
        super::messaggi::EsitoWorkerSulFilo::Panic { .. } => Ok(()),
    }
}

fn verifica_diagnostica(diagnostica: &super::messaggi::DiagnosticaSulFilo) -> Result<()> {
    limita(
        &diagnostica.contract,
        MAX_IDENTIFICATORE_BYTES,
        "diagnostica.contract",
    )?;
    limita(
        &diagnostica.scope,
        MAX_IDENTIFICATORE_BYTES,
        "diagnostica.scope",
    )?;
    limita(
        &diagnostica.completeness,
        MAX_IDENTIFICATORE_BYTES,
        "diagnostica.completeness",
    )?;
    limita_elementi(
        &diagnostica.conteggi,
        MAX_CONTEGGI_DIAGNOSTICA,
        "diagnostica.conteggi",
    )?;
    for (chiave, _) in &diagnostica.conteggi {
        limita(chiave, MAX_CHIAVE_CONTEGGIO_BYTES, "chiave di conteggio")?;
    }
    limita_elementi(
        &diagnostica.esempi,
        MAX_ESEMPI_DIAGNOSTICA,
        "diagnostica.esempi",
    )?;
    for esempio in &diagnostica.esempi {
        limita(&esempio.codice, MAX_ESEMPIO_BYTES, "esempio.codice")?;
    }
    Ok(())
}
