### Che cosa fa

Traspone la tabella: le colonne diventano righe e le righe colonne. La
prima colonna d'uscita elenca i nomi delle colonne dati; ogni riga
d'ingresso diventa una colonna. Il numero di colonne d'uscita dipende dai
dati: il runner rifiuta l'operazione in validazione, e l'esempio sotto è
eseguito chiamando il kernel.

### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `id_column` | stringa | nessuno | nome di una colonna dell'ingresso | i suoi valori danno i nomi delle colonne d'uscita, e non si traspone |
| `output_columns` | lista di stringhe | `[]` | nomi di colonna | nomi delle colonne d'uscita, per posizione di riga |
| `type_policy` | stringa | `"reject"` | `"reject"`, `"string"`; `null` non ammesso | colonne dati di tipi diversi: rifiuto, o conversione in testo |

Nome della colonna della riga i (da 0): `output_columns[i]` se c'è e non
è vuoto; altrimenti il testo della cella di `id_column` alla riga i, se non
è nulla; altrimenti `col_<i+1>`. I nomi in più di `output_columns` si
ignorano. `type_policy` con colonne dati tutte dello stesso tipo si accetta
e non cambia niente: dipende dall'ingresso.

### Schema

Prima colonna: `id_column` (o `col_0` senza), `utf8` non nullabile, con i
nomi delle colonne dati (tutte tranne `id_column`, nel loro ordine). Poi
una colonna per riga d'ingresso, nullabile: del tipo comune se le colonne
dati hanno tutte lo stesso tipo Arrow, altrimenti `utf8` con
`type_policy: "string"`, con il testo delle celle (`1.0` è `"1"`, le date
`AAAA-MM-GG`). Metadati di campo e di schema non si conservano. Un
ingresso senza righe esce invariato, schema compreso.

### Righe

Una riga per colonna dati dell'ingresso.

### Ordine

Righe nell'ordine delle colonne dati; colonne nell'ordine delle righe
d'ingresso.

### Errori

In validazione, il runner rifiuta sempre `table.transpose`: `InvalidPlan`
per una config con campi sconosciuti o `type_policy: null` esplicito (il
parametro si omette), `output_columns` con un nome
ripetuto o non valido (anche vuoto) o oltre il limite di colonne, o
`id_column` assente; altrimenti `Unsupported` (lo schema d'uscita dipende
dai dati).

Chiamando il kernel:

- `Schema`: `id_column` assente; una cella di `id_column`, o con la
  conversione in testo una cella dati, che non si converte in testo (tipo
  non leggibile come testo, `binary` non UTF-8, date fuori intervallo);
- `InvalidPlan`: colonne dati di tipi diversi con `type_policy: "reject"`;
  un nome di colonna d'uscita non valido (per esempio il testo di
  `id_column` vuoto o di soli spazi);
- `ResourceLimit`: colonne dati oltre `max_rows` o righe più una oltre
  `max_columns`; un testo oltre `max_string_bytes`.

### Limiti e deviazioni

Chiamando il kernel non si rifiutano nomi d'uscita ripetuti: valori
ripetuti di `id_column`, o nomi ripetuti in `output_columns`, danno
colonne con lo stesso nome. Il kernel tratta una voce vuota di
`output_columns` come assente, mentre l'analisi la rifiuta.

### Complessità

Tempo e memoria O(n · c) per n righe e c colonne dati.

### Esempio

```json
{
  "config": {"id_column": "metrica"},
  "ingressi": [
    {"nome": "misure", "colonne": [
      {"nome": "metrica", "tipo": "utf8", "valori": ["min", "max"]},
      {"nome": "a", "tipo": "int64", "valori": [1, 9]},
      {"nome": "b", "tipo": "int64", "valori": [2, 8]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "metrica", "tipo": "utf8", "valori": ["a", "b"]},
    {"nome": "min", "tipo": "int64", "valori": [1, 2]},
    {"nome": "max", "tipo": "int64", "valori": [9, 8]}
  ]}
}
```
