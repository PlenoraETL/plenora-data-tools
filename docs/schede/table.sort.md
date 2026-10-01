### Che cosa fa

Riordina le righe secondo le colonne `columns`: decide la prima, a parità
la seconda, e così via. Le colonne e i valori non cambiano, cambia solo
l'ordine delle righe. Il confronto è sul valore nativo di ogni tipo, mai
sulla sua forma in testo.

### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `columns` | lista di stringhe | obbligatorio | nomi di colonne dell'ingresso, almeno uno, senza ripetizioni, di tipo ordinabile | chiavi d'ordinamento, dalla più significativa |
| `ascending` | booleano | `true` | `true`, `false` | verso, lo stesso per tutte le chiavi |

Tipi ordinabili e come si confrontano:

- `int64`, `uint64`: interi esatti, anche oltre `2^53`;
- `float64`: ordine totale IEEE (`total_cmp`): `-0.0` prima di `0.0`, un
  NaN positivo dopo `+inf`, uno negativo prima di `-inf`;
- `utf8`: byte per byte del testo UTF-8 (nessuna collazione linguistica);
- `bool`: `false` prima di `true`;
- `date32`: giorni dall'epoca; `date64`: millisecondi dall'epoca;
  `timestamp` di ogni unità, con o senza timezone: il valore dall'epoca
  nella sua unità, cioè l'istante (la timezone non conta; due unità diverse
  si confrontano esatte, in nanosecondi);
- `decimal128`: per valore;
- `binary`: byte per byte;
- `dictionary<utf8>` (chiavi `int32`): sul testo decodificato.

Ogni altro tipo (fra gli altri `int32`, `float32`, `large_utf8`, le liste)
non è ordinabile.

### Schema

Identico all'ingresso: colonne, tipi, nullabilità e metadati di campo e di
schema. Il contratto dichiara l'uscita ordinata sulle colonne di `columns`
(`sorted_by`, con i null in coda in ascendente e in testa in discendente) e
conserva il conteggio delle righe.

### Righe

1:1: le stesse righe, permutate.

### Ordine

Una cella nulla (anche la voce nulla di un dizionario) viene dopo ogni
valore; `ascending: false` rovescia l'intero confronto, quindi in
discendente i null vengono per primi. Il sort è stabile: righe con le
stesse chiavi restano nell'ordine d'ingresso, in entrambi i versi.

### Errori

In validazione, `InvalidPlan`:

- `columns` vuoto, con un nome ripetuto o non valido, o oltre il limite di
  colonne;
- una colonna di `columns` assente o di un tipo non ordinabile;
- config con campi sconosciuti.

In esecuzione:

- `Schema`: una chiave di dizionario fuori dal proprio dizionario;
- `ResourceLimit`: più di `u32::MAX` righe; nella variante spilled, file
  temporanei oltre `max_temp_bytes`;
- `Io`: nella variante spilled, un errore sui file temporanei.

### Limiti e deviazioni

Nessuno oltre ai limiti comuni.

### Complessità

Tempo O(n log n) confronti su n righe, ciascuno fino al numero di chiavi;
memoria O(n) indici più la copia dell'uscita. Da 32.768 righe il sort è un
merge sort parallelo, con la stessa permutazione del sequenziale.

La variante spilled la sceglie il runner quando quella in memoria non sta
nel budget ([README, «Budget di memoria»](../README.md#budget-di-memoria)):
l'ingresso si divide in run dimensionate su `max_governed_memory_bytes`,
ognuna ordinata in memoria con lo stesso comparatore e scritta su file
Arrow IPC temporanei con l'indice originale di ogni riga; una fusione in
streaming delle run (una scansione lineare delle run per ogni riga emessa)
produce la permutazione, e a parità di chiavi vince l'indice originale
minore. L'uscita è identica a quella in memoria, e resta intera in memoria.

### Esempio

```json
{
  "config": {"columns": ["importo"], "ascending": false},
  "ingressi": [
    {"nome": "ordini", "colonne": [
      {"nome": "id", "tipo": "int64", "valori": [1, 2, 3, 4]},
      {"nome": "importo", "tipo": "float64", "valori": [5.5, null, 12.0, 5.5]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "id", "tipo": "int64", "valori": [2, 3, 1, 4]},
    {"nome": "importo", "tipo": "float64", "valori": [null, 12.0, 5.5, 5.5]}
  ]}
}
```
