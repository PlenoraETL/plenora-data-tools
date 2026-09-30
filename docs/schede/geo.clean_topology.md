### Che cosa fa

Pulisce la topologia di una tabella di poligoni già validi, riga per riga
nell'ordine della tabella (kernel `topology::clean_valid_polygon_topology`,
con la chiusura dei varchi e la regola «vince la prima riga» di Manipola):
con `fill_gaps` chiude rientranze e varchi stretti dentro ogni riga, con
`remove_overlaps` toglie a ogni riga la parte già coperta dalle righe
precedenti, così le righe non si sovrappongono più. Il runner lo esegue
su tutta la tabella insieme ([README, «Operazioni
geo»](../README.md#operazioni-geo)).

### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `snap_tolerance` | numero | obbligatorio | finito, `>= 0`, nelle unità del CRS | raggio della chiusura morfologica di `fill_gaps` |
| `remove_overlaps` | booleano | obbligatorio | `true`, `false` | toglie a ogni riga la parte coperta dalle righe precedenti |
| `fill_gaps` | booleano | obbligatorio | `true`, `false` | chiude, dentro ogni riga, rientranze e varchi più stretti di `2 · snap_tolerance` |

`remove_overlaps` e `fill_gaps` sono obbligatori, senza valore
predefinito: cambiano la geometria delle righe, e il piano lo dice
esplicitamente. Fino alla versione 1 del catalogo erano facoltativi e il
runner, quando mancavano, usava `true` per entrambi. `fill_gaps` con
`snap_tolerance` pari a `0` non fa nulla. Con tutti e due `false` le righe
escono invariate (dopo il controllo di validità).

### Schema

Quello dell'ingresso: stesse colonne, tipi e metadati di schema. La
colonna geometria resta al suo posto, con lo stesso nome e lo stesso CRS,
in XY; i tipi geometrici dichiarati diventano `Polygon` e `MultiPolygon`
(`exact`) e le chiavi dei tipi ereditate dal campo si tolgono. La colonna
geometria dell'uscita è sempre nullable, anche quando quella d'ingresso
non lo è: una riga coperta del tutto dalle precedenti diventa null. Le
altre colonne conservano la loro nullabilità; le proprietà del contratto
(`sorted_by`, `row_count`) restano.

### Righe

1:1: il kernel rende un risultato per riga, nella stessa posizione.

- **Chiusura** (`fill_gaps` e `snap_tolerance > 0`): ogni riga, da sola,
  passa per un buffer di `+snap_tolerance` e poi di `-snap_tolerance`
  (estremità tonde) e diventa un `MultiPolygon`. Riempie le rientranze e i
  varchi della riga più stretti di `2 · snap_tolerance`, non i vuoti fra
  righe diverse.
- **Sovrapposizioni** (`remove_overlaps`): ogni riga perde la parte coperta
  dalle righe precedenti che la toccano, prese dopo la chiusura. Resta un
  `MultiPolygon`; una riga coperta del tutto resta senza geometria (`None`
  dal kernel). Una riga senza precedenti che la toccano resta com'è dopo
  la chiusura, senza chiusura anche nel tipo.

Il runner passa al kernel solo le geometrie non nulle, nell'ordine delle
righe, e riporta ogni risultato alla sua riga: una cella nulla resta
nulla e non conta come riga precedente; una riga che il kernel rende
senza geometria (coperta del tutto) diventa null. Gli altri attributi
restano invariati.

### Ordine

Quello dell'ingresso, che decide anche il risultato: una riga precedente
vince sempre su una successiva nelle sovrapposizioni.

### Errori

In validazione (analisi del contratto), `InvalidPlan`:

- config con campi sconosciuti, `snap_tolerance`, `remove_overlaps` o
  `fill_gaps` assente («missing field»), o un campo del tipo sbagliato;
- `snap_tolerance` negativa o non finita.

Sempre in validazione: `Schema` se l'ingresso non ha esattamente una
colonna geometria o la colonna non è riconoscibile come geometria WKB;
`Unsupported` se non è XY; `Crs` se il CRS non è risolto o non è
proiettato (o non ha unità lineare).

In esecuzione, prima del kernel, su ogni cella non nulla ([README,
«Operazioni geo»](../README.md#operazioni-geo)): `InvalidPlan` per un WKB
malformato o con coordinate non finite, `Unsupported` per dimensioni Z/M o
SRID, `Crs` per una coordinata fuori dal dominio di validità del CRS della
colonna, `Schema` per una geometria di un tipo che il contratto
dell'ingresso dichiara con un elenco e che non vi compare. Poi la
decodifica completa con la validazione OGC: `InvalidPlan` per una
geometria non valida (la riparazione è di `geo.make_valid`), `Internal`
se la validazione non conclude.

Poi il kernel `topology::clean_valid_polygon_topology_validated`, con
errore `TopologyError` che il runner traduce così: `ValidazioneNonConclusa`
e `CalcoloNonConcluso` diventano `Internal`, `PrecisionInsufficient`
diventa `Unsupported`, le altre `InvalidPlan`. Il messaggio è quello del
kernel; un indice che vi compare conta le sole geometrie non nulle, non le
righe. Le varianti:

- `ResourceLimit`: le geometrie non nulle superano 100.000.000 o i
  vertici `MAX_CLEAN_VERTICES` (100.000.000), i limiti che il runner passa
  al kernel;
- `UnsupportedGeometry`: una geometria non è `Polygon`/`MultiPolygon`;
- `InvalidGeometry`: una geometria d'ingresso non supera la validazione
  OGC (il runner la rifiuta già alla decodifica, sopra), o la
  chiusura o la rimozione delle sovrapposizioni produce una geometria non
  valida;
- `ValidazioneNonConclusa`: una validazione OGC non ha concluso;
- `PrecisionInsufficient`: un buffer della chiusura o un overlay
  sposterebbe il risultato oltre la precisione (sotto, «Precisione»);
- `CalcoloNonConcluso`: un buffer o un overlay di `geo` è andato in panico.

### Limiti e deviazioni

Gli archi della chiusura hanno freccia al più `max(p / 8, 0,001 ·
snap_tolerance)`: oltre `snap_tolerance` di 1,25 m il bordo chiuso può
rientrare fino allo 0,1% della tolleranza, senza errore ([README,
«Precisione delle operazioni geografiche: 1 cm a
terra»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra),
«Deviazione: archi del buffer»). La nullabilità dichiarata della colonna
geometria dell'uscita è sempre nullable, perché il kernel può rendere
righe senza geometria. Nessuna diagnostica per riga: il passo rende il primo
errore ([README, «Limiti dichiarati del
runner»](../README.md#limiti-dichiarati-del-runner), voci «Geo senza
diagnostica per riga» e «Modelli di costo geo»).

### Precisione

Entro 1 cm a terra, diviso fra i passi. Con la chiusura: due buffer, ognuno
con archi di freccia `p / 8` (o la deviazione sopra) e griglia entro `p /
4`, e la rimozione delle sovrapposizioni (unione delle righe vicine e
differenza, due overlay in catena) con griglia entro `p / 4`: in tutto
`p`. Senza chiusura la rimozione ha mezzo centimetro. Ogni griglia è
controllata prima del calcolo; oltre, `PrecisionInsufficient`. Ogni resto
si calcola dalle righe d'ingresso vicine, non da un'unione accumulata riga
dopo riga, così gli spostamenti non si sommano lungo la tabella. Parti più
sottili di 1 cm possono sparire o fondersi senza errore; vedi [README,
«Precisione delle operazioni geografiche: 1 cm a
terra»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra).

### Complessità

Chiusura: due buffer per riga, ognuno un overlay sui vertici della riga e
dei suoi archi. Sovrapposizioni: un R-tree dei rettangoli d'ingombro,
O(n log n) per `n` righe, poi per ogni riga un'unione delle precedenti
vicine e una differenza: di norma proporzionale ai vicini, nel caso
peggiore (rettangoli tutti sovrapposti) O(n²) coppie di righe. Memoria:
l'intera tabella e l'indice.

### Esempio

```json
{
  "config": {"snap_tolerance": 0, "remove_overlaps": true, "fill_gaps": false},
  "ingressi": [
    {"nome": "particelle", "colonne": [
      {"nome": "id", "tipo": "int64", "valori": [1, 2, 3]},
      {"nome": "geometry", "tipo": "geometry", "crs": "EPSG:3857", "valori": ["POLYGON((0 0,2 0,2 2,0 2,0 0))", "POLYGON((1 0,3 0,3 2,1 2,1 0))", "POLYGON((0.5 0.5,1.5 0.5,1.5 1.5,0.5 1.5,0.5 0.5))"]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "id", "tipo": "int64", "valori": [1, 2, 3]},
    {"nome": "geometry", "tipo": "geometry", "crs": "EPSG:3857", "valori": ["POLYGON((0 0,2 0,2 2,0 2,0 0))", "MULTIPOLYGON(((2 2,2 0,3 0,3 2,2 2)))", null]}
  ]}
}
```
