### Che cosa fa

Raggruppa le righe per le colonne `group_by` e raccoglie le geometrie di
ogni gruppo in una sola, **senza unione topologica**: punti, linee o
poligoni tutti dello stesso tipo diventano il multi corrispondente, un
gruppo misto una `GeometryCollection`, un gruppo di una sola geometria
resta quella. Le geometrie nulle si saltano. Le colonne che non sono chiavi
spariscono.

Il calcolo di un gruppo è `extensions::collect_geometries`, che riceve le
geometrie del gruppo già raccolte e ordinate. Il raggruppamento a livello
di tabella non è implementato in questo workspace: il runner non esegue le
operazioni geo, e nessun altro codice forma i gruppi.

### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `group_by` | lista di stringhe | obbligatorio | colonne dell'ingresso, non vuota, senza ripetizioni, diversa dalla colonna geometria | chiavi di gruppo |

### Schema

Prima la colonna geometria, con nome e metadati d'ingresso (la
dichiarazione dei tipi riscritta) e nullable; poi le colonne `group_by`,
nell'ordine della lista, identiche all'ingresso (tipo, nullabilità,
metadati). I metadati di schema restano; nessuna proprietà del contratto
sopravvive. Tipi dichiarati, se l'ingresso li dichiara `exact` o `mixed`
con elenco: quelli d'ingresso, più il multi di `Point`, `LineString` e
`Polygon` presenti, più `GeometryCollection` salvo che l'ingresso dichiari
un solo tipo semplice.

### Righe

Aggregazione: una riga per gruppo. Un gruppo senza geometrie non nulle dà
una geometria nulla. Come si trattano le chiavi nulle non è ancora fissato
da nessuna esecuzione a livello di tabella.

### Ordine

Il catalogo dichiara l'ordine canonico dei valori; nessuna esecuzione a
livello di tabella lo realizza ancora. Dentro una geometria raccolta, i
membri seguono l'ordine in cui il gruppo arriva al calcolo.

### Errori

In validazione (analisi del contratto):

- `InvalidPlan`: config con campi sconosciuti; `group_by` vuota, con nomi
  vuoti, con ripetizioni o con la colonna geometria;
- `Schema`: una colonna di `group_by` non esiste; l'ingresso non ha
  esattamente una colonna geometria, o la colonna non è riconoscibile come
  WKB;
- `Unsupported`: dimensioni della geometria diverse da `xy`;
- `Crs`: colonna senza CRS o con un'incoerenza CRS non risolta.

In esecuzione, il calcolo di un gruppo rifiuta una geometria che non supera
la validazione OGC (`ExtensionError::InvalidInput`) e una raccolta che non
la supera (`ExtensionError::InvalidOutput`): per esempio due poligoni che si
sovrappongono non formano un `MultiPolygon` valido. Nella traduzione
`ExtensionError::del_passo` sono `InvalidPlan`; una validazione che non
conclude è `Internal`.

### Limiti e deviazioni

Nessuna unione: poligoni che si sovrappongono o si toccano lungo un lato
si rifiutano invece di fondersi (per l'unione c'è `geo.dissolve`).

### Precisione

Esatta: le geometrie si copiano senza calcolo
([README, «Precisione delle operazioni geografiche: 1 cm a terra»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)).

### Complessità

O(n) sulle righe per la raccolta, più la validazione OGC di ogni geometria
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
