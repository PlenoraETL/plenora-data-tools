//! Un'invocazione: argomenti → comando → inviluppo.
//!
//! Tutto ciò che un'invocazione produce passa da qui come [`Uscita`]: testo
//! per stdout e codice d'uscita. `main.rs` scrive il testo e termina; non
//! stampa altro, mai su stderr.

use std::fmt::Write as _;
use std::panic::{catch_unwind, AssertUnwindSafe};

use plenora_core::panic_policy::panici_fuori_dalle_barriere;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use plenora_core::{ErrorPhase, PlenoraError, Result};
use plenora_io::{FileIngresso, FileUscita, OpzioniScrittura};
use plenora_pipeline::Interruzione;
use serde_json::{json, Value};

use crate::argomenti::{self, Comando, Formato, Invocazione, Scadenza};
use crate::inviluppo::{self, Identita, Uscita};
use crate::operazioni::{self, OPERAZIONI};
use crate::{api, capacita, PROTOCOLLO_CLI, VERSIONE_COMPONENTE};

/// Contratto del risultato di `--version`.
pub const CONTRATTO_VERSIONE: &str = "plenora-data-version-result-v1";
/// Contratto del risultato di `--help --format json`.
pub const CONTRATTO_AIUTO: &str = "plenora-data-help-result-v1";

/// Il segnale di annullamento del processo, se installato.
///
/// `None` vuol dire che il gestore di Ctrl-C/SIGTERM non si è potuto
/// installare: i comandi che dichiarano l'annullamento (`describe`,
/// `validate`, `run`) allora si rifiutano, invece di girare con un
/// annullamento dichiarato e non funzionante.
pub type Segnale = Option<Arc<AtomicBool>>;

/// Esegue un'invocazione (argomenti senza il nome del programma).
///
/// Un panico in qualunque punto diventa l'inviluppo `internal` (exit 70),
/// senza il testo del payload; l'hook di panico che non stampa nulla lo
/// installa `main.rs` (`plenora_core::panic_policy`).
///
/// Il conto dei panici di base è quello di adesso: un panico di un altro
/// thread avvenuto prima della chiamata non conta. Il binario usa
/// [`esegui_invocazione_dal`] con il conto letto subito dopo l'installazione
/// dell'hook.
#[must_use]
pub fn esegui_invocazione(argomenti: &[String], segnale: &Segnale) -> Uscita {
    esegui_invocazione_dal(argomenti, segnale, panici_fuori_dalle_barriere())
}

/// [`esegui_invocazione`] con il conto dei panici di base dato dal chiamante.
///
/// La base è `panic_policy::panici_fuori_dalle_barriere` letto subito
/// dopo `install` e prima di avviare il gestore di Ctrl-C: un panico fuori
/// dalle barriere contato fra la base e il controllo finale di
/// [`proteggi`], in qualunque thread, trasforma un `ok` in `internal`. Un
/// panico in corsa con quel controllo o successivo non si osserva: il
/// lavoro era già concluso e il suo esito è vero; si perde solo
/// l'annullabilità in quella finestra.
///
/// Il conto è del processo: con più invocazioni concorrenti nello stesso
/// processo (un uso da libreria) un panico dell'una, contato prima del
/// controllo finale di un'altra, fa fallire anche quella con `internal`,
/// cioè un `internal` falso al posto di un successo vero (docs/cli.md, «Panici
/// fuori dal thread principale»).
#[must_use]
pub fn esegui_invocazione_dal(argomenti: &[String], segnale: &Segnale, panici_base: u64) -> Uscita {
    let identita = identita_di(argomenti::nome_comando(argomenti));
    proteggi(identita, panici_base, || {
        invocazione(argomenti, segnale, identita)
    })
}

/// Esegue `lavoro`; un panico diventa l'inviluppo `internal`, con effetto
/// `unknown` per il comando che scrive file (`run`).
///
/// Anche un panico in un **altro** thread durante il lavoro (il thread del
/// gestore di Ctrl-C di `ctrlc`, staccato: con l'hook silenzioso morirebbe
/// senza che nessuno lo veda, e l'annullamento smetterebbe di funzionare)
/// si vede, se è contato prima del controllo finale: l'hook installato da
/// `main.rs` conta i panici fuori dalle barriere di dipendenza
/// (`panic_policy::panici_fuori_dalle_barriere`), e un conto cambiato
/// dalla base `panici_prima` alla lettura finale trasforma un `ok`
/// nell'errore `internal`. Un panico in corsa con quella lettura, o dopo,
/// non si osserva (docs/cli.md, «Panici fuori dal thread principale»). Un
/// errore già tipizzato resta quello: dice già che il comando non è
/// riuscito.
fn proteggi(identita: Identita, panici_prima: u64, lavoro: impl FnOnce() -> Uscita) -> Uscita {
    let con_effetti = operazioni::per_comando(identita.comando)
        .is_some_and(|operazione| operazione.effetto != operazioni::Effetto::Nessuno);
    let uscita = catch_unwind(AssertUnwindSafe(lavoro))
        .unwrap_or_else(|_| inviluppo::panico(identita, con_effetti));
    if uscita.codice == 0 && panici_fuori_dalle_barriere() != panici_prima {
        return inviluppo::panico(identita, con_effetti);
    }
    uscita
}

/// Codice d'uscita quando il documento non si può consegnare su stdout
/// (pipe chiusa, disco pieno): la categoria `io` di CLI 2.0, sezione 8. Il
/// documento manca o è troncato, e non se ne scrive un secondo.
pub const CODICE_STDOUT_NON_SCRIVIBILE: u8 = 5;

/// Scrive l'inviluppo su `stdout` e rende il codice d'uscita del processo.
///
/// È quello dell'esito, o [`CODICE_STDOUT_NON_SCRIVIBILE`] se la scrittura o
/// il `flush` falliscono (docs/cli.md, «Stdout non scrivibile»). Niente va su
/// stderr.
pub fn consegna(uscita: &Uscita, stdout: &mut impl std::io::Write) -> u8 {
    match stdout
        .write_all(uscita.stdout.as_bytes())
        .and_then(|()| stdout.flush())
    {
        Ok(()) => uscita.codice,
        Err(_) => CODICE_STDOUT_NON_SCRIVIBILE,
    }
}

/// [`esegui_invocazione`] sugli argomenti del sistema operativo.
///
/// Un argomento che non è UTF-8 si rifiuta (`InvalidConfiguration`):
/// tradurlo con caratteri sostitutivi cambierebbe in silenzio un percorso
/// in un altro. `panici_base` come in [`esegui_invocazione_dal`].
#[must_use]
pub fn esegui_invocazione_os(
    grezzi: Vec<std::ffi::OsString>,
    segnale: &Segnale,
    panici_base: u64,
) -> Uscita {
    let mut convertiti = Vec::with_capacity(grezzi.len());
    for (indice, argomento) in grezzi.into_iter().enumerate() {
        match argomento.into_string() {
            Ok(testo) => convertiti.push(testo),
            Err(_) => {
                return inviluppo::errore(
                    identita_di(argomenti::nome_comando(&convertiti)),
                    &PlenoraError::InvalidConfiguration(format!(
                        "argomento in posizione {} non UTF-8",
                        indice + 1
                    )),
                )
            }
        }
    }
    esegui_invocazione_dal(&convertiti, segnale, panici_base)
}

/// Comando e contratto dell'inviluppo per il nome canonico del comando.
fn identita_di(nome: Option<&'static str>) -> Identita {
    match nome {
        Some("help") => Identita {
            comando: "help",
            contratto: CONTRATTO_AIUTO,
        },
        Some("version") => Identita {
            comando: "version",
            contratto: CONTRATTO_VERSIONE,
        },
        Some("capabilities") => Identita {
            comando: "capabilities",
            contratto: capacita::CONTRATTO_CAPACITA,
        },
        Some(comando) => {
            operazioni::per_comando(comando).map_or(Identita::SCONOSCIUTA, |op| Identita {
                comando: op.comando,
                contratto: op.uscita,
            })
        }
        None => Identita::SCONOSCIUTA,
    }
}

fn invocazione(argomenti: &[String], segnale: &Segnale, identita: Identita) -> Uscita {
    let invocazione = match argomenti::leggi(argomenti) {
        Ok(invocazione) => invocazione,
        Err(errore) => return inviluppo::errore(identita, &errore),
    };
    if invocazione.comando == Comando::Aiuto && invocazione.formato == Formato::Implicito {
        // L'aiuto senza `--format json` è testo per persone (CLI 2.0,
        // sezione 2: il formato umano è esplicito o, come qui, il solo
        // aiuto). Exit 0.
        return Uscita {
            stdout: testo_aiuto(),
            codice: 0,
        };
    }
    match risultato(invocazione, segnale) {
        Ok(valore) => inviluppo::successo(identita, valore),
        Err(errore) => inviluppo::errore(identita, &errore),
    }
}

fn risultato(invocazione: Invocazione, segnale: &Segnale) -> Result<Value> {
    let Invocazione {
        comando, scadenza, ..
    } = invocazione;
    match comando {
        Comando::Aiuto => Ok(json!({
            "usage": testo_aiuto(),
            "commands": comandi_compilati(),
        })),
        Comando::Versione => Ok(json!({
            "component_version": VERSIONE_COMPONENTE,
            "protocol_version": PROTOCOLLO_CLI,
        })),
        Comando::Capacita => Ok(capacita::documento()),
        Comando::Catalogo => Ok(api::catalogo()),
        Comando::Descrivi { ingresso } => {
            let interruzione = interruzione(scadenza.as_ref(), segnale)?;
            api::descrivi(&ingresso, &interruzione)
        }
        Comando::Valida { piano, ingressi } => {
            let interruzione = interruzione(scadenza.as_ref(), segnale)?;
            let piano = api::leggi_piano(&piano)?;
            verifica_nomi(
                &piano.inputs,
                ingressi.iter().map(|(nome, _)| nome),
                "--input",
            )?;
            api::valida(&piano, &file_ingresso(ingressi), &interruzione)
        }
        Comando::Esegui {
            piano,
            ingressi,
            uscite,
            sovrascrivi,
        } => {
            let interruzione = interruzione(scadenza.as_ref(), segnale)?;
            let piano = api::leggi_piano(&piano)?;
            let uscite = file_uscita(&piano.outputs, uscite)?;
            verifica_nomi(
                &piano.inputs,
                ingressi.iter().map(|(nome, _)| nome),
                "--input",
            )?;
            verifica_nomi(&piano.outputs, uscite.iter().map(|u| &u.nome), "--output")?;
            api::esegui(
                &piano,
                &file_ingresso(ingressi),
                &uscite,
                &OpzioniScrittura {
                    sovrascrivi,
                    ..OpzioniScrittura::default()
                },
                &interruzione,
            )
        }
    }
}

/// I NAME di `--input` (o `--output`) contro i nomi del piano, prima di
/// leggere qualunque file: ognuno è un nome dichiarato, nessuno si ripete,
/// nessun nome dichiarato manca.
///
/// Il NAME ricevuto non entra nel messaggio (una divisione al primo `=` di
/// un percorso con `=` darebbe un pezzo di percorso): il messaggio dice
/// quale occorrenza del flag. I nomi del piano invece sì: sono del piano.
fn verifica_nomi<'a>(
    dichiarati: &[String],
    dati: impl Iterator<Item = &'a String>,
    flag: &str,
) -> Result<()> {
    let mut visti: Vec<&String> = Vec::new();
    for (indice, nome) in dati.enumerate() {
        let occorrenza = indice + 1;
        if !dichiarati.contains(nome) {
            return Err(PlenoraError::InvalidConfiguration(format!(
                "`{flag}` numero {occorrenza}: il NAME non e' un nome del piano"
            )));
        }
        if visti.contains(&nome) {
            return Err(PlenoraError::InvalidConfiguration(format!(
                "`{flag}` numero {occorrenza}: NAME gia' dato da un altro `{flag}`"
            )));
        }
        visti.push(nome);
    }
    if let Some(mancante) = dichiarati.iter().find(|nome| !visti.contains(nome)) {
        return Err(PlenoraError::InvalidConfiguration(format!(
            "`{mancante}` del piano senza `{flag}`"
        )));
    }
    Ok(())
}

fn file_ingresso(ingressi: Vec<(String, PathBuf)>) -> Vec<FileIngresso> {
    ingressi
        .into_iter()
        .map(|(nome, percorso)| FileIngresso {
            nome,
            percorso,
            formato: None,
        })
        .collect()
}

/// Gli output di `run`: `--output PATH` senza nome vale per l'unico output
/// del piano; con più output del piano ognuno va nominato.
fn file_uscita(
    output_del_piano: &[String],
    uscite: Vec<argomenti::Uscita>,
) -> Result<Vec<FileUscita>> {
    uscite
        .into_iter()
        .map(|uscita| match uscita {
            argomenti::Uscita::Nominata(nome, percorso) => Ok(FileUscita {
                nome,
                percorso,
                formato: None,
            }),
            argomenti::Uscita::Unica(percorso) => match output_del_piano {
                [unico] => Ok(FileUscita {
                    nome: unico.clone(),
                    percorso,
                    formato: None,
                }),
                _ => Err(PlenoraError::InvalidConfiguration(
                    "`--output PATH` senza nome vale solo per un piano con un solo output: \
                     usare --output NAME=PATH per ogni output"
                        .to_owned(),
                )),
            },
        })
        .collect()
}

/// La scadenza e il segnale del comando. La scadenza si fissa ora: un
/// `--timeout-ms` conta dall'avvio del comando, un `--deadline` passato
/// scade al primo controllo.
fn interruzione(scadenza: Option<&Scadenza>, segnale: &Segnale) -> Result<Interruzione> {
    let Some(segnale) = segnale.clone() else {
        return Err(PlenoraError::Internal(
            "gestore dell'annullamento (Ctrl-C, SIGTERM) non installato: il comando, che \
             dichiara l'annullamento, non parte"
                .to_owned(),
        )
        .with_phase(ErrorPhase::Validate));
    };
    let adesso = Instant::now();
    let scadenza = match scadenza {
        None => None,
        Some(Scadenza::Relativa(millisecondi)) => Some(
            adesso
                .checked_add(Duration::from_millis(*millisecondi))
                .ok_or_else(|| fuori_scala("--timeout-ms"))?,
        ),
        Some(Scadenza::Assoluta(testo)) => {
            let istante = chrono::DateTime::parse_from_rfc3339(testo).map_err(|_| {
                PlenoraError::InvalidConfiguration(
                    "`--deadline`: atteso un istante RFC 3339 (2030-01-01T00:00:00Z)".to_owned(),
                )
            })?;
            let istante = SystemTime::from(istante);
            // Già passata: scade al primo controllo, senza aritmetica
            // all'indietro sull'`Instant`.
            let restante = istante
                .duration_since(SystemTime::now())
                .unwrap_or(Duration::ZERO);
            Some(
                adesso
                    .checked_add(restante)
                    .ok_or_else(|| fuori_scala("--deadline"))?,
            )
        }
    };
    Ok(Interruzione {
        scadenza,
        annullamento: Some(segnale),
    })
}

fn fuori_scala(flag: &str) -> PlenoraError {
    PlenoraError::InvalidConfiguration(format!(
        "`{flag}`: scadenza oltre quanto il clock monotono rappresenta"
    ))
}

/// I comandi di questo binario, nell'ordine dell'aiuto: scoperta, poi le
/// operazioni della tabella.
fn comandi_compilati() -> Vec<Value> {
    let mut comandi = vec![
        json!({"command": "--help", "operation": null}),
        json!({"command": "--version", "operation": null}),
        json!({"command": "capabilities", "operation": null}),
    ];
    comandi.extend(
        OPERAZIONI
            .iter()
            .map(|op| json!({"command": op.comando, "operation": op.id})),
    );
    comandi
}

/// Il testo d'aiuto: solo i comandi compilati in questo binario, dalla
/// tabella delle operazioni.
#[must_use]
pub fn testo_aiuto() -> String {
    let mut testo = format!(
        "plenora-data {VERSIONE_COMPONENTE} — plenora-data-tools, CLI protocol {PROTOCOLLO_CLI}\n\n\
         Uso:\n  \
         plenora-data --help [--format json]\n  \
         plenora-data --version [--format json]\n  \
         plenora-data capabilities [--format json]\n"
    );
    for operazione in OPERAZIONI {
        // Scrivere su una `String` non fallisce.
        let _ = write!(
            testo,
            "  plenora-data {} [--format json]\n      {} ({})\n",
            operazione.sintassi, operazione.riassunto, operazione.id
        );
    }
    testo.push_str(
        "\nUscita: un documento JSON su stdout (inviluppo plenora-cli-v2), niente su stderr.\n\
         Codici: 0 ok, 2 piano o configurazione, 3 schema/dati/CRS/non supportato,\n\
         4 limite di risorsa, 5 I/O o scadenza, 6 esecuzione, 70 interno, 130 annullato.\n\
         Annullamento: Ctrl-C o SIGTERM, controllato fra le letture, i passi e le scritture.\n",
    );
    testo
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use super::{consegna, identita_di, proteggi, CODICE_STDOUT_NON_SCRIVIBILE};
    use crate::inviluppo::Uscita;

    /// Uno scrittore che accetta `capienza` byte e poi fallisce.
    struct Rotto {
        capienza: usize,
        scritti: Vec<u8>,
    }

    impl std::io::Write for Rotto {
        fn write(&mut self, byte: &[u8]) -> std::io::Result<usize> {
            if self.scritti.len() >= self.capienza {
                return Err(std::io::ErrorKind::BrokenPipe.into());
            }
            let quanti = byte.len().min(self.capienza - self.scritti.len());
            self.scritti.extend_from_slice(&byte[..quanti]);
            Ok(quanti)
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// Stdout chiuso o pieno: codice della categoria `io`, mai 0, e nessun
    /// secondo documento; con uno stdout sano il codice dell'esito.
    #[test]
    fn stdout_non_scrivibile() {
        let uscita = Uscita {
            stdout: "{\"status\":\"ok\"}\n".to_owned(),
            codice: 0,
        };
        for capienza in [0, 5] {
            let mut rotto = Rotto {
                capienza,
                scritti: Vec::new(),
            };
            assert_eq!(consegna(&uscita, &mut rotto), CODICE_STDOUT_NON_SCRIVIBILE);
            assert_eq!(rotto.scritti, uscita.stdout.as_bytes()[..capienza]);
        }
        let mut sano = Vec::new();
        assert_eq!(consegna(&uscita, &mut sano), 0);
        assert_eq!(sano, uscita.stdout.as_bytes());
    }

    /// Un panico diventa l'inviluppo `internal`, exit 70, senza il testo del
    /// payload; per `run` l'effetto è `unknown` (un output può essere già
    /// scritto), per gli altri `none`.
    #[test]
    fn un_panico_diventa_un_errore_interno_senza_payload() {
        for (comando, effetto) in [
            ("run", "unknown"),
            ("describe", "none"),
            ("catalog", "none"),
        ] {
            let identita = identita_di(Some(comando));
            let base = plenora_core::panic_policy::panici_fuori_dalle_barriere();
            let uscita = proteggi(identita, base, || -> Uscita {
                std::panic::panic_any(String::from("PAYLOAD-SEGRETO"))
            });
            assert_eq!(uscita.codice, 70);
            assert!(!uscita.stdout.contains("PAYLOAD-SEGRETO"));
            let documento: Value = serde_json::from_str(&uscita.stdout).expect("JSON");
            assert_eq!(documento["status"], "error");
            assert_eq!(documento["command"], comando);
            assert_eq!(documento["error"]["category"], "internal");
            assert_eq!(documento["error"]["remote_effect"], effetto);
            assert_eq!(documento["error"]["retry"]["kind"], "never");
        }
    }
}
