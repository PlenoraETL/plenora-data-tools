//! Le operazioni geo del runner: funzioni `RecordBatch` → `RecordBatch`
//! sui kernel di `plenora-kernels-geo`.
//!
//! Il contratto d'uscita lo decide l'analisi dei kernel
//! (`analyze_geo_contract`, in validazione); qui la config si legge con gli
//! stessi tipi dell'analisi (`plenora_kernels_geo::analyze::config`), una
//! volta, e in esecuzione il passo calcola le colonne dell'uscita nell'ordine
//! del contratto e le monta sul suo schema. Nessuna fusione, nessuno
//! streaming: una tabella intera per ingresso, come per le tabellari.
//!
//! Per ogni colonna geometria d'ingresso, prima del kernel:
//! - le celle si decodificano (contratto WKB strutturale) e ogni coordinata
//!   deve stare nel **dominio di validita'** del CRS della colonna
//!   (`validate_geometry_domain`): i kernel non ricevono un CRS, il
//!   controllo e' del chiamante (README, «CRS integrati»);
//! - se il contratto dichiara i tipi geometrici con un elenco, ogni cella
//!   deve essere di uno di quei tipi (`Schema`): la dichiarazione di un
//!   ingresso non si prende sulla parola.
//!
//! La validazione OGC la fanno i kernel, una volta per geometria. La
//! precisione dei kernel che la chiedono e' 1 cm a terra nelle unita' del CRS
//! (`Precision::from_crs`), calcolata in validazione.
//!
//! Sull'uscita: una geometria null in una colonna che il contratto dichiara
//! non nullable e' un errore esplicito del passo (una geometria vuota in
//! ingresso a `point_on_surface`, per esempio), e ogni geometria prodotta
//! deve avere un tipo che il contratto dichiara (altrimenti `Internal`:
//! analisi e kernel divergono).

mod binari;
mod collettivi;
mod errori;
mod unari;

use std::sync::Arc;

use geo::Geometry;
use plenora_core::arrow::array::{Array, ArrayRef, BinaryArray, RecordBatch};
use plenora_core::catalog::OperationDescriptor;
use plenora_core::contract::{
    DataContract, GeometryColumnContract, GeometryType, TypesDeclaration,
};
use plenora_core::crs::ResolvedCrs;
use plenora_core::limits::Limits;
use plenora_core::{PlenoraError, Result};
use plenora_kernels_geo::arrow_adapter::{batch_geometry_cells, map_nullable};
use plenora_kernels_geo::rust_backend::precision::Precision;
use serde::de::DeserializeOwned;
use serde_json::Value;

use self::binari::KernelBinario;
use self::collettivi::KernelCollettivo;
use self::unari::KernelUnario;

/// Una colonna geometria d'ingresso di un passo.
#[derive(Debug)]
pub struct Lato {
    /// Posizione nello schema dell'ingresso.
    indice: usize,
    nome: String,
    crs: ResolvedCrs,
    /// Tipi dichiarati con un elenco (`exact` o `mixed`), se ci sono.
    tipi: Option<Vec<GeometryType>>,
}

impl Lato {
    /// La colonna geometria attiva del contratto, con il suo CRS risolto.
    ///
    /// L'analisi ha gia' chiesto una sola geometria identificabile, XY e con
    /// CRS risolto: un'assenza qui e' `Internal`.
    fn di(op: &str, contratto: &DataContract) -> Result<Self> {
        let geometria = contratto.active_geometry_column().ok_or_else(|| {
            PlenoraError::Internal(format!("{op}: ingresso senza geometria dopo l'analisi"))
        })?;
        let crs = geometria.crs.as_resolved().cloned().ok_or_else(|| {
            PlenoraError::Internal(format!("{op}: geometria senza CRS risolto dopo l'analisi"))
        })?;
        let indice = contratto.schema.index_of(&geometria.name).map_err(|_| {
            PlenoraError::Internal(format!("{op}: colonna geometria assente dallo schema"))
        })?;
        Ok(Self {
            indice,
            nome: geometria.name.clone(),
            crs,
            tipi: tipi_con_elenco(geometria),
        })
    }

    /// Le celle WKB della colonna in un batch.
    fn celle<'a>(&self, batch: &'a RecordBatch) -> Result<&'a BinaryArray> {
        batch_geometry_cells(batch, self.indice, &self.nome)
    }

    /// La precisione dichiarata delle operazioni geografiche nel CRS della
    /// colonna: 1 cm a terra.
    fn precisione(&self) -> Result<Precision> {
        Precision::from_crs(&self.crs).map_err(PlenoraError::from)
    }

    /// Dominio di validita' e tipi dichiarati di ogni cella non-null, dopo la
    /// sola decodifica strutturale (la validazione OGC resta ai kernel).
    fn verifica(&self, op: &str, batch: &RecordBatch) -> Result<()> {
        let celle = self.celle(batch)?;
        verifica_tipi_dichiarati(celle, self.tipi.as_deref()).map_err(|()| {
            PlenoraError::Schema(format!(
                "{op}: una geometria della colonna `{}` ha un tipo che il contratto \
                 dell'ingresso non dichiara",
                self.nome
            ))
        })?;
        verifica_dominio(op, celle, &self.crs)
    }
}

/// Ogni cella non-null nel dominio di validita' del CRS, dopo la sola
/// decodifica strutturale.
fn verifica_dominio(op: &str, celle: &BinaryArray, crs: &ResolvedCrs) -> Result<()> {
    map_nullable(celle, |cella| {
        let geometria = plenora_kernels_geo::wkb_decoder::decode_validated(cella)?;
        nel_dominio(op, &geometria, crs)?;
        Ok(None::<()>)
    })
    .map(drop)
}

/// I tipi dichiarati con un elenco non vuoto (`exact` o `mixed`).
fn tipi_con_elenco(geometria: &GeometryColumnContract) -> Option<Vec<GeometryType>> {
    let dichiarati = geometria.types.value()?;
    match dichiarati.declaration() {
        TypesDeclaration::Exact | TypesDeclaration::Mixed if !dichiarati.types().is_empty() => {
            Some(dichiarati.types().to_vec())
        }
        _ => None,
    }
}

/// Ogni coordinata nel dominio di validita' del CRS.
fn nel_dominio(op: &str, geometria: &Geometry<f64>, crs: &ResolvedCrs) -> Result<()> {
    plenora_kernels_geo::crs::validate_geometry_domain(geometria, crs)
        .map_err(|errore| PlenoraError::Crs(format!("{op}: {errore}")))
}

/// Il tipo geometrico dal type code di una cella WKB (ISO, o EWKB con i
/// flag alti); `None` se la cella e' troncata o il codice sconosciuto.
fn tipo_wkb(cella: &[u8]) -> Option<GeometryType> {
    let (&ordine, resto) = cella.split_first()?;
    let byte = *resto.first_chunk::<4>()?;
    let codice = match ordine {
        0 => u32::from_be_bytes(byte),
        1 => u32::from_le_bytes(byte),
        _ => return None,
    };
    let base = if codice & 0xE000_0000 == 0 {
        codice % 1000
    } else {
        codice & 0x0000_FFFF
    };
    GeometryType::from_wkb_base_type(base)
}

/// Ogni cella non-null e' di un tipo dell'elenco (nessun controllo senza
/// elenco). `Err(())` alla prima che non lo e'.
fn verifica_tipi_dichiarati(
    celle: &BinaryArray,
    ammessi: Option<&[GeometryType]>,
) -> std::result::Result<(), ()> {
    let Some(ammessi) = ammessi else {
        return Ok(());
    };
    for riga in 0..celle.len() {
        if celle.is_null(riga) {
            continue;
        }
        match tipo_wkb(celle.value(riga)) {
            Some(tipo) if ammessi.contains(&tipo) => {}
            _ => return Err(()),
        }
    }
    Ok(())
}

/// Monta le colonne calcolate sullo schema del contratto d'uscita.
///
/// # Errors
///
/// - `InvalidPlan` per una geometria null in una colonna non nullable;
/// - `Internal` per un numero di colonne diverso dal contratto o una
///   geometria di un tipo che il contratto non dichiara.
fn monta(
    op: &str,
    uscita: &DataContract,
    colonne: Vec<ArrayRef>,
    righe: usize,
) -> Result<RecordBatch> {
    if colonne.len() != uscita.schema.fields().len() {
        return Err(PlenoraError::Internal(format!(
            "{op}: colonne calcolate diverse da quelle del contratto"
        )));
    }
    for geometria in &uscita.geometries {
        let indice = uscita.schema.index_of(&geometria.name).map_err(|_| {
            PlenoraError::Internal(format!(
                "{op}: geometria del contratto assente dallo schema"
            ))
        })?;
        let celle = colonne
            .get(indice)
            .and_then(|colonna| colonna.as_any().downcast_ref::<BinaryArray>())
            .ok_or_else(|| {
                PlenoraError::Internal(format!("{op}: colonna geometria calcolata non Binary"))
            })?;
        if !geometria.nullable && celle.null_count() > 0 {
            return Err(PlenoraError::InvalidPlan(format!(
                "{op}: il risultato di una riga e' vuoto (null), ma la colonna geometria \
                 `{}` del contratto non ammette null",
                geometria.name
            )));
        }
        verifica_tipi_dichiarati(celle, tipi_con_elenco(geometria).as_deref()).map_err(|()| {
            PlenoraError::Internal(format!(
                "{op}: una geometria prodotta ha un tipo che il contratto d'uscita non \
                     dichiara (analisi e kernel divergono)"
            ))
        })?;
    }
    plenora_core::batch_with_rows(uscita.schema.clone(), colonne, righe)
}

/// Sostituisce la colonna `indice` con `nuove` (una o piu' colonne);
/// `Internal` se l'indice e' fuori dalle colonne.
fn sostituisci(
    op: &str,
    colonne: &mut Vec<ArrayRef>,
    indice: usize,
    nuove: Vec<ArrayRef>,
) -> Result<()> {
    if indice >= colonne.len() {
        return Err(PlenoraError::Internal(format!(
            "{op}: colonna geometria oltre le colonne dell'ingresso"
        )));
    }
    colonne.splice(indice..=indice, nuove).for_each(drop);
    Ok(())
}

/// Una colonna `Binary` di celle WKB.
fn binaria(celle: &[Option<Vec<u8>>]) -> ArrayRef {
    Arc::new(celle.iter().map(Option::as_deref).collect::<BinaryArray>())
}

/// La config nei tipi dell'analisi: l'analisi l'ha gia' accettata con lo
/// stesso tipo, un rifiuto qui e' comunque un errore di piano.
fn config<T: DeserializeOwned>(op: &str, config: &Value) -> Result<T> {
    T::deserialize(config)
        .map_err(|errore| PlenoraError::InvalidPlan(format!("{op}: config non valida: {errore}")))
}

/// Il kernel di un passo geo, con la config gia' letta.
#[derive(Debug)]
enum Kernel {
    Unario(Box<KernelUnario>),
    /// Con il limite di righe dell'arco d'uscita, tetto dei kernel.
    Collettivo(Box<KernelCollettivo>, u64),
    /// Due tabelle (left, right), con lo stesso limite di righe.
    Binario(Box<KernelBinario>, u64),
}

/// Un passo geo validato: il kernel e le colonne geometria degli ingressi.
#[derive(Debug)]
pub struct PassoGeo {
    op: &'static str,
    /// Una voce per ingresso: la sua colonna geometria, se ne ha una.
    lati: Vec<Option<Lato>>,
    kernel: Kernel,
}

/// Il limite di righe dell'arco d'uscita, che i kernel ricevono come tetto
/// dell'output (come D14.6 a `190c493`): `max_output_rows` per un output
/// del piano, `max_rows_per_edge` altrimenti.
const fn righe_massime(limiti: &Limits, uscita_del_piano: bool) -> u64 {
    if uscita_del_piano {
        limiti.rows.max_output_rows
    } else {
        limiti.rows.max_rows_per_edge
    }
}

impl PassoGeo {
    /// Legge la config e prepara il kernel.
    ///
    /// # Errors
    ///
    /// - `Unsupported`: operazione geo senza dispatch nel runner;
    /// - gli errori di lettura della config e della precisione del CRS.
    pub fn prepara(
        descrittore: &'static OperationDescriptor,
        valore: &Value,
        ingressi: &[DataContract],
        uscita: &DataContract,
        limiti: &Limits,
        uscita_del_piano: bool,
    ) -> Result<Self> {
        let op = descrittore.id;
        let lati = ingressi
            .iter()
            .map(|contratto| {
                if contratto.geometries.is_empty() {
                    Ok(None)
                } else {
                    Lato::di(op, contratto).map(Some)
                }
            })
            .collect::<Result<Vec<_>>>()?;
        let righe = righe_massime(limiti, uscita_del_piano);
        let kernel = if let Some(unario) =
            KernelUnario::prepara(op, valore, &lati, ingressi, uscita, righe)?
        {
            Kernel::Unario(Box::new(unario))
        } else if let Some(collettivo) = KernelCollettivo::prepara(op, valore, &lati, ingressi)? {
            Kernel::Collettivo(Box::new(collettivo), righe)
        } else if let Some(binario) = KernelBinario::prepara(op, valore, &lati, limiti)? {
            Kernel::Binario(Box::new(binario), righe)
        } else {
            return Err(PlenoraError::Unsupported(format!(
                "{op}: operazione geo senza dispatch nel runner"
            )));
        };
        Ok(Self { op, lati, kernel })
    }

    /// Righe dell'uscita note a secco, per il modello di costo
    /// (`generate_grid`: le celle della griglia); zero altrimenti.
    pub fn righe_previste(uscita: &DataContract) -> u64 {
        uscita
            .properties
            .row_count
            .as_ref()
            .and_then(|righe| righe.value().copied())
            .unwrap_or(0)
    }

    /// Esegue il passo sugli ingressi e monta l'uscita sul contratto.
    ///
    /// # Errors
    ///
    /// Gli errori dei controlli sugli ingressi (dominio del CRS, tipi
    /// dichiarati), dei kernel e del montaggio.
    pub fn esegui(&self, ingressi: &[&RecordBatch], uscita: &DataContract) -> Result<RecordBatch> {
        if ingressi.len() != self.lati.len() {
            return Err(PlenoraError::Internal(format!(
                "{}: numero di ingressi diverso da quello validato",
                self.op
            )));
        }
        for (lato, batch) in self.lati.iter().zip(ingressi) {
            if let Some(lato) = lato {
                lato.verifica(self.op, batch)?;
            }
        }
        let (colonne, righe) = match &self.kernel {
            Kernel::Unario(kernel) => {
                let [batch] = ingressi else {
                    return Err(PlenoraError::Internal(format!(
                        "{}: operazione unaria con piu' ingressi",
                        self.op
                    )));
                };
                kernel.esegui(
                    self.op,
                    self.lati.first().and_then(Option::as_ref),
                    batch,
                    uscita,
                )?
            }
            Kernel::Collettivo(kernel, righe_massime) => {
                let [batch] = ingressi else {
                    return Err(PlenoraError::Internal(format!(
                        "{}: operazione unaria con piu' ingressi",
                        self.op
                    )));
                };
                kernel.esegui(
                    self.op,
                    self.lati.first().and_then(Option::as_ref),
                    batch,
                    *righe_massime,
                )?
            }
            Kernel::Binario(kernel, righe_massime) => {
                let ([sinistra, destra], [Some(geo_left), Some(geo_right)]) =
                    (ingressi, self.lati.as_slice())
                else {
                    return Err(PlenoraError::Internal(format!(
                        "{}: operazione binaria senza due ingressi geometrici",
                        self.op
                    )));
                };
                kernel.esegui(
                    self.op,
                    (geo_left, geo_right),
                    (sinistra, destra),
                    *righe_massime,
                )?
            }
        };
        monta(self.op, uscita, colonne, righe)
    }
}

#[cfg(test)]
mod tests {
    use plenora_core::contract::GeometryType;

    use super::tipo_wkb;

    #[test]
    fn il_tipo_si_legge_dal_type_code_iso_ed_ewkb() {
        assert_eq!(tipo_wkb(&[1, 3, 0, 0, 0]), Some(GeometryType::Polygon));
        assert_eq!(tipo_wkb(&[0, 0, 0, 0, 6]), Some(GeometryType::MultiPolygon));
        // ISO Z (1001) e EWKB Z (0x80000001).
        assert_eq!(tipo_wkb(&[1, 0xE9, 3, 0, 0]), Some(GeometryType::Point));
        assert_eq!(tipo_wkb(&[1, 1, 0, 0, 0x80]), Some(GeometryType::Point));
        assert_eq!(tipo_wkb(&[1, 3, 0]), None);
        assert_eq!(tipo_wkb(&[2, 1, 0, 0, 0]), None);
        assert_eq!(tipo_wkb(&[1, 99, 0, 0, 0]), None);
    }
}
