//! Il corpo comune dei target di lettura: i byte del payload scritti in un
//! file temporaneo e letti dal confine di `plenora-io` nel formato dato.
//!
//! Invarianti: mai panico; un rifiuto non è `Internal` né contiene la
//! sentinella; due letture dello stesso file danno lo stesso esito su ogni
//! asse; una tabella letta, se la scrittura la accetta, riletta è:
//! - per Arrow IPC, identica, scritta sia come file sia come stream;
//! - per Parquet, identica se non ha colonne geometriche; altrimenti la
//!   trasformazione GeoParquet documentata (`geoparquet::prepara`, poi
//!   `geoparquet::applica` sul metadato di file), che riscrive i metadati
//!   dei campi geometria e lascia i valori.

use std::io::Write;
use std::path::Path;

use plenora_core::arrow::RecordBatch;
use plenora_io::{
    geoparquet, leggi_tabella_con_limiti, scrivi_tabella, CompressioneParquet, Formato,
    LimitiLettura, OpzioniScrittura,
};

/// Budget della tabella letta: abbastanza per i file del corpus, abbastanza
/// stretto perché una dimensione dichiarata enorme si fermi al confine
/// invece di esaurire la memoria del fuzzer.
const RESIDUO: u64 = 64 * 1024 * 1024;

fn limiti() -> LimitiLettura {
    LimitiLettura {
        max_byte_metadati: 1024 * 1024,
        max_byte_metadati_custom: 256 * 1024,
        max_blocchi: 1024,
    }
}

/// Scrive `tabella` in `formato` e la rilegge; `None` se la scrittura la
/// rifiuta (con un errore ammesso).
fn scritta_e_riletta(
    cartella: &Path,
    nome: &str,
    tabella: &RecordBatch,
    formato: Formato,
    barriere: u64,
) -> Option<RecordBatch> {
    let uscita = cartella.join(nome);
    let opzioni = OpzioniScrittura {
        formato: Some(formato),
        sovrascrivi: false,
        compressione: CompressioneParquet::default(),
    };
    if let Err(errore) = scrivi_tabella(tabella, &uscita, &opzioni) {
        crate::esiti::errore_ammesso("scrittura", &errore, barriere);
        return None;
    }
    let riletta =
        leggi_tabella_con_limiti(&uscita, Some(formato), u64::MAX, &LimitiLettura::default());
    Some(riletta.unwrap_or_else(|errore| panic!("una tabella scritta si rilegge: {errore}")))
}

pub fn prova(dati: &[u8], formato: Formato) {
    let barriere = crate::aggancio::panici_in_barriera();
    let cartella = tempfile::tempdir().expect("cartella temporanea");
    let percorso = cartella.path().join("ingresso");
    std::fs::File::create(&percorso)
        .and_then(|mut file| file.write_all(dati))
        .expect("file d'ingresso");

    let primo = leggi_tabella_con_limiti(&percorso, Some(formato), RESIDUO, &limiti());
    let secondo = leggi_tabella_con_limiti(&percorso, Some(formato), RESIDUO, &limiti());
    let tabella = match (primo, secondo) {
        (Ok(primo), Ok(secondo)) => {
            assert_eq!(primo, secondo);
            primo
        }
        (Err(primo), Err(secondo)) => {
            crate::esiti::stesso_errore("lettura", &primo, &secondo);
            crate::esiti::errore_ammesso("lettura", &primo, barriere);
            return;
        }
        _ => panic!("lettura non deterministica"),
    };

    match formato {
        Formato::ArrowIpc | Formato::ArrowIpcStream => {
            for (nome, uscita) in [
                ("file", Formato::ArrowIpc),
                ("stream", Formato::ArrowIpcStream),
            ] {
                if let Some(riletta) =
                    scritta_e_riletta(cartella.path(), nome, &tabella, uscita, barriere)
                {
                    assert_eq!(riletta, tabella, "round-trip IPC {nome}");
                }
            }
        }
        Formato::Parquet => {
            let Some(riletta) =
                scritta_e_riletta(cartella.path(), "uscita", &tabella, formato, barriere)
            else {
                return;
            };
            // La scrittura è riuscita: anche la preparazione GeoParquet.
            let attesa = match geoparquet::prepara(&tabella).expect("preparazione già riuscita") {
                None => tabella,
                Some(preparata) => geoparquet::applica(&preparata.tabella, &preparata.geo)
                    .expect("metadato GeoParquet appena scritto"),
            };
            assert_eq!(riletta, attesa, "round-trip Parquet");
        }
    }
}
