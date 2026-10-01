//! Pubblicazione dello schema al confine (`pubblica_schema`): versione del
//! contratto e identità dei campi (`plenora.field_id`, ARROW-003/004).

use std::collections::{BTreeSet, HashMap};

use plenora_core::arrow::schema::{DataType, Field, Schema};
use plenora_core::contract::arrow_schema::pubblica_schema;
use plenora_core::PlenoraError;

const ID: &str = "plenora.field_id";

fn campo(nome: &str, id: Option<&str>) -> Field {
    let campo = Field::new(nome, DataType::Int64, true);
    match id {
        Some(id) => campo.with_metadata(HashMap::from([(ID.to_owned(), id.to_owned())])),
        None => campo,
    }
}

fn identita(schema: &Schema) -> Vec<String> {
    schema
        .fields()
        .iter()
        .map(|campo| campo.metadata().get(ID).cloned().expect("identita'"))
        .collect()
}

fn insieme(valori: &[u32]) -> BTreeSet<u32> {
    valori.iter().copied().collect()
}

#[test]
fn uno_schema_senza_identita_le_riceve_in_ordine_di_colonna() {
    let schema = Schema::new(vec![campo("a", None), campo("b", None), campo("c", None)]);
    let pubblicato = pubblica_schema(&schema, &BTreeSet::new(), &BTreeSet::new()).expect("ok");
    assert_eq!(identita(&pubblicato), ["0", "1", "2"]);
    assert_eq!(
        pubblicato
            .metadata()
            .get("plenora.contract.version")
            .map(String::as_str),
        Some("1")
    );
    // Deterministica e idempotente: lo schema pubblicato si ripubblica
    // uguale.
    let di_nuovo = pubblica_schema(&schema, &BTreeSet::new(), &BTreeSet::new()).expect("ok");
    assert_eq!(pubblicato, di_nuovo);
    let ripubblicato =
        pubblica_schema(&pubblicato, &insieme(&[0, 1, 2]), &BTreeSet::new()).expect("ok");
    assert_eq!(pubblicato, ripubblicato);
}

#[test]
fn le_identita_portate_restano_byte_per_byte() {
    // La forma `07` resta tale: l'identita' e' quella che la lineage ha
    // portato, non la sua riscrittura.
    let schema = Schema::new(vec![campo("a", Some("07")), campo("b", Some("3"))]);
    let pubblicato = pubblica_schema(&schema, &insieme(&[3, 7]), &BTreeSet::new()).expect("ok");
    assert_eq!(identita(&pubblicato), ["07", "3"]);
}

#[test]
fn una_colonna_nuova_non_prende_mai_un_identita_gia_usata_da_un_ingresso() {
    // L'ingresso aveva 0..=5; l'uscita tiene 2 e aggiunge una colonna: la
    // nuova parte da 6, mai da 0 (che un consumatore leggerebbe come il
    // campo 0 dell'ingresso).
    let schema = Schema::new(vec![campo("tenuta", Some("2")), campo("nuova", None)]);
    let pubblicato =
        pubblica_schema(&schema, &insieme(&[0, 1, 2, 3, 4, 5]), &BTreeSet::new()).expect("ok");
    assert_eq!(identita(&pubblicato), ["2", "6"]);
}

#[test]
fn un_identita_ripetuta_o_ambigua_si_perde_non_si_trasferisce() {
    // Ripetuta nello schema (una colonna duplicata, un self-join): tutti i
    // campi che la portano ne ricevono una nuova.
    let schema = Schema::new(vec![
        campo("a", Some("4")),
        campo("a_right", Some("4")),
        campo("b", Some("1")),
    ]);
    let pubblicato = pubblica_schema(&schema, &insieme(&[1, 4]), &BTreeSet::new()).expect("ok");
    assert_eq!(identita(&pubblicato), ["5", "6", "1"]);
    // Ambigua: dichiarata da due ingressi diversi.
    let schema = Schema::new(vec![campo("x", Some("0")), campo("y", Some("1"))]);
    let pubblicato = pubblica_schema(&schema, &insieme(&[0, 1]), &insieme(&[0])).expect("ok");
    assert_eq!(identita(&pubblicato), ["2", "1"]);
}

#[test]
fn versione_diversa_e_spazio_esaurito_sono_errori() {
    let schema = Schema::new_with_metadata(
        vec![campo("a", None)],
        HashMap::from([("plenora.contract.version".to_owned(), "2".to_owned())]),
    );
    let errore = pubblica_schema(&schema, &BTreeSet::new(), &BTreeSet::new());
    assert!(matches!(errore, Err(PlenoraError::Schema(_))), "{errore:?}");
    let schema = Schema::new(vec![campo("a", None)]);
    let errore = pubblica_schema(&schema, &insieme(&[u32::MAX]), &BTreeSet::new());
    assert!(
        matches!(errore, Err(PlenoraError::Unsupported(_))),
        "{errore:?}"
    );
}
