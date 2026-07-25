use std::collections::HashSet;
use std::fs::File;
use std::io::{BufReader, BufWriter, ErrorKind, Read, Write};
use std::path::PathBuf;

use plenora_core::arrow::array::{Array, RecordBatch};
use sha2::{Digest, Sha256};
use tempfile::TempDir;

use crate::Limits;
use plenora_core::{PlenoraError, Result};
use crate::select_rows;
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

pub fn should_spill(left: &RecordBatch, right: &RecordBatch, limits: &Limits) -> bool {
    let bytes = left
        .columns()
        .iter()
        .chain(right.columns())
        .fold(0_usize, |total, column| {
            total.saturating_add(column.get_array_memory_size())
        });
    bytes > limits.max_memory_bytes
}

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
}
