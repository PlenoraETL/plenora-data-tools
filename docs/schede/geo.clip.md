### Che cosa fa

Ritaglia ogni geometria della sinistra sulla maschera data dalla destra:
tutte le geometrie della destra si uniscono in una sola maschera, e ogni
riga della sinistra diventa la sua intersezione con la maschera (kernel
`topology::clip_to_mask_validated`; [README, «Operazioni
geo»](../README.md#operazioni-geo)). Lavora solo su `Polygon` e
`MultiPolygon`.

### Parametri

Nessuno: la config è `{}`.

### Schema

Quello della sinistra: stesse colonne, nello stesso ordine, con gli stessi
tipi; le colonne della destra non passano. La colonna geometria resta al
suo posto, con lo stesso nome e lo stesso CRS della sinistra (uguale a
quello della destra), in XY, ed è nullable anche quando quella della
sinistra non lo è (un ritaglio vuoto è nullo); i tipi geometrici
dichiarati diventano `MultiPolygon` (`exact`) e le chiavi dei tipi
ereditate dal campo si tolgono. Gli altri metadati di campo restano. I
metadati di schema sono la fusione dei due lati: una chiave presente da un
solo lato o uguale sui due passa. Le proprietà del contratto della sinistra
(`sorted_by`, `row_count`) restano.

### Righe

1:1 con la sinistra: la destra conta solo come maschera, qualunque sia il
suo numero di righe, e le sue geometrie nulle non ne fanno parte. Una riga
il cui ritaglio è vuoto (fuori dalla maschera, o con una destra senza
geometrie) resta, con la geometria nulla; una riga con la geometria
sinistra nulla resta nulla e non entra nel kernel.

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

Dal kernel (`topology::clip_to_mask_validated`, errore `TopologyError`,
sulle geometrie già validate: restano le validazioni OGC della maschera
unita e dei ritagli), nella categoria del passo geo indicata fra
parentesi:

- `UnsupportedGeometry` (`InvalidPlan`): una geometria, di un lato o
  dell'altro, non è `Polygon`/`MultiPolygon`;
- `InvalidGeometry` (`InvalidPlan`): la maschera unita o un ritaglio non
  supera la validazione OGC;
- `PrecisionInsufficient` (`Unsupported`): la griglia di uno dei due
  overlay sposterebbe il risultato oltre la precisione (sotto,
  «Precisione»);
- `ValidazioneNonConclusa`, `CalcoloNonConcluso` (`Internal`): la
  validazione OGC o un overlay di `geo` non ha concluso.

Il runner verifica che il kernel renda un risultato per ogni riga non
nulla, altrimenti `Internal`.

Il primo errore è quello della prima riga, in ordine di riga, senza
diagnostica per riga ([README, «Limiti dichiarati del
runner»](../README.md#limiti-dichiarati-del-runner), voce «Geo senza
diagnostica per riga»).

### Limiti e deviazioni

Solo poligoni: un ritaglio che si riduce a linee o punti è vuoto, quindi
la riga resta con la geometria nulla. Nessun controllo a posteriori del risultato
contro gli ingressi ([README, «Precisione delle operazioni geografiche: 1
cm a
terra»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)).

### Precisione

La precisione è 1 cm a terra nelle unità del CRS della sinistra
(`Precision::from_crs`, calcolata in validazione). Entro 1 cm a terra, con
due overlay in catena: l'unione della maschera e poi l'intersezione di
ogni riga, ognuno con la griglia controllata prima del calcolo entro un
quarto di centimetro (la catena entro mezzo). Se lo spostamento a priori
supera quella quota, o le coordinate sono troppo rade per il centimetro,
`PrecisionInsufficient` e nessun calcolo. Parti più sottili di 1 cm
possono sparire o fondersi senza errore; vedi [README, «Precisione delle
operazioni geografiche: 1 cm a
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
