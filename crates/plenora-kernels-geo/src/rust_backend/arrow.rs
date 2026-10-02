//! Esecuzione Arrow di `geo.make_valid`, `geo.polygonize` e `geo.split`
//! sopra il backend Rust.
//!
//! Riproduce la semantica del trasporto Arrow di
//! `plenora-data-tools@190c493` (`plenora-engine::geo_transport`, bracci
//! `MakeValid`, `polygonize_batches` e `PairOperation::Split`): stesse
//! colonne, stesse classi, stessi limiti, stesso trattamento dei null.
//! Engine, envelope e diagnostica per riga non sono in questo workspace: gli
//! errori tornano come [`PlenoraError`], il primo in ordine di riga.

use std::sync::Arc;

use geo::{CoordsIter, Geometry, LineString};
use plenora_core::arrow::array::builder::BinaryBuilder;
use plenora_core::arrow::array::{Array, ArrayRef, BinaryArray, StringArray, UInt64Array};
use plenora_core::arrow::select::{concat::concat, take::take};
use plenora_core::arrow::{DataType, Field, RecordBatch, Schema, SchemaRef};
use plenora_core::contract::arrow_metadata::{
    geometry_column_index, strip_rewritten_types_declarations, MAX_CELL_COORDINATES,
};
use plenora_core::PlenoraError;

use super::precision::Precision;
use super::{
    make_valid_wkb, polygonize_linework, residual_classes, split_polygon_by_linework, RepairMethod,
    MAX_CLEAN_VERTICES, MAX_NODING_WORK, MAX_SPLIT_WORK,
};
use crate::arrow_adapter::{
    batch_geometry_cells, decode_geometry_cell, encode_geometry, map_nullable,
};
use crate::extended_algorithms::{split_line, ExtendedAlgorithmError};

/// Colonna di classificazione dei pezzi di `polygonize`.
pub use crate::analyze::CLASS_COLUMN;
/// Colonna di lineage dei pezzi di `split`.
pub use crate::analyze::PARENT_INDEX_COLUMN;

/// Parametri di `geo.polygonize` (config del piano, entrambi facoltativi).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PolygonizeParams {
    /// Nodare l'input prima del grafo; assente vale `true`.
    pub node_input: Option<bool>,
    /// Fallire se restano residui; assente vale `false`.
    pub require_complete: Option<bool>,
}

/// `geo.make_valid` su una tabella.
///
/// Ogni cella non-null e' riparata con `LINEWORK` e `keep_collapsed = true`,
/// come nel trasporto GEOS; i null restano null, le celle gia' valide restano
/// byte per byte. Lo schema e' quello dell'ingresso salvo la dichiarazione
/// dei tipi geometrici del campo, che la riparazione riscrive (l'analisi
/// dichiara `mixed`; [`output_geometry_field`], oracolo
/// `analyze::tests::kernel_crosscheck`).
///
/// Le differenze di contenuto dal backend GEOS sono quelle di
/// [`make_valid_wkb`].
///
/// # Errors
///
/// `PlenoraError::Schema` se la colonna geometria manca o non e' WKB;
/// `PlenoraError::ResourceLimit` per una cella oltre il limite; l'errore
/// della prima cella che fallisce in ordine di riga, tradotto con
/// `From<RustBackendError>`.
pub fn make_valid_batches(
    schema: &SchemaRef,
    batches: &[RecordBatch],
    geometry_column: &str,
    precision: Precision,
) -> Result<Vec<RecordBatch>, PlenoraError> {
    let geometry_index = geometry_column_index(schema, geometry_column)?;
    let mut fields: Vec<Field> = schema
        .fields()
        .iter()
        .map(|field| field.as_ref().clone())
        .collect();
    let input_geometry = schema.field(geometry_index);
    fields[geometry_index] = output_geometry_field(input_geometry, input_geometry.is_nullable());
    let output_schema = Arc::new(Schema::new_with_metadata(fields, schema.metadata().clone()));
    let mut output = Vec::with_capacity(batches.len());
    for batch in batches {
        let cells = batch_geometry_cells(batch, geometry_index, geometry_column)?;
        let repaired = map_nullable(cells, |payload| {
            make_valid_wkb(payload, RepairMethod::Linework, true, precision)
                .map(Some)
                .map_err(PlenoraError::from)
        })?;
        let mut columns = batch.columns().to_vec();
        columns[geometry_index] = Arc::new(
            repaired
                .iter()
                .map(|cell| cell.as_deref())
                .collect::<BinaryArray>(),
        );
        output.push(plenora_core::batch_with_rows(
            output_schema.clone(),
            columns,
            batch.num_rows(),
        )?);
    }
    Ok(output)
}

/// `geo.polygonize` su una tabella.
///
/// Tutte le linee non-null di tutti i batch sono raccolte in una `GeometryCollection`, nodate (salvo
/// `node_input = false`) e poligonizzate. L'output ha una riga per poligono
/// e per residuo, nessun attributo propagato: la colonna geometria (il campo
/// dell'ingresso, [`output_geometry_field`]) e `__class` (`polygon`,
/// `cut_edge`, `dangle`, `invalid_ring`), in quest'ordine di classe.
///
/// Limiti come nel trasporto GEOS: [`MAX_CLEAN_VERTICES`] coordinate in
/// ingresso e in uscita, [`MAX_NODING_WORK`], `max_output_rows` geometrie.
/// Dentro ogni classe l'ordine e' quello del kernel Rust, non quello di
/// GEOS (vedi [`polygonize_linework`]). `precision` e' la precisione
/// dichiarata del noding (1 cm a terra, [`Precision::from_crs`]), argomento
/// in piu' rispetto a 190c493; `output_crs` di 190c493 non c'e' piu': il CRS
/// e' quello dell'ingresso, portato dal campo geometria.
///
/// # Errors
///
/// `PlenoraError::Schema` se la colonna geometria manca o non e' WKB;
/// `PlenoraError::ResourceLimit` per una cella oltre il limite; gli errori
/// di decode delle celle e del backend, tradotti.
pub fn polygonize_batches(
    schema: &SchemaRef,
    batches: &[RecordBatch],
    geometry_column: &str,
    params: PolygonizeParams,
    max_output_rows: u64,
    precision: Precision,
) -> Result<(SchemaRef, Vec<RecordBatch>), PlenoraError> {
    let geometry_index = geometry_column_index(schema, geometry_column)?;
    let mut lines = Vec::new();
    for batch in batches {
        let cells = batch_geometry_cells(batch, geometry_index, geometry_column)?;
        for row in 0..cells.len() {
            if cells.is_null(row) {
                continue;
            }
            lines.push(decode_geometry_cell(cells.value(row))?);
        }
    }
    let linework = Geometry::GeometryCollection(lines.into());
    let result = polygonize_linework(
        &linework,
        params.node_input.unwrap_or(true),
        params.require_complete.unwrap_or(false),
        MAX_CLEAN_VERTICES,
        MAX_NODING_WORK,
        max_output_rows,
        MAX_CLEAN_VERTICES,
        precision,
    )?;
    let mut geometries: Vec<Vec<u8>> = Vec::new();
    let mut classes: Vec<&'static str> = Vec::new();
    for polygon in &result.polygons {
        geometries.push(encode_geometry(&Geometry::Polygon(polygon.clone()))?);
        classes.push("polygon");
    }
    for (class, lines) in residual_classes(&result) {
        for line in lines {
            geometries.push(encode_geometry(&Geometry::LineString(line.clone()))?);
            classes.push(class);
        }
    }
    let output_schema = Arc::new(Schema::new_with_metadata(
        vec![
            output_geometry_field(schema.field(geometry_index), false),
            Field::new(CLASS_COLUMN, DataType::Utf8, false),
        ],
        schema.metadata().clone(),
    ));
    let columns: Vec<ArrayRef> = vec![
        Arc::new(
            geometries
                .iter()
                .map(|cell| Some(cell.as_slice()))
                .collect::<BinaryArray>(),
        ),
        Arc::new(StringArray::from(classes)),
    ];
    let batch = RecordBatch::try_new(output_schema.clone(), columns)?;
    Ok((output_schema, vec![batch]))
}

/// Il campo geometria dell'uscita di `make_valid`, `polygonize` e `split`: quello
/// dell'ingresso, con tutti i suoi metadati (CRS, dimensioni,
/// encoding e lineage passano invariati), senza la dichiarazione dei tipi
/// geometrici, che l'operazione riscrive (l'analisi la ridichiara nel
/// contratto, `analyze::tipi`), e con la nullability del contratto: non
/// nullable per `polygonize` e `split` (una riga per geometria prodotta; una
/// sorgente null non produce righe), quella dell'ingresso per `make_valid`.
///
/// E' lo schema che l'analisi dichiara (oracolo
/// `analyze::tests::kernel_crosscheck`); a 190c493 il campo nasceva da
/// `geometry_output_field` e perdeva i metadati dell'ingresso.
fn output_geometry_field(input: &Field, nullable: bool) -> Field {
    let mut metadata = input.metadata().clone();
    strip_rewritten_types_declarations(&mut metadata);
    Field::new(input.name(), DataType::Binary, nullable).with_metadata(metadata)
}

/// L'errore dello split lineare nella lingua del passo: interno cio' che
/// non ha concluso, `ResourceLimit` un limite superato (come
/// `From<RustBackendError>` e il runner), `InvalidPlan` il resto.
fn errore_dello_split_lineare(error: ExtendedAlgorithmError) -> PlenoraError {
    match error {
        ExtendedAlgorithmError::ValidazioneNonConclusa(_)
        | ExtendedAlgorithmError::Internal(_)
        | ExtendedAlgorithmError::CalcoloNonConcluso(_) => {
            PlenoraError::Internal(error.to_string())
        }
        ExtendedAlgorithmError::CoordinateLimit { .. }
        | ExtendedAlgorithmError::OutputLimit { .. }
        | ExtendedAlgorithmError::WorkLimit { .. } => {
            PlenoraError::ResourceLimit(error.to_string())
        }
        other => PlenoraError::InvalidPlan(other.to_string()),
    }
}

/// `geo.split` su una tabella.
///
/// La riga `i` della sorgente (colonna
/// `geometry_column` di `left_batches`, concatenati) e' divisa dalla riga `i`
/// di `splitters`. Righe con uno dei due null sono saltate. Una sorgente
/// `LineString` passa da `split_line` (Rust puro gia' in GEOS-era, con
/// `tolerance`); una `Polygon`/`MultiPolygon` da
/// [`split_polygon_by_linework`] con [`MAX_CELL_COORDINATES`],
/// [`MAX_NODING_WORK`] e `max_output_rows`. Ogni parte diventa una riga con
/// gli attributi della sorgente, la geometria riscritta (nel campo
/// dell'ingresso, [`output_geometry_field`]) e `__parent_index` (`UInt64`,
/// indice della riga sorgente).
///
/// L'ordine delle righe e' quello delle sorgenti; dentro una sorgente
/// poligonale e' quello delle facce del kernel Rust, non quello di GEOS.
///
/// # Errors
///
/// `PlenoraError::InvalidPlan` se le righe dei due lati non coincidono, se
/// una sorgente non e' `LineString`/`Polygon`/`MultiPolygon`, o se le parti
/// superano `max_output_rows`; gli errori di decode e dei kernel, tradotti.
// Gli argomenti sono quelli del trasporto di 190c493 piu' la precisione
// dichiarata e meno `output_crs`: raggrupparli cambierebbe la forma del
// contratto.
#[allow(clippy::too_many_arguments)]
pub fn split_batches(
    left_schema: &SchemaRef,
    left_batches: &[RecordBatch],
    geometry_column: &str,
    splitters: &BinaryArray,
    tolerance: Option<f64>,
    max_output_rows: u64,
    precision: Precision,
) -> Result<(SchemaRef, Vec<RecordBatch>), PlenoraError> {
    let geometry_index = geometry_column_index(left_schema, geometry_column)?;
    let left_rows = left_batches
        .iter()
        .map(RecordBatch::num_rows)
        .sum::<usize>();
    if left_rows != splitters.len() {
        return Err(PlenoraError::InvalidPlan(
            "split: righe della sorgente e dello splitter non allineate".to_owned(),
        ));
    }
    let tolerance = tolerance.unwrap_or(0.0);
    let mut pieces = SplitPieces::default();
    let mut row = 0_usize;
    for batch in left_batches {
        let cells = batch_geometry_cells(batch, geometry_index, geometry_column)?;
        for local in 0..cells.len() {
            let index = row;
            row += 1;
            // Ogni cella non-null dei due lati si valida, anche se l'altro
            // lato e' null e la coppia non produce righe (come
            // `decode_geometry_side` a 190c493): prima la sorgente, poi lo
            // splitter, in ordine di riga.
            let source = (!cells.is_null(local))
                .then(|| decode_geometry_cell(cells.value(local)))
                .transpose()?;
            let splitter = (!splitters.is_null(index))
                .then(|| decode_geometry_cell(splitters.value(index)))
                .transpose()?;
            let (Some(source), Some(splitter)) = (source, splitter) else {
                continue;
            };
            let parts = split_row(&source, &splitter, tolerance, max_output_rows, precision)?;
            pieces.push(index, &parts, max_output_rows)?;
        }
    }
    split_output(left_schema, left_batches, geometry_index, pieces)
}

/// Le parti gia' codificate e la riga sorgente di ciascuna. Le parti vanno
/// subito nel buffer della colonna d'uscita, contiguo: niente `Vec` per
/// parte e niente seconda copia alla fine (su milioni di parti il conto
/// separato costava centinaia di MB in piu').
struct SplitPieces {
    encoded: BinaryBuilder,
    parents: Vec<u64>,
}

impl Default for SplitPieces {
    fn default() -> Self {
        Self {
            encoded: BinaryBuilder::new(),
            parents: Vec::new(),
        }
    }
}

impl SplitPieces {
    /// Aggiunge le parti di una riga, con il limite `max_output_rows` sul
    /// totale gia' prodotto.
    fn push(
        &mut self,
        index: usize,
        parts: &[Geometry<f64>],
        max_output_rows: u64,
    ) -> Result<(), PlenoraError> {
        let non_rappresentabile =
            || PlenoraError::Internal("split: conteggio parti non rappresentabile".to_owned());
        let count = u64::try_from(parts.len()).map_err(|_| non_rappresentabile())?;
        let total = u64::try_from(self.parents.len()).map_err(|_| non_rappresentabile())?;
        let next = total.checked_add(count).ok_or_else(non_rappresentabile)?;
        if next > max_output_rows {
            return Err(PlenoraError::InvalidPlan(format!(
                "righe di output {next} oltre il limite max_output_rows {max_output_rows}"
            )));
        }
        let parent = u64::try_from(index).map_err(|_| {
            PlenoraError::Internal("split: indice di riga non rappresentabile".to_owned())
        })?;
        for part in parts {
            let wkb = encode_geometry(part)?;
            // Gli offset della colonna `Binary` sono `i32`: oltre, un errore
            // esplicito invece del panico del builder.
            if self.encoded.values_slice().len().saturating_add(wkb.len())
                > usize::try_from(i32::MAX).unwrap_or(usize::MAX)
            {
                return Err(PlenoraError::ResourceLimit(
                    "split: parti codificate oltre i 2 GiB di una colonna Binary".to_owned(),
                ));
            }
            self.encoded.append_value(&wkb);
            self.parents.push(parent);
        }
        Ok(())
    }
}

/// Lo split lineare di una sorgente `LineString`.
fn split_linestring(
    line: &LineString<f64>,
    splitter: &Geometry<f64>,
    tolerance: f64,
    max_output_rows: u64,
) -> Result<Vec<Geometry<f64>>, PlenoraError> {
    Ok(split_line(
        line,
        splitter,
        tolerance,
        MAX_CELL_COORDINATES,
        MAX_SPLIT_WORK,
        max_output_rows,
        MAX_CELL_COORDINATES,
    )
    .map_err(errore_dello_split_lineare)?
    .into_iter()
    .map(Geometry::LineString)
    .collect())
}

/// Le parti di una riga: split lineare per `LineString`, backend Rust per le
/// sorgenti poligonali, rifiuto per ogni altro tipo.
fn split_row(
    source: &Geometry<f64>,
    splitter: &Geometry<f64>,
    tolerance: f64,
    max_output_rows: u64,
    precision: Precision,
) -> Result<Vec<Geometry<f64>>, PlenoraError> {
    match source {
        Geometry::LineString(line) => {
            // `split_line` (codice precedente al porting) ammette un punto
            // di taglio entro `tolerance` piu' un margine numerico di `16 *
            // EPSILON` per il modulo delle coordinate: a `2^48` circa 1 m.
            // Con la spaziatura dei `f64` entro `p / 64` il margine resta
            // sotto `p / 2`, e un punto a piu' di `p` dalla linea (con
            // tolleranza nulla) non taglia; oltre, nessun calcolo.
            let magnitude = super::precision::modulo_massimo(
                line.0.iter().copied().chain(splitter.coords_iter()),
            );
            if !super::precision::coordinate_abbastanza_fitte(magnitude, precision.value()) {
                return Err(super::RustBackendError::PrecisionInsufficient.into());
            }
            split_linestring(line, splitter, tolerance, max_output_rows)
        }
        Geometry::Polygon(_) | Geometry::MultiPolygon(_) => Ok(split_polygon_by_linework(
            source,
            splitter,
            MAX_CELL_COORDINATES,
            MAX_NODING_WORK,
            max_output_rows,
            MAX_CELL_COORDINATES,
            precision,
        )?
        .into_iter()
        .map(Geometry::Polygon)
        .collect()),
        // Il tipo della cella non entra nel messaggio («errori senza dati»).
        _ => Err(PlenoraError::InvalidPlan(
            "split: attesa geometria LineString o Polygon/MultiPolygon, la cella ne ha un altro tipo"
                .to_owned(),
        )),
    }
}

/// Il batch di output di `split`: attributi della sorgente ripetuti per
/// parte, geometria riscritta, `__parent_index` in coda.
fn split_output(
    left_schema: &SchemaRef,
    left_batches: &[RecordBatch],
    geometry_index: usize,
    pieces: SplitPieces,
) -> Result<(SchemaRef, Vec<RecordBatch>), PlenoraError> {
    let SplitPieces {
        mut encoded,
        parents,
    } = pieces;
    let geometrie: ArrayRef = Arc::new(encoded.finish());
    let mut output_fields: Vec<Field> = left_schema
        .fields()
        .iter()
        .map(|field| field.as_ref().clone())
        .collect();
    let input_geometry = left_schema.field(geometry_index);
    output_fields[geometry_index] = output_geometry_field(input_geometry, false);
    output_fields.push(Field::new(PARENT_INDEX_COLUMN, DataType::UInt64, false));
    let output_schema = Arc::new(Schema::new_with_metadata(
        output_fields,
        left_schema.metadata().clone(),
    ));
    let take_indices = UInt64Array::from(parents.clone());
    let mut columns: Vec<ArrayRef> = Vec::with_capacity(left_schema.fields().len() + 1);
    for index in 0..left_schema.fields().len() {
        if index == geometry_index {
            columns.push(geometrie.clone());
        } else {
            let parts = left_batches
                .iter()
                .map(|batch| batch.column(index).as_ref())
                .collect::<Vec<_>>();
            let column = if parts.is_empty() {
                plenora_core::arrow::array::new_empty_array(left_schema.field(index).data_type())
            } else {
                concat(&parts)?
            };
            columns.push(take(&column, &take_indices, None)?);
        }
    }
    let parents_len = parents.len();
    columns.push(Arc::new(UInt64Array::from(parents)));
    let batch = plenora_core::batch_with_rows(output_schema.clone(), columns, parents_len)?;
    Ok((output_schema, vec![batch]))
}

// I test del trasporto Arrow di `plenora-data-tools@190c493`
// (`plenora-engine/src/geo_transport/transport.rs`) per le tre operazioni,
// senza envelope: stesse fixture, stesse attese.
#[cfg(test)]
mod tests {
    use super::*;

    /// Revisione, sesto giro: a `2^48` il margine numerico di `split_line`
    /// valeva circa 1 m, e un punto a 25 cm dalla linea tagliava con
    /// tolleranza nulla. Ora la spaziatura delle coordinate si confronta con
    /// la precisione: errore esplicito. Vicino all'origine il punto a 25 cm
    /// non taglia, quello sulla linea si'.
    #[test]
    fn split_lineare_rispetta_la_precisione() {
        let centimetro = Precision::new(0.01).unwrap();
        let caso = |base: f64, scarto: f64| {
            split_row(
                &Geometry::LineString(LineString::from(vec![(base, 0.0), (base + 10.0, 0.0)])),
                &Geometry::Point(geo::Point::new(base + 5.0, scarto)),
                0.0,
                16,
                centimetro,
            )
        };
        assert!(matches!(
            caso(2_f64.powi(48), 0.25),
            Err(PlenoraError::Unsupported(_))
        ));
        assert_eq!(caso(0.0, 0.25).unwrap().len(), 1);
        assert_eq!(caso(0.0, 0.0).unwrap().len(), 2);
        assert_eq!(caso(2_f64.powi(30), 0.25).unwrap().len(), 1);
    }

    /// Precisione dei test: coordinate astratte fino a qualche decina di
    /// unita', un milionesimo di unita' (la griglia degli overlay resta sotto).
    fn precisione() -> Precision {
        Precision::new(1e-6).unwrap()
    }
    use crate::geometry_from_wkb;
    use crate::test_support::{linestring_wkb_le, point_wkb_le, polygon_wkb_le};
    use geo::Area;
    use plenora_core::arrow::array::Int64Array;
    use plenora_core::contract::arrow_metadata::{geometry_output_field, DEFAULT_GEOMETRY_COLUMN};

    const CRS: &str = "EPSG:3857";

    fn square_wkb(size: f64) -> Vec<u8> {
        polygon_wkb_le(&[
            (0.0, 0.0),
            (size, 0.0),
            (size, size),
            (0.0, size),
            (0.0, 0.0),
        ])
    }

    fn fixture_batch(cells: &[Option<&Vec<u8>>]) -> (SchemaRef, RecordBatch) {
        let schema = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int64, false),
            geometry_output_field(DEFAULT_GEOMETRY_COLUMN, CRS).expect("campo geometria"),
        ]));
        let ids = (0..cells.len())
            .map(|row| i64::try_from(row).unwrap())
            .collect::<Vec<_>>();
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(Int64Array::from(ids)),
                Arc::new(
                    cells
                        .iter()
                        .map(|cell| cell.map(Vec::as_slice))
                        .collect::<BinaryArray>(),
                ),
            ],
        )
        .expect("batch");
        (schema, batch)
    }

    fn column<'a, T: 'static>(batch: &'a RecordBatch, schema: &SchemaRef, name: &str) -> &'a T {
        batch
            .column(schema.index_of(name).unwrap())
            .as_any()
            .downcast_ref::<T>()
            .unwrap()
    }

    #[test]
    fn make_valid_repairs_bowtie_and_preserves_valid_geometries() {
        let bowtie = polygon_wkb_le(&[(0.0, 0.0), (2.0, 2.0), (0.0, 2.0), (2.0, 0.0), (0.0, 0.0)]);
        assert!(geometry_from_wkb(&bowtie).is_err());
        let square = square_wkb(2.0);
        let (schema, batch) = fixture_batch(&[Some(&bowtie), None, Some(&square)]);
        let output = make_valid_batches(&schema, &[batch], DEFAULT_GEOMETRY_COLUMN, precisione())
            .expect("make_valid");
        let cells = column::<BinaryArray>(&output[0], &schema, DEFAULT_GEOMETRY_COLUMN);
        let repaired = geometry_from_wkb(cells.value(0)).expect("riparata");
        assert!((repaired.unsigned_area() - 2.0).abs() < 1e-12);
        assert!(cells.is_null(1));
        assert_eq!(cells.value(2), square.as_slice());
        assert_eq!(output[0].schema(), schema, "schema invariato");
    }

    #[test]
    fn polygonize_classifies_faces_and_residuals() {
        // quadrato chiuso + dangle: attesi un poligono e un dangle.
        let ring = linestring_wkb_le(&[(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0), (0.0, 0.0)]);
        let tail = linestring_wkb_le(&[(1.0, 1.0), (3.0, 3.0)]);
        let mut collection = vec![1_u8, 7, 0, 0, 0, 2, 0, 0, 0];
        collection.extend_from_slice(&ring);
        collection.extend_from_slice(&tail);
        let (schema, batch) = fixture_batch(&[Some(&collection)]);
        let (out_schema, batches) = polygonize_batches(
            &schema,
            &[batch],
            DEFAULT_GEOMETRY_COLUMN,
            PolygonizeParams::default(),
            16,
            precisione(),
        )
        .expect("polygonize");
        let classes = column::<StringArray>(&batches[0], &out_schema, CLASS_COLUMN);
        let classes: Vec<&str> = (0..batches[0].num_rows())
            .map(|row| classes.value(row))
            .collect();
        assert_eq!(classes, ["polygon", "dangle"]);
        assert_eq!(out_schema.fields().len(), 2, "nessun attributo propagato");
    }

    #[test]
    fn polygonize_honours_require_complete_and_the_row_limit() {
        let ring = linestring_wkb_le(&[(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0), (0.0, 0.0)]);
        let tail = linestring_wkb_le(&[(1.0, 1.0), (3.0, 3.0)]);
        let (schema, batch) = fixture_batch(&[Some(&ring), None, Some(&tail)]);
        let complete = PolygonizeParams {
            node_input: None,
            require_complete: Some(true),
        };
        assert!(matches!(
            polygonize_batches(
                &schema,
                std::slice::from_ref(&batch),
                DEFAULT_GEOMETRY_COLUMN,
                complete,
                16,
                precisione()
            ),
            Err(PlenoraError::InvalidPlan(_))
        ));
        assert!(matches!(
            polygonize_batches(
                &schema,
                &[batch],
                DEFAULT_GEOMETRY_COLUMN,
                PolygonizeParams::default(),
                1,
                precisione()
            ),
            Err(PlenoraError::ResourceLimit(_))
        ));
    }

    #[test]
    fn split_produces_pieces_with_lineage_and_conserves_measures() {
        let line = linestring_wkb_le(&[(0.0, 0.0), (10.0, 0.0)]);
        let cutter = point_wkb_le(5.0, 0.0);
        let (schema, batch) = fixture_batch(&[Some(&line), None]);
        let splitters = [Some(cutter.as_slice()), Some(cutter.as_slice())]
            .into_iter()
            .collect::<BinaryArray>();
        let (out_schema, batches) = split_batches(
            &schema,
            &[batch],
            DEFAULT_GEOMETRY_COLUMN,
            &splitters,
            None,
            16,
            precisione(),
        )
        .expect("split lineare");
        assert_eq!(batches[0].num_rows(), 2);
        let parents = column::<UInt64Array>(&batches[0], &out_schema, PARENT_INDEX_COLUMN);
        assert_eq!(parents.values(), &[0, 0]);
        let ids = column::<Int64Array>(&batches[0], &out_schema, "id");
        assert_eq!(ids.values(), &[0, 0]);
        let cells = column::<BinaryArray>(&batches[0], &out_schema, DEFAULT_GEOMETRY_COLUMN);
        let total: f64 = (0..2)
            .map(|row| match geometry_from_wkb(cells.value(row)).unwrap() {
                Geometry::LineString(piece) => geo::algorithm::line_measures::Length::length(
                    &geo::algorithm::line_measures::Euclidean,
                    &piece,
                ),
                other => panic!("atteso LineString: {other:?}"),
            })
            .sum();
        assert!((total - 10.0).abs() < 1e-9);

        // split poligonale: quadrato tagliato da una retta verticale.
        let square = square_wkb(2.0);
        let blade = linestring_wkb_le(&[(1.0, -1.0), (1.0, 3.0)]);
        let (schema, batch) = fixture_batch(&[Some(&square)]);
        let splitters = std::iter::once(Some(blade.as_slice())).collect::<BinaryArray>();
        let (out_schema, batches) = split_batches(
            &schema,
            std::slice::from_ref(&batch),
            DEFAULT_GEOMETRY_COLUMN,
            &splitters,
            None,
            16,
            precisione(),
        )
        .expect("split poligonale");
        assert_eq!(batches[0].num_rows(), 2);
        let cells = column::<BinaryArray>(&batches[0], &out_schema, DEFAULT_GEOMETRY_COLUMN);
        let total: f64 = (0..2)
            .map(|row| geometry_from_wkb(cells.value(row)).unwrap().unsigned_area())
            .sum();
        assert!((total - 4.0).abs() < 1e-9);

        // tipo sorgente non supportato.
        let point = point_wkb_le(0.0, 0.0);
        let (schema, bad) = fixture_batch(&[Some(&point)]);
        assert!(matches!(
            split_batches(
                &schema,
                &[bad],
                DEFAULT_GEOMETRY_COLUMN,
                &splitters,
                None,
                16,
                precisione()
            ),
            Err(PlenoraError::InvalidPlan(_))
        ));

        // righe non allineate.
        let (schema, batch) = fixture_batch(&[Some(&square), Some(&square)]);
        assert!(matches!(
            split_batches(
                &schema,
                &[batch],
                DEFAULT_GEOMETRY_COLUMN,
                &splitters,
                None,
                16,
                precisione()
            ),
            Err(PlenoraError::InvalidPlan(_))
        ));
    }

    /// Come `decode_geometry_side` a 190c493: ogni cella non-null dei due
    /// lati e' validata anche quando la coppia non produce righe perche'
    /// l'altro lato e' null.
    #[test]
    fn split_validates_both_sides_even_when_the_pair_is_skipped() {
        let square = square_wkb(2.0);
        let malformed = vec![1_u8, 2, 3];
        let bowtie = polygon_wkb_le(&[(0.0, 0.0), (2.0, 2.0), (0.0, 2.0), (2.0, 0.0), (0.0, 0.0)]);
        let blade = linestring_wkb_le(&[(1.0, -1.0), (1.0, 3.0)]);
        // Sorgente null, splitter malformato o invalido.
        for bad in [&malformed, &bowtie] {
            let (schema, batch) = fixture_batch(&[None]);
            let splitters = std::iter::once(Some(bad.as_slice())).collect::<BinaryArray>();
            assert!(
                split_batches(
                    &schema,
                    &[batch],
                    DEFAULT_GEOMETRY_COLUMN,
                    &splitters,
                    None,
                    16,
                    precisione()
                )
                .is_err(),
                "splitter non valido accettato accanto a una sorgente null"
            );
        }
        // Splitter null, sorgente malformata o invalida.
        for bad in [&malformed, &bowtie] {
            let (schema, batch) = fixture_batch(&[Some(bad)]);
            let splitters = std::iter::once(None::<&[u8]>).collect::<BinaryArray>();
            assert!(
                split_batches(
                    &schema,
                    &[batch],
                    DEFAULT_GEOMETRY_COLUMN,
                    &splitters,
                    None,
                    16,
                    precisione()
                )
                .is_err(),
                "sorgente non valida accettata accanto a uno splitter null"
            );
        }
        // Coppie valide con un lato null: nessuna riga, nessun errore.
        let (schema, batch) = fixture_batch(&[Some(&square), None]);
        let splitters = [None, Some(blade.as_slice())]
            .into_iter()
            .collect::<BinaryArray>();
        let (_, batches) = split_batches(
            &schema,
            &[batch],
            DEFAULT_GEOMETRY_COLUMN,
            &splitters,
            None,
            16,
            precisione(),
        )
        .expect("lati validi");
        assert_eq!(batches[0].num_rows(), 0);
    }

    #[test]
    fn split_enforces_max_output_rows_across_rows() {
        // La lama va da bordo a bordo: nessun dangle, due facce, nessun
        // residuo. Ogni sorgente da sola sta nel limite 3 (anche nel budget
        // del polygonize, che conta pure i residui scartati); solo il totale
        // cumulato, 4, lo supera.
        let square = square_wkb(2.0);
        let blade = linestring_wkb_le(&[(1.0, 0.0), (1.0, 2.0)]);
        let one = |rows: usize| {
            let cells = vec![Some(&square); rows];
            let (schema, batch) = fixture_batch(&cells);
            let splitters =
                std::iter::repeat_n(Some(blade.as_slice()), rows).collect::<BinaryArray>();
            split_batches(
                &schema,
                &[batch],
                DEFAULT_GEOMETRY_COLUMN,
                &splitters,
                None,
                3,
                precisione(),
            )
        };
        let single = one(1).expect("una sorgente sta nel limite");
        assert_eq!(single.1[0].num_rows(), 2);
        let message = match one(2) {
            Err(PlenoraError::InvalidPlan(message)) => message,
            other => panic!("atteso il limite cumulato: {other:?}"),
        };
        assert!(
            message.contains("max_output_rows"),
            "errore del limite cumulato di split_batches: {message}"
        );
    }
}
