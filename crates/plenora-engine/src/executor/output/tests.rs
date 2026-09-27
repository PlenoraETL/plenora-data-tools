//! I casi del ciclo di scrittura: l'aritmetica dei conteggi, e i rifiuti
//! dell'apertura esclusiva.

use plenora_core::error::{ErrorCategory, ErrorPhase};

use crate::protocollo::messaggi::ConteggiDichiarati;

use super::{avanza, non_apribile};

/// Il punto di partenza di un conteggio.
const fn da(righe: u64, batch: u64) -> ConteggiDichiarati {
    ConteggiDichiarati { righe, batch }
}

/// Un batch in piu' aggiunge le sue righe e se stesso.
#[test]
fn i_conteggi_avanzano_di_un_batch_e_delle_sue_righe() {
    let dopo = avanza(da(10, 2), 7).expect("dieci piu' sette stanno in un u64");
    assert_eq!(dopo, da(17, 3));
}

/// **La somma delle righe che trabocca si rifiuta: non avvolge e non satura.**
///
/// Con `wrapping_add` i conteggi di un artefatto con 2^64 righe di troppo
/// combacerebbero con quelli osservati al passo 8; con `saturating_add`
/// sarebbero `u64::MAX` per due artefatti diversi. L'unica risposta vera e'
/// un errore.
#[test]
fn la_somma_delle_righe_che_trabocca_e_un_errore() {
    let esito = avanza(da(u64::MAX - 3, 1), 10);
    let Err(errore) = esito else {
        panic!("una somma che non sta in un u64 non ha un risultato: {esito:?}");
    };
    assert_eq!(errore.category(), ErrorCategory::Internal);

    // Al limite esatto passa: il rifiuto e' del traboccamento, non della
    // vicinanza al limite.
    assert_eq!(
        avanza(da(u64::MAX - 3, 1), 3).expect("esattamente al limite ci sta"),
        da(u64::MAX, 2)
    );
}

/// **Anche il numero di batch che trabocca si rifiuta.**
///
/// Il caso e' separato da quello delle righe perche' i due contatori avanzano
/// in modo diverso — uno di `num_rows()`, l'altro sempre di uno — e un
/// `checked_add` dimenticato su un solo dei due non lo si vede dall'altro.
#[test]
fn il_numero_di_batch_che_trabocca_e_un_errore() {
    let esito = avanza(da(0, u64::MAX), 1);
    assert!(
        esito.is_err(),
        "un batch oltre u64::MAX non ha un numero: {esito:?}"
    );
}

/// Un batch con piu' righe di quante ne dica un `u64` non e' rappresentabile.
///
/// Su una piattaforma a 64 bit `usize` e `u64` coincidono e il caso non ha un
/// ingresso che lo provochi: la conversione resta perche' la larghezza di
/// `usize` e' una proprieta' della piattaforma, non una garanzia del tipo.
#[test]
fn le_righe_di_un_batch_passano_per_una_conversione_controllata() {
    assert_eq!(
        avanza(da(0, 0), usize::MAX).is_err(),
        usize::BITS > u64::BITS,
        "il rifiuto deve dipendere dalla piattaforma, non dal caso"
    );
}

/// **I generi dell'incarico sono questi cinque, con queste fasi.**
///
/// L'elenco e' riscritto qui invece di essere letto dalla tabella: un caso
/// che legge la tabella per giudicarla resta verde qualunque riga porti. Il
/// confronto e' nei due versi, quindi una riga aggiunta solo alla tabella fa
/// cadere il caso. Si guarda la categoria, non il testo di `io::Error`, che
/// dipende da piattaforma e lingua.
#[test]
fn i_generi_dell_incarico_sono_questi_cinque() {
    use std::io::{Error, ErrorKind};

    let dove = std::path::Path::new("/non/importa/artefatto.arrow");

    let attesi = [
        (ErrorKind::AlreadyExists, ErrorPhase::Commit),
        (ErrorKind::NotFound, ErrorPhase::Probe),
        (ErrorKind::InvalidInput, ErrorPhase::Probe),
        (ErrorKind::NotADirectory, ErrorPhase::Probe),
        (ErrorKind::IsADirectory, ErrorPhase::Commit),
    ];

    // Nei due versi: la tabella non ha righe in meno...
    for (genere, fase) in attesi {
        let classificato = non_apribile(dove, &Error::from(genere));
        assert_eq!(
            classificato.category(),
            ErrorCategory::InvalidPlan,
            "{genere:?} e' un difetto dell'incarico, non dell'ambiente"
        );
        assert_eq!(
            classificato.phase(),
            fase,
            "{genere:?} deve portare la fase dichiarata qui"
        );
    }
    // ...e non ne ha in piu'.
    let nella_tabella: Vec<_> = super::GENERI_DELL_INCARICO
        .iter()
        .map(|(genere, fase, _)| (*genere, *fase))
        .collect();
    assert_eq!(
        nella_tabella,
        attesi.to_vec(),
        "la tabella dei generi dell'incarico non e' piu' quella dichiarata qui"
    );
}

/// **Cio' che non e' nella tabella resta dell'ambiente.**
///
/// # Che cosa esclude
///
/// Che la tabella cresca fino a inghiottire tutto. Se ogni genere diventasse
/// `InvalidPlan`, il caso qui sopra resterebbe verde e chi legge un guasto di
/// permessi andrebbe a correggere un incarico che e' giusto.
#[test]
fn cio_che_non_e_nella_tabella_resta_dell_ambiente() {
    use std::io::{Error, ErrorKind};

    let dove = std::path::Path::new("/non/importa/artefatto.arrow");

    for genere in [
        ErrorKind::PermissionDenied,
        ErrorKind::StorageFull,
        ErrorKind::ReadOnlyFilesystem,
    ] {
        let ambiente = non_apribile(dove, &Error::from(genere));
        assert_eq!(
            ambiente.category(),
            ErrorCategory::Io,
            "{genere:?} e' l'ambiente che risponde di no, non un incarico da correggere"
        );
        assert_eq!(ambiente.phase(), ErrorPhase::Write);
    }
}

/// Il rifiuto vero di un'apertura esclusiva su quel percorso.
///
/// Gli stessi flag del percorso di scrittura — `create_new`, cioe'
/// `O_CREAT|O_EXCL` — perche' un'apertura con flag diversi risponderebbe a una
/// domanda diversa.
fn rifiuto_vero(percorso: &std::path::Path) -> std::io::Error {
    std::fs::File::options()
        .write(true)
        .create_new(true)
        .open(percorso)
        .expect_err("un percorso di questa forma non si apre")
}

/// **Due forme di percorso arrivano come difetto dell'incarico, su ogni
/// piattaforma.**
///
/// Esclude righe della tabella che nessuna apertura reale produce. Si
/// pretende anche il genere, non solo la categoria: un componente intermedio
/// che e' un file da' `NotADirectory` su Unix e `NotFound` su Windows, e con la
/// sola categoria la divergenza resterebbe invisibile.
#[test]
fn due_forme_di_percorso_sono_difetti_dell_incarico_ovunque() {
    let stanza = tempfile::tempdir().expect("una directory temporanea");
    let un_file = stanza.path().join("questo-e-un-file");
    std::fs::write(&un_file, b"x").expect("si scrive il file");

    // Un componente intermedio che e' un file, non una directory.
    #[cfg(unix)]
    let genere_sotto_un_file = std::io::ErrorKind::NotADirectory;
    #[cfg(windows)]
    let genere_sotto_un_file = std::io::ErrorKind::NotFound;

    // Un percorso che il sistema non accetta: un byte NUL non puo' stare in un
    // nome, e nessun tetto sulla lunghezza lo esclude.
    let forme = [
        (un_file.join("artefatto.arrow"), genere_sotto_un_file),
        (
            stanza.path().join("artefatto\0.arrow"),
            std::io::ErrorKind::InvalidInput,
        ),
    ];

    for (percorso, genere_atteso) in forme {
        let causa = rifiuto_vero(&percorso);
        assert_eq!(
            causa.kind(),
            genere_atteso,
            "il sistema ha cambiato risposta su «{}»: serve una decisione nuova, \
             non un caso allargato",
            percorso.display()
        );
        let classificato = non_apribile(&percorso, &causa);
        assert_eq!(classificato.category(), ErrorCategory::InvalidPlan);
        assert_eq!(classificato.phase(), ErrorPhase::Probe);
    }
}

/// **Una directory esistente e' ambigua, e le due piattaforme non concordano.**
///
/// Su Unix arriva `AlreadyExists` (incarico, fase `Commit`), su Windows
/// `PermissionDenied`, indistinguibile da un vero difetto di permessi e quindi
/// `Io`, fase `Write`: la diagnosi conservativa, registrata in
/// errori-e-limiti.md#lapertura-dellartefatto-temporaneo-quali-generi-sono-dellincarico.
/// Se una piattaforma cambiasse risposta, il caso diventerebbe rosso.
#[test]
fn una_directory_esistente_e_ambigua_fra_le_piattaforme() {
    let stanza = tempfile::tempdir().expect("una directory temporanea");
    let causa = rifiuto_vero(stanza.path());

    #[cfg(unix)]
    let (genere_atteso, categoria_attesa, fase_attesa) = (
        std::io::ErrorKind::AlreadyExists,
        ErrorCategory::InvalidPlan,
        ErrorPhase::Commit,
    );
    #[cfg(windows)]
    let (genere_atteso, categoria_attesa, fase_attesa) = (
        std::io::ErrorKind::PermissionDenied,
        ErrorCategory::Io,
        ErrorPhase::Write,
    );

    assert_eq!(
        causa.kind(),
        genere_atteso,
        "questa piattaforma ha cambiato risposta su una directory esistente: \
         la classificazione va ridecisa, non adattata"
    );
    let classificato = non_apribile(stanza.path(), &causa);
    assert_eq!(classificato.category(), categoria_attesa);
    assert_eq!(classificato.phase(), fase_attesa);
}
