### Che cosa fa

Costruisce un `Polygon` senza buchi dai punti della colonna geometria,
nell'ordine delle righe: i punti sono l'anello esterno, chiuso da sé se
l'ultimo non ripete il primo. La config è vuota: non ci sono colonne di
gruppo né d'ordine, quindi per contratto tutta la tabella diventa una sola
geometria, e le colonne attributo si perdono.

### Parametri

Nessuno: la config è `{}`.

### Schema

L'uscita ha la sola colonna geometria: stesso nome, CRS, dimensioni e
metadati di campo, nullable. Le altre colonne non passano; i metadati di
schema sì. Le proprietà del contratto (`sorted_by`, `row_count`) cadono, e
la dichiarazione dei tipi ereditata si toglie senza sostituirla (nessuna
dichiarazione).

### Righe

Aggregazione: tutte le righe in una. Il kernel
(`construction::polygon_from_ordered_points`) riceve il gruppo ordinato
delle geometrie, salta quelle assenti (`None`) e, con meno di tre punti,
non costruisce il poligono (`None`, non un errore). Il runner gli passa
tutte le celle della colonna nell'ordine delle righe, una cella nulla come
`None`, e rende sempre una riga: il poligono, o una geometria nulla quando
il poligono manca (anche per una tabella vuota).

### Ordine

I vertici seguono l'ordine delle righe d'ingresso: un ordine diverso dà un
poligono diverso, o un anello che si incrocia e quindi un errore.

### Errori

In validazione (analisi del contratto):

- `Schema`: l'ingresso non ha esattamente una colonna geometria, o la
  colonna non si riconosce come WKB (né estensione `geoarrow.wkb` né chiavi
  `plenora.geometry.*`);
- `Unsupported`: dimensioni della geometria diverse da `xy`;
- `InvalidPlan`: config diversa da `{}`;
- `Crs`: CRS della colonna assente o non risolto, geografico, o proiettato
  senza unità lineare.

In esecuzione, prima del kernel, su ogni cella non nulla ([README,
«Operazioni geo»](../README.md#operazioni-geo)): `InvalidPlan` per un WKB
malformato o con coordinate non finite, `Unsupported` per dimensioni Z/M o
SRID, `Crs` per una coordinata fuori dal dominio di validità del CRS della
colonna, `Schema` per una geometria di un tipo che il contratto
dell'ingresso dichiara con un elenco e che non vi compare. Poi la
decodifica completa con la validazione OGC: `InvalidPlan` per una
geometria non valida, `Internal` se la validazione non conclude.

Poi il kernel, sul gruppo, con errore `ConstructionError` che il runner
traduce così: `ValidazioneNonConclusa` diventa `Internal`, le altre
`InvalidPlan`:

- `ExpectedPoint`: una geometria non è un `Point` (il messaggio dà la
  posizione nel gruppo, assenti comprese, e il tipo trovato);
- `InvalidOutput`: il poligono non supera la validazione OGC (anello che si
  incrocia, punti collineari o troppo pochi punti distinti);
- `ValidazioneNonConclusa`: la validazione OGC va in panico dentro la
  barriera (il messaggio porta solo la forma del payload).

### Limiti e deviazioni

Non ci sono colonne di gruppo né d'ordine (il kernel lavora su un gruppo
già ordinato, ma nessun parametro lo forma): un poligono per gruppo non si
può chiedere. Nessun buco,
nessun riordino dei punti, nessuna riparazione dell'anello. Nessuna diagnostica per riga: il passo rende il primo errore ([README,
«Limiti dichiarati del runner»](../README.md#limiti-dichiarati-del-runner),
voci «Geo senza diagnostica per riga» e «Modelli di costo geo»).

### Precisione

Esatta: i vertici sono le coordinate dei punti, copiate
([README, «Precisione delle operazioni geografiche»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)).

### Complessità

Tempo O(n) sulle righe più la validazione OGC del poligono (sub-quadratica
nel caso tipico, O(n²) nel peggiore:
[README, «Validazione OGC»](../README.md#validazione-ogc-la-ricerca-delle-auto-intersezioni-non-è-quella-di-geo-il-verdetto-sì));
memoria O(n): tutta la colonna diventa una geometria.

### Esempio

```json
{
  "config": {},
  "ingressi": [
    {"nome": "vertici", "colonne": [
      {"nome": "id", "tipo": "int64", "valori": [1, 2, 3]},
      {"nome": "geometry", "tipo": "geometry", "crs": "EPSG:3857", "valori": ["POINT(0 0)", "POINT(4 0)", "POINT(4 3)"]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "geometry", "tipo": "geometry", "crs": "EPSG:3857", "valori": ["POLYGON((0 0,4 0,4 3,0 0))"]}
  ]}
}
```
