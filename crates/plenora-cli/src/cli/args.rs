//! Superficie degli argomenti: aiuto, flag ammessi, e il loro rifiuto.
//!
//! [`superficie`] dichiara che cosa ogni comando accetta, e
//! [`reject_unknown_flags`] lo fa rispettare prima di qualunque uscita
//! anticipata, help compreso: un parser che accetta un'invocazione che non ha
//! capito e' fail-open. I testi di aiuto stanno qui accanto, e `matrice_cli`
//! verifica che dichiarino esattamente i flag del dispatch.

use plenora_core::PlenoraError;

use crate::contract;

pub fn help_text() -> String {
    format!(
        "plenora-data-tools {}

  plenora-data-tools catalog [--family table|geo]
  plenora-data-tools describe --input INPUT.arrow                          (alias: inspect-dataset)
  plenora-data-tools validate --plan PLAN.json --input NOME=INPUT.arrow...
  plenora-data-tools run --plan PLAN.json --input NOME=INPUT.arrow... --output OUTPUT.arrow [--no-geo-fusion]   (piani DAG v5 e v6)
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
/// L'unico posto in cui la superficie e' dichiarata: il controllo di §1.4 la
/// confronta con l'help, il dispatch la usa per rifiutare. `--format` non
/// compare: e' globale e lo toglie prima `strip_output_format`.
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
/// Ogni argomento e' un flag dichiarato o il valore di un flag che ne prende
/// uno. Si rifiutano flag sconosciuti (anche brevi), flag a valore singolo
/// ripetuti, posizionali inattesi, flag usati come valore (`--plan
/// --output`) e argomenti extra dopo `--version` e `--help`.
///
/// # Errors
///
/// `PlenoraError::InvalidPlan` con l'elenco dei flag ammessi.
pub fn reject_unknown_flags(comando: &str, args: &[String]) -> Result<(), PlenoraError> {
    // `--help` e `--version` non prendono argomenti: qualunque token in piu'
    // e' un'invocazione che non si sta eseguendo.
    if matches!(comando, "--help" | "-h" | "--version" | "-V") {
        // `--json` e' il modificatore di formato di `--version` E SOLO SUO:
        // accettarlo e ignorarlo su `--help` significherebbe dichiarare
        // valida un'invocazione che il parser non esegue.
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
            // canonica, altrimenti `run --help -h` non risulterebbe una
            // ripetizione e passerebbe.
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
        // altro flag. Senza questo controllo `--plan --output out.arrow`
        // prenderebbe `--output` come nome del piano e fallirebbe molto piu'
        // tardi, con un errore che non parla del vero problema.
        let Some(valore) = args.get(indice) else {
            return Err(contract(format!("valore mancante per {argument}")));
        };
        // Anche la forma breve e' un flag: senza questo controllo `--plan -x`
        // consumerebbe `-x` come nome del piano, con lo stesso fallimento
        // tardivo e lo stesso errore fuori bersaglio.
        if valore.starts_with('-') && valore.len() > 1 {
            return Err(contract(format!(
                "valore mancante per {argument}: `{valore}` e' un flag, non un valore"
            )));
        }
        indice += 1;
    }
    Ok(())
}
