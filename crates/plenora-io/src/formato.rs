//! Formati di file e loro riconoscimento.

use std::path::Path;

use plenora_core::{PlenoraError, Result};

/// Formato di un file di tabella.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Formato {
    /// Arrow IPC, formato file (`application/vnd.apache.arrow.file`). In
    /// lettura file (Feather v2) o stream, riconosciuti dal contenuto; in
    /// scrittura formato file.
    ArrowIpc,
    /// Arrow IPC, formato stream (`application/vnd.apache.arrow.stream`).
    /// In lettura come [`Formato::ArrowIpc`] (file o stream, dal
    /// contenuto); in scrittura formato stream, con il marcatore di fine.
    ArrowIpcStream,
    /// Parquet; `GeoParquet` 1.1 quando c'è il metadato di file `geo` (in
    /// lettura) o una colonna geometrica nello schema (in scrittura).
    Parquet,
}

impl Formato {
    /// Il formato dall'estensione, senza distinzione di maiuscole:
    /// `.arrow`, `.feather`, `.ipc` → [`Formato::ArrowIpc`]; `.arrows` (il
    /// nome che Arrow dà agli stream) → [`Formato::ArrowIpcStream`];
    /// `.parquet` → [`Formato::Parquet`].
    ///
    /// # Errors
    ///
    /// `Unsupported` per ogni altra estensione, o se manca: il formato non si
    /// indovina, si dichiara.
    pub fn da_percorso(percorso: &Path) -> Result<Self> {
        let estensione = percorso
            .extension()
            .and_then(|estensione| estensione.to_str())
            .map(str::to_ascii_lowercase);
        match estensione.as_deref() {
            Some("arrow" | "feather" | "ipc") => Ok(Self::ArrowIpc),
            Some("arrows") => Ok(Self::ArrowIpcStream),
            Some("parquet") => Ok(Self::Parquet),
            _ => Err(PlenoraError::Unsupported(
                "estensione di file non riconosciuta: attese .arrow, .feather, .ipc, .arrows \
                 o .parquet, oppure un formato esplicito"
                    .to_owned(),
            )),
        }
    }

    /// Il formato esplicito, o quello dell'estensione.
    ///
    /// # Errors
    ///
    /// Come [`Formato::da_percorso`] quando `esplicito` è `None`.
    pub fn risolvi(esplicito: Option<Self>, percorso: &Path) -> Result<Self> {
        esplicito.map_or_else(|| Self::da_percorso(percorso), Ok)
    }
}

/// Compressione delle pagine Parquet in scrittura.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum CompressioneParquet {
    /// Nessuna compressione.
    Nessuna,
    /// SNAPPY.
    Snappy,
    /// ZSTD al livello 3 (il default di libzstd).
    #[default]
    Zstd,
}

/// Opzioni di scrittura.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OpzioniScrittura {
    /// Formato; `None` lo deduce dall'estensione.
    pub formato: Option<Formato>,
    /// Sostituisce un file esistente. Senza, un percorso esistente è un
    /// errore (`Conflict`) e il file resta com'era.
    pub sovrascrivi: bool,
    /// Compressione Parquet (ignorata per Arrow IPC, che si scrive senza).
    pub compressione: CompressioneParquet,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn estensioni() {
        for (nome, atteso) in [
            ("a.arrow", Formato::ArrowIpc),
            ("a.FEATHER", Formato::ArrowIpc),
            ("dir.x/a.ipc", Formato::ArrowIpc),
            ("a.arrows", Formato::ArrowIpcStream),
            ("a.ARROWS", Formato::ArrowIpcStream),
            ("a.Parquet", Formato::Parquet),
        ] {
            assert_eq!(Formato::da_percorso(Path::new(nome)).ok(), Some(atteso));
        }
        for nome in ["a", "a.csv", "a.parquet.tmp", ".parquet"] {
            assert!(Formato::da_percorso(Path::new(nome)).is_err(), "{nome}");
        }
        assert_eq!(
            Formato::risolvi(Some(Formato::Parquet), Path::new("a.bin")).ok(),
            Some(Formato::Parquet)
        );
    }
}
