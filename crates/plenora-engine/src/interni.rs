//! Facciata **minima** per i due consumatori fuori dal crate.
//!
//! # Superficie pubblica instabile e non-production
//!
//! Questo modulo esiste solo con la feature `internals`, che non e' nel
//! `default` e che **nessun consumatore di produzione** abilita: gli unici a
//! farlo sono il crate `fuzz/` e la sonda di calibrazione, che sono strumenti
//! di verifica di questo repository. Non ha garanzie di stabilita' e non e'
//! pensato per essere usato in produzione: puo' cambiare o sparire senza
//! preavviso.
//!
//! # Perche' una facciata e non il modulo
//!
//! Esporre `pub mod protocollo` sotto la stessa feature lo renderebbe API
//! pubblica a tutti gli effetti ogni volta che la feature e' attiva — e il
//! crate `fuzz/` e' precisamente un consumatore che la attiva. «Privato
//! tranne che per chi lo usa» non e' privato.
//!
//! Qui invece esce un verdetto per confine e nessun DTO del protocollo:
//!
//! - [`verifica_giro_del_frame`], che esercita codificatore e decodificatore
//!   **dall'interno** e restituisce un verdetto, non una struttura;
//! - [`verifica_lettore_frame_geo`], che esercita il lettore dello stream
//!   `PLNGEO2` e rende un verdetto, non i frame letti;
//! - [`verifica_artefatto_ostile`], che esercita il verificatore
//!   dell'artefatto su byte arbitrari e rende accettato/rifiutato, con
//!   [`artefatto_di_prova`], [`schema_di_prova`] e [`TOKEN_DI_PROVA`] a
//!   descrivere le attese che quel verdetto presuppone;
//! - [`MAX_PIANO_CANONICO_BYTES`], l'unico limite che la sonda di
//!   calibrazione deve leggere.
//!
//! Il vantaggio non e' solo di superficie: scritte qui, le invarianti del
//! fuzzer stanno **dentro** il crate, quindi le compila e le controlla la
//! build normale invece della sola toolchain nightly.

use crate::protocollo::codifica::{codifica, decodifica, BYTE_PREFISSO, MAX_PROTOCOL_FRAME_BYTES};

/// Il tetto della forma canonica di un piano nel profilo isolato.
///
/// Ri-esportato e non ridefinito: due copie dello stesso numero sono due
/// numeri che possono divergere.
pub const MAX_PIANO_CANONICO_BYTES: usize = crate::protocollo::limiti::MAX_PIANO_CANONICO_BYTES;

/// Esercita il giro completo su byte arbitrari e rende un verdetto.
///
/// Un ingresso che non e' un frame **non e' un guasto**: la gran parte di
/// cio' che produce un fuzzer non lo e', e restituisce `Ok(())`. Sono guasti
/// solo le rotture d'invariante.
///
/// # Errors
///
/// Descrive quale invariante e' saltata:
///
/// - un frame decodificato che non si ricodifica;
/// - una forma canonica oltre [`MAX_PROTOCOL_FRAME_BYTES`];
/// - un prefisso che non dichiara i byte del payload;
/// - una forma canonica che non si rilegge, o che rilegge un'altra struttura;
/// - una seconda codifica diversa dalla prima.
///
/// Le ultime due sono la difesa contro la deriva silenziosa fra i due versi:
/// un decoder che accettasse qualcosa che il codificatore non sa riprodurre
/// romperebbe il giro senza che nessun test nominale se ne accorga.
pub fn verifica_giro_del_frame(byte: &[u8]) -> Result<(), String> {
    let Ok(frame) = decodifica(byte) else {
        return Ok(());
    };

    let canonico = codifica(&frame)
        .map_err(|origine| format!("un frame decodificato non si ricodifica: {origine}"))?;
    let payload = canonico
        .len()
        .checked_sub(BYTE_PREFISSO)
        .ok_or_else(|| "il frame codificato non contiene il prefisso".to_owned())?;
    if payload > MAX_PROTOCOL_FRAME_BYTES {
        return Err(format!(
            "la forma canonica supera il tetto: {payload} byte > {MAX_PROTOCOL_FRAME_BYTES}"
        ));
    }

    let dichiarata =
        u32::from_be_bytes([canonico[0], canonico[1], canonico[2], canonico[3]]) as usize;
    if dichiarata != payload {
        return Err(format!(
            "il prefisso dichiara {dichiarata} byte, il payload ne ha {payload}"
        ));
    }

    let riletto = decodifica(&canonico)
        .map_err(|origine| format!("la forma canonica non si rilegge: {origine}"))?;
    if riletto != frame {
        return Err("il giro non rende la stessa struttura".to_owned());
    }
    let di_nuovo =
        codifica(&riletto).map_err(|origine| format!("la ricodifica fallisce: {origine}"))?;
    if di_nuovo != canonico {
        return Err("codifica non deterministica".to_owned());
    }
    Ok(())
}

/// La richiesta di lettura piu' grande che il lettore dei frame puo' fare.
///
/// E' il buffer fisso di `geo_transport::protocol`, non ri-derivato: quel
/// buffer e' privato al modulo, e questo numero e' la promessa che sorveglia,
/// non una seconda definizione della stessa cosa. Se il buffer cresce senza
/// che cresca anche questo, il gate diventa piu' severo del codice — cioe'
/// rosso, che e' il verso giusto in cui sbagliare.
const RICHIESTA_MASSIMA_AMMESSA: usize = 16 * 1024;

/// Sorgente in memoria che **registra la fetta piu' grande** che le viene
/// passata.
///
/// La misura non e' lo heap: e' la taglia che il lettore chiede a
/// `Read::read`. E' cio' che distingue un lettore incrementale da uno che
/// dimensiona sul dichiarato, ed e' osservabile senza agganciare l'allocatore.
struct SorgenteSorvegliata<'a> {
    byte: &'a [u8],
    letto: usize,
    massima: &'a std::cell::Cell<usize>,
}

impl std::io::Read for SorgenteSorvegliata<'_> {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        self.massima.set(self.massima.get().max(out.len()));
        let resto = self.byte.len() - self.letto;
        let quanti = resto.min(out.len());
        out[..quanti].copy_from_slice(&self.byte[self.letto..self.letto + quanti]);
        self.letto += quanti;
        Ok(quanti)
    }
}

/// Esaurisce il lettore dello stream `PLNGEO2` su byte arbitrari e rende un
/// verdetto.
///
/// `schema_rows` e' un ingresso del chiamante, non dello stream: il lettore
/// pretende che coincida con il contatore dichiarato nell'header, quindi
/// arriva come parametro e permette di esercitare sia il ramo che accetta sia
/// quello che rifiuta.
///
/// Un ingresso che non e' uno stream **non e' un guasto**: la gran parte di
/// cio' che produce un fuzzer non lo e', e restituisce `Ok(())`.
///
/// # Errors
///
/// Descrive quale invariante e' saltata:
///
/// - una lettura ha chiesto piu' di [`RICHIESTA_MASSIMA_AMMESSA`];
/// - un frame reso porta piu' byte di quanti la sorgente ne contenesse;
/// - la somma dei frame resi supera i byte della sorgente;
/// - il lettore rende ancora un frame dopo aver dichiarato la fine.
///
/// La prima e' la difesa sull'**amplificazione**, ed e' l'unica che la
/// sorveglia: `length` arriva dallo stream e puo' dichiarare fino a
/// `MAX_GEOMETRY_BYTES`, ma la fetta passata a `Read::read` resta il buffer
/// fisso. Un lettore che dimensionasse sul dichiarato la romperebbe **anche
/// quando poi fallisce**, perche' la richiesta precede l'EOF che la delude:
/// per questo il controllo si fa alla fine, sul massimo osservato, e vale su
/// tutti i percorsi compresi quelli d'errore.
///
/// Non misura lo heap. Misura la taglia richiesta, che e' cio' che il lettore
/// decide; quanto l'allocatore poi tocchi non e' una scelta di questo codice.
///
/// La seconda e la terza sono un'altra promessa: un frame accettato non
/// materializza byte che la sorgente non ha consegnato. Proteggono dalla
/// duplicazione, non dall'allocazione anticipata, e le due cose restano
/// distinte.
///
/// La quarta chiude il ciclo: un lettore che tornasse a rendere frame dopo
/// `Ok(None)` non avrebbe una fine, e la campagna girerebbe per sempre su un
/// ingresso da poche decine di byte.
pub fn verifica_lettore_frame_geo(byte: &[u8], schema_rows: u64) -> Result<(), String> {
    let massima = std::cell::Cell::new(0_usize);
    let esito = esaurisci_lo_stream(
        SorgenteSorvegliata {
            byte,
            letto: 0,
            massima: &massima,
        },
        byte.len(),
        schema_rows,
    );

    // Prima della promessa sui frame, e fuori dal ramo che li legge: una
    // lettura sovradimensionata e' un guasto anche quando lo stream finisce
    // male, ed e' proprio li' che un lettore dimensionato sul dichiarato
    // sfuggirebbe a un controllo fatto sul solo esito riuscito.
    if massima.get() > RICHIESTA_MASSIMA_AMMESSA {
        return Err(format!(
            "una lettura ha chiesto {} byte, oltre i {RICHIESTA_MASSIMA_AMMESSA} della finestra fissa",
            massima.get()
        ));
    }
    esito
}

/// La fetta piu' grande che il lettore ha chiesto su questo ingresso.
///
/// Esiste per mostrare che [`verifica_lettore_frame_geo`] **non e' vacuo**.
/// «Nessuna lettura oltre 16 KiB» sarebbe vero anche se nessuna lettura
/// avvenisse, e un oracolo che non puo' fallire non e' un oracolo: un test lo
/// interroga sul caso che dichiara molto e consegna poco, e pretende di
/// vedere il buffer fisso al posto della lunghezza dichiarata.
#[cfg(test)]
#[must_use]
pub fn massima_richiesta_di_lettura(byte: &[u8], schema_rows: u64) -> usize {
    let massima = std::cell::Cell::new(0_usize);
    let _ = esaurisci_lo_stream(
        SorgenteSorvegliata {
            byte,
            letto: 0,
            massima: &massima,
        },
        byte.len(),
        schema_rows,
    );
    massima.get()
}

/// Il giro sui frame, separato perche' il suo esito non e' l'ultima parola:
/// la sorveglianza sulle letture vale anche quando questo rende `Ok`.
fn esaurisci_lo_stream(
    sorgente: SorgenteSorvegliata<'_>,
    disponibili: usize,
    schema_rows: u64,
) -> Result<(), String> {
    use crate::geo_transport::protocol::{Frame, FrameReader};

    let Ok(mut lettore) = FrameReader::new(sorgente, schema_rows) else {
        return Ok(());
    };

    let mut resi: usize = 0;
    loop {
        match lettore.next_frame() {
            Ok(Some(Frame::Wkb(payload))) => {
                if payload.len() > disponibili {
                    return Err(format!(
                        "un frame reso porta {} byte, la sorgente ne conteneva {disponibili}",
                        payload.len()
                    ));
                }
                resi += payload.len();
                if resi > disponibili {
                    return Err(format!(
                        "i frame resi sommano {resi} byte, la sorgente ne conteneva {disponibili}"
                    ));
                }
            }
            Ok(Some(Frame::Null)) => {}
            Ok(None) => break,
            Err(_) => return Ok(()),
        }
    }

    match lettore.next_frame() {
        Ok(None) | Err(_) => Ok(()),
        Ok(Some(_)) => Err("il lettore rende un frame dopo aver dichiarato la fine".to_owned()),
    }
}

/// Esercita il **verificatore dell'artefatto** su byte arbitrari e rende un
/// verdetto.
///
/// # L'invariante che sorveglia
///
/// Byte arbitrari non sono quasi mai un artefatto valido, quindi il caso
/// ordinario e' un rifiuto: rifiutare **non e' un guasto**, ed e' la ragione
/// per cui un verificatore che respinge rende `Ok(false)` e non un errore. Il
/// guasto e' uno solo: che il verificatore **panichi** invece di concludere.
///
/// Il panico non si intercetta qui — `libfuzzer` lo rileva da se', e prenderlo
/// significherebbe nasconderlo — quindi la funzione non ha nulla da
/// restituire quando tutto va come deve. Il valore del target sta nel
/// percorso che attraversa: framing, footer, tetto sui dizionari, digest,
/// schema, contratto, conteggi e token, tutti sotto la barriera anti-panico
/// del confine.
///
/// # Perche' non sorveglia anche l'eco dei dati nell'errore
///
/// La regola «errori senza dati» vale, e la prova sta nella suite, dove i
/// valori delle celle sono sentinelle riconoscibili. Un fuzzer non puo'
/// esercitarla: cercare una finestra dell'ingresso nel testo dell'errore
/// segnala anche quando l'ingresso **coincide** con un nostro messaggio
/// statico, e i nostri messaggi sono testo noto e breve. Un ingresso costruito
/// per combaciare con essi fa scattare la guardia senza che nulla sia stato
/// riecheggiato, e una guardia che un avversario sa far scattare a comando non
/// sorveglia niente.
///
/// # Che cosa rende
///
/// `Ok(true)` se il verificatore ha **accettato** l'artefatto, `Ok(false)` se
/// lo ha **rifiutato**. Entrambi sono esiti legittimi da byte arbitrari, e il
/// target non pretende nessuno dei due; ma l'esito dev'essere **osservabile**,
/// altrimenti nulla puo' provare che l'harness sia in grado di arrivare in
/// fondo — e un harness che si ferma sempre al primo passo passerebbe per
/// funzionante.
///
/// # Errors
///
/// Mai per un ingresso rifiutato, che e' `Ok(false)`. Due famiglie:
///
/// - guasti dell'**harness**: workspace non raggiungibile o gia' in uso,
///   apertura, scrittura, svuotamento, token di prova non canonico;
/// - esiti del **verificatore** che non sono rifiuti: un errore di categoria
///   `io`, che dice dell'ambiente e non dell'ingresso, e qualunque categoria
///   che il contratto di `verifica_artefatto` non dichiari. La regola completa
///   e' in [`classifica_esito`].
pub fn verifica_artefatto_ostile(byte: &[u8]) -> Result<bool, String> {
    use plenora_core::contract::DataContract;

    use crate::commit_token::CommitToken;
    use crate::geo_transport::ipc::IpcLimits;
    use crate::protocollo::messaggi::{ConteggiDichiarati, DigestArtefatto};
    use crate::verifica::{verifica_artefatto, AtteseVerifica, ALGORITMO_DIGEST};

    // Il workspace e' **riusato**: una directory per thread, un file riscritto.
    let percorso = scrivi_nel_workspace(byte)?;

    // Attese fissate: quale sia il contratto atteso non cambia cio' che questa
    // prova sorveglia, e sceglierne uno stabile rende il percorso riproducibile
    // a parita' di ingresso. Lo schema viene da [`schema_di_prova`], che e' lo
    // stesso da cui nasce [`artefatto_di_prova`]: due definizioni separate
    // potrebbero divergere, e la prova che l'harness arriva in fondo si
    // spegnerebbe senza che nessun test lo dica.
    let contratto = DataContract::tabular(schema_di_prova());
    // Il digest e' quello VERO dei byte in prova. Con un valore fisso il passo
    // 5-bis fallirebbe sempre, e il fuzzer non arriverebbe mai a schema,
    // contratto, conteggi e token: sarebbe un target che esercita i primi tre
    // passi e sembra esercitarli tutti.
    let valore = {
        use sha2::{Digest as _, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(byte);
        let grezzi: [u8; 32] = hasher.finalize().into();
        crate::esadecimale32::Esadecimale32::dai_byte(grezzi).in_esadecimale()
    };
    let digest = DigestArtefatto {
        algoritmo: ALGORITMO_DIGEST.to_owned(),
        valore,
    };
    let token = CommitToken::da_esadecimale(TOKEN_DI_PROVA)
        .map_err(|_| "harness: il token di prova non e' canonico".to_owned())?;
    let attese = AtteseVerifica {
        contratto: &contratto,
        digest: &digest,
        conteggi: ConteggiDichiarati { righe: 0, batch: 0 },
        commit_token: &token,
    };
    let limiti = IpcLimits {
        max_retained_dictionary_body_bytes: 1024 * 1024,
        ..IpcLimits::default()
    };

    // Accettare o rifiutare sono entrambi legittimi; l'esito si **classifica**
    // invece di appiattirlo, perche' non tutti gli errori sono rifiuti.
    classifica_esito(verifica_artefatto(
        &percorso,
        &attese,
        plenora_core::crs::resolve_crs,
        &limiti,
    ))
}

/// Traduce l'esito del verificatore nel verdetto del target.
///
/// # Perche' non basta `is_ok()`
///
/// Perche' appiattirebbe **ogni** errore su «rifiutato», e un errore di I/O —
/// il file che non si apre, il disco che non risponde — diventerebbe un
/// ingresso respinto. Il fuzzer registrerebbe copertura per un percorso che non
/// ha attraversato, e un ambiente rotto passerebbe per un milione di rifiuti
/// legittimi. E' la stessa classe di un harness che scarta l'esito: un
/// verdetto che non distingue non e' un verdetto.
///
/// La classificazione segue il contratto di `verifica_artefatto`, che dichiara
/// quali categorie puo' produrre:
///
/// | esito | verdetto |
/// |---|---|
/// | `Ok(())` | `Ok(true)`: accettato |
/// | `data_mapping`, `resource_limit` | `Ok(false)`: rifiutato, ed e' il caso ordinario |
/// | `io` | `Err`: guasto dell'harness o dell'ambiente, non dell'ingresso |
/// | qualunque altra | `Err`: il verificatore non la dichiara, quindi o il contratto e' cambiato o e' un difetto. In entrambi i casi tacere sarebbe peggio |
///
/// L'ultima riga e' la piu' importante: senza, una categoria nuova verrebbe
/// letta come rifiuto e nessuno se ne accorgerebbe.
///
/// # Errors
///
/// Sulle categorie `io` e su quelle non dichiarate dal contratto.
pub(crate) fn classifica_esito(esito: plenora_core::error::Result<()>) -> Result<bool, String> {
    use plenora_core::error::ErrorCategory;

    let Err(errore) = esito else {
        return Ok(true);
    };
    match errore.category() {
        ErrorCategory::DataMapping | ErrorCategory::ResourceLimit => Ok(false),
        ErrorCategory::Io => Err("harness: errore di I/O durante la verifica".to_owned()),
        // Il nome della categoria e' una stringa stabile del nostro canone,
        // non testo dell'ingresso: puo' entrare nel messaggio.
        altra => Err(format!(
            "harness: il verificatore ha reso una categoria non dichiarata dal suo contratto: {}",
            altra.as_str()
        )),
    }
}

thread_local! {
    /// Il workspace di questo thread: creato alla prima iterazione, riusato da
    /// tutte le successive.
    ///
    /// **Per thread e non globale.** `libfuzzer` puo' girare con piu' worker, e
    /// un percorso unico condiviso richiederebbe un lock: due iterazioni che
    /// riscrivono lo stesso file mentre il verificatore lo legge darebbero
    /// esiti che non appartengono a nessuno dei due ingressi. Una risorsa per
    /// thread toglie il problema invece di sincronizzarlo.
    static WORKSPACE: std::cell::RefCell<Option<(tempfile::TempDir, std::path::PathBuf)>> =
        const { std::cell::RefCell::new(None) };
}

/// Scrive i byte nel workspace del thread e ne rende il percorso.
///
/// # Perche' non una temp dir per iterazione
///
/// Perche' una campagna esegue milioni di casi, e creare e distruggere una
/// directory e un file per ciascuno e' traffico sul filesystem proporzionale
/// agli ingressi. Un workspace per thread lo rende **costante**, ed e' una
/// ragione che si regge da sola: meno lavoro per lo stesso risultato.
///
/// `scripts/fuzz-campaign.sh` tiene corpus e artifact in `tmpfs`, e i runner
/// impostano `TMPDIR` di conseguenza. La ragione documentata di quella scelta
/// e' **prudenziale, non causale**: durante le campagne su un host poi escluso
/// si sono osservati blocchi con una firma comune nel kernel — un task in
/// stato `D` dentro una `mmap` — e la causa prima **non e' stata attribuita**.
/// Ridurre l'I/O riduce una superficie sospetta; non e' dimostrato che sia
/// quella la causa, e scriverlo come se lo fosse sarebbe una spiegazione presa
/// per una prova. L'evidenza sta in `archivio-wedge`, e la regola sulle
/// campagne in release.md.
///
/// # Il troncamento non e' un dettaglio
///
/// `create(true).truncate(true)` azzera il file **prima** di scrivere: senza,
/// un ingresso corto lascerebbe in coda i byte di quello lungo che lo precede,
/// e il verificatore leggerebbe un artefatto che il fuzzer non ha mai
/// prodotto. Sarebbe un caso non riproducibile per costruzione.
///
/// # Errors
///
/// Ogni guasto e' un `Err` dell'harness, mai un rifiuto ordinario:
///
/// - il thread local **non e' raggiungibile**, perche' gia' distrutto;
/// - il workspace e' **gia' in prestito** su questo thread;
/// - creazione della directory, apertura, scrittura, svuotamento.
///
/// I primi due nascono dalle forme fallibili di `try_with` e `try_borrow_mut`:
/// con `with` e `borrow_mut` sarebbero panici, cioe' guasti dell'harness
/// indistinguibili da un crash del target.
pub(crate) fn scrivi_nel_workspace(byte: &[u8]) -> Result<std::path::PathBuf, String> {
    use std::io::Write as _;

    // `try_with` e `try_borrow_mut`, non `with` e `borrow_mut`: entrambe le
    // forme brevi panicano — la prima se il thread local e' gia' distrutto, la
    // seconda su un prestito in corso — e un panico qui sarebbe un guasto
    // dell'harness travestito da crash del target.
    WORKSPACE
        .try_with(|cella| {
            let mut cella = cella
                .try_borrow_mut()
                .map_err(|_| "harness: workspace gia' in uso su questo thread".to_owned())?;
            if cella.is_none() {
                let directory = tempfile::tempdir()
                    .map_err(|errore| format!("harness: workspace non creato: {errore}"))?;
                let percorso = directory.path().join("artefatto.arrow");
                *cella = Some((directory, percorso));
            }
            let (_, percorso) = cella
                .as_ref()
                .ok_or_else(|| "harness: workspace assente dopo l'inizializzazione".to_owned())?;

            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .open(percorso)
                .map_err(|errore| format!("harness: artefatto non aperto: {errore}"))?;
            file.write_all(byte)
                .map_err(|errore| format!("harness: artefatto non scritto: {errore}"))?;
            file.flush()
                .map_err(|errore| format!("harness: artefatto non svuotato: {errore}"))?;
            // Il writer si chiude **prima** che il verificatore riapra il percorso.
            // Su Windows un handle di scrittura ancora aperto puo' negare
            // l'apertura in lettura, e il guasto sarebbe di piattaforma, non
            // dell'ingresso.
            drop(file);

            Ok(percorso.clone())
        })
        .map_err(|_| "harness: workspace non raggiungibile su questo thread".to_owned())?
}

/// Il token che [`verifica_artefatto_ostile`] pretende nel footer.
pub const TOKEN_DI_PROVA: &str = "0000000000000000000000000000000000000000000000000000000000000000";

/// Lo schema che [`verifica_artefatto_ostile`] si attende.
#[must_use]
pub fn schema_di_prova() -> plenora_core::arrow::schema::SchemaRef {
    use plenora_core::arrow::schema::{DataType, Field, Schema};
    std::sync::Arc::new(Schema::new(vec![Field::new("a", DataType::Int64, true)]))
}

/// Un artefatto **coerente con le attese** di [`verifica_artefatto_ostile`]:
/// lo schema di prova, zero batch, zero righe e il token di prova.
///
/// Esiste perche' la garanzia «l'harness sa arrivare in fondo» sia
/// dimostrabile. Il corpus del fuzzer non e' versionato, quindi una campagna da
/// zero non produce da sola un Arrow IPC valido con queste attese: questo e' il
/// seed, nella sola forma che il repository conserva.
///
/// # Panics
///
/// Se il writer Arrow fallisce su uno schema costante: sarebbe un difetto
/// nostro, non un ingresso.
#[must_use]
pub fn artefatto_di_prova() -> Vec<u8> {
    use plenora_core::arrow::ipc::writer::FileWriter;

    use crate::commit_footer::scrivi_commit_token;
    use crate::commit_token::CommitToken;

    let token = CommitToken::da_esadecimale(TOKEN_DI_PROVA).expect("token di prova canonico");
    let mut byte = Vec::new();
    {
        let mut scrittore = FileWriter::try_new(&mut byte, &schema_di_prova())
            .expect("writer sullo schema di prova");
        // Nessun batch: le attese dichiarano zero righe e zero batch.
        scrivi_commit_token(&mut scrittore, Some(&token));
        scrittore.finish().expect("chiusura del file di prova");
    }
    byte
}
