//! I fork vendorizzati valgono anche per chi dipende da data-tools fuori dal
//! suo workspace.
//!
//! `geo`, `wkt` e `parquet` sono copie vendorizzate con patch (Cargo.toml
//! radice, «Copie vendorizzate»). Finché erano `[patch.crates-io]`, Cargo le
//! applicava solo compilando questo workspace: un crate che dipendeva da
//! data-tools per percorso o git riceveva `geo` 0.33.1 con `i_overlay`
//! 4.5.2, `wkt` e `parquet` di crates.io, cioè geometrie, WKT e letture
//! Parquet diverse senza nessun errore. Ora i fork hanno un nome di
//! pacchetto proprio (`plenora-geo`, `plenora-wkt`, `plenora-parquet`) e
//! sono dipendenze normali.
//!
//! La prova costruisce un crate consumatore in una cartella temporanea,
//! fuori dal workspace, che dipende per percorso da ogni crate di libreria,
//! e legge con `cargo metadata` il grafo che Cargo compilerebbe: lo stesso
//! controllo vale per il workspace e per quello separato di `fuzz/`.
//! Il consumatore ha accanto una copia del `Cargo.lock` del workspace, così
//! il risultato non dipende dalle release uscite dopo; il workspace e
//! `fuzz/` si risolvono con `--locked`. Niente `--offline`: un job che non
//! ha scaricato le dipendenze di `fuzz/` (la copertura) non le troverebbe
//! nella cache, e `cargo metadata` le scarica come farebbe una build.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

/// I fork: nome del pacchetto e cartella sotto `vendor/`.
const FORK: [(&str, &str); 3] = [
    ("plenora-geo", "geo-0.33.1-exact-filtered"),
    ("plenora-wkt", "wkt-0.14.0-v2"),
    ("plenora-parquet", "parquet-60.0.0-eof"),
];

/// I crate di libreria che un consumatore Rust può usare
/// (`plenora-data-py` è il modulo Python, non una libreria Rust).
const LIBRERIE: [&str; 6] = [
    "plenora-core",
    "plenora-kernels-table",
    "plenora-kernels-geo",
    "plenora-pipeline",
    "plenora-io",
    "plenora-cli",
];

/// L'unica `i_overlay` ammessa: quella che il `geo` vendorizzato fissa
/// (`patches/geo-i-overlay-9.patch`).
const I_OVERLAY: &str = "9.0.0";

fn radice() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("radice del repository")
        .to_path_buf()
}

fn cargo() -> String {
    std::env::var("CARGO").unwrap_or_else(|_| env!("CARGO").to_owned())
}

/// Il grafo risolto di `manifesto`; `bloccato` aggiunge `--locked` (il
/// lockfile non può cambiare).
fn metadati(manifesto: &Path, bloccato: bool) -> Value {
    let mut comando = Command::new(cargo());
    comando
        .args(["metadata", "--format-version", "1"])
        .arg("--manifest-path")
        .arg(manifesto);
    if bloccato {
        comando.arg("--locked");
    }
    let uscita = comando.output().expect("cargo metadata si avvia");
    assert!(
        uscita.status.success(),
        "cargo metadata fallisce su {}: {}",
        manifesto.display(),
        String::from_utf8_lossy(&uscita.stderr)
    );
    serde_json::from_slice(&uscita.stdout).expect("JSON di cargo metadata")
}

fn canonico(percorso: &Path) -> PathBuf {
    fs::canonicalize(percorso).expect("percorso esistente")
}

/// Il grafo risolto rispetta i fork: ognuno una volta sola, dal suo
/// percorso sotto `vendor/`; nessun `geo` o `parquet` di crates.io;
/// `i_overlay` solo nella versione del fork; una sola `geo-types`; il `wkt`
/// di crates.io solo come dipendenza di `geozero` (il suo lettore WKT,
/// vietato nel codice: `clippy.toml` e la prova sotto).
fn verifica_grafo(metadati: &Value, contesto: &str) {
    let vendor = radice().join("vendor");
    let pacchetti = metadati["packages"].as_array().expect("packages");
    let mut per_nome: BTreeMap<&str, Vec<&Value>> = BTreeMap::new();
    for pacchetto in pacchetti {
        per_nome
            .entry(pacchetto["name"].as_str().expect("name"))
            .or_default()
            .push(pacchetto);
    }

    for nome in ["geo", "parquet"] {
        let sorgenti: Vec<&str> = per_nome
            .get(nome)
            .map_or(&[][..], Vec::as_slice)
            .iter()
            .map(|pacchetto| pacchetto["source"].as_str().unwrap_or("percorso"))
            .collect();
        assert!(
            sorgenti.is_empty(),
            "{contesto}: `{nome}` con il nome upstream nel grafo ({sorgenti:?}): \
             fuori da questo workspace arriva da crates.io senza fork, e \
             geometrie o letture Parquet cambiano in silenzio"
        );
    }

    let versioni: BTreeSet<&str> = per_nome
        .get("i_overlay")
        .map_or(&[][..], Vec::as_slice)
        .iter()
        .map(|pacchetto| pacchetto["version"].as_str().expect("version"))
        .collect();
    assert_eq!(
        versioni,
        BTreeSet::from([I_OVERLAY]),
        "{contesto}: `i_overlay` diversa da quella del fork"
    );

    assert_eq!(
        per_nome.get("geo-types").map_or(0, Vec::len),
        1,
        "{contesto}: `geo-types` deve essere una sola (stessi tipi fra i crate)"
    );

    // Chi dipende dal `wkt` di crates.io: solo `geozero`.
    let nome_di_id: BTreeMap<&str, &str> = pacchetti
        .iter()
        .map(|pacchetto| {
            (
                pacchetto["id"].as_str().expect("id"),
                pacchetto["name"].as_str().expect("name"),
            )
        })
        .collect();
    let nodi = metadati["resolve"]["nodes"].as_array().expect("resolve");
    for nodo in nodi {
        let da = nome_di_id[nodo["id"].as_str().expect("id")];
        for dipendenza in nodo["deps"].as_array().expect("deps") {
            let verso = nome_di_id[dipendenza["pkg"].as_str().expect("pkg")];
            if verso == "wkt" {
                assert_eq!(
                    da, "geozero",
                    "{contesto}: `{da}` dipende da `wkt` (nome upstream: fuori da \
                     questo workspace è quello di crates.io) invece di `plenora-wkt`"
                );
            }
        }
    }

    for (nome, cartella) in FORK {
        let copie = per_nome.get(nome).map_or(&[][..], Vec::as_slice);
        assert_eq!(
            copie.len(),
            1,
            "{contesto}: `{nome}` deve esserci una volta"
        );
        assert!(
            copie[0]["source"].is_null(),
            "{contesto}: `{nome}` deve venire da un percorso"
        );
        let manifesto = Path::new(copie[0]["manifest_path"].as_str().expect("manifest_path"));
        assert_eq!(
            canonico(manifesto),
            canonico(&vendor.join(cartella).join("Cargo.toml")),
            "{contesto}: `{nome}` deve venire da vendor/{cartella}"
        );
    }
}

#[test]
fn un_consumatore_fuori_dal_workspace_riceve_i_fork() {
    let cartella = tempfile::tempdir().expect("cartella temporanea");
    let radice = radice();
    let mut dipendenze = String::new();
    for crate_ in LIBRERIE {
        // Stringa letterale TOML: nessun escape dei separatori di Windows.
        let percorso = radice.join("crates").join(crate_);
        writeln!(
            dipendenze,
            "{crate_} = {{ path = '{}' }}",
            percorso.display()
        )
        .expect("testo in memoria");
    }
    let manifesto = cartella.path().join("Cargo.toml");
    fs::write(
        &manifesto,
        format!(
            "[package]\nname = \"consumatore-esterno\"\nversion = \"0.0.0\"\n\
             edition = \"2021\"\npublish = false\n\n\
             # Un workspace proprio: il consumatore non eredita nulla da quello di data-tools.\n\
             [workspace]\n\n[dependencies]\n{dipendenze}"
        ),
    )
    .expect("manifesto del consumatore");
    fs::create_dir(cartella.path().join("src")).expect("src");
    fs::write(cartella.path().join("src").join("lib.rs"), "").expect("lib.rs");
    fs::copy(
        radice.join("Cargo.lock"),
        cartella.path().join("Cargo.lock"),
    )
    .expect("Cargo.lock");

    let metadati = metadati(&manifesto, false);
    // Il consumatore è davvero fuori: la radice del suo workspace è la sua.
    assert_eq!(
        canonico(Path::new(
            metadati["workspace_root"].as_str().expect("workspace_root")
        )),
        canonico(cartella.path()),
    );
    verifica_grafo(&metadati, "consumatore esterno");
}

#[test]
fn il_workspace_e_quello_di_fuzz_ricevono_i_fork() {
    let radice = radice();
    verifica_grafo(&metadati(&radice.join("Cargo.toml"), true), "workspace");
    verifica_grafo(
        &metadati(&radice.join("fuzz").join("Cargo.toml"), true),
        "workspace di fuzz",
    );
}

/// `geozero::wkt` legge il WKT con il `wkt` di crates.io. `clippy.toml`
/// vieta i suoi tipi nelle posizioni di tipo e negli `use`, ma non un
/// costruttore scritto per esteso (`geozero::wkt::Wkt(...)`): questa prova
/// chiude il caso cercando il modulo nei sorgenti del workspace e di `fuzz/`.
#[test]
fn nessun_sorgente_usa_il_lettore_wkt_di_geozero() {
    let radice = radice();
    let mut file = Vec::new();
    for cartella in ["crates", "fuzz"] {
        raccogli_rs(&radice.join(cartella), &mut file);
    }
    assert!(!file.is_empty(), "nessun sorgente trovato");
    // Questo file nomina il modulo negli esempi della prova sotto.
    let questo = canonico(&radice.join(file!()));
    let mut violazioni = Vec::new();
    for percorso in file {
        if canonico(&percorso) == questo {
            continue;
        }
        let testo = fs::read_to_string(&percorso).expect("sorgente UTF-8");
        if usa_geozero_wkt(&testo) {
            violazioni.push(percorso);
        }
    }
    assert!(
        violazioni.is_empty(),
        "il lettore WKT di geozero usa il wkt di crates.io: {violazioni:?}"
    );
}

#[test]
fn la_ricerca_di_geozero_wkt_riconosce_le_forme() {
    for vietato in [
        "let g = geozero::wkt::Wkt(\"POINT(1 2)\");",
        "use geozero :: wkt::WktStr;",
        "use geozero::{ToGeo, wkt::Wkt};",
        "use geozero::{wkt, ToGeo};",
        "use geozero::{wkt as w};",
        "use geozero::{\n    wkb::Wkb,\n    wkt::{Ewkt, Wkt},\n};",
    ] {
        assert!(usa_geozero_wkt(vietato), "{vietato}");
    }
    for ammesso in [
        "use geozero::{wkb::Wkb, CoordDimensions, ToGeo, ToWkb};",
        "use wkt::TryFromWkt;",
        "let s = geometria.try_wkt_string();",
        "use geozero::{wkt_like, ToWkb};",
    ] {
        assert!(!usa_geozero_wkt(ammesso), "{ammesso}");
    }
}

fn raccogli_rs(cartella: &Path, file: &mut Vec<PathBuf>) {
    for voce in fs::read_dir(cartella).expect("cartella leggibile") {
        let percorso = voce.expect("voce").path();
        if percorso.is_dir() {
            if percorso.file_name().is_some_and(|nome| nome == "target") {
                continue;
            }
            raccogli_rs(&percorso, file);
        } else if percorso
            .extension()
            .is_some_and(|estensione| estensione == "rs")
        {
            file.push(percorso);
        }
    }
}

/// `geozero::wkt` per esteso o dentro un gruppo `geozero::{...}`, a
/// qualunque profondità, con o senza spazi.
fn usa_geozero_wkt(testo: &str) -> bool {
    let compatto: String = testo.chars().filter(|c| !c.is_whitespace()).collect();
    let mut resto = compatto.as_str();
    while let Some(posizione) = resto.find("geozero::") {
        resto = &resto[posizione + "geozero::".len()..];
        if inizia_con_wkt(resto) {
            return true;
        }
        if let Some(gruppo) = resto.strip_prefix('{') {
            let mut profondita = 1_usize;
            let mut fine = gruppo.len();
            for (indice, carattere) in gruppo.char_indices() {
                match carattere {
                    '{' => profondita += 1,
                    '}' => {
                        profondita -= 1;
                        if profondita == 0 {
                            fine = indice;
                            break;
                        }
                    }
                    _ => {}
                }
            }
            if gruppo[..fine].split([',', '{']).any(inizia_con_wkt) {
                return true;
            }
        }
    }
    false
}

/// Il segmento comincia con il modulo `wkt` (`wkt`, `wkt::...`, `wkt as`,
/// spazi già tolti), non con un nome che ne è solo il prefisso.
fn inizia_con_wkt(segmento: &str) -> bool {
    segmento.strip_prefix("wkt").is_some_and(|dopo| {
        dopo.is_empty()
            || dopo.starts_with("::")
            || dopo.starts_with('}')
            || dopo.starts_with(',')
            || dopo.starts_with(';')
            || dopo.starts_with("as")
    })
}
