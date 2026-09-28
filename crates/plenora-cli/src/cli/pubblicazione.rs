//! L'esito della pubblicazione nei documenti di uscita.

use std::path::Path;

use plenora_engine::geo_transport::publish::{
    EsitoDellaPubblicazione, PublishOutcome, PuliziaDelTemporaneo, RagioneNonAccertabile,
};
use serde_json::Value;

/// Esito tipizzato del publish (errori-e-limiti.md#publish-e-cleanup) in forma verificabile, **senza
/// scrivere su stderr**.
///
/// Il chiamante lo riporta nel proprio documento di uscita: stderr resta
/// vuoto per contratto (errori-e-limiti.md#envelope-e-canali). Con il profilo
/// `Atomic` l'esito e' sempre `Published`.
pub const fn durabilita_confermata(outcome: PublishOutcome) -> bool {
    !matches!(outcome, PublishOutcome::PublishedButDurabilityUnconfirmed)
}

/// L'esito di una pubblicazione, nei campi del documento di uscita.
///
/// Due campi, perche' durabilita' della destinazione e pulizia del
/// temporaneo sono fatti distinti. `temp_cleanup` e' sempre un oggetto, con
/// lo stato in `state`, cosi' chi lo consuma non deve prima scoprirne il
/// tipo.
pub fn campi_della_pubblicazione(esito: &EsitoDellaPubblicazione) -> Vec<(String, Value)> {
    let pulizia = match &esito.pulizia {
        PuliziaDelTemporaneo::Rimosso => serde_json::json!({ "state": "removed" }),
        PuliziaDelTemporaneo::Presente { percorso, byte } => {
            let mut oggetto = serde_json::Map::new();
            oggetto.insert("state".to_owned(), Value::String("present".to_owned()));
            aggiungi_il_percorso(&mut oggetto, percorso);
            oggetto.insert("bytes".to_owned(), Value::from(*byte));
            Value::Object(oggetto)
        }
        PuliziaDelTemporaneo::NonAccertabile { percorso, ragione } => {
            let mut oggetto = serde_json::Map::new();
            oggetto.insert(
                "state".to_owned(),
                Value::String("unascertainable".to_owned()),
            );
            aggiungi_il_percorso(&mut oggetto, percorso);
            oggetto.insert(
                "reason".to_owned(),
                Value::String(
                    match ragione {
                        RagioneNonAccertabile::PermessoNegato => "permission_denied",
                        RagioneNonAccertabile::GuastoDiLettura => "read_failure",
                    }
                    .to_owned(),
                ),
            );
            Value::Object(oggetto)
        }
    };
    vec![
        (
            "durability_confirmed".to_owned(),
            Value::Bool(durabilita_confermata(esito.durabilita)),
        ),
        ("temp_cleanup".to_owned(), pulizia),
    ]
}

/// Scrive il percorso del residuo in una forma **ricostruibile**.
///
/// Non `Path::display()`, che e' lossy e indicherebbe un file che non esiste;
/// non `OsStr::as_encoded_bytes`, forma interna non specificata. Si usano le
/// codifiche native, e `path_encoding` c'e' sempre:
///
/// | `path_encoding` | campo | come si ricostruisce |
/// |---|---|---|
/// | `utf8` | `path`, stringa | e' gia' il percorso |
/// | `unix_bytes` | `path_units`, interi 0-255 | `OsStringExt::from_vec` |
/// | `windows_utf16` | `path_units`, interi 0-65535 | `OsStringExt::from_wide` |
///
/// `path` resta quando il percorso e' testo valido, il caso ordinario.
fn aggiungi_il_percorso(oggetto: &mut serde_json::Map<String, Value>, percorso: &Path) {
    if let Some(testo) = percorso.to_str() {
        oggetto.insert("path_encoding".to_owned(), Value::String("utf8".to_owned()));
        oggetto.insert("path".to_owned(), Value::String(testo.to_owned()));
        return;
    }
    let (codifica, unita) = unita_native(percorso);
    oggetto.insert(
        "path_encoding".to_owned(),
        Value::String(codifica.to_owned()),
    );
    oggetto.insert("path_units".to_owned(), Value::Array(unita));
}

/// Il percorso nelle unita' che il sistema operativo usa davvero.
#[cfg(unix)]
fn unita_native(percorso: &Path) -> (&'static str, Vec<Value>) {
    use std::os::unix::ffi::OsStrExt as _;
    (
        "unix_bytes",
        percorso
            .as_os_str()
            .as_bytes()
            .iter()
            .map(|byte| Value::from(*byte))
            .collect(),
    )
}

/// Il percorso nelle unita' che il sistema operativo usa davvero.
#[cfg(windows)]
fn unita_native(percorso: &Path) -> (&'static str, Vec<Value>) {
    use std::os::windows::ffi::OsStrExt as _;
    (
        "windows_utf16",
        percorso
            .as_os_str()
            .encode_wide()
            .map(Value::from)
            .collect(),
    )
}

/// Su una piattaforma che non e' ne' Unix ne' Windows non si inventa una
/// codifica: si dichiara di non saperla dire, che e' l'unica cosa vera.
#[cfg(not(any(unix, windows)))]
fn unita_native(_percorso: &Path) -> (&'static str, Vec<Value>) {
    ("unsupported", Vec::new())
}

/// L'esito della pubblicazione, come frammento di un documento scritto a mano.
///
/// Per i comandi che compongono il JSON con `format!`: rende i soli campi
/// nuovi, gia' con l'escape giusto, senza virgola in testa ne' in coda, cosi'
/// ordine e formattazione dei documenti esistenti non cambiano.
pub fn frammento_della_pubblicazione(esito: &EsitoDellaPubblicazione) -> String {
    campi_della_pubblicazione(esito)
        .into_iter()
        .map(|(chiave, valore)| format!("\"{chiave}\":{valore}"))
        .collect::<Vec<_>>()
        .join(",")
}

#[cfg(test)]
mod uscita_della_pubblicazione {
    use super::{campi_della_pubblicazione, EsitoDellaPubblicazione, PuliziaDelTemporaneo};
    use plenora_engine::geo_transport::publish::{PublishOutcome, RagioneNonAccertabile};
    use std::path::PathBuf;

    fn documento(esito: &EsitoDellaPubblicazione) -> serde_json::Value {
        serde_json::Value::Object(campi_della_pubblicazione(esito).into_iter().collect())
    }

    /// **`temp_cleanup` e' un oggetto in tutti e tre gli stati.**
    ///
    /// Chi consuma il documento non deve prima scoprire di che tipo sia il
    /// valore: una stringa per un caso e un oggetto per gli altri costringe a
    /// due rami prima ancora di leggere lo stato.
    #[test]
    fn la_pulizia_ha_sempre_la_stessa_forma() {
        for pulizia in [
            PuliziaDelTemporaneo::Rimosso,
            PuliziaDelTemporaneo::Presente {
                percorso: PathBuf::from("/tmp/x.partial"),
                byte: 12,
            },
            PuliziaDelTemporaneo::NonAccertabile {
                percorso: PathBuf::from("/tmp/x.partial"),
                ragione: RagioneNonAccertabile::PermessoNegato,
            },
        ] {
            let esito = EsitoDellaPubblicazione {
                durabilita: PublishOutcome::Published,
                pulizia,
            };
            let campo = documento(&esito)["temp_cleanup"].clone();
            assert!(
                campo.is_object(),
                "ogni stato e' un oggetto, non solo due su tre: {campo}"
            );
            assert!(
                campo["state"].is_string(),
                "e lo stato sta sempre nella stessa chiave: {campo}"
            );
        }
    }

    /// **Il percorso del residuo si ricostruisce dal documento.**
    ///
    /// Non si confronta il campo con lo stesso encoder che l'ha prodotto — quello
    /// direbbe soltanto che una funzione e' uguale a se' stessa. Si fa cio' che
    /// farebbe un consumatore: si legge `path_encoding`, si prendono le unita' e
    /// si chiama la funzione **standard** della piattaforma, poi si pretende che
    /// il percorso ottenuto sia quello di partenza.
    #[test]
    fn un_percorso_non_testuale_si_ricostruisce_dal_json() {
        let percorso = percorso_non_testuale();
        let esito = EsitoDellaPubblicazione {
            durabilita: PublishOutcome::Published,
            pulizia: PuliziaDelTemporaneo::Presente {
                percorso: percorso.clone(),
                byte: 3,
            },
        };
        let campo = documento(&esito)["temp_cleanup"].clone();

        let codifica = campo["path_encoding"]
            .as_str()
            .expect("la codifica e' sempre dichiarata")
            .to_owned();
        assert_eq!(
            codifica,
            codifica_attesa(),
            "la codifica dichiarata e' quella nativa della piattaforma"
        );
        assert!(
            campo.get("path").is_none(),
            "un percorso non testuale non ha il campo leggibile: {campo}"
        );

        let unita: Vec<u64> = campo["path_units"]
            .as_array()
            .expect("le unita' ci sono")
            .iter()
            .map(|valore| valore.as_u64().expect("unita' intera"))
            .collect();
        assert_eq!(
            ricostruisci(&unita),
            percorso,
            "il consumatore ricostruisce il percorso esatto"
        );
    }

    /// **Un percorso testuale esce leggibile, e si dichiara `utf8`.**
    #[test]
    fn un_percorso_testuale_resta_leggibile() {
        let percorso = PathBuf::from("/tmp/uscita.arrow.partial");
        let esito = EsitoDellaPubblicazione {
            durabilita: PublishOutcome::Published,
            pulizia: PuliziaDelTemporaneo::Presente {
                percorso: percorso.clone(),
                byte: 7,
            },
        };
        let campo = documento(&esito)["temp_cleanup"].clone();

        assert_eq!(campo["path_encoding"], "utf8");
        assert_eq!(campo["path"], "/tmp/uscita.arrow.partial");
        assert!(
            campo.get("path_units").is_none(),
            "e non porta anche le unita': una forma sola per volta"
        );
        assert_eq!(
            PathBuf::from(campo["path"].as_str().expect("testo")),
            percorso
        );
    }

    // --- cio' che cambia fra le piattaforme, e nient'altro -------------------

    /// Un percorso che il sistema accetta e che **non e'** UTF-8 valido.
    #[cfg(unix)]
    fn percorso_non_testuale() -> PathBuf {
        use std::os::unix::ffi::OsStrExt as _;
        // `0xff 0xfe` non e' una sequenza UTF-8 valida, ed e' un nome di file
        // perfettamente legale su Unix.
        PathBuf::from(std::ffi::OsStr::from_bytes(b"/tmp/\xff\xfe.partial"))
    }

    /// Su Windows: un surrogato spaiato, che UTF-16 ammette e UTF-8 no.
    #[cfg(windows)]
    fn percorso_non_testuale() -> PathBuf {
        use std::os::windows::ffi::OsStringExt as _;
        let unita: Vec<u16> = "C:\\temp\\x"
            .encode_utf16()
            .chain(std::iter::once(0xd800_u16))
            .chain(".partial".encode_utf16())
            .collect();
        PathBuf::from(std::ffi::OsString::from_wide(&unita))
    }

    #[cfg(unix)]
    fn codifica_attesa() -> &'static str {
        "unix_bytes"
    }

    #[cfg(windows)]
    fn codifica_attesa() -> &'static str {
        "windows_utf16"
    }

    /// Rilegge le unita' **come farebbe un consumatore**, con la funzione
    /// standard della piattaforma.
    #[cfg(unix)]
    fn ricostruisci(unita: &[u64]) -> PathBuf {
        use std::os::unix::ffi::OsStringExt as _;
        let byte: Vec<u8> = unita
            .iter()
            .map(|unita| u8::try_from(*unita).expect("un ottetto sta in un byte"))
            .collect();
        PathBuf::from(std::ffi::OsString::from_vec(byte))
    }

    #[cfg(windows)]
    fn ricostruisci(unita: &[u64]) -> PathBuf {
        use std::os::windows::ffi::OsStringExt as _;
        let parole: Vec<u16> = unita
            .iter()
            .map(|unita| u16::try_from(*unita).expect("un'unita' UTF-16 sta in un u16"))
            .collect();
        PathBuf::from(std::ffi::OsString::from_wide(&parole))
    }

    /// `installa_gestore_segnale_isolato` non fa nascere un thread e accetta
    /// registrazioni multiple, quindi si puo' installare in un test senza
    /// interferire con il processo; si accerta la registrazione stessa.
    #[test]
    #[cfg(target_os = "linux")]
    fn installa_gestore_segnale_isolato_riesce_su_un_token_nuovo() {
        let token = plenora_engine::CancellationToken::new();
        let esito = crate::installa_gestore_segnale_isolato(&token);
        assert!(
            esito.is_ok(),
            "la registrazione presso il kernel deve riuscire in un ambiente Linux ordinario: {esito:?}"
        );
        assert!(
            !token.is_cancelled(),
            "installare il gestore non deve cancellare il token da solo"
        );
    }
}
