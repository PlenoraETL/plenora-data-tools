//! Superficie degli argomenti: aiuto, flag ammessi, e il loro rifiuto.
//!
//! Che cosa la CLI accetta e' una decisione, non una conseguenza di come il
//! dispatch e' scritto: [`superficie`] la dichiara comando per comando, e
//! [`reject_unknown_flags`] la fa rispettare PRIMA di qualunque uscita
//! anticipata, help compreso. Un parser che risponde «va bene» a
//! un'invocazione che non ha capito e' fail-open anche quando non pubblica
//! nulla.
//!
//! I testi di aiuto stanno qui accanto perche' sono la stessa superficie
//! vista dall'altro lato: `matrice_cli` verifica che dichiarino esattamente
//! i flag che il dispatch accetta.

use plenora_core::PlenoraError;

use crate::contract;

pub fn help_text() -> String {
    format!(
        "plenora-data-tools {}

  plenora-data-tools catalog [--family table|geo]
  plenora-data-tools describe --input INPUT.arrow                          (alias: inspect-dataset)
  plenora-data-tools validate --plan PLAN.json --input NOME=INPUT.arrow...
  plenora-data-tools run --plan PLAN.json --input NOME=INPUT.arrow... --output OUTPUT.arrow [--no-geo-fusion]   (piani DAG v5)
  plenora-data-tools run --plan PLAN.json --input INPUT.arrow [--right RIGHT.arrow] --output OUTPUT.arrow       (piani legacy, schema_version <= 3)
  plenora-data-tools run --plan PLAN.json --inputs INPUT.arrow --output OUTPUT.arrow                            (posizionale: solo piani a UN input)
  plenora-data-tools capabilities
  plenora-data-tools transform --input INPUT --schema SCHEMA.json --output OUTPUT                               (deprecato: usare run con un piano)
  plenora-data-tools spatial-join --left LEFT --right RIGHT --schema SCHEMA.json --output PAIRS                  (deprecato: usare run con un piano)
  plenora-data-tools transform-arrow --input INPUT --schema SCHEMA.json --output OUTPUT                          (deprecato: usare run con un piano)
  plenora-data-tools pair-arrow --left LEFT --right RIGHT --schema SCHEMA.json --output PAIRS                    (deprecato: usare run con un piano)
  plenora-data-tools self-test [--output RESULT.bin]
  plenora-data-tools --version",
        env!("CARGO_PKG_VERSION")
    )
}

pub fn subcommand_help_text(command: &str) -> Option<&'static str> {
    match command {
        "catalog" => Some("Usage: plenora-data-tools catalog [--family table|geo]"),
        "describe" | "inspect-dataset" => Some(
            "Usage: plenora-data-tools describe --input INPUT.arrow

Stampa in JSON il contratto dell'input: campi, colonna geometrica, CRS,
encoding, tipi dichiarati e fingerprint del contratto. Non esegue nulla.",
        ),
        "validate" => Some(
            "Usage:
  plenora-data-tools validate --plan PLAN.json --input NOME=INPUT.arrow... [--no-geo-fusion]
  plenora-data-tools validate --plan PLAN.json --inputs INPUT.arrow   (posizionale: solo piani a UN input)",
        ),
        "run" => Some(
            "Usage:
  plenora-data-tools run --plan PLAN.json --input NOME=INPUT.arrow... --output OUTPUT.arrow [--no-geo-fusion]
  plenora-data-tools run --plan PLAN.json --input INPUT.arrow [--right RIGHT.arrow] --output OUTPUT.arrow   (piani legacy)
  plenora-data-tools run --plan PLAN.json --inputs INPUT.arrow --output OUTPUT.arrow                        (posizionale: solo piani a UN input)

La forma nominale lega ogni percorso al nome dell'input dichiarato dal piano:
due file scambiati diventano un errore invece di un risultato sbagliato. Con
piu' di un input dichiarato e' l'unica forma ammessa.",
        ),
        "capabilities" => Some("Usage: plenora-data-tools capabilities"),
        "transform" => Some(
            "Usage: plenora-data-tools transform --input INPUT --schema SCHEMA.json --output OUTPUT",
        ),
        "spatial-join" => Some(
            "Usage: plenora-data-tools spatial-join --left LEFT --right RIGHT --schema SCHEMA.json --output PAIRS",
        ),
        "transform-arrow" => Some(
            "Usage: plenora-data-tools transform-arrow --input INPUT --schema SCHEMA.json --output OUTPUT [--output-format plngeo3|ipc-file]",
        ),
        "pair-arrow" => Some(
            "Usage: plenora-data-tools pair-arrow --left LEFT --right RIGHT --schema SCHEMA.json --output OUTPUT [--output-format plngeo3|ipc-file]",
        ),
        "self-test" => Some("Usage: plenora-data-tools self-test [--output RESULT.bin]"),
        _ => None,
    }
}

/// Flag accettati da ciascun sottocomando, e quali possono ripetersi.
///
/// E' l'unico posto in cui la superficie degli argomenti e' dichiarata: il
/// controllo di §1.4 la confronta con l'help, e il dispatch la usa per
/// rifiutare cio' che non conosce. Tre elenchi separati divergerebbero.
///
/// `--format` non compare: e' globale e viene tolto dagli argomenti prima
/// del dispatch (`strip_output_format`).
pub struct SuperficieComando {
    /// Flag ammessi, compresi quelli senza valore.
    flag: &'static [&'static str],
    /// Flag che possono comparire piu' di una volta.
    ripetibili: &'static [&'static str],
}

pub const fn superficie(comando: &str) -> Option<SuperficieComando> {
    Some(match comando.as_bytes() {
        b"catalog" => SuperficieComando {
            flag: &["--family"],
            ripetibili: &[],
        },
        b"describe" | b"inspect-dataset" => SuperficieComando {
            flag: &["--input"],
            ripetibili: &[],
        },
        b"validate" => SuperficieComando {
            flag: &["--plan", "--input", "--inputs", "--no-geo-fusion"],
            // `--input NOME=PERCORSO` si ripete: un input per occorrenza.
            ripetibili: &["--input"],
        },
        b"run" => SuperficieComando {
            flag: &[
                "--plan",
                "--input",
                "--inputs",
                "--right",
                "--output",
                "--no-geo-fusion",
            ],
            ripetibili: &["--input"],
        },
        b"capabilities" => SuperficieComando {
            flag: &[],
            ripetibili: &[],
        },
        b"transform" => SuperficieComando {
            flag: &["--input", "--schema", "--output"],
            ripetibili: &[],
        },
        b"spatial-join" => SuperficieComando {
            flag: &["--left", "--right", "--schema", "--output"],
            ripetibili: &[],
        },
        b"transform-arrow" => SuperficieComando {
            flag: &["--input", "--schema", "--output", "--output-format"],
            ripetibili: &[],
        },
        b"pair-arrow" => SuperficieComando {
            flag: &[
                "--left",
                "--right",
                "--schema",
                "--output",
                "--output-format",
            ],
            ripetibili: &[],
        },
        b"self-test" => SuperficieComando {
            flag: &["--output"],
            ripetibili: &[],
        },
        _ => return None,
    })
}

/// Convalida la riga di comando di un sottocomando: nessun token puo'
/// restare inosservato.
///
/// Un parser che ignora cio' che non riconosce pubblica un output basato su
/// un'invocazione DIVERSA da quella che l'utente ha scritto. Qui ogni
/// argomento deve essere o un flag dichiarato, o il valore di un flag che ne
/// prende uno: tutto il resto e' un errore.
///
/// Casi chiusi, tutti verificati dalla matrice:
///
/// - flag sconosciuto (`--boh`), anche in forma breve (`-x`);
/// - flag a valore singolo ripetuto;
/// - **posizionale inatteso** (`run pippo --plan ...`);
/// - **flag usato come valore** (`--plan --output`), che silenziosamente
///   rendeva `--output` il nome del piano;
/// - **argomenti extra** dopo `--version` e `--help`.
///
/// # Errors
///
/// `PlenoraError::InvalidPlan` con l'elenco dei flag ammessi.
pub fn reject_unknown_flags(comando: &str, args: &[String]) -> Result<(), PlenoraError> {
    // `--help` e `--version` non prendono argomenti: qualunque token in piu'
    // e' un'invocazione che non si sta eseguendo.
    if matches!(comando, "--help" | "-h" | "--version" | "-V") {
        // `--json` e' il modificatore di formato di `--version` E SOLO SUO:
        // su `--help` veniva accettato e ignorato, cioe' un'invocazione che
        // il parser non eseguiva ma dichiarava valida.
        let ammette_json = matches!(comando, "--version" | "-V");
        let mut json_visto = false;
        for argument in args.iter().skip(1) {
            if ammette_json && argument.as_str() == "--json" && !json_visto {
                json_visto = true;
                continue;
            }
            return Err(contract(format!(
                "`{comando}` non accetta argomenti: `{argument}` di troppo"
            )));
        }
        return Ok(());
    }
    let Some(superficie) = superficie(comando) else {
        return Ok(());
    };
    let mut visti: Vec<&str> = Vec::new();
    let mut indice = 1;
    while indice < args.len() {
        let argument = args[indice].as_str();
        if argument == "--help" || argument == "-h" {
            // Ammesso, ma NON e' un lasciapassare per il resto della riga:
            // `run --help junk` deve fallire come qualunque altra
            // invocazione con un token estraneo.
            //
            // `--help` e `-h` sono lo STESSO flag: si registra la forma
            // canonica, altrimenti `run --help -h` non risultava una
            // ripetizione e passava.
            if visti.contains(&"--help") {
                return Err(contract(format!(
                    "flag `{argument}` ripetuto: `{comando}` ne accetta una sola occorrenza"
                )));
            }
            visti.push("--help");
            indice += 1;
            continue;
        }
        if !argument.starts_with('-') {
            return Err(contract(format!(
                "argomento posizionale `{argument}` non atteso da `{comando}`: \
                 ogni valore va introdotto dal proprio flag"
            )));
        }
        if !argument.starts_with("--") {
            // Forma breve: nessun sottocomando ne dichiara, e accettarla in
            // silenzio significherebbe ignorarla.
            return Err(contract(format!(
                "flag `{argument}` non riconosciuto da `{comando}`: le opzioni \
                 sono nella forma lunga `--nome`"
            )));
        }
        if !superficie.flag.contains(&argument) {
            return Err(contract(format!(
                "flag `{argument}` non riconosciuto da `{comando}` (ammessi: {})",
                if superficie.flag.is_empty() {
                    "nessuno".to_owned()
                } else {
                    superficie.flag.join(", ")
                }
            )));
        }
        if visti.contains(&argument) && !superficie.ripetibili.contains(&argument) {
            return Err(contract(format!(
                "flag `{argument}` ripetuto: `{comando}` ne accetta una sola occorrenza"
            )));
        }
        visti.push(argument);
        indice += 1;
        if argument == "--no-geo-fusion" {
            // Flag senza valore.
            continue;
        }
        if argument == "--inputs" {
            // Lista: consuma i valori fino al prossimo flag, ma almeno uno.
            let inizio = indice;
            while indice < args.len() && !(args[indice].starts_with('-') && args[indice].len() > 1)
            {
                indice += 1;
            }
            if indice == inizio {
                return Err(contract(format!("valore mancante per {argument}")));
            }
            continue;
        }
        // Flag a valore singolo: il valore deve esserci e NON deve essere un
        // altro flag. `--plan --output out.arrow` prendeva `--output` come
        // nome del piano e falliva molto piu' tardi, con un errore che non
        // parlava del vero problema.
        let Some(valore) = args.get(indice) else {
            return Err(contract(format!("valore mancante per {argument}")));
        };
        // Anche la forma breve e' un flag: `--plan -x` consumava `-x` come
        // nome del piano e falliva molto piu' tardi, con un errore che non
        // parlava del vero problema.
        if valore.starts_with('-') && valore.len() > 1 {
            return Err(contract(format!(
                "valore mancante per {argument}: `{valore}` e' un flag, non un valore"
            )));
        }
        indice += 1;
    }
    Ok(())
}
