#![no_main]

use geo::algorithm::validation::Validation;
use geozero::{ToGeo, wkb::Wkb};
use libfuzzer_sys::fuzz_target;
use plenora_kernels_geo::{geometry_from_wkb, transform_wkb, validate_wkb_contract, Operation};

#[path = "comune/aggancio.rs"]
mod aggancio;

/// Il giudizio del riferimento su una geometria: accetta, rifiuta, o **non
/// conclude**.
///
/// # Che cosa giudica
///
/// Il contratto del decoder: validita' OGC di `geo` **e** nessun anello con una
/// punta, che `geo` 0.33.1 non cerca (salta le coppie di segmenti adiacenti) e
/// il decoder rifiuta.
///
/// # Perche' non la barriera di produzione
///
/// Perche' il riferimento deve restare **indipendente** da cio' che giudica:
/// se usasse `ValidazioneProtetta`, l'oracolo misurerebbe la stessa difesa che
/// e' incaricato di sorvegliare, e un difetto di quella difesa passerebbe
/// inosservato da entrambe le parti. Chiama percio' `geo` direttamente, e cerca
/// le punte con un'altra formulazione ([`ha_una_punta`]).
///
/// La chiamata a `geo` passa da una `barriera_di_dipendenza`: il panico di
/// `relate` e' atteso, e l'hook del target (`comune/aggancio.rs`) lo tollera.
/// Rende `None` quando la validazione non conclude: un riferimento che non
/// conclude non ha giudicato niente, e trattarlo come «invalida» direbbe che il
/// decoder diverge quando invece nessuno ha deciso.
fn riferimento_giudica(geometry: &geo::Geometry<f64>) -> Option<bool> {
    if ha_una_punta(geometry) {
        return Some(false);
    }
    plenora_core::panic_policy::barriera_di_dipendenza(std::panic::AssertUnwindSafe(|| {
        geometry.check_validation().is_ok()
    }))
    .ok()
}

/// Una punta, formulata come **sovrapposizione**: due segmenti consecutivi di
/// un anello che `line_intersection` dice collineari su un tratto di lunghezza
/// non nulla. Il decoder la cerca invece con orientamento e verso: due strade,
/// perche' un difetto dell'una non si nasconda nell'altra.
fn ha_una_punta(geometry: &geo::Geometry<f64>) -> bool {
    use geo::{Coord, Line, LineIntersection};

    fn anello(anello: &geo::LineString<f64>) -> bool {
        let mut vertici: Vec<Coord<f64>> = Vec::new();
        for &vertice in &anello.0 {
            if !vertice.x.is_finite() || !vertice.y.is_finite() {
                return false;
            }
            if vertici.last() != Some(&vertice) {
                vertici.push(vertice);
            }
        }
        while vertici.len() > 1 && vertici.first() == vertici.last() {
            vertici.pop();
        }
        let n = vertici.len();
        if n < 3 {
            return false;
        }
        (0..n).any(|i| {
            let entrante = Line::new(vertici[(i + n - 1) % n], vertici[i]);
            let uscente = Line::new(vertici[i], vertici[(i + 1) % n]);
            matches!(
                geo::line_intersection::line_intersection(entrante, uscente),
                Some(LineIntersection::Collinear { intersection }) if intersection.start != intersection.end
            )
        })
    }
    fn poligono(p: &geo::Polygon<f64>) -> bool {
        std::iter::once(p.exterior()).chain(p.interiors()).any(anello)
    }
    match geometry {
        geo::Geometry::Polygon(p) => poligono(p),
        geo::Geometry::MultiPolygon(m) => m.iter().any(poligono),
        geo::Geometry::GeometryCollection(c) => c.iter().any(ha_una_punta),
        _ => false,
    }
}

fuzz_target!(init: aggancio::installa(), |payload: &[u8]| {
    // Oracolo differenziale architettura.md#geometrie: il decoder validante (via
    // `geometry_from_wkb`) e il percorso precedente (validatore strutturale
    // + geozero) devono accettare/rifiutare gli stessi payload e produrre
    // la stessa geometria, coordinata per coordinata.
    //
    // I due percorsi non hanno pero' lo stesso contratto: `geometry_from_wkb`
    // applica anche la validazione OGC, che geozero non fa. Senza il filtro
    // qui sotto l'oracolo segnala come divergenza ogni geometria
    // strutturalmente ben formata ma non valida — per esempio
    // `LINESTRING(0 0, 0 0, 0 0, 0 0)`, che geozero decodifica e il decoder
    // rifiuta correttamente con "line string must have at least 2 distinct
    // points". Un target rosso per questo motivo non puo' segnalare
    // divergenze vere.
    // Il decoder si esercita **sempre**: che non panichi e' un'invariante di
    // questo target, indipendente dal fatto che il riferimento sappia giudicare.
    let decoded = geometry_from_wkb(payload).ok();

    let grezzo = validate_wkb_contract(payload)
        .ok()
        .and_then(|()| Wkb(payload).to_geo().ok());
    // Tre esiti del riferimento, non due: accetta, rifiuta, oppure **non
    // conclude**. Il terzo sospende il confronto invece di inventarne uno.
    let confronto = match grezzo {
        None => Some(None),
        Some(geometry) => match riferimento_giudica(&geometry) {
            Some(true) => Some(Some(geometry)),
            Some(false) => Some(None),
            None => None,
        },
    };
    if let Some(reference) = confronto {
        match (&reference, &decoded) {
            (Some(expected), Some(actual)) => assert_eq!(expected, actual),
            (None, None) => {}
            (Some(_), None) => panic!("divergenza decoder: riferimento Ok, decoder Err"),
            (None, Some(_)) => panic!("divergenza decoder: riferimento Err, decoder Ok"),
        }
    }
    if decoded.is_some() {
        for operation in Operation::ALL {
            if let Ok(output) = transform_wkb(operation, payload) {
                assert!(geometry_from_wkb(&output).is_ok());
            }
        }
    }
});
