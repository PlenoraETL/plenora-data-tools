//! I casi del selettore: che l'identita' e la funzione dicano la stessa cosa.

use plenora_core::crs::CrsError;

use super::{risolvi, Risolutore, VERSIONE};

/// **I due `cfg` dicono la stessa cosa.**
///
/// L'identita' viene dal selettore, la funzione dalla scelta del simbolo: chi
/// ne modificasse una sola avrebbe un programma che risolve i CRS in un modo e
/// li descrive in un altro. Si usa una definizione **valida**, perche' una
/// malformata misurerebbe la validazione invece del backend.
#[test]
fn l_identita_dichiarata_e_la_funzione_concordano() {
    let scelto = Risolutore::di_questa_build();
    let esito = risolvi("EPSG:4326", "crs");

    match scelto {
        Risolutore::SenzaBackend => {
            assert_eq!(scelto.identita(), "senza-backend");
            assert!(
                matches!(esito, Err(CrsError::BackendUnavailable)),
                "il selettore dice «senza backend», e la funzione ne ha uno: {esito:?}"
            );
        }
        Risolutore::Proj => {
            assert_eq!(scelto.identita(), "proj");
            assert!(
                !matches!(esito, Err(CrsError::BackendUnavailable)),
                "il selettore dice «proj», e la funzione non ha un backend"
            );
        }
    }
}

/// Le due identita' sono **distinte**, e restano stabili.
///
/// Attraversano il filo e due lati le confrontano per uguaglianza: due
/// implementazioni che si chiamassero uguale si accorderebbero fra loro.
#[test]
fn le_identita_sono_distinte_e_non_vuote() {
    let nomi = [
        Risolutore::SenzaBackend.identita(),
        Risolutore::Proj.identita(),
    ];
    assert_ne!(nomi[0], nomi[1]);
    for nome in nomi {
        assert!(!nome.is_empty(), "un'identita' vuota non identifica niente");
        assert!(
            !nome.contains(char::is_whitespace),
            "«{nome}»: un'identita' che attraversa il filo non porta spazi"
        );
    }
}

/// **Solo la build senza backend sa inventariare il proprio ambiente.**
///
/// Con PROJ non esiste una radice esclusiva, immutabile e inventariabile.
#[test]
fn l_ambiente_e_inventariabile_solo_senza_backend() {
    assert!(Risolutore::SenzaBackend.ambiente_inventariabile());
    assert!(
        !Risolutore::Proj.ambiente_inventariabile(),
        "dichiarare inventariabile l'ambiente PROJ sarebbe una falsa garanzia"
    );
}

/// La versione c'e', e non e' un segnaposto.
#[test]
fn la_versione_del_componente_e_dichiarata() {
    assert!(!VERSIONE.is_empty());
    assert!(
        VERSIONE.contains('.'),
        "«{VERSIONE}» non somiglia a una versione"
    );
}
