### Che cosa fa

Join spaziale interno: abbina ogni riga della sinistra alle righe della
destra la cui geometria soddisfa `predicate` con la sua, e dà una riga per
coppia, con la posizione della riga destra in `__right_index` (kernel
`spatial_join::spatial_join_nullable_validated`, [README, «Operazioni
geo»](../README.md#operazioni-geo)).

### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `predicate` | stringa | obbligatorio | `intersects`, `contains`, `within`, `crosses`, `overlaps`, `touches` | la relazione fra la geometria sinistra `L` e la destra `R` |

I predicati sono quelli di `geo`, esatti; tutti tranne `intersects`
passano dalla matrice DE-9IM:

- `intersects`: almeno un punto in comune, bordo compreso;
- `contains`: `L` contiene `R` (nessun punto di `R` fuori da `L`, almeno
  un punto dell'interno di `R` nell'interno di `L`: `R` sul solo bordo di
  `L` non conta);
- `within`: `R` contiene `L`;
- `crosses`, `overlaps`, `touches`: come nella DE-9IM (attraversamento,
  sovrapposizione parziale, contatto solo sul bordo).

### Schema

Le colonne della sinistra, invariate, più `__right_index` in coda, `uint64`
non nullable. Le colonne della destra non passano: si ricollegano con
`__right_index`; non c'è `__left_index`. La colonna geometria resta quella
della sinistra, non nullable: una geometria null non ha coppie. I metadati di schema sono la fusione dei due lati; le
proprietà del contratto (`sorted_by`, `row_count`) si perdono.

### Righe

Espansione 1:N: una riga per coppia che soddisfa il predicato, con le
colonne della riga sinistra della coppia, quindi una riga sinistra si
ripete per ogni destra abbinata e non compare se non ne ha nessuna. Le
geometrie nulle o vuote, da un lato o dall'altro, non si abbinano mai;
`__right_index` è la posizione della riga destra contando anche le nulle.
Le coppie sono al più il limite di righe dell'arco d'uscita
(`max_output_rows` se il passo è un output del piano, `max_rows_per_edge`
altrimenti).

### Ordine

Per riga sinistra, poi per `__right_index` crescente (ordine
lessicografico delle coppie).

### Errori

In validazione (analisi del contratto):

- `InvalidPlan`: un numero di ingressi diverso da due; config con campi
  sconosciuti, `predicate` assente o fuori elenco; un metadato di schema
  presente sui due lati con valori diversi;
- `Schema`: `__right_index` esiste già nella sinistra; un lato senza
  esattamente una colonna geometria, o con una colonna non riconoscibile
  come geometria WKB (né estensione `geoarrow.wkb` né chiavi canoniche
  `plenora.geometry.*`);
- `Unsupported`: una colonna geometria non XY;
- `Crs`: un lato senza CRS risolto, un CRS non proiettato (o senza unità
  lineare), CRS dei due lati non equivalenti.

In esecuzione, prima del kernel, su ogni cella non nulla dei due lati
([README, «Operazioni geo»](../README.md#operazioni-geo)):

- `Schema`: il contratto di un lato dichiara i tipi geometrici con un
  elenco e la cella è di un altro tipo;
- `Crs`: una coordinata fuori dal dominio di validità del CRS della
  colonna;
- `InvalidPlan`: la cella viola il contratto WKB o non supera la
  validazione OGC (`Unsupported` per dimensioni Z o M, `Internal` se la
  validazione non conclude, `ResourceLimit` per una cella oltre il limite
  di byte).

Dal kernel (`spatial_join::spatial_join_nullable_validated`, errore
`SpatialJoinError`, sulle geometrie già validate), nella categoria del
passo geo indicata fra parentesi:

- `PairLimitExceeded` (`ResourceLimit`): le coppie confermate superano il
  limite di righe dell'arco (si controlla coppia per coppia, prima di
  materializzarle; ogni altro errore, il primo in ordine di riga, ha la
  precedenza);
- `MargineMemoria` (`ResourceLimit`): le coppie confermate, con la riga di
  left più larga (l'uscita ripete una riga di left per coppia), non
  starebbero nel margine di memoria del passo
  (budget meno byte vivi); stesso conteggio e stessa precedenza del limite
  di righe, che vince se scatta prima ([README, «Limiti dichiarati del runner»](../README.md#limiti-dichiarati-del-runner), voce «Modelli di costo geo»);
- `ValidazioneNonConclusa`, `CalcoloNonConcluso`, `Internal` (`Internal`):
  l'indice o il predicato di `geo` non ha concluso, o un'invariante
  interna violata;
- `IndexOverflow` (`InvalidPlan`): numero di righe oltre `u64`.

Il primo errore è quello della prima riga, in ordine di riga, senza
diagnostica per riga ([README, «Limiti dichiarati del
runner»](../README.md#limiti-dichiarati-del-runner), voce «Geo senza
diagnostica per riga»).

### Limiti e deviazioni

Solo join interno: il kernel non emette le righe sinistre senza
abbinamento, e non c'è una variante che le tenga con valori nulli. Gli
attributi della destra non entrano nell'uscita, a differenza di `sjoin` di
GeoPandas: si ricollegano con `__right_index`. Le coppie dipendono dai
dati: il modello di costo non le prevede, e le limita solo il limite di
righe dell'arco ([README, «Limiti dichiarati del
runner»](../README.md#limiti-dichiarati-del-runner), voce «Modelli di costo
geo»).

### Precisione

Nessun calcolo di geometrie e nessuna griglia: i predicati di `geo` si
valutano sulle coordinate `f64` d'ingresso, senza tolleranza, e la regola
di 1 cm non sposta nulla. Due geometrie a meno di 1 cm si toccano o no
secondo le loro coordinate esatte ([README, «Precisione delle operazioni
geografiche: 1 cm a
terra»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra),
«Feature d'ingresso più vicine della precisione»).

### Complessità

Un R-tree dei rettangoli d'ingombro della destra, O(m log m) per `m`
righe; per ognuna delle `n` righe sinistre, in parallelo, una ricerca
nell'albero e il predicato esatto sui soli candidati. Di norma
O((n + m) log m) più il costo dei predicati e delle coppie `p`; nel caso
peggiore (rettangoli tutti sovrapposti) O(n · m) predicati. Più la
validazione OGC di ogni geometria. Memoria O(m + p).

### Esempio

Il primo punto sta sull'angolo comune dei due quadrati e il terzo sul lato
comune: ognuno interseca entrambi e dà due righe; il secondo non ne
interseca nessuno e non esce.

```json
{
  "config": {"predicate": "intersects"},
  "ingressi": [
    {"nome": "pozzi", "colonne": [
      {"nome": "id", "tipo": "int64", "valori": [1, 2, 3]},
      {"nome": "geometry", "tipo": "geometry", "crs": "EPSG:3857", "valori": ["POINT(2 2)", "POINT(5 5)", "POINT(2 1)"]}
    ]},
    {"nome": "aree", "colonne": [
      {"nome": "nome", "tipo": "utf8", "valori": ["A", "B"]},
      {"nome": "geometry", "tipo": "geometry", "crs": "EPSG:3857", "valori": ["POLYGON((0 0,2 0,2 2,0 2,0 0))", "POLYGON((2 0,4 0,4 2,2 2,2 0))"]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "id", "tipo": "int64", "valori": [1, 1, 3, 3]},
    {"nome": "geometry", "tipo": "geometry", "crs": "EPSG:3857", "valori": ["POINT(2 2)", "POINT(2 2)", "POINT(2 1)", "POINT(2 1)"]},
    {"nome": "__right_index", "tipo": "uint64", "valori": [0, 1, 0, 1]}
  ]}
}
```
