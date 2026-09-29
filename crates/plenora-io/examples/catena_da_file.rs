//! Una catena di piani da file a file, con i tempi e la memoria per fase.
//!
//! ```text
//! cargo run --release -p plenora-io --example catena_da_file -- \
//!     catena.json cartella_uscite nome=percorso.parquet [nome=percorso ...]
//! ```
//!
//! `catena.json` e' `{"piani": [piano, ...]}`: piani del runner eseguiti in
//! sequenza, ognuno sugli output del precedente (il primo sui file). Il
//! testo si controlla per chiavi ripetute prima di ogni lettura serde (come
//! `Pipeline::from_json`), e l'oggetto ha solo `piani`.
//!
//! Ogni output dell'ultimo piano si scrive in `cartella_uscite/<nome>.parquet`:
//! nomi di soli `[A-Za-z0-9_-]`, distinti anche senza distinzione di
//! maiuscole (file system di Windows), nessun percorso uguale a un ingresso.
//! Tutto si verifica prima di leggere.
//!
//! Come `esegui_da_file` (ogni input letto con il budget residuo, byte vivi
//! esatti dopo ogni lettura), con in piu' i tempi di lettura, catena e
//! scrittura e il resoconto del runner passo per passo, in JSON su stdout.
//! Nessun valore dei dati esce: solo nomi, tipi, conteggi, byte e tempi.
//!
//! Una conversione fuori dal piano, dichiarata nel resoconto: le colonne
//! `Timestamp(Microsecond)` diventano `Timestamp(Millisecond)`, l'unica
//! unita' che i kernel tabellari leggono come testo. Si rifiuta se un valore
//! non e' un multiplo esatto di 1000 microsecondi (nessun troncamento).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use plenora_core::arrow::array::{
    Array, ArrayRef, RecordBatch, TimestampMicrosecondArray, TimestampMillisecondArray,
};
use plenora_core::arrow::schema::{DataType, Field, Schema, TimeUnit};
use plenora_core::{PlenoraError, Result};
use plenora_io::{leggi_tabella, scrivi_tabella, OpzioniScrittura};
use plenora_pipeline::{byte_vivi, Pipeline};
use serde_json::{json, Value};

/// I piani della catena dal testo di `catena.json`.
///
/// # Errors
///
/// `InvalidPlan` per chiavi ripetute a qualunque profondita', JSON non
/// valido, chiavi diverse da `piani`, piani non validi (`Pipeline::from_json`).
fn leggi_catena(testo: &str) -> Result<Vec<Pipeline>> {
    // Prima di ogni lettura serde, che terrebbe in silenzio l'ultima di due
    // chiavi ripetute (anche nelle config dei passi).
    plenora_core::json::ensure_no_duplicate_keys(testo)?;
    let catena: Value = serde_json::from_str(testo)
        .map_err(|_| PlenoraError::InvalidPlan("catena.json non e' JSON".into()))?;
    let oggetto = catena
        .as_object()
        .ok_or_else(|| PlenoraError::InvalidPlan("catena.json non e' un oggetto".into()))?;
    if oggetto.keys().any(|chiave| chiave != "piani") {
        return Err(PlenoraError::InvalidPlan(
            "catena.json: ammessa solo la chiave `piani`".into(),
        ));
    }
    oggetto
        .get("piani")
        .and_then(Value::as_array)
        .ok_or_else(|| PlenoraError::InvalidPlan("catena.json senza `piani`".into()))?
        .iter()
        .map(|piano| {
            serde_json::to_string(piano)
                .map_err(|_| PlenoraError::InvalidPlan("piano non serializzabile".into()))
                .and_then(|testo| Pipeline::from_json(&testo))
        })
        .collect()
}

/// Un file per output nella cartella, verificati prima di leggere.
///
/// # Errors
///
/// `InvalidPlan` per un nome vuoto o con caratteri fuori da `[A-Za-z0-9_-]`,
/// due nomi uguali senza distinzione di maiuscole, un percorso d'uscita
/// uguale a un ingresso.
fn percorsi_uscita(
    cartella: &Path,
    nomi: &[String],
    ingressi: &[&Path],
) -> Result<Vec<(String, PathBuf)>> {
    let canonico = |percorso: &Path| {
        std::fs::canonicalize(percorso).unwrap_or_else(|_| percorso.to_path_buf())
    };
    let mut visti = std::collections::HashSet::new();
    let mut uscite = Vec::with_capacity(nomi.len());
    for nome in nomi {
        let ammesso =
            |carattere: char| carattere.is_ascii_alphanumeric() || matches!(carattere, '_' | '-');
        if nome.is_empty() || !nome.chars().all(ammesso) {
            return Err(PlenoraError::InvalidPlan(
                "nome di output non utilizzabile come nome di file".into(),
            ));
        }
        if !visti.insert(nome.to_ascii_lowercase()) {
            return Err(PlenoraError::InvalidPlan(
                "due output con lo stesso file (maiuscole a parte)".into(),
            ));
        }
        let percorso = cartella.join(format!("{nome}.parquet"));
        let destinazione = canonico(&percorso);
        if ingressi
            .iter()
            .any(|ingresso| canonico(ingresso) == destinazione)
        {
            return Err(PlenoraError::InvalidPlan(
                "un percorso d'uscita coincide con un ingresso".into(),
            ));
        }
        uscite.push((nome.clone(), percorso));
    }
    Ok(uscite)
}

fn in_millisecondi(tabella: &RecordBatch) -> Result<(RecordBatch, Vec<String>)> {
    let mut convertite = Vec::new();
    let mut campi = Vec::with_capacity(tabella.num_columns());
    let mut colonne: Vec<ArrayRef> = Vec::with_capacity(tabella.num_columns());
    for (campo, colonna) in tabella.schema().fields().iter().zip(tabella.columns()) {
        if let DataType::Timestamp(TimeUnit::Microsecond, fuso) = campo.data_type() {
            let micro = colonna
                .as_any()
                .downcast_ref::<TimestampMicrosecondArray>()
                .ok_or_else(|| PlenoraError::Internal("timestamp incoerente".into()))?;
            let milli = micro
                .iter()
                .map(|valore| {
                    valore
                        .map(|us| {
                            if us % 1000 == 0 {
                                Ok(us / 1000)
                            } else {
                                Err(PlenoraError::DataMapping(format!(
                                    "colonna `{}`: timestamp non intero in millisecondi",
                                    campo.name()
                                )))
                            }
                        })
                        .transpose()
                })
                .collect::<Result<TimestampMillisecondArray>>()?
                .with_timezone_opt(fuso.clone());
            campi.push(
                Field::new(
                    campo.name(),
                    DataType::Timestamp(TimeUnit::Millisecond, fuso.clone()),
                    campo.is_nullable(),
                )
                .with_metadata(campo.metadata().clone()),
            );
            colonne.push(Arc::new(milli));
            convertite.push(campo.name().clone());
        } else {
            campi.push(campo.as_ref().clone());
            colonne.push(Arc::clone(colonna));
        }
    }
    let schema = Schema::new_with_metadata(campi, tabella.schema().metadata().clone());
    Ok((RecordBatch::try_new(Arc::new(schema), colonne)?, convertite))
}

fn millis(inizio: Instant) -> u128 {
    inizio.elapsed().as_millis()
}

/// Resoconto di un piano: righe per passo con la previsione e i byte vivi.
fn resoconto_passi(report: &plenora_pipeline::Report, piano: usize) -> Vec<Value> {
    let mut vivi_prima = report.byte_vivi_iniziali;
    report
        .passi
        .iter()
        .map(|passo| {
            let riga = json!({
                "piano": piano, "out": passo.out, "op": passo.op,
                "righe_in": passo.righe_in, "righe_out": passo.righe_out,
                "variante": format!("{:?}", passo.variante),
                "byte_vivi_prima": vivi_prima,
                "byte_previsti": passo.byte_previsti,
                "previsione_totale": vivi_prima.saturating_add(passo.byte_previsti),
                "byte_vivi_con_uscita": passo.byte_vivi_con_uscita,
                "byte_output_esclusivi": passo.byte_output_esclusivi,
                "byte_vivi_dopo": passo.byte_vivi,
                "margine_kernel": passo.margine_kernel,
                "sfrattati": passo.sfrattati, "ricaricati": passo.ricaricati,
            });
            vivi_prima = passo.byte_vivi;
            riga
        })
        .collect()
}

// Sequenza lineare lettura, piani, scrittura, resoconto: spezzarla
// peggiora la lettura.
#[allow(clippy::too_many_lines)]
fn main() -> Result<()> {
    let argomenti: Vec<String> = std::env::args().skip(1).collect();
    let [catena, cartella_uscite, coppie @ ..] = argomenti.as_slice() else {
        return Err(PlenoraError::InvalidPlan(
            "uso: catena_da_file catena.json cartella_uscite nome=percorso ...".into(),
        ));
    };
    let avvio = Instant::now();
    let piani = leggi_catena(&std::fs::read_to_string(catena)?)?;
    let coppie = coppie
        .iter()
        .map(|coppia| {
            coppia
                .split_once('=')
                .ok_or_else(|| PlenoraError::InvalidPlan("atteso nome=percorso".into()))
        })
        .collect::<Result<Vec<_>>>()?;
    let percorsi_ingresso: Vec<&Path> = coppie
        .iter()
        .map(|(_, percorso)| Path::new(*percorso))
        .collect();
    let destinazioni = percorsi_uscita(
        Path::new(cartella_uscite),
        piani
            .last()
            .map_or(&[][..], |piano| piano.outputs.as_slice()),
        &percorsi_ingresso,
    )?;
    let budget_di = |piano: &Pipeline| -> Result<u64> {
        Ok(piano
            .limits
            .as_ref()
            .map_or_else(
                || Ok(plenora_core::limits::Limits::default()),
                plenora_pipeline::LimitiParziali::applica,
            )?
            .max_governed_memory_bytes)
    };
    let budget = piani.first().map_or(Ok(0), budget_di)?;

    let inizio_lettura = Instant::now();
    let mut tabelle: Vec<(String, RecordBatch)> = Vec::new();
    let mut letture = Vec::new();
    for (nome, percorso) in coppie {
        let vivi = byte_vivi(tabelle.iter().map(|(_, tabella)| tabella))?;
        let inizio = Instant::now();
        let letta = leggi_tabella(Path::new(percorso), None, budget.saturating_sub(vivi))?;
        let ms_lettura = millis(inizio);
        let inizio = Instant::now();
        let (letta, convertite) = in_millisecondi(&letta)?;
        let ms_conversione = millis(inizio);
        letture.push(json!({
            "nome": nome, "righe": letta.num_rows(), "colonne": letta.num_columns(),
            "byte": byte_vivi([&letta])?, "ms_lettura": ms_lettura,
            "ms_conversione_timestamp": ms_conversione, "timestamp_convertiti": convertite,
        }));
        tabelle.push((nome.to_owned(), letta));
        let vivi = byte_vivi(tabelle.iter().map(|(_, tabella)| tabella))?;
        if vivi > budget {
            return Err(PlenoraError::ResourceLimit(format!(
                "input `{nome}` oltre il budget"
            )));
        }
    }
    let ms_lettura = millis(inizio_lettura);
    let vivi_input = byte_vivi(tabelle.iter().map(|(_, tabella)| tabella))?;
    eprintln!("[{} ms] inizio catena", millis(avvio));

    // I piani in sequenza: ogni piano prende i suoi input dalle tabelle
    // correnti e le sostituisce con i suoi output. Fra un piano e l'altro
    // non resta niente fuori dal piano successivo (il traduttore fa passare
    // ogni tabella viva come input e output).
    let inizio_catena = Instant::now();
    let mut passi = Vec::new();
    let mut piani_json = Vec::new();
    for (indice, piano) in piani.iter().enumerate() {
        let inizio = Instant::now();
        let mut ingressi = Vec::with_capacity(piano.inputs.len());
        for nome in &piano.inputs {
            let posizione = tabelle
                .iter()
                .position(|(candidato, _)| candidato == nome)
                .ok_or_else(|| PlenoraError::InvalidPlan(format!("input `{nome}` assente")))?;
            ingressi.push(tabelle.swap_remove(posizione));
        }
        if !tabelle.is_empty() {
            return Err(PlenoraError::InvalidPlan(format!(
                "piano {}: tabelle vive fuori dal piano",
                indice + 1
            )));
        }
        let schemi: Vec<(&str, _)> = ingressi
            .iter()
            .map(|(nome, tabella)| (nome.as_str(), tabella.schema()))
            .collect();
        let validata = piano.validate(&schemi)?;
        drop(schemi);
        let ms_validazione = millis(inizio);
        let esito = validata.run(ingressi)?;
        passi.extend(resoconto_passi(&esito.report, indice + 1));
        piani_json.push(json!({
            "piano": indice + 1, "budget": budget_di(piano)?,
            "byte_vivi_iniziali": esito.report.byte_vivi_iniziali,
            "byte_su_disco_massimi": esito.report.byte_su_disco_massimi,
            "ms_validazione": ms_validazione, "ms_totale": millis(inizio),
        }));
        tabelle = esito.outputs;
    }
    let ms_catena = millis(inizio_catena);
    eprintln!("[{} ms] inizio scrittura", millis(avvio));

    let inizio_scrittura = Instant::now();
    let mut uscite = Vec::new();
    for (nome, tabella) in &tabelle {
        let (_, file_uscita) = destinazioni
            .iter()
            .find(|(candidato, _)| candidato == nome)
            .ok_or_else(|| PlenoraError::Internal("output senza percorso".into()))?;
        scrivi_tabella(
            tabella,
            file_uscita,
            &OpzioniScrittura {
                sovrascrivi: true,
                ..OpzioniScrittura::default()
            },
        )?;
        let campi: Vec<Value> = tabella
            .schema()
            .fields()
            .iter()
            .map(|campo| json!([campo.name(), format!("{}", campo.data_type())]))
            .collect();
        uscite.push(json!({"nome": nome, "righe": tabella.num_rows(), "campi": campi}));
    }
    let ms_scrittura = millis(inizio_scrittura);
    eprintln!("[{} ms] fine", millis(avvio));

    let resoconto = json!({
        "budget": budget,
        "ms": {"lettura": ms_lettura, "catena": ms_catena, "scrittura": ms_scrittura,
               "totale": millis(avvio)},
        "byte_vivi_input": vivi_input,
        "letture": letture, "piani": piani_json, "passi": passi, "uscite": uscite,
    });
    println!(
        "{}",
        serde_json::to_string_pretty(&resoconto)
            .map_err(|_| PlenoraError::Internal("resoconto JSON".into()))?
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const PIANO: &str = r#"{"version": 1, "inputs": ["t"], "steps": [
        {"out": "a", "op": "table.limit", "in": ["t"], "config": {"n": 1}},
        {"out": "b", "op": "table.limit", "in": ["t"], "config": {"n": 2}}],
        "outputs": ["a", "b"]}"#;

    fn catena(piano: &str) -> String {
        format!(r#"{{"piani": [{piano}]}}"#)
    }

    #[test]
    fn le_chiavi_ripetute_si_rifiutano_prima_di_leggere() {
        assert_eq!(
            leggi_catena(&catena(PIANO)).map(|piani| piani.len()).ok(),
            Some(1)
        );
        let ripetute = [
            // Nella config di un passo, nel piano e nella catena.
            catena(&PIANO.replace(r#""config": {"n": 1}"#, r#""config": {"n": 1, "n": 3}"#)),
            catena(&PIANO.replace(r#""version": 1,"#, r#""version": 1, "version": 1,"#)),
            format!(r#"{{"piani": [{PIANO}], "piani": []}}"#),
            format!(r#"{{"piani": [{PIANO}], "altro": 1}}"#),
        ];
        for testo in ripetute {
            let errore = leggi_catena(&testo).expect_err("rifiuto");
            assert!(matches!(errore, PlenoraError::InvalidPlan(_)), "{errore}");
        }
    }

    #[test]
    fn ogni_output_ha_il_suo_file() {
        let cartella = Path::new("uscite");
        let nomi = ["a".to_owned(), "b".to_owned()];
        let uscite = percorsi_uscita(cartella, &nomi, &[]).expect("due output");
        assert_eq!(
            uscite,
            [
                ("a".to_owned(), cartella.join("a.parquet")),
                ("b".to_owned(), cartella.join("b.parquet"))
            ]
        );
        for nomi in [
            vec!["a".to_owned(), "A".to_owned()],
            vec![String::new()],
            vec!["../fuori".to_owned()],
            vec!["con spazio".to_owned()],
        ] {
            assert!(matches!(
                percorsi_uscita(cartella, &nomi, &[]),
                Err(PlenoraError::InvalidPlan(_))
            ));
        }
        let ingresso = cartella.join("a.parquet");
        assert!(matches!(
            percorsi_uscita(cartella, &["a".to_owned()], &[ingresso.as_path()]),
            Err(PlenoraError::InvalidPlan(_))
        ));
    }
}
