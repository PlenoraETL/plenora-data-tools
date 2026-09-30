//! Struct serde di configurazione per l'analisi dei contratti `geo.*`.
//!
//! Pubbliche perche' l'esecutore (`plenora-pipeline`) legga la config con
//! gli stessi tipi dell'analisi: una sola lettura della config, nessuna
//! seconda copia dei nomi e dei default che potrebbe divergere.

use serde::Deserialize;

use crate::spatial_join::JoinPredicate;
use crate::topology::OverlayMode;

// ---------------------------------------------------------------------------
// Config serde minimali: stessi nomi e domini del protocollo legacy, senza i
// parametri di trasporto.
// ---------------------------------------------------------------------------

/// Config vuota (`{}`) delle operazioni senza parametri: ogni chiave si
/// rifiuta con `InvalidPlan`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EmptyConfig {}

/// Config delle misure e delle conversioni che aggiungono una colonna
/// (`area`, `length`, `perimeter`, `vertex_count`, `to_wkt`, misure
/// geodetiche).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutputColumnConfig {
    /// Nome della colonna aggiunta; assente vale il nome di default
    /// dell'operazione (l'id senza `geo.`, `wkt` per `to_wkt`). Deve essere
    /// un nome valido e libero nello schema d'ingresso.
    pub output_column: Option<String>,
}

/// Estremita' delle linee di `geo.buffer` (`cap`; default `round`, come il
/// kernel). Specchio di `operations::BufferCapStyle`.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum BufferCapParam {
    /// `round`: arco attorno all'estremo.
    Round,
    /// `flat`: taglio netto all'estremo; i punti non hanno buffer.
    Flat,
    /// `square`: quadrato che sporge di `|distance|` oltre l'estremo.
    Square,
}

/// Algoritmo di `geo.simplify` (`policy`; default `douglas_peucker`).
/// Specchio di `operations::SimplifyPolicy`.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum SimplifyPolicyParam {
    /// `douglas_peucker`: Ramer-Douglas-Peucker, `tolerance` e' una
    /// distanza.
    DouglasPeucker,
    /// `preserve_topology`: Visvalingam-Whyatt con conservazione della
    /// topologia, `tolerance` e' un'area.
    PreserveTopology,
}

/// Config di `geo.buffer`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BufferConfig {
    /// Distanza del buffer nelle unita' del CRS, obbligatoria e finita;
    /// negativa erode le parti areali, nulla ne da' l'unione.
    pub distance: f64,
    /// Estremita' delle linee; assente vale `round`.
    pub cap: Option<BufferCapParam>,
}

/// Config di `geo.simplify`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SimplifyConfig {
    /// Soglia, obbligatoria, finita e non negativa: distanza (unita' del
    /// CRS) con `douglas_peucker`, area (unita' al quadrato) con
    /// `preserve_topology`; 0 lascia la geometria invariata.
    pub tolerance: f64,
    /// Algoritmo; assente vale `douglas_peucker`.
    pub policy: Option<SimplifyPolicyParam>,
}

/// `affine_transform`: la matrice affine 2D.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AffineTransformConfig {
    /// Obbligatorio: esattamente sei numeri finiti `[a, b, xoff, d, e,
    /// yoff]`; `(x, y)` diventa `(a x + b y + xoff, d x + e y + yoff)`.
    pub coefficients: Vec<f64>,
}

/// `translate`: spostamento nelle unita' del CRS.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TranslateConfig {
    /// Obbligatorio, finito: spostamento lungo x.
    pub x_offset: f64,
    /// Obbligatorio, finito: spostamento lungo y.
    pub y_offset: f64,
}

/// `scale`: fattori per asse attorno a un'origine.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScaleConfig {
    /// Obbligatorio, finito: fattore lungo x (negativo: riflessione).
    pub x_factor: f64,
    /// Obbligatorio, finito: fattore lungo y (negativo: riflessione).
    pub y_factor: f64,
    /// Facoltativo, finito: x dell'origine che resta ferma. Il valore
    /// usato quando manca non e' deciso qui: il kernel riceve l'origine
    /// esplicita, e il runner passa `0`.
    pub x_origin: Option<f64>,
    /// Facoltativo, finito: y dell'origine, come `x_origin`.
    pub y_origin: Option<f64>,
}

/// `rotate`: angolo attorno a un centro.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RotateConfig {
    /// Obbligatorio, finito: angolo in gradi, positivo in verso
    /// antiorario.
    pub degrees: f64,
    /// Facoltativo, finito: x del centro di rotazione. Il valore usato
    /// quando manca non e' deciso qui: il kernel riceve il centro esplicito,
    /// e il runner passa `0`.
    pub x_origin: Option<f64>,
    /// Facoltativo, finito: y del centro di rotazione, come `x_origin`.
    pub y_origin: Option<f64>,
}

/// `concave_hull`: parametri dell'algoritmo di `geo`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConcaveHullConfig {
    /// Obbligatorio, finito e maggiore di zero: piu' piccolo, piu'
    /// concavo.
    pub concavity: f64,
    /// Facoltativo, finito e non negativo: lunghezza sotto la quale un
    /// lato non si scava. Il valore usato quando manca non e' deciso qui:
    /// il kernel lo riceve esplicito, e il runner passa `0`.
    pub length_threshold: Option<f64>,
}

/// `densify`: un solo parametro, obbligatorio.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DensifyConfig {
    /// Lunghezza massima di un lato dopo la densificazione, nelle unita' del
    /// CRS; finita e maggiore di zero.
    pub max_segment_length: f64,
}

/// `snap_to_grid`: un solo parametro, obbligatorio.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SnapToGridConfig {
    /// Passo della griglia con origine in `(0, 0)`, nelle unita' del CRS;
    /// finito e maggiore di zero.
    pub grid_size: f64,
}

/// `line_substring`: due frazioni obbligatorie, `start_ratio <= end_ratio`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LineSubstringConfig {
    /// Frazione della lunghezza a cui comincia la porzione; in `[0, 1]`.
    pub start_ratio: f64,
    /// Frazione della lunghezza a cui finisce la porzione; in `[0, 1]`,
    /// non minore di `start_ratio`.
    pub end_ratio: f64,
}

/// `line_interpolate_point`: un solo parametro, obbligatorio.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LineInterpolatePointConfig {
    /// Frazione della lunghezza dall'inizio della linea; in `[0, 1]`.
    pub ratio: f64,
}

/// `geo.clean_topology`: pulizia topologica ordinata di poligoni validi
/// (kernel `topology::clean_valid_polygon_topology`).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CleanTopologyConfig {
    /// Raggio della chiusura morfologica, nelle unita' del CRS;
    /// obbligatorio, finito e non negativo (`InvalidPlan` altrimenti).
    /// Con `0` la chiusura non si fa.
    pub snap_tolerance: f64,
    /// Toglie a ogni riga la parte coperta dalle righe precedenti.
    /// Facoltativo: l'analisi lo accetta senza leggerlo, e quando manca il
    /// runner usa `true`.
    pub remove_overlaps: Option<bool>,
    /// Chiude, riga per riga, rientranze e varchi piu' stretti di
    /// `2 * snap_tolerance`. Facoltativo, come `remove_overlaps` (assente:
    /// `true`).
    pub fill_gaps: Option<bool>,
}

/// `voronoi`: tutti i campi opzionali.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VoronoiConfig {
    /// Numero massimo di punti (righe) dell'ingresso; almeno 2 se presente.
    /// Senza, l'analisi non fissa un limite.
    pub max_points: Option<u64>,
}

/// `polygonize`: entrambi i campi facoltativi.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolygonizeConfig {
    /// Noda le linee negli incroci prima del grafo; assente vale `true`.
    pub node_input: Option<bool>,
    /// Fallisce se restano residui; assente vale `false`.
    pub require_complete: Option<bool>,
}

/// Config di `geo.from_coords`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FromCoordsConfig {
    /// Colonna della x, `float64` o `int64`; assente vale `x`.
    pub x_column: Option<String>,
    /// Colonna della y, `float64` o `int64`; assente vale `y`.
    pub y_column: Option<String>,
    /// Nome della colonna geometria prodotta, libero; assente vale
    /// `geometry`.
    pub geometry_column: Option<String>,
    /// CRS della colonna prodotta (tabella integrata, o la definizione del
    /// CRS di piano scritta uguale); assente vale il CRS di piano, e senza
    /// nessuno dei due il passo si rifiuta. Deve essere proiettato.
    pub crs: Option<String>,
}

/// Config dei predicati e delle distanze fra la geometria di ogni riga e un
/// secondo operando fisso.
///
/// Un ingresso ha una sola colonna geometria attiva, quindi il secondo
/// operando arriva dalla config, come WKB esadecimale, e si valida in
/// analisi: contratto WKB strutturale, validità OGC e dominio del CRS
/// dell'ingresso, che si assume sia anche il suo.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OtherWkbConfig {
    /// Secondo operando: WKB ISO XY in esadecimale.
    pub other_wkb: String,
    /// Nome della colonna aggiunta; assente vale l'id dell'operazione senza
    /// `geo.`.
    pub output_column: Option<String>,
}

/// `split`: la lama dalla config, nello stesso CRS della colonna.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SplitConfig {
    /// La lama, WKB esadecimale: struttura, validita' OGC e coordinate nel
    /// dominio del CRS della colonna, verificate in analisi.
    pub other_wkb: String,
    /// Tolleranza del taglio delle sorgenti `LineString`, finita e non
    /// negativa; assente vale `0`. Sulle sorgenti poligonali non ha effetto.
    pub tolerance: Option<f64>,
}

/// `geo.sjoin`: join spaziale (kernel `spatial_join::spatial_join_nullable`).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SJoinConfig {
    /// Il predicato che una coppia (sinistra, destra) deve soddisfare;
    /// obbligatorio: `intersects`, `contains`, `within`, `crosses`,
    /// `overlaps`, `touches` ([`JoinPredicate`]).
    pub predicate: JoinPredicate,
}

/// `geo.nearest`: vicino piu' prossimo (kernel
/// `analysis::nearest_matches`).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NearestConfig {
    /// Distanza massima, nelle unita' del CRS: una riga sinistra il cui
    /// vicino e' piu' lontano non ha abbinamenti. Facoltativa (assente:
    /// nessun limite); finita e non negativa (`InvalidPlan` altrimenti).
    pub max_distance: Option<f64>,
}

/// `geo.overlay`: overlay poligonale con provenienza (kernel
/// `topology::polygon_overlay`).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OverlayConfig {
    /// Quali pezzi emettere; obbligatorio: `intersection`, `union`,
    /// `identity`, `symmetric_difference` ([`OverlayMode`]).
    pub mode: OverlayMode,
}

/// `from_wkt`: colonna `Utf8` con il testo WKT. `on_error` (default `null`)
/// si valida qui e non cambia nulla: entrambi i valori rifiutano la colonna
/// al primo WKT invalido.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FromWktConfig {
    /// La colonna `Utf8` da leggere; non vuota.
    pub wkt_column: String,
    /// Nome della colonna geometria creata; assente vale `geometry`.
    pub output_column: Option<String>,
    /// `null` o `fail`; assente vale `null`.
    pub on_error: Option<crate::extensions::OnWktError>,
    /// CRS della colonna creata; assente vale il CRS di piano.
    pub crs: Option<String>,
}

/// Campo accessorio richiedibile in `geometry_accessors.fields`.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum AccessorFieldParam {
    /// `geometry_type`: il nome del tipo (`Utf8`).
    GeometryType,
    /// `num_geometries`: le parti (`UInt64`).
    NumGeometries,
    /// `num_interior_rings`: gli anelli interni (`UInt64`).
    NumInteriorRings,
    /// `start_point`: il primo punto di una linea aperta, in WKT (`Utf8`).
    StartPoint,
    /// `end_point`: l'ultimo punto di una linea aperta, in WKT (`Utf8`).
    EndPoint,
    /// `is_closed`: se la geometria e' chiusa (`Boolean`).
    IsClosed,
}

impl AccessorFieldParam {
    /// Indice in [`super::ACCESSOR_COLUMNS`] (ordine canonico di output).
    #[must_use]
    pub const fn column_index(self) -> usize {
        match self {
            Self::GeometryType => 0,
            Self::NumGeometries => 1,
            Self::NumInteriorRings => 2,
            Self::StartPoint => 3,
            Self::EndPoint => 4,
            Self::IsClosed => 5,
        }
    }
}

/// `geometry_accessors`: entrambi i campi facoltativi.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GeometryAccessorsConfig {
    /// Colonne da aggiungere, non vuota e senza ripetizioni; assente vale
    /// tutte e sei. Escono sempre nell'ordine di
    /// [`super::ACCESSOR_COLUMNS`].
    pub fields: Option<Vec<AccessorFieldParam>>,
    /// Prefisso dei nomi delle colonne aggiunte; assente vale `""`.
    pub output_prefix: Option<String>,
}

/// `collect`: le chiavi di gruppo.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CollectConfig {
    /// Colonne dell'ingresso, non vuota, senza ripetizioni, senza la
    /// colonna geometria.
    pub group_by: Vec<String>,
}

/// `line_locate_point`: il punto dalla config, nello stesso CRS della
/// colonna.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LineLocatePointConfig {
    /// Il punto, WKB esadecimale di un `Point` valido nel dominio del CRS
    /// della colonna.
    pub point_wkb: String,
    /// Nome della colonna aggiunta; assente vale `fraction`.
    pub output_column: Option<String>,
}

/// Extent di `generate_grid`: finito e non degenere (dominio verificato dal
/// kernel [`crate::extensions2::GridExtent`]), con i quattro vertici nel
/// dominio del CRS della griglia.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GridExtentConfig {
    /// Ascissa minima.
    pub xmin: f64,
    /// Ordinata minima.
    pub ymin: f64,
    /// Ascissa massima, maggiore di `xmin`.
    pub xmax: f64,
    /// Ordinata massima, maggiore di `ymin`.
    pub ymax: f64,
}

/// `generate_grid` (generativa): `shape` default `square`,
/// `include_centroid` default false, CRS da `crs` o di piano.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GenerateGridConfig {
    /// Il rettangolo da coprire.
    pub extent: GridExtentConfig,
    /// Lato della cella, finito e positivo.
    pub cell_size: f64,
    /// `square` o `hex`; assente vale `square`.
    pub shape: Option<crate::extensions2::GridShape>,
    /// CRS della griglia; assente vale il CRS di piano.
    pub crs: Option<String>,
    /// Aggiunge `centroid_x` e `centroid_y`; assente vale `false`.
    pub include_centroid: Option<bool>,
}

/// `subdivide`: `output_column` rinomina la colonna geometria
/// (default: nome invariato, in place come `explode`).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubdivideConfig {
    /// Vertici massimi per parte, almeno
    /// [`crate::extensions2::MIN_SUBDIVIDE_VERTICES`].
    pub max_vertices: usize,
    /// Nuovo nome della colonna geometria; assente la lascia com'e'.
    pub output_column: Option<String>,
}

/// `snap`: riferimento WKB esadecimale dalla config, nello stesso CRS della
/// colonna, decodificato e validato (OGC e dominio del CRS) in analisi.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SnapConfig {
    /// Il riferimento: ne contano solo i vertici.
    pub reference_wkb: String,
    /// Distanza massima d'aggancio, finita e non negativa.
    pub tolerance: f64,
}

/// `coverage_validate`: tutti i campi opzionali; default kernel
/// (`tolerance` 0, `max_issues` [`crate::extensions3::DEFAULT_MAX_ISSUES`]).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CoverageValidateConfig {
    /// Area minima segnalata (strettamente maggiore), finita e non
    /// negativa.
    pub tolerance: Option<f64>,
    /// Sovrapposizioni massime, maggiore di zero: oltre, errore.
    pub max_issues: Option<usize>,
}

/// `shared_paths`: tutti i campi opzionali; default kernel
/// (`tolerance` 0, `min_length` 0).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SharedPathsConfig {
    /// Lunghezza sotto cui (compresa) un tratto condiviso si scarta,
    /// finita e non negativa.
    pub tolerance: Option<f64>,
    /// Lunghezza condivisa totale minima di una coppia, finita e non
    /// negativa.
    pub min_length: Option<f64>,
}

/// `cluster_dbscan`: `eps` e `min_points` obbligatori; `output_column`
/// opzionale (default [`super::CLUSTER_ID_COLUMN`]).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClusterDbscanConfig {
    /// Raggio del vicinato, finito e positivo.
    pub eps: f64,
    /// Punti minimi del vicinato di un core, almeno 1.
    pub min_points: usize,
    /// Nome della colonna dell'etichetta.
    pub output_column: Option<String>,
}
