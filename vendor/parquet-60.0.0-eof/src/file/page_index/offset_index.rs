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

//! [`OffsetIndexMetaData`] structure holding decoded [`OffsetIndex`] information
//!
//! [`OffsetIndex`]: https://github.com/apache/parquet-format/blob/master/PageIndex.md

use std::io::Write;

use crate::parquet_thrift::{
    ElementType, FieldType, ReadThrift, ThriftCompactInputProtocol, ThriftCompactOutputProtocol,
    WriteThrift, WriteThriftField, read_thrift_vec, validate_list_type,
};
use crate::{
    errors::{ParquetError, Result},
    thrift_struct,
};

thrift_struct!(
/// Page location information for [`OffsetIndexMetaData`]
pub struct PageLocation {
  /// Offset of the page in the file
  1: required i64 offset
  /// Size of the page, including header. Sum of compressed_page_size and header
  2: required i32 compressed_page_size
  /// Index within the RowGroup of the first row of the page. When an
  /// OffsetIndex is present, pages must begin on row boundaries
  /// (repetition_level = 0).
  3: required i64 first_row_index
}
);

thrift_struct!(
/// [`OffsetIndex`] information for a column chunk. Contains offsets and sizes for each page
/// in the chunk. Optionally stores fully decoded page sizes for BYTE_ARRAY columns.
///
/// See [`PageIndex`] for more information.
///
/// [`PageIndex`]: crate::file::metadata::page_index::PageIndex
/// [`OffsetIndex`]: https://github.com/apache/parquet-format/blob/master/PageIndex.md
pub struct OffsetIndexMetaData {
  /// Vector of [`PageLocation`] objects, one per page in the chunk.
  1: required list<PageLocation> page_locations
  /// Optional vector of unencoded page sizes, one per page in the chunk.
  /// Only defined for BYTE_ARRAY columns.
  2: optional list<i64> unencoded_byte_array_data_bytes
}
);

impl OffsetIndexMetaData {
    /// Vector of [`PageLocation`] objects, one per page in the chunk.
    pub fn page_locations(&self) -> &Vec<PageLocation> {
        &self.page_locations
    }

    /// Optional vector of unencoded page sizes, one per page in the chunk. Only defined
    /// for BYTE_ARRAY columns.
    pub fn unencoded_byte_array_data_bytes(&self) -> Option<&Vec<i64>> {
        self.unencoded_byte_array_data_bytes.as_ref()
    }

    // Fast-path read of offset index. This works because we expect all field deltas to be 1,
    // and there's no nesting beyond PageLocation, so no need to save the last field id. Like
    // read_page_locations(), this will fail if absolute field id's are used.
    pub(super) fn try_from_fast<'a, R: ThriftCompactInputProtocol<'a>>(
        prot: &mut R,
    ) -> Result<Self> {
        // Offset index is a struct with 2 fields. First field is an array of PageLocations,
        // the second an optional array of i64.

        // read field 1 header, then list header, then vec of PageLocations
        let (field_type, delta) = prot.read_field_header()?;
        if delta != 1 || field_type != FieldType::List as u8 {
            return Err(general_err!("error reading OffsetIndex::page_locations"));
        }

        // we have to do this manually because we want to use the fast PageLocation decoder
        let list_ident = prot.read_list_begin()?;
        validate_list_type(ElementType::Struct, &list_ident)?;
        // PLENORA: capacity bounded by the remaining input bytes.
        let mut page_locations = Vec::with_capacity(crate::parquet_thrift::capacita_dichiarata(
            &*prot,
            list_ident.size,
        )?);
        for _ in 0..list_ident.size {
            page_locations.push(read_page_location(prot)?);
        }

        let mut unencoded_byte_array_data_bytes: Option<Vec<i64>> = None;

        // read second field...if it's Stop we're done
        let (mut field_type, delta) = prot.read_field_header()?;
        if field_type == FieldType::List as u8 {
            if delta != 1 {
                return Err(general_err!(
                    "encountered unknown field while reading OffsetIndex"
                ));
            }
            let vec = read_thrift_vec::<i64, R>(&mut *prot)?;
            unencoded_byte_array_data_bytes = Some(vec);

            // this one should be Stop
            (field_type, _) = prot.read_field_header()?;
        }

        if field_type != FieldType::Stop as u8 {
            return Err(general_err!(
                "encountered unknown field while reading OffsetIndex"
            ));
        }

        Ok(Self {
            page_locations,
            unencoded_byte_array_data_bytes,
        })
    }
}

/// PLENORA: checks the page locations of a column chunk against the chunk:
/// first row indexes not negative, not decreasing and within the rows of the
/// row group, page byte ranges not negative and inside the chunk. The
/// readers subtract and cast these values without checks (a negative or
/// decreasing `first_row_index` wrapped to a huge row count).
pub(crate) fn verifica_posizioni(
    pagine: &[PageLocation],
    righe: u64,
    inizio: u64,
    lunghezza: u64,
) -> Result<()> {
    let fine = inizio
        .checked_add(lunghezza)
        .ok_or_else(|| general_err!("invalid offset index: column chunk out of range"))?;
    let mut riga_precedente = 0u64;
    for pagina in pagine {
        let riga = u64::try_from(pagina.first_row_index)
            .map_err(|_| general_err!("invalid offset index: negative first row index"))?;
        if riga < riga_precedente || riga > righe {
            return Err(general_err!(
                "invalid offset index: first row indexes out of order or beyond the row group"
            ));
        }
        riga_precedente = riga;
        let posizione = u64::try_from(pagina.offset).ok();
        let dimensione = u64::try_from(pagina.compressed_page_size).ok();
        let dentro = match (posizione, dimensione) {
            (Some(posizione), Some(dimensione)) => {
                posizione >= inizio
                    && posizione
                        .checked_add(dimensione)
                        .is_some_and(|termine| termine <= fine)
            }
            _ => false,
        };
        if !dentro {
            return Err(general_err!(
                "invalid offset index: page outside its column chunk"
            ));
        }
    }
    Ok(())
}

// hand coding this one because it is very time critical

// Note: this will fail if the fields are either out of order, or if a suboptimal
// encoder doesn't use field deltas.
fn read_page_location<'a, R: ThriftCompactInputProtocol<'a>>(prot: &mut R) -> Result<PageLocation> {
    // there are 3 fields, all mandatory, so all field deltas should be 1
    let (field_type, delta) = prot.read_field_header()?;
    if delta != 1 || field_type != FieldType::I64 as u8 {
        return Err(general_err!("error reading PageLocation::offset"));
    }
    let offset = prot.read_i64()?;

    let (field_type, delta) = prot.read_field_header()?;
    if delta != 1 || field_type != FieldType::I32 as u8 {
        return Err(general_err!(
            "error reading PageLocation::compressed_page_size"
        ));
    }
    let compressed_page_size = prot.read_i32()?;

    let (field_type, delta) = prot.read_field_header()?;
    if delta != 1 || field_type != FieldType::I64 as u8 {
        return Err(general_err!("error reading PageLocation::first_row_index"));
    }
    let first_row_index = prot.read_i64()?;

    // read end of struct...return error if there are unknown fields present
    let (field_type, _) = prot.read_field_header()?;
    if field_type != FieldType::Stop as u8 {
        return Err(general_err!("unexpected field in PageLocation"));
    }

    Ok(PageLocation {
        offset,
        compressed_page_size,
        first_row_index,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parquet_thrift::tests::test_roundtrip;

    #[test]
    fn test_offset_idx_roundtrip() {
        let page_locations = [
            PageLocation {
                offset: 0,
                compressed_page_size: 10,
                first_row_index: 0,
            },
            PageLocation {
                offset: 10,
                compressed_page_size: 20,
                first_row_index: 100,
            },
        ]
        .to_vec();
        let unenc = [0i64, 100i64].to_vec();

        test_roundtrip(OffsetIndexMetaData {
            page_locations: page_locations.clone(),
            unencoded_byte_array_data_bytes: Some(unenc),
        });
        test_roundtrip(OffsetIndexMetaData {
            page_locations,
            unencoded_byte_array_data_bytes: None,
        });
    }
}
