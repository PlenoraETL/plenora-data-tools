//! Kernel di `geo.from_wkt`, `geo.geometry_accessors`, `geo.collect` e
//! `geo.line_locate_point`.
//!
//! Kernel puri su `geo::Geometry<f64>` piu' l'adapter di colonna per
//! `from_wkt` (input `Utf8` -> celle WKB). Il raggruppamento di `collect`
//! per chiave non e' in questo workspace: il kernel riceve un gruppo gia'
//! formato e ordinato. I kernel puri rendono [`ExtensionError`]
//! ([`ExtensionError::del_passo`] lo porta su [`PlenoraError`]); l'adapter
//! di `from_wkt` rende direttamente [`PlenoraError`].

use std::collections::BTreeMap;

use geo::algorithm::line_locate_point::LineLocatePoint;
use geo::{Geometry, MultiLineString, MultiPoint, MultiPolygon, Point};
use plenora_core::arrow::array::StringArray;
use plenora_core::diagnostics::{
    RowDiagnosticExample, RowDiagnosticScope, RowDiagnostics, RowDiagnosticsCompleteness,
    ROW_DIAGNOSTICS_CONTRACT, ROW_DIAGNOSTICS_INDEX_BASIS,
};
use plenora_core::PlenoraError;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use wkt::ToWkt;

use crate::arrow_adapter::encode_geometry;
use crate::construction::{geometry_from_wkt, ConstructionError};
use crate::geometry_type_name;
use crate::ValidazioneProtetta as _;

/// L'errore delle estensioni geo (`extensions`, `extensions2`, `extensions3`):
/// un tipo solo, perche' la regola di categoria ([`Self::e_interna`]) si
/// scrive una volta.
#[derive(Debug, Error)]
pub enum ExtensionError {
    /// Un parametro fuori dominio.
    #[error("parametro {name} non valido: {reason}")]
    InvalidParameter {
        /// Il nome del parametro.
        name: &'static str,
        /// Il dominio violato, mai il valore.
        reason: &'static str,
    },
    /// La geometria d'ingresso non supera la validazione OGC; il payload e'
    /// la ragione del validatore.
    #[error("geometria di input non valida: {0}")]
    InvalidInput(String),
    /// La geometria in posizione `index` non e' poligonale
    /// (`coverage_validate`, `shared_paths`).
    #[error("geometria {index} non poligonale ({found}): attesa Polygon/MultiPolygon")]
    UnsupportedGeometry { index: usize, found: &'static str },
    /// La geometria in posizione `index` non supera la validazione OGC.
    #[error("geometria {index} non valida: {reason}")]
    InvalidGeometry { index: usize, reason: String },
    /// La geometria in posizione `index` ha coordinate non finite.
    #[error("geometria {index} contiene coordinate NaN o infinite")]
    NonFiniteCoordinate { index: usize },
    /// La geometria prodotta non supera la validazione OGC.
    #[error("geometria prodotta non valida: {0}")]
    InvalidOutput(String),
    /// Celle di `generate_grid` oltre [`crate::extensions2::MAX_GRID_CELLS`].
    #[error("celle della griglia oltre il limite {limit}: {actual}")]
    CellLimit { actual: u64, limit: u64 },
    /// Sovrapposizioni di `coverage_validate` oltre `max_issues`: errore,
    /// mai un elenco troncato.
    #[error("issues di copertura oltre il limite {limit}")]
    IssueLimit { limit: u64 },
    /// Un indice o un conteggio non rappresentabile.
    #[error("indice o conteggio non rappresentabile come uint64")]
    IndexOverflow,
    /// Il taglio di `subdivide` non scende sotto la soglia entro
    /// [`crate::extensions2::MAX_SUBDIVIDE_DEPTH`] livelli.
    #[error("subdivide non converge entro {limit} livelli di ricorsione")]
    SubdivideDepth { limit: u32 },
    /// Invariante interna violata: un errore propagato, mai un panico.
    #[error("internal error: {0}")]
    Internal(&'static str),
    /// La validazione OGC non ha concluso: `geo` si e' interrotta.
    ///
    /// **Non** e' una geometria invalida. Nessuno ha dimostrato che l'ingresso
    /// sia sbagliato, e accusarlo manderebbe chi legge a correggere un errore
    /// che non ha commesso. Porta la *forma* del payload, mai il contenuto.
    #[error("validazione OGC non conclusa: {0} (contenuto non pubblicato)")]
    ValidazioneNonConclusa(&'static str),
    /// Lo spostamento che la griglia di `i_overlay` introdurrebbe supera la
    /// precisione dichiarata (`geo.subdivide`, `geo.coverage_validate`).
    /// Nessun dato nel messaggio.
    #[error("geometria troppo estesa per la precisione dichiarata")]
    PrecisionInsufficient,
    /// Un calcolo di `geo`, `i_overlay` o `rstar` e' andato in panico dentro
    /// `crate::calcolo_protetto`: non accusa l'ingresso, porta la *forma*
    /// del payload, mai il contenuto. Interno.
    #[error("calcolo geometrico non concluso: {0} (contenuto non pubblicato)")]
    CalcoloNonConcluso(&'static str),
}

/// Un calcolo di `geo` (o `rstar`) dietro la barriera dei panici: un panico
/// e' [`ExtensionError::CalcoloNonConcluso`], interno.
pub(crate) fn protetto<T>(calcolo: impl FnOnce() -> T) -> Result<T, ExtensionError> {
    crate::calcolo_protetto(calcolo).map_err(ExtensionError::CalcoloNonConcluso)
}

impl From<crate::rust_backend::griglia::PrecisioneInsufficiente> for ExtensionError {
    fn from(_: crate::rust_backend::griglia::PrecisioneInsufficiente) -> Self {
        Self::PrecisionInsufficient
    }
}

impl ExtensionError {
    /// `true` se la colpa non e' del piano: un'invariante saltata o una
    /// validazione che non ha concluso. Il resto e' `InvalidPlan`.
    #[must_use]
    pub const fn e_interna(&self) -> bool {
        matches!(
            self,
            Self::Internal(_) | Self::ValidazioneNonConclusa(_) | Self::CalcoloNonConcluso(_)
        )
    }

    /// `true` se e' un limite superato dai dati (`IssueLimit`: le
    /// sovrapposizioni di `coverage_validate` oltre `max_issues`): il piano
    /// e' corretto e sono i dati a non entrarci, `ResourceLimit` come ogni
    /// limite (`PlenoraError::ResourceLimit`). `CellLimit` no: le celle di
    /// `generate_grid` dipendono solo dalla config, e superarle e' un piano
    /// sbagliato (`InvalidPlan`, come in analisi).
    #[must_use]
    pub const fn e_un_limite(&self) -> bool {
        matches!(self, Self::IssueLimit { .. })
    }

    /// L'errore nella categoria giusta, con il prefisso dell'operazione:
    /// interno cio' che e' interno, `Unsupported` la precisione
    /// insufficiente (come `RustBackendError::PrecisionInsufficient`),
    /// `ResourceLimit` un limite superato, `InvalidPlan` il resto.
    #[must_use]
    pub fn del_passo(&self, operazione: &str) -> PlenoraError {
        if self.e_interna() {
            PlenoraError::Internal(format!("{operazione}: {self}"))
        } else if matches!(self, Self::PrecisionInsufficient) {
            PlenoraError::Unsupported(format!("{operazione}: {self}"))
        } else if self.e_un_limite() {
            PlenoraError::ResourceLimit(format!("{operazione}: {self}"))
        } else {
            PlenoraError::InvalidPlan(format!("{operazione}: {self}"))
        }
    }
}

pub(crate) fn ensure_valid(geometry: &Geometry<f64>) -> Result<(), ExtensionError> {
    geometry.validazione_protetta().map_err(|esito| {
        esito.separa(
            |ragione| ExtensionError::InvalidInput(ragione.to_string()),
            ExtensionError::ValidazioneNonConclusa,
        )
    })
}

pub(crate) fn validate_output(geometry: Geometry<f64>) -> Result<Geometry<f64>, ExtensionError> {
    geometry.validazione_protetta().map_err(|esito| {
        esito.separa(
            |ragione| ExtensionError::InvalidOutput(ragione.to_string()),
            ExtensionError::ValidazioneNonConclusa,
        )
    })?;
    Ok(geometry)
}

pub(crate) fn u64_len(len: usize) -> Result<u64, ExtensionError> {
    u64::try_from(len).map_err(|_| ExtensionError::IndexOverflow)
}

pub(crate) const fn invalid_parameter(name: &'static str, reason: &'static str) -> ExtensionError {
    ExtensionError::InvalidParameter { name, reason }
}

pub(crate) fn check_tolerance(tolerance: f64) -> Result<(), ExtensionError> {
    if !tolerance.is_finite() || tolerance < 0.0 {
        return Err(invalid_parameter(
            "tolerance",
            "deve essere finita e non negativa",
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// geo.from_wkt
// ---------------------------------------------------------------------------

/// Politica sugli errori di parsing WKT (`on_error` di `geo.from_wkt`),
/// accettata in deserializzazione.
///
/// Entrambe le varianti rifiutano l'intera colonna con la diagnostica per
/// riga: nessuna delle due autorizza un rimedio implicito.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OnWktError {
    /// Nessuna cella invalida viene convertita in null, nonostante il nome.
    Null,
    /// Cella WKT invalida -> rifiuto fail-closed row-scoped.
    Fail,
}

/// Perche' una cella WKT non e' diventata WKB.
///
/// # Perche' due casi e non un codice solo
///
/// Perche' «questa cella e' invalida» e «non si e' potuto stabilirlo» sono
/// affermazioni diverse. Con un codice solo la validazione OGC interrotta
/// finirebbe contata fra le celle invalide, e l'adapter renderebbe
/// `DataMapping`: chi legge andrebbe a correggere una riga che nessuno ha
/// dimostrato sbagliata.
enum CellaNonConvertita {
    /// Difetto della cella: entra nella diagnostica row-scoped con questo
    /// codice.
    Invalida(&'static str),
    /// La validazione OGC non ha concluso. Porta la **forma** del payload,
    /// mai il contenuto.
    NonConclusa(&'static str),
}

/// Converte una cella WKT non-null in WKB senza percorso di remediation.
fn wkt_cell_to_wkb(value: &str) -> Result<Vec<u8>, CellaNonConvertita> {
    let geometry = geometry_from_wkt(value).map_err(|errore| match errore {
        ConstructionError::ValidazioneNonConclusa(forma) => CellaNonConvertita::NonConclusa(forma),
        _ => CellaNonConvertita::Invalida("geometry.invalid_wkt"),
    })?;
    encode_geometry(&geometry).map_err(|_| CellaNonConvertita::Invalida("geometry.encoding_failed"))
}

/// Adapter di colonna per `geo.from_wkt`: celle `Utf8` -> celle WKB.
///
/// Decodifica ogni cella `Utf8`, codifica il risultato in WKB e preserva
/// i null. Le righe sono indipendenti: iterazione parallela con collect
/// indicizzato, quindi l'ordine dell'output resta deterministico.
///
/// # Errors
///
/// `PlenoraError::DataMapping` (fase di lettura) con diagnostica per riga
/// completa se almeno una cella non diventa WKB: testo non WKT, `SRID=`,
/// dimensioni Z/M, geometria OGC-invalida (causa `geometry.invalid_wkt`) o
/// WKB oltre il limite per cella (causa `geometry.encoding_failed`).
/// `PlenoraError::Internal` se la validazione OGC di una cella non
/// conclude. Entrambi i valori di [`OnWktError`] sono fail-closed.
pub fn from_wkt_column(
    values: &StringArray,
    on_error: OnWktError,
) -> Result<Vec<Option<Vec<u8>>>, PlenoraError> {
    from_wkt_column_named(values, on_error, None)
}

/// Variante di [`from_wkt_column`] che scrive il nome della colonna
/// sorgente negli esempi della diagnostica, senza includerne mai i valori.
///
/// # Errors
///
/// Come [`from_wkt_column`]: `DataMapping` con la diagnostica per riga
/// (conteggi completi per causa, al piu' 10 esempi, nessun valore),
/// `Internal` per una validazione che non conclude.
pub fn from_wkt_column_named(
    values: &StringArray,
    _on_error: OnWktError,
    column: Option<&str>,
) -> Result<Vec<Option<Vec<u8>>>, PlenoraError> {
    const EXAMPLES_LIMIT: u64 = 10;
    let cells: Vec<Option<&str>> = values.iter().collect();
    // Come `map_nullable`: il primo errore IN ORDINE DI RIGA e'
    // selezionato dal collect sequenziale — la riga riportata nel messaggio
    // non puo' dipendere dallo scheduling di rayon.
    let results: Vec<Result<Option<Vec<u8>>, CellaNonConvertita>> = cells
        .into_par_iter()
        .map(|cell| cell.map_or_else(|| Ok(None), |value| wkt_cell_to_wkb(value).map(Some)))
        .collect();
    let mut output = Vec::with_capacity(results.len());
    let mut examples = Vec::new();
    let mut observed_total = 0_u64;
    let mut counts = BTreeMap::new();
    for (row, result) in results.into_iter().enumerate() {
        match result {
            Ok(cell) => output.push(cell),
            // Una validazione che non conclude non e' una cella invalida:
            // esce subito come difetto interno invece di essere contata fra
            // le righe sbagliate. Il ciclo scorre in ordine di riga, quindi
            // quale interruzione vinca non dipende dallo scheduling.
            Err(CellaNonConvertita::NonConclusa(forma)) => {
                return Err(PlenoraError::Internal(format!(
                    "validazione OGC non conclusa sulla cella WKT: {forma} \
                     (contenuto non pubblicato)"
                )));
            }
            Err(CellaNonConvertita::Invalida(cause)) => {
                observed_total = observed_total.checked_add(1).ok_or_else(|| {
                    PlenoraError::Internal("overflow del conteggio diagnostico WKT".into())
                })?;
                let cause_count = counts.entry(cause.to_owned()).or_insert(0_u64);
                *cause_count = cause_count.checked_add(1).ok_or_else(|| {
                    PlenoraError::Internal("overflow del conteggio causa WKT".into())
                })?;
                let source_index = u64::try_from(row).map_err(|_| {
                    PlenoraError::Internal("indice sorgente WKT non rappresentabile".into())
                })?;
                if u64::try_from(examples.len())
                    .map_err(|_| PlenoraError::Internal("troppi esempi WKT".into()))?
                    < EXAMPLES_LIMIT
                {
                    examples.push(RowDiagnosticExample {
                        source_index,
                        cause: cause.to_owned(),
                        column: column.map(ToOwned::to_owned),
                        key: None,
                        write_state: None,
                    });
                }
            }
        }
    }
    if observed_total == 0 {
        return Ok(output);
    }
    let report = RowDiagnostics {
        contract: ROW_DIAGNOSTICS_CONTRACT.to_owned(),
        scope: RowDiagnosticScope::Read,
        index_basis: ROW_DIAGNOSTICS_INDEX_BASIS.to_owned(),
        completeness: RowDiagnosticsCompleteness::Complete,
        knowledge_limits: None,
        observed_total,
        total: Some(observed_total),
        input_total: None,
        counts,
        examples_limit: EXAMPLES_LIMIT,
        examples_truncated: observed_total > EXAMPLES_LIMIT,
        examples,
        diagnostic_state_counts: None,
        write_outcome: None,
    };
    Err(
        PlenoraError::DataMapping("geometrie WKT rifiutate; consultare row_diagnostics".into())
            .with_row_diagnostics(report),
    )
}

// ---------------------------------------------------------------------------
// geo.geometry_accessors
// ---------------------------------------------------------------------------

/// Accessori per riga di `geo.geometry_accessors`.
///
/// Convenzioni:
/// - `num_geometries`: parti della geometria (1 per le geometrie semplici,
///   N per Multi*/`GeometryCollection`);
/// - `num_interior_rings`: anelli interni di Polygon/MultiPolygon, 0 altro;
/// - `start_point`/`end_point`: WKT `POINT(x y)` solo per una `LineString`
///   (o Line) aperta; null per poligoni, multi-parti e linee chiuse;
/// - `is_closed`: `LineString::is_closed` per le linee, true per i tipi
///   poligonali (anelli chiusi per definizione), false altro.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GeometryAccessors {
    /// Nome del tipo: `Point`, `LineString`, `Polygon`, `MultiPoint`,
    /// `MultiLineString`, `MultiPolygon`, `GeometryCollection`.
    pub geometry_type: &'static str,
    /// Parti della geometria.
    pub num_geometries: u64,
    /// Anelli interni.
    pub num_interior_rings: u64,
    /// Primo punto in WKT, solo per una linea aperta.
    pub start_point: Option<String>,
    /// Ultimo punto in WKT, solo per una linea aperta.
    pub end_point: Option<String>,
    /// Se la geometria e' chiusa.
    pub is_closed: bool,
}

fn point_wkt(point: Point<f64>) -> String {
    Geometry::Point(point).wkt_string()
}

/// Calcola gli accessori per riga di `geo.geometry_accessors`.
///
/// Le convenzioni sui campi sono documentate su [`GeometryAccessors`].
///
/// # Errors
///
/// - `ExtensionError::InvalidInput`: la geometria non supera la
///   validazione OGC;
/// - `ExtensionError::IndexOverflow`: un conteggio (parti o anelli
///   interni) non e' rappresentabile come `u64`.
pub fn geometry_accessors(geometry: &Geometry<f64>) -> Result<GeometryAccessors, ExtensionError> {
    ensure_valid(geometry)?;
    let num_geometries = match geometry {
        Geometry::MultiPoint(values) => u64_len(values.0.len())?,
        Geometry::MultiLineString(values) => u64_len(values.0.len())?,
        Geometry::MultiPolygon(values) => u64_len(values.0.len())?,
        Geometry::GeometryCollection(values) => u64_len(values.0.len())?,
        _ => 1,
    };
    let num_interior_rings = match geometry {
        Geometry::Polygon(polygon) => u64_len(polygon.interiors().len())?,
        Geometry::MultiPolygon(polygons) => polygons.iter().try_fold(0_u64, |total, polygon| {
            u64_len(polygon.interiors().len()).map(|count| total + count)
        })?,
        _ => 0,
    };
    let (start_point, end_point, is_closed) = match geometry {
        Geometry::LineString(line) if !line.is_closed() && line.0.len() >= 2 => (
            Some(point_wkt(Point::from(line.0[0]))),
            Some(point_wkt(Point::from(line.0[line.0.len() - 1]))),
            false,
        ),
        Geometry::LineString(line) => (None, None, line.is_closed()),
        Geometry::Line(line) => (
            Some(point_wkt(line.start_point())),
            Some(point_wkt(line.end_point())),
            false,
        ),
        Geometry::Polygon(_) | Geometry::MultiPolygon(_) => (None, None, true),
        _ => (None, None, false),
    };
    Ok(GeometryAccessors {
        geometry_type: geometry_type_name(geometry),
        num_geometries,
        num_interior_rings,
        start_point,
        end_point,
        is_closed,
    })
}

// ---------------------------------------------------------------------------
// geo.collect
// ---------------------------------------------------------------------------

/// Aggrega un gruppo ordinato di geometrie in una collezione SENZA unione
/// topologica.
///
/// Gruppo omogeneo di Point/LineString/Polygon -> Multi*, gruppo misto ->
/// `GeometryCollection`. I null sono saltati; un gruppo con una sola
/// geometria non-null resta la geometria singola; un gruppo senza
/// geometrie non-null produce `None`.
///
/// # Errors
///
/// - `ExtensionError::InvalidInput`: una geometria del gruppo non supera
///   la validazione OGC;
/// - `ExtensionError::InvalidOutput`: la collezione prodotta non supera
///   la validazione OGC;
/// - `ExtensionError::Internal`: invariante interna violata (omogeneita'
///   del gruppo gia' verificata a monte).
pub fn collect_geometries(
    geometries: &[Option<Geometry<f64>>],
) -> Result<Option<Geometry<f64>>, ExtensionError> {
    let present: Vec<&Geometry<f64>> = geometries.iter().flatten().collect();
    for geometry in &present {
        ensure_valid(geometry)?;
    }
    match present.as_slice() {
        [] => Ok(None),
        [single] => Ok(Some((*single).clone())),
        _ => {
            let all_points = present
                .iter()
                .all(|geometry| matches!(geometry, Geometry::Point(_)));
            let all_lines = present
                .iter()
                .all(|geometry| matches!(geometry, Geometry::LineString(_)));
            let all_polygons = present
                .iter()
                .all(|geometry| matches!(geometry, Geometry::Polygon(_)));
            let collected = if all_points {
                Geometry::MultiPoint(MultiPoint::new(
                    present
                        .iter()
                        .map(|geometry| match geometry {
                            Geometry::Point(point) => Ok(*point),
                            _ => Err(ExtensionError::Internal("all_points verificato")),
                        })
                        .collect::<Result<Vec<_>, ExtensionError>>()?,
                ))
            } else if all_lines {
                Geometry::MultiLineString(MultiLineString::new(
                    present
                        .iter()
                        .map(|geometry| match geometry {
                            Geometry::LineString(line) => Ok(line.clone()),
                            _ => Err(ExtensionError::Internal("all_lines verificato")),
                        })
                        .collect::<Result<Vec<_>, ExtensionError>>()?,
                ))
            } else if all_polygons {
                Geometry::MultiPolygon(MultiPolygon::new(
                    present
                        .iter()
                        .map(|geometry| match geometry {
                            Geometry::Polygon(polygon) => Ok(polygon.clone()),
                            _ => Err(ExtensionError::Internal("all_polygons verificato")),
                        })
                        .collect::<Result<Vec<_>, ExtensionError>>()?,
                ))
            } else {
                Geometry::GeometryCollection(
                    present.iter().map(|geometry| (*geometry).clone()).collect(),
                )
            };
            validate_output(collected).map(Some)
        }
    }
}

// ---------------------------------------------------------------------------
// geo.line_locate_point
// ---------------------------------------------------------------------------

/// Frazione in `[0, 1]` della proiezione del punto piu' vicino sulla linea
/// (semantica `ST_LineLocatePoint`).
///
/// `None` per geometrie non-LineString (incluse `MultiLineString`)
/// o linee vuote.
///
/// # Errors
///
/// `ExtensionError::InvalidInput` se la geometria non supera la
/// validazione OGC.
pub fn line_locate_point(
    geometry: &Geometry<f64>,
    point: &Point<f64>,
) -> Result<Option<f64>, ExtensionError> {
    ensure_valid(geometry)?;
    // `geo` dietro la barriera: un suo panico e' `Internal`, con la sola
    // forma del payload.
    let fraction = crate::calcolo_protetto(|| match geometry {
        // Linea vuota/degenere: nessuna proiezione definita.
        Geometry::LineString(line) if line.0.len() >= 2 => line.line_locate_point(point),
        Geometry::Line(line) => line.line_locate_point(point),
        _ => None,
    })
    .map_err(ExtensionError::Internal)?;
    Ok(fraction)
}

#[cfg(test)]
// Confronti float esatti intenzionali: le fixture sono costruite per
// produrre valori esatti (coordinate note, round-trip bit-esatti); il
// confronto per bit e' il contratto verificato, non un'approssimazione.
#[allow(clippy::float_cmp)]
mod tests {
    use super::*;
    use crate::test_support::rect;
    use geo::{line_string, polygon, GeometryCollection, LineString};
    use plenora_core::arrow::array::Array;

    fn point(x: f64, y: f64) -> Geometry<f64> {
        Geometry::Point(Point::new(x, y))
    }

    fn square() -> Geometry<f64> {
        square_at(0.0)
    }

    fn square_at(offset: f64) -> Geometry<f64> {
        rect(offset, offset, offset + 4.0, offset + 4.0)
    }

    // --- geo.from_wkt -------------------------------------------------------

    #[test]
    fn from_wkt_column_parses_preserves_nulls_and_decodes_back() {
        let values = StringArray::from(vec![
            Some("POINT(12 41)"),
            None,
            Some("LINESTRING(0 0, 1 1)"),
        ]);
        let cells = from_wkt_column(&values, OnWktError::Fail).expect("parse");
        assert_eq!(cells.len(), 3);
        assert!(cells[1].is_none());
        assert_eq!(
            crate::geometry_from_wkb(cells[0].as_deref().expect("wkb")).expect("decode"),
            point(12.0, 41.0)
        );
        assert_eq!(
            crate::geometry_from_wkb(cells[2].as_deref().expect("wkb")).expect("decode"),
            Geometry::LineString(line_string![(x: 0.0, y: 0.0), (x: 1.0, y: 1.0)])
        );
    }

    #[test]
    fn from_wkt_column_applies_the_on_error_policy() {
        let invalid = StringArray::from(vec![Some("POINT(12 41"), Some("SRID=4326;POINT(1 2)")]);
        for policy in [OnWktError::Null, OnWktError::Fail] {
            let failed = from_wkt_column(&invalid, policy)
                .expect_err("politica legacy non deve pubblicare null sintetici");
            assert_eq!(
                failed.row_diagnostics().map(|report| report.observed_total),
                Some(2)
            );
        }
        let dimensional = StringArray::from(vec![Some("POINT Z (1 2 3)")]);
        assert!(from_wkt_column(&dimensional, OnWktError::Fail)
            .is_err_and(|error| error.row_diagnostics().is_some()));
    }

    #[test]
    fn from_wkt_never_publishes_invalid_cells_and_reports_all_source_rows() {
        let values = StringArray::from(vec![
            Some("POINT(1 2)"),
            Some("POINT(12 41"),
            None,
            Some("SRID=4326;POINT(1 2)"),
        ]);
        for policy in [OnWktError::Null, OnWktError::Fail] {
            let error = from_wkt_column(&values, policy)
                .expect_err("WKT invalido trasformato in output accettato");
            let report = error
                .row_diagnostics()
                .expect("diagnostica row-scoped mancante");
            assert_eq!(report.observed_total, 2);
            assert_eq!(report.total, Some(2));
            assert_eq!(report.counts["geometry.invalid_wkt"], 2);
            assert_eq!(
                report
                    .examples
                    .iter()
                    .map(|example| example.source_index)
                    .collect::<Vec<_>>(),
                vec![1, 3]
            );
            assert!(report
                .examples
                .iter()
                .all(|example| example.column.is_none()));
            assert!(!error.to_string().contains("POINT"));
        }
    }

    #[test]
    fn from_wkt_column_empty_input_produces_empty_output() {
        let values = StringArray::from(Vec::<Option<&str>>::new());
        assert_eq!(values.len(), 0);
        assert!(from_wkt_column(&values, OnWktError::Null)
            .expect("vuoto")
            .is_empty());
    }

    // --- geo.geometry_accessors ---------------------------------------------

    #[test]
    fn accessors_cover_polygons_with_interior_rings() {
        let with_hole = Geometry::Polygon(polygon!(
            exterior: [(x: 0.0, y: 0.0), (x: 8.0, y: 0.0), (x: 8.0, y: 8.0), (x: 0.0, y: 8.0), (x: 0.0, y: 0.0)],
            interiors: [[(x: 2.0, y: 2.0), (x: 4.0, y: 2.0), (x: 4.0, y: 4.0), (x: 2.0, y: 2.0)]],
        ));
        let accessors = geometry_accessors(&with_hole).expect("accessors");
        assert_eq!(accessors.geometry_type, "Polygon");
        assert_eq!(accessors.num_geometries, 1);
        assert_eq!(accessors.num_interior_rings, 1);
        assert_eq!(accessors.start_point, None);
        assert_eq!(accessors.end_point, None);
        assert!(accessors.is_closed);

        let multi = Geometry::MultiPolygon(MultiPolygon::new(vec![
            match with_hole {
                Geometry::Polygon(polygon) => polygon,
                _ => unreachable!(),
            },
            match square_at(10.0) {
                Geometry::Polygon(polygon) => polygon,
                _ => unreachable!(),
            },
        ]));
        let accessors = geometry_accessors(&multi).expect("accessors");
        assert_eq!(accessors.geometry_type, "MultiPolygon");
        assert_eq!(accessors.num_geometries, 2);
        assert_eq!(accessors.num_interior_rings, 1);
        assert!(accessors.is_closed);
    }

    #[test]
    fn accessors_expose_endpoints_only_for_open_single_lines() {
        let open = Geometry::LineString(line_string![(x: 0.0, y: 0.0), (x: 3.0, y: 4.0)]);
        let accessors = geometry_accessors(&open).expect("accessors");
        assert_eq!(accessors.geometry_type, "LineString");
        assert!(!accessors.is_closed);
        assert_eq!(
            accessors.start_point.as_deref(),
            Some(point_wkt(Point::new(0.0, 0.0)).as_str())
        );
        assert_eq!(
            accessors.end_point.as_deref(),
            Some(point_wkt(Point::new(3.0, 4.0)).as_str())
        );

        let closed = Geometry::LineString(line_string![
            (x: 0.0, y: 0.0), (x: 1.0, y: 0.0), (x: 0.0, y: 0.0)
        ]);
        let accessors = geometry_accessors(&closed).expect("accessors");
        assert!(accessors.is_closed);
        assert_eq!(accessors.start_point, None);
        assert_eq!(accessors.end_point, None);

        let multi = Geometry::MultiLineString(MultiLineString::new(vec![line_string![
            (x: 0.0, y: 0.0),
            (x: 1.0, y: 1.0)
        ]]));
        let accessors = geometry_accessors(&multi).expect("accessors");
        assert_eq!(accessors.num_geometries, 1);
        assert_eq!(accessors.start_point, None);
    }

    #[test]
    fn accessors_handle_points_multiparts_collections_and_empty_geometries() {
        let accessors = geometry_accessors(&point(1.0, 2.0)).expect("accessors");
        assert_eq!(accessors.geometry_type, "Point");
        assert_eq!(accessors.num_geometries, 1);
        assert_eq!(accessors.num_interior_rings, 0);
        assert!(!accessors.is_closed);

        let multi = Geometry::MultiPoint(MultiPoint::new(vec![
            Point::new(0.0, 0.0),
            Point::new(1.0, 1.0),
        ]));
        assert_eq!(geometry_accessors(&multi).unwrap().num_geometries, 2);

        let collection =
            Geometry::GeometryCollection(GeometryCollection(vec![point(0.0, 0.0), square()]));
        let accessors = geometry_accessors(&collection).expect("accessors");
        assert_eq!(accessors.geometry_type, "GeometryCollection");
        assert_eq!(accessors.num_geometries, 2);
        assert_eq!(accessors.num_interior_rings, 0);

        let empty = Geometry::MultiPoint(MultiPoint::new(Vec::new()));
        let accessors = geometry_accessors(&empty).expect("accessors");
        assert_eq!(accessors.num_geometries, 0);
        assert_eq!(accessors.start_point, None);
    }

    // --- geo.collect ---------------------------------------------------------

    #[test]
    fn collect_keeps_single_geometry_and_skips_nulls() {
        assert_eq!(collect_geometries(&[]).unwrap(), None);
        assert_eq!(collect_geometries(&[None, None]).unwrap(), None);
        assert_eq!(
            collect_geometries(&[None, Some(point(1.0, 2.0)), None]).unwrap(),
            Some(point(1.0, 2.0))
        );
    }

    #[test]
    fn collect_promotes_homogeneous_groups_to_multi_geometries() {
        let points = collect_geometries(&[Some(point(0.0, 0.0)), Some(point(1.0, 1.0))])
            .unwrap()
            .expect("gruppo");
        assert_eq!(
            points,
            Geometry::MultiPoint(MultiPoint::new(vec![
                Point::new(0.0, 0.0),
                Point::new(1.0, 1.0)
            ]))
        );

        let lines = collect_geometries(&[
            Some(Geometry::LineString(
                line_string![(x: 0.0, y: 0.0), (x: 1.0, y: 0.0)],
            )),
            Some(Geometry::LineString(
                line_string![(x: 2.0, y: 0.0), (x: 3.0, y: 0.0)],
            )),
        ])
        .unwrap()
        .expect("gruppo");
        assert!(matches!(lines, Geometry::MultiLineString(_)));

        let polygons = collect_geometries(&[Some(square()), Some(square_at(10.0))])
            .unwrap()
            .expect("gruppo");
        assert!(matches!(polygons, Geometry::MultiPolygon(_)));
    }

    #[test]
    fn collect_mixed_groups_become_geometry_collections_without_union() {
        let collected = collect_geometries(&[Some(square()), Some(point(9.0, 9.0)), None])
            .unwrap()
            .expect("gruppo");
        match collected {
            Geometry::GeometryCollection(values) => {
                assert_eq!(values.len(), 2);
                assert!(matches!(values[0], Geometry::Polygon(_)));
                assert_eq!(values[1], point(9.0, 9.0));
            }
            other => panic!("attesa GeometryCollection, ricevuto {other:?}"),
        }
    }

    // --- geo.line_locate_point ----------------------------------------------

    #[test]
    fn line_locate_point_projects_known_points() {
        let line = Geometry::LineString(line_string![(x: 0.0, y: 0.0), (x: 10.0, y: 0.0)]);
        assert_eq!(
            line_locate_point(&line, &Point::new(5.0, 3.0)).unwrap(),
            Some(0.5)
        );
        assert_eq!(
            line_locate_point(&line, &Point::new(0.0, 0.0)).unwrap(),
            Some(0.0)
        );
        assert_eq!(
            line_locate_point(&line, &Point::new(10.0, 0.0)).unwrap(),
            Some(1.0)
        );
    }

    #[test]
    fn line_locate_point_clamps_far_points_and_handles_closed_lines() {
        let line = Geometry::LineString(line_string![(x: 0.0, y: 0.0), (x: 10.0, y: 0.0)]);
        // Punto lontano oltre l'estremo: la proiezione cade sull'estremo.
        let fraction = line_locate_point(&line, &Point::new(1e9, 1e9))
            .unwrap()
            .expect("frazione");
        assert!((0.0..=1.0).contains(&fraction));
        assert_eq!(fraction, 1.0);

        let closed = Geometry::LineString(line_string![
            (x: 0.0, y: 0.0), (x: 10.0, y: 0.0),
            (x: 10.0, y: 10.0), (x: 0.0, y: 0.0),
        ]);
        let fraction = line_locate_point(&closed, &Point::new(5.0, 0.0))
            .unwrap()
            .expect("frazione");
        assert!((0.0..=1.0).contains(&fraction));
    }

    #[test]
    fn line_locate_point_returns_none_for_non_lines_and_empty_lines() {
        assert_eq!(
            line_locate_point(&square(), &Point::new(0.0, 0.0)).unwrap(),
            None
        );
        assert_eq!(
            line_locate_point(&point(0.0, 0.0), &Point::new(0.0, 0.0)).unwrap(),
            None
        );
        let multi = Geometry::MultiLineString(MultiLineString::new(vec![line_string![
            (x: 0.0, y: 0.0),
            (x: 1.0, y: 1.0)
        ]]));
        assert_eq!(
            line_locate_point(&multi, &Point::new(0.0, 0.0)).unwrap(),
            None
        );
        let empty = Geometry::LineString(LineString::new(Vec::new()));
        assert_eq!(
            line_locate_point(&empty, &Point::new(0.0, 0.0)).unwrap(),
            None
        );
    }
}
