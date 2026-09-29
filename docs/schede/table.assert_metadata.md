### Che cosa fa

Verifica i metadati di schema dell'ingresso: ogni coppia di `expected` deve
esserci, con lo stesso valore; con `allow_extra=false` non devono esserci
altre chiavi. Se l'asserzione regge, l'uscita è l'ingresso invariato; se non
regge, il passo fallisce e non produce uscita. Guarda solo lo schema: nel
runner l'esito si decide in validazione.

### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `expected` | oggetto stringa → stringa | obbligatorio | da 1 a 4096 coppie; chiavi non vuote; chiavi e valori al più `max_string_bytes` byte | coppie che i metadati di schema devono contenere |
| `allow_extra` | booleano | `true` | `true`, `false` | con `false` i metadati hanno esattamente le chiavi di `expected` |

Il confronto è fra testi, byte per byte. Solo i metadati di schema, non
quelli di campo. Il runner toglie dagli ingressi la chiave `pandas` prima
della validazione: non si può asserire.

### Schema

Identico all'ingresso: stesse colonne, tipi, nullabilità, metadati di campo
e di schema; il contratto conserva ordinamento dichiarato (`sorted_by`) e
numero di righe.

### Righe

1:1: tutte le righe, invariate.

### Ordine

L'ordine d'ingresso.

### Errori

In validazione, `InvalidPlan`:

- `expected` vuoto, oltre 4096 coppie, con una chiave vuota o una chiave
  o un valore oltre `max_string_bytes`;
- una chiave di `expected` manca dai metadati di schema o ha un altro
  valore (il messaggio nomina la chiave, non i valori);
- `allow_extra=false` e metadati con chiavi in più;
- config con campi sconosciuti.

In esecuzione: nel runner nessuno, perché i metadati dei dati sono quelli
del contratto controllato in validazione. Il kernel chiamato fuori dal
runner rifiuta gli stessi casi con `Schema` (`metadata non conforme`).

### Limiti e deviazioni

Nessuno oltre ai limiti comuni.

### Complessità

Tempo O(k) sulle coppie attese, indipendente dalle righe; memoria O(1):
l'uscita condivide le colonne dell'ingresso.

### Esempio

```json
{
  "config": {"expected": {"fonte": "anagrafe"}},
  "ingressi": [
    {"nome": "comuni", "metadati": {"fonte": "anagrafe"}, "colonne": [
      {"nome": "codice", "tipo": "utf8", "valori": ["001", "002"]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "codice", "tipo": "utf8", "valori": ["001", "002"]}
  ]}
}
```
