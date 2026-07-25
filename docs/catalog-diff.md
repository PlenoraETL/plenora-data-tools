# Diff dei cataloghi — plenora-nogeo-tools vs plenora-geo-tools-arrow

Data: 2026-07-24. Fonte: `src/catalog.rs` dei due progetti. Prerequisito di
Fase 0 per la tabella alias versionata (decisione D20).

## Conteggi

| Catalogo | Operazioni | Note |
|---|---|---|
| nogeo-tools (tabellare) | 62 | 37 Manipola-compat + 25 estensioni |
| geo-tools-arrow (geografico) | 65 | 33 Manipola-compat + 32 estensioni (di cui 11 predicati DE-9IM) |
| **Totale unificato** | **127** | |

## Esito principale: zero collisioni esatte di id

Gli id dei due cataloghi **non si sovrappongono mai esattamente**. Il
catalogo geo usa già prefissi (`geo_*`, `predicate_*`) per 44 operazioni su
65; le 21 estensioni geo con id "nudi" (`translate`, `scale`, `rotate`,
`split`, `densify`, …) non collidono con nessun id tabellare.

La tabella alias può quindi essere **puramente additiva**: nessun conflitto da
risolvere, nessuna operazione rinominata per collisione.

## Coppie semanticamente vicine (attenzione in documentazione, non collisioni)

| Tabellare | Geografica | Nota |
|---|---|---|
| `filter` | `geo_within` | filtro per predicato vs filtro spaziale |
| `explode` | `geo_explode` | explode di array/List vs esplosione di multi-geometrie |
| `join` | `geo_nearest` / `geo_overlay` | join su chiave vs spatial join |
| `union_distinct` | `geo_union` | set union di righe vs union geometrica |
| `intersect` | `geo_intersection` | set intersection vs intersection geometrica |
| `except` | `geo_difference` | set difference vs difference geometrica |
| `aggregate` | `geo_dissolve` | aggregazione tabellare vs dissolve |
| `concat` | `line_merge` | concatenazione righe vs merge di linee |
| `distinct` | — | nessun equivalente geo |
| `split_column` | `split` | split di stringhe vs split di geometrie (GEOS) |

Queste coppie vanno rese evidenti nella documentazione del catalogo unificato:
stesso "nome di famiglia", semantica completamente diversa.

## Mapping proposto verso gli id canonici

Namespace come da Architetture.md par. 4.3: `table.*` e `geo.*`.

### Tabellari (62)

`table.<id>` con id invariato: `filter` → `table.filter_rows`? **No** —
decisione: l'id storico resta invariato sotto il namespace
(`filter` → `table.filter`), per minimizzare la distanza dal piano legacy.
Eventuali rinomine migliorative (es. `filter` → `filter_rows`) sono alias
aggiuntivi, non sostituzioni.

### Geografiche (65)

Il prefisso `geo_` storico diventa il namespace (gli esempi in Architetture.md
usano `geo.buffer`, `geo.reproject`):

- `geo_buffer` → `geo.buffer` (33 operazioni Manipola-compat con prefisso);
- `predicate_intersects` → `geo.predicate_intersects` (11 predicati);
- estensioni nude → namespace diretto: `translate` → `geo.translate`,
  `scale` → `geo.scale`, `rotate` → `geo.rotate`, `split` → `geo.split`,
  `concave_hull` → `geo.concave_hull`, ecc.

## Tabella alias versionata (D20)

Forma: `(schema_version, legacy_alias) -> canonical_operation_id`, immutabile
per le versioni pubblicate.

Regole per `schema_version <= 3` (piani nogeo legacy):

- ogni id nogeo storico → `table.<id>` (62 voci, tutte non ambigue);
- nessuna voce geo: i piani nogeo non potevano contenerne.

Regole per gli schemi del protocollo geo (v2/v3, `TransformArrowSchema`):

- `geo_buffer` → `geo.buffer`, `translate` → `geo.translate`, ecc.
  (65 voci, tutte non ambigue).

Voci totali della tabella alias v1: 62 + 65 = **127**, una per operazione,
nessuna ambiguità. Un alias introdotto non può mai essere riassegnato: nuove
operazioni future ricevono id canonici direttamente namespaced.

## Nota di copertura

Il catalogo geo dichiara livelli di supporto (`public_protocol`,
`kernel_validated`, `backend_pending`, `planned`): il mapping sopra copre
tutte le 65 voci. Semantica di maturity (decisione registrata):

- `Planned`: **non eseguibile** (rifiutata in validazione);
- `KernelValidated`: **sperimentale ma eseguibile**, senza garanzia di
  stabilità del contratto (il validatore la accetta);
- `PublicProtocol`: contratto stabile e compatibile;
- `BackendPending`: eseguibile solo quando la capability richiesta è
  disponibile (es. backend GEOS/PROJ compilato).

Le 19 operazioni aggiunte nelle estensioni v1.1–v1.3 restano
`KernelValidated` fino alla campagna fuzz: solo dopo quella e una release
saranno candidate alla promozione a `PublicProtocol`.
