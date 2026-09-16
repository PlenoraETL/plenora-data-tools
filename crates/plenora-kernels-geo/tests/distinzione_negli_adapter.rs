//! La distinzione sopravvive anche **a valle**, negli adapter di colonna.
//!
//! # Perche' non bastano i casi sui kernel
//!
//! Perche' un adapter non rende l'errore del kernel: lo traduce in una
//! diagnostica di riga. Un esito interrotto contato fra le celle invalide
//! diventa `DataMapping`, e chi legge va a correggere una riga che nessuno ha
//! dimostrato sbagliata. La distinzione nasce nel kernel e puo' morire qui.

use plenora_core::arrow::array::StringArray;
use plenora_core::ErrorCategory;
use plenora_kernels_geo::extensions::{from_wkt_column, OnWktError};

const REPERTO: &[u8] = &[
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

/// La coppia di poligoni del reperto che provoca il conflitto, in WKT.
///
/// Non il reperto intero: contiene anche un poligono **vuoto**, e l'encoder
/// WKT panica su quello prima ancora di arrivare alla validazione. Si prendono
/// percio' i poligoni 1 e 2 — il valido e l'invalido che si sovrappongono —
/// che sono esattamente la coppia da cui nasce il difetto.
fn coppia_del_reperto_in_wkt() -> String {
    use geo::{Geometry, MultiPolygon};
    use geozero::{wkb::Wkb, ToGeo};
    use wkt::ToWkt;
    let Geometry::MultiPolygon(mp) = Wkb(REPERTO).to_geo().expect("decodifica grezza") else {
        panic!("il reperto e' un MultiPolygon");
    };
    MultiPolygon::new(vec![mp.0[1].clone(), mp.0[2].clone()])
        .to_wkt()
        .to_string()
}

/// **Col candidato esatto, questa coppia e' una cella invalida in ogni
/// profilo — mai piu' una validazione interrotta.**
///
/// Senza il diff 1 (`vendor/geo-0.33.1-exact`), con le asserzioni di debug
/// attive la cella fa panicare `geo` e l'adapter rende
/// `Internal`, non `DataMapping`. Il segno corretto di `orient2d` toglie la
/// causa dell'asserzione: `geo` conclude sempre — misurato qui in entrambi i
/// profili, non dedotto. La distinzione fra i due esiti (provata, senza il
/// diff 1, con un reperto reale per entrambi) non e' cambiata nell'adapter,
/// solo il verdetto su QUESTA coppia: resta la meta' "conclusa".
#[test]
fn l_adapter_wkt_su_reperto_concluso_resta_datamapping() {
    let wkt = coppia_del_reperto_in_wkt();
    let colonna = StringArray::from(vec![Some(wkt.as_str())]);
    let errore = from_wkt_column(&colonna, OnWktError::Fail).expect_err("il reperto non passa");

    assert_eq!(
        errore.category(),
        ErrorCategory::DataMapping,
        "col candidato esatto `geo` conclude sempre — categoria inattesa: {errore}"
    );
    // La cella e' contata fra le invalide, con il suo codice. Il codice sta
    // nella diagnostica di riga, non nel messaggio, ed e' li' che il caso lo
    // cerca.
    let plenora_core::PlenoraError::RowDiagnostics { diagnostics, .. } = &errore else {
        panic!("attesa la diagnostica di riga, trovato {errore:?}");
    };
    assert_eq!(
        diagnostics.counts.get("geometry.invalid_wkt"),
        Some(&1),
        "attesa una cella invalida contata: {:?}",
        diagnostics.counts
    );
}

/// **Una cella WKT ordinariamente invalida resta una cella invalida.**
///
/// La meta' che rende la distinzione una distinzione: se ogni esito diventasse
/// `Internal`, il caso qui sopra passerebbe senza dire niente. Il testo qui
/// non e' WKT affatto, quindi il verdetto e' sulla cella in ogni profilo.
#[test]
fn una_cella_ordinariamente_invalida_resta_tale() {
    let colonna = StringArray::from(vec![Some("NON E' WKT")]);
    let errore =
        from_wkt_column(&colonna, OnWktError::Fail).expect_err("una cella non-WKT non passa");
    assert_eq!(
        errore.category(),
        ErrorCategory::DataMapping,
        "una cella davvero invalida resta un difetto dei dati — {errore}"
    );
}

/// **Le celle valide continuano a passare.**
#[test]
fn le_celle_valide_passano() {
    let colonna = StringArray::from(vec![Some("POINT (1 2)"), None]);
    let uscita = from_wkt_column(&colonna, OnWktError::Fail).expect("celle valide");
    assert_eq!(uscita.len(), 2);
    assert!(uscita[0].is_some() && uscita[1].is_none());
}
