### Che cosa fa

Aggiunge quattro colonne `float64` con il rettangolo d'ingombro della
geometria di ogni riga: coordinate minime e massime in x e in y, nelle
unità del CRS. Per una geometria vuota il kernel non dà un rettangolo.

### Parametri

Nessuno: la config è `{}`.

### Schema

Aggiunge in coda, in quest'ordine, `<geometria>_minx`, `<geometria>_miny`,
`<geometria>_maxx`, `<geometria>_maxy` (`<geometria>` è il nome della
colonna geometria), `float64` nullable, senza metadati di campo. Le altre
colonne (geometria compresa), i metadati di schema e le proprietà del
contratto (`sorted_by`, `row_count`) passano invariati.

### Righe

1:1: un rettangolo per riga, dal kernel `operations::bounds`. Una
geometria nulla dà quattro celle nulle, e anche una geometria vuota (il
kernel non ha un rettangolo da rendere).

### Ordine

L'ordine d'ingresso: l'analisi conserva `sorted_by`.

### Errori

In validazione (analisi del contratto):

- `Schema`: l'ingresso non ha esattamente una colonna geometria, o la
  colonna non si riconosce come WKB (né estensione `geoarrow.wkb` né chiavi
  `plenora.geometry.*`); una delle quattro colonne esiste già;
- `Unsupported`: dimensioni della geometria diverse da `xy`;
- `InvalidPlan`: config diversa da `{}`;
- `Crs`: CRS della colonna assente o non risolto, geografico, o proiettato
  senza unità lineare.

In esecuzione ([README, «Operazioni geo»](../README.md#operazioni-geo))
il passo rende il primo errore in ordine di riga, senza diagnostica per
riga. Prima del kernel, per ogni cella non nulla della colonna geometria:

- `InvalidPlan`: struttura WKB non valida; `Unsupported`: la cella porta
  Z/M o uno SRID; `ResourceLimit`: la cella supera 64 MiB;
- `Crs`: una coordinata fuori dal dominio di validità del CRS della
  colonna;
- `Schema`: il contratto d'ingresso dichiara i tipi geometrici con un
  elenco e la cella è di un altro tipo.

Dal kernel (`OperationError`), per geometria:

- `InvalidPlan` (`InvalidInput`): la geometria non supera la validazione
  OGC;
- `Internal` (`ValidazioneNonConclusa`): la validazione OGC va in panico
  dentro la barriera (il messaggio porta solo la forma del payload).

### Limiti e deviazioni

Il catalogo chiede un CRS proiettato, anche se il rettangolo non dipende
dalla metrica. Errori senza indice di riga della sorgente
([README, «Limiti dichiarati del runner»](../README.md#limiti-dichiarati-del-runner),
voce «Geo senza diagnostica per riga»).

### Precisione

Esatta: i quattro valori sono coordinate d'ingresso, copiate senza calcolo
([README, «Precisione delle operazioni geografiche»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)).

### Complessità

Per geometria di n vertici: tempo O(n) per l'ingombro, più la validazione
OGC dell'ingresso (sub-quadratica nel caso tipico, O(n²) nel peggiore:
[README, «Validazione OGC»](../README.md#validazione-ogc-la-ricerca-delle-auto-intersezioni-non-è-quella-di-geo-il-verdetto-sì));
memoria O(n) per la validazione.

### Esempio

```json
{
  "config": {},
  "ingressi": [
    {"nome": "lotti", "colonne": [
      {"nome": "id", "tipo": "int64", "valori": [1, 2]},
      {"nome": "geometry", "tipo": "geometry", "crs": "EPSG:3857", "valori": ["POLYGON((0 0,4 1,2 3,0 0))", "POINT(5 6)"]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "id", "tipo": "int64", "valori": [1, 2]},
    {"nome": "geometry", "tipo": "geometry", "crs": "EPSG:3857", "valori": ["POLYGON((0 0,4 1,2 3,0 0))", "POINT(5 6)"]},
    {"nome": "geometry_minx", "tipo": "float64", "valori": [0.0, 5.0]},
    {"nome": "geometry_miny", "tipo": "float64", "valori": [0.0, 6.0]},
    {"nome": "geometry_maxx", "tipo": "float64", "valori": [4.0, 5.0]},
    {"nome": "geometry_maxy", "tipo": "float64", "valori": [3.0, 6.0]}
  ]}
}
```
