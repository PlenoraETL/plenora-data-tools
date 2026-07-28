use std::cell::Cell;
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{BufReader, BufWriter, ErrorKind, Read, Write};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use plenora_core::arrow::array::{Array, RecordBatch, UInt64Array};
use plenora_core::arrow::ipc::reader::StreamReader;
use plenora_core::arrow::ipc::writer::StreamWriter;
use plenora_core::arrow::schema::DataType;
use plenora_core::arrow::select::concat::concat_batches;
use sha2::{Digest, Sha256};
use tempfile::TempDir;

use crate::aggregation::{self, Aggregate, Distinct, Keep, KeyColumn, KeyHasher, Sort};
use crate::Limits;
use plenora_core::{PlenoraError, Result};
use crate::{column_index, replace_or_append, select_rows};
use crate::setops::{self, CompactRowEncoder};

const RECORD_OVERHEAD_ESTIMATE: usize = 64;

struct SpillWorkspace {
    directory: TempDir,
    bytes_written: u64,
    max_temp_bytes: u64,
}

impl SpillWorkspace {
    fn new(max_temp_bytes: u64) -> Result<Self> {
        Ok(Self {
            directory: tempfile::Builder::new()
                .prefix("plenora-nogeo-spill-")
                .tempdir()?,
            bytes_written: 0,
            max_temp_bytes,
        })
    }

    fn paths(&self, prefix: &str, partitions: usize) -> Vec<PathBuf> {
        (0..partitions)
            .map(|index| {
                self.directory
                    .path()
                    .join(format!("{prefix}-{index:04}.bin"))
            })
            .collect()
    }

    fn account(&mut self, bytes: usize) -> Result<()> {
        let bytes = bytes as u64;
        self.bytes_written = self
            .bytes_written
            .checked_add(bytes)
            .ok_or_else(|| PlenoraError::Contract("overflow quota spill".into()))?;
        if self.bytes_written > self.max_temp_bytes {
            return Err(PlenoraError::Contract(format!(
                "spill oltre max_temp_bytes: {} > {}",
                self.bytes_written, self.max_temp_bytes
            )));
        }
        Ok(())
    }
}

fn partition(key: &[u8], partitions: usize) -> Result<usize> {
    if partitions == 0 {
        return Err(PlenoraError::Contract(
            "spill richiede almeno una partizione".into(),
        ));
    }
    let digest = Sha256::digest(key);
    let mut prefix = [0_u8; 8];
    prefix.copy_from_slice(&digest[..8]);
    let hash = u64::from_be_bytes(prefix);
    let divisor = partitions as u64;
    usize::try_from(hash % divisor)
        .map_err(|_| PlenoraError::Contract("indice partizione non rappresentabile".into()))
}

fn open_writers(paths: &[PathBuf]) -> Result<Vec<BufWriter<File>>> {
    paths
        .iter()
        .map(|path| {
            File::create(path)
                .map(BufWriter::new)
                .map_err(PlenoraError::from)
        })
        .collect()
}

fn close_writers(mut writers: Vec<BufWriter<File>>) -> Result<()> {
    writers.iter_mut().try_for_each(Write::flush)?;
    Ok(())
}

fn max_record_bytes(limits: &Limits) -> usize {
    usize::try_from(limits.max_temp_bytes).unwrap_or(usize::MAX)
}

fn write_record(
    writer: &mut BufWriter<File>,
    ordinal: usize,
    key: &[u8],
    workspace: &mut SpillWorkspace,
) -> Result<()> {
    let ordinal = ordinal as u64;
    let length = key.len() as u64;
    workspace.account(16_usize.saturating_add(key.len()))?;
    writer.write_all(&ordinal.to_be_bytes())?;
    writer.write_all(&length.to_be_bytes())?;
    writer.write_all(key)?;
    Ok(())
}

fn spill_batch(
    batch: &RecordBatch,
    ordinal_offset: usize,
    writers: &mut [BufWriter<File>],
    workspace: &mut SpillWorkspace,
    limits: &Limits,
) -> Result<()> {
    let encoder = CompactRowEncoder::try_new(batch)?;
    let mut key = Vec::new();
    for row in 0..batch.num_rows() {
        encoder.encode_into(row, &mut key)?;
        if key.len() > max_record_bytes(limits) {
            return Err(PlenoraError::Contract(
                "singola chiave oltre max_temp_bytes".into(),
            ));
        }
        let index = partition(&key, writers.len())?;
        let ordinal = ordinal_offset
            .checked_add(row)
            .ok_or_else(|| PlenoraError::Contract("overflow ordinal spill".into()))?;
        write_record(&mut writers[index], ordinal, &key, workspace)?;
    }
    Ok(())
}

fn read_u64(reader: &mut BufReader<File>) -> Result<Option<u64>> {
    let mut bytes = [0_u8; 8];
    match reader.read(&mut bytes)? {
        0 => Ok(None),
        count => {
            reader.read_exact(&mut bytes[count..]).map_err(|error| {
                if error.kind() == ErrorKind::UnexpectedEof {
                    PlenoraError::Contract("record spill troncato".into())
                } else {
                    PlenoraError::Io(error)
                }
            })?;
            Ok(Some(u64::from_be_bytes(bytes)))
        }
    }
}

fn read_record(
    reader: &mut BufReader<File>,
    max_record_bytes: usize,
) -> Result<Option<(usize, Vec<u8>)>> {
    let Some(ordinal) = read_u64(reader)? else {
        return Ok(None);
    };
    let length = read_u64(reader)?
        .ok_or_else(|| PlenoraError::Contract("record spill senza lunghezza".into()))?;
    let length = usize::try_from(length)
        .map_err(|_| PlenoraError::Contract("record spill non rappresentabile".into()))?;
    if length > max_record_bytes {
        return Err(PlenoraError::Contract(
            "record spill oltre il limite di sicurezza".into(),
        ));
    }
    let mut key = vec![0_u8; length];
    reader.read_exact(&mut key).map_err(|error| {
        if error.kind() == ErrorKind::UnexpectedEof {
            PlenoraError::Contract("chiave spill troncata".into())
        } else {
            PlenoraError::Io(error)
        }
    })?;
    Ok(Some((
        usize::try_from(ordinal)
            .map_err(|_| PlenoraError::Contract("ordinal spill non rappresentabile".into()))?,
        key,
    )))
}

fn load_key_set(path: &PathBuf, limits: &Limits) -> Result<HashSet<Box<[u8]>>> {
    let mut reader = BufReader::new(File::open(path)?);
    let mut keys = HashSet::new();
    let mut estimated = 0_usize;
    while let Some((_, key)) = read_record(&mut reader, max_record_bytes(limits))? {
        if !keys.contains(key.as_slice()) {
            estimated = estimated
                .checked_add(key.len().saturating_add(RECORD_OVERHEAD_ESTIMATE))
                .ok_or_else(|| PlenoraError::Contract("overflow memoria spill".into()))?;
            if estimated > limits.max_memory_bytes {
                return Err(PlenoraError::Contract(format!(
                    "partizione spill oltre max_memory_bytes: {estimated} > {}",
                    limits.max_memory_bytes
                )));
            }
            keys.insert(key.into_boxed_slice());
        }
    }
    Ok(keys)
}

fn collect_distinct(path: &PathBuf, limits: &Limits, output: &mut Vec<usize>) -> Result<()> {
    let mut reader = BufReader::new(File::open(path)?);
    let mut emitted: HashSet<Box<[u8]>> = HashSet::new();
    let mut estimated = 0_usize;
    while let Some((ordinal, key)) = read_record(&mut reader, max_record_bytes(limits))? {
        if !emitted.contains(key.as_slice()) {
            estimated = estimated
                .checked_add(key.len().saturating_add(RECORD_OVERHEAD_ESTIMATE))
                .ok_or_else(|| PlenoraError::Contract("overflow memoria spill".into()))?;
            if estimated > limits.max_memory_bytes {
                return Err(PlenoraError::Contract(
                    "partizione union spill oltre max_memory_bytes".into(),
                ));
            }
            emitted.insert(key.into_boxed_slice());
            output.push(ordinal);
        }
    }
    Ok(())
}

fn collect_membership(
    left_path: &PathBuf,
    right_path: &PathBuf,
    limits: &Limits,
    intersect: bool,
    output: &mut Vec<usize>,
) -> Result<()> {
    let mut right = load_key_set(right_path, limits)?;
    let mut emitted = HashSet::<Box<[u8]>>::new();
    let mut emitted_bytes = 0_usize;
    let mut reader = BufReader::new(File::open(left_path)?);
    while let Some((ordinal, key)) = read_record(&mut reader, max_record_bytes(limits))? {
        if intersect {
            if right.remove(key.as_slice()) {
                output.push(ordinal);
            }
        } else if !right.contains(key.as_slice()) && !emitted.contains(key.as_slice()) {
            emitted_bytes = emitted_bytes
                .checked_add(key.len().saturating_add(RECORD_OVERHEAD_ESTIMATE))
                .ok_or_else(|| PlenoraError::Contract("overflow memoria except spill".into()))?;
            if emitted_bytes > limits.max_memory_bytes {
                return Err(PlenoraError::Contract(
                    "partizione except spill oltre max_memory_bytes".into(),
                ));
            }
            emitted.insert(key.into_boxed_slice());
            output.push(ordinal);
        }
    }
    Ok(())
}

/// Stima dei byte in memoria di un batch (somma delle colonne Arrow).
#[must_use]
pub fn estimated_batch_bytes(batch: &RecordBatch) -> usize {
    batch
        .columns()
        .iter()
        .fold(0_usize, |total, column| {
            total.saturating_add(column.get_array_memory_size())
        })
}

#[must_use]
pub fn should_spill(left: &RecordBatch, right: &RecordBatch, limits: &Limits) -> bool {
    estimated_batch_bytes(left).saturating_add(estimated_batch_bytes(right))
        > limits.max_memory_bytes
}

/// Variante unaria di `should_spill` per gli operatori a un input (sort,
/// distinct, hash aggregation; ADR-0002): si spilla quando il solo input
/// supera il budget `max_memory_bytes`.
#[must_use]
pub fn should_spill_unary(batch: &RecordBatch, limits: &Limits) -> bool {
    estimated_batch_bytes(batch) > limits.max_memory_bytes
}

/// Set operation binaria con spill su disco delle chiavi compatte.
///
/// Le chiavi delle righe sono partizionate per hash su file binari con
/// quota `max_temp_bytes`; il matching per partizione applica
/// `union_distinct`, `intersect` oppure `except` (default) rispettando
/// `max_memory_bytes` sul working set, poi le righe sopravvissute sono
/// riselezionate dagli input con `select_rows`.
///
/// # Errors
///
/// - `Schema`: schemi dei due input incompatibili (`validate_schema`) o
///   errore Arrow in `select_rows`/`concat_compatible`;
/// - `Contract`: vincoli dello spill violati (partizioni zero, chiave
///   oltre `max_temp_bytes`, working set di una partizione oltre
///   `max_memory_bytes`, overflow interni di accounting);
/// - `Io`: errori sui file temporanei (creazione, scrittura, lettura).
pub fn execute_set_operation(
    operation: &str,
    left: &RecordBatch,
    right: &RecordBatch,
    limits: &Limits,
) -> Result<RecordBatch> {
    setops::validate_schema(left, right)?;
    let mut workspace = SpillWorkspace::new(limits.max_temp_bytes)?;
    let left_paths = workspace.paths("left", limits.spill_partitions);
    let mut left_writers = open_writers(&left_paths)?;
    spill_batch(left, 0, &mut left_writers, &mut workspace, limits)?;
    let mut ordinals = Vec::new();

    if operation == "union_distinct" {
        spill_batch(
            right,
            left.num_rows(),
            &mut left_writers,
            &mut workspace,
            limits,
        )?;
        close_writers(left_writers)?;
        for path in &left_paths {
            collect_distinct(path, limits, &mut ordinals)?;
        }
        ordinals.sort_unstable();
        let split = left.num_rows();
        let left_rows = ordinals
            .iter()
            .copied()
            .take_while(|ordinal| *ordinal < split)
            .collect::<Vec<_>>();
        let right_rows = ordinals
            .iter()
            .copied()
            .skip_while(|ordinal| *ordinal < split)
            .map(|ordinal| ordinal - split)
            .collect::<Vec<_>>();
        let selected_left = select_rows(left, &left_rows)?;
        let selected_right = select_rows(right, &right_rows)?;
        return setops::concat_compatible(&selected_left, &selected_right, limits);
    }

    close_writers(left_writers)?;
    let right_paths = workspace.paths("right", limits.spill_partitions);
    let mut right_writers = open_writers(&right_paths)?;
    spill_batch(right, 0, &mut right_writers, &mut workspace, limits)?;
    close_writers(right_writers)?;
    for (left_path, right_path) in left_paths.iter().zip(&right_paths) {
        collect_membership(
            left_path,
            right_path,
            limits,
            operation == "intersect",
            &mut ordinals,
        )?;
    }
    ordinals.sort_unstable();
    select_rows(left, &ordinals)
}

// ---------------------------------------------------------------------------
// Spill generalizzato a righe complete (M2a Fase 2B, ADR-0002 "Spill
// selettivo"): sort, distinct e hash aggregation.
//
// Formato su disco: Arrow IPC *stream* per partizione/run. Scelta rispetto a
// un formato custom di record binari: serializzazione gia' collaudata,
// nessun parser binario da mantenere, streaming nativo batch-per-batch (il
// lettore non carica mai l'intera partizione) e schema autodescrittivo
// (nullability e metadata Arrow preservati). L'overhead di framing per batch
// e' irrilevante a queste dimensioni di chunk: nessuna controindicazione
// forte, si usa IPC.
//
// Il partizionamento hash riusa `partition` (Sha256 sui byte di chiave di
// `KeyColumn`, gli stessi di `row_key`): chiavi uguali finiscono sempre nella
// stessa partizione, quindi gruppi e duplicati non attraversano mai le
// partizioni e l'aggregazione/distinct per partizione e' esatta.
//
// Integrazione con il governor (ADR-0002): la directory temporanea puo'
// venire dal chiamante (`RowSpillWorkspace::with_directory`), pensata per il
// `TempStore` condiviso per execution_id di plenora-engine: kernels-table
// resta senza dipendenze da engine e riceve solo path + quota. Con directory
// esterna la rimozione dei file resta a questo modulo (`cleanup`/`Drop`), la
// directory stessa appartiene al chiamante.
// ---------------------------------------------------------------------------

/// Colonna tecnica con l'indice di riga originale, aggiunta all'input prima
/// dello spill di `distinct` (il partizionamento disperde l'ordine di
/// arrivo, necessario per `keep=first`/`last`).
const SPILL_ORDINAL_COLUMN: &str = "__plenora_spill_ordinal";

/// Righe per chunk IPC: il lettore streaming tiene in memoria un chunk alla
/// volta per file aperto (run del merge sort, partizioni).
const SPILL_CHUNK_ROWS: usize = 8_192;

/// Metriche di spill richieste da ADR-0002: byte scritti e letti sui file
/// temporanei e numero di file (partizioni/run) materializzati.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SpillMetrics {
    pub bytes_written: u64,
    pub bytes_read: u64,
    pub files: usize,
}

enum SpillRoot {
    /// Directory temporanea posseduta: rimossa automaticamente al drop.
    Owned(TempDir),
    /// Directory del chiamante (es. il `TempStore` di engine): mai rimossa.
    External(PathBuf),
}

/// Workspace di spill a righe complete.
///
/// Quota `max_temp_bytes` con accounting `checked` a ogni chunk IPC, metriche
/// di scrittura/lettura e pulizia dei file registrati. I contatori sono
/// condivisi (`Rc<Cell>`) con i writer/reader conteggiati, cosi' la quota e'
/// verificata mentre i writer sono ancora aperti.
pub struct RowSpillWorkspace {
    root: SpillRoot,
    files: Vec<PathBuf>,
    files_created: usize,
    bytes_written: Rc<Cell<u64>>,
    bytes_read: Rc<Cell<u64>>,
    max_temp_bytes: u64,
}

impl RowSpillWorkspace {
    /// Workspace con directory temporanea posseduta (rimossa al drop).
    ///
    /// # Errors
    ///
    /// - `Io`: creazione della directory temporanea fallita.
    pub fn new(max_temp_bytes: u64) -> Result<Self> {
        Ok(Self {
            root: SpillRoot::Owned(
                tempfile::Builder::new()
                    .prefix("plenora-rows-spill-")
                    .tempdir()?,
            ),
            files: Vec::new(),
            files_created: 0,
            bytes_written: Rc::new(Cell::new(0)),
            bytes_read: Rc::new(Cell::new(0)),
            max_temp_bytes,
        })
    }

    /// Workspace su directory del chiamante (es. il `TempStore` condiviso
    /// per `execution_id` di plenora-engine).
    ///
    /// La directory e' creata se manca e MAI rimossa da questo modulo; i
    /// file di spill registrati sono comunque ripuliti.
    ///
    /// # Errors
    ///
    /// - `Io`: creazione della directory fallita.
    pub fn with_directory(directory: &Path, max_temp_bytes: u64) -> Result<Self> {
        std::fs::create_dir_all(directory)?;
        Ok(Self {
            root: SpillRoot::External(directory.to_path_buf()),
            files: Vec::new(),
            files_created: 0,
            bytes_written: Rc::new(Cell::new(0)),
            bytes_read: Rc::new(Cell::new(0)),
            max_temp_bytes,
        })
    }

    #[must_use]
    pub fn directory(&self) -> &Path {
        match &self.root {
            SpillRoot::Owned(directory) => directory.path(),
            SpillRoot::External(path) => path.as_path(),
        }
    }

    fn register(&mut self, path: PathBuf) {
        self.files.push(path);
        self.files_created += 1;
    }

    /// Verifica la quota dopo l'ultimo chunk scritto: errore dedicato
    /// `Contract`, stessa forma dello spill set-op.
    fn check_quota(&self) -> Result<()> {
        let written = self.bytes_written.get();
        if written > self.max_temp_bytes {
            return Err(PlenoraError::Contract(format!(
                "spill oltre max_temp_bytes: {} > {}",
                written, self.max_temp_bytes
            )));
        }
        Ok(())
    }

    /// Metriche accumulate (ADR-0002): `files` conta i file materializzati,
    /// anche se gia' ripuliti.
    #[must_use]
    pub fn metrics(&self) -> SpillMetrics {
        SpillMetrics {
            bytes_written: self.bytes_written.get(),
            bytes_read: self.bytes_read.get(),
            files: self.files_created,
        }
    }

    /// Rimuove i file di spill registrati (la directory resta: al drop per
    /// quella posseduta, al chiamante per quella esterna).
    ///
    /// # Errors
    ///
    /// - `Io`: rimozione di un file fallita con errore diverso da `NotFound`.
    pub fn cleanup(&mut self) -> Result<()> {
        for path in std::mem::take(&mut self.files) {
            match std::fs::remove_file(&path) {
                Ok(()) => {}
                Err(error) if error.kind() == ErrorKind::NotFound => {}
                Err(error) => return Err(PlenoraError::Io(error)),
            }
        }
        Ok(())
    }
}

impl Drop for RowSpillWorkspace {
    fn drop(&mut self) {
        for path in std::mem::take(&mut self.files) {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// Writer conteggiato: ogni byte scritto finisce nel contatore condiviso del
/// workspace (la quota e' verificata dal chiamante a ogni chunk IPC).
struct CountingWriter {
    inner: BufWriter<File>,
    counter: Rc<Cell<u64>>,
}

impl CountingWriter {
    fn create(path: &Path, counter: &Rc<Cell<u64>>) -> Result<Self> {
        Ok(Self {
            inner: BufWriter::new(File::create(path)?),
            counter: Rc::clone(counter),
        })
    }
}

impl Write for CountingWriter {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        let written = self.inner.write(buffer)?;
        self.counter
            .set(self.counter.get().saturating_add(written as u64));
        Ok(written)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

/// Reader conteggiato: alimenta la metrica `bytes_read` (nessuna quota in
/// lettura, solo osservabilita' ADR-0002).
struct CountingReader {
    inner: BufReader<File>,
    counter: Rc<Cell<u64>>,
}

impl CountingReader {
    fn open(path: &Path, counter: &Rc<Cell<u64>>) -> Result<Self> {
        Ok(Self {
            inner: BufReader::new(File::open(path)?),
            counter: Rc::clone(counter),
        })
    }
}

impl Read for CountingReader {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let read = self.inner.read(buffer)?;
        self.counter
            .set(self.counter.get().saturating_add(read as u64));
        Ok(read)
    }
}

/// Scrive un batch in un file IPC stream a chunk di `SPILL_CHUNK_ROWS`,
/// verificando la quota a ogni chunk.
fn write_ipc_chunks(
    workspace: &RowSpillWorkspace,
    path: &Path,
    batch: &RecordBatch,
) -> Result<()> {
    let writer = CountingWriter::create(path, &workspace.bytes_written)?;
    let mut writer = StreamWriter::try_new(writer, &batch.schema())?;
    workspace.check_quota()?;
    let mut offset = 0;
    while offset < batch.num_rows() {
        let length = SPILL_CHUNK_ROWS.min(batch.num_rows() - offset);
        writer.write(&batch.slice(offset, length))?;
        workspace.check_quota()?;
        offset += length;
    }
    writer.finish()?;
    workspace.check_quota()
}

/// Spilla `batch` in `partitions` file IPC partizionati per hash.
///
/// Il partizionamento usa i byte di chiave (`KeyColumn`, stessi byte di
/// `row_key`): chiavi uguali finiscono sempre nella stessa partizione.
/// Indici per partizione in memoria (un `usize` per riga) per una sola
/// passata di encoding delle chiavi.
fn spill_partitioned(
    workspace: &mut RowSpillWorkspace,
    batch: &RecordBatch,
    key_columns: &[KeyColumn],
    prefix: &str,
    partitions: usize,
) -> Result<Vec<PathBuf>> {
    if partitions == 0 {
        return Err(PlenoraError::Contract(
            "spill richiede almeno una partizione".into(),
        ));
    }
    let mut indices: Vec<Vec<usize>> = vec![Vec::new(); partitions];
    let mut key = String::new();
    let mut scratch = String::new();
    for row in 0..batch.num_rows() {
        key.clear();
        for column in key_columns {
            column.write_key(row, &mut key, &mut scratch)?;
        }
        indices[partition(key.as_bytes(), partitions)?].push(row);
    }
    let paths = (0..partitions)
        .map(|index| {
            workspace
                .directory()
                .join(format!("{prefix}-{index:04}.ipc"))
        })
        .collect::<Vec<_>>();
    for (path, rows) in paths.iter().zip(&indices) {
        workspace.register(path.clone());
        let partition_batch = select_rows(batch, rows)?;
        write_ipc_chunks(workspace, path, &partition_batch)?;
    }
    Ok(paths)
}

/// Legge una partizione IPC in streaming e la ricompone; fallisce se la
/// partizione supera `max_memory_bytes` (partizioni troppo poche o skew
/// delle chiavi: aumentare `spill_partitions`).
fn read_partition(
    workspace: &RowSpillWorkspace,
    path: &Path,
    limits: &Limits,
) -> Result<RecordBatch> {
    let reader = CountingReader::open(path, &workspace.bytes_read)?;
    let mut reader = StreamReader::try_new(reader, None)?;
    let schema = reader.schema();
    let mut batches = Vec::new();
    let mut estimated = 0_usize;
    while let Some(batch) = reader.next().transpose()? {
        estimated = estimated
            .checked_add(estimated_batch_bytes(&batch))
            .ok_or_else(|| PlenoraError::Contract("overflow memoria spill".into()))?;
        if estimated > limits.max_memory_bytes {
            return Err(PlenoraError::Contract(
                "partizione spill oltre max_memory_bytes".into(),
            ));
        }
        batches.push(batch);
    }
    if batches.is_empty() {
        return Ok(RecordBatch::new_empty(schema));
    }
    Ok(concat_batches(&schema, &batches)?)
}

/// `table.distinct` con spill (ADR-0002).
///
/// Righe complete partizionate su disco per hash della chiave, poi una
/// passata in streaming che accumula le statistiche per chiave
/// (prima/ultima occorrenza globale, conteggio) con gli stessi byte di
/// `row_key` del percorso in memoria. Output identico a `distinct`: la mappa
/// delle chiavi resta in RAM (e' dimensionata sull'output, una voce per
/// chiave distinta) ed e' contabilizzata su `max_memory_bytes`.
///
/// # Errors
///
/// Come [`distinct_spilled_in`]; in piu' `Io` se la creazione della
/// directory temporanea fallisce.
pub fn distinct_spilled(
    batch: &RecordBatch,
    config: &Distinct,
    limits: &Limits,
) -> Result<(RecordBatch, SpillMetrics)> {
    let mut workspace = RowSpillWorkspace::new(limits.max_temp_bytes)?;
    distinct_spilled_in(batch, config, limits, &mut workspace)
}

/// Come [`distinct_spilled`], ma su un workspace del chiamante.
///
/// Punto di ingresso per l'integrazione con il `TempStore` di plenora-engine
/// (directory esterna + quota condivisa, nessuna dipendenza da engine).
///
/// # Errors
///
/// - `Schema`: colonna di `config.subset` assente (`column_index`) o
///   errore Arrow in `replace_or_append`/`select_rows`;
/// - `Contract`: colonna riservata allo spill gia' presente, quote
///   superate (`max_temp_bytes`, `max_memory_bytes`), `spill_partitions`
///   zero, ordinal/chiave non rappresentabili;
/// - `Io`: errori sui file di spill (creazione, scrittura/lettura IPC,
///   pulizia).
///
/// Su input vuoto delega a `distinct` e ne propaga gli errori.
#[allow(clippy::too_many_lines)] // Una sola passata di streaming con i suoi invarianti di accounting.
pub fn distinct_spilled_in(
    batch: &RecordBatch,
    config: &Distinct,
    limits: &Limits,
    workspace: &mut RowSpillWorkspace,
) -> Result<(RecordBatch, SpillMetrics)> {
    struct KeyStats {
        first: u64,
        last: u64,
        count: usize,
    }
    if batch.num_rows() == 0 {
        return Ok((
            aggregation::distinct(batch, config)?,
            SpillMetrics::default(),
        ));
    }
    let indices = if config.subset.is_empty() {
        (0..batch.num_columns()).collect::<Vec<_>>()
    } else {
        config
            .subset
            .iter()
            .map(|name| column_index(batch, name))
            .collect::<Result<Vec<_>>>()?
    };
    if batch.schema().index_of(SPILL_ORDINAL_COLUMN).is_ok() {
        return Err(PlenoraError::Contract(format!(
            "colonna riservata allo spill: {SPILL_ORDINAL_COLUMN}"
        )));
    }
    let key_columns = indices
        .iter()
        .map(|index| KeyColumn::new(batch.column(*index)))
        .collect::<Vec<_>>();
    let ordinals = (0..batch.num_rows())
        .map(|row| {
            u64::try_from(row)
                .map_err(|_| PlenoraError::Contract("ordinal spill oltre u64".into()))
        })
        .collect::<Result<Vec<_>>>()?;
    let with_ordinal = replace_or_append(
        batch,
        SPILL_ORDINAL_COLUMN,
        DataType::UInt64,
        false,
        Arc::new(UInt64Array::from(ordinals)),
    )?;
    let paths = spill_partitioned(
        workspace,
        &with_ordinal,
        &key_columns,
        "distinct",
        limits.spill_partitions,
    )?;

    let mut stats: HashMap<Box<[u8]>, KeyStats, std::hash::BuildHasherDefault<KeyHasher>> =
        HashMap::default();
    let mut estimated = 0_usize;
    let mut key = String::new();
    let mut scratch = String::new();
    for path in &paths {
        let reader = CountingReader::open(path, &workspace.bytes_read)?;
        let mut reader = StreamReader::try_new(reader, None)?;
        while let Some(partition_batch) = reader.next().transpose()? {
            let ordinal_column = partition_batch
                .column(partition_batch.num_columns() - 1)
                .as_any()
                .downcast_ref::<UInt64Array>()
                .ok_or_else(|| PlenoraError::Contract("colonna ordinale spill mancante".into()))?;
            let read_keys = indices
                .iter()
                .map(|index| KeyColumn::new(partition_batch.column(*index)))
                .collect::<Vec<_>>();
            for row in 0..partition_batch.num_rows() {
                key.clear();
                for column in &read_keys {
                    column.write_key(row, &mut key, &mut scratch)?;
                }
                // Le occorrenze di una chiave stanno in una sola partizione,
                // scritte in ordine crescente di indice: min/max sono
                // comunque applicati per robustezza.
                let ordinal = ordinal_column.value(row);
                if let Some(entry) = stats.get_mut(key.as_bytes()) {
                    entry.first = entry.first.min(ordinal);
                    entry.last = entry.last.max(ordinal);
                    entry.count += 1;
                } else {
                    estimated = estimated
                        .checked_add(key.len().saturating_add(RECORD_OVERHEAD_ESTIMATE))
                        .ok_or_else(|| PlenoraError::Contract("overflow memoria spill".into()))?;
                    if estimated > limits.max_memory_bytes {
                        return Err(PlenoraError::Contract(
                            "distinct spill oltre max_memory_bytes".into(),
                        ));
                    }
                    stats.insert(
                        key.clone().into_bytes().into_boxed_slice(),
                        KeyStats {
                            first: ordinal,
                            last: ordinal,
                            count: 1,
                        },
                    );
                }
            }
        }
    }
    let mut rows = stats
        .values()
        .filter_map(|entry| match config.keep {
            Keep::First => Some(entry.first),
            Keep::Last => Some(entry.last),
            Keep::False => (entry.count == 1).then_some(entry.first),
        })
        .map(|ordinal| {
            usize::try_from(ordinal)
                .map_err(|_| PlenoraError::Contract("ordinal spill non rappresentabile".into()))
        })
        .collect::<Result<Vec<_>>>()?;
    rows.sort_unstable();
    let output = select_rows(batch, &rows)?;
    workspace.cleanup()?;
    Ok((output, workspace.metrics()))
}

/// `table.aggregate` con spill (ADR-0002).
///
/// Righe complete partizionate per hash della chiave di gruppo (ogni gruppo
/// vive interamente in una partizione), aggregazione in memoria di una
/// partizione alla volta e riordino finale sull'ordine canonico delle chiavi
/// (lessicografico sui byte di `row_key`, lo stesso del `BTreeMap` del
/// percorso in memoria). La mappa dei gruppi in RAM e' quindi dimensionata
/// su una partizione, non sull'intero input. Output identico ad `aggregate`.
///
/// # Errors
///
/// Come [`aggregate_spilled_in`]; in piu' `Io` se la creazione della
/// directory temporanea fallisce.
pub fn aggregate_spilled(
    batch: &RecordBatch,
    config: &Aggregate,
    limits: &Limits,
) -> Result<(RecordBatch, SpillMetrics)> {
    let mut workspace = RowSpillWorkspace::new(limits.max_temp_bytes)?;
    aggregate_spilled_in(batch, config, limits, &mut workspace)
}

/// Come [`aggregate_spilled`], ma su un workspace del chiamante (integrazione
/// `TempStore` di plenora-engine, cfr. [`distinct_spilled_in`]).
///
/// # Errors
///
/// - `Schema`: colonna di `group_by` assente (`column_index`) o errore
///   Arrow in `aggregate`/`concat_batches`/`select_rows`;
/// - `Contract`: `group_by` vuoto, quote superate (`max_temp_bytes`,
///   `max_memory_bytes`), `spill_partitions` zero, output di partizione
///   incoerente con i gruppi (invariante interna);
/// - `Io`: errori sui file di spill (creazione, scrittura/lettura IPC,
///   pulizia).
///
/// Su input vuoto delega ad `aggregate` e ne propaga gli errori.
pub fn aggregate_spilled_in(
    batch: &RecordBatch,
    config: &Aggregate,
    limits: &Limits,
    workspace: &mut RowSpillWorkspace,
) -> Result<(RecordBatch, SpillMetrics)> {
    let group_indices = config
        .group_by
        .iter()
        .map(|name| column_index(batch, name))
        .collect::<Result<Vec<_>>>()?;
    if group_indices.is_empty() {
        return Err(PlenoraError::Contract("aggregate richiede group_by".into()));
    }
    if batch.num_rows() == 0 {
        return Ok((
            aggregation::aggregate(batch, config)?,
            SpillMetrics::default(),
        ));
    }
    let key_columns = group_indices
        .iter()
        .map(|index| KeyColumn::new(batch.column(*index)))
        .collect::<Vec<_>>();
    let paths = spill_partitioned(
        workspace,
        batch,
        &key_columns,
        "aggregate",
        limits.spill_partitions,
    )?;
    let mut outputs = Vec::new();
    let mut keys = Vec::new();
    let mut key = String::new();
    let mut scratch = String::new();
    for path in &paths {
        let partition_batch = read_partition(workspace, path, limits)?;
        if partition_batch.num_rows() == 0 {
            continue;
        }
        // Chiavi distinte della partizione in ordine canonico: la riga i
        // dell'output di `aggregate` corrisponde alla i-esima chiave
        // ordinata. Vanno calcolate QUI, sul batch di partizione (con
        // `KeyColumn` costruiti su di esso): una aggregazione con lo stesso
        // nome di una colonna di gruppo la rimpiazza nell'output
        // (`replace_or_append`), rendendo le colonne di gruppo non piu'
        // leggibili dal risultato.
        let read_keys = group_indices
            .iter()
            .map(|index| KeyColumn::new(partition_batch.column(*index)))
            .collect::<Vec<_>>();
        let mut distinct_keys = std::collections::BTreeSet::new();
        for row in 0..partition_batch.num_rows() {
            key.clear();
            for column in &read_keys {
                column.write_key(row, &mut key, &mut scratch)?;
            }
            distinct_keys.insert(key.clone());
        }
        let partition_output = aggregation::aggregate(&partition_batch, config)?;
        if partition_output.num_rows() != distinct_keys.len() {
            return Err(PlenoraError::Contract(
                "aggregate spill: righe di output diverse dai gruppi".into(),
            ));
        }
        keys.extend(distinct_keys);
        outputs.push(partition_output);
    }
    // `batch` non vuoto implica almeno una partizione non vuota.
    let Some(first) = outputs.first() else {
        return Err(PlenoraError::Contract("aggregate spill senza partizioni".into()));
    };
    let combined = concat_batches(&first.schema(), &outputs)?;
    // Le chiavi sono univoche per costruzione (un gruppo = una riga),
    // quindi l'ordinamento e' deterministico.
    let mut order: Vec<usize> = (0..combined.num_rows()).collect();
    order.sort_unstable_by(|left, right| keys[*left].cmp(&keys[*right]));
    let output = select_rows(&combined, &order)?;
    workspace.cleanup()?;
    Ok((output, workspace.metrics()))
}

/// Righe per run dell'external merge sort: un quarto del budget memoria
/// diviso per la stima byte/riga (una run ordinata occupa batch sorgente,
/// permutazione e copia ordinata), almeno una riga.
fn sort_run_rows(batch: &RecordBatch, limits: &Limits) -> usize {
    let bytes = estimated_batch_bytes(batch).max(1);
    let per_row = (bytes / batch.num_rows().max(1)).max(1);
    (limits.max_memory_bytes / 4 / per_row).max(1)
}

/// Cursore di streaming su una run IPC ordinata: tiene in memoria un solo
/// chunk alla volta; `base` e' l'indice globale (nel batch di input) della
/// prima riga del chunk corrente.
struct RunCursor {
    reader: StreamReader<CountingReader>,
    current: Option<RecordBatch>,
    row: usize,
    base: usize,
}

impl RunCursor {
    fn open(workspace: &RowSpillWorkspace, path: &Path, base: usize) -> Result<Self> {
        let reader = CountingReader::open(path, &workspace.bytes_read)?;
        let mut cursor = Self {
            reader: StreamReader::try_new(reader, None)?,
            current: None,
            row: 0,
            base,
        };
        cursor.next_chunk()?;
        Ok(cursor)
    }

    fn next_chunk(&mut self) -> Result<()> {
        self.base += self.current.as_ref().map_or(0, RecordBatch::num_rows);
        self.current = self.reader.next().transpose()?;
        self.row = 0;
        Ok(())
    }

    const fn exhausted(&self) -> bool {
        self.current.is_none()
    }

    fn advance(&mut self) -> Result<()> {
        self.row += 1;
        let end = self.current.as_ref().map_or(0, RecordBatch::num_rows);
        if self.row >= end {
            self.next_chunk()?;
        }
        Ok(())
    }

    const fn global_index(&self) -> usize {
        self.base + self.row
    }
}

/// Confronto tra celle di batch diversi (merge k-way).
///
/// Delega a `compare_cells_typed`, il comparatore tipizzato unico
/// condiviso con `compare_at` e `ColumnComparator` (null in coda,
/// `i64::cmp`/`u64::cmp` esatti, `total_cmp` su Float64, confronto
/// testuale altrove).
fn compare_cells(
    challenger: &RunCursor,
    champion: &RunCursor,
    column: usize,
) -> Result<Ordering> {
    let left_batch = challenger
        .current
        .as_ref()
        .ok_or_else(|| PlenoraError::Contract("cursore spill esaurito".into()))?;
    let right_batch = champion
        .current
        .as_ref()
        .ok_or_else(|| PlenoraError::Contract("cursore spill esaurito".into()))?;
    aggregation::compare_cells_typed(
        left_batch.column(column),
        challenger.row,
        right_batch.column(column),
        champion.row,
    )
}

/// `table.sort` con spill (ADR-0002): external merge sort.
///
/// L'input e' affettato in run dimensionate su `max_memory_bytes`, ogni run
/// e' ordinata in memoria con `sort` e spillata su IPC; il merge k-way in
/// streaming (un chunk per run alla volta) produce la permutazione globale e
/// l'output e' ricostruito con `select_rows` sull'input. A parita' di chiavi
/// vince l'indice globale minore (stabilita'): output identico a `sort`.
///
/// # Errors
///
/// Come [`sort_spilled_in`]; in piu' `Io` se la creazione della directory
/// temporanea fallisce.
pub fn sort_spilled(
    batch: &RecordBatch,
    config: &Sort,
    limits: &Limits,
) -> Result<(RecordBatch, SpillMetrics)> {
    let mut workspace = RowSpillWorkspace::new(limits.max_temp_bytes)?;
    sort_spilled_in(batch, config, limits, &mut workspace)
}

/// Come [`sort_spilled`], ma su un workspace del chiamante (integrazione
/// `TempStore` di plenora-engine, cfr. [`distinct_spilled_in`]).
///
/// # Errors
///
/// - `Schema`: colonna di sort assente (`column_index`) o errore Arrow in
///   `sort`/`select_rows`;
/// - `Contract`: nessuna colonna di sort, quota `max_temp_bytes` superata,
///   confronto tra celle fallito (`compare_cells_typed`);
/// - `Io`: errori sui file di spill (creazione, scrittura/lettura IPC,
///   pulizia).
///
/// Su input vuoto delega a `sort` e ne propaga gli errori.
pub fn sort_spilled_in(
    batch: &RecordBatch,
    config: &Sort,
    limits: &Limits,
    workspace: &mut RowSpillWorkspace,
) -> Result<(RecordBatch, SpillMetrics)> {
    let indices = config
        .columns
        .iter()
        .map(|name| column_index(batch, name))
        .collect::<Result<Vec<_>>>()?;
    if indices.is_empty() {
        return Err(PlenoraError::Contract("sort richiede colonne".into()));
    }
    if batch.num_rows() == 0 {
        return Ok((aggregation::sort(batch, config)?, SpillMetrics::default()));
    }
    let run_rows = sort_run_rows(batch, limits);
    let mut run_paths = Vec::new();
    let mut run_bases = Vec::new();
    let mut start = 0;
    while start < batch.num_rows() {
        let length = run_rows.min(batch.num_rows() - start);
        let sorted = aggregation::sort(&batch.slice(start, length), config)?;
        let path = workspace
            .directory()
            .join(format!("sort-run-{:04}.ipc", run_paths.len()));
        workspace.register(path.clone());
        write_ipc_chunks(workspace, &path, &sorted)?;
        run_paths.push(path);
        run_bases.push(start);
        start += length;
    }
    let mut cursors = Vec::new();
    for (path, base) in run_paths.iter().zip(&run_bases) {
        cursors.push(RunCursor::open(workspace, path, *base)?);
    }
    // Merge k-way a scansione lineare: il numero di run e' piccolo (una per
    // fetta di budget memoria) e il confronto fallibile resta propagabile
    // con `?`, cosa scomoda con un BinaryHeap.
    let mut permutation = Vec::with_capacity(batch.num_rows());
    loop {
        let mut best: Option<usize> = None;
        for index in 0..cursors.len() {
            if cursors[index].exhausted() {
                continue;
            }
            let Some(champion) = best else {
                best = Some(index);
                continue;
            };
            let mut ordering = Ordering::Equal;
            for column in &indices {
                ordering = compare_cells(&cursors[index], &cursors[champion], *column)?;
                if ordering != Ordering::Equal {
                    break;
                }
            }
            if !config.ascending {
                ordering = ordering.reverse();
            }
            // A parita' resta il campione: i cursori sono scanditi in ordine
            // di run (offset globali crescenti), quindi vince l'indice
            // globale minore, come lo spareggio stabile di `sort`.
            if ordering == Ordering::Less {
                best = Some(index);
            }
        }
        let Some(champion) = best else {
            break;
        };
        permutation.push(cursors[champion].global_index());
        cursors[champion].advance()?;
    }
    drop(cursors);
    let output = select_rows(batch, &permutation)?;
    workspace.cleanup()?;
    Ok((output, workspace.metrics()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reader_with(bytes: &[u8]) -> (TempDir, BufReader<File>) {
        let directory = tempfile::tempdir().expect("tempdir");
        let path = directory.path().join("record.bin");
        let mut file = File::create(&path).expect("create");
        file.write_all(bytes).expect("write");
        file.flush().expect("flush");
        drop(file);
        let reader = BufReader::new(File::open(path).expect("open"));
        (directory, reader)
    }

    #[test]
    fn malformed_spill_records_and_zero_partitions_fail_closed() {
        assert!(partition(b"key", 0).is_err());
        let (_, mut empty) = reader_with(&[]);
        assert!(read_record(&mut empty, 16).expect("empty").is_none());

        let (_, mut partial_header) = reader_with(&[1]);
        assert!(read_u64(&mut partial_header).is_err());

        let (_, mut missing_length) = reader_with(&1_u64.to_be_bytes());
        assert!(read_record(&mut missing_length, 16).is_err());

        let mut oversized = Vec::new();
        oversized.extend_from_slice(&1_u64.to_be_bytes());
        oversized.extend_from_slice(&32_u64.to_be_bytes());
        let (_, mut oversized) = reader_with(&oversized);
        assert!(read_record(&mut oversized, 16).is_err());

        let mut truncated = Vec::new();
        truncated.extend_from_slice(&1_u64.to_be_bytes());
        truncated.extend_from_slice(&3_u64.to_be_bytes());
        truncated.push(7);
        let (_, mut truncated) = reader_with(&truncated);
        assert!(read_record(&mut truncated, 16).is_err());
    }

    #[test]
    fn spill_quotas_and_partition_working_sets_are_enforced() {
        let mut tiny = SpillWorkspace::new(1).expect("workspace");
        assert!(tiny.account(2).is_err());

        let mut workspace = SpillWorkspace::new(1_024).expect("workspace");
        let paths = workspace.paths("keys", 1);
        let mut writers = open_writers(&paths).expect("writers");
        write_record(&mut writers[0], 0, b"one", &mut workspace).expect("first");
        write_record(&mut writers[0], 1, b"two", &mut workspace).expect("second");
        write_record(&mut writers[0], 2, b"one", &mut workspace).expect("duplicate");
        close_writers(writers).expect("close");

        let tight = Limits {
            max_memory_bytes: 70,
            ..Limits::default()
        };
        assert!(load_key_set(&paths[0], &tight).is_err());
        assert!(collect_distinct(&paths[0], &tight, &mut Vec::new()).is_err());

        let missing = workspace.directory.path().join("missing.bin");
        assert!(load_key_set(&missing, &Limits::default()).is_err());

        let empty_path = workspace.directory.path().join("empty.bin");
        File::create(&empty_path).expect("empty");
        assert!(
            collect_membership(&paths[0], &empty_path, &tight, false, &mut Vec::new()).is_err()
        );
    }

    // ------------------------------------------------------------------
    // Oracoli memoria-vs-spill (M2a Fase 2B): input piccoli deterministici,
    // output esattamente identico al percorso in memoria.
    // ------------------------------------------------------------------

    use crate::aggregation::{AggFunction, Aggregate, Aggregation};
    use plenora_core::arrow::array::{Float64Array, Int64Array, StringArray};
    use plenora_core::arrow::schema::{Field, Schema};

    fn spill_test_limits(max_memory_bytes: usize) -> Limits {
        Limits {
            max_memory_bytes,
            max_temp_bytes: 1 << 30,
            spill_partitions: 8,
            ..Limits::default()
        }
    }

    /// 48 righe deterministiche con duplicati e null su tre tipi diversi.
    fn rows_fixture() -> RecordBatch {
        let ints: Vec<Option<i64>> = (0..48)
            .map(|i| match i % 7 {
                0 => None,
                r => Some(i64::from((r * 13 + i * 5) % 9) - 4),
            })
            .collect();
        let texts: Vec<Option<&str>> = (0..48)
            .map(|i| match i % 5 {
                0 => None,
                1 => Some("delta"),
                2 | 3 => Some("alfa"),
                _ => Some("zulu"),
            })
            .collect();
        let floats: Vec<Option<f64>> = (0..48)
            .map(|i| match i % 4 {
                0 => None,
                r => Some(f64::from(r).mul_add(0.5, f64::from(i % 3))),
            })
            .collect();
        RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("a", DataType::Int64, true),
                Field::new("b", DataType::Utf8, true),
                Field::new("c", DataType::Float64, true),
            ])),
            vec![
                Arc::new(Int64Array::from(ints)),
                Arc::new(StringArray::from(texts)),
                Arc::new(Float64Array::from(floats)),
            ],
        )
        .expect("fixture")
    }

    #[test]
    fn spilled_sort_matches_in_memory() {
        let batch = rows_fixture();
        // Budget minuscolo: run di poche righe, molte run su disco.
        let limits = spill_test_limits(128);
        for (columns, ascending) in [
            (vec!["a".to_string()], true),
            (vec!["a".to_string()], false),
            (vec!["b".to_string(), "a".to_string()], true),
            (vec!["c".to_string(), "b".to_string()], false),
        ] {
            let config = Sort { columns, ascending };
            let expected = aggregation::sort(&batch, &config).expect("sort in memoria");
            let (spilled, metrics) = sort_spilled(&batch, &config, &limits).expect("sort spilled");
            assert_eq!(spilled, expected, "config {config:?}");
            assert!(metrics.files > 1, "attese piu' run: {metrics:?}");
            assert!(metrics.bytes_written > 0 && metrics.bytes_read > 0);
        }
    }

    #[test]
    fn spilled_sort_orders_i64_and_u64_exactly_across_runs() {
        // Regressione (bug 6/7) nel merge k-way: la coppia 2^53 / 2^53+1
        // (stesso double) sta in run diverse e UInt64 non deve cadere nel
        // confronto testuale ("10" < "9").
        let big: i64 = 1 << 53;
        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("i", DataType::Int64, false),
                Field::new("u", DataType::UInt64, false),
            ])),
            vec![
                Arc::new(Int64Array::from(vec![
                    big + 1,
                    big,
                    big + 2,
                    -big - 1,
                    -big,
                    0,
                    7,
                    -3,
                ])),
                Arc::new(UInt64Array::from(vec![9_u64, 10, 100, 99, 0, 1000, 55, 42])),
            ],
        )
        .expect("fixture");
        // Budget minuscolo: run di una riga, merge a otto vie.
        let limits = spill_test_limits(16);

        let config = Sort {
            columns: vec!["i".to_string()],
            ascending: true,
        };
        let (spilled, metrics) = sort_spilled(&batch, &config, &limits).expect("sort spilled i64");
        let values = spilled
            .column(0)
            .as_any()
            .downcast_ref::<Int64Array>()
            .expect("colonna i64")
            .values()
            .to_vec();
        assert_eq!(values, vec![-big - 1, -big, -3, 0, 7, big, big + 1, big + 2]);
        assert!(metrics.files > 1, "attese piu' run: {metrics:?}");
        // Oracolo: identico al percorso in memoria.
        assert_eq!(spilled, aggregation::sort(&batch, &config).expect("sort"));

        let config = Sort {
            columns: vec!["u".to_string()],
            ascending: true,
        };
        let (spilled, _) = sort_spilled(&batch, &config, &limits).expect("sort spilled u64");
        let values = spilled
            .column(1)
            .as_any()
            .downcast_ref::<UInt64Array>()
            .expect("colonna u64")
            .values()
            .to_vec();
        assert_eq!(values, vec![0, 9, 10, 42, 55, 99, 100, 1000]);
        assert_eq!(spilled, aggregation::sort(&batch, &config).expect("sort"));
    }

    #[test]
    fn spilled_distinct_matches_in_memory() {
        let batch = rows_fixture();
        let limits = spill_test_limits(1 << 20);
        for config in [
            Distinct {
                subset: vec![],
                keep: Keep::First,
            },
            Distinct {
                subset: vec![],
                keep: Keep::Last,
            },
            Distinct {
                subset: vec![],
                keep: Keep::False,
            },
            Distinct {
                subset: vec!["a".to_string()],
                keep: Keep::First,
            },
            Distinct {
                subset: vec!["b".to_string(), "a".to_string()],
                keep: Keep::Last,
            },
        ] {
            let expected = aggregation::distinct(&batch, &config).expect("distinct in memoria");
            let (spilled, metrics) =
                distinct_spilled(&batch, &config, &limits).expect("distinct spilled");
            assert_eq!(spilled, expected, "config {config:?}");
            assert_eq!(metrics.files, limits.spill_partitions);
            assert!(metrics.bytes_written > 0 && metrics.bytes_read > 0);
        }
    }

    fn aggregate_config(group_by: &[&str]) -> Aggregate {
        let aggregation = |column: &str, function: AggFunction, alias: &str| Aggregation {
            column: column.to_string(),
            function,
            separator: ", ".into(),
            distinct: false,
            skip_null: true,
            alias: alias.to_string(),
            quantile: None,
            ddof: 1,
        };
        Aggregate {
            group_by: group_by.iter().map(|name| (*name).to_string()).collect(),
            aggregations: vec![
                aggregation("c", AggFunction::Sum, "tot"),
                aggregation("c", AggFunction::Mean, "media"),
                aggregation("c", AggFunction::Min, ""),
                aggregation("c", AggFunction::Max, ""),
                aggregation("a", AggFunction::Count, ""),
                aggregation("b", AggFunction::First, ""),
                aggregation("b", AggFunction::Last, ""),
                aggregation("b", AggFunction::Concat, ""),
                aggregation("b", AggFunction::Nunique, ""),
            ],
        }
    }

    #[test]
    fn spilled_aggregate_matches_in_memory() {
        let batch = rows_fixture();
        let limits = spill_test_limits(1 << 20);
        for group_by in [vec!["a"], vec!["b"], vec!["b", "a"]] {
            let config = aggregate_config(&group_by);
            let expected = aggregation::aggregate(&batch, &config).expect("aggregate in memoria");
            let (spilled, metrics) =
                aggregate_spilled(&batch, &config, &limits).expect("aggregate spilled");
            assert_eq!(spilled, expected, "group_by {group_by:?}");
            assert_eq!(metrics.files, limits.spill_partitions);
            assert!(metrics.bytes_written > 0 && metrics.bytes_read > 0);
        }
        // Variante a solo conteggio (aggregations vuote).
        let config = Aggregate {
            group_by: vec!["a".to_string()],
            aggregations: vec![],
        };
        let expected = aggregation::aggregate(&batch, &config).expect("conteggio in memoria");
        let (spilled, _) = aggregate_spilled(&batch, &config, &limits).expect("conteggio spilled");
        assert_eq!(spilled, expected);
    }

    #[test]
    fn spilled_paths_enforce_quotas_and_clean_up_files() {
        let batch = rows_fixture();
        let root = tempfile::tempdir().expect("tempdir");
        let directory = root.path().join("exec-1/node-distinct");
        let config = Distinct {
            subset: vec![],
            keep: Keep::First,
        };

        // Quota temp superata: errore dedicato e file parziali rimossi.
        {
            let mut workspace =
                RowSpillWorkspace::with_directory(&directory, 1).expect("workspace");
            let limits = spill_test_limits(1 << 20);
            let error = distinct_spilled_in(&batch, &config, &limits, &mut workspace)
                .expect_err("quota temp superata");
            assert!(error.to_string().contains("max_temp_bytes"), "{error}");
            drop(workspace);
            assert!(std::fs::read_dir(&directory)
                .expect("read_dir")
                .next()
                .is_none());
        }

        // Budget memoria insufficiente per la mappa delle chiavi in lettura.
        {
            let mut workspace =
                RowSpillWorkspace::with_directory(&directory, 1 << 30).expect("workspace");
            let tight = spill_test_limits(1);
            let error = distinct_spilled_in(&batch, &config, &tight, &mut workspace)
                .expect_err("quota memoria superata");
            assert!(error.to_string().contains("max_memory_bytes"), "{error}");
            drop(workspace);
        }

        // Percorso felice: metriche esposte, file ripuliti, directory
        // esterna mai rimossa (appartiene al chiamante, es. TempStore).
        let mut workspace =
            RowSpillWorkspace::with_directory(&directory, 1 << 30).expect("workspace");
        let limits = spill_test_limits(1 << 20);
        let (_, metrics) = distinct_spilled_in(&batch, &config, &limits, &mut workspace)
            .expect("distinct spilled");
        assert_eq!(metrics.files, limits.spill_partitions);
        assert!(metrics.bytes_written > 0 && metrics.bytes_read > 0);
        assert!(std::fs::read_dir(&directory)
            .expect("read_dir")
            .next()
            .is_none());
        drop(workspace);
        assert!(directory.exists());
    }

    #[test]
    fn should_spill_unary_tracks_memory_budget_and_empty_inputs_delegate() {
        let batch = rows_fixture();
        assert!(!should_spill_unary(&batch, &spill_test_limits(1 << 30)));
        assert!(should_spill_unary(&batch, &spill_test_limits(1)));
        assert!(estimated_batch_bytes(&batch) > 0);

        let empty = batch.slice(0, 0);
        let limits = spill_test_limits(1 << 20);
        let sort_config = Sort {
            columns: vec!["a".to_string()],
            ascending: true,
        };
        let (output, metrics) = sort_spilled(&empty, &sort_config, &limits).expect("sort vuoto");
        assert_eq!(output.num_rows(), 0);
        assert_eq!(metrics, SpillMetrics::default());
        let distinct_config = Distinct {
            subset: vec![],
            keep: Keep::First,
        };
        let (output, metrics) =
            distinct_spilled(&empty, &distinct_config, &limits).expect("distinct vuoto");
        assert_eq!(output.num_rows(), 0);
        assert_eq!(metrics, SpillMetrics::default());
        let aggregate_config = Aggregate {
            group_by: vec!["a".to_string()],
            aggregations: vec![],
        };
        let (output, metrics) =
            aggregate_spilled(&empty, &aggregate_config, &limits).expect("aggregate vuoto");
        assert_eq!(output.num_rows(), 0);
        assert_eq!(metrics, SpillMetrics::default());
    }
}

