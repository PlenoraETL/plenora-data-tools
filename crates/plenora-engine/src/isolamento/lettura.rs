//! Leggere un file di `/proc` o di `cgroup2` senza fidarsi della sua taglia.
//!
//! # Perche' non `fs::read_to_string`
//!
//! Non perche' non funzioni: la funzione comoda usa i metadati solo per
//! **dimensionare il buffer iniziale**, e un file che dichiara lunghezza zero
//! — come fanno quelli di `/proc` e di `cgroup2` — viene letto lo stesso, con
//! qualche riallocazione in piu'.
//!
//! La ragione e' un'altra, ed e' che quella funzione legge **fino alla fine**.
//! Su questi file la fine e' decisa da chi li produce, e alcuni sono
//! virtualmente illimitati: `/proc/self/mountinfo` cresce con i mount,
//! `/proc/self/status` con i campi che il kernel aggiunge. Nessuno di essi
//! dovrebbe essere grande, ma «non dovrebbe» non e' un tetto, ed e' la stessa
//! forma di fiducia che questo progetto rifiuta altrove: una lunghezza che
//! decide qualcun altro.
//!
//! Qui il tetto e' nostro, ed e' applicato **durante** la lettura e non dopo:
//! `take` non consegna piu' byte di quanti se ne ammettano, quindi un file che
//! cresce senza fine non fa crescere il buffer con se'.

use super::DifettoSuperficie;

/// Quanto si ammette da un file di `/proc` o di `cgroup2`.
///
/// Un megabyte e' due ordini di grandezza sopra il piu' grande di quelli che
/// leggiamo — `/proc/self/mountinfo` su una macchina affollata sta in qualche
/// decina di kilobyte — e resta abbastanza piccolo da non essere
/// un'allocazione di cui preoccuparsi.
pub(super) const MASSIMO_BYTE: usize = 1024 * 1024;

/// Legge un file per intero, ma non oltre [`MASSIMO_BYTE`].
///
/// # Errors
///
/// [`DifettoSuperficie::Lettura`] se il file non si apre, non si legge, non e'
/// UTF-8, oppure **e' piu' grande del tetto**: un contenuto troncato non e' il
/// contenuto, e interpretarlo significherebbe decidere su meta' file.
pub(super) fn leggi_limitato(
    percorso: &std::path::Path,
) -> std::result::Result<String, DifettoSuperficie> {
    use std::io::Read as _;

    let nome = percorso.display().to_string();
    let file = std::fs::File::open(percorso)
        .map_err(|causa| DifettoSuperficie::lettura(nome.clone(), causa))?;
    let mut byte = Vec::new();
    // Un byte oltre il tetto: e' cosi' che si distingue «esattamente al
    // tetto», che e' legittimo, da «oltre», che e' un troncamento.
    file.take(MASSIMO_BYTE as u64 + 1)
        .read_to_end(&mut byte)
        .map_err(|causa| DifettoSuperficie::lettura(nome.clone(), causa))?;
    if byte.len() > MASSIMO_BYTE {
        return Err(DifettoSuperficie::Forma(format!(
            "{nome}: oltre {MASSIMO_BYTE} byte, e un contenuto troncato non e' il contenuto"
        )));
    }
    String::from_utf8(byte).map_err(|causa| DifettoSuperficie::Forma(format!("{nome}: {causa}")))
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;

    use super::{leggi_limitato, MASSIMO_BYTE};

    /// Un file esattamente al tetto si legge; uno di un byte piu' grande no.
    ///
    /// Le due meta' insieme: senza la prima il tetto potrebbe essere `0` e il
    /// caso passerebbe lo stesso, senza la seconda potrebbe non esserci.
    #[test]
    fn il_tetto_e_esatto_e_si_applica_durante_la_lettura() {
        let cartella = tempfile::tempdir().expect("tempdir");

        let al_tetto = cartella.path().join("al-tetto");
        std::fs::File::create(&al_tetto)
            .expect("create")
            .write_all(&vec![b'a'; MASSIMO_BYTE])
            .expect("write");
        assert_eq!(
            leggi_limitato(&al_tetto).expect("al tetto si legge").len(),
            MASSIMO_BYTE
        );

        let oltre = cartella.path().join("oltre");
        std::fs::File::create(&oltre)
            .expect("create")
            .write_all(&vec![b'a'; MASSIMO_BYTE + 1])
            .expect("write");
        let difetto = leggi_limitato(&oltre).expect_err("oltre il tetto si rifiuta");
        assert!(
            difetto.to_string().contains("troncato"),
            "il motivo deve dire perche' non si interpreta: {difetto}"
        );
    }

    /// Un file che non e' UTF-8 e' un rifiuto, non una stringa approssimata.
    #[test]
    fn un_contenuto_non_utf8_si_rifiuta() {
        let cartella = tempfile::tempdir().expect("tempdir");
        let percorso = cartella.path().join("binario");
        std::fs::write(&percorso, [0xff, 0xfe]).expect("write");
        assert!(leggi_limitato(&percorso).is_err());
    }

    /// Un file che non esiste e' un difetto di lettura, con il percorso dentro.
    #[test]
    fn un_file_assente_nomina_il_percorso() {
        let difetto = leggi_limitato(std::path::Path::new("/proc/self/non-esiste"))
            .expect_err("il file non c'e'");
        assert!(difetto.to_string().contains("non-esiste"), "{difetto}");
    }
}
