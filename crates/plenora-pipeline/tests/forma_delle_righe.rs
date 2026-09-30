//! Il fattore di espansione segue la forma delle righe che il catalogo
//! dichiara, allineata a quella che il runner rende: casi che la forma del
//! progetto d'origine (più larga o più stretta del kernel) rifiutava con
//! `ResourceLimit`. La forma di ogni operazione si prova sugli esempi delle
//! schede (`plenora-io/tests/operazioni_doc.rs`, `verifica_forma`), anche su
//! ingressi vuoti; qui restano i casi oltre il fattore di default.

mod comune_geo;

use geo::{Geometry, LineString, MultiLineString};
use plenora_core::catalog::{find_operation, ExpansionConstraint, ResultShape};
use plenora_core::limits::Limits;
use serde_json::json;

use comune_geo::{quadrato, tabella, un_passo, UTM, X0, Y0};

/// Righe oltre il fattore di espansione di default contro una riga.
fn oltre_il_fattore() -> usize {
    let fattore = Limits::default().rows.max_expansion_factor;
    // Il fattore di default è un intero piccolo: la conversione è esatta.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let righe = fattore as usize;
    righe + 20
}

#[test]
fn clip_su_una_maschera_di_una_riga_oltre_il_fattore() {
    let descrittore = find_operation("geo.clip").expect("clip");
    assert_eq!(descrittore.result_shape, Some(ResultShape::OneToOne));
    assert_eq!(
        descrittore.expansion_constraint,
        ExpansionConstraint::LeftRelative
    );
    let righe = oltre_il_fattore();
    let sinistra: Vec<Option<Geometry<f64>>> = (0..righe)
        .map(|riga| {
            #[allow(clippy::cast_precision_loss)] // Poche centinaia di righe.
            let dx = riga as f64 * 20.0;
            Some(Geometry::Polygon(quadrato(X0 + dx, Y0, 10.0)))
        })
        .collect();
    let maschera = [Some(Geometry::Polygon(quadrato(
        X0 - 5.0,
        Y0 - 5.0,
        100_000.0,
    )))];
    // Con `MaxRelative` (la dichiarazione del progetto d'origine) le righe
    // d'uscita contro la riga della maschera superavano il fattore.
    let uscita = un_passo(
        "geo.clip",
        json!({}),
        &[tabella(UTM, &sinistra), tabella(UTM, &maschera)],
    )
    .expect("una riga per riga sinistra");
    assert_eq!(uscita.num_rows(), righe);
}

#[test]
fn line_merge_di_una_riga_in_piu_percorsi_oltre_il_fattore() {
    let descrittore = find_operation("geo.line_merge").expect("line_merge");
    assert_eq!(descrittore.result_shape, Some(ResultShape::WholeToMany));
    assert!(descrittore.expansion_factor_exempt);
    let percorsi = oltre_il_fattore();
    let linee: Vec<LineString<f64>> = (0..percorsi)
        .map(|riga| {
            #[allow(clippy::cast_precision_loss)] // Poche centinaia di righe.
            let scarto = riga as f64 * 10.0;
            let y = Y0 + scarto;
            LineString::from(vec![(X0, y), (X0 + 5.0, y)])
        })
        .collect();
    let una_riga = [Some(Geometry::MultiLineString(MultiLineString::new(linee)))];
    // Una riga, un percorso per linea disgiunta: la forma N:1 dichiarata dal
    // progetto d'origine era più stretta, e il fattore li rifiutava.
    let uscita = un_passo("geo.line_merge", json!({}), &[tabella(UTM, &una_riga)])
        .expect("un percorso per linea");
    assert_eq!(uscita.num_rows(), percorsi);
}

#[test]
fn validate_rules_riepiloga_anche_una_tabella_vuota() {
    use plenora_core::arrow::array::{Int64Array, RecordBatch};
    use plenora_core::arrow::schema::{DataType, Field, Schema};
    use std::sync::Arc;

    assert!(
        find_operation("table.validate_rules")
            .expect("validate_rules")
            .expansion_factor_exempt
    );
    let vuota = RecordBatch::try_new(
        Arc::new(Schema::new(vec![Field::new("v", DataType::Int64, true)])),
        vec![Arc::new(Int64Array::from(Vec::<i64>::new()))],
    )
    .expect("tabella vuota");
    let config = json!({"output_mode": "summary", "rules": [
        {"name": "positivo", "operator": "gt", "column": "v", "value": 0},
        {"name": "presente", "operator": "notnull", "column": "v"}]});
    // Una riga per regola, conteggi a zero: prima il fattore di espansione
    // (due righe da nessuna) la rifiutava.
    let uscita = un_passo("table.validate_rules", config, &[vuota]).expect("riepilogo");
    assert_eq!(uscita.num_rows(), 2);
}

/// `geo.union` dichiarava `SumRelative`: una riga per lato dà
/// `1 / (1 + 1) = 0,5`, e con `max_expansion_factor` 0,75 il passo era
/// accettato. Con `LeftRelative` il rapporto è 1 e si rifiuta (semantica 2).
/// `geo.difference` dichiarava `MaxRelative`, che su lati di righe uguali
/// decide come `LeftRelative`: rifiutata prima e dopo (semantica 1).
#[test]
fn union_con_fattore_sotto_uno_ora_si_rifiuta_come_le_altre_booleane() {
    use comune_geo::{esegui, passo, piano};
    use plenora_core::ErrorCategory;
    use plenora_pipeline::LimitiParziali;

    let sinistra = tabella(UTM, &[Some(Geometry::Polygon(quadrato(X0, Y0, 10.0)))]);
    let destra = tabella(
        UTM,
        &[Some(Geometry::Polygon(quadrato(X0 + 5.0, Y0, 10.0)))],
    );
    for (op, semantica) in [("geo.union", 2), ("geo.difference", 1)] {
        let descrittore = find_operation(op).expect(op);
        assert_eq!(
            descrittore.expansion_constraint,
            ExpansionConstraint::LeftRelative
        );
        assert_eq!(descrittore.semantic_version, semantica, "{op}");
        let mut pipeline = piano(
            &["t", "u"],
            vec![passo("x", op, &["t", "u"], json!({}))],
            &["x"],
        );
        pipeline.limits = Some(LimitiParziali {
            max_expansion_factor: Some(0.75),
            ..LimitiParziali::default()
        });
        let errore = esegui(&pipeline, &[("t", sinistra.clone()), ("u", destra.clone())])
            .expect_err("rapporto 1 oltre 0,75");
        assert_eq!(errore.category(), ErrorCategory::ResourceLimit, "{op}");
        // Con il fattore a 1 lo stesso passo gira.
        pipeline.limits = Some(LimitiParziali {
            max_expansion_factor: Some(1.0),
            ..LimitiParziali::default()
        });
        let esito = esegui(&pipeline, &[("t", sinistra.clone()), ("u", destra.clone())])
            .expect("rapporto 1");
        assert_eq!(esito.outputs[0].1.num_rows(), 1, "{op}");
    }
}
