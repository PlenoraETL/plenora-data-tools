# Rilascio

## Gate

Nessuno di questi è facoltativo. La CI li esegue tutti a ogni PR e a ogni push
su `main`: **tredici job**, e un rosso qualsiasi ferma il rilascio.

| gate | comando |
|---|---|
| formattazione | `cargo fmt --all --check` |
| suite completa | `cargo test --workspace --locked` |
| **anti-panico (R6)** | `cargo clippy -p plenora-core -p plenora-engine -p plenora-kernels-table -p plenora-kernels-geo -p plenora-cli --lib --bins --locked -- -D unsafe-code -D clippy::unwrap_used -D clippy::expect_used -D clippy::panic -D clippy::unreachable -D clippy::todo -D clippy::unimplemented` |
| lint | `cargo clippy --workspace --all-targets --locked` |
| backend nativi | gli stessi, con `--features full-backends`, solo Linux |
| coverage | `cargo llvm-cov report --fail-under-lines 90 --fail-under-functions 85 --fail-under-regions 89` |
| compilazione fuzz | `cargo check --manifest-path fuzz/Cargo.toml --locked` |
| audit supply-chain | `cargo audit` |
| manifesti storici | `python scripts/check_release_manifest.py release/<v>.json --repo .` (uno per manifesto) |
| documentazione generata | `python docs/_build/assemble.py --verify` |
| superficie documentale | `python scripts/verifica_documentazione.py` |
| memoria governata | `python scripts/verifica_memoria_governata.py` |

Due note sul gate anti-panico, entrambe imparate a caro prezzo:

- **mai** aggiungere `--cap-lints=warn`: cappa anche i `-D` espliciti,
  declassandoli a warning. È già successo, e il gate non ha bloccato nulla per
  giorni mentre si accumulavano ventisette siti;
- il perimetro include `plenora-cli` con `--bins`: la CLI è codice di
  produzione a tutti gli effetti — è l'unico eseguibile distribuito — ma non
  ha un target `--lib`, quindi senza `--bins` resterebbe fuori.

I gate di rilascio stanno in **job separati**, non in step dello stesso job:
con controlli in serie il primo fallimento nasconde tutto ciò che segue.

## Riprodurre i gate in locale

La toolchain è pinnata a **1.92.0** da `rust-toolchain.toml`, e ogni comando
usa `--locked`: un `Cargo.lock` modificato dalla build è un fallimento, non un
aggiornamento.

`scripts/coverage.sh` riproduce il verdetto del gate coverage dentro un
container con la stessa toolchain; le soglie sono le stesse dei due punti e se
cambiano vanno cambiate in entrambi.

## Piattaforme

| piattaforma | stato |
|---|---|
| Linux x86_64 | verificata in CI: suite, backend nativi, coverage, fuzz |
| Windows x86_64 | verificata in CI: suite completa |
| macOS | **non verificata**. Non è nella matrice, e il profilo isolato di memoria non sarà supportato lì finché un prototipo non lo dimostri |

Il publish atomico ha rami per piattaforma, ed è la ragione per cui la matrice
non è solo-Linux: un test su una sola piattaforma non eserciterebbe l'altro
ramo.

## Backend

`geos-backend` e `proj-backend` sono feature opzionali, compilate e testate
insieme come `full-backends` **solo su Linux**, che è la piattaforma di
riferimento. Le dipendenze di build sono `cmake`, `sqlite3`, `libsqlite3-dev`.

Senza le feature il comportamento è **fail-closed e dichiarato**: un piano che
richiede la riproiezione fallisce in validazione, invece di produrre un
risultato approssimato. `plenora-data-tools capabilities` riporta che cosa la
build offre davvero.

Fino a quando quel job non è esistito, i rami feature-gated erano verificati
solo in locale, e lint su di essi sono emersi mesi dopo.

## Fuzzing

Workflow separato (`.github/workflows/fuzz.yml`), con un target per voce di
matrice.

Il solo step `cargo fuzz run` gira su toolchain **nightly**, mentre build,
test, clippy e gate anti-panico restano sulla pinnata. È una divergenza
dichiarata: le flag sanitizer non sono disponibili sulla stabile. Un crash
conta **solo se riproducibile sulla pinnata** — riproduzione e minimizzazione
avvengono lì, prima di aprire una correzione — e la deroga va riesaminata a
ogni bump di toolchain.

Il target `arrow_transform` è **disattivato**, con la ragione scritta accanto:
`libfuzzer` chiama `abort()` prima dell'unwinding, quindi il target resterebbe
rosso a barriera funzionante, e un job perennemente rosso smette di essere
letto. Va riattivato quando `arrow-rs` renderà fallibile la conversione dello
schema (`apache/arrow-rs#10575`).

La campagna fuzz completa è prevista alla chiusura del lavoro sulla memoria
governata: vedi [`stato-e-roadmap.md`](stato-e-roadmap.md).

## Documentazione generata

`docs/operazioni.md` è generato dal catalogo e dai frammenti. Il file
committato deve coincidere **byte per byte** con la rigenerazione, e il gate
lo verifica su una copia in una directory temporanea, senza toccare il
worktree.

```bash
python docs/_build/assemble.py            # rigenera
python docs/_build/assemble.py --verify   # confronta, senza scrivere
```

Nessuna dipendenza: solo la libreria standard. La generazione **fallisce** se
il catalogo non è agganciato, se compare una chiave o una variante
sconosciuta, se un frammento è duplicato, se un'operazione è ripetuta nelle
sezioni o se un'operazione catalogata non ha firma. Il dettaglio è in
`docs/_build/RIPRODUCIBILITA.md`.

Non esiste un PDF: era documentazione obsoleta e non riproducibile, ed è stato
ritirato. Un passo di CI verifica che non ricompaia, insieme a qualunque
bytecode Python o artefatto di build tracciato.

## Manifesti di rilascio

`release/*.json` sono i manifesti **storici** (`rc`, `1.0.0`–`1.0.3`),
verificati a ogni CI da `scripts/check_release_manifest.py`, che vive in
questo repository.

Sono documenti **immutabili**: contengono il testo dell'epoca e non vanno
reinterpretati con regole nate dopo di loro. Il repository normativo esterno
citato dai manifesti più vecchi è stato sostituito nel 2026-08-18 e il suo
contenuto attuale è una linea nuova: **non va clonato** e non va usato per
verificarli. I manifesti futuri richiederanno un profilo separato del checker,
costruito sulla struttura nuova.

`check_release_candidate.py` va invocato **solo finché il tag non esiste** —
verifica proprio che il tag previsto non sia ancora stato creato — quindi il
suo step va aggiunto alla CI all'apertura di un candidato e rimosso alla
creazione del tag. È già successo che sopravvivesse al tag e facesse fallire
ogni push su `main`.

## Versione e packaging

La versione è unica per il workspace, in `Cargo.toml`. Oggi: **1.0.3**.

Il prossimo rilascio richiede un **bump maggiore**: la superficie pubblica è
cambiata in modo incompatibile.

| rottura | dettaglio |
|---|---|
| `Limits::max_memory_bytes` → `max_governed_memory_bytes` | in `plenora-core` e in `plenora-kernels-table`; chi costruisce con struct literal non compila |
| versione canonica del piano da 4 a 5 | i piani v4 continuano a funzionare, migrati |
| `PlanV4`, `NodeV4`, `ValidatedPlanV4` → `PlanV5`, `NodeV5`, `ValidatedPlanV5` | e `PLAN_SCHEMA_VERSION_V4` non è più la canonica |
| `plan_hash` in un dominio nuovo | ogni valore precedente è invalidato |
| `check_compatibility` confronta `plan_format_version` | un grafo di un'altra versione del formato è ora rifiutato |
| categoria d'errore dei limiti di piano malformati | `data_mapping` in v4 e in v5, dove prima il percorso v4 dava `invalid_plan`: cambia l'exit code |
| `Inputs::add` / `Inputs::with` deprecate | il percorso senza contratto non è più una forma sostenuta |
| struct di metriche `#[non_exhaustive]` | da qui in avanti un campo nuovo non sarà più una rottura |

L'artefatto distribuito è il binario `plenora-data-tools`. Non esiste ancora
un pacchetto Python: vedi [`stato-e-roadmap.md`](stato-e-roadmap.md).

## Procedura

1. tutti i gate verdi su `main`;
2. lavoro aperto di [`stato-e-roadmap.md`](stato-e-roadmap.md) chiuso fino al
   punto «Release» incluso;
3. campagna fuzz completa eseguita, con `arrow_transform` riattivato o la sua
   quarantena riconfermata per iscritto;
4. bump di versione in `Cargo.toml`, `Cargo.lock` aggiornato con `--locked`
   verde;
5. manifesto del candidato in `release/<versione>.json`, con lo step di
   `check_release_candidate.py` **aggiunto** alla CI;
6. CI completa verde sul candidato;
7. tag, e **rimozione** dello step del candidato dalla CI;
8. CI post-tag verde.
