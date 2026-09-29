//! Sfratto delle tabelle fredde su file Arrow IPC temporanei.
//!
//! Quando un passo non sta nel budget, il runner scrive su disco le tabelle
//! residenti che il passo non usa e le rilegge prima del loro consumatore.
//! I file stanno in una directory temporanea del processo (`tempfile`),
//! cancellata alla fine dell'esecuzione anche in caso di errore; un file
//! si cancella appena riletto.
//!
//! Quota: i byte su disco degli sfratti non superano `max_temp_bytes`, e i
//! kernel con spill ricevono come quota quella rimasta. Oltre la quota lo
//! sfratto fallisce con `ResourceLimit`, mai troncando un file.
//!
//! La scrittura procede a blocchi di righe di circa [`BYTE_PER_BLOCCO`]
//! byte di dati. Il transitorio della scrittura non è però solo il blocco:
//! Arrow IPC codifica ogni blocco in un `Vec<u8>` prima di scriverlo (fino
//! al doppio del contenuto per la crescita del vettore), e i **valori dei
//! dizionari** non si affettano con le righe, per cui il primo blocco li
//! codifica interi. [`Piano::transitorio`] è il limite superiore di questo
//! transitorio, calcolato prima di scrivere con
//! `ArrayData::get_slice_memory_size` di ogni blocco (che conta interi i
//! valori dei dizionari e i figli delle liste: per eccesso); il runner lo
//! verifica nel budget prima di sfrattare, e la quota su disco si verifica
//! su [`Piano::byte_file`] prima di codificare. La rilettura ricompone i blocchi con `concat_batches` (con un solo blocco
//! le colonne restano viste del buffer del messaggio, contate una volta da
//! `byte_vivi`) e rimette lo schema originale, metadati compresi.

use std::cell::Cell;
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use plenora_core::arrow::array::cast::AsArray;
use plenora_core::arrow::array::{Array, RecordBatch};
use plenora_core::arrow::ipc::reader::StreamReader;
use plenora_core::arrow::ipc::writer::StreamWriter;
use plenora_core::arrow::schema::SchemaRef;
use plenora_core::arrow::select::concat::concat_batches;
use plenora_core::{PlenoraError, Result};
use tempfile::TempDir;

/// Byte di dati per blocco scritto.
const BYTE_PER_BLOCCO: u64 = 1024 * 1024;

/// Margine per blocco: metadati flatbuffer, padding a 8 byte dei buffer,
/// buffer di `BufWriter`.
const MARGINE_PER_BLOCCO: u64 = 64 * 1024;

fn in_u64(valore: usize) -> u64 {
    u64::try_from(valore).unwrap_or(u64::MAX)
}

/// Byte di un batch come li codifica Arrow IPC, per eccesso: i dati della
/// fetta di ogni colonna, con i valori dei dizionari e i figli delle liste
/// interi. Un tipo che Arrow non sa misurare conta le sue viste intere.
fn byte_fetta(batch: &RecordBatch) -> u64 {
    batch.columns().iter().fold(0_u64, |totale, colonna| {
        let byte = colonna
            .to_data()
            .get_slice_memory_size()
            .unwrap_or_else(|_| plenora_core::memoria::byte_viste(colonna.as_ref()));
        totale.saturating_add(in_u64(byte))
    })
}

/// Valori dei dizionari di primo livello, interi.
fn byte_dizionari(batch: &RecordBatch) -> u64 {
    batch.columns().iter().fold(0_u64, |totale, colonna| {
        let byte = colonna.as_any_dictionary_opt().map_or(0, |dizionario| {
            let valori = dizionario.values();
            valori
                .to_data()
                .get_slice_memory_size()
                .unwrap_or_else(|_| plenora_core::memoria::byte_viste(valori.as_ref()))
        });
        totale.saturating_add(in_u64(byte))
    })
}

/// Come si scriverà una tabella: righe per blocco e limiti superiori di
/// memoria transitoria e byte su disco, calcolati prima di scrivere.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Piano {
    righe_per_blocco: usize,
    /// Memoria transitoria massima della scrittura: il doppio del blocco
    /// più grande (crescita del vettore di codifica), più il margine.
    pub transitorio: u64,
    /// Byte del file, per eccesso: i blocchi senza i dizionari, i
    /// dizionari una volta, il margine per blocco.
    pub byte_file: u64,
}

/// Il piano di scrittura di una tabella.
pub fn pianifica(tabella: &RecordBatch) -> Piano {
    let righe = tabella.num_rows();
    let dizionari = byte_dizionari(tabella);
    // Righe per blocco dalla media dei dati senza i dizionari.
    let dati = byte_fetta(tabella).saturating_sub(dizionari).max(1);
    let per_blocco =
        u128::from(BYTE_PER_BLOCCO) * u128::from(in_u64(righe).max(1)) / u128::from(dati);
    let righe_per_blocco = usize::try_from(per_blocco).unwrap_or(usize::MAX).max(1);
    let mut massimo = 0_u64;
    let mut file = dizionari;
    let mut inizio = 0;
    loop {
        let lunghezza = righe_per_blocco.min(righe - inizio);
        let blocco = tabella.slice(inizio, lunghezza);
        let byte = byte_fetta(&blocco);
        massimo = massimo.max(byte);
        file = file
            .saturating_add(byte.saturating_sub(byte_dizionari(&blocco)))
            .saturating_add(MARGINE_PER_BLOCCO);
        inizio += lunghezza;
        if inizio >= righe {
            break;
        }
    }
    Piano {
        righe_per_blocco,
        transitorio: massimo.saturating_mul(2).saturating_add(MARGINE_PER_BLOCCO),
        byte_file: file,
    }
}

/// Una tabella sfrattata: dove sta e quanto pesava.
#[derive(Debug)]
pub struct Sfrattata {
    percorso: PathBuf,
    schema: SchemaRef,
    /// Righe della tabella.
    pub righe: u64,
    /// Byte del file.
    pub byte_file: u64,
    /// `byte_vivi` della sola tabella quando è stata sfrattata.
    pub byte_in_memoria: u64,
}

impl Sfrattata {
    /// Byte che la tabella occuperà riletta: il maggiore fra il file (che
    /// contiene i buffer dei messaggi) e la memoria di prima.
    pub fn byte_stimati(&self) -> u64 {
        self.byte_file.max(self.byte_in_memoria)
    }
}

/// Scrittore che conta i byte e si ferma oltre la quota.
///
/// Il rifiuto resta segnato in `superata`: l'errore che risale da Arrow
/// ne perde il tipo, il segnale no.
struct Contatore {
    interno: BufWriter<File>,
    scritti: u64,
    disponibili: u64,
    superata: Rc<Cell<bool>>,
}

impl Write for Contatore {
    fn write(&mut self, byte: &[u8]) -> std::io::Result<usize> {
        let nuovi = u64::try_from(byte.len()).unwrap_or(u64::MAX);
        match self.scritti.checked_add(nuovi) {
            Some(totale) if totale <= self.disponibili => {
                let scritti = self.interno.write(byte)?;
                self.scritti = self
                    .scritti
                    .saturating_add(u64::try_from(scritti).unwrap_or(u64::MAX));
                Ok(scritti)
            }
            _ => {
                self.superata.set(true);
                Err(std::io::Error::other("quota di sfratto superata"))
            }
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.interno.flush()
    }
}

/// Le tabelle sfrattate di un'esecuzione e la loro quota su disco.
#[derive(Debug)]
pub struct AreaSfratti {
    directory: Option<TempDir>,
    quota: u64,
    su_disco: u64,
    massimo_su_disco: u64,
    prossimo_file: u64,
    tabelle: BTreeMap<String, Sfrattata>,
}

impl AreaSfratti {
    pub const fn new(quota: u64) -> Self {
        Self {
            directory: None,
            quota,
            su_disco: 0,
            massimo_su_disco: 0,
            prossimo_file: 0,
            tabelle: BTreeMap::new(),
        }
    }

    /// La tabella sfrattata con questo nome, se c'è.
    pub fn sfrattata(&self, nome: &str) -> Option<&Sfrattata> {
        self.tabelle.get(nome)
    }

    /// Nomi delle tabelle sfrattate, in ordine.
    pub fn nomi(&self) -> impl Iterator<Item = &str> {
        self.tabelle.keys().map(String::as_str)
    }

    /// Quota rimasta per lo spill dei kernel.
    pub const fn quota_rimasta(&self) -> u64 {
        self.quota.saturating_sub(self.su_disco)
    }

    /// Massimo dei byte su disco raggiunto dagli sfratti.
    pub const fn massimo_su_disco(&self) -> u64 {
        self.massimo_su_disco
    }

    fn directory(&mut self) -> Result<PathBuf> {
        if let Some(directory) = &self.directory {
            return Ok(directory.path().to_path_buf());
        }
        let directory = tempfile::Builder::new()
            .prefix("plenora-pipeline-sfratti-")
            .tempdir()?;
        let percorso = directory.path().to_path_buf();
        self.directory = Some(directory);
        Ok(percorso)
    }

    /// Scrive la tabella su disco; la tabella esce dalla memoria del
    /// runner quando il chiamante la lascia cadere.
    ///
    /// # Errors
    ///
    /// `ResourceLimit` oltre `max_temp_bytes`; `Io`, `Arrow` dalla
    /// scrittura; `Internal` se il nome è già sfrattato.
    pub fn sfratta(
        &mut self,
        nome: &str,
        tabella: &RecordBatch,
        byte_in_memoria: u64,
    ) -> Result<()> {
        if self.tabelle.contains_key(nome) {
            return Err(PlenoraError::Internal(format!(
                "`{nome}` sfrattata due volte"
            )));
        }
        let percorso = self
            .directory()?
            .join(format!("tabella-{:06}.arrows", self.prossimo_file));
        self.prossimo_file += 1;
        let esito = self.scrivi(&percorso, tabella);
        let byte_file = match esito {
            Ok(byte_file) => byte_file,
            Err(errore) => {
                // Un file parziale non resta su disco: non si rilegge mai.
                let _ = std::fs::remove_file(&percorso);
                return Err(errore);
            }
        };
        self.su_disco = self.su_disco.checked_add(byte_file).ok_or_else(|| {
            PlenoraError::Internal("byte su disco non rappresentabili".to_owned())
        })?;
        self.massimo_su_disco = self.massimo_su_disco.max(self.su_disco);
        let righe = u64::try_from(tabella.num_rows())
            .map_err(|_| PlenoraError::Internal("righe non rappresentabili".to_owned()))?;
        self.tabelle.insert(
            nome.to_owned(),
            Sfrattata {
                percorso,
                schema: tabella.schema(),
                righe,
                byte_file,
                byte_in_memoria,
            },
        );
        Ok(())
    }

    fn scrivi(&self, percorso: &Path, tabella: &RecordBatch) -> Result<u64> {
        let piano = pianifica(tabella);
        // La quota si verifica prima di codificare, sul limite superiore
        // del file; il contatore la riverifica sui byte veri.
        if piano.byte_file > self.quota_rimasta() {
            return Err(PlenoraError::ResourceLimit(format!(
                "sfratto oltre max_temp_bytes {}",
                self.quota
            )));
        }
        let superata = Rc::new(Cell::new(false));
        let esito = Self::scrivi_blocchi(
            Contatore {
                interno: BufWriter::new(File::create(percorso)?),
                scritti: 0,
                disponibili: self.quota_rimasta(),
                superata: Rc::clone(&superata),
            },
            tabella,
            piano.righe_per_blocco,
        );
        if superata.get() {
            return Err(PlenoraError::ResourceLimit(format!(
                "sfratto oltre max_temp_bytes {}",
                self.quota
            )));
        }
        esito
    }

    fn scrivi_blocchi(
        contatore: Contatore,
        tabella: &RecordBatch,
        per_blocco: usize,
    ) -> Result<u64> {
        let mut scrittore = StreamWriter::try_new(contatore, &tabella.schema())?;
        let righe = tabella.num_rows();
        let mut inizio = 0;
        while inizio < righe {
            let lunghezza = per_blocco.min(righe - inizio);
            scrittore.write(&tabella.slice(inizio, lunghezza))?;
            inizio += lunghezza;
        }
        scrittore.finish()?;
        let mut contatore = scrittore.into_inner()?;
        contatore.flush()?;
        Ok(contatore.scritti)
    }

    /// Rilegge una tabella sfrattata e cancella il suo file.
    ///
    /// # Errors
    ///
    /// `Internal` se il nome non è sfrattato, o se il file riletto ha un
    /// numero di righe o tipi diversi da quelli scritti (il contenuto non
    /// si verifica: la directory è privata del processo); `Io`, `Arrow`
    /// dalla lettura.
    pub fn ricarica(&mut self, nome: &str) -> Result<RecordBatch> {
        let sfrattata = self
            .tabelle
            .remove(nome)
            .ok_or_else(|| PlenoraError::Internal(format!("`{nome}` non e' sfrattata")))?;
        let lettore =
            StreamReader::try_new(BufReader::new(File::open(&sfrattata.percorso)?), None)?;
        let schema_file = lettore.schema();
        let blocchi = lettore.collect::<std::result::Result<Vec<_>, _>>()?;
        let unita = concat_batches(&schema_file, &blocchi)?;
        drop(blocchi);
        std::fs::remove_file(&sfrattata.percorso)?;
        self.su_disco = self.su_disco.saturating_sub(sfrattata.byte_file);
        let righe = u64::try_from(unita.num_rows()).unwrap_or(u64::MAX);
        if righe != sfrattata.righe {
            return Err(PlenoraError::Internal(format!(
                "`{nome}` riletta con un numero di righe diverso da quello scritto"
            )));
        }
        plenora_core::batch_with_rows(sfrattata.schema, unita.columns().to_vec(), unita.num_rows())
            .map_err(|_| {
                PlenoraError::Internal(format!(
                    "`{nome}` riletta con uno schema diverso da quello scritto"
                ))
            })
    }
}
