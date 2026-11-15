//! Valori esterni ai limiti del calendario e barriera INT96 prima dei dati.
mod comune;
#[macro_use]
#[path = "../../../tests/comune/esito.rs"]
mod esito;
use parquet::file::reader::{FileReader, SerializedFileReader};
use parquet::{
    errors::ParquetError,
    record::{Field, Row},
};
use plenora_core::{ErrorCategory, ErrorPhase};
use std::{fs::File, sync::Arc};

fn rifiuto_json(field: &Field) {
    let Err(ParquetError::General(message)) = field.to_json_value() else {
        panic!("atteso rifiuto temporale tipizzato");
    };
    assert_eq!(message, "temporal value outside supported domain");
    assert_eq!(field.to_string(), "<invalid temporal value>");
}

#[test]
fn record_calendario_e_orari_fuori_dominio() {
    for field in [
        Field::Date(i32::MIN),
        Field::Date(i32::MAX),
        Field::TimestampMillis(i64::MIN),
        Field::TimestampMillis(i64::MAX),
        Field::TimestampMicros(i64::MIN),
        Field::TimestampMicros(i64::MAX),
        Field::TimeMillis(i32::MIN),
        Field::TimeMillis(-1),
        Field::TimeMillis(86_400_000),
        Field::TimeMillis(i32::MAX),
        Field::TimeMicros(i64::MIN),
        Field::TimeMicros(-1),
        Field::TimeMicros(86_400_000_000),
        Field::TimeMicros(i64::MAX),
    ] {
        rifiuto_json(&field);
        let row = Row::new(vec![("campo".into(), field)]);
        let Err(ParquetError::General(message)) = row.to_json_value() else {
            panic!("atteso rifiuto annidato");
        };
        assert_eq!(message, "temporal value outside supported domain");
    }
    for (field, text) in [
        (Field::TimeMillis(0), "00:00:00.000"),
        (Field::TimeMillis(86_399_999), "23:59:59.999"),
        (Field::TimeMicros(0), "00:00:00.000000"),
        (Field::TimeMicros(86_399_999_999), "23:59:59.999999"),
        (Field::Date(-719_162), "0001-01-01"),
        (Field::Date(2_932_896), "9999-12-31"),
        (Field::Date(2_932_897), "+10000-01-01"),
    ] {
        assert_eq!(field.to_string(), text);
        assert_successo!(
            field.to_json_value(),
            serde_json::Value::String(text.into())
        );
    }
}

#[test]
fn record_limiti_chrono_e_un_tick_oltre() {
    let min = chrono::NaiveDateTime::MIN.and_utc();
    let max = chrono::NaiveDateTime::MAX.and_utc();
    for field in [
        Field::TimestampMillis(min.timestamp_millis() - 1),
        Field::TimestampMillis(max.timestamp_millis() + 1),
        Field::TimestampMicros(min.timestamp_micros() - 1),
        Field::TimestampMicros(max.timestamp_micros() + 1),
    ] {
        rifiuto_json(&field);
    }
    for (value, format) in [
        (min, "%Y-%m-%d %H:%M:%S%.6f %:z"),
        (max, "%Y-%m-%d %H:%M:%S%.6f %:z"),
    ] {
        let field = Field::TimestampMicros(value.timestamp_micros());
        let text = value.format(format).to_string();
        assert_eq!(field.to_string(), text);
        assert_successo!(field.to_json_value(), serde_json::Value::String(text));
    }
}

#[test]
fn int96_estremi_statistiche_e_gruppi_sono_rifiutati() {
    use parquet::{
        basic::{Repetition, Type},
        data_type::{Int96, Int96Type},
        file::{properties::WriterProperties, writer::SerializedFileWriter},
        schema::types::Type as SchemaType,
    };
    let dir = comune::cartella();
    let path = dir.path().join("estremi.parquet");
    let schema = Arc::new(
        SchemaType::group_type_builder("schema")
            .with_fields(vec![Arc::new(
                SchemaType::primitive_type_builder("segreto", Type::INT96)
                    .with_repetition(Repetition::REQUIRED)
                    .build()
                    .unwrap(),
            )])
            .build()
            .unwrap(),
    );
    // Anche lo schema senza row group deve essere rifiutato.
    for groups in [0, 2] {
        let mut writer = SerializedFileWriter::new(
            File::create(&path).unwrap(),
            schema.clone(),
            Arc::new(WriterProperties::builder().build()),
        )
        .unwrap();
        for _ in 0..groups {
            let mut group = writer.next_row_group().unwrap();
            let mut col = group.next_column().unwrap().unwrap();
            let values = [
                [0, 0, 0],
                [u32::MAX, u32::MAX, u32::MAX],
                [0, 0, 5_373_484],
                [0, 0, 2_440_588],
                [0, u32::MAX, 2_440_588],
            ]
            .map(|v| {
                let mut value = Int96::new();
                value.set_data(v[0], v[1], v[2]);
                value
            });
            col.typed::<Int96Type>()
                .write_batch(&values, None, None)
                .unwrap();
            col.close().unwrap();
            group.close().unwrap();
        }
        writer.close().unwrap();
        let reader = SerializedFileReader::new(File::open(&path).unwrap()).unwrap();
        assert_eq!(reader.metadata().num_row_groups(), groups);
        for group in reader.metadata().row_groups() {
            assert!(group.column(0).statistics().is_some());
        }
        let error = assert_rifiuto!(
            plenora_io::leggi_tabella(&path, None, u64::MAX),
            ErrorCategory::Unsupported,
            ErrorPhase::Validate
        );
        assert!(!error.to_string().contains("segreto"));
    }
}

#[test]
fn arrow_estremi_conservano_unita_e_valore_nel_file() {
    use plenora_core::arrow::{
        array::{
            ArrayRef, RecordBatch, TimestampMicrosecondArray, TimestampMillisecondArray,
            TimestampNanosecondArray, TimestampSecondArray,
        },
        schema::{Field as ArrowField, Schema},
    };
    let dir = comune::cartella();
    let path = dir.path().join("timestamp.parquet");
    let values = vec![i64::MIN, -9_223_372_036_855, 0, 9_223_372_036_855, i64::MAX];
    let arrays: Vec<ArrayRef> = vec![
        Arc::new(TimestampSecondArray::from(values.clone())),
        Arc::new(TimestampMillisecondArray::from(values.clone())),
        Arc::new(TimestampMicrosecondArray::from(values.clone())),
        Arc::new(TimestampNanosecondArray::from(values)),
    ];
    for array in arrays {
        let schema = Arc::new(Schema::new(vec![ArrowField::new(
            "t",
            array.data_type().clone(),
            false,
        )]));
        let batch = RecordBatch::try_new(schema, vec![array]).unwrap();
        let mut writer = parquet::arrow::ArrowWriter::try_new(
            File::create(&path).unwrap(),
            batch.schema(),
            None,
        )
        .unwrap();
        writer.write(&batch).unwrap();
        writer.close().unwrap();
        assert_successo!(plenora_io::leggi_tabella(&path, None, u64::MAX), batch);
    }
}
