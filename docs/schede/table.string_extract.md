### Che cosa fa

Cerca l'espressione regolare `pattern` nel testo della colonna `column` ed
estrae ciò che trova in colonne `utf8`. Con gruppi con nome
(`(?P<nome>...)`) scrive una colonna per gruppo, col nome del gruppo, dal
primo match. Senza, scrive una colonna sola con il primo gruppo di cattura,
o con il match intero se il pattern non ha gruppi; con `extract_all` unisce
con `","` i valori di tutti i match.

### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `column` | stringa | obbligatorio | colonna `utf8` dell'ingresso | testo in cui cercare |
| `pattern` | stringa | obbligatorio | regex non vuota della sintassi del crate `regex`, al più `max_regex_bytes` byte | espressione da cercare |
| `output_column` | stringa o `null` | `null` | nome non vuoto, al più 1024 byte | colonna d'uscita senza gruppi con nome; `null` vale `<column>_extracted` |
| `extract_all` | booleano | `false` | `true`, `false` | estrae tutti i match invece del primo |

Con gruppi con nome `output_column` ed `extract_all` si rifiutano: le
colonne prendono il nome dei gruppi e si estrae il primo match.

La sintassi è quella del crate Rust `regex`: niente lookaround né
riferimenti all'indietro, classi Unicode per default (`\d` riconosce anche
le cifre non latine, come `٣`).

### Schema

Ogni colonna prodotta è `utf8` nullable: se esiste già si sostituisce nella
sua posizione (senza i metadati di campo di prima), altrimenti si aggiunge
in coda; i gruppi con nome nell'ordine in cui compaiono nel pattern. Le
altre colonne e i metadati di schema restano.

Contratto: il conteggio delle righe resta; l'ordinamento dichiarato resta
solo se nessuna colonna esistente è stata sostituita.

### Righe

1:1. Danno null: la cella null, nessun match, un gruppo che nel match non
partecipa. Con `extract_all` i match in cui il primo gruppo non partecipa
si saltano; nessun valore dà null.

### Ordine

Righe nell'ordine d'ingresso; con `extract_all` i match da sinistra a
destra, senza sovrapposizioni.

### Errori

In validazione, `InvalidPlan`:

- `column` assente o non `utf8`;
- `pattern` vuoto, oltre `max_regex_bytes`, o non valido (compresi i nomi
  di gruppo ripetuti e i pattern troppo grandi una volta compilati);
- gruppi con nome insieme a `output_column` o a `extract_all`;
- un nome d'uscita (scritto, derivato o di gruppo) oltre 1024 byte;
- config con campi sconosciuti.

In esecuzione: nessun errore che dipenda dai dati.

### Limiti e deviazioni

Con `extract_all` la virgola separa i match anche se un valore estratto
contiene una virgola: il risultato non si può sempre ridividere senza
ambiguità.

### Complessità

Tempo lineare nei byte della colonna per un pattern fissato (il crate
`regex` garantisce ricerca in tempo lineare); memoria pari ai byte delle
colonne d'uscita.

### Esempio

```json
{
  "config": {"column": "codice", "pattern": "(?P<sede>[A-Z]{2})-(?P<numero>\\d+)"},
  "ingressi": [
    {"nome": "t", "colonne": [
      {"nome": "codice", "tipo": "utf8", "valori": ["rif MI-204", "nessuno", null]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "codice", "tipo": "utf8", "valori": ["rif MI-204", "nessuno", null]},
    {"nome": "sede", "tipo": "utf8", "valori": ["MI", null, null]},
    {"nome": "numero", "tipo": "utf8", "valori": ["204", null, null]}
  ]}
}
```
