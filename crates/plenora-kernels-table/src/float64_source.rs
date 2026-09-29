//! Lettura di una colonna Arrow come `f64` **con arrotondamento dichiarato**.
//!
//! Il nome dice la semantica: questa sorgente serve alle operazioni il cui
//! risultato e' un `Float64` per contratto (medie, statistiche, formule),
//! dove il double e' il tipo del risultato e non un passaggio intermedio: un
//! intero oltre 2^53 o un decimale diventano il double piu' vicino, senza
//! errore.
//! Chi deve **decidere** — confrontare, raggruppare, scegliere una riga — non
//! passa di qui: usa `scalar_as_f64` (esatto o errore) o `scalar_compare`,
//! che non converte affatto.
//!
//! Qui stanno solo downcast, lettura, null e conversione; riduzioni,
//! comparatori e politiche delle singole operazioni restano nei loro moduli.
//! I rami veloci sono un'ottimizzazione sul tipo fisico Arrow: ogni ramo
//! rende cio' che renderebbe `scalar_as_f64_rounded`, e tenerli in un punto
//! solo impedisce che lo stesso valore dia esiti diversi secondo la codifica
//! della colonna. Il modulo e' privato: il `pub` non esce dal crate.

use std::cmp::Ordering;

use plenora_core::arrow::array::{Array, ArrayRef, Float64Array, Int64Array, UInt64Array};
use plenora_core::arrow::schema::{DataType, TimeUnit};
use plenora_core::{PlenoraError, Result};

use crate::aggregation::compare_cells_typed;
use crate::scalar_as_f64_rounded;

/// Colonna letta come `f64`: percorsi nativi per i tipi piu' comuni,
/// `scalar_as_f64_rounded` per tutti gli altri.
pub enum Float64Source<'a> {
    /// Colonna `Float64`: il valore cosi' com'e'.
    Float64(&'a Float64Array),
    /// Colonna `Int64`: cast a `f64`, arrotondato oltre 2^53.
    Int64(&'a Int64Array),
    /// Colonna `UInt64`: cast a `f64`, arrotondato oltre 2^53.
    UInt64(&'a UInt64Array),
    /// Ogni altro tipo: `scalar_as_f64_rounded` riga per riga.
    Generic(&'a ArrayRef),
}

impl<'a> Float64Source<'a> {
    /// Sceglie il percorso una volta sola, invece che riga per riga.
    pub fn new(array: &'a ArrayRef) -> Self {
        if let Some(values) = array.as_any().downcast_ref::<Float64Array>() {
            return Self::Float64(values);
        }
        if let Some(values) = array.as_any().downcast_ref::<Int64Array>() {
            return Self::Int64(values);
        }
        if let Some(values) = array.as_any().downcast_ref::<UInt64Array>() {
            return Self::UInt64(values);
        }
        Self::Generic(array)
    }

    /// Il valore della riga, `None` se null.
    ///
    /// **Tutti i rami concordano con [`scalar_as_f64_rounded`]**, altrimenti
    /// lo stesso valore darebbe esiti diversi secondo la codifica.
    ///
    /// # Errors
    ///
    /// Gli errori di [`scalar_as_f64_rounded`] sul percorso generico: testo
    /// non convertibile in numero, tipo non convertibile, decimal128
    /// incoerente. I percorsi nativi non falliscono.
    pub fn value(&self, row: usize) -> Result<Option<f64>> {
        match self {
            Self::Float64(values) => Ok(if values.is_null(row) {
                None
            } else {
                Some(values.value(row))
            }),
            #[allow(clippy::cast_precision_loss)] // Arrotondamento voluto: vedi sopra.
            Self::Int64(values) => Ok(if values.is_null(row) {
                None
            } else {
                Some(values.value(row) as f64)
            }),
            #[allow(clippy::cast_precision_loss)] // Arrotondamento voluto: vedi sopra.
            Self::UInt64(values) => Ok(if values.is_null(row) {
                None
            } else {
                Some(values.value(row) as f64)
            }),
            Self::Generic(array) => scalar_as_f64_rounded(array.as_ref(), row),
        }
    }
}

/// I tipi che il contratto considera **numerici**.
///
/// Autorita' unica per l'analizzatore (`require_numeric`) e i kernel: due
/// elenchi separati farebbero divergere analisi ed esecuzione.
///
/// `Utf8` c'e' perche' il contratto ammette il testo **interpretato come
/// numero**; `Boolean`, `Binary` e le dictionary non ci sono.
#[must_use]
pub const fn dominio_numerico(data_type: &DataType) -> bool {
    matches!(
        data_type,
        DataType::Float64
            | DataType::Int64
            | DataType::UInt64
            | DataType::Date32
            | DataType::Timestamp(TimeUnit::Millisecond, _)
            | DataType::Decimal128(_, _)
            | DataType::Utf8
    )
}

/// Ordine sul dominio **originale**, per le operazioni che decidono.
///
/// Chi confronta non converte: due interi distinti oltre 2^53 hanno lo stesso
/// `f64`, e ordinarli come double li renderebbe a pari merito. Il confronto
/// passa da [`compare_cells_typed`], la stessa autorita' del sort — non una
/// seconda tabella dei tipi.
///
/// # Il testo numerico non e' ordinabile qui
///
/// Interpretato come double, `"9007199254740993"` e `"9007199254740992"`
/// sarebbero a pari merito, e il kernel non ha un'aritmetica decimale per
/// confrontarli esattamente. Le operazioni di rango rifiutano quindi `Utf8`,
/// in analisi e in esecuzione: e' un restringimento dichiarato.
///
/// Su `Float64` e `Int64` il confronto di due righe nell'array e' fatto sul
/// tipo gia' risolto: e' cio' che `compare_cells_typed` decide per la stessa
/// coppia (null dopo i valori, `total_cmp` sui double, `cmp` sugli interi)
/// senza rifare per ogni confronto famiglia, downcast e controlli
/// dictionary, che per questi tipi non possono fallire. Un indice fuori
/// dall'array e ogni altro tipo passano da `compare_cells_typed`, con i suoi
/// errori.
pub struct OrdineNumerico<'a> {
    array: &'a ArrayRef,
    tipizzato: OrdineTipizzato<'a>,
}

/// Array gia' risolto per i confronti di [`OrdineNumerico`].
enum OrdineTipizzato<'a> {
    Float64(&'a Float64Array),
    Int64(&'a Int64Array),
    Generico,
}

/// Null dopo i valori, poi `confronta` sui valori: l'ordine dei null di
/// `compare_cells_typed`.
fn null_in_coda<T>(
    sinistra: Option<T>,
    destra: Option<T>,
    confronta: impl FnOnce(T, T) -> Ordering,
) -> Ordering {
    match (sinistra, destra) {
        (None, None) => Ordering::Equal,
        (None, Some(_)) => Ordering::Greater,
        (Some(_), None) => Ordering::Less,
        (Some(sinistra), Some(destra)) => confronta(sinistra, destra),
    }
}

impl<'a> OrdineNumerico<'a> {
    /// Prepara l'ordine, rifiutando cio' che non ha un ordine esatto.
    ///
    /// # Errors
    ///
    /// `PlenoraError::Schema` se il tipo e' fuori dal dominio numerico o se
    /// e' `Utf8`, che il rango non ordina.
    pub fn new(array: &'a ArrayRef) -> Result<Self> {
        if !dominio_numerico(array.data_type()) {
            return Err(PlenoraError::Schema(format!(
                "tipo {:?} non convertibile in numero",
                array.data_type()
            )));
        }
        if array.data_type() == &DataType::Utf8 {
            return Err(PlenoraError::Schema(
                "il testo numerico non ha un ordine esatto: le funzioni di rango non lo accettano"
                    .to_owned(),
            ));
        }
        let tipizzato = array
            .as_any()
            .downcast_ref::<Float64Array>()
            .map(OrdineTipizzato::Float64)
            .or_else(|| {
                array
                    .as_any()
                    .downcast_ref::<Int64Array>()
                    .map(OrdineTipizzato::Int64)
            })
            .unwrap_or(OrdineTipizzato::Generico);
        Ok(Self { array, tipizzato })
    }

    /// Confronta due righe nel dominio originale.
    ///
    /// # Errors
    ///
    /// Gli errori di [`compare_cells_typed`].
    pub fn compare(&self, sinistra: usize, destra: usize) -> Result<Ordering> {
        let righe = self.array.len();
        if sinistra < righe && destra < righe {
            match self.tipizzato {
                OrdineTipizzato::Float64(values) => {
                    let valore = |riga: usize| (!values.is_null(riga)).then(|| values.value(riga));
                    return Ok(null_in_coda(valore(sinistra), valore(destra), |a, b| {
                        a.total_cmp(&b)
                    }));
                }
                OrdineTipizzato::Int64(values) => {
                    let valore = |riga: usize| (!values.is_null(riga)).then(|| values.value(riga));
                    return Ok(null_in_coda(valore(sinistra), valore(destra), |a, b| {
                        a.cmp(&b)
                    }));
                }
                OrdineTipizzato::Generico => {}
            }
        }
        compare_cells_typed(self.array, sinistra, self.array, destra)
    }
}

/// Pretende che i valori rispettino il contratto numerico della colonna.
///
/// Serve alle varianti che dipendono solo dalla posizione: una colonna di
/// testo non numerico resta un ingresso invalido anche se il kernel non ne
/// legge i valori. Per i tipi nativi il dominio e' il tipo.
///
/// # Errors
///
/// `PlenoraError::Schema` se il tipo e' fuori dal dominio o se una cella di
/// testo non e' interpretabile come numero.
pub fn valida_valori_numerici(array: &ArrayRef) -> Result<()> {
    if !dominio_numerico(array.data_type()) {
        return Err(PlenoraError::Schema(format!(
            "tipo {:?} non convertibile in numero",
            array.data_type()
        )));
    }
    if array.data_type() == &DataType::Utf8 {
        for row in 0..array.len() {
            scalar_as_f64_rounded(array.as_ref(), row)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    /// Il confronto tipizzato di `OrdineNumerico` coincide con
    /// `compare_cells_typed` su ogni coppia di righe, anche fuori
    /// dall'array (stesso errore).
    #[test]
    fn l_ordine_tipizzato_coincide_con_compare_cells_typed() {
        let double: ArrayRef = Arc::new(Float64Array::from(vec![
            Some(1.5),
            None,
            Some(-0.0),
            Some(0.0),
            Some(f64::NAN),
            Some(f64::from_bits(0xfff8_0000_0000_0000)),
            Some(f64::from_bits(0x7ff0_0000_0000_0001)),
            Some(f64::INFINITY),
            Some(f64::NEG_INFINITY),
            None,
            Some(1.5),
        ]));
        let interi: ArrayRef = Arc::new(Int64Array::from(vec![
            Some(i64::MAX),
            None,
            Some(i64::MIN),
            Some(0),
            Some(-1),
            Some(i64::MAX),
            None,
        ]));
        for array in [&double, &interi] {
            let ordine = OrdineNumerico::new(array).expect("tipo numerico");
            for sinistra in 0..=array.len() + 1 {
                for destra in 0..=array.len() + 1 {
                    let veloce = ordine.compare(sinistra, destra);
                    let generico = compare_cells_typed(array, sinistra, array, destra);
                    match (veloce, generico) {
                        (Ok(veloce), Ok(generico)) => assert_eq!(veloce, generico),
                        (Err(veloce), Err(generico)) => {
                            assert_eq!(veloce.to_string(), generico.to_string());
                        }
                        (veloce, generico) => panic!(
                            "esiti diversi su ({sinistra}, {destra}): {veloce:?} / {generico:?}"
                        ),
                    }
                }
            }
        }
    }
}
