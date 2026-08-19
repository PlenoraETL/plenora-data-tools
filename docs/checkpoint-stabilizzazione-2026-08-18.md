# Checkpoint della stabilizzazione — 2026-08-18

Chiude il ciclo di review indipendenti iniziato il 2026-08-16 (giri 2→16).
Baseline funzionale per il lavoro sull'orchestratore.

**Non e' una dichiarazione di produzione**: i rischi della sezione 6 restano
aperti e sono la condizione per quella dichiarazione, non per l'inizio del
lavoro architetturale.

---

## 1. Inventario

`HEAD` di partenza: `4b9edda`. Nessun commit prodotto durante la
stabilizzazione: il worktree e' l'unico portatore del lavoro.

| | file |
|---|---|
| Modificati | **86** |
| Nuovi | **39** (13 845 righe) |
| **Totale** | **125** |

Modificati per area:

| area | file |
|---|---|
| `plenora-kernels-table` | 37 |
| `plenora-engine` | 23 |
| `docs` | 9 |
| `plenora-cli` | 7 |
| `plenora-core` | 6 |
| `plenora-kernels-geo` | 3 |
| `README.md` | 1 |

Diff sui soli file tracciati: **+10 360 / −2 781**.

### Nuovi moduli sorgente

| percorso | cosa contiene |
|---|---|
| `plenora-core/src/json.rs` | `ensure_no_duplicate_keys` |
| `plenora-core/src/panic_policy.rs` | politica del panic hook (DER-010) |
| `plenora-engine/src/error_propagation.rs` | `categoria_preservata`: ogni errore gia' classificato conserva la categoria |
| `plenora-engine/src/ipc_boundary.rs` (+ `tests.rs`) | confine di lettura Arrow IPC |
| `plenora-engine/src/parallelism.rs` | parallelismo di processo (DER-006) |
| `plenora-kernels-table/src/exact_compare.rs` | confronti esatti senza conversione |
| `plenora-kernels-table/src/hashing.rs` | `KeyHasher` / `FastHasher` (DER-009) |
| `plenora-kernels-table/src/expressions/static_type.rs` | tipo statico dell'AST dal solo schema: sorgente unica per kernel e analizzatore |

### Nuovi file di test

`plenora-core`: `cardinalita_zero_colonne.rs`, `panic_policy_processo.rs` ·
`plenora-engine`: `cardinalita_engine.rs`, `catalog_coverage.rs`,
`review_regressions.rs` · `plenora-kernels-table`: `review_regressions.rs` ·
`plenora-cli`: `examples_e2e.rs`, `matrice_cli.rs`.

**Test `#[test]`: da 1 336 a 1 475 (+139).** Il conteggio eseguito e' piu'
alto perche' molti test sono tabellari.

### Documentazione

- **15 report di review** (`docs/review-2…16-*.md`): uno per giro, ciascuno
  con finding → causa → fix → ricerca della classe → regressione, i gate e la
  sezione «che cosa resta NON verificato»;
- `docs/api-breaking-2026-08-16.md`: rotture e comportamenti osservabili,
  aggiornato fino al sedicesimo giro;
- `docs/deroghe.md`: DER-001…DER-011;
- `docs/piano-usabilita.md`, `docs/checkpoint-review-2026-08-16.md`;
- 5 ADR emendati: 0003, 0004, 0006, 0009, 0010;
- `docs/kernel-signatures.md` e `docs/_fragments/*` allineati.

### Esempi

`examples/e1-filtro-ordinamento/` (piano, dati, atteso, README), esercitato
da `plenora-cli/tests/examples_e2e.rs`.

---

## 2. Che cosa e' stato chiuso, per classe

Non un elenco di bug: le classi che i sedici giri hanno chiuso.

| classe | forma del fix |
|---|---|
| **Categoria degli errori non conservata** | ogni errore gia' classificato mantiene la propria categoria attraverso entrambi i propagatori (`error_propagation::categoria_preservata`); `Execution` si costruisce solo da un errore gia' `Execution` |
| **Null logico dei dictionary** | risolutore condiviso: pivot, aggregazione, formula, coalesce, confronti |
| **Cardinalita' a zero colonne** | `plenora_core::batch_with_rows` unico costruttore + presidio strutturale fail-closed sul sorgente |
| **Limiti applicati dopo l'allocazione** | tetti e stime spostati in testa; criterio scritto in ADR-0009; `ResourceLimit` modellato per ~120 conversioni |
| **Predicati copiati** | eliminate quattro copie del profilo scalare testuale; `text_convertible`/`validate_text_convertible` unici; `setops::key_encodable` accanto all'encoder |
| **Nomi di output che collidono** | `resolve_output_names` sequenziale, con riserva e validazione del nome **generato**; `MAX_SUFFISSI_COLLISIONE` |
| **Analizzatore divergente dal runtime** | `expressions::static_type` e `formula::column_formula_type` sono la sorgente unica: l'analizzatore le chiama con lo schema del contratto, il kernel con quello del batch |
| **Tipo dell'output deciso dai valori** | il tipo di `expression`/`formula` si ricava dallo SCHEMA: batch pieni, tutti null e vuoti producono lo stesso schema, ed e' quello del contratto |
| **Presidi che non presidiavano** | macro `categorie_errore!` a sorgente unica; gate di cardinalita' fail-closed; regressione diagnostica che attraversa davvero la correzione |

**Metodo, costante nei sedici giri.** Ogni regressione e' stata verificata
**rimettendo temporaneamente il difetto** e osservandone la caduta. Dove
l'attesa scritta a mano era sbagliata, e' stato il test a correggerla — tre
volte nel quindicesimo giro, una nel quattordicesimo — e la correzione e'
registrata nel report del giro.

---

## 3. Stato dei gate all'ultimo giro

| gate | esito |
|---|---|
| `cargo fmt --all --check` | pulito |
| Clippy `--workspace --all-targets --locked` | zero errori, zero warning |
| R6 anti-panic (cinque crate, `--lib --bins`) | pulito |
| Suite default `--workspace --no-fail-fast` | **1 396 test, 0 falliti** |
| Clippy full-backends | pulito |
| R6 full-backends | pulito |
| Test full-backends | **1 458 test, 0 falliti** |
| Coverage (soglie CI 90 / 85 / 89) | **92.04 / 87.64 / 92.88** |
| Fine riga (`.gitattributes`: `eol=lf`) | nessun CRLF |

Toolchain: `rust:1.92.0-slim` in container, `CARGO_BUILD_JOBS=2` (con la
parallelizzazione predefinita il container esaurisce la memoria in
compilazione).

**Correzione a conteggi precedenti.** I report dall'ottavo all'undicesimo
riportano «992 test full-backends»: era una somma su output **troncato** dalla
lettura, non il totale. Con lo stesso comando il totale e' quello riportato
qui.

---

## 4. Pulizia del worktree

`git add -A` in prova a vuoto seleziona **125 percorsi**, nessuno dei quali e'
un artefatto: `.gitignore` copre `/target`, `target-*/`, `**/*.rs.bk`,
`/.venv-doc`, `/fuzz/artifacts`, `/fuzz/corpus`, `.debug_*.sh`. L'output di
coverage (`target/tmp/lcov.info`) e le directory di build del container
(`target-linux`, `target-cov`, `target-backends`) ricadono tutte li' dentro.

Nessun file temporaneo, log, `.orig`, `.rej` o backup fra i percorsi
selezionati. Peso dei nuovi file: 684 KB, tutto testo.

---

## 5. Commit proposto

**Non eseguito**: la consegna finora e' stata «modifica il worktree, mai
committare», quindi il commit resta da autorizzare.

Il maintainer ha autorizzato il commit e il push diretti sul ramo di default
`main`. Il messaggio di commit e' conservato fuori dal repository: non e'
parte della baseline che descrive.

---

## 6. Rischi che restano aperti

Esplicitamente **non chiusi**, e non chiudibili qui.

| rischio | dove e' tracciato | perche' resta aperto |
|---|---|---|
| **DER-010** — hook di panico non installato d'ufficio per gli embedder | `docs/deroghe.md` | protocollo cooperativo: un embedder che non lo installa non ottiene la politica |
| **DER-011** — `max_memory_bytes` non e' un tetto duro | `docs/deroghe.md` | vale per entrambi i percorsi e per ogni operazione, preflight comprese |
| ~~**Clippy MSVC**~~ | — | **CHIUSO** (correzione del 2026-08-19): il job `test windows-latest` gira con la toolchain nativa su Windows — quindi MSVC — ed esegue sia R6 sia `cargo clippy --workspace --all-targets --locked`. Il rischio era registrato per la sola impossibilita' di riprodurlo dal container Linux, che e' un limite dell'ambiente locale, non della CI. Resta non verificato il solo **full-backends su Windows**, job Linux-only |
| ~~**Campagna fuzz completa**~~ | — | **CHIUSA** (2026-08-19, run 32221206230 su `671214c`): 30 minuti per target, **16 target attivi su 16 verdi**. NON include `arrow_transform`, in quarantena dal 2026-08-07 con motivazione accanto alla matrice: quella quarantena e' il residuo che resta |

Limiti minori, gia' dichiarati nei report dei rispettivi giri e non ripetuti
qui come rischi di rilascio: la permissivita' dichiarata dell'analisi con
`output_type` esplicito, il rifiuto piu' stretto su colonne tutte nulle, il
presidio della cardinalita' come filtro sintattico, le categorie disomogenee
dell'analizzatore, `docs/_build/assemble.py` fuori uso (gia' su `HEAD`), il
difetto diagnostico latente.

**Aggiornamento del 2026-08-19.** MSVC e la campagna fuzz sono chiusi (sopra).
I residui reali sono **DER-010**, **DER-011** e la **quarantena di
`arrow_transform`**: nessuno dei tre blocca l'inizio dell'orchestratore, e
restano tracciati per la dichiarazione di produzione.

---

## 7. Identita' del checkpoint

Questo documento appartiene al commit di stabilizzazione che lo contiene.
Lo SHA autoritativo e' nella history Git e nel resoconto di pubblicazione; non
viene replicato qui per evitare un riferimento auto-referenziale (modificare
il documento cambierebbe lo SHA del commit).
