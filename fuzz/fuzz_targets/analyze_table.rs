#![no_main]

//! `analyze_contract` dei kernel tabellari: config arbitrarie su
//! contratti di input sintetici. La config base valida di ogni operazione
//! viene fusa con un oggetto JSON derivato dal payload (chiavi arbitrarie con
//! valori arbitrari): l'inferenza deve sempre terminare con `Ok` o un errore
//! tipizzato, mai panic. Invariante extra: il contratto prodotto deve
//! superare la propria validazione strutturale (`DataContract::new` e'
//! gia' fail-closed) e lo schema non deve avere nomi duplicati.

use std::collections::HashSet;
use std::sync::Arc;

use arrow_schema::{DataType, Field, Schema};
use libfuzzer_sys::fuzz_target;
use plenora_core::contract::{DataContract, FieldAllocator};
use plenora_kernels_table::analyze::analyze_table_contract;
use serde_json::{json, Map, Value};

fn contract() -> DataContract {
    DataContract::tabular(Arc::new(Schema::new(vec![
        Field::new("id", DataType::Int64, false),
        Field::new("num", DataType::Float64, true),
        Field::new("flag", DataType::Boolean, true),
        Field::new("group", DataType::Utf8, true),
        Field::new("text", DataType::Utf8, true),
        Field::new("date", DataType::Utf8, true),
    ])))
}

/// (op, arieta', config base valida) — copre famiglie unarie, binarie
/// ordinate e N-arie del catalogo `table.*`.
fn cases() -> Vec<(&'static str, usize, Value)> {
    vec![
        ("fill_na", 1, json!({"column":"text","method":"constant","value":"x"})),
        ("replace", 1, json!({"column":"text","old_value":"a","new_value":"b","regex":false})),
        ("type_cast", 1, json!({"column":"text","target_type":"float","errors":"coerce"})),
        ("filter", 1, json!({"column":"num","operator":">=","value":0})),
        ("conditional", 1, json!({"column":"num","conditions":[{"operator":">","value":0,"result":"hi"}],"default_value":"lo"})),
        ("string_extract", 1, json!({"column":"text","pattern":"(.)","output_column":"first"})),
        ("lookup", 1, json!({"column":"group","mapping":{"a":"A"},"default":"B"})),
        ("md5_hash", 1, json!({"columns":["group","text"],"output_column":"hash"})),
        ("add_row_number", 1, json!({"output_column":"row"})),
        ("bin", 1, json!({"column":"num","bins":4})),
        ("sample", 1, json!({"n":8,"random_state":1})),
        ("statistics", 1, json!({"column":"num","stats":["count","mean"]})),
        ("sort", 1, json!({"columns":["group","num"]})),
        ("distinct", 1, json!({"subset":["text"],"keep":"first"})),
        ("dedup_advanced", 1, json!({"subset":["text"],"keep":"last","order_column":"num"})),
        ("aggregate", 1, json!({"group_by":["group"],"aggregations":[{"column":"num","function":"sum","alias":"total"}]})),
        ("window_function", 1, json!({"column":"num","function":"rank","group_by":"group"})),
        ("formula", 1, json!({"new_column":"calc","formula":"num * 2 + 1"})),
        ("coalesce", 1, json!({"columns":["num"],"output_column":"out"})),
        ("rolling_window", 1, json!({"column":"num","function":"mean","window":4})),
        ("assert_unique", 1, json!({"columns":["id"]})),
        ("assert_range", 1, json!({"column":"id","min":0.0,"max":1000.0})),
        ("assert_regex", 1, json!({"column":"group","pattern":"^."})),
        ("date_format", 1, json!({"column":"date","input_format":"%Y-%m-%d","output_format":"%d/%m/%Y"})),
        ("date_add", 1, json!({"column":"date","input_format":"%Y-%m-%d","amount":1,"unit":"days"})),
        ("text_normalize", 1, json!({"columns":["text"],"operations":"full"})),
        ("string_pad", 1, json!({"column":"text","width":8,"side":"right","fill_char":" "})),
        ("string_length", 1, json!({"column":"text","output_column":"len"})),
        ("rename", 1, json!({"renames":[{"old_name":"text","new_name":"label"}]})),
        ("expression", 1, json!({"output_column":"out","expression":{"kind":"column","name":"num"}})),
        ("melt", 1, json!({"id_columns":["id"],"value_columns":["num"]})),
        ("join", 2, json!({"left_keys":["id"],"right_keys":["id"],"how":"inner"})),
        ("semi_join", 2, json!({"left_keys":["id"],"right_keys":["id"]})),
        ("anti_join", 2, json!({"left_keys":["id"],"right_keys":["id"]})),
        ("asof_join", 2, json!({"left_on":"id","right_on":"id","direction":"backward"})),
        ("union_distinct", 2, json!({})),
        ("intersect", 2, json!({})),
        ("except", 2, json!({})),
        ("cross_join", 2, json!({})),
        ("table_diff", 2, json!({"left_keys":["id"],"right_keys":["id"]})),
        ("reconcile", 2, json!({"left_keys":["id"],"right_keys":["id"]})),
        ("assert_foreign_key", 2, json!({"left_keys":["id"],"right_keys":["id"]})),
        ("concat", 3, json!({"ignore_index":true})),
    ]
}

/// Fonde le chiavi di un oggetto JSON arbitrario sopra la config base:
/// valori di tipo sbagliato, chiavi ignote, stringhe enormi sono esattamente
/// cio' che la validazione fail-closed deve rifiutare senza panicare.
fn merge(base: &Value, patch: Option<Value>) -> Value {
    let (Some(Value::Object(patch)), Value::Object(base)) = (patch, base) else {
        return base.clone();
    };
    let mut merged: Map<String, Value> = base.clone();
    for (key, value) in patch {
        // Chiavi di lunghezza limitata: gli identificatori enormi restano
        // coperti dai valori, non dai nomi chiave (deny_unknown_fields le
        // rifiuterebbe comunque a priori).
        if key.len() <= 64 {
            merged.insert(key, value);
        }
    }
    Value::Object(merged)
}

fuzz_target!(|payload: &[u8]| {
    let table = cases();
    let selector = payload.first().copied().unwrap_or_default() as usize;
    let (op, arity, base) = &table[selector % table.len()];
    let patch = serde_json::from_slice::<Value>(payload.get(1..).unwrap_or(&[])).ok();
    let config = merge(base, patch);
    let input = contract();
    let inputs: Vec<DataContract> = (0..*arity).map(|_| input.clone()).collect();
    let mut fields = FieldAllocator::default();
    if let Ok(output) = analyze_table_contract(op, &inputs, &config, &mut fields) {
        let names: HashSet<&str> = output
            .schema
            .fields()
            .iter()
            .map(|field| field.name().as_str())
            .collect();
        assert_eq!(
            names.len(),
            output.schema.fields().len(),
            "schema con nomi duplicati da {op}"
        );
    }
});
