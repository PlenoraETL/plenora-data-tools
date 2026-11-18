// Snapshot di sola prova: v6.0.0, blob 181b507e942da466ce0f6e62be09a20ac4bee245.
// Prefisso del file pubblicato, fino al modulo tests; nessuna logica modificata.
//! Predicati spaziali OGC/DE-9IM fra due geometrie: i kernel di
//! `geo.predicate_*`.
//!
//! Ogni predicato legge la matrice d'intersezione DE-9IM che `relate` di
//! `geo` 0.33.1 calcola sulle geometrie `f64` cosi' come sono, con predicati
//! d'orientazione esatti: nessuna tolleranza, nessuna griglia. Il confine
//! segue la regola mod-2 dell'OGC (l'estremo condiviso da un numero pari di
//! linee e' interno; una linea chiusa non ha confine). Le collezioni non si
//! uniscono prima del confronto: il lato condiviso da due membri poligonali
//! adiacenti resta confine, e membri poligonali che si sovrappongono
//! possono far fallire `relate` (`CalcoloNonConcluso`).

use crate::ValidazioneProtetta as _;
use geo::{CoordsIter, Geometry, Relate};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Il predicato da valutare fra `left` (A) e `right` (B).
///
/// Le maschere sono quelle DE-9IM, righe e colonne nell'ordine interno,
/// confine, esterno;
/// `T` vuol dire non vuoto, `F` vuoto, `*` qualunque, `0`/`1` la dimensione.
/// Il nome serde e' quello in `snake_case`.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SpatialPredicate {
    /// A e B hanno almeno un punto in comune: una fra `T********`,
    /// `*T*******`, `***T*****`, `****T****`. Negazione di `Disjoint`.
    Intersects,
    /// A e B non hanno punti in comune: `FF*FF****`. Vero se una delle due
    /// e' vuota.
    Disjoint,
    /// Nessun punto di B e' fuori da A e gli interni si toccano:
    /// `T*****FF*`. Un B tutto sul confine di A non e' contenuto.
    Contains,
    /// Nessun punto di A e' fuori da B e gli interni si toccano:
    /// `T*F**F***` (`Contains` a parti scambiate).
    Within,
    /// A e B sono lo stesso insieme di punti: `T*F**FFF*`, qualunque sia la
    /// rappresentazione (vertici in piu' sui lati, punto iniziale, verso).
    /// Due geometrie vuote sono uguali.
    EqualsTopo,
    /// Nessun punto di B e' fuori da A, e B non e' vuota: una fra
    /// `T*****FF*`, `*T****FF*`, `***T**FF*`, `****T*FF*`. A differenza di
    /// `Contains`, un B tutto sul confine di A e' coperto.
    Covers,
    /// Nessun punto di A e' fuori da B, e A non e' vuota: una fra
    /// `T*F**F***`, `*TF**F***`, `**FT*F***`, `**F*TF***` (`Covers` a
    /// parti scambiate).
    CoveredBy,
    /// B e' tutta nell'interno di A, senza toccarne il confine: `T**FF*FF*`.
    /// Una geometria non contiene propriamente se stessa.
    ContainsProperly,
    /// A e B si toccano solo sui confini: gli interni sono disgiunti e c'e'
    /// almeno un punto in comune (una fra `FT*******`, `F**T*****`,
    /// `F***T****`). Falso fra due punti.
    Touches,
    /// Gli interni si intersecano e: con dim(A) < dim(B) parte dell'interno
    /// di A e' fuori da B (`T*T******`); con dim(A) > dim(B) parte
    /// dell'interno di B e' fuori da A (`T*****T**`); fra due linee gli
    /// interni si toccano solo in punti (`0********`). Falso fra due
    /// poligoni e fra due punti.
    Crosses,
    /// A e B hanno la stessa dimensione, gli interni si intersecano e
    /// ciascuna ha punti interni fuori dall'altra: fra due linee
    /// `1*T***T**` (sovrapposte per un tratto); fra punti o fra poligoni
    /// `T*T***T**`. Falso fra dimensioni diverse.
    Overlaps,
}

/// Errori di [`evaluate`] e [`evaluate_validated`]. `side` e' `"left"` o
/// `"right"`; nessun messaggio riporta coordinate.
#[derive(Debug, Error)]
pub enum PredicateError {
    /// La geometria `side` ha una coordinata NaN o infinita.
    #[error("geometria {side} contiene coordinate NaN o infinite")]
    NonFiniteCoordinate { side: &'static str },
    /// La geometria `side` non supera la validazione OGC; `reason` e' la
    /// ragione classificata, senza coordinate.
    #[error("geometria {side} non valida: {reason}")]
    InvalidGeometry { side: &'static str, reason: String },
    /// La validazione OGC non ha concluso: `geo` si e' interrotta.
    ///
    /// **Non** e' una geometria invalida. Nessuno ha dimostrato che l'ingresso
    /// sia sbagliato, e accusarlo manderebbe chi legge a correggere un errore
    /// che non ha commesso. Porta la *forma* del payload, mai il contenuto.
    #[error("validazione OGC non conclusa: {0} (contenuto non pubblicato)")]
    ValidazioneNonConclusa(&'static str),
    /// `relate` non ha concluso su geometrie **valide**
    /// (`crate::calcolo_protetto`): non accusa l'ingresso, e porta la
    /// *forma* del payload, mai il contenuto.
    #[error("predicato non concluso: {0} (contenuto non pubblicato)")]
    CalcoloNonConcluso(&'static str),
}

/// Mappa l'esito della barriera sull'errore proprio di questo modulo,
/// nominando il lato. Estratta a parte (non solo inline in [`validate`])
/// perche' e' la conversione che una prova sintetica deve esercitare per
/// intero: chiamarla con un [`crate::EsitoValidazione::NonConclusa`] fatto a
/// mano prova che QUESTA mappatura — non una sua reimplementazione — non
/// appiattisce la distinzione, senza bisogno di un ingresso che faccia
/// davvero panicare `geo`.
fn classifica_lato(esito: crate::EsitoValidazione, side: &'static str) -> PredicateError {
    esito.separa(
        |ragione| PredicateError::InvalidGeometry {
            side,
            reason: ragione.to_string(),
        },
        PredicateError::ValidazioneNonConclusa,
    )
}

fn validate(geometry: &Geometry<f64>, side: &'static str) -> Result<(), PredicateError> {
    if geometry
        .coords_iter()
        .any(|coordinate| !coordinate.x.is_finite() || !coordinate.y.is_finite())
    {
        return Err(PredicateError::NonFiniteCoordinate { side });
    }
    geometry
        .validazione_protetta()
        .map_err(|esito| classifica_lato(esito, side))
}

/// Valuta il predicato OGC/DE-9IM `predicate` fra `left` (A) e `right` (B),
/// dopo aver validato entrambe (prima `left`, poi `right`).
///
/// # Errors
///
/// - `PredicateError::NonFiniteCoordinate`: `left` o `right` contiene
///   coordinate NaN o infinite;
/// - `PredicateError::InvalidGeometry`: `left` o `right` non supera la
///   validazione OGC (es. anello auto-intersecato);
/// - `PredicateError::ValidazioneNonConclusa`: la validazione OGC non
///   conclude;
/// - `PredicateError::CalcoloNonConcluso`: `relate` si interrompe su due
///   geometrie valide (per esempio una `GeometryCollection` con membri
///   poligonali sovrapposti).
pub fn evaluate(
    left: &Geometry<f64>,
    right: &Geometry<f64>,
    predicate: SpatialPredicate,
) -> Result<bool, PredicateError> {
    validate(left, "left")?;
    validate(right, "right")?;
    evaluate_unchecked(left, right, predicate)
}

/// Variante di [`evaluate`] SENZA il gate di ingresso (scansione di
/// finitezza + validazione OGC su entrambe le geometrie).
///
/// # Precondizione (contratto del chiamante)
///
/// Entrambe le geometrie devono essere GIA' validate (coordinate finite,
/// validita' OGC), come da [`crate::geometry_from_wkb`] o da un kernel che
/// valida il proprio output. Altrimenti il risultato e' indefinito. Solo per
/// percorsi validati per costruzione; il gate resta in [`evaluate`].
///
/// # Errors
///
/// `PredicateError::CalcoloNonConcluso` quando `relate` si interrompe: la
/// validazione non basta a escluderlo (`crate::calcolo_protetto`).
pub fn evaluate_validated(
    left: &Geometry<f64>,
    right: &Geometry<f64>,
    predicate: SpatialPredicate,
) -> Result<bool, PredicateError> {
    evaluate_unchecked(left, right, predicate)
}

fn evaluate_unchecked(
    left: &Geometry<f64>,
    right: &Geometry<f64>,
    predicate: SpatialPredicate,
) -> Result<bool, PredicateError> {
    let matrix = crate::calcolo_protetto(|| left.relate(right))
        .map_err(PredicateError::CalcoloNonConcluso)?;
    Ok(match predicate {
        SpatialPredicate::Intersects => matrix.is_intersects(),
        SpatialPredicate::Disjoint => matrix.is_disjoint(),
        SpatialPredicate::Contains => matrix.is_contains(),
        SpatialPredicate::Within => matrix.is_within(),
        SpatialPredicate::EqualsTopo => matrix.is_equal_topo(),
        SpatialPredicate::Covers => matrix.is_covers(),
        SpatialPredicate::CoveredBy => matrix.is_coveredby(),
        SpatialPredicate::ContainsProperly => matrix.is_contains_properly(),
        SpatialPredicate::Touches => matrix.is_touches(),
        SpatialPredicate::Crosses => matrix.is_crosses(),
        SpatialPredicate::Overlaps => matrix.is_overlaps(),
    })
}
