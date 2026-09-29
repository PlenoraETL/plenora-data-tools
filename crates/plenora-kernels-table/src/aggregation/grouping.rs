use std::borrow::Cow;
use std::cmp::Ordering;
use std::collections::HashMap;
use std::fmt::Write as _;

use plenora_core::arrow::array::{
    Array, ArrayRef, BooleanArray, Float64Array, Int64Array, RecordBatch, StringArray, UInt64Array,
};
use rayon::prelude::*;

use plenora_core::{PlenoraError, Result};

use crate::hashing::FastHasher;
use crate::interning::KeyInterner;
use crate::scalar_as_string;

// ---------------------------------------------------------------------------
// Fast path di `table.aggregate`.
//
// Semantica byte-identica al percorso generico:
// 1. identita' di gruppo dal valore nativo (colonna singola) o da chiavi
//    binarie con la stessa identita' dei byte di `row_key`
//    (`BinaryKeyEncoder`), ordinamento finale dei gruppi con comparatori che
//    riproducono l'ordine lessicografico delle chiavi testuali (stesso
//    ordine del BTreeMap); `KeyColumn` resta il formato testuale dello
//    spill;
// 2. aggregazioni numeriche su valori nativi Int64/UInt64/Float64 con la
//    stessa sequenza di operazioni del generico; gli altri tipi ricadono su
//    `scalar_as_f64_rounded`;
// 3. nunique/concat su Utf8 senza copie; gli altri tipi ricadono su
//    `scalar_as_string`.
// ---------------------------------------------------------------------------

/// Colonna di group-by con formattatore tipizzato: produce gli stessi byte
/// di `row_key` (`{tipo}\u{1e}{1|0}{len}:{value}\u{1f}`).
///
/// `pub(crate)` per il modulo `spill`: il partizionamento hash e la
/// ricostruzione dell'ordine canonico dei gruppi riusano gli stessi byte di
/// chiave, cosi' i percorsi spilled hanno identita' di gruppo identica.
pub enum KeyColumn {
    Int64 {
        prefix: String,
        values: Int64Array,
    },
    UInt64 {
        prefix: String,
        values: UInt64Array,
    },
    Float64 {
        prefix: String,
        values: Float64Array,
    },
    Utf8 {
        prefix: String,
        values: StringArray,
    },
    Boolean {
        prefix: String,
        values: BooleanArray,
    },
    /// Qualunque altro tipo: chiave via `scalar_as_string`, il percorso
    /// generico.
    Generic {
        prefix: String,
        array: ArrayRef,
    },
}

impl KeyColumn {
    pub(crate) fn new(array: &ArrayRef) -> Self {
        let prefix = array.data_type().to_string();
        if let Some(values) = array.as_any().downcast_ref::<Int64Array>() {
            return Self::Int64 {
                prefix,
                values: values.clone(),
            };
        }
        if let Some(values) = array.as_any().downcast_ref::<UInt64Array>() {
            return Self::UInt64 {
                prefix,
                values: values.clone(),
            };
        }
        if let Some(values) = array.as_any().downcast_ref::<Float64Array>() {
            return Self::Float64 {
                prefix,
                values: values.clone(),
            };
        }
        if let Some(values) = array.as_any().downcast_ref::<StringArray>() {
            return Self::Utf8 {
                prefix,
                values: values.clone(),
            };
        }
        if let Some(values) = array.as_any().downcast_ref::<BooleanArray>() {
            return Self::Boolean {
                prefix,
                values: values.clone(),
            };
        }
        Self::Generic {
            prefix,
            array: array.clone(),
        }
    }

    pub(crate) fn write_key(
        &self,
        row: usize,
        key: &mut String,
        scratch: &mut String,
    ) -> Result<()> {
        match self {
            Self::Int64 { prefix, values } => {
                key.push_str(prefix);
                key.push('\u{1e}');
                if values.is_null(row) {
                    key.push('0');
                } else {
                    scratch.clear();
                    // La scrittura su String non fallisce mai; l'errore resta
                    // esplicito perche' fmt::Result non lo dimostra (R6).
                    write!(scratch, "{}", values.value(row)).map_err(|_| {
                        PlenoraError::Internal("formattazione chiave di gruppo su String".into())
                    })?;
                    push_key_value(key, scratch)?;
                }
            }
            Self::UInt64 { prefix, values } => {
                key.push_str(prefix);
                key.push('\u{1e}');
                if values.is_null(row) {
                    key.push('0');
                } else {
                    scratch.clear();
                    write!(scratch, "{}", values.value(row)).map_err(|_| {
                        PlenoraError::Internal("formattazione chiave di gruppo su String".into())
                    })?;
                    push_key_value(key, scratch)?;
                }
            }
            Self::Float64 { prefix, values } => {
                key.push_str(prefix);
                key.push('\u{1e}');
                if values.is_null(row) {
                    key.push('0');
                } else {
                    scratch.clear();
                    write!(scratch, "{}", values.value(row)).map_err(|_| {
                        PlenoraError::Internal("formattazione chiave di gruppo su String".into())
                    })?;
                    push_key_value(key, scratch)?;
                }
            }
            Self::Boolean { prefix, values } => {
                key.push_str(prefix);
                key.push('\u{1e}');
                if values.is_null(row) {
                    key.push('0');
                } else {
                    // "true"/"false": stessi byte di bool::to_string.
                    push_key_value(key, if values.value(row) { "true" } else { "false" })?;
                }
            }
            Self::Utf8 { prefix, values } => {
                key.push_str(prefix);
                key.push('\u{1e}');
                if values.is_null(row) {
                    key.push('0');
                } else {
                    push_key_value(key, values.value(row))?;
                }
            }
            Self::Generic { prefix, array } => {
                key.push_str(prefix);
                key.push('\u{1e}');
                match scalar_as_string(array.as_ref(), row)? {
                    Some(value) => push_key_value(key, &value)?,
                    None => key.push('0'),
                }
            }
        }
        key.push('\u{1f}');
        Ok(())
    }
}

/// Frammento `1{len}:{value}` della chiave di `row_key`.
fn push_key_value(key: &mut String, value: &str) -> Result<()> {
    key.push('1');
    // Come sopra: fmt su String e' infallibile, ma l'errore e' esplicito.
    write!(key, "{}", value.len())
        .map_err(|_| PlenoraError::Internal("formattazione chiave di gruppo su String".into()))?;
    key.push(':');
    key.push_str(value);
    Ok(())
}

/// Soglia condivisa per l'uso di rayon (ordinamento chiavi e calcolo per
/// gruppo): sotto soglia l'overhead non ripaga.
pub(in crate::aggregation) const PARALLEL_THRESHOLD: usize = 32_768;

/// Cifre decimali di un intero senza segno (0 conta come una cifra).
const fn decimal_digits(value: u64) -> u32 {
    if value == 0 {
        1
    } else {
        value.ilog10() + 1
    }
}

/// Confronto lessicografico dei "tag di lunghezza" `{decimal(len)}:` delle
/// chiavi di `row_key`.
///
/// Quando una rappresentazione finisce, il suo byte successivo nella chiave
/// e' ':' (0x3A), maggiore di ogni cifra: il piu' corto, a parita' di
/// prefisso, e' quindi MAGGIORE.
fn cmp_len_tag(a: u64, b: u64) -> Ordering {
    let digits_a = decimal_digits(a);
    let digits_b = decimal_digits(b);
    if digits_a == digits_b {
        return a.cmp(&b);
    }
    if digits_a < digits_b {
        let prefix = b / 10_u64.pow(digits_b - digits_a);
        a.cmp(&prefix).then(Ordering::Greater)
    } else {
        let prefix = a / 10_u64.pow(digits_a - digits_b);
        prefix.cmp(&b).then(Ordering::Less)
    }
}

/// Ordine delle chiavi di `row_key` per una colonna Int64, senza
/// materializzare le stringhe.
///
/// Null in testa (gestito dal chiamante), poi tag di lunghezza, poi forma
/// decimale (il segno '-' precede le cifre, tra negativi l'ordine
/// lessicografico e' l'inverso di quello numerico).
pub(in crate::aggregation) fn cmp_i64_group_key(a: i64, b: i64) -> Ordering {
    let digits_a = u64::from(decimal_digits(a.unsigned_abs()));
    let digits_b = u64::from(decimal_digits(b.unsigned_abs()));
    cmp_len_tag(digits_a + u64::from(a < 0), digits_b + u64::from(b < 0)).then_with(|| {
        match (a < 0, b < 0) {
            (true, false) => Ordering::Less,
            (false, true) => Ordering::Greater,
            (true, true) => b.cmp(&a),
            (false, false) => a.cmp(&b),
        }
    })
}

/// Ordine delle chiavi di `row_key` per una colonna `UInt64`: tag di
/// lunghezza della forma decimale, poi valore (a parita' di cifre
/// lessicografico e numerico coincidono).
pub(in crate::aggregation) fn cmp_u64_group_key(a: u64, b: u64) -> Ordering {
    cmp_len_tag(u64::from(decimal_digits(a)), u64::from(decimal_digits(b))).then_with(|| a.cmp(&b))
}

/// Ordine delle chiavi di `row_key` per una colonna Utf8: tag di lunghezza
/// in byte, poi confronto per byte.
pub(in crate::aggregation) fn cmp_str_group_key(a: &str, b: &str) -> Ordering {
    cmp_len_tag(a.len() as u64, b.len() as u64).then_with(|| a.cmp(b))
}

/// Gruppi di righe nell'ordine canonico, con le righe di ogni gruppo in
/// ordine crescente di indice.
///
/// Due forme, scelte da [`GroupAccumulator`] sul numero di gruppi:
///
/// - `Lists`: un vettore per gruppo, la forma storica. Con pochi gruppi
///   grandi costa meno: nessuna seconda passata di distribuzione.
/// - `Compact` (CSR): le righe di tutti i gruppi in un solo vettore, gruppo
///   dopo gruppo, piu' gli offset di inizio. Con molti gruppi piccoli evita
///   un'allocazione e un rilascio per gruppo: con un milione di gruppi da
///   una riga erano un milione di allocazioni.
///
/// La forma non e' osservabile: [`Groups::iter`] e [`map_groups`] danno le
/// stesse fette nello stesso ordine.
pub(in crate::aggregation) enum Groups {
    Lists(Vec<Vec<usize>>),
    Compact {
        /// `offsets[g]..offsets[g + 1]`: le righe del gruppo `g` in `rows`.
        offsets: Vec<usize>,
        rows: Vec<usize>,
    },
}

impl Groups {
    /// Numero di gruppi.
    pub(in crate::aggregation) const fn len(&self) -> usize {
        match self {
            Self::Lists(lists) => lists.len(),
            Self::Compact { offsets, .. } => offsets.len().saturating_sub(1),
        }
    }

    /// Righe del gruppo `gruppo`, crescenti.
    fn get(&self, gruppo: usize) -> &[usize] {
        match self {
            Self::Lists(lists) => &lists[gruppo],
            Self::Compact { offsets, rows } => &rows[offsets[gruppo]..offsets[gruppo + 1]],
        }
    }

    /// Gruppi nell'ordine canonico.
    pub(in crate::aggregation) fn iter(&self) -> impl Iterator<Item = &[usize]> + '_ {
        (0..self.len()).map(|gruppo| self.get(gruppo))
    }
}

/// Numero di gruppi oltre il quale [`GroupAccumulator`] passa alla forma
/// compatta: gruppi di meno di 16 righe in media, e comunque oltre 1024
/// gruppi. Sotto, il costo delle allocazioni per gruppo e' minore di quello
/// della passata di distribuzione del CSR.
fn soglia_compatta(righe: usize) -> usize {
    (righe / 16).max(1024)
}

/// Accumula l'assegnazione riga -> gruppo provvisorio in una scansione per
/// righe crescenti, e produce i [`Groups`] nell'ordine canonico.
///
/// Parte nella forma a liste; quando i gruppi superano [`soglia_compatta`]
/// converte quanto raccolto in assegnazione per riga e prosegue in forma
/// compatta.
struct GroupAccumulator {
    /// Righe della scansione (capacita' della forma compatta).
    righe: usize,
    soglia: usize,
    lists: Vec<Vec<usize>>,
    /// Righe gia' registrate: la prossima deve essere esattamente questa.
    viste: usize,
    /// Forma compatta: gruppo provvisorio di ogni riga gia' vista.
    row_group: Vec<usize>,
    /// Forma compatta: righe per gruppo provvisorio.
    conteggi: Vec<usize>,
    compatta: bool,
}

impl GroupAccumulator {
    fn new(righe: usize) -> Self {
        Self {
            righe,
            soglia: soglia_compatta(righe),
            lists: Vec::new(),
            viste: 0,
            row_group: Vec::new(),
            conteggi: Vec::new(),
            compatta: false,
        }
    }

    const fn gruppi(&self) -> usize {
        if self.compatta {
            self.conteggi.len()
        } else {
            self.lists.len()
        }
    }

    /// Apre un gruppo provvisorio e ne restituisce l'indice.
    fn nuovo_gruppo(&mut self) -> usize {
        let gruppo = self.gruppi();
        if self.compatta {
            self.conteggi.push(0);
        } else {
            self.lists.push(Vec::new());
            if self.lists.len() > self.soglia {
                self.compatta_ora();
            }
        }
        gruppo
    }

    /// Passaggio alla forma compatta: le righe sono arrivate in ordine
    /// crescente e contiguo da zero (lo verifica `aggiungi`), quindi le liste
    /// ricostruiscono l'assegnazione di ognuna delle `viste` righe.
    fn compatta_ora(&mut self) {
        // Capacita' per tutte le righe della scansione: niente
        // riallocazioni a raddoppio sul vettore piu' grande.
        let mut row_group = Vec::with_capacity(self.righe.max(self.viste));
        row_group.resize(self.viste, 0_usize);
        for (gruppo, rows) in self.lists.iter().enumerate() {
            for row in rows {
                row_group[*row] = gruppo;
            }
        }
        self.conteggi = self.lists.iter().map(Vec::len).collect();
        self.row_group = row_group;
        self.lists = Vec::new();
        self.compatta = true;
    }

    /// Registra `row`, la successiva della scansione, nel gruppo `gruppo`.
    ///
    /// # Errors
    ///
    /// `Internal` se `row` non e' la riga successiva (scansione non
    /// contigua da zero) o se il gruppo non esiste: invarianti del chiamante.
    #[inline]
    fn aggiungi(&mut self, row: usize, gruppo: usize) -> Result<()> {
        let invariante = || PlenoraError::Internal("riga assegnata a un gruppo inesistente".into());
        if row != self.viste {
            return Err(PlenoraError::Internal(
                "scansione dei gruppi non contigua".into(),
            ));
        }
        self.viste += 1;
        if self.compatta {
            *self.conteggi.get_mut(gruppo).ok_or_else(invariante)? += 1;
            self.row_group.push(gruppo);
        } else {
            self.lists.get_mut(gruppo).ok_or_else(invariante)?.push(row);
        }
        Ok(())
    }

    /// Gruppi nell'ordine canonico: `order[posizione]` e' il gruppo
    /// provvisorio che occupa `posizione`.
    ///
    /// # Errors
    ///
    /// `Internal` se `order` non e' una permutazione dei gruppi provvisori o
    /// se i conteggi non corrispondono all'assegnazione: invarianti del
    /// chiamante, mai un dato.
    fn finisci(self, order: &[usize]) -> Result<Groups> {
        let invariante =
            || PlenoraError::Internal("assegnazione dei gruppi incoerente con l'ordine".into());
        if order.len() != self.gruppi() {
            return Err(invariante());
        }
        if !self.compatta {
            let mut lists = self.lists;
            let mut collocati = vec![false; lists.len()];
            let mut ordinate = Vec::with_capacity(lists.len());
            for gruppo in order {
                let segnato = collocati.get_mut(*gruppo).ok_or_else(invariante)?;
                if *segnato {
                    return Err(invariante());
                }
                *segnato = true;
                ordinate.push(std::mem::take(&mut lists[*gruppo]));
            }
            return Ok(Groups::Lists(ordinate));
        }
        // `liberi[g]`: prossima posizione libera del gruppo provvisorio `g`
        // in `rows`; `usize::MAX` finche' `order` non lo ha collocato.
        let mut liberi = vec![usize::MAX; self.conteggi.len()];
        let mut offsets = Vec::with_capacity(order.len() + 1);
        let mut fine = 0_usize;
        offsets.push(fine);
        for gruppo in order {
            let inizio = liberi.get_mut(*gruppo).ok_or_else(invariante)?;
            if *inizio != usize::MAX {
                return Err(invariante());
            }
            *inizio = fine;
            fine = fine
                .checked_add(self.conteggi[*gruppo])
                .ok_or_else(invariante)?;
            offsets.push(fine);
        }
        if fine != self.row_group.len() {
            return Err(invariante());
        }
        let mut rows = vec![0_usize; self.row_group.len()];
        for (row, gruppo) in self.row_group.iter().enumerate() {
            let prossima = liberi.get_mut(*gruppo).ok_or_else(invariante)?;
            *rows.get_mut(*prossima).ok_or_else(invariante)? = row;
            *prossima += 1;
        }
        // Ogni gruppo deve aver riempito esattamente il proprio intervallo:
        // un conteggio sbagliato sconfinerebbe nel gruppo successivo senza
        // uscire da `rows`.
        for (posizione, gruppo) in order.iter().enumerate() {
            if liberi[*gruppo] != offsets[posizione + 1] {
                return Err(invariante());
            }
        }
        Ok(Groups::Compact { offsets, rows })
    }
}

/// Raggruppamento su chiave nativa di colonna singola (Int64/UInt64/Utf8).
///
/// Nessuna stringa di chiave, hash del valore nativo, ordinamento finale
/// con il comparatore che riproduce l'ordine lessicografico delle chiavi
/// di `row_key`. Il gruppo dei null, se presente, e' sempre in testa
/// (`"...0"` precede `"...1..."` nelle chiavi testuali).
///
/// # Errors
///
/// Solo invarianti interne (`GroupAccumulator`).
pub(in crate::aggregation) fn build_native_groups<K: Copy + Eq + std::hash::Hash + Send + Sync>(
    rows: usize,
    key_at: impl Fn(usize) -> Option<K> + Sync,
    cmp: impl Fn(&K, &K) -> Ordering + Sync,
) -> Result<Groups> {
    let mut lookup: HashMap<K, usize, FastHasher> = HashMap::default();
    let mut null_group: Option<usize> = None;
    let mut accumulo = GroupAccumulator::new(rows);
    for row in 0..rows {
        let gruppo = match key_at(row) {
            Some(key) => *lookup.entry(key).or_insert_with(|| accumulo.nuovo_gruppo()),
            None => *null_group.get_or_insert_with(|| accumulo.nuovo_gruppo()),
        };
        accumulo.aggiungi(row, gruppo)?;
    }
    let mut keyed = lookup.into_iter().collect::<Vec<_>>();
    if keyed.len() >= PARALLEL_THRESHOLD {
        keyed.par_sort_by(|left, right| cmp(&left.0, &right.0));
    } else {
        keyed.sort_by(|left, right| cmp(&left.0, &right.0));
    }
    let order = null_group
        .into_iter()
        .chain(keyed.iter().map(|(_, gruppo)| *gruppo))
        .collect::<Vec<_>>();
    drop(keyed);
    accumulo.finisci(&order)
}

/// Colonna di chiave di gruppo in forma binaria.
///
/// Identita' IDENTICA a quella dei byte di `row_key` (`KeyColumn`): due
/// celle hanno la stessa codifica se e solo se hanno lo stesso testo di
/// `scalar_as_string`. Per tipo:
///
/// - Int64/UInt64/Boolean: il valore (la forma decimale e' biiettiva);
/// - Float64: i bit, con ogni NaN ricondotto a un solo NaN. Il testo di
///   `Display` e' la rappresentazione piu' corta che ritorna allo stesso
///   double, quindi e' iniettivo sui non-NaN (`-0` e `0` restano distinti,
///   come gli infiniti), e scrive `NaN` per ogni payload e segno;
/// - Utf8: i byte;
/// - ogni altro tipo: il testo di `scalar_as_string`, con gli stessi errori
///   alla stessa riga (una cella nulla non passa dal convertitore, come in
///   `KeyColumn`).
///
/// Ogni frammento e' autodelimitato (marcatore di null, poi larghezza fissa
/// o lunghezza a 8 byte): la concatenazione delle colonne e' iniettiva, e
/// ("ab","c") resta distinto da ("a","bc").
enum BinaryKeyColumn<'a> {
    Int64(&'a Int64Array),
    UInt64(&'a UInt64Array),
    Float64(&'a Float64Array),
    Utf8(&'a StringArray),
    Boolean(&'a BooleanArray),
    Generic(&'a ArrayRef),
}

/// Bit del NaN canonico della chiave binaria (quelli di `f64::NAN`).
const NAN_CANONICO: u64 = 0x7ff8_0000_0000_0000;

impl<'a> BinaryKeyColumn<'a> {
    fn new(array: &'a ArrayRef) -> Self {
        // Stessi downcast di `KeyColumn::new`: la variante scelta deve essere
        // la stessa, o gli errori divergerebbero.
        if let Some(values) = array.as_any().downcast_ref::<Int64Array>() {
            return Self::Int64(values);
        }
        if let Some(values) = array.as_any().downcast_ref::<UInt64Array>() {
            return Self::UInt64(values);
        }
        if let Some(values) = array.as_any().downcast_ref::<Float64Array>() {
            return Self::Float64(values);
        }
        if let Some(values) = array.as_any().downcast_ref::<StringArray>() {
            return Self::Utf8(values);
        }
        if let Some(values) = array.as_any().downcast_ref::<BooleanArray>() {
            return Self::Boolean(values);
        }
        Self::Generic(array)
    }

    fn encode(&self, row: usize, key: &mut Vec<u8>) -> Result<()> {
        match self {
            Self::Int64(values) => {
                if values.is_null(row) {
                    key.push(0);
                } else {
                    key.push(1);
                    key.extend_from_slice(&values.value(row).to_be_bytes());
                }
            }
            Self::UInt64(values) => {
                if values.is_null(row) {
                    key.push(0);
                } else {
                    key.push(1);
                    key.extend_from_slice(&values.value(row).to_be_bytes());
                }
            }
            Self::Float64(values) => {
                if values.is_null(row) {
                    key.push(0);
                } else {
                    let value = values.value(row);
                    let bits = if value.is_nan() {
                        NAN_CANONICO
                    } else {
                        value.to_bits()
                    };
                    key.push(1);
                    key.extend_from_slice(&bits.to_be_bytes());
                }
            }
            Self::Utf8(values) => {
                if values.is_null(row) {
                    key.push(0);
                } else {
                    push_binary_value(key, values.value(row).as_bytes());
                }
            }
            Self::Boolean(values) => {
                if values.is_null(row) {
                    key.push(0);
                } else {
                    key.push(1);
                    key.push(u8::from(values.value(row)));
                }
            }
            Self::Generic(array) => match scalar_as_string(array.as_ref(), row)? {
                Some(value) => push_binary_value(key, value.as_bytes()),
                None => key.push(0),
            },
        }
        Ok(())
    }
}

/// Frammento non nullo a lunghezza variabile: marcatore, lunghezza a 8 byte,
/// byte.
fn push_binary_value(key: &mut Vec<u8>, value: &[u8]) {
    key.push(1);
    key.extend_from_slice(&(value.len() as u64).to_be_bytes());
    key.extend_from_slice(value);
}

/// Encoder binario delle chiavi di gruppo di una riga (vedi
/// [`BinaryKeyColumn`]).
pub struct BinaryKeyEncoder<'a> {
    columns: Vec<BinaryKeyColumn<'a>>,
}

impl<'a> BinaryKeyEncoder<'a> {
    pub fn new(batch: &'a RecordBatch, indices: &[usize]) -> Self {
        Self {
            columns: indices
                .iter()
                .map(|index| BinaryKeyColumn::new(batch.column(*index)))
                .collect(),
        }
    }

    /// Scrive in `key` (svuotato e riusato fra le righe) la chiave della
    /// riga `row`.
    ///
    /// # Errors
    ///
    /// Gli errori di `scalar_as_string` sulle colonne fuori dai tipi nativi,
    /// nello stesso ordine riga-colonna di `row_key`.
    pub fn encode_into(&self, row: usize, key: &mut Vec<u8>) -> Result<()> {
        key.clear();
        for column in &self.columns {
            column.encode(row, key)?;
        }
        Ok(())
    }
}

/// Assegna a ogni riga, in ordine crescente, l'indice della sua chiave sulle
/// colonne `indices`: indici densi da zero in ordine di prima apparizione,
/// con l'identita' di [`BinaryKeyEncoder`] (quella dei byte di `row_key`).
///
/// `visita(row, indice, nuova)` riceve ogni riga; `nuova` e' vero alla prima
/// occorrenza della chiave. Un errore di `visita` interrompe la scansione.
///
/// Su una sola colonna Int64/UInt64/Float64/Utf8/Boolean la chiave e' il
/// valore nativo in una mappa (i bit per Float64, con il NaN canonico),
/// il null un indice a parte: stessa identita' senza codifica ne' arena.
/// Altrimenti chiave binaria in un [`KeyInterner`].
///
/// # Errors
///
/// Gli errori di `scalar_as_string` sulle colonne generiche, alla stessa
/// riga del percorso testuale, e quelli di `visita`.
pub fn visit_key_ids(
    batch: &RecordBatch,
    indices: &[usize],
    visita: impl FnMut(usize, usize, bool) -> Result<()>,
) -> Result<()> {
    visit_key_ids_where(batch, indices, |_| true, visita)
}

/// Come [`visit_key_ids`], sulle sole righe per cui `include` e' vero: le
/// altre non sono ne' codificate (quindi non producono errori di
/// conversione) ne' visitate.
///
/// # Errors
///
/// Come [`visit_key_ids`].
pub fn visit_key_ids_where(
    batch: &RecordBatch,
    indices: &[usize],
    include: impl Fn(usize) -> bool,
    mut visita: impl FnMut(usize, usize, bool) -> Result<()>,
) -> Result<()> {
    if let [index] = indices {
        let array = batch.column(*index);
        if let Some(values) = array.as_any().downcast_ref::<Int64Array>() {
            return visit_native_ids(
                batch.num_rows(),
                &include,
                |row| (!values.is_null(row)).then(|| values.value(row)),
                visita,
            );
        }
        if let Some(values) = array.as_any().downcast_ref::<UInt64Array>() {
            return visit_native_ids(
                batch.num_rows(),
                &include,
                |row| (!values.is_null(row)).then(|| values.value(row)),
                visita,
            );
        }
        if let Some(values) = array.as_any().downcast_ref::<Float64Array>() {
            return visit_native_ids(
                batch.num_rows(),
                &include,
                |row| {
                    (!values.is_null(row)).then(|| {
                        let value = values.value(row);
                        if value.is_nan() {
                            NAN_CANONICO
                        } else {
                            value.to_bits()
                        }
                    })
                },
                visita,
            );
        }
        if let Some(values) = array.as_any().downcast_ref::<StringArray>() {
            return visit_native_ids(
                batch.num_rows(),
                &include,
                |row| (!values.is_null(row)).then(|| values.value(row)),
                visita,
            );
        }
        if let Some(values) = array.as_any().downcast_ref::<BooleanArray>() {
            return visit_native_ids(
                batch.num_rows(),
                &include,
                |row| (!values.is_null(row)).then(|| values.value(row)),
                visita,
            );
        }
    }
    let encoder = BinaryKeyEncoder::new(batch, indices);
    let mut interner = KeyInterner::with_capacity(0);
    let mut key = Vec::new();
    for row in (0..batch.num_rows()).filter(|row| include(*row)) {
        encoder.encode_into(row, &mut key)?;
        let (indice, nuova) = interner.inserisci(&key);
        visita(row, indice, nuova)?;
    }
    Ok(())
}

/// Ramo nativo di [`visit_key_ids`]: `key_at` da' `None` per il null.
fn visit_native_ids<K: Eq + std::hash::Hash>(
    rows: usize,
    include: &impl Fn(usize) -> bool,
    key_at: impl Fn(usize) -> Option<K>,
    mut visita: impl FnMut(usize, usize, bool) -> Result<()>,
) -> Result<()> {
    let mut lookup: HashMap<K, usize, FastHasher> = HashMap::default();
    let mut null_id: Option<usize> = None;
    let mut prossimo = 0_usize;
    for row in (0..rows).filter(|row| include(*row)) {
        let mut nuova = false;
        let indice = match key_at(row) {
            Some(key) => *lookup.entry(key).or_insert_with(|| {
                nuova = true;
                prossimo
            }),
            None => *null_id.get_or_insert_with(|| {
                nuova = true;
                prossimo
            }),
        };
        if nuova {
            prossimo += 1;
        }
        visita(row, indice, nuova)?;
    }
    Ok(())
}

/// Ordine canonico di una colonna di gruppo: quello lessicografico dei
/// frammenti di `row_key`, calcolato sulle righe rappresentative dei gruppi
/// (una per gruppo, mai una per riga).
///
/// Null prima dei valori (`"0"` precede `"1..."`), poi il tag di lunghezza e
/// il testo del valore (`cmp_len_tag`, o i comparatori nativi che lo
/// riproducono senza materializzare il testo).
enum OrderColumn<'a> {
    Int64(&'a Int64Array),
    UInt64(&'a UInt64Array),
    Utf8(&'a StringArray),
    Boolean(&'a BooleanArray),
    /// Testo di `KeyColumn` per gruppo provvisorio (Float64 e tipi
    /// generici): `None` per il null.
    Text(Vec<Option<String>>),
}

impl<'a> OrderColumn<'a> {
    fn new(array: &'a ArrayRef, representatives: &[usize]) -> Result<Self> {
        if let Some(values) = array.as_any().downcast_ref::<Int64Array>() {
            return Ok(Self::Int64(values));
        }
        if let Some(values) = array.as_any().downcast_ref::<UInt64Array>() {
            return Ok(Self::UInt64(values));
        }
        if let Some(values) = array.as_any().downcast_ref::<StringArray>() {
            return Ok(Self::Utf8(values));
        }
        if let Some(values) = array.as_any().downcast_ref::<BooleanArray>() {
            return Ok(Self::Boolean(values));
        }
        let float = array.as_any().downcast_ref::<Float64Array>();
        let texts = representatives
            .iter()
            .map(|row| {
                if let Some(values) = float {
                    if values.is_null(*row) {
                        return Ok(None);
                    }
                    let mut text = String::new();
                    // Stessi byte di `KeyColumn::write_key` (fmt su String e'
                    // infallibile; l'errore resta esplicito, R6).
                    write!(text, "{}", values.value(*row)).map_err(|_| {
                        PlenoraError::Internal("formattazione chiave di gruppo su String".into())
                    })?;
                    return Ok(Some(text));
                }
                scalar_as_string(array.as_ref(), *row)
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(Self::Text(texts))
    }

    /// Confronto fra i gruppi provvisori `a` e `b`, rappresentati dalle
    /// righe `row_a` e `row_b`.
    fn compare(&self, a: usize, row_a: usize, b: usize, row_b: usize) -> Ordering {
        match self {
            Self::Int64(values) => nulls_first(
                (!values.is_null(row_a)).then(|| values.value(row_a)),
                (!values.is_null(row_b)).then(|| values.value(row_b)),
                cmp_i64_group_key,
            ),
            Self::UInt64(values) => nulls_first(
                (!values.is_null(row_a)).then(|| values.value(row_a)),
                (!values.is_null(row_b)).then(|| values.value(row_b)),
                cmp_u64_group_key,
            ),
            Self::Utf8(values) => nulls_first(
                (!values.is_null(row_a)).then(|| values.value(row_a)),
                (!values.is_null(row_b)).then(|| values.value(row_b)),
                cmp_str_group_key,
            ),
            Self::Boolean(values) => {
                // Testo "true"/"false", come `bool::to_string`.
                let testo = |row: usize| if values.value(row) { "true" } else { "false" };
                nulls_first(
                    (!values.is_null(row_a)).then(|| testo(row_a)),
                    (!values.is_null(row_b)).then(|| testo(row_b)),
                    cmp_str_group_key,
                )
            }
            Self::Text(texts) => {
                nulls_first(texts[a].as_deref(), texts[b].as_deref(), cmp_str_group_key)
            }
        }
    }
}

/// Null prima dei valori, poi `cmp` sui valori.
fn nulls_first<T>(a: Option<T>, b: Option<T>, cmp: impl FnOnce(T, T) -> Ordering) -> Ordering {
    match (a, b) {
        (None, None) => Ordering::Equal,
        (None, Some(_)) => Ordering::Less,
        (Some(_), None) => Ordering::Greater,
        (Some(a), Some(b)) => cmp(a, b),
    }
}

/// Raggruppamento generico su chiavi binarie (multi-colonna o tipi fuori
/// dal fast path nativo).
///
/// Identita' di gruppo da [`BinaryKeyEncoder`] (la stessa dei byte di
/// `row_key`) in un [`KeyInterner`]; ordine finale dei gruppi uguale a
/// quello lessicografico delle chiavi testuali di `row_key`, calcolato
/// colonna per colonna sui rappresentanti. Il confronto per colonne
/// coincide con quello della stringa intera: i frammenti di una colonna
/// hanno lo stesso prefisso di tipo e sono autodelimitati (tag di
/// lunghezza, poi il valore), quindi la prima differenza cade sempre dentro
/// il frammento della prima colonna diversa.
///
/// # Errors
///
/// Gli errori di `scalar_as_string` sulle colonne generiche, alla stessa
/// riga del percorso testuale; invarianti interne (`Internal`).
pub(in crate::aggregation) fn build_binary_groups(
    batch: &RecordBatch,
    group_indices: &[usize],
) -> Result<Groups> {
    let mut accumulo = GroupAccumulator::new(batch.num_rows());
    let mut representatives: Vec<usize> = Vec::new();
    visit_key_ids(batch, group_indices, |row, gruppo, nuova| {
        if nuova {
            // Gli indici di chiave e i gruppi provvisori nascono insieme, uno
            // per chiave nuova: devono coincidere.
            if accumulo.nuovo_gruppo() != gruppo {
                return Err(PlenoraError::Internal(
                    "gruppo provvisorio diverso dall'indice di chiave".into(),
                ));
            }
            representatives.push(row);
        }
        accumulo.aggiungi(row, gruppo)
    })?;
    let order_columns = group_indices
        .iter()
        .map(|index| OrderColumn::new(batch.column(*index), &representatives))
        .collect::<Result<Vec<_>>>()?;
    let compare = |a: &usize, b: &usize| {
        let (row_a, row_b) = (representatives[*a], representatives[*b]);
        order_columns
            .iter()
            .map(|column| column.compare(*a, row_a, *b, row_b))
            .find(|ordering| ordering.is_ne())
            .unwrap_or(Ordering::Equal)
    };
    // Chiavi distinte per costruzione: l'ordine e' totale e deterministico.
    let mut order = (0..representatives.len()).collect::<Vec<_>>();
    if order.len() >= PARALLEL_THRESHOLD {
        order.par_sort_by(compare);
    } else {
        order.sort_by(compare);
    }
    accumulo.finisci(&order)
}

/// Sorgente testuale per nunique/concat: valori Utf8 presi in prestito,
/// `scalar_as_string` (invariato) per gli altri tipi.
pub(in crate::aggregation) enum TextSource<'a> {
    Utf8(&'a StringArray),
    Generic(&'a ArrayRef),
}

impl<'a> TextSource<'a> {
    pub(in crate::aggregation) fn new(array: &'a ArrayRef) -> Self {
        if let Some(values) = array.as_any().downcast_ref::<StringArray>() {
            return Self::Utf8(values);
        }
        Self::Generic(array)
    }

    pub(in crate::aggregation) fn value(&self, row: usize) -> Result<Option<Cow<'a, str>>> {
        match self {
            Self::Utf8(values) => Ok(if values.is_null(row) {
                None
            } else {
                Some(Cow::Borrowed(values.value(row)))
            }),
            Self::Generic(array) => Ok(scalar_as_string(array.as_ref(), row)?.map(Cow::Owned)),
        }
    }
}

/// Applica `f` ai gruppi in ordine canonico; sopra soglia in parallelo con
/// rayon (raccolta posizionale: output identico al sequenziale).
pub(in crate::aggregation) fn map_groups<T>(
    groups: &Groups,
    parallel: bool,
    f: impl Fn(&[usize]) -> Result<T> + Sync,
) -> Result<Vec<T>>
where
    T: Send,
{
    if parallel {
        (0..groups.len())
            .into_par_iter()
            .map(|gruppo| f(groups.get(gruppo)))
            .collect()
    } else {
        groups.iter().map(f).collect()
    }
}

/// Partizioni di `build_partitions`: chiave testuale della colonna di
/// partizione e indici di riga della partizione.
type KeyPartitions<'a> = Vec<(Option<Cow<'a, str>>, Vec<usize>)>;

/// Partizioni di `window_function`/`rolling_window`.
///
/// Righe raggruppate per la chiave testuale della colonna di partizione
/// (`TextSource`) con `KeyHasher`. Le partizioni escono nell'ordine di un
/// `BTreeMap` (chiave `Option<String>` crescente): gli errori per partizione
/// emergono in ordine deterministico.
pub(in crate::aggregation) fn build_partitions(
    batch: &RecordBatch,
    group: Option<usize>,
) -> Result<KeyPartitions<'_>> {
    let source = group.map(|index| TextSource::new(batch.column(index)));
    let mut lookup: HashMap<Option<Cow<'_, str>>, usize, FastHasher> = HashMap::default();
    let mut partitions: Vec<(Option<Cow<'_, str>>, Vec<usize>)> = Vec::new();
    for row in 0..batch.num_rows() {
        let key = source
            .as_ref()
            .map(|source| source.value(row))
            .transpose()?
            .flatten();
        if let Some(index) = lookup.get(&key) {
            partitions[*index].1.push(row);
        } else {
            lookup.insert(key.clone(), partitions.len());
            partitions.push((key, vec![row]));
        }
    }
    // Le chiavi sono univoche: l'ordinamento e' esatto e deterministico.
    partitions.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(partitions)
}

/// Calcola i valori di output per partizione e li scrive alle posizioni di
/// riga originali.
///
/// Sopra soglia il calcolo va in parallelo con raccolta posizionale (stessi
/// valori del sequenziale); la scrittura riproduce il riempimento diretto
/// originale.
pub(in crate::aggregation) fn scatter_partitions(
    batch: &RecordBatch,
    partitions: &[(Option<Cow<'_, str>>, Vec<usize>)],
    output: &mut [Option<f64>],
    compute: impl Fn(&[usize]) -> Result<Vec<Option<f64>>> + Sync,
) -> Result<()> {
    let parallel = batch.num_rows() >= PARALLEL_THRESHOLD && partitions.len() > 1;
    let partials = if parallel {
        partitions
            .par_iter()
            .map(|(_, rows)| compute(rows))
            .collect::<Result<Vec<_>>>()?
    } else {
        partitions
            .iter()
            .map(|(_, rows)| compute(rows))
            .collect::<Result<Vec<_>>>()?
    };
    for ((_, rows), values) in partitions.iter().zip(&partials) {
        for (row, value) in rows.iter().zip(values) {
            output[*row] = *value;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn accumulo(assegnazione: &[usize], gruppi: usize) -> GroupAccumulator {
        let mut accumulo = GroupAccumulator::new(assegnazione.len());
        for _ in 0..gruppi {
            accumulo.nuovo_gruppo();
        }
        for (row, gruppo) in assegnazione.iter().enumerate() {
            accumulo.aggiungi(row, *gruppo).expect("aggiungi");
        }
        accumulo
    }

    #[test]
    fn l_accumulatore_rifiuta_invarianti_rotte_in_entrambe_le_forme() {
        let assegnazione = [0, 1, 0, 2];
        // Forma a liste (3 gruppi, sotto soglia) e forma compatta (piu'
        // gruppi aperti di `soglia_compatta`).
        for gruppi in [3, soglia_compatta(assegnazione.len()) + 1] {
            let permutazione = (0..gruppi).rev().collect::<Vec<_>>();
            let ordinati = accumulo(&assegnazione, gruppi)
                .finisci(&permutazione)
                .expect("ordine valido");
            assert_eq!(ordinati.len(), gruppi);
            let mut righe = ordinati.iter().flatten().copied().collect::<Vec<_>>();
            righe.sort_unstable();
            assert_eq!(righe, vec![0, 1, 2, 3], "ogni riga una volta sola");

            let mut ripetuto = permutazione.clone();
            ripetuto[0] = ripetuto[1];
            assert!(accumulo(&assegnazione, gruppi).finisci(&ripetuto).is_err());
            assert!(accumulo(&assegnazione, gruppi)
                .finisci(&permutazione[1..])
                .is_err());
            let mut fuori = permutazione.clone();
            fuori[0] = gruppi;
            assert!(accumulo(&assegnazione, gruppi).finisci(&fuori).is_err());
        }
        // Righe non contigue o gruppo inesistente: errore esplicito.
        let mut accumulo = GroupAccumulator::new(4);
        accumulo.nuovo_gruppo();
        assert!(accumulo.aggiungi(1, 0).is_err());
        assert!(accumulo.aggiungi(0, 5).is_err());
    }
}
