// Licensed to the Apache Software Foundation (ASF) under one
// or more contributor license agreements.  See the NOTICE file
// distributed with this work for additional information
// regarding copyright ownership.  The ASF licenses this file
// to you under the Apache License, Version 2.0 (the
// "License"); you may not use this file except in compliance
// with the License.  You may obtain a copy of the License at
//
//   http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing,
// software distributed under the License is distributed on an
// "AS IS" BASIS, WITHOUT WARRANTIES OR CONDITIONS OF ANY
// KIND, either express or implied.  See the License for the
// specific language governing permissions and limitations
// under the License.

use bytes::Bytes;

use crate::data_type::Int32Type;
use crate::encodings::decoding::{Decoder, DeltaBitPackDecoder};
use crate::errors::{ParquetError, Result};

/// PLENORA: the prefix and suffix lengths of one value, read from the file.
///
/// The prefix reuses that many bytes of the previous value: a negative
/// prefix, or one longer than the previous value, is an encoding that no
/// writer produces, and `truncate` would accept it in silence (a longer
/// prefix keeps the whole previous value, and the value comes out wrong). A
/// negative suffix converted to `usize` overflows the end offset. Returns
/// the prefix length and the end of the suffix in the page.
fn checked_value(
    prefix_length: i32,
    suffix_length: i32,
    previous_length: usize,
    data_offset: usize,
    data_length: usize,
) -> Result<(usize, usize)> {
    let prefix_length = usize::try_from(prefix_length)
        .ok()
        .filter(|prefix| *prefix <= previous_length)
        .ok_or_else(|| {
            general_err!("invalid DELTA_BYTE_ARRAY prefix length: beyond the previous value")
        })?;
    let suffix_length = usize::try_from(suffix_length)
        .map_err(|_| general_err!("invalid DELTA_BYTE_ARRAY suffix length: negative"))?;
    let end = data_offset
        .checked_add(suffix_length)
        .filter(|end| *end <= data_length)
        .ok_or_else(|| ParquetError::EOF("eof decoding byte array".into()))?;
    Ok((prefix_length, end))
}

/// Decoder for `Encoding::DELTA_BYTE_ARRAY`
pub struct DeltaByteArrayDecoder {
    prefix_lengths: Vec<i32>,
    suffix_lengths: Vec<i32>,
    data: Bytes,
    length_offset: usize,
    data_offset: usize,
    last_value: Vec<u8>,
}

impl DeltaByteArrayDecoder {
    /// Create a new [`DeltaByteArrayDecoder`] with the provided data page
    ///
    /// PLENORA: `massimo` is the page value count, which bounds the counts the
    /// two delta headers declare.
    pub fn new(data: Bytes, massimo: usize) -> Result<Self> {
        let mut prefix = DeltaBitPackDecoder::<Int32Type>::new();
        prefix.set_data(data.clone(), massimo)?;

        let num_prefix = prefix.values_left();
        let mut prefix_lengths = vec![0; num_prefix];
        // PLENORA: a short read is an error, not a panic.
        if prefix.get(&mut prefix_lengths)? != num_prefix {
            return Err(eof_err!("eof decoding DELTA_BYTE_ARRAY prefix lengths"));
        }

        let mut suffix = DeltaBitPackDecoder::<Int32Type>::new();
        suffix.set_data(data.slice(prefix.get_offset()..), massimo)?;

        let num_suffix = suffix.values_left();
        let mut suffix_lengths = vec![0; num_suffix];
        if suffix.get(&mut suffix_lengths)? != num_suffix {
            return Err(eof_err!("eof decoding DELTA_BYTE_ARRAY suffix lengths"));
        }

        if num_prefix != num_suffix {
            return Err(general_err!(format!(
                "inconsistent DELTA_BYTE_ARRAY lengths, prefixes: {num_prefix}, suffixes: {num_suffix}"
            )));
        }

        Ok(Self {
            prefix_lengths,
            suffix_lengths,
            data,
            length_offset: 0,
            data_offset: prefix.get_offset() + suffix.get_offset(),
            last_value: vec![],
        })
    }

    /// Returns the number of values remaining
    pub fn remaining(&self) -> usize {
        self.prefix_lengths.len() - self.length_offset
    }

    /// Read up to `len` values, returning the number of values read
    /// and calling `f` with each decoded byte slice
    ///
    /// Will short-circuit and return on error
    pub fn read<F>(&mut self, len: usize, mut f: F) -> Result<usize>
    where
        F: FnMut(&[u8]) -> Result<()>,
    {
        let to_read = len.min(self.remaining());

        let length_range = self.length_offset..self.length_offset + to_read;
        let iter = self.prefix_lengths[length_range.clone()]
            .iter()
            .zip(&self.suffix_lengths[length_range]);

        let data = self.data.as_ref();

        for (prefix_length, suffix_length) in iter {
            let (prefix_length, end) =
                checked_value(*prefix_length, *suffix_length, self.last_value.len(), self.data_offset, data.len())?;

            self.last_value.truncate(prefix_length);
            self.last_value
                .extend_from_slice(&data[self.data_offset..end]);
            f(&self.last_value)?;

            self.data_offset = end;
        }

        self.length_offset += to_read;
        Ok(to_read)
    }

    /// Skip up to `to_skip` values, returning the number of values skipped
    pub fn skip(&mut self, to_skip: usize) -> Result<usize> {
        let to_skip = to_skip.min(self.prefix_lengths.len() - self.length_offset);

        let length_range = self.length_offset..self.length_offset + to_skip;
        let iter = self.prefix_lengths[length_range.clone()]
            .iter()
            .zip(&self.suffix_lengths[length_range]);

        let data = self.data.as_ref();

        for (prefix_length, suffix_length) in iter {
            let (prefix_length, end) =
                checked_value(*prefix_length, *suffix_length, self.last_value.len(), self.data_offset, data.len())?;

            self.last_value.truncate(prefix_length);
            self.last_value
                .extend_from_slice(&data[self.data_offset..end]);
            self.data_offset = end;
        }
        self.length_offset += to_skip;
        Ok(to_skip)
    }
}
