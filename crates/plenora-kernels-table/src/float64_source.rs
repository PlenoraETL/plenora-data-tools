//! Lettura di una colonna Arrow come `f64` **con arrotondamento dichiarato**,
//! e le letture esatte che la affiancano.
//!
//! Il nome dice la semantica: questa sorgente serve alle operazioni il cui
//! risultato e' un `Float64` per contratto (medie, statistiche, formule),
//! dove il double e' il tipo del risultato e non un passaggio intermedio: un
//! intero oltre 2^53 o un decimale diventano il double piu' vicino, senza
//! errore. Le somme di interi **non** passano di qui: [`SommaEsatta`] le
//! accumula in `i128` ([`ColonnaIntera`]), e gli estremi su interi e
//! decimali si scelgono sul valore esatto ([`ColonnaEsatta`]); i tipi
//! d'uscita delle riduzioni li dicono [`tipo_somma`] e [`tipo_estremo`].
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

use num_traits::ToPrimitive;
use plenora_core::arrow::array::{
    Array, ArrayRef, Date32Array, Decimal128Array, Float64Array, Int64Array,
    TimestampMillisecondArray, UInt32Array, UInt64Array,
};
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

// ---------------------------------------------------------------------------
// Somme esatte e tipi d'uscita delle riduzioni numeriche.
//
// Una somma di interi non passa da `f64`: oltre 2^53 il double arrotonda gia'
// gli addendi, e sommare double accumula l'errore. Sul dominio intero (`Int64`,
// `UInt64`, `Date32` in giorni, `Timestamp(ms)` in millisecondi) la somma si
// accumula in `i128` ed esce `Int64` (errore oltre la gamma); la media e le
// statistiche di dispersione restano `Float64`, calcolate dalla somma esatta.
// Gli estremi (`min`, `max`) su interi e decimali scelgono la riga col
// confronto esatto e tengono il tipo d'ingresso. Autorita' unica per kernel e
// analisi: le funzioni qui sotto dicono il tipo d'uscita di ogni riduzione.
// ---------------------------------------------------------------------------

/// Il tipo e' del dominio intero: la somma e' esatta e rende `Int64`.
#[must_use]
pub const fn dominio_intero(data_type: &DataType) -> bool {
    matches!(
        data_type,
        DataType::Int64
            | DataType::UInt64
            | DataType::Date32
            | DataType::Timestamp(TimeUnit::Millisecond, _)
    )
}

/// Gli estremi del tipo si scelgono sul valore esatto e tengono il tipo
/// d'ingresso: il dominio intero e `Decimal128`.
#[must_use]
pub const fn estremo_esatto(data_type: &DataType) -> bool {
    dominio_intero(data_type) || matches!(data_type, DataType::Decimal128(_, _))
}

/// Una somma (`sum`, `cumsum`) di date o istanti non ha un significato:
/// giorni o millisecondi dall'epoca sommati non sono una data. Si rifiuta,
/// nel kernel e nell'analisi, invece di rendere un `Int64` che sembra un
/// numero. La media resta ammessa: la media di istanti e' un istante, reso
/// come `Float64` nell'unita' della colonna (giorni o millisecondi).
///
/// # Errors
///
/// `InvalidPlan` per `Date32` e `Timestamp`.
pub fn verifica_somma(data_type: &DataType) -> Result<()> {
    if matches!(data_type, DataType::Date32 | DataType::Timestamp(_, _)) {
        return Err(PlenoraError::InvalidPlan(
            "somma di date o istanti non definita: sommare una colonna numerica".into(),
        ));
    }
    Ok(())
}

/// Tipo d'uscita di una somma sul tipo `data_type`: `Int64` sul dominio
/// intero, `Float64` altrimenti (le date e gli istanti si rifiutano prima,
/// [`verifica_somma`]).
#[must_use]
pub const fn tipo_somma(data_type: &DataType) -> DataType {
    if dominio_intero(data_type) {
        DataType::Int64
    } else {
        DataType::Float64
    }
}

/// Tipo d'uscita di `min` e `max` sul tipo `data_type`: il tipo d'ingresso
/// dove l'estremo e' esatto ([`estremo_esatto`]), `Float64` altrimenti (un
/// `Float64` resta tale, il testo numerico diventa il suo double).
#[must_use]
pub fn tipo_estremo(data_type: &DataType) -> DataType {
    if estremo_esatto(data_type) {
        data_type.clone()
    } else {
        DataType::Float64
    }
}

/// Colonna del dominio intero letta come `i128` esatto.
pub enum ColonnaIntera<'a> {
    /// `Int64`.
    Int64(&'a Int64Array),
    /// `UInt64`.
    UInt64(&'a UInt64Array),
    /// `Date32`, in giorni.
    Date32(&'a Date32Array),
    /// `Timestamp(ms)`, in millisecondi.
    TimestampMs(&'a TimestampMillisecondArray),
}

impl<'a> ColonnaIntera<'a> {
    /// `None` se la colonna non e' del dominio intero.
    #[must_use]
    pub fn new(array: &'a ArrayRef) -> Option<Self> {
        let any = array.as_any();
        if let Some(values) = any.downcast_ref::<Int64Array>() {
            return Some(Self::Int64(values));
        }
        if let Some(values) = any.downcast_ref::<UInt64Array>() {
            return Some(Self::UInt64(values));
        }
        if let Some(values) = any.downcast_ref::<Date32Array>() {
            return Some(Self::Date32(values));
        }
        any.downcast_ref::<TimestampMillisecondArray>()
            .map(Self::TimestampMs)
    }

    /// Il valore della riga, `None` se null (questi tipi non hanno null
    /// logici oltre a quelli fisici).
    #[must_use]
    pub fn value(&self, row: usize) -> Option<i128> {
        match self {
            Self::Int64(values) => (!values.is_null(row)).then(|| i128::from(values.value(row))),
            Self::UInt64(values) => (!values.is_null(row)).then(|| i128::from(values.value(row))),
            Self::Date32(values) => (!values.is_null(row)).then(|| i128::from(values.value(row))),
            Self::TimestampMs(values) => {
                (!values.is_null(row)).then(|| i128::from(values.value(row)))
            }
        }
    }
}

/// Somma esatta di interi, con il numero di addendi.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SommaEsatta {
    somma: i128,
    valori: usize,
}

impl SommaEsatta {
    /// Aggiunge un addendo.
    ///
    /// # Errors
    ///
    /// `DataMapping` se la somma esce da `i128` (servirebbero oltre `2^63`
    /// addendi `u64`: irraggiungibile, ma controllato).
    pub fn aggiungi(&mut self, valore: i128) -> Result<()> {
        self.somma = self
            .somma
            .checked_add(valore)
            .ok_or_else(somma_fuori_gamma)?;
        self.valori += 1;
        Ok(())
    }

    /// Il numero di addendi.
    #[must_use]
    pub const fn valori(&self) -> usize {
        self.valori
    }

    /// La somma come `Int64`, il tipo d'uscita delle somme intere.
    ///
    /// # Errors
    ///
    /// `DataMapping` se la somma esce dalla gamma di `i64`: un errore, mai
    /// un valore saturato o arrotondato.
    pub fn in_int64(&self) -> Result<i64> {
        i64::try_from(self.somma).map_err(|_| somma_fuori_gamma())
    }

    /// La media, dalla somma esatta: un solo arrotondamento della somma a
    /// double (nessuno fino a 2^53) e la divisione IEEE. `None` senza
    /// addendi. Non e' la media del vecchio percorso in `f64`, che sommava
    /// arrotondando a ogni passo: `[2^53, 1, -2^53]` dava 0, qui 1/3.
    #[must_use]
    pub fn media(&self) -> Option<f64> {
        if self.valori == 0 {
            return None;
        }
        Some(intero_in_f64(self.somma) / self.valori.to_f64()?)
    }
}

/// Varianza di interi (divisore `valori - ddof`) dagli scarti **esatti**.
///
/// Lo scarto di un valore dalla media esatta `S / n` e' `(n x - S) / n`: il
/// numeratore si calcola in `i128`, senza arrotondare, e si converte in
/// double una volta sola. Sottrarre la media in double da valori oltre
/// 2^53 (arrotondati) darebbe una varianza non nulla a valori tutti uguali:
/// tre volte `2^53 + 1` davano 4 invece di 0. `None` con `valori <= ddof`.
///
/// # Errors
///
/// `DataMapping` se `n x - S` esce da `i128` (valori `u64` e oltre `2^63`
/// addendi: irraggiungibile, ma controllato); `ResourceLimit` per
/// dimensioni non rappresentabili.
pub fn varianza_intera(valori: &[i128], ddof: usize) -> Result<Option<f64>> {
    if valori.len() <= ddof {
        return Ok(None);
    }
    let mut somma = SommaEsatta::default();
    for valore in valori {
        somma.aggiungi(*valore)?;
    }
    let fuori = || PlenoraError::DataMapping("scarto intero oltre la gamma di i128".into());
    let conteggio = i128::try_from(valori.len()).map_err(|_| fuori())?;
    let (Some(n), Some(divisore)) = (valori.len().to_f64(), (valori.len() - ddof).to_f64()) else {
        return Err(PlenoraError::ResourceLimit(
            "divisore statistico non rappresentabile".into(),
        ));
    };
    let mut quadrati = 0.0_f64;
    for valore in valori {
        let scarto = valore
            .checked_mul(conteggio)
            .and_then(|prodotto| prodotto.checked_sub(somma.somma))
            .ok_or_else(fuori)?;
        let scarto = intero_in_f64(scarto) / n;
        // Niente mul_add: la fusione cambierebbe l'arrotondamento secondo la
        // piattaforma; la forma non fusa e' il contratto numerico.
        #[allow(clippy::suboptimal_flops)]
        {
            quadrati += scarto * scarto;
        }
    }
    Ok(Some(quadrati / divisore))
}

fn somma_fuori_gamma() -> PlenoraError {
    PlenoraError::DataMapping("somma intera oltre la gamma di Int64".into())
}

/// Il double piu' vicino a un intero: l'arrotondamento dichiarato delle
/// statistiche `Float64` (esatto fino a 2^53).
#[allow(clippy::cast_precision_loss)] // Arrotondamento dichiarato: vedi il doc.
#[must_use]
pub const fn intero_in_f64(valore: i128) -> f64 {
    valore as f64
}

/// Colonna i cui estremi si scelgono sul valore esatto ([`estremo_esatto`]):
/// il dominio intero come `i128`, un `Decimal128` come intero non scalato
/// (la scala e' una sola per colonna, quindi l'ordine degli interi non
/// scalati e' quello dei valori).
pub enum ColonnaEsatta<'a> {
    /// Dominio intero.
    Intera(ColonnaIntera<'a>),
    /// `Decimal128`, non scalato.
    Decimale(&'a Decimal128Array),
}

impl<'a> ColonnaEsatta<'a> {
    /// `None` se gli estremi del tipo non sono esatti.
    #[must_use]
    pub fn new(array: &'a ArrayRef) -> Option<Self> {
        ColonnaIntera::new(array).map(Self::Intera).or_else(|| {
            array
                .as_any()
                .downcast_ref::<Decimal128Array>()
                .map(Self::Decimale)
        })
    }

    /// Il valore esatto della riga, `None` se null.
    #[must_use]
    pub fn value(&self, row: usize) -> Option<i128> {
        match self {
            Self::Intera(interi) => interi.value(row),
            Self::Decimale(values) => (!values.is_null(row)).then(|| values.value(row)),
        }
    }

    /// L'estremo aggiornato con la riga `riga` (non nulla, di valore
    /// `valore`): resta il corrente se `valore` non lo supera strettamente,
    /// quindi a pari valore vince la riga prima.
    #[must_use]
    pub fn aggiorna(
        corrente: Option<(usize, i128)>,
        riga: usize,
        valore: i128,
        massimo: bool,
    ) -> Option<(usize, i128)> {
        let supera = corrente.is_none_or(|(_, estremo)| {
            if massimo {
                valore > estremo
            } else {
                valore < estremo
            }
        });
        if supera {
            Some((riga, valore))
        } else {
            corrente
        }
    }
}

/// Le celle delle righe scelte, nel tipo d'ingresso (`take`); `None` da'
/// null.
///
/// # Errors
///
/// `ResourceLimit` per un indice oltre `u32`; gli errori Arrow di `take`.
pub fn prendi_righe(array: &ArrayRef, righe: &[Option<usize>]) -> Result<ArrayRef> {
    let indici = righe
        .iter()
        .map(|riga| {
            riga.map(u32::try_from)
                .transpose()
                .map_err(|_| PlenoraError::ResourceLimit("indice riga oltre u32".into()))
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(plenora_core::arrow::select::take::take(
        array.as_ref(),
        &UInt32Array::from(indici),
        None,
    )?)
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

    /// Regressione (revisione Codex): valori interi uguali oltre 2^53 hanno
    /// varianza zero. Sottraendo la media in double, tre volte `2^53 + 1`
    /// davano 4 con `ddof = 0` e 6 con `ddof = 1`.
    #[test]
    #[allow(clippy::float_cmp)] // Zero esatto.
    fn la_varianza_di_interi_uguali_e_zero() {
        let tre = [9_007_199_254_740_993_i128; 3];
        assert_eq!(varianza_intera(&tre, 0).expect("ok"), Some(0.0));
        assert_eq!(varianza_intera(&tre, 1).expect("ok"), Some(0.0));
        assert_eq!(varianza_intera(&tre, 3).expect("ok"), None);
        let estremi = [i128::from(u64::MAX); 4];
        assert_eq!(varianza_intera(&estremi, 1).expect("ok"), Some(0.0));
    }

    /// Oracolo razionale esatto: la varianza e' `sum (n x - S)^2 / (n^2 (n -
    /// ddof))`, calcolata in interi senza arrotondare; il risultato del
    /// kernel ne dista al piu' pochi ulp, su basi oltre 2^53 dove la media in
    /// double sbaglierebbe. Generatore deterministico.
    #[test]
    #[allow(clippy::float_cmp)] // Zero esatto quando l oracolo e zero.
    fn la_varianza_intera_segue_l_oracolo_razionale() {
        let mut stato = 0x9e37_79b9_7f4a_7c15_u64;
        let mut prossimo = || {
            stato ^= stato << 13;
            stato ^= stato >> 7;
            stato ^= stato << 17;
            stato
        };
        for caso in 0..2_000 {
            let base = i128::from(prossimo() >> 1) * if caso % 2 == 0 { 1 } else { -1 };
            let n = 1 + usize::try_from(prossimo() % 40).expect("piccolo");
            let valori = (0..n)
                .map(|_| base + i128::from(prossimo() % 2001) - 1000)
                .collect::<Vec<_>>();
            for ddof in [0_usize, 1] {
                let kernel = varianza_intera(&valori, ddof).expect("nessun trabocco");
                if n <= ddof {
                    assert_eq!(kernel, None);
                    continue;
                }
                let nn = i128::try_from(n).expect("n");
                let somma = valori.iter().sum::<i128>();
                let numeratore = valori
                    .iter()
                    .map(|valore| {
                        let scarto = valore * nn - somma;
                        u128::try_from(scarto * scarto).expect("quadrato")
                    })
                    .sum::<u128>();
                let denominatore =
                    u128::try_from(nn * nn * (nn - i128::try_from(ddof).expect("ddof")))
                        .expect("denominatore");
                #[allow(clippy::cast_precision_loss)]
                let esatta = numeratore as f64 / denominatore as f64;
                let kernel = kernel.expect("valore");
                if esatta == 0.0 {
                    assert_eq!(kernel, 0.0, "caso {caso}");
                } else {
                    let errore = ((kernel - esatta) / esatta).abs();
                    assert!(errore < 1e-12, "caso {caso}: {kernel} contro {esatta}");
                }
            }
        }
    }

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
