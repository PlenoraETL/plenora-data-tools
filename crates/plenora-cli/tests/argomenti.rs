//! Invocazioni fuori grammatica: falliscono chiuse, con l'inviluppo
//! d'errore e exit 2, senza ripetere l'argomento ricevuto (CLI 2.0,
//! sezione 2).

mod comune;

use comune::{esito, invoca};

const CANARINO: &str = "CANARINO-arg-55e1";

#[test]
fn argomenti_fuori_grammatica_falliscono_chiusi() {
    let casi: &[&[&str]] = &[
        &[],
        &["--format", "json"],
        &[CANARINO],
        &["catalog", CANARINO],
        &["catalog", "--input", CANARINO],
        &["catalog", "--deadline", "2030-01-01T00:00:00Z"],
        &["capabilities", "--verbose"],
        &["--version", "--format", "json", "--format", "json"],
        &["--version", "--format", "xml"],
        &["--version", "--format"],
        &["describe"],
        &["describe", "--input"],
        &["describe", "--input", "--format", "json"],
        &["describe", "--input", CANARINO, "--input", CANARINO],
        &["describe", "--input", CANARINO, "--timeout-ms", "uno"],
        &["describe", "--input", CANARINO, "--timeout-ms", "+5"],
        &[
            "describe",
            "--input",
            CANARINO,
            "--timeout-ms",
            "1",
            "--deadline",
            "2030-01-01T00:00:00Z",
        ],
        &["validate", "--input", "t=x.arrow"],
        &["validate", "--plan", CANARINO, "--plan", CANARINO],
        &["validate", "--plan", CANARINO, "--input", CANARINO],
        &["validate", "--plan", CANARINO, "--input", "=x"],
        &["validate", "--plan", CANARINO, "--output", "x.arrow"],
        &["run", "--plan", CANARINO],
        &[
            "run", "--plan", CANARINO, "--output", "a=x", "--output", CANARINO,
        ],
        &[
            "run",
            "--plan",
            CANARINO,
            "--output",
            "x",
            "--overwrite",
            "--overwrite",
        ],
        &[
            "run",
            "--plan",
            CANARINO,
            "--output",
            "x",
            "--overwrite",
            "si",
        ],
        &["inspect-dataset", "--input", CANARINO],
    ];
    for argomenti in casi {
        let esito = invoca(argomenti);
        assert_eq!(esito.codice, 2, "{argomenti:?}: {}", esito.stdout);
        assert_eq!(
            esito.documento["error"]["category"],
            "invalid_configuration"
        );
        assert_eq!(esito.documento["error"]["remote_effect"], "none");
        assert!(
            !esito.stdout.contains(CANARINO),
            "{argomenti:?}: {}",
            esito.stdout
        );
    }
}

#[test]
fn il_comando_dell_inviluppo_d_errore() {
    let esito = invoca(&["describe"]);
    assert_eq!(esito.documento["command"], "describe");
    assert_eq!(esito.documento["contract"], "plenora-data-description-v1");
    let esito = invoca(&["sconosciuto"]);
    assert_eq!(esito.documento["command"], "unknown");
    assert_eq!(esito.documento["contract"], "plenora-error-v1");
}

/// Un argomento che non è UTF-8 si rifiuta: tradurlo cambierebbe in
/// silenzio un percorso.
#[test]
fn argomento_non_utf8() {
    #[cfg(windows)]
    let grezzo = {
        use std::os::windows::ffi::OsStringExt;
        std::ffi::OsString::from_wide(&[0x0061, 0xD800, 0x0062])
    };
    #[cfg(unix)]
    let grezzo = {
        use std::os::unix::ffi::OsStringExt;
        std::ffi::OsString::from_vec(vec![0x61, 0xFF, 0x62])
    };
    let uscita = std::process::Command::new(comune::binario())
        .arg("describe")
        .arg("--input")
        .arg(grezzo)
        .output()
        .expect("avvio");
    let esito = esito(&uscita);
    assert_eq!(esito.codice, 2);
    assert_eq!(
        esito.documento["error"]["category"],
        "invalid_configuration"
    );
    assert_eq!(esito.documento["command"], "describe");
}
