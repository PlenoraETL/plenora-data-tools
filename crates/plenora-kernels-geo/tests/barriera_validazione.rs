//! I due reperti che `fuzz wkb_contract` ha trovato, e cio' che la barriera
//! deve garantire.
//!
//! # Perche' i byte stanno qui per intero
//!
//! Perche' sono la prova. Un caso che li ricostruisse da una descrizione
//! proverebbe che la descrizione e' sbagliata quanto il codice; ridurli a un
//! caso «equivalente» scritto a mano vorrebbe dire scegliere che cosa conta,
//! che e' la scelta che il fuzzer ha smentito.
//!
//! # Che cosa NON dicono
//!
//! Che il difetto sia dei numeri denormali. Delle quattro coordinate che i due
//! panici nominano, **tre sono normali**: 3.79e-270, 7.38e-304 e 2.18e-289
//! stanno sopra il minimo normale del `f64` (circa 2.2251e-308), e solo
//! 2.47e-312 e' subnormale. Vietare una classe di magnitudini rifiuterebbe
//! geometrie valide senza chiudere il difetto.
//!
//! # Dov'e' davvero il punto di rottura
//!
//! Entrambi i reperti sono `MultiPolygon` di tre poligoni: i primi due validi
//! da soli, il terzo no. Il panico e' un **`debug_assert!`** di `geo`
//! (`edge_end_bundle_star.rs:116`) il cui guardiano chiede se la geometria sia
//! valida — ma `propagate_side_labels` riceve **un solo** operando, e il
//! conflitto topologico nasce dalla *coppia*: relazionare il poligono 1
//! (valido) con il 2 (invalido). Il guardiano guarda quello valido, non vede
//! l'altro, e l'asserzione scatta. La conclusione ordinaria di `geo`, quando la
//! raggiunge, e' infatti `ElementsOverlaps(1, 2)`.
//!
//! Ne segue che il panico esiste **solo dove le asserzioni di debug sono
//! attive**: la batteria, e il target del fuzz. Non la produzione.
//!
//! # La verifica su stderr non e' qui
//!
//! Sta in `barriera_privacy_processo.rs`, perche' l'hook di panico e' **stato
//! globale del processo**: installarlo in questo binario legherebbe fra loro i
//! casi che lo ospitano, e osservare stderr dall'interno dello stesso processo
//! non e' osservare cio' che esce.

use geo::{Coord, LineString, Polygon};
use plenora_core::ErrorCategory;
use plenora_kernels_geo::geometry_from_wkb;

/// Il reperto della campagna schedulata del 5 settembre 2026.
const REPERTO_A: &[u8] = &[
    1, 6, 0, 0, 0, 3, 0, 0, 0, 1, 3, 0, 0, 0, 0, 0, 0, 0, 1, 3, 0, 0, 0, 1, 0, 0, 0, 7, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 12, 1, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 5, 46, 254, 255, 255, 253, 15, 0, 0, 16, 64, 64, 64, 64, 0, 0, 1, 3, 0, 0, 0,
    1, 0, 0, 0, 7, 0, 0, 44, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 212, 0, 0, 0, 4, 0, 4, 0, 0, 8, 116,
    116, 116, 116, 116, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 3, 0, 0, 0, 1,
    0, 0, 0, 7, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 3, 0, 0, 0, 0, 0, 0, 6, 0, 0, 0, 0, 0, 0, 0, 5, 46, 254,
    255, 0, 0, 1, 0, 0, 0, 7, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 212, 0, 0, 0, 0, 0, 4, 0,
    0, 8, 116, 116, 116, 116, 116, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
];
/// Il reperto della campagna schedulata del 4 settembre 2026.
const REPERTO_B: &[u8] = &[
    0, 0, 0, 0, 6, 0, 0, 0, 3, 0, 0, 0, 0, 3, 0, 0, 0, 0, 0, 0, 0, 0, 3, 0, 0, 0, 1, 0, 0, 0, 8, 1,
    1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0, 4, 1, 1, 1, 1, 0, 8, 1, 1, 1, 1,
    1, 1, 1, 1, 1, 1, 210, 210, 210, 210, 122, 210, 210, 210, 210, 210, 210, 210, 1, 1, 4, 255, 1,
    1, 1, 1, 1, 1, 1, 255, 254, 254, 254, 254, 254, 254, 250, 1, 1, 1, 1, 42, 1, 1, 1, 1, 1, 1, 0,
    0, 0, 0, 0, 0, 0, 4, 1, 1, 1, 1, 0, 8, 1, 1, 1, 64, 1, 1, 1, 1, 1, 1, 1, 65, 1, 1, 1, 1, 1, 1,
    1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 3, 0, 0, 0, 1, 0, 0, 0, 8, 1, 1, 1, 1,
    1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 128, 0, 4, 1, 1, 1, 1, 0, 8, 1, 1, 1, 1, 1, 1,
    1, 1, 129, 1, 1, 1, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 255, 254,
    254, 254, 254, 254, 254, 250, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 0, 50, 0, 0, 0, 0, 0, 4, 1, 1,
    1, 1, 0, 8, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 9, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1,
    1, 1, 1, 1, 0,
];

/// **Una validazione che non conclude non e' un piano invalido.**
///
/// Quando i reperti fanno panicare `geo`, la barriera li trasforma in errore, e
/// l'errore e' `Internal`. Non `InvalidPlan`, perche' nessuno ha **dimostrato**
/// che quell'ingresso sia invalido — il validatore si e' interrotto. Dire
/// «piano invalido» manderebbe chi legge a correggere un errore che non ha
/// commesso, ed e' la stessa distinzione che l'executor fa per il panico di un
/// kernel.
///
/// # Perche' l'attesa dipende dal profilo
///
/// Perche' il panico di `geo` **e' un `debug_assert!`**: esiste solo dove le
/// asserzioni di debug sono attive. Con `debug_assertions` spente — il profilo
/// `release`, cioe' la produzione — `geo` non panica affatto: conclude, e
/// rifiuta i reperti come `ElementsOverlaps`. Sono due comportamenti corretti
/// della stessa barriera, e il caso pretende quello giusto per il profilo in cui
/// gira invece di codificarne uno solo. Un caso che pretendesse sempre
/// `Internal` sarebbe verde in `cargo test` e rosso in `--release` senza che il
/// codice sia cambiato.
#[test]
fn una_validazione_interrotta_e_un_difetto_interno_non_un_ingresso_invalido() {
    for (nome, payload) in [("5 settembre", REPERTO_A), ("4 settembre", REPERTO_B)] {
        let errore = geometry_from_wkb(payload).expect_err(nome);
        let attesa = if cfg!(debug_assertions) {
            // `geo` panica: la barriera lo contiene e non giudica l'ingresso.
            ErrorCategory::Internal
        } else {
            // `geo` conclude: i poligoni 1 e 2 si sovrappongono davvero, e
            // giudicare l'ingresso e' corretto.
            ErrorCategory::InvalidPlan
        };
        assert_eq!(
            errore.category(),
            attesa,
            "{nome}: categoria inattesa per questo profilo — {errore}"
        );
    }
}

/// Il vocabolario **controllato** delle ragioni: cio' che il confine puo'
/// pubblicare, per intero. Il testo di `geo` non ne fa parte.
const RAGIONI_NOSTRE: [&str; 7] = [
    "coordinata non finita",
    "punti distinti insufficienti",
    "anello con auto-intersezione",
    "anelli che si intersecano",
    "anello interno fuori dal proprio esterno",
    "poligoni sovrapposti",
    "forma non valida non ulteriormente distinta",
];

/// Poligono «a farfalla»: l'anello esterno interseca se stesso. Supera la
/// decodifica strutturale — i byte sono ben formati — e cade sulla sola
/// validazione OGC, che e' il ramo che questi casi devono raggiungere.
fn farfalla() -> Vec<u8> {
    let mut wkb: Vec<u8> = vec![1];
    wkb.extend_from_slice(&3_u32.to_le_bytes());
    wkb.extend_from_slice(&1_u32.to_le_bytes());
    wkb.extend_from_slice(&5_u32.to_le_bytes());
    for (x, y) in [
        (0.0_f64, 0.0_f64),
        (10.0, 10.0),
        (0.0, 10.0),
        (10.0, 0.0),
        (0.0, 0.0),
    ] {
        wkb.extend_from_slice(&x.to_le_bytes());
        wkb.extend_from_slice(&y.to_le_bytes());
    }
    wkb
}

/// **Una geometria davvero invalida resta un ingresso invalido.**
///
/// E' la meta' che rende la distinzione una distinzione: se ogni esito fosse
/// `Internal`, il caso qui sopra passerebbe senza dire niente. Un anello che
/// si auto-interseca e' invalido secondo OGC, il validatore **conclude**, e la
/// categoria e' quella di chi ha scritto l'ingresso.
#[test]
fn una_geometria_invalida_resta_un_ingresso_invalido() {
    let errore = geometry_from_wkb(&farfalla()).expect_err("un poligono a farfalla non e' valido");
    assert_eq!(
        errore.category(),
        ErrorCategory::InvalidPlan,
        "il validatore ha concluso: e' l'ingresso a essere invalido — {errore}"
    );
}

/// **Nessuno dei due esiti pubblica il testo della dipendenza.**
///
/// Non si verifica l'assenza di qualche frammento di coordinata — un elenco di
/// frammenti prova solo che quei frammenti non ci sono. Si verifica che il
/// testo reso appartenga al **vocabolario nostro**: una ragione controllata per
/// la geometria invalida, la forma del payload per la validazione interrotta.
#[test]
fn nessun_esito_pubblica_il_testo_della_dipendenza() {
    // 1. Il reperto. Quale dei due rami lo tratti dipende dal profilo — vedi
    //    `una_validazione_interrotta_...` — ma l'obbligo e' lo stesso in
    //    entrambi: il testo reso e' nostro. Il caso pretende percio' la forma
    //    del profilo in cui gira, non una delle due a caso.
    let reperto = geometry_from_wkb(REPERTO_A)
        .expect_err("rifiutato")
        .to_string();
    if cfg!(debug_assertions) {
        assert!(
            reperto.contains("la validazione non ha potuto concludere")
                && reperto.contains("contenuto non pubblicato"),
            "forma inattesa per la validazione interrotta: {reperto}"
        );
    } else {
        assert!(
            RAGIONI_NOSTRE.iter().any(|nostra| reperto.ends_with(nostra)),
            "la ragione non appartiene al vocabolario controllato: {reperto}"
        );
    }
    // In nessuno dei due profili il testo di `geo` attraversa il confine.
    for parola in ["index", "ring", "intersection", "coordinate", "polygon"] {
        assert!(!reperto.contains(parola), "testo della dipendenza: {reperto}");
    }

    // 2. Geometria invalida: la ragione appartiene al vocabolario nostro.
    let invalida = geometry_from_wkb(&farfalla())
        .expect_err("farfalla")
        .to_string();
    assert!(
        RAGIONI_NOSTRE
            .iter()
            .any(|nostra| invalida.ends_with(nostra)),
        "la ragione non appartiene al vocabolario controllato: {invalida}"
    );
    // E il testo di `geo` non compare: nessun indice, nessuna sua parola.
    for parola in ["index", "ring", "intersection", "coordinate", "polygon"] {
        assert!(
            !invalida.contains(parola),
            "testo della dipendenza: {invalida}"
        );
    }
}

/// **Le geometrie valide con coordinate piccolissime continuano a passare.**
///
/// La barriera non e' un filtro sui numeri: non rifiuta una geometria perche'
/// le sue coordinate sono minuscole. Il caso usa un poligono ben formato con
/// ordinate **sotto** il minimo normale del `f64` — piu' estreme di tre delle
/// quattro che compaiono nei panici — e pretende che sia accettato.
#[test]
fn le_geometrie_valide_con_coordinate_minuscole_passano() {
    let minuscolo = 1e-320_f64;
    assert!(minuscolo > 0.0 && minuscolo < f64::MIN_POSITIVE);

    // WKB scritto a mano: little-endian, tipo 3 (Polygon), un anello, cinque
    // punti. A mano e non con un encoder, perche' cio' che il caso fissa sono i
    // BYTE che entrano nel confine, non il comportamento di chi li produce.
    let mut wkb: Vec<u8> = vec![1];
    wkb.extend_from_slice(&3_u32.to_le_bytes());
    wkb.extend_from_slice(&1_u32.to_le_bytes());
    wkb.extend_from_slice(&5_u32.to_le_bytes());
    for (x, y) in [
        (0.0, 0.0),
        (minuscolo, 0.0),
        (minuscolo, minuscolo),
        (0.0, minuscolo),
        (0.0, 0.0),
    ] {
        wkb.extend_from_slice(&f64::to_le_bytes(x));
        wkb.extend_from_slice(&f64::to_le_bytes(y));
    }

    let geometria = geometry_from_wkb(&wkb).expect("un poligono valido resta valido");
    assert!(matches!(geometria, geo::Geometry::Polygon(_)));
}
