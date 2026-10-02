//! Lettura degli argomenti: fail closed (CLI 2.0, sezione 2).
//!
//! Comandi, flag sconosciuti, valori mancanti, flag ripetuti e argomenti
//! posizionali in più si rifiutano con `InvalidConfiguration` (exit 2). I
//! messaggi dicono la posizione e il nome del flag atteso, mai il testo
//! ricevuto: un argomento può essere un percorso, e i percorsi non entrano
//! nei messaggi pubblici.
//!
//! Grammatica (il primo token è il comando, o `--help`/`--version`;
//! `--format json` può stare ovunque, una volta):
//!
//! ```text
//! plenora-data --help [--format json]
//! plenora-data --version [--format json]
//! plenora-data capabilities [--format json]
//! plenora-data catalog [--format json]
//! plenora-data describe --input PATH [CONTROLLI] [--format json]
//! plenora-data validate --plan PATH [--input NAME=PATH]... [CONTROLLI] [--format json]
//! plenora-data run --plan PATH [--input NAME=PATH]... --output [NAME=]PATH...
//!                  [--overwrite] [CONTROLLI] [--format json]
//! CONTROLLI: --deadline RFC3339 | --timeout-ms MS
//! ```
//!
//! Un valore non comincia mai con `--`: un flag seguito da un altro flag è
//! un valore mancante, non un valore che si chiama come un flag.

use std::path::PathBuf;

use plenora_core::PlenoraError;

/// Il formato d'uscita. JSON è l'unico contratto macchina; senza `--format`
/// l'uscita è comunque JSON, tranne per `--help`, che senza `--format json`
/// stampa il testo d'aiuto.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Formato {
    /// `--format json` esplicito.
    Json,
    /// Nessun `--format`.
    Implicito,
}

/// Scadenza come richiesta dal chiamante, convertita in `Instant` quando il
/// comando parte.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Scadenza {
    /// `--deadline`: istante RFC 3339, come `plenora.execution.deadline` del
    /// binding di runtime.
    Assoluta(String),
    /// `--timeout-ms`: millisecondi dall'avvio del comando.
    Relativa(u64),
}

/// Un `--output` di `run`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Uscita {
    /// `--output PATH`: l'unico output del piano.
    Unica(PathBuf),
    /// `--output NAME=PATH`.
    Nominata(String, PathBuf),
}

/// Il comando letto.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Comando {
    Aiuto,
    Versione,
    Capacita,
    Catalogo,
    Descrivi {
        ingresso: PathBuf,
    },
    Valida {
        piano: PathBuf,
        ingressi: Vec<(String, PathBuf)>,
    },
    Esegui {
        piano: PathBuf,
        ingressi: Vec<(String, PathBuf)>,
        uscite: Vec<Uscita>,
        sovrascrivi: bool,
    },
}

/// Un'invocazione letta per intero.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invocazione {
    pub comando: Comando,
    pub formato: Formato,
    pub scadenza: Option<Scadenza>,
}

/// Il nome canonico del primo token, se è un comando di questo binario:
/// serve all'inviluppo d'errore anche quando il resto non si legge.
#[must_use]
pub fn nome_comando(argomenti: &[String]) -> Option<&'static str> {
    let mut posizione = 0;
    while let Some(token) = argomenti.get(posizione) {
        if token == "--format" {
            posizione += 2;
            continue;
        }
        return COMANDI
            .iter()
            .find(|(nome, _)| nome == token)
            .map(|(_, canonico)| *canonico);
    }
    None
}

/// Token del primo argomento e nome canonico del comando nell'inviluppo.
pub const COMANDI: &[(&str, &str)] = &[
    ("--help", "help"),
    ("--version", "version"),
    ("capabilities", "capabilities"),
    ("catalog", "catalog"),
    ("describe", "describe"),
    ("validate", "validate"),
    ("run", "run"),
];

fn uso(motivo: &str) -> PlenoraError {
    PlenoraError::InvalidConfiguration(format!("{motivo} (plenora-data --help)"))
}

/// I flag che un comando accetta oltre a `--format`, e quali si possono
/// ripetere.
fn flag_ammessi(comando: &str) -> (&'static [&'static str], &'static [&'static str]) {
    const CONTROLLI: [&str; 2] = ["--deadline", "--timeout-ms"];
    match comando {
        "describe" => (&["--input", CONTROLLI[0], CONTROLLI[1]], &[]),
        "validate" => (
            &["--plan", "--input", CONTROLLI[0], CONTROLLI[1]],
            &["--input"],
        ),
        "run" => (
            &[
                "--plan",
                "--input",
                "--output",
                "--overwrite",
                CONTROLLI[0],
                CONTROLLI[1],
            ],
            &["--input", "--output"],
        ),
        _ => (&[], &[]),
    }
}

/// Valori letti, prima di comporre il comando.
#[derive(Default)]
struct Letti {
    formato: Option<Formato>,
    input: Vec<String>,
    plan: Option<String>,
    output: Vec<String>,
    overwrite: bool,
    scadenza: Option<Scadenza>,
    /// Flag già visti, per rifiutare quelli ripetuti.
    visti: Vec<&'static str>,
}

/// Il valore del flag in `posizione`: il token seguente, non vuoto e che
/// non comincia con `--`.
fn valore(argomenti: &[String], posizione: usize, flag: &str) -> Result<String, PlenoraError> {
    match argomenti.get(posizione + 1) {
        Some(valore) if !valore.starts_with("--") && !valore.is_empty() => Ok(valore.clone()),
        _ => Err(uso(&format!(
            "`{flag}` in posizione {} senza valore",
            posizione + 1
        ))),
    }
}

/// Legge gli argomenti (senza il nome del programma).
///
/// # Errors
///
/// `InvalidConfiguration` per ogni invocazione fuori dalla grammatica.
pub fn leggi(argomenti: &[String]) -> Result<Invocazione, PlenoraError> {
    let mut letti = Letti::default();
    let mut posizione = 0_usize;
    // Il comando: il primo token che non sia `--format` col suo valore.
    let comando = loop {
        let Some(token) = argomenti.get(posizione) else {
            return Err(uso("nessun comando"));
        };
        if token == "--format" {
            leggi_formato(&mut letti, &valore(argomenti, posizione, "--format")?)?;
            posizione += 2;
            continue;
        }
        let Some((_, canonico)) = COMANDI.iter().find(|(nome, _)| nome == token) else {
            return Err(uso(&format!(
                "comando sconosciuto in posizione {}: attesi --help, --version, capabilities, \
                 catalog, describe, validate, run",
                posizione + 1
            )));
        };
        posizione += 1;
        break *canonico;
    };
    while posizione < argomenti.len() {
        posizione += leggi_flag(argomenti, posizione, comando, &mut letti)?;
    }
    let formato = letti.formato.unwrap_or(Formato::Implicito);
    let scadenza = letti.scadenza.clone();
    Ok(Invocazione {
        comando: componi(comando, letti)?,
        formato,
        scadenza,
    })
}

/// Legge il flag in `posizione` e rende quanti token ha consumato.
fn leggi_flag(
    argomenti: &[String],
    posizione: usize,
    comando: &str,
    letti: &mut Letti,
) -> Result<usize, PlenoraError> {
    let numero = posizione + 1;
    let token = argomenti
        .get(posizione)
        .map(String::as_str)
        .unwrap_or_default();
    if token == "--format" {
        leggi_formato(letti, &valore(argomenti, posizione, token)?)?;
        return Ok(2);
    }
    let (ammessi, ripetibili) = flag_ammessi(comando);
    let Some(flag) = ammessi.iter().copied().find(|flag| *flag == token) else {
        return Err(uso(&if token.starts_with("--") {
            format!("flag sconosciuto o non ammesso da `{comando}` in posizione {numero}")
        } else {
            format!("argomento posizionale inatteso in posizione {numero}")
        }));
    };
    if letti.visti.contains(&flag) && !ripetibili.contains(&flag) {
        return Err(uso(&format!("`{flag}` ripetuto in posizione {numero}")));
    }
    letti.visti.push(flag);
    if flag == "--overwrite" {
        letti.overwrite = true;
        return Ok(1);
    }
    let testo = valore(argomenti, posizione, flag)?;
    match flag {
        "--input" => letti.input.push(testo),
        "--output" => letti.output.push(testo),
        "--plan" => letti.plan = Some(testo),
        _ => {
            // `--deadline` o `--timeout-ms`: uno solo dei due.
            if letti.scadenza.is_some() {
                return Err(uso(&format!(
                    "una sola fra --deadline e --timeout-ms (posizione {numero})"
                )));
            }
            letti.scadenza = Some(if flag == "--deadline" {
                Scadenza::Assoluta(testo)
            } else {
                // Solo cifre: `parse` accetterebbe anche un `+` in testa.
                let cifre = testo.bytes().all(|byte| byte.is_ascii_digit());
                let millisecondi = if cifre {
                    testo.parse::<u64>().ok()
                } else {
                    None
                };
                Scadenza::Relativa(millisecondi.ok_or_else(|| {
                    uso(&format!(
                        "`--timeout-ms` in posizione {numero}: atteso un intero di millisecondi"
                    ))
                })?)
            });
        }
    }
    Ok(2)
}

fn leggi_formato(letti: &mut Letti, valore: &str) -> Result<(), PlenoraError> {
    if letti.formato.is_some() {
        return Err(uso("`--format` ripetuto"));
    }
    if valore != "json" {
        return Err(uso("`--format`: l'unico formato e' json"));
    }
    letti.formato = Some(Formato::Json);
    Ok(())
}

/// `NAME=PATH`, diviso al primo `=`; nome e percorso non vuoti.
fn nominato(testo: &str, flag: &str) -> Result<(String, PathBuf), PlenoraError> {
    match testo.split_once('=') {
        Some((nome, percorso)) if !nome.is_empty() && !percorso.is_empty() => {
            Ok((nome.to_owned(), PathBuf::from(percorso)))
        }
        _ => Err(uso(&format!("`{flag}`: atteso NAME=PATH"))),
    }
}

fn componi(comando: &str, letti: Letti) -> Result<Comando, PlenoraError> {
    let Letti {
        input,
        plan,
        output,
        overwrite,
        ..
    } = letti;
    let ingressi = |input: Vec<String>| -> Result<Vec<(String, PathBuf)>, PlenoraError> {
        input
            .iter()
            .map(|testo| nominato(testo, "--input"))
            .collect()
    };
    let piano = |plan: Option<String>| {
        plan.map(PathBuf::from)
            .ok_or_else(|| uso(&format!("`{comando}` richiede --plan PLAN.json")))
    };
    Ok(match comando {
        "help" => Comando::Aiuto,
        "version" => Comando::Versione,
        "capabilities" => Comando::Capacita,
        "catalog" => Comando::Catalogo,
        "describe" => Comando::Descrivi {
            ingresso: input
                .into_iter()
                .next()
                .map(PathBuf::from)
                .ok_or_else(|| uso("`describe` richiede --input INPUT.arrow"))?,
        },
        "validate" => Comando::Valida {
            piano: piano(plan)?,
            ingressi: ingressi(input)?,
        },
        "run" => {
            if output.is_empty() {
                return Err(uso("`run` richiede almeno un --output"));
            }
            // `--output PATH` senza nome vale solo da solo: con più output
            // ognuno si nomina, e un `=` nel testo è sempre il separatore.
            let uscite = if output.len() == 1 && !output[0].contains('=') {
                vec![Uscita::Unica(PathBuf::from(&output[0]))]
            } else {
                output
                    .iter()
                    .map(|testo| {
                        nominato(testo, "--output").map(|(nome, p)| Uscita::Nominata(nome, p))
                    })
                    .collect::<Result<_, _>>()?
            };
            Comando::Esegui {
                piano: piano(plan)?,
                ingressi: ingressi(input)?,
                uscite,
                sovrascrivi: overwrite,
            }
        }
        _ => {
            return Err(PlenoraError::Internal(
                "comando canonico senza composizione".to_owned(),
            ))
        }
    })
}
