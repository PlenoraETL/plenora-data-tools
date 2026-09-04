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
/// # Che cosa uccide
///
/// Le due scorciatoie, e le uccide separatamente perche' falliscono in modi
/// diversi. Con `wrapping_add` il risultato sarebbe `6`: un artefatto con 2^64
/// righe di troppo avrebbe conteggi che **combaciano** con quelli osservati da
/// chi verifica, e il passo 8 direbbe che va tutto bene. Con `saturating_add`
/// sarebbe `u64::MAX`, cioe' lo stesso numero per due artefatti diversi — una
/// perdita silenziosa, che e' la peggiore.
///
/// L'unica risposta che resta vera in entrambi i casi e' non rispondere: un
/// errore.
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
/// # Perche' la distinzione conta
///
/// Perche' dice a chi legge dove intervenire. Questi generi parlano di cio' che
/// l'incarico ha chiesto — un percorso, una directory, la forma del percorso — e
/// nessun permesso li risolve; tutto il resto e' l'ambiente che risponde di no,
/// e correggere l'incarico non servirebbe a niente.
///
/// # Perche' l'elenco e' riscritto qui, e non letto dalla tabella
///
/// Perche' un caso che legge la tabella per giudicare la tabella non giudica
/// niente: qualunque riga la tabella porti, il caso la trova coerente con se
/// stessa e resta verde. E' il verso in cui un controllo si svuota senza che
/// nessuno se ne accorga.
///
/// L'elenco qui e' quindi un'**affermazione indipendente**: dice quali generi
/// devono essere dell'incarico e con quale fase, e il confronto e' nei due
/// versi — niente di meno, niente di piu'. Una riga aggiunta alla tabella e non
/// qui fa cadere il caso, ed e' cio' che si vuole: chi la aggiunge deve dire
/// anche perche'.
///
/// Il caso guarda la **categoria**, non il messaggio: il testo di `io::Error`
/// dipende dalla piattaforma e dalla lingua del sistema.
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

/// **I generi della forma del percorso si raggiungono davvero.**
///
/// # Che cosa esclude
///
/// Che la tabella classifichi generi che nessuna apertura puo' produrre. Una
/// riga che non si raggiunge non e' una difesa: e' una dichiarazione, e il caso
/// che la guarda con un `ErrorKind` costruito a mano resterebbe verde qualunque
/// cosa faccia il sistema vero.
///
/// Qui l'apertura e' quella vera, con gli stessi flag del percorso di scrittura
/// — `create_new`, cioe' `O_CREAT|O_EXCL` — e si guarda che l'errore finisca fra
/// i difetti dell'incarico. **Quale** genere arrivi lo decide il kernel e non lo
/// si pretende: si pretende la classificazione, che e' la decisione nostra.
#[test]
fn i_percorsi_malformati_arrivano_classificati_come_incarico() {
    let stanza = tempfile::tempdir().expect("una directory temporanea");
    let un_file = stanza.path().join("questo-e-un-file");
    std::fs::write(&un_file, b"x").expect("si scrive il file");

    // Un componente intermedio che e' un file, non una directory.
    let sotto_un_file = un_file.join("artefatto.arrow");
    // Il percorso nomina una directory dove serve un file.
    let una_directory = stanza.path().to_path_buf();
    // Un percorso che il sistema non accetta: un byte NUL non puo' stare in un
    // nome, e nessun tetto sulla lunghezza lo esclude.
    let con_nul = stanza.path().join("artefatto\0.arrow");

    for percorso in [sotto_un_file, una_directory, con_nul] {
        let causa = std::fs::File::options()
            .write(true)
            .create_new(true)
            .open(&percorso)
            .expect_err("un percorso di questa forma non si apre");
        let classificato = non_apribile(&percorso, &causa);
        assert_eq!(
            classificato.category(),
            ErrorCategory::InvalidPlan,
            "«{}» e' un difetto dell'incarico: il sistema ha detto {:?}",
            percorso.display(),
            causa.kind()
        );
    }
}
