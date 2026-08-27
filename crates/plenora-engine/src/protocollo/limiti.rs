//! I tetti del protocollo: **interni, non configurabili, non negoziabili**.
//!
//! «Un protocollo interno con limiti negoziabili e' un protocollo con una
//! superficie d'attacco negoziabile» (§4.1 di `isolamento.md`). Nessuna di
//! queste costanti e' un campo di configurazione, e nessuna viaggia sul filo
//! come valore modificabile.
//!
//! # Non sono i default del piano
//!
//! `PlanLimits::max_plan_json_bytes`, `max_inputs` e `max_identifier_bytes`
//! sono **default ampliabili** dalla policy di chi esegue: un chiamante puo'
//! dichiararne di piu' larghi, e un piano che li sfrutta resta valido.
//!
//! Qui invece sono **tetti del profilo isolato**. Ne segue una conseguenza
//! che va detta e non scoperta: un piano valido sotto una policy piu' larga
//! puo' essere **non isolabile**, e deve ricevere un rifiuto esplicito che lo
//! dica — non un errore di serializzazione.

/// Byte della forma canonica di un piano che il protocollo trasporta.
///
/// **Limite di policy del profilo isolato, non massimo universale.** La
/// calibrazione (`examples/calibra_canonico.rs`) misura il rapporto fra testo
/// e canonico su piani costruiti per massimizzarlo: il caso peggiore
/// osservato porta un testo al tetto a ~54,3 MiB, e questo limite lascia il
/// 17,8 % di margine sulla proiezione.
///
/// Nessun insieme finito di piani dimostra quel rapporto per **tutti** i
/// documenti validi, quindi il presidio non e' il numero: e' il writer
/// limitato, che si ferma appena lo supera invece di scoprirlo dopo.
pub const MAX_PIANO_CANONICO_BYTES: usize = 64 * 1024 * 1024;

/// Ingressi che un `Incarico` puo' descrivere.
pub const MAX_INGRESSI: usize = 16;

/// Byte **decodificati** di un identificatore: nome di ingresso, di nodo, di
/// risorsa.
pub const MAX_IDENTIFICATORE_BYTES: usize = 256;

/// Byte **decodificati** di un percorso.
pub const MAX_PERCORSO_BYTES: usize = 4_096;

/// Caratteri di un digest nella forma sul filo: l'esadecimale minuscolo dei
/// 32 byte di uno SHA-256.
///
/// **Derivata, non scelta**: l'autorita' e' `digest::DIGEST_BYTES`, che e' il
/// numero che non puo' cambiare senza cambiare algoritmo. Scriverne uno qui e
/// uno la' sarebbero due autorita' che possono divergere.
///
/// Non e' un tetto come gli altri: la forma canonica pretende **esattamente**
/// questa lunghezza, non «al piu'». E non c'e' un controllo che la applichi —
/// c'e' un **tipo**, `digest::DigestSha256`, di cui non esiste un valore non
/// canonico. Questa costante resta perche' la derivazione del tetto del frame
/// ha bisogno di un numero.
///
/// Essendo ASCII, caratteri e byte coincidono: il nome dice `BYTES` perche' e'
/// nella famiglia dei tetti per campo, che si applicano ai byte decodificati.
pub const MAX_DIGEST_BYTES: usize = super::digest::DIGEST_BYTES * 2;

/// Byte **decodificati** di una versione dichiarata.
pub const MAX_VERSIONE_BYTES: usize = 64;

/// Capability che una `Risposta` puo' dichiarare.
pub const MAX_CAPABILITY: usize = 32;

/// Risorse risolte che l'handshake puo' elencare.
pub const MAX_RISORSE: usize = 64;

/// Backend dinamici che l'handshake puo' elencare.
pub const MAX_BACKEND_DINAMICI: usize = 16;

/// Byte **decodificati** del motivo di un `Annulla`.
pub const MAX_MOTIVO_BYTES: usize = 1_024;

/// Byte **decodificati** del messaggio sanitizzato di un errore.
pub const MAX_MESSAGGIO_BYTES: usize = 4_096;

/// Voci di conteggio nella diagnostica di riga.
pub const MAX_CONTEGGI_DIAGNOSTICA: usize = 64;

/// Byte **decodificati** di una chiave di conteggio.
pub const MAX_CHIAVE_CONTEGGIO_BYTES: usize = 128;

/// Esempi che la diagnostica di riga puo' portare.
pub const MAX_ESEMPI_DIAGNOSTICA: usize = 32;

/// Byte **decodificati** di un esempio.
pub const MAX_ESEMPIO_BYTES: usize = 512;

/// Messaggi che il supervisore puo' inviare al worker.
///
/// `Saluto`, `Incarico`, e al piu' un `Annulla`. Non c'e' ragione perche' ne
/// esistano altri.
pub const MAX_MESSAGGI_VERSO_WORKER: usize = 3;

/// Messaggi che il worker puo' inviare al supervisore.
///
/// `Risposta` + `Esito` + al piu' [`MAX_PROGRESSO`] messaggi di progresso.
pub const MAX_MESSAGGI_VERSO_SUPERVISORE: usize = 2 + MAX_PROGRESSO;

/// Quota **totale di emissione** dei messaggi di progresso.
///
/// Esaurita la quota il worker **smette di emetterli e continua a
/// lavorare**: il progresso e' facoltativo e il supervisore non ne dipende.
/// Riceverne uno oltre la quota e' invece una violazione del protocollo — e
/// va distinta, perche' significa che l'altro capo non rispetta il proprio
/// contatore.
///
/// Non e' un tetto sulla frequenza: quello e' meccanismo, e appartiene al
/// supervisore.
pub const MAX_PROGRESSO: usize = 1_024;

/// Espansione massima di un byte quando JSON lo codifica come escape.
///
/// Un carattere di controllo diventa `\uXXXX`, sei byte. E' una
/// maggiorazione sicura applicata ai campi **stringa**: il piano canonico
/// viaggia come valore JSON grezzo, quindi non la subisce.
pub const ESPANSIONE_ESCAPE: usize = 6;
