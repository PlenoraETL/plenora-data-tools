//! Lettura di una colonna Arrow come `f64` **con arrotondamento dichiarato**.
//!
//! Il nome dice la semantica: questa sorgente serve alle operazioni il cui
//! risultato e' un `Float64` per contratto, dove il double e' il tipo del
//! risultato e non un passaggio intermedio
//! (errori-e-limiti.md#arrotondamento-nelle-operazioni-a-risultato-float64).
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
    Float64(&'a Float64Array),
    Int64(&'a Int64Array),
    UInt64(&'a UInt64Array),
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
pub struct OrdineNumerico<'a>(&'a ArrayRef);

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
        Ok(Self(array))
    }

    /// Confronta due righe nel dominio originale.
    ///
    /// # Errors
    ///
    /// Gli errori di [`compare_cells_typed`].
    pub fn compare(&self, sinistra: usize, destra: usize) -> Result<Ordering> {
        compare_cells_typed(self.0, sinistra, self.0, destra)
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
