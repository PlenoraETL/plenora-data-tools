### Che cosa fa

Tiene al più `n` righe consecutive, a partire dalla riga `offset` (contata
da 0), e scarta le altre. Nel runner si applica alla tabella intera. Le
righe tenute non si copiano: l'uscita è una finestra sull'ingresso.

### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `n` | intero | obbligatorio | da 0 a `max_rows` | righe da tenere al più |
| `offset` | intero | `0` | da 0 a `max_rows` | righe da saltare in testa |

`max_rows` è il `max_input_rows` del piano. Un `offset` oltre le righe
dell'ingresso dà una tabella vuota; `n = 0` dà una tabella vuota con lo
stesso schema.

### Schema

Identico all'ingresso: stesse colonne, tipi, nullabilità e metadati.
Contratto: l'ordinamento dichiarato resta, il conteggio delle righe non è
più noto.

### Righe

Filtro: le righe da `offset` a `offset + n - 1`, quelle che esistono.

### Ordine

Le righe tenute restano nell'ordine d'ingresso: `limit` dopo un
`table.sort` dà le prime `n` secondo l'ordinamento.

### Errori

In validazione, `InvalidPlan`:

- `n` assente, `n` o `offset` negativi o oltre `max_rows`;
- config con campi sconosciuti.

In esecuzione: nessun errore che dipenda dai dati (un `ResourceLimit` solo
se `n` o `offset` non stanno in un `usize` della piattaforma).

### Limiti e deviazioni

Chiamato direttamente su un solo blocco di righe, il kernel limita quel
blocco: non tiene stato fra blocchi. Nel runner ogni tabella è un blocco
solo, quindi il limite vale per la tabella.

### Complessità

Tempo O(c) sulle colonne, indipendente dalle righe; nessuna memoria per i
dati (finestra sugli array d'ingresso).

### Esempio

```json
{
  "config": {"n": 2, "offset": 1},
  "ingressi": [
    {"nome": "t", "colonne": [
      {"nome": "id", "tipo": "int64", "valori": [10, 20, 30, 40]},
      {"nome": "nome", "tipo": "utf8", "valori": ["a", null, "c", "d"]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "id", "tipo": "int64", "valori": [20, 30]},
    {"nome": "nome", "tipo": "utf8", "valori": [null, "c"]}
  ]}
}
```
