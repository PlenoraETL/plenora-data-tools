# Addendum 2026-08-20 — i documenti storici e il nome del budget di memoria

ADR 15 ha rinominato il budget di memoria della libreria in
`max_governed_memory_bytes`, e l'attuazione del livello 1 lo ha portato nel
codice, nel formato del piano e nella documentazione normativa.

**I documenti conclusi non sono stati riscritti.** Questo addendum esiste
perché la correzione di un documento concluso è una perdita di prova: chi
rilegge un verbale vuole sapere che cosa è stato osservato e con quali nomi,
non che cosa avremmo scritto oggi.

## Che cosa resta col nome della v4, e perché va bene

| documento | natura |
|---|---|
| `docs/misure/baseline-671214c.{json,txt}`, `docs/misure/varianza/`, `docs/misure/decomposizione/`, `docs/misure/sonde-formula/`, `docs/misure/m2d-dopo/` | grezzi di misura, verificati da script ad ancore |
| `docs/misura-orchestrazione-2026-08-19.md`, `docs/decomposizione-wall-2026-08-20.md`, `docs/formula-strumentazione-2026-08-20.md`, `docs/m2d-staging-memory-first-2026-08-21.md` | verbali chiusi |
| `docs/der011-censimento-2026-08-21.md` | censimento dei siti di allocazione |
| `docs/review-*.md`, `docs/checkpoint-*.md` | esiti di review |
| `release/1.0.0.json` … `release/1.0.3.json`, `release/rc.json` | manifesti di rilascio |

Tutti questi restano **byte per byte** come erano. I quattro verificatori dei
verbali e il verificatore del censimento continuano a passare: sono ancorati
al contenuto e alla forma del codice, non al nome del campo.

## La regola di lettura

Dove un documento datato prima del 2026-08-20 scrive `max_memory_bytes`,
intende ciò che oggi si chiama `max_governed_memory_bytes`. Il **numero** non
è cambiato, il **perimetro** non è cambiato, e nessuna misura è stata
ripetuta: è cambiato il nome, e con esso la promessa che il nome faceva.

Una conseguenza pratica: un piano preso da uno di quei documenti e incollato
in un file non funziona più così com'è. Va portato alla v5 — chiave rinominata
e `schema_version: 5` — oppure lasciato a `schema_version: 4`, e in quel caso
la migrazione lo converte da sé.

## Che cosa NON è cambiato nelle misure

I `plan_hash` **cambiano tutti** (ADR 4, emendamento 2026-08-20), ma nessun
verbale ne registra uno: i verbali misurano tempi, byte e conteggi. Chi
riesegue `misura_orchestrazione` oggi ottiene gli stessi ordini di grandezza
sugli stessi piani — i piani nel sorgente dell'harness sono stati portati alla
v5 — e non c'è ragione di aspettarsi uno spostamento, perché la
rinominazione non tocca alcun percorso di esecuzione. Detto con precisione:
**non è stato rimisurato**, e questa frase è un'aspettativa, non un dato.

## Dove leggere la versione corrente

- `docs/adr/ADR-0015-contratto-della-memoria.md` §7 — che cosa è stato attuato
  e che cosa no;
- `docs/adr/ADR-0004-validatedgraph-fingerprint.md`, emendamento 2026-08-20 —
  il separatore di dominio del `plan_hash`;
- `docs/adr/ADR-0006-semantica-limiti.md` — semantica del limite e ordine
  rispetto alla migrazione;
- `docs/api-breaking-2026-08-16.md`, aggiunta 2026-08-20 — la superficie rotta;
- `docs/deroghe.md`, DER-011, aggiornamento 2026-08-20 — che cosa la
  rinominazione chiude e che cosa no.
