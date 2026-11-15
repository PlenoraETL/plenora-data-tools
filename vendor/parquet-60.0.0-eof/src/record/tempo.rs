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

//! Checked calendar and time-of-day formatting for external record values.

use crate::errors::{ParquetError, Result};
use chrono::{DateTime, Utc};

pub(super) fn invalid() -> ParquetError {
    ParquetError::General("temporal value outside supported domain".into())
}

pub(super) fn date(value: i32) -> Result<String> {
    let seconds = i64::from(value).checked_mul(86_400).ok_or_else(invalid)?;
    let dt = DateTime::<Utc>::from_timestamp(seconds, 0).ok_or_else(invalid)?;
    Ok(dt.format("%Y-%m-%d").to_string())
}

pub(super) fn timestamp_millis(value: i64) -> Result<String> {
    let dt = DateTime::<Utc>::from_timestamp_millis(value).ok_or_else(invalid)?;
    Ok(dt.format("%Y-%m-%d %H:%M:%S%.3f %:z").to_string())
}

pub(super) fn timestamp_micros(value: i64) -> Result<String> {
    let dt = DateTime::<Utc>::from_timestamp_micros(value).ok_or_else(invalid)?;
    Ok(dt.format("%Y-%m-%d %H:%M:%S%.6f %:z").to_string())
}

pub(super) fn time_millis(value: i32) -> Result<String> {
    time(i64::from(value), 1_000, 3)
}

pub(super) fn time_micros(value: i64) -> Result<String> {
    time(value, 1_000_000, 6)
}

fn time(value: i64, scale: i64, width: usize) -> Result<String> {
    if !(0..86_400 * scale).contains(&value) {
        return Err(invalid());
    }
    let seconds = value / scale;
    Ok(format!(
        "{:02}:{:02}:{:02}.{:0width$}",
        seconds / 3600,
        seconds / 60 % 60,
        seconds % 60,
        value % scale
    ))
}
