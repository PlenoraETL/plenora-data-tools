### Che cosa fa

Raggruppa le righe per le colonne `group_by` e raccoglie le geometrie di
ogni gruppo in una sola, **senza unione topologica**: punti, linee o
poligoni tutti dello stesso tipo diventano il multi corrispondente, un
gruppo misto una `GeometryCollection`, un gruppo di una sola geometria
resta quella. Le geometrie nulle si saltano. Le colonne che non sono chiavi
spariscono.

Il runner forma i gruppi su tutta la tabella e per ciascuno chiama
`extensions::collect_geometries` con le geometrie del gruppo nell'ordine
delle righe ([Runner, «Operazioni geo»](runner.md#operazioni-geo)).

### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `group_by` | lista di stringhe | obbligatorio | colonne dell'ingresso, non vuota, senza ripetizioni, diversa dalla colonna geometria, di un tipo con un ordine naturale (quelli di `table.sort`) | chiavi di gruppo |

### Schema

Prima la colonna geometria, con nome e metadati d'ingresso (la
dichiarazione dei tipi riscritta), nullable solo se lo è quella
d'ingresso (null per un gruppo di sole geometrie null); poi le colonne `group_by`,
nell'ordine della lista, identiche all'ingresso (tipo, nullabilità,
metadati). I metadati di schema restano; nessuna proprietà del contratto
sopravvive. Tipi dichiarati, se l'ingresso li dichiara `exact` o `mixed`
con elenco: quelli d'ingresso, più il multi di `Point`, `LineString` e
`Polygon` presenti, più `GeometryCollection` salvo che l'ingresso dichiari
un solo tipo semplice.

### Righe

Aggregazione: una riga per gruppo, con la geometria raccolta e i valori
chiave della prima riga del gruppo. Due righe stanno nello stesso gruppo
se ogni chiave è uguale per il confronto tipizzato di `table.sort`
(`compare_cells_typed` dei kernel tabellari: numeri per valore, testo per
byte, istanti per istante, `Float64` per `total_cmp`, quindi `-0.0` e
`0.0` in gruppi distinti e un NaN uguale solo a un NaN con gli stessi
bit), o è nulla in entrambe: un valore nullo è un valore di gruppo come
gli altri, distinto dal testo vuoto. Un gruppo senza geometrie non nulle dà una geometria nulla; una
tabella vuota non dà righe.

### Ordine

I gruppi escono nell'**ordine naturale dei valori** delle chiavi, dalla
prima alla seconda a parità della prima, come `table.sort` crescente:
numeri per valore (`-10` prima di `-1`, `9` prima di `10`), testo per
byte UTF-8 (`"aaaaaaaaa"` prima di `"bbbbbbbbbb"` qualunque sia la
lunghezza), date e istanti per istante, booleani `false` prima di `true`,
`Float64` per `total_cmp`; una chiave nulla dopo i valori. Dentro una
geometria raccolta i membri seguono l'ordine delle righe. Fino alla
versione 1 del catalogo l'ordine era quello della chiave testuale di
`190c493`, con la lunghezza del valore scritta come testo in testa: un
valore di 10 caratteri prima di uno di 9, `3` prima di `-5`, e una chiave
nulla prima dei valori.

### Errori

In validazione (analisi del contratto):

- `InvalidPlan`: config con campi sconosciuti; `group_by` vuota, con nomi
  vuoti, con ripetizioni o con la colonna geometria; una chiave di un tipo
  senza un ordine naturale (`is_sortable` dei kernel tabellari, lo stesso
  controllo di `table.sort`);
- `Schema`: una colonna di `group_by` non esiste; l'ingresso non ha
  esattamente una colonna geometria, o la colonna non è riconoscibile come
  WKB;
- `Unsupported`: dimensioni della geometria diverse da `xy`;
- `Crs`: colonna senza CRS o con un'incoerenza CRS non risolta.

In esecuzione, prima del kernel, su ogni cella non nulla ([Runner,
«Operazioni geo»](runner.md#operazioni-geo)): `InvalidPlan` per un WKB
malformato o con coordinate non finite, `Unsupported` per dimensioni Z/M o
SRID, `Crs` per una coordinata fuori dal dominio di validità del CRS della
colonna, `Schema` per una geometria di un tipo che il contratto
dell'ingresso dichiara con un elenco e che non vi compare. Poi la
decodifica completa con la validazione OGC: `InvalidPlan` per una
geometria non valida, `Internal` se la validazione non conclude.

Il confronto delle chiavi fallisce, con un errore esplicito (`Schema`),
per una chiave di dizionario fuori dal dizionario.

Il calcolo di un gruppo rifiuta una geometria che non supera la validazione
OGC (`ExtensionError::InvalidInput`) e una raccolta che non la supera
(`ExtensionError::InvalidOutput`): per esempio due poligoni che si
sovrappongono non formano un `MultiPolygon` valido. Il runner li traduce
in `InvalidPlan`; una validazione che non conclude è `Internal`.

### Limiti e deviazioni

Nessuna unione: poligoni che si sovrappongono o si toccano lungo un lato
si rifiutano invece di fondersi (per l'unione c'è `geo.dissolve`).
Nessuna diagnostica per riga: il passo rende il primo errore ([Runner,
«Limiti dichiarati del runner»](runner.md#limiti-dichiarati-del-runner),
voci «Geo senza diagnostica per riga» e «Modelli di costo geo»).

### Precisione

Esatta: le geometrie si copiano senza calcolo
([Limiti dichiarati, «Precisione delle operazioni geografiche: 1 cm a terra»](limiti.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)).

### Complessità

O(n log n) sulle righe per l'ordinamento delle chiavi e O(n) per la
raccolta, più la validazione OGC di ogni geometria
e di ogni raccolta (O(v²) nel caso peggiore sui vertici `v` del gruppo);
memoria O(n) per le geometrie raccolte.

### Esempio

```json
{
  "config": {"group_by": ["zona"]},
  "ingressi": [
    {"nome": "pozzi", "colonne": [
      {"nome": "zona", "tipo": "utf8", "valori": ["a", "a", "b"]},
      {"nome": "geometry", "tipo": "geometry", "crs": "EPSG:3857", "valori": ["POINT(1 1)", "POINT(2 2)", "POINT(5 5)"]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "geometry", "tipo": "geometry", "crs": "EPSG:3857", "valori": ["MULTIPOINT((1 1),(2 2))", "POINT(5 5)"]},
    {"nome": "zona", "tipo": "utf8", "valori": ["a", "b"]}
  ]}
}
```
