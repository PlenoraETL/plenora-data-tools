### Che cosa fa

Sostituisce la geometria della sinistra con la parte che non sta nella
geometria della destra, `sinistra \ destra`, calcolata dal kernel
`topology::boolean_operation_validated` ([README, «Operazioni
geo»](../README.md#operazioni-geo)). Lavora solo su `Polygon` e
`MultiPolygon` e rende sempre un `MultiPolygon`.

### Parametri

Nessuno: la config è `{}`.

### Schema

Quello della sinistra: stesse colonne, nello stesso ordine, con gli stessi
tipi; le colonne della destra non passano. La colonna geometria resta al
suo posto, con lo stesso nome e lo stesso CRS della sinistra (uguale a
quello della destra), in XY, ed è nullable anche quando quella della
sinistra non lo è (un risultato vuoto è nullo); i tipi geometrici
dichiarati diventano `MultiPolygon` (`exact`) e le chiavi dei tipi
ereditate dal campo si tolgono. Gli altri metadati di campo restano. I
metadati di schema sono la fusione dei due lati: una chiave presente da un
solo lato o uguale sui due passa. Le proprietà del contratto della sinistra
(`sorted_by`, `row_count`) restano.

### Righe

Allineate: la riga `i` della sinistra con la riga `i` della destra, e le
due tabelle devono avere le stesse righe (altrimenti `InvalidPlan`, in
esecuzione: le righe non si conoscono a secco). Una riga d'uscita per riga
della sinistra, con la geometria nulla dove una delle due è nulla o il
risultato è vuoto. Dove la destra copre tutta la sinistra il kernel rende
un `MultiPolygon` vuoto e la riga resta con la geometria nulla.

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
  di byte);
- `InvalidPlan`: le due tabelle hanno un numero di righe diverso.

Dal kernel (`topology::boolean_operation_validated`, errore
`TopologyError`, sulle geometrie già validate: resta la validazione OGC
del risultato), nella categoria del passo geo indicata fra parentesi:

- `UnsupportedGeometry` (`InvalidPlan`): una geometria non è
  `Polygon`/`MultiPolygon`;
- `InvalidGeometry` (`InvalidPlan`): il risultato non supera la
  validazione OGC;
- `PrecisionInsufficient` (`Unsupported`): la griglia dell'overlay
  sposterebbe il risultato oltre la precisione (sotto, «Precisione»);
- `ValidazioneNonConclusa`, `CalcoloNonConcluso` (`Internal`): la
  validazione OGC o l'overlay di `geo` non ha concluso.

Il primo errore è quello della prima riga, in ordine di riga, senza
diagnostica per riga ([README, «Limiti dichiarati del
runner»](../README.md#limiti-dichiarati-del-runner), voce «Geo senza
diagnostica per riga»).

### Limiti e deviazioni

Solo poligoni: una destra lineare o puntuale si rifiuta
(`UnsupportedGeometry`), dove GEOS e PostGIS renderebbero la sinistra
invariata. Il catalogo dichiara forma 1:N e vincolo `max(uscita /
sinistra, uscita / destra)`, ma il passo rende una riga per riga della
sinistra. Nessun controllo a posteriori del risultato contro gli ingressi
([README, «Precisione delle operazioni geografiche: 1 cm a
terra»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)).

### Precisione

La precisione è 1 cm a terra nelle unità del CRS della sinistra
(`Precision::from_crs`, calcolata in validazione). Entro 1 cm a terra: un
solo overlay di `i_overlay` su interi `i64`, con la griglia controllata
prima del calcolo sull'ingombro dei due operandi. Se lo spostamento a
priori supera mezzo centimetro, o le coordinate sono troppo rade per il
centimetro, `PrecisionInsufficient` e nessun calcolo. Parti più sottili di
1 cm possono sparire o fondersi senza errore; vedi [README, «Precisione
delle operazioni geografiche: 1 cm a
terra»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra).

### Complessità

Per coppia l'overlay a scansione di `i_overlay`, di norma O((v + k) log v)
con `v` i vertici dei due operandi e `k` gli incroci fra i lati, più la
validazione OGC di ingressi e risultato (di norma O(v log v), nel caso
peggiore O(v²): [README, «Validazione OGC: la
ricerca delle auto-intersezioni non è quella di `geo`, il verdetto
sì»](../README.md#validazione-ogc-la-ricerca-delle-auto-intersezioni-non-è-quella-di-geo-il-verdetto-sì)).
Memoria O(v + k).

### Esempio

Una riga per lato: la riga `i` della sinistra con la riga `i` della destra.

```json
{
  "config": {},
  "ingressi": [
    {"nome": "lotti", "colonne": [
      {"nome": "id", "tipo": "int64", "valori": [1]},
      {"nome": "geometry", "tipo": "geometry", "crs": "EPSG:3857", "valori": ["POLYGON((0 0,2 0,2 2,0 2,0 0))"]}
    ]},
    {"nome": "vincoli", "colonne": [
      {"nome": "zona", "tipo": "utf8", "valori": ["A"]},
      {"nome": "geometry", "tipo": "geometry", "crs": "EPSG:3857", "valori": ["POLYGON((1 0,3 0,3 2,1 2,1 0))"]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "id", "tipo": "int64", "valori": [1]},
    {"nome": "geometry", "tipo": "geometry", "crs": "EPSG:3857", "valori": ["MULTIPOLYGON(((0 2,0 0,1 0,1 2,0 2)))"]}
  ]}
}
```
