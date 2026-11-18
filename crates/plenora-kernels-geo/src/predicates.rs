//! Predicati spaziali OGC/DE-9IM fra due geometrie: i kernel di
//! `geo.predicate_*`.
//!
//! La matrice usa un arrangiamento razionale esatto degli ingressi `f64`,
//! senza tolleranze o griglie. I predicati su `De9imMatrix` leggono quelle
//! celle. `evaluate` ed `evaluate_validated` conservano il backend 6.0.0.
//! Le regole per le collezioni miste sono dichiarate in [`relate_matrix`].

use crate::ValidazioneProtetta as _;
use geo::{CoordsIter, Geometry, Relate};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[cfg(test)]
#[deny(clippy::disallowed_methods)]
mod arrangement;
#[cfg(test)]
#[deny(clippy::disallowed_methods)]
mod arrangement_exhaustive;
pub(crate) mod arrangement_indexed;
mod interval_index;

/// Limiti della matrice sul noder comune (API 6.1.0 non rilasciata).
/// Lavoro e nodi sono cumulativi; contenitori e aritmetica hanno ricevute vive.
#[derive(Clone, Copy, Debug)]
pub struct RelateLimits {
    /// Test esatti dopo entrambi i filtri inclusivi X/Y, anche ripetuti
    /// nella validazione degli anelli. Le sole visite bbox consumano passi.
    pub max_segment_pairs: u64,
    /// Passi di visita delle celle e classificazione dei segmenti.
    pub max_steps: u64,
    /// Byte richiesti per le capacita' dei vettori strutturali ancora vivi;
    /// pop/dedup non rilasciano capacita', il drop la rilascia (anche dopo
    /// conversione in iteratore). Durante una crescita si prenota anche
    /// la vecchia capacita', per la coesistenza dei due buffer di realloc.
    /// Ogni intero aritmetico, inclusi temporanei
    /// e cloni, addebita bit arrotondati ai limb nativi piu' header, subito
    /// dopo la creazione; il drop rilascia la ricevuta. La capacita' privata
    /// e gli scratch di `num-bigint` non sono misurabili con questa API:
    /// questo conteggio non certifica l'heap totale (vedi docs/topologia.md).
    /// Lo sforamento contabile prima del rifiuto e' al massimo un intero
    /// di 145173 bit arrotondati ai limb piu' l'header `BigInt`.
    ///
    /// Il test d'integrazione `heap_reale` certifica separatamente il corpus
    /// reticolo/pettine/sparso/estremi al confine di `GlobalAlloc`: misura
    /// il picco di byte richiesti vivi, compresi gli scratch e i due buffer
    /// durante realloc. L'ingresso preso in prestito precede la misura
    /// (esclusione incrementale zero); l'esclusione del tracker e' il picco
    /// della somma viva `wrapped_size - object_size`, cioe' header e padding
    /// effettivamente osservati, senza tolleranze empiriche. Il limite al
    /// rifiuto usa [`Self::MAX_VALUE_BYTES`]. Metadati privati di `System`,
    /// pagine OS e RSS non sono osservabili da `GlobalAlloc`: il test non
    /// certifica questi costi, ne' prova tutti i possibili ingressi `f64`.
    pub max_reserved_bytes: u64,
    /// Punti d'ingresso costruiti (copie incluse) e incidenze degli eventi:
    /// due per incrocio, quattro per overlap non puntuale. Ripetizioni
    /// durante la validazione incluse; controllo prima della costruzione.
    pub max_nodes: u64,
    /// Scadenza monotona, verificata insieme all'annullamento.
    pub deadline: Option<std::time::Instant>,
}

impl RelateLimits {
    /// Massimo addebito di un singolo intero dopo la sua costruzione.
    /// Deriva dal limite algebrico di `noding/bounds.rs`; non include gli
    /// scratch privati del backend ne' l'overhead dell'allocatore.
    pub const MAX_VALUE_BYTES: u64 = crate::noding::bounds::MAX_VALUE_BYTES;
}

impl Default for RelateLimits {
    fn default() -> Self {
        Self {
            max_segment_pairs: 10_000_000,
            max_steps: 100_000_000,
            max_reserved_bytes: 268_435_456,
            max_nodes: 10_000_000,
            deadline: None,
        }
    }
}

/// Calcola la matrice con limiti espliciti e annullamento cooperativo.
///
/// Il flag viene letto con ordinamento `Relaxed`: non pubblica altri dati.
/// Nessun prodotto del numero dei segmenti decide l'ammissione. Il noder
/// emette ascisse critiche senza materializzare il grafo; lavoro, nodi e
/// scadenza includono la validazione. Le prenotazioni seguono la durata
/// dei vettori temporanei. Il flag ha precedenza sulla scadenza.
///
/// # Errors
/// Gli errori di [`relate_matrix`], piu' limite di lavoro/prenotazioni,
/// allocazione fallita, annullamento e [`PredicateError::DeadlineExceeded`].
pub fn relate_matrix_with_limits(
    left: &Geometry<f64>,
    right: &Geometry<f64>,
    limits: RelateLimits,
    cancelled: Option<&std::sync::atomic::AtomicBool>,
) -> Result<De9imMatrix, PredicateError> {
    let mut work = arrangement_indexed::Work::new(limits, cancelled);
    work.step()?;
    arrangement_indexed::relate(left, right, &mut work)
}

/// Pattern DE-9IM OGC: esattamente nove byte ASCII in `TF*012`.
///
/// Il parsing controlla tutta la grammatica prima di confrontare una cella.
/// Non ammette spazi, minuscole o espressioni di linguaggi del consumatore.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct De9imPattern([u8; 9]);

/// Pattern malformato; il testo ricevuto non viene pubblicato.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
#[error("pattern DE-9IM non valido: richiesti nove caratteri ASCII TF*012")]
pub struct InvalidDe9imPattern;

impl std::str::FromStr for De9imPattern {
    type Err = InvalidDe9imPattern;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let bytes: [u8; 9] = value
            .as_bytes()
            .try_into()
            .map_err(|_| InvalidDe9imPattern)?;
        if bytes
            .iter()
            .all(|b| matches!(b, b'T' | b'F' | b'*' | b'0' | b'1' | b'2'))
        {
            Ok(Self(bytes))
        } else {
            Err(InvalidDe9imPattern)
        }
    }
}

/// Matrice dimensionale nell'ordine `II IB IE BI BB BE EI EB EE`.
///
/// Ogni cella e' `F`, `0`, `1` o `2`. Il costruttore resta privato: una
/// stringa arbitraria non e' attestata come risultato geometrico.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct De9imMatrix(geo::relate::IntersectionMatrix);

impl De9imMatrix {
    /// Le nove celle in ordine OGC SFA 1.2.1, sezione 6.1.15.2.
    #[must_use]
    pub fn cells(&self) -> [u8; 9] {
        use geo::{coordinate_position::CoordPos, dimensions::Dimensions};
        let positions = [CoordPos::Inside, CoordPos::OnBoundary, CoordPos::Outside];
        std::array::from_fn(
            |index| match self.0.get(positions[index / 3], positions[index % 3]) {
                Dimensions::Empty => b'F',
                Dimensions::ZeroDimensional => b'0',
                Dimensions::OneDimensional => b'1',
                Dimensions::TwoDimensional => b'2',
            },
        )
    }

    /// Confronto senza allocazioni con un pattern gia' interamente validato.
    #[must_use]
    pub fn matches(&self, pattern: De9imPattern) -> bool {
        self.cells()
            .into_iter()
            .zip(pattern.0)
            .all(|(cell, expected)| {
                expected == b'*' || expected == cell || (expected == b'T' && cell != b'F')
            })
    }

    /// Valuta sulla matrice uno dei predicati esistenti, senza ricalcolare
    /// le intersezioni. Conserva anche la convenzione sui due vuoti.
    #[must_use]
    pub fn evaluate(&self, predicate: SpatialPredicate) -> bool {
        evaluate_matrix(&self.0, predicate)
    }
}

impl std::fmt::Display for De9imMatrix {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for cell in self.cells() {
            write!(formatter, "{}", char::from(cell))?;
        }
        Ok(())
    }
}

/// Calcola la matrice completa sull'unione dei membri, con aritmetica
/// razionale esatta (anche sugli incroci non rappresentabili in `f64`).
///
/// OGC SFA 1.2.1, 6.1.15.2. Per le collezioni miste (6.1.3.1) si adotta
/// questa regola dimensionale: prima si classifica l'unione delle aree;
/// fuori dalle aree si classificano le linee, poi i punti residui. Un punto
/// o una linea aggiunti sul bordo di un'area non ne cambiano il confine.
/// I lati interni all'unione delle aree sono interni, anche nei vertici
/// con piu' membri incidenti. Sovrapposizioni fra membri di una collezione
/// sono ammesse; quelle fra membri di un `MultiPolygon` sono invalide.
///
/// Il confine lineare e' Mod-2 degli estremi dei membri (6.1.8.1), dopo
/// aver appiattito le collezioni annidate. La parita' dipende dai membri:
/// due linee sovrapposte [0,2] e [1,3] hanno confine anche in 1 e 2;
/// non sono una riscrittura del confine della singola linea [0,3].
///
/// Coordinate XY planari: il chiamante assicura lo stesso CRS e ordine
/// degli assi per i due lati.
///
/// Default: 10 milioni di coppie effettive, 100 milioni di passi,
/// 10 milioni di nodi, 256 MiB di prenotazioni strutturali vive, nessuna
/// deadline ([`RelateLimits`]).
/// Nessuna stima quadratica rifiuta l'ingresso prima del lavoro.
///
/// # Errors
///
/// Operandi non finiti o invalidi, limiti superati e prenotazione fallita;
/// nessun messaggio contiene coordinate. La validazione include anelli
/// semplici, contenimento dei buchi e connessione dell'interno poligonale.
pub fn relate_matrix(
    left: &Geometry<f64>,
    right: &Geometry<f64>,
) -> Result<De9imMatrix, PredicateError> {
    relate_matrix_with_limits(left, right, RelateLimits::default(), None)
}

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
    /// La pubblicazione binary64 altererebbe la topologia esatta.
    #[error("topologia non conservata dalla pubblicazione binary64")]
    InvalidRoundedTopology,
    /// Il calcolo ha consumato il numero consentito di coppie o passi.
    #[error("limite di lavoro DE-9IM superato")]
    WorkLimit,
    /// La prossima prenotazione supererebbe il limite dei byte vivi.
    #[error("limite di prenotazione DE-9IM superato")]
    ReservationLimit,
    /// Una prenotazione fallibile non e' riuscita.
    #[error("prenotazione DE-9IM fallita")]
    AllocationFailed,
    /// Il chiamante ha annullato il calcolo.
    #[error("calcolo DE-9IM annullato")]
    Cancelled,
    /// La scadenza monotona e' stata raggiunta.
    #[error("scadenza del calcolo DE-9IM raggiunta")]
    DeadlineExceeded,
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
/// - `PredicateError::CalcoloNonConcluso`: la costruzione della matrice
///   non conclude.
///
/// Compatibilita' 6.0.0: conserva `geo::Relate` e il gate OGC preesistente.
/// Per la matrice razionale nuova usare [`relate_matrix`] e poi
/// [`De9imMatrix::evaluate`]. I due percorsi possono differire su collezioni
/// unite e intersezioni non rappresentabili esattamente in `f64`.
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
/// `PredicateError::CalcoloNonConcluso` quando `geo::Relate` si interrompe.
/// Backend, risultati e default sono quelli pubblicati nella 6.0.0.
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
    Ok(evaluate_matrix(&matrix, predicate))
}

fn evaluate_matrix(matrix: &geo::relate::IntersectionMatrix, predicate: SpatialPredicate) -> bool {
    match predicate {
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
    }
}

// Il corpus 1a qualifica i predicati della nuova matrice, non le API 6.0.0.
#[cfg(test)]
#[deny(clippy::disallowed_methods)]
fn exact_predicate(
    left: &Geometry<f64>,
    right: &Geometry<f64>,
    predicate: SpatialPredicate,
) -> Result<bool, PredicateError> {
    relate_matrix(left, right).map(|matrix| matrix.evaluate(predicate))
}

#[cfg(test)]
#[deny(clippy::disallowed_methods)]
fn exact_predicate_validated(
    left: &Geometry<f64>,
    right: &Geometry<f64>,
    predicate: SpatialPredicate,
) -> Result<bool, PredicateError> {
    let mut work = arrangement_indexed::Work::new(RelateLimits::default(), None);
    arrangement_indexed::relate_validated(left, right, &mut work)
        .map(|matrix| matrix.evaluate(predicate))
}

#[cfg(test)]
#[deny(clippy::disallowed_methods)]
mod baseline_600;

#[deny(clippy::disallowed_methods)]
#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{bowtie, rect};
    use geo::{line_string, Point};

    /// O-M4/D-a: confronto input per input con il sorgente pubblicato
    /// v6.0.0 (snapshot `baseline_600`). Risultati ed errori, non solo firme.
    #[test]
    fn published_predicates_match_600_on_the_analytic_corpus() {
        use wkt::TryFromWkt as _;
        let predicates = [
            SpatialPredicate::Intersects,
            SpatialPredicate::Disjoint,
            SpatialPredicate::Contains,
            SpatialPredicate::Within,
            SpatialPredicate::EqualsTopo,
            SpatialPredicate::Covers,
            SpatialPredicate::CoveredBy,
            SpatialPredicate::ContainsProperly,
            SpatialPredicate::Touches,
            SpatialPredicate::Crosses,
            SpatialPredicate::Overlaps,
        ];
        let mut pairs: Vec<_> = ANALYTIC_MATRICES
            .iter()
            .map(|(a, b, _)| {
                (
                    Geometry::try_from_wkt_str(a).expect("fixture"),
                    Geometry::try_from_wkt_str(b).expect("fixture"),
                )
            })
            .collect();
        for scale in [f64::from_bits(1), f64::MAX] {
            pairs.push((
                Geometry::Line(geo::Line::new((0., 0.), (scale, scale))),
                Geometry::Line(geo::Line::new((0., scale), (scale, -scale))),
            ));
        }
        for (a, b) in pairs {
            for predicate in predicates {
                let old = serde_json::from_value(serde_json::to_value(predicate).expect("nome"))
                    .expect("nome pubblicato");
                assert_eq!(
                    evaluate(&a, &b, predicate).map_err(|e| e.to_string()),
                    baseline_600::evaluate(&a, &b, old).map_err(|e| e.to_string()),
                );
                let old = serde_json::from_value(serde_json::to_value(predicate).expect("nome"))
                    .expect("nome pubblicato");
                assert_eq!(
                    evaluate_validated(&a, &b, predicate).map_err(|e| e.to_string()),
                    baseline_600::evaluate_validated(&a, &b, old).map_err(|e| e.to_string()),
                );
            }
        }
    }

    /// OGC SFA 1.2.1 (06-103r4), 6.1.15.2: partizione I/B/E e
    /// dimensione delle nove intersezioni. Coordinate intere, attese
    /// analitiche: siti, intervalli sugli assi e quadrati di lato due.
    /// Confine curve e Mod-2: 6.1.6.1 e 6.1.8.1; poligoni: 6.1.11.1.
    /// Nessuna attesa e' generata dal backend sotto prova.
    const ANALYTIC_MATRICES: &[(&str, &str, &str)] = &[
        // SFA 1.2.1, 6.1.3.1 e 6.1.11.1: unione di rettangoli
        // razionali; il lato interno condiviso scompare dal confine.
        (
            "GEOMETRYCOLLECTION(POLYGON((0 0,2 0,2 2,0 2,0 0)),POLYGON((2 0,4 0,4 2,2 2,2 0)))",
            "POINT(2 1)", "0F2FF1FF2",
        ),
        // SFA 6.1.3.1, 6.1.15.2: sovrapposizione e insieme unito.
        (
            "GEOMETRYCOLLECTION(POLYGON((0 0,2 0,2 2,0 2,0 0)),POLYGON((1 0,3 0,3 2,1 2,1 0)))",
            "POLYGON((0 0,3 0,3 2,0 2,0 0))", "2FFF1FFF2",
        ),
        // Stessa costruzione: linea interna con estremi sul confine.
        (
            "GEOMETRYCOLLECTION(POLYGON((0 0,2 0,2 2,0 2,0 0)),POLYGON((1 0,3 0,3 2,1 2,1 0)))",
            "LINESTRING(1 0,1 2)", "1F2F01FF2",
        ),
        // SFA 6.1.11.1: due settori soltanto, il contatto resta confine.
        (
            "GEOMETRYCOLLECTION(POLYGON((0 0,1 0,1 1,0 1,0 0)),POLYGON((1 1,2 1,2 2,1 2,1 1)))",
            "POINT(1 1)", "FF20F1FF2",
        ),
        // SFA 6.1.11.1: quattro settori coprono un intorno completo.
        (
            "GEOMETRYCOLLECTION(POLYGON((0 0,1 0,1 1,0 1,0 0)),POLYGON((0 0,0 1,-1 1,-1 0,0 0)),POLYGON((0 0,-1 0,-1 -1,0 -1,0 0)),POLYGON((0 0,0 -1,1 -1,1 0,0 0)))",
            "POINT(0 0)", "0F2FF1FF2",
        ),
        // SFA 6.1.11.1: riempimento esatto del buco [1,3]^2.
        (
            "GEOMETRYCOLLECTION(POLYGON((0 0,4 0,4 4,0 4,0 0),(1 1,1 3,3 3,3 1,1 1)),POLYGON((1 1,3 1,3 3,1 3,1 1)))",
            "POINT(1 2)", "0F2FF1FF2",
        ),
        // SFA 6.1.8.1: parita' degli estremi, anche su tratti sovrapposti.
        (
            "GEOMETRYCOLLECTION(LINESTRING(0 0,2 0),LINESTRING(1 0,3 0))",
            "POINT(1 0)", "FF10F0FF2",
        ),
        // SFA 6.1.3.1: convenzione dimensionale dichiarata; un punto
        // aggiunto sul confine poligonale non ne cambia la classificazione.
        (
            "GEOMETRYCOLLECTION(POINT(0 1),POLYGON((0 0,2 0,2 2,0 2,0 0)))",
            "POINT(0 1)", "FF20F1FF2",
        ),
        // SFA 6.1.4.1, 6.1.15.2: punto coincidente, confini vuoti.
        ("POINT(0 0)", "POINT(0 0)", "0FFFFFFF2"),
        // SFA 6.1.4.1, 6.1.15.2: due singleton disgiunti.
        ("POINT(0 0)", "POINT(1 0)", "FF0FFF0F2"),
        // SFA 6.1.5: due insiemi finiti con un punto in comune.
        (
            "MULTIPOINT((0 0),(1 0))",
            "MULTIPOINT((1 0),(2 0))",
            "0F0FFF0F2",
        ),
        // SFA 6.1.14: punto interno a uno dei quadrati disgiunti.
        (
            "POINT(1 1)",
            "MULTIPOLYGON(((0 0,2 0,2 2,0 2,0 0)),((4 0,6 0,6 2,4 2,4 0)))",
            "0FFFFF212",
        ),
        // SFA 6.1.3.1: punto interno alla componente lineare residua.
        (
            "POINT(1 0)",
            "GEOMETRYCOLLECTION(POINT(5 5),LINESTRING(0 0,2 0))",
            "0FFFFF102",
        ),
        // SFA 6.1.6.1: punto interno all'intervallo aperto (0,2).
        ("POINT(1 0)", "LINESTRING(0 0,2 0)", "0FFFFF102"),
        // SFA 6.1.6.1: estremo del medesimo intervallo.
        ("POINT(0 0)", "LINESTRING(0 0,2 0)", "F0FFFF102"),
        // SFA 6.1.11.1: punto interno al quadrato [0,2]^2.
        ("POINT(1 1)", "POLYGON((0 0,2 0,2 2,0 2,0 0))", "0FFFFF212"),
        // SFA 6.1.11.1: punto sul lato del quadrato.
        ("POINT(0 1)", "POLYGON((0 0,2 0,2 2,0 2,0 0))", "F0FFFF212"),
        // SFA 6.1.11.1: punto esterno al quadrato.
        ("POINT(3 1)", "POLYGON((0 0,2 0,2 2,0 2,0 0))", "FF0FFF212"),
        // SFA 6.1.15.2: solo l'intersezione dei due esterni e' non vuota.
        (
            "GEOMETRYCOLLECTION EMPTY",
            "GEOMETRYCOLLECTION EMPTY",
            "FFFFFFFF2",
        ),
        // SFA 6.1.15.2: tutta la linea giace nell'esterno del vuoto.
        ("MULTIPOINT EMPTY", "LINESTRING(0 0,2 0)", "FFFFFF102"),
        // SFA 6.1.15.3, Crosses: assi con incrocio interno nell'origine.
        ("LINESTRING(-1 0,1 0)", "LINESTRING(0 -1,0 1)", "0F1FF0102"),
        // SFA 6.1.15.3, Touches: solo gli estremi in 1 coincidono.
        ("LINESTRING(0 0,1 0)", "LINESTRING(1 0,2 0)", "FF1F00102"),
        // SFA 6.1.15.3, Overlaps: intersezione [1,2], residui non vuoti.
        ("LINESTRING(0 0,2 0)", "LINESTRING(1 0,3 0)", "1010F0102"),
        // SFA 6.1.6.1, 6.1.15.2: stesso interno e stessi due estremi.
        ("LINESTRING(0 0,2 0)", "LINESTRING(0 0,2 0)", "1FFF0FFF2"),
        // SFA 6.1.6.1: linea chiusa, confine vuoto.
        (
            "LINESTRING(0 0,2 0,1 1,0 0)",
            "LINESTRING(0 0,2 0,1 1,0 0)",
            "1FFFFFFF2",
        ),
        // SFA 6.1.6.1: anche il vertice iniziale e' interno se chiusa.
        ("POINT(0 0)", "LINESTRING(0 0,2 0,1 1,0 0)", "0FFFFF1F2"),
        // SFA 6.1.8.1, 6.1.9 figura 9(a): estremo condiviso pari.
        (
            "POINT(1 0)",
            "MULTILINESTRING((0 0,1 0),(1 0,2 0))",
            "0FFFFF102",
        ),
        // SFA 6.1.15.3, Touches: quadrati con un lato comune di dimensione 1.
        (
            "POLYGON((0 0,2 0,2 2,0 2,0 0))",
            "POLYGON((2 0,4 0,4 2,2 2,2 0))",
            "FF2F11212",
        ),
        // SFA 6.1.15.3, Touches: quadrati con un solo vertice comune.
        (
            "POLYGON((0 0,2 0,2 2,0 2,0 0))",
            "POLYGON((2 2,4 2,4 4,2 4,2 2))",
            "FF2F01212",
        ),
        // SFA 6.1.15.2, figura 19: realizzazione razionale di due aree
        // sovrapposte, con interni residui e confini che si incrociano.
        (
            "POLYGON((0 0,2 0,2 2,0 2,0 0))",
            "POLYGON((1 1,3 1,3 3,1 3,1 1))",
            "212101212",
        ),
    ];

    #[test]
    fn de9im_analytic_cells_transpose_patterns_and_named_predicates() {
        use wkt::TryFromWkt as _;
        for &(left, right, expected) in ANALYTIC_MATRICES {
            let left = Geometry::<f64>::try_from_wkt_str(left).expect("WKT analitico");
            let right = Geometry::<f64>::try_from_wkt_str(right).expect("WKT analitico");
            let matrix = relate_matrix(&left, &right).expect("matrice analitica");
            assert_eq!(
                matrix,
                arrangement_exhaustive::relate(
                    &left,
                    &right,
                    &mut arrangement_indexed::Work::new(RelateLimits::default(), None)
                )
                .expect("classificazione esaustiva")
            );
            assert_eq!(matrix.to_string(), expected);
            assert!(matrix.matches(expected.parse().expect("pattern esatto")));
            let transpose = relate_matrix(&right, &left)
                .expect("matrice trasposta")
                .cells();
            for (index, cell) in matrix.cells().into_iter().enumerate() {
                assert_eq!(cell, transpose[(index % 3) * 3 + index / 3]);
                for token in *b"TF*012" {
                    let mut pattern = [b'*'; 9];
                    pattern[index] = token;
                    let text = std::str::from_utf8(&pattern).expect("ASCII");
                    let expected = match token {
                        b'*' => true,
                        b'T' => cell != b'F',
                        _ => token == cell,
                    };
                    assert_eq!(
                        matrix.matches(text.parse().expect("pattern valido")),
                        expected
                    );
                }
            }
            for predicate in [
                SpatialPredicate::Intersects,
                SpatialPredicate::Disjoint,
                SpatialPredicate::Contains,
                SpatialPredicate::Within,
                SpatialPredicate::EqualsTopo,
                SpatialPredicate::Covers,
                SpatialPredicate::CoveredBy,
                SpatialPredicate::ContainsProperly,
                SpatialPredicate::Touches,
                SpatialPredicate::Crosses,
                SpatialPredicate::Overlaps,
            ] {
                assert_eq!(
                    matrix.evaluate(predicate),
                    exact_predicate(&left, &right, predicate).expect("predicato")
                );
                assert_eq!(
                    matrix.evaluate(predicate),
                    exact_predicate_validated(&left, &right, predicate)
                        .expect("predicato validato")
                );
            }
        }
    }

    /// Grammatica OGC 6.1.15.2: solo nove simboli TF*012. La controprova
    /// colloca il simbolo errato DOPO una cella che darebbe gia' false;
    /// il parsing non deve dipendere dal contenuto della matrice.
    #[test]
    fn de9im_rejects_the_entire_invalid_pattern_without_echoing_input() {
        for invalid in [
            "",
            "********",
            "**********",
            "********X",
            "F*******X",
            "t********",
            "f********",
            "é*******",
            " ******* ",
            "RELATE(G1,G2,\"*********\")",
        ] {
            assert_eq!(invalid.parse::<De9imPattern>(), Err(InvalidDe9imPattern));
        }
        assert_eq!(
            InvalidDe9imPattern.to_string(),
            "pattern DE-9IM non valido: richiesti nove caratteri ASCII TF*012"
        );
    }

    /// Contratto di ingresso del kernel: stessi errori tipizzati dei
    /// predicati, senza sostituire invalidita' con una matrice plausibile.
    #[test]
    fn de9im_matrix_rejects_invalid_operands() {
        let point = Geometry::Point(Point::new(0.0, 0.0));
        let non_finite = Geometry::Point(Point::new(f64::INFINITY, 0.0));
        assert!(matches!(
            relate_matrix(&non_finite, &point),
            Err(PredicateError::NonFiniteCoordinate { side: "left" })
        ));
        assert!(matches!(
            relate_matrix(&point, &non_finite),
            Err(PredicateError::NonFiniteCoordinate { side: "right" })
        ));
        assert!(matches!(
            relate_matrix(&bowtie(), &point),
            Err(PredicateError::InvalidGeometry { side: "left", .. })
        ));
        assert!(matches!(
            relate_matrix(&point, &bowtie()),
            Err(PredicateError::InvalidGeometry { side: "right", .. })
        ));
    }

    /// **Sintetico attraverso la conversione reale**: `classifica_lato` e' la
    /// stessa funzione che `validate` chiama davvero, non una copia.
    /// L'innesco e' costruito a mano perche' nessun reperto reale interrompe
    /// `geo` col candidato esatto (vedi
    /// `tests/distinzione_attraverso_i_chiamanti.rs`). Prova che l'interruzione
    /// resta un `ValidazioneNonConclusa` col lato di questo modulo.
    #[test]
    fn classifica_lato_non_appiattisce_l_interruzione() {
        let esito = crate::EsitoValidazione::NonConclusa("forma di prova");
        let errore = classifica_lato(esito, "left");
        assert!(
            matches!(
                errore,
                PredicateError::ValidazioneNonConclusa("forma di prova")
            ),
            "atteso ValidazioneNonConclusa, ottenuto: {errore:?}"
        );
        assert_eq!(
            errore.to_string(),
            "validazione OGC non conclusa: forma di prova (contenuto non pubblicato)"
        );
    }

    /// Controprova: lo stesso lato, con un esito **concluso** (geometria
    /// davvero invalida), produce l'altra variante — mai la stessa. Senza
    /// questa meta', la prova sopra non dimostrerebbe una distinzione: solo
    /// che un input produce un'etichetta.
    #[test]
    fn classifica_lato_su_esito_concluso_resta_invalidgeometry() {
        let esito = crate::EsitoValidazione::NonValida(crate::RagioneNonValida::AutoIntersezione);
        let errore = classifica_lato(esito, "right");
        assert!(
            matches!(
                errore,
                PredicateError::InvalidGeometry { side: "right", .. }
            ),
            "atteso InvalidGeometry, ottenuto: {errore:?}"
        );
        assert_eq!(
            errore.to_string(),
            "geometria right non valida: anello con auto-intersezione"
        );
    }

    #[test]
    fn de9im_predicates_distinguish_boundary_and_interior() {
        let area = rect(0.0, 0.0, 2.0, 2.0);
        let inside = Geometry::Point(Point::new(1.0, 1.0));
        let boundary = Geometry::Point(Point::new(0.0, 1.0));
        assert!(evaluate(&area, &inside, SpatialPredicate::Contains).unwrap());
        assert!(evaluate(&area, &inside, SpatialPredicate::ContainsProperly).unwrap());
        assert!(!evaluate(&area, &boundary, SpatialPredicate::Contains).unwrap());
        assert!(evaluate(&area, &boundary, SpatialPredicate::Covers).unwrap());
        assert!(evaluate(&boundary, &area, SpatialPredicate::CoveredBy).unwrap());
        assert!(evaluate(&area, &boundary, SpatialPredicate::Touches).unwrap());
    }

    #[test]
    fn equality_crossing_overlap_and_disjoint_are_exact() {
        let horizontal = Geometry::LineString(line_string![
            (x: -1.0, y: 0.0), (x: 1.0, y: 0.0)
        ]);
        let vertical = Geometry::LineString(line_string![
            (x: 0.0, y: -1.0), (x: 0.0, y: 1.0)
        ]);
        assert!(evaluate(&horizontal, &vertical, SpatialPredicate::Crosses).unwrap());
        assert!(evaluate(&horizontal, &horizontal, SpatialPredicate::EqualsTopo).unwrap());
        let far = Geometry::Point(Point::new(100.0, 100.0));
        assert!(evaluate(&horizontal, &far, SpatialPredicate::Disjoint).unwrap());
    }

    #[test]
    fn invalid_left_and_right_inputs_are_rejected_before_relate() {
        let valid = Geometry::Point(Point::new(0.0, 0.0));
        let nan = Geometry::Point(Point::new(f64::NAN, 0.0));
        assert!(matches!(
            evaluate(&nan, &valid, SpatialPredicate::Intersects),
            Err(PredicateError::NonFiniteCoordinate { side: "left" })
        ));
        assert!(matches!(
            evaluate(&valid, &nan, SpatialPredicate::Intersects),
            Err(PredicateError::NonFiniteCoordinate { side: "right" })
        ));
        let invalid = bowtie();
        assert!(matches!(
            evaluate(&valid, &invalid, SpatialPredicate::Intersects),
            Err(PredicateError::InvalidGeometry { side: "right", .. })
        ));
    }

    #[test]
    fn evaluate_validated_matches_the_gated_path_on_valid_inputs() {
        let area = rect(0.0, 0.0, 2.0, 2.0);
        let others = [
            Geometry::Point(Point::new(1.0, 1.0)),
            Geometry::Point(Point::new(0.0, 1.0)),
            Geometry::Point(Point::new(100.0, 100.0)),
            Geometry::LineString(line_string![(x: -1.0, y: 1.0), (x: 3.0, y: 1.0)]),
        ];
        for other in &others {
            for predicate in [
                SpatialPredicate::Intersects,
                SpatialPredicate::Disjoint,
                SpatialPredicate::Contains,
                SpatialPredicate::Within,
                SpatialPredicate::EqualsTopo,
                SpatialPredicate::Covers,
                SpatialPredicate::CoveredBy,
                SpatialPredicate::ContainsProperly,
                SpatialPredicate::Touches,
                SpatialPredicate::Crosses,
                SpatialPredicate::Overlaps,
            ] {
                assert_eq!(
                    evaluate(&area, other, predicate).unwrap(),
                    evaluate_validated(&area, other, predicate).unwrap(),
                    "{predicate:?}"
                );
            }
        }
    }

    #[test]
    fn evaluate_validated_documents_the_caller_precondition() {
        // Test di documentazione del contratto, NON un nuovo modo di
        // accettare geometrie invalide in produzione: il percorso gated
        // rifiuta il bowtie (gate intatto), la variante validated lo prende
        // perche' la precondizione e' del chiamante — qui violata ad arte.
        let bowtie = bowtie();
        let valid = Geometry::Point(Point::new(1.0, 1.0));
        assert!(matches!(
            evaluate(&bowtie, &valid, SpatialPredicate::Intersects),
            Err(PredicateError::InvalidGeometry { side: "left", .. })
        ));
        assert!(
            (evaluate_validated(&bowtie, &valid, SpatialPredicate::Intersects))
                .expect("successo atteso")
        );
    }
}
