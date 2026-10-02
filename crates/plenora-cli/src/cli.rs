//! Un'invocazione: argomenti → comando → inviluppo.
//!
//! Tutto ciò che un'invocazione produce passa da qui come [`Uscita`]: testo
//! per stdout e codice d'uscita. `main.rs` scrive il testo e termina; non
//! stampa altro, mai su stderr.

use std::fmt::Write as _;
use std::panic::{catch_unwind, AssertUnwindSafe};
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
#[must_use]
pub fn esegui_invocazione(argomenti: &[String], segnale: &Segnale) -> Uscita {
    let identita = identita_di(argomenti::nome_comando(argomenti));
    proteggi(identita, || invocazione(argomenti, segnale, identita))
}

/// Esegue `lavoro`; un panico diventa l'inviluppo `internal`, con effetto
/// `unknown` per il comando che scrive file (`run`).
fn proteggi(identita: Identita, lavoro: impl FnOnce() -> Uscita) -> Uscita {
    let con_effetti = operazioni::per_comando(identita.comando)
        .is_some_and(|operazione| operazione.effetto != operazioni::Effetto::Nessuno);
    catch_unwind(AssertUnwindSafe(lavoro))
        .unwrap_or_else(|_| inviluppo::panico(identita, con_effetti))
}

/// [`esegui_invocazione`] sugli argomenti del sistema operativo.
///
/// Un argomento che non è UTF-8 si rifiuta (`InvalidConfiguration`):
/// tradurlo con caratteri sostitutivi cambierebbe in silenzio un percorso
/// in un altro.
#[must_use]
pub fn esegui_invocazione_os(grezzi: Vec<std::ffi::OsString>, segnale: &Segnale) -> Uscita {
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
    esegui_invocazione(&convertiti, segnale)
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

    use super::{identita_di, proteggi};
    use crate::inviluppo::Uscita;

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
            let uscita = proteggi(identita, || -> Uscita {
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
