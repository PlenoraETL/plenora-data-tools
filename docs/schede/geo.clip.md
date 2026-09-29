### Che cosa fa

Ritaglia ogni geometria della sinistra sulla maschera data dalla destra:
tutte le geometrie della destra si uniscono in una sola maschera, e ogni
riga della sinistra diventa la sua intersezione con la maschera (kernel
`topology::clip_to_mask`). Lavora solo su `Polygon` e `MultiPolygon`. Il
runner non esegue ancora le operazioni geo ([README, «Che cosa non c'è
ancora»](../README.md#che-cosa-non-cè-ancora)): lo schema qui descritto è
quello dell'analisi del contratto, i valori quelli del kernel.

### Parametri

Nessuno: la config è `{}`.

### Schema

Quello della sinistra: stesse colonne, nello stesso ordine, con gli stessi
tipi e la stessa nullabilità; le colonne della destra non passano. La
colonna geometria resta al suo posto, con lo stesso nome e lo stesso CRS
della sinistra (uguale a quello della destra), in XY; i tipi geometrici
dichiarati diventano `MultiPolygon` (`exact`) e le chiavi dei tipi
ereditate dal campo si tolgono. Gli altri metadati di campo restano. I
metadati di schema sono la fusione dei due lati: una chiave presente da un
solo lato o uguale sui due passa. Le proprietà del contratto della sinistra
(`sorted_by`, `row_count`) restano.

### Righe

1:1 con la sinistra: la destra conta solo come maschera, qualunque sia il
suo numero di righe. Una riga il cui ritaglio è vuoto (fuori dalla
maschera, o con una destra senza righe) resta, senza geometria: il kernel
rende `None` in quella posizione. Le celle nulle non entrano nel kernel,
che riceve solo geometrie: come si trattano a livello di tabella non è
ancora fissato.

### Ordine

Quello della sinistra.

### Errori

In validazione (analisi del contratto):

- `InvalidPlan`: un numero di ingressi diverso da due; config non vuota;
  un metadato di schema presente sui due lati con valori diversi;
- `Schema`: un lato senza esattamente una colonna geometria, o con una
  colonna non riconoscibile come geometria WKB (né estensione
  `geoarrow.wkb` né chiavi canoniche `plenora.geometry.*`);
- `Unsupported`: una colonna geometria non XY;
- `Crs`: un lato senza CRS risolto, un CRS non proiettato (o senza unità
  lineare), CRS dei due lati non equivalenti.

In esecuzione (kernel `topology::clip_to_mask`, errore `TopologyError`;
nessun codice di questo repository lo traduce ancora in `PlenoraError`):

- `UnsupportedGeometry`: una geometria, di un lato o dell'altro, non è
  `Polygon`/`MultiPolygon`;
- `InvalidGeometry`: una geometria d'ingresso, la maschera unita o un
  ritaglio non supera la validazione OGC;
- `ValidazioneNonConclusa`: la validazione OGC non ha concluso;
- `PrecisionInsufficient`: la griglia di uno dei due overlay sposterebbe il
  risultato oltre la precisione (sotto, «Precisione»);
- `CalcoloNonConcluso`: un overlay di `geo` è andato in panico.

### Limiti e deviazioni

Solo poligoni: un ritaglio che si riduce a linee o punti è vuoto, quindi
la riga resta senza geometria. La nullabilità dichiarata della colonna
geometria è quella della sinistra anche se il kernel rende righe senza
geometria: con una colonna sinistra non nullable contratto e kernel non
concordano (da fissare quando il runner eseguirà l'operazione). Il catalogo
dichiara forma 1:N e vincolo `max(uscita / sinistra, uscita / destra)`, ma
il kernel rende una geometria per riga della sinistra. Nessun controllo a
posteriori del risultato contro gli ingressi ([README, «Precisione delle
operazioni geografiche: 1 cm a
terra»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)).

### Precisione

Entro 1 cm a terra, con due overlay in catena: l'unione della maschera e
poi l'intersezione di ogni riga, ognuno con la griglia controllata prima
del calcolo entro un quarto di centimetro (la catena entro mezzo). Se lo
spostamento a priori supera quella quota, o le coordinate sono troppo rade
per il centimetro, `PrecisionInsufficient` e nessun calcolo. Parti più
sottili di 1 cm possono sparire o fondersi senza errore; vedi [README,
«Precisione delle operazioni geografiche: 1 cm a
terra»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra),
«overlay in catena».

### Complessità

Un overlay per l'unione della maschera, sui `m` vertici della destra, poi
per ognuna delle `n` righe un overlay con la maschera intera: di norma
O(n · (v + m) log(v + m)) con `v` i vertici della riga, perché ogni riga
si confronta con tutta la maschera. Più la validazione OGC di ingressi,
maschera e ritagli. Memoria O(m) per la maschera più i ritagli.

### Esempio

```json
{
  "config": {},
  "ingressi": [
    {"nome": "lotti", "colonne": [
      {"nome": "id", "tipo": "int64", "valori": [1, 2]},
      {"nome": "geometry", "tipo": "geometry", "crs": "EPSG:3857", "valori": ["POLYGON((0 0,2 0,2 2,0 2,0 0))", "POLYGON((10 10,11 10,11 11,10 11,10 10))"]}
    ]},
    {"nome": "comune", "colonne": [
      {"nome": "parte", "tipo": "utf8", "valori": ["sud", "nord"]},
      {"nome": "geometry", "tipo": "geometry", "crs": "EPSG:3857", "valori": ["POLYGON((1 0,3 0,3 1,1 1,1 0))", "POLYGON((1 1,3 1,3 2,1 2,1 1))"]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "id", "tipo": "int64", "valori": [1, 2]},
    {"nome": "geometry", "tipo": "geometry", "crs": "EPSG:3857", "valori": ["MULTIPOLYGON(((1 2,1 0,2 0,2 2,1 2)))", null]}
  ]}
}
```
