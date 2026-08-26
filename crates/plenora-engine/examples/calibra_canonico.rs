//! **Calibrazione** di `MAX_PIANO_CANONICO_BYTES`, non prova del limite.
//!
//! Nessun insieme finito di piani dimostra il rapporto massimo per tutti i
//! documenti validi. Questo harness serve a scegliere un limite di **policy**
//! con margine dichiarato, e a rendere quella scelta riproducibile. Il
//! presidio vero sta nel protocollo: un writer limitato che si ferma appena
//! supera il tetto.
//!
//! # Il modello: due termini, non un rapporto
//!
//! ```text
//!     canonico  ≈  overhead  +  b × testo
//! ```
//!
//! - **`overhead`**: cio' che il piano minimo guadagna passando al canonico.
//!   Comprende il blocco `limits` materializzato **e** involucro, nodo,
//!   input e normalizzazioni della fixture: e' un overhead **osservato**, non
//!   una misura del solo blocco `limits`. Su un piano minuscolo domina, e
//!   produce rapporti altissimi che **non scalano**;
//! - **`b`**: la riscrittura del contenuto, proporzionale. Soprattutto
//!   `canonical_number`, che porta un float a valore intero alla forma intera
//!   quando `|v| <= 2^53`: `1e15` sono quattro caratteri e diventano sedici
//!   cifre.
//!
//! # Il caso peggiore va scritto a mano
//!
//! `Value::from(1.0e15)` serializza gia' `1000000000000000.0`: il testo
//! conterrebbe gia' la forma lunga e non ci sarebbe nulla da espandere. La
//! notazione compatta esiste solo se il testo la scrive.
//!
//! ```text
//! cargo run -p plenora-engine --example calibra_canonico
//! ```

use plenora_core::limits::PlanLimits;
use plenora_engine::interni::MAX_PIANO_CANONICO_BYTES;
use plenora_engine::plan::{valida_per_versione, PLAN_SCHEMA_VERSION_V6};

struct Caso {
    nome: &'static str,
    perche: &'static str,
    testo: String,
}

/// Involucro di un piano **v6**, con il campo che solo la v6 ha.
///
/// La sonda deve attraversare il formato che il profilo isolato trasporta:
/// misurare la v5 misurerebbe un documento diverso da quello che finisce sul
/// filo.
fn piano_v6(nodi: &str, output: &str) -> String {
    format!(
        r#"{{"schema_version":{PLAN_SCHEMA_VERSION_V6},"limits":{{"max_domain_memory_bytes":1073741824}},"inputs":["main"],"nodes":[{nodi}],"output":"{output}"}}"#
    )
}

/// Nodo unario con `op` e `config` dati come testo grezzo.
fn nodo(id: &str, op: &str, ingresso: &str, config: &str) -> String {
    format!(r#"{{"id":"{id}","op":"{op}","in":["{ingresso}"],"config":{config}}}"#)
}

const CONFIG_PICCOLA: &str = r#"{"column":"id","operator":">","value":0}"#;

/// L'alias con la massima espansione verso l'id canonico, **enumerando**
/// `ALIASES`.
///
/// Dedurlo da un solo alias sarebbe stato un'assunzione travestita da misura:
/// basta che ne venga aggiunto uno piu' lungo perche' il caso peggiore si
/// sposti senza che nessuno se ne accorga.
fn alias_piu_espansivo() -> (&'static str, &'static str, usize) {
    let mut migliore = ("", "", 0_usize);
    for (_, alias, canonico) in plenora_core::catalog::ALIASES {
        let espansione = canonico.len().saturating_sub(alias.len());
        if espansione > migliore.2 {
            migliore = (alias, canonico, espansione);
        }
    }
    migliore
}

/// Config con `quanti` numeri in notazione compatta: cinque caratteri col
/// separatore contro diciassette.
fn config_compatta(quanti: usize) -> String {
    let mut config = String::from(r#"{"column":"id","operator":">","value":0,"misura":["#);
    for indice in 0..quanti {
        if indice > 0 {
            config.push(',');
        }
        config.push_str("1e15");
    }
    config.push_str("]}");
    config
}

/// Catena di `quanti` nodi: rispetta `max_fan_out`, e' vincolata da
/// `max_plan_depth`.
fn catena(config: &str, quanti: usize, op: &str) -> String {
    let mut nodi = Vec::with_capacity(quanti);
    for indice in 0..quanti {
        let ingresso = if indice == 0 {
            "main".to_owned()
        } else {
            format!("n{}", indice - 1)
        };
        nodi.push(nodo(&format!("n{indice}"), op, &ingresso, config));
    }
    nodi.join(",")
}

/// DAG **ramificato e ricongiunto**, vicino a `max_plan_nodes`.
///
/// Correzione di un'affermazione sbagliata della prima stesura: `max_plan_depth`
/// limita una **catena**, non il numero totale di nodi. Con `rami` catene
/// parallele da `main` — al piu' `max_fan_out` — e un `table.concat` finale
/// che le ricongiunge, si arriva vicino a 1 024 nodi restando entro
/// profondita' e fan-out.
///
/// Il `concat` finale serve anche a un'altra proprieta': **ogni nodo e'
/// antenato dell'output**, quindi nessun nodo morto.
fn dag_ramificato(config: &str, rami: usize, per_ramo: usize) -> (String, String) {
    let mut nodi = Vec::new();
    let mut code = Vec::with_capacity(rami);
    for ramo in 0..rami {
        for passo in 0..per_ramo {
            let ingresso = if passo == 0 {
                "main".to_owned()
            } else {
                format!("r{ramo}s{}", passo - 1)
            };
            nodi.push(nodo(
                &format!("r{ramo}s{passo}"),
                "table.filter",
                &ingresso,
                config,
            ));
        }
        code.push(format!("r{ramo}s{}", per_ramo - 1));
    }
    let ingressi = code
        .iter()
        .map(|id| format!(r#""{id}""#))
        .collect::<Vec<_>>()
        .join(",");
    nodi.push(format!(
        r#"{{"id":"unione","op":"table.concat","in":[{ingressi}],"config":{{}}}}"#
    ));
    (nodi.join(","), "unione".to_owned())
}

fn casi(alias: &str, alias_perche: &'static str) -> Vec<Caso> {
    let (dag, uscita_dag) = dag_ramificato(CONFIG_PICCOLA, 63, 16);
    let (dag_denso, uscita_denso) = dag_ramificato(&config_compatta(150), 63, 16);
    vec![
        Caso {
            nome: "minimo",
            perche: "overhead osservato del piano minimo",
            testo: piano_v6(&catena(CONFIG_PICCOLA, 1, "table.filter"), "n0"),
        },
        Caso {
            nome: "catena 250, config piccola",
            perche: "overhead ammortizzato: il rapporto scende verso b",
            testo: piano_v6(&catena(CONFIG_PICCOLA, 250, "table.filter"), "n249"),
        },
        Caso {
            nome: "alias con espansione massima",
            perche: alias_perche,
            testo: piano_v6(&catena("{}", 250, alias), "n249"),
        },
        Caso {
            nome: "DAG ramificato ~1009 nodi",
            perche: "63 rami x 16 + concat: tutti antenati dell'output",
            testo: piano_v6(&dag, &uscita_dag),
        },
        Caso {
            nome: "DAG ramificato, config densa",
            perche: "molti nodi E numeri espandibili insieme",
            testo: piano_v6(&dag_denso, &uscita_denso),
        },
        Caso {
            nome: "numeri compatti, catena 250",
            perche: "il caso peggiore osservato finora",
            testo: piano_v6(
                &catena(&config_compatta(2_000), 250, "table.filter"),
                "n249",
            ),
        },
        Caso {
            nome: "numeri compatti, ~97% del tetto",
            perche: "il caso peggiore alla scala che conta (non AL tetto: ~500 KiB sotto)",
            testo: piano_v6(
                &catena(&config_compatta(13_000), 250, "table.filter"),
                "n249",
            ),
        },
    ]
}

// Il limite non e' piu' definito qui. La sonda **misura** e confronta con
// l'unica autorita', `interni::MAX_PIANO_CANONICO_BYTES`: se
// qualcuno abbassasse quel numero sotto la proiezione, questa sonda uscirebbe
// non-zero invece di continuare a dichiarare un margine che non esiste piu'.

/// Le ragioni per cui la sonda esce non-zero.
///
/// Stampare un'avvertenza e terminare con successo sarebbe un fallimento
/// silenzioso: chi la lancia in una catena non se ne accorgerebbe.
fn calibra() -> Result<(), String> {
    let limiti = PlanLimits::default();
    let (alias, canonico_alias, espansione) = alias_piu_espansivo();
    let alias_perche: &'static str = Box::leak(
        format!("`{alias}` -> `{canonico_alias}`: {espansione} byte per nodo").into_boxed_str(),
    );

    println!(
        "{:<40} {:>12} {:>12} {:>9}  perche'",
        "caso", "testo", "canonico", "rapporto"
    );
    println!("{}", "-".repeat(120));

    let mut misure: Vec<(&'static str, usize, usize)> = Vec::new();
    let mut rifiutati: Vec<String> = Vec::new();
    for caso in casi(alias, alias_perche) {
        let byte_testo = caso.testo.len();
        match valida_per_versione(&caso.testo, &limiti) {
            Ok(validato) => {
                let byte_canonico = validato.canonical_json().to_string().len();
                #[allow(clippy::cast_precision_loss)]
                let rapporto = byte_canonico as f64 / byte_testo as f64;
                println!(
                    "{:<40} {byte_testo:>12} {byte_canonico:>12} {rapporto:>9.3}  {}",
                    caso.nome, caso.perche
                );
                misure.push((caso.nome, byte_testo, byte_canonico));
            }
            Err(errore) => {
                println!(
                    "{:<40} {byte_testo:>12} {:>12} {:>9}  RIFIUTATO: {errore}",
                    caso.nome, "-", "-"
                );
                rifiutati.push(format!("«{}»: {errore}", caso.nome));
            }
        }
    }

    if !rifiutati.is_empty() {
        return Err(format!(
            "casi attesi rifiutati ({}): {}. Una fixture che non passa non e' una misura \
             mancante, e' una fixture sbagliata",
            rifiutati.len(),
            rifiutati.join("; ")
        ));
    }
    let Some(minimo) = misure.first().copied() else {
        return Err("nessuna misura prodotta".to_owned());
    };
    let Some(piu_grande) = misure.iter().copied().max_by_key(|(_, testo, _)| *testo) else {
        return Err("nessuna misura prodotta".to_owned());
    };
    let Some(peggiore) = misure.iter().copied().max_by(|sinistra, destra| {
        #[allow(clippy::cast_precision_loss)]
        let rapporto = |m: &(&str, usize, usize)| m.2 as f64 / m.1 as f64;
        rapporto(sinistra).total_cmp(&rapporto(destra))
    }) else {
        return Err("nessuna misura prodotta".to_owned());
    };

    let overhead = minimo
        .2
        .checked_sub(minimo.1)
        .ok_or_else(|| "il canonico minimo e' piu' piccolo del testo minimo".to_owned())?;

    // Proiezione in aritmetica INTERA e per ECCESSO:
    //
    //     proiezione = overhead + ceil((canonico_max - overhead) * tetto / testo_max)
    //
    // Il fattore `b` resta implicito nella frazione, quindi non c'e' nessun
    // arrotondamento intermedio. Passare da `f64` e troncare verso `usize`
    // puo' dare un numero PIU' PICCOLO del vero, proprio dove serve un
    // maggiorante.
    let tetto = limiti.max_plan_json_bytes;
    let crescita = piu_grande
        .2
        .checked_sub(overhead)
        .ok_or_else(|| "overhead maggiore del canonico piu' grande".to_owned())?;
    let numeratore = u128::try_from(crescita)
        .map_err(|_| "crescita fuori intervallo".to_owned())?
        .checked_mul(u128::try_from(tetto).map_err(|_| "tetto fuori intervallo".to_owned())?)
        .ok_or_else(|| "traboccamento nel prodotto della proiezione".to_owned())?;
    let denominatore =
        u128::try_from(piu_grande.1).map_err(|_| "testo fuori intervallo".to_owned())?;
    if denominatore == 0 {
        return Err("denominatore nullo nella proiezione".to_owned());
    }
    let proiezione = usize::try_from(numeratore.div_ceil(denominatore))
        .map_err(|_| "proiezione fuori intervallo".to_owned())?
        .checked_add(overhead)
        .ok_or_else(|| "traboccamento nella somma della proiezione".to_owned())?;

    riporta(
        &misure, overhead, crescita, proiezione, tetto, minimo, piu_grande, peggiore,
    )
}

/// La parte che stampa, separata da quella che misura.
///
/// Non e' cosmesi: `calibra` faceva due lavori e superava le cento righe, e
/// una funzione che misura *e* riporta e' anche una funzione dove un errore
/// di stampa puo' nascondere un errore di misura.
#[allow(clippy::too_many_arguments)]
fn riporta(
    _misure: &[(&'static str, usize, usize)],
    overhead: usize,
    crescita: usize,
    proiezione: usize,
    tetto: usize,
    _minimo: (&'static str, usize, usize),
    piu_grande: (&'static str, usize, usize),
    peggiore: (&'static str, usize, usize),
) -> Result<(), String> {
    println!();
    println!("modello  canonico ≈ overhead + b × testo");
    println!("  overhead = {overhead} byte  (piano minimo: NON solo il blocco `limits`)");
    #[allow(clippy::cast_precision_loss)]
    let b = crescita as f64 / piu_grande.1 as f64;
    println!("  b ≈ {b:.4}  (dal testo piu' grande: «{}»)", piu_grande.0);
    #[allow(clippy::cast_precision_loss)]
    let rapporto_peggiore = peggiore.2 as f64 / peggiore.1 as f64;
    println!(
        "  rapporto massimo osservato: {rapporto_peggiore:.3} nel caso «{}»",
        peggiore.0
    );

    println!();
    println!("proiezione su un testo al tetto ({tetto} byte, da PlanLimits::default):");
    #[allow(clippy::cast_precision_loss)]
    let mib = proiezione as f64 / (1024.0 * 1024.0);
    println!("  {proiezione} byte  (~{mib:.2} MiB), aritmetica intera per eccesso");

    println!();
    println!("MAX_PIANO_CANONICO_BYTES del protocollo: {MAX_PIANO_CANONICO_BYTES}");
    // I DUE margini sono cose diverse, e confonderli fa sembrare piu' ampio
    // quello che conta: la proiezione e' un maggiorante costruito, il massimo
    // osservato e' un numero misurato.
    let Some(margine_proiezione) = MAX_PIANO_CANONICO_BYTES.checked_sub(proiezione) else {
        return Err(format!(
            "la proiezione ({proiezione}) SUPERA il limite del protocollo ({MAX_PIANO_CANONICO_BYTES}): \
             il limite non e' difendibile"
        ));
    };
    #[allow(clippy::cast_precision_loss)]
    let percento_proiezione = margine_proiezione as f64 * 100.0 / proiezione as f64;
    println!("  margine sulla PROIEZIONE: {margine_proiezione} byte ({percento_proiezione:.1} %)");
    let osservato = piu_grande.2;
    let margine_osservato = MAX_PIANO_CANONICO_BYTES
        .checked_sub(osservato)
        .ok_or_else(|| format!("il massimo osservato ({osservato}) supera il limite"))?;
    #[allow(clippy::cast_precision_loss)]
    let percento_osservato = margine_osservato as f64 * 100.0 / osservato as f64;
    println!(
        "  margine sul MASSIMO OSSERVATO ({osservato} byte): {margine_osservato} byte \
         ({percento_osservato:.1} %)"
    );

    println!();
    println!("Calibrazione, non prova: nessun insieme finito di piani fissa `b` per");
    println!("tutti i documenti validi. Il presidio e' il writer limitato del protocollo.");
    println!();
    println!("Sulla larghezza strutturale: NON domina *nelle fixture misurate*. Non");
    println!("autorizza a togliere i DAG dalle campagne future — dice soltanto che in");
    println!("questi piani i nodi non espandono e i numeri si'.");
    Ok(())
}

fn main() -> std::process::ExitCode {
    match calibra() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(motivo) => {
            eprintln!();
            eprintln!("CALIBRAZIONE FALLITA: {motivo}");
            std::process::ExitCode::FAILURE
        }
    }
}
