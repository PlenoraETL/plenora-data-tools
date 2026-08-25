# Rilascio

## Gate

Nessuno di questi è facoltativo. La CI li esegue tutti a ogni PR e a ogni push
su `main`: **quindici job**, e un rosso qualsiasi ferma il rilascio.

| gate | comando |
|---|---|
| formattazione | `cargo fmt --all --check` |
| suite completa | `cargo test --workspace --locked` |
| **anti-panico (R6)** | `cargo clippy -p plenora-core -p plenora-engine -p plenora-kernels-table -p plenora-kernels-geo -p plenora-cli --lib --bins --locked -- -D unsafe-code -D clippy::unwrap_used -D clippy::expect_used -D clippy::panic -D clippy::unreachable -D clippy::todo -D clippy::unimplemented` |
| **assert nel codice di produzione** | `python scripts/verifica_assenza_assert.py` |
| **pin delle action** | `python scripts/verifica_pin_workflow.py` |
| lint | `cargo clippy --workspace --all-targets --locked` |
| backend nativi | gli stessi, con `--features full-backends`, solo Linux |
| coverage | `cargo llvm-cov report --fail-under-lines 90 --fail-under-functions 85 --fail-under-regions 89` |
| compilazione fuzz | `cargo check --manifest-path fuzz/Cargo.toml --locked` |
| audit supply-chain | `cargo audit` |
| manifesti storici | `python scripts/check_release_manifest.py release/<v>.json --repo .` (uno per manifesto) |
| documentazione generata | `python docs/_build/assemble.py --verify` |
| superficie documentale | `python scripts/verifica_documentazione.py` |
| memoria governata | `python scripts/verifica_memoria_governata.py` |

Tre note sul gate anti-panico, tutte imparate a caro prezzo:

- **mai** aggiungere `--cap-lints=warn`: cappa anche i `-D` espliciti,
  declassandoli a warning. È già successo, e il gate non ha bloccato nulla per
  giorni mentre si accumulavano ventisette siti;
- il perimetro include `plenora-cli` con `--bins`: la CLI è codice di
  produzione a tutti gli effetti — è l'unico eseguibile distribuito — ma non
  ha un target `--lib`, quindi senza `--bins` resterebbe fuori;
- clippy non ha un lint per `assert!`, `assert_eq!`, `assert_ne!` e le
  varianti `debug_assert*`, che sono primitive di panico quanto le altre.
  Servono due gate, non uno: il secondo è una scansione dei sorgenti
  (`scripts/verifica_assenza_assert.py`) con lo stesso perimetro, e si
  autoverifica iniettando mutazioni. Senza, la promessa era più larga del
  controllo che avrebbe dovuto imporla.

I gate di rilascio stanno in **job separati**, non in step dello stesso job:
con controlli in serie il primo fallimento nasconde tutto ciò che segue.

## Riproducibilità della CI

Lo stesso commit dev'essere verificato dallo **stesso codice**, non solo dalla
stessa toolchain. Ogni `uses:` dei workflow riferisce quindi una **SHA
completa** con un commento che ne dice la versione; un tag come `v4` o un ramo
come `1.98.0` è mobile, e chi lo controlla può spostarlo fra due esecuzioni
della stessa PR. Con la SHA il ramo non seleziona più la toolchain, che va
perciò dichiarata esplicitamente nell'input `toolchain:`.

`scripts/verifica_pin_workflow.py` è il gate che tiene la regola: rifiuta i
riferimenti mobili, le SHA abbreviate — ambigue per costruzione — e le SHA
senza commento di versione, che diventano illeggibili e quindi non si
aggiornano più. Anche gli strumenti installati con `cargo install` sono
pinnati a una versione esatta (`cargo-llvm-cov`, `cargo-audit`, `cargo-fuzz`),
e il fuzzing usa una **nightly datata** invece del canale mobile.

## Riprodurre i gate in locale

La toolchain è pinnata a **1.98.0** da `rust-toolchain.toml`, e ogni comando
usa `--locked`: un `Cargo.lock` modificato dalla build è un fallimento, non un
aggiornamento.

`scripts/coverage.sh` riproduce il verdetto del gate coverage dentro un
container con la stessa toolchain; le soglie sono le stesse dei due punti e se
cambiano vanno cambiate in entrambi.

## Baseline di compilazione: analisi di impatto

`rust-toolchain.toml` dichiara che ogni variazione della baseline richiede una
change impact analysis (R13.5). Questa è quella della revisione corrente.

### Toolchain

Da **1.92.0** a **1.98.0**. `cargo fmt` non ha prodotto differenze e il codice
ha compilato invariato: l'unico effetto sono stati **lint nuovi**, perché il
workspace tiene `clippy::all`, `pedantic` e `nursery` a `deny`. Quattordici
sono stati risolti riscrivendo il codice; tre sono stati **rifiutati con
motivazione** — due proposte di `mul_add`, che cambierebbe il risultato in
virgola mobile e quindi il determinismo, e sono marcate `#[allow]` con il
perché accanto.

Un canale che alza le lint a `deny` fa fallire la build su codice corretto: è
un costo previsto e voluto, non una sorpresa. Va messo in conto a ogni bump.

### Dipendenze dirette

| crate | da | a | note |
|---|---|---|---|
| `arrow-array` / `-schema` / `-ipc` / `-select` | 59.1.0 | 59.2.0 | vedi impatto sotto |
| `thiserror` | 2.0.19 | 2.0.20 | |
| `chrono` | 0.4.42 | 0.4.45 | |
| `regex` | 1.12.2 | 1.13.1 | |
| `uuid` | 1.18.1 | 1.25.0 | |
| `proptest` | 1.10.0 | 1.11.0 | solo test |
| `md-5` | 0.10.6 | 0.11.0 | `digest` 0.11 |
| `sha2` | 0.10.9 | 0.11.0 | `digest` 0.11 |

Nessuno di questi aggiornamenti è forzato da una vulnerabilità: `cargo audit`
era pulito **prima** di applicarli, su 187 dipendenze del workspace e 152 del
progetto fuzz. Sono manutenzione, e la conseguenza pratica è che qualunque di
essi può essere annullato senza aprire un buco.

### Impatto: `arrow_version` invalida i grafi già validati

La versione di Arrow entra nell'identità di `ValidatedGraph`, e
`check_compatibility` la confronta: un grafo validato sotto 59.1.0 viene ora
respinto con `GRAPH_MISMATCH`. È la semantica voluta — la stessa di un bump di
`engine_version` — e va **prevista da chi conserva grafi validati fra le
esecuzioni**: vanno rivalidati, non riusati.

Il `plan_hash` **non** è toccato: è `SHA256(dominio ‖ canonical_json)` e non
include la versione di Arrow. I piani mantengono la loro identità.

### Impatto: `digest` 0.11 cambia i tipi, non i byte

`digest` 0.11 sostituisce `generic-array` con `hybrid-array`, e il valore
restituito da `finalize()` non implementa più `LowerHex`. I due siti che
formattavano un digest con `{:x}` usano ora `push_hex`, già presente e già
documentato come equivalente byte a byte.

MD5 e SHA-256 sono algoritmi fissi, quindi l'output non poteva cambiare; ma
«non poteva» non è una verifica. La verifica sono i **valori d'oro** già
presenti nella suite — fra cui `d41d8cd98f00b204e9800998ecf8427e`, l'MD5 noto
della stringa vuota — che sono indipendenti da qualunque libreria e passano.

### Sospeso: `rstar` resta a 0.12.2

`rstar` 0.13.0 esiste, ma **nessuna versione pubblicata di `geo` lo supporta**:
anche l'ultima, la 0.33.1 che già usiamo, dichiara `rstar = "0.12.0"` come
dipendenza **obbligatoria** — non opzionale, non dietro una feature. Non c'è
un interruttore da spegnere.

`geo-types` è invece già pronto: usa dipendenze opzionali rinominate
(`package = "rstar"`) per offrire una feature per versione, e la 0.7.20 ha
aggiunto `rstar_0_13`. Non ci aiuta finché `geo` impone la 0.12, perché
l'albero conterrebbe comunque entrambe.

Aggiornare solo il nostro pin farebbe quindi convivere due implementazioni di
R-tree nello stesso binario, in una libreria che ha un contratto di
determinismo sui join spaziali, senza alcun beneficio in cambio.

Condizione di rientro: `geo` pubblica una versione che dipende da `rstar`
0.13. La feature già presente in `geo-types` dice che l'ecosistema si sta
muovendo in quella direzione; quando succede, si aggiornano insieme.

### Fuori dall'analisi: `benchmarks/baseline/`

Quei file **non** sono stati aggiornati, ed è deliberato: registrano una
misura presa il 2026-07-24, con la toolchain e le dipendenze di allora.
Riscriverne le versioni renderebbe falso il registro. L'harness resta pinnato
alle versioni di quel giorno perché è la riproduzione congelata della misura;
il confronto con i numeri attuali è un'attività separata, che deve dichiarare
di stare confrontando due ambienti diversi.

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

**I profili supportati sono due: `default` e `full-backends`.** Le
combinazioni parziali — solo `geos-backend`, solo `proj-backend` — si
compilano, ma non sono esercitate dal CI e non sono profili supportati.
L'oracolo della superficie CLI ha comunque un'aspettativa esplicita per
ciascuna delle quattro combinazioni: una build parziale non deve produrre un
verde falso solo perché nessuno l'ha prevista.

Che `capabilities` dichiari i backend ha una conseguenza sui test: la
superficie CLI **dipende dalle feature**. Un oracolo che la fissa una volta
sola è vero per un profilo e falso per l'altro, ed è esattamente l'errore che
ha reso rosso il job `full-backends`
([`stato-e-roadmap.md`](stato-e-roadmap.md)). L'oracolo è perciò un modello
con un marcatore, non un'istantanea.

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

## Prestazioni

**Che cosa esiste.** Una baseline di riferimento in
`benchmarks/baseline/baseline.json`, con le misure grezze in
`benchmarks/baseline/raw/` e gli harness che le producono in
`benchmarks/baseline/harness/`; gli sweep per famiglia di kernel in
`benchmarks/sweep/*.json`; e gli esempi `bench_*` dei crate, che si eseguono a
mano e stampano una riga JSON per scenario.

**Che cosa non esiste.** Non c'è un gate prestazionale: nessun job di CI e
nessun checker di rilascio legge quei file, e nessuna soglia è applicata da
qualcosa. Non ci sono soglie definitive — i numeri in giro per il codice sono
citati come riferimento, mai come limite — né un runner controllato su cui
misurare: le esecuzioni note vengono da container su host di sviluppo, con la
variabilità che ne consegue.

Di conseguenza un numero, da solo, **non è un verdetto**: dipende dall'host,
dal kernel e dal carico. Serve a confrontare due esecuzioni fatte nello stesso
posto, non a decidere se una versione è abbastanza veloce. La qualifica
prestazionale — matrice, ambiente e soglie — è lavoro dichiarato aperto in
[`stato-e-roadmap.md`](stato-e-roadmap.md).

## Documentazione generata

`docs/operazioni.md` è generato dal catalogo e dai frammenti. Il file
committato deve coincidere **byte per byte** con la rigenerazione, e il gate
lo verifica su una copia in una directory temporanea, senza toccare il
worktree.

```bash
python docs/_build/assemble.py            # rigenera
python docs/_build/assemble.py --verify   # confronta, senza scrivere
```

Nessuna dipendenza: solo la libreria standard. Nessun pacchetto da
installare, nessun container, nessuno stato della macchina. Il confronto è in
**byte** e non in testo di proposito: su Windows le funzioni di lettura e
scrittura testuale traducono le fine riga in entrambi i versi, e un confronto
testuale passerebbe su un file diverso da quello prodotto su Linux.

La generazione **fallisce**, invece di produrre un documento incompleto che
sembra completo, quando:

| condizione | che cosa sarebbe sparito |
|---|---|
| **zero operazioni** lette dal catalogo | tutto: non è un catalogo vuoto, è il parser che non aggancia più |
| chiave opzionale di `op!` **sconosciuta** | ciò che l'operazione dichiara e il documento non riporterebbe |
| variante `CrsRequirement` **sconosciuta** | un requisito CRS reale, omesso dalla firma |
| **frammento duplicato** (stesso id in due file) | una delle due firme, senza che si sappia quale |
| **operazione ripetuta** nelle sezioni | niente, ma i conteggi dell'intestazione sarebbero falsi |
| operazione documentata **assente dal catalogo**, o senza frammento | il documento parlerebbe di ciò che non esiste |
| **operazione catalogata senza firma** | la copertura totale dichiarata nell'intestazione |

Le regressioni girano a **ogni invocazione** dello script, non in una suite
che qualcuno deve ricordarsi di lanciare: costano microsecondi, e il guasto
che le ha motivate era del tipo che nessuno va a cercare. Coprono la macro su
una riga e su più righe in forma `rustfmt`, l'annidamento, il catalogo vuoto,
la chiave e la variante sconosciute, i frammenti duplicati e le sezioni
ripetute. Sono **negative** dove conta: passano al controllo una struttura
costruita apposta e pretendono il rifiuto.

Perché tutto questo serviva: il generatore è rimasto **rotto per mesi in
silenzio**. `rustfmt` aveva riformattato la macro `op!` del catalogo, la regex
del parser aveva smesso di agganciarla, e lo script usciva senza riscrivere
nulla. Un generatore che si ferma non rompe niente di visibile — il documento
resta com'era e sembra aggiornato. Solo un confronto lo scopre.

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
| i limiti dichiarati in `limits.plan` governano davvero il piano | un piano che li dichiarava più larghi della policy di chi esegue, o che li dichiarava e poi li violava, era accettato e ora è rifiutato: vedi [`piano-v5.md`](piano-v5.md) |
| errori sanitizzati di arrow, GEOS e della trasformazione PROJ | il testo della dipendenza non attraversa più il confine: chi confrontava quei messaggi non li ritrova |
| cinque varianti nuove in `ArrowTransportError` | `IpcSchemaInvalid`, `IpcMetadataInvalid` e i tre tetti sui custom metadata: chi faceva un `match` esaustivo su quell'enum non compila più. Comprimerle in una variante generica avrebbe reso i tre tetti indistinguibili, e un test non potrebbe dire quale ha parato |
| `ArrowTransportError` `#[non_exhaustive]` | da qui in avanti una variante nuova non sarà più una rottura: chi vi fa `match` deve prevedere un ramo generico |
| `with_phase` su un errore con diagnostica di riga | produce ora la forma canonica `RowDiagnostics → Tagged → causa` invece di `Tagged → RowDiagnostics → …`: `Display`, categoria, fase, effetto, retry, contesto e payload sono invariati, ma **cambiano il pattern matching e la catena `Error::source()`** di chi osserva la struttura. Ripristina il contratto già documentato — primo tag vince, nessun annidamento — che l'implementazione violava appena c'era un wrapper di mezzo |
| sei varianti nuove in `PlenoraError` | `Protocol`, `Timeout`, `Conflict`, `InvalidConfiguration`, `IsolationUnavailable` e `UnattributedMemoryPressure`: chi faceva un `match` esaustivo su quell'enum **non compila più**. Le prime quattro producono categorie che erano già dichiarate e che nessuna variante sapeva produrre — i loro exit code esistevano ed erano irraggiungibili |
| due categorie nuove in `ErrorCategory` | `isolation_unavailable` e `unattributed_memory_pressure`, **estensioni locali** e non canone: un componente gemello non le riconosce, e la deviazione è registrata in [`errori-e-limiti.md`](errori-e-limiti.md). Per questo il costruttore inverso si chiama `ErrorCategory::from_stable_name` e non `from_canonical`: accetta anche le due locali, e il nome non deve promettere che ogni stringa riconosciuta appartenga al canone congelato. Per lo stesso motivo `ErrorCategory::ALL` è l'elenco delle categorie **supportate**, non «canoniche». Entrambe proiettate su exit code `5`. Nessun exit code nuovo è stato introdotto: la distinzione fine resta in `error.category`, che è machine-readable. Chi fa `match` su `ErrorCategory` non compila più |
| `EvidenzaDiLimite` è un tipo pubblico | accompagna `UnattributedMemoryPressure` e porta i **cinque segnali** della §10.0-bis di [`isolamento.md`](isolamento.md) — `Ol`, `Kl`, `Kh`, `G` e `Oa` — con tetto, picco e `memory.events:max` tenuti a parte in `DiagnosticaSupplementare`, perché non fondano attribuzione. I contatori sono `Option<u64>` perché **`None` non è zero**: un contatore non osservabile letto come «nessun evento» è la conclusione opposta a quella vera, e per `G` — il solo segnale causale — significherebbe non attribuire mai oppure attribuire su un contatore mai visto |
| `PressioneDegliAntenati` ha **campi privati** | si costruisce con `nuova`, che verifica la forma canonica e **rifiuta** l'incoerente invece di normalizzarla; si legge con `livello`, `livelli_presenti` e `antenati_oltre_capacita`. Nessuno `struct literal` esterno, quindi `MAX_ANTENATI_OSSERVATI` può cambiare senza rompere chi costruisce, e l'array non diventa contratto |
| `LivelloAntenato` distingue quattro casi | `Inesistente`, `NonOsservato`, `Osservato(delta)` e `ProfonditaNonStabilita`. Un `Option<u64>` ne confondeva due — un livello che non c'è perché la radice è più vicina, e un livello che c'è e non si è letto — e la confusione avrebbe fatto dire a `Oa` che la gerarchia finisce dove invece finisce la nostra vista |
| `Oa` identifica l'antenato per **distanza** | non per percorso: il percorso sarebbe di lunghezza non limitata e porterebbe fuori dal confine la disposizione del filesystem dell'host. Il tetto di otto livelli è registrato in [`errori-e-limiti.md`](errori-e-limiti.md) con regola, perimetro, pericolo e condizione di rientro |
| proiezione categoria → exit code tipizzata | `exit_code_di` fa `match` esaustivo su `ErrorCategory` invece di confrontare stringhe: una categoria nuova senza decisione **ferma la compilazione**. Il ripiego su `70` resta solo per envelope che portano stringhe non riconosciute. Nessun exit code esistente cambia |
| `protocol` mancava dalla tabella degli exit code | il codice lo proiettava su `5` già prima; [`cli.md`](cli.md) non lo elencava. Cambia la **documentazione**, non il comportamento: chi si era regolato sulla tabella invece che sull'osservazione trovava un `5` che non si aspettava |
| input Arrow IPC che oggi passano e domani no | i custom metadata entrano nel confine ostile: coppie oltre i tetti, chiave o valore assenti, chiave vuota, UTF-8 non valido, chiavi duplicate. Un file **Arrow valido** può essere rifiutato di proposito ([`errori-e-limiti.md`](errori-e-limiti.md)) |

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
