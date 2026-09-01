# Rilascio

## Gate

Nessuno di questi è facoltativo. La CI li esegue tutti a ogni PR e a ogni push
su `main`: **sedici job**, e un rosso qualsiasi ferma il rilascio.

| gate | comando |
|---|---|
| formattazione | `cargo fmt --all --check` |
| suite completa | `cargo test --workspace --locked` |
| **anti-panico (R6)** | `cargo clippy -p plenora-core -p plenora-engine -p plenora-kernels-table -p plenora-kernels-geo -p plenora-cli --lib --bins --locked -- -D unsafe-code -D clippy::unwrap_used -D clippy::expect_used -D clippy::panic -D clippy::unreachable -D clippy::todo -D clippy::unimplemented` |
| **assert nel codice di produzione** | `python scripts/verifica_assenza_assert.py` |
| **pin delle action** | `python scripts/verifica_pin_workflow.py` |
| **commenti al presente** | `python scripts/verifica_commenti.py` |
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

## Superficie pubblica e formato dell'artefatto

### `CommitToken`: due nomi, non sei

`plenora-engine` esporta `CommitToken` e `FormaTokenNonValida`, e nient'altro
del modulo che li definisce. Il modulo è **privato**: un `pub mod` più il
re-export avrebbe dato due percorsi per la stessa cosa e, con essi, costanti
che a un consumatore non servono: `CHIAVE_FOOTER_COMMIT_TOKEN` è il nome di
una chiave che scriviamo noi, e la lunghezza della forma canonica è un
dettaglio della rappresentazione interna.

Ciò che il chiamante deve poter fare è costruire un token e ricevere il
rifiuto motivato quando il testo non è canonico. Due nomi bastano, e ogni nome
in più è una promessa in più.

### Il footer dell'artefatto può portare una chiave in più

Gli artefatti Arrow IPC prodotti dal percorso isolato portano nel footer la
chiave `plenora.commit.token`. È un **cambiamento del formato**, e va detto
anche se è additivo:

- gli artefatti prodotti in-process **non** la portano, e i loro byte restano
  identici a quelli delle versioni precedenti — con `None` non si scrive
  nulla, nemmeno una chiave vuota, ed è provato;
- un lettore che ignori le chiavi sconosciute non nota la differenza: il
  confine valida la forma, non il vocabolario;
- un lettore che *cerchi* quella chiave deve accettare che sia assente, perché
  per un artefatto ordinario lo è.

La regola completa — forma canonica, riservatezza del valore, quattro forme
del footer — è in
[errori-e-limiti.md](errori-e-limiti.md#il-commit-token-forma-canonica-unica-e-valore-mai-mostrato).

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

### Quando gira che cosa

Il fuzzing non è un gate unico: è una scala, e ogni gradino sta dove il suo
costo è sostenibile. Metterlo tutto prima del commit lo renderebbe una tassa
che si impara ad aggirare; metterlo tutto dopo il merge lascerebbe entrare
difetti che una corsa breve avrebbe visto.

| Momento | Che cosa è obbligatorio |
| --- | --- |
| **Prima del commit** | i gate deterministici, suite completa del workspace inclusa |
| **Prima del merge** | test mirati, coverage, **compilazione** del crate `fuzz/` e **smoke dei soli target coinvolti** dalla modifica |
| **Dopo il merge** | campagna completa — **30 minuti per target**, il default di `fuzz.yml` — in parallelo alla PR successiva |
| **Obbligatoria e lunga** | **almeno 1 ora per target**, il default di `fuzz-campaign.sh`: **una sola scadenza ordinaria per ciclo di rilascio**, dopo **`PR-12`**, sul candidato congelato, su VM qualificata e con watchdog esterno |

Le due durate non sono scelte qui: sono i default degli strumenti che le
eseguono (`minutes_per_target: 30` in `.github/workflows/fuzz.yml`,
`FUZZ_HOURS_PER_TARGET=1` in `scripts/fuzz-campaign.sh`). Scriverne altre
significherebbe avere due numeri per la stessa cosa, e uno dei due sarebbe
falso.

#### Una scadenza per ciclo, e perché dopo `PR-12`

Una campagna lunga misura l'albero su cui gira. Ripeterla a ogni tappa
significa spendere ore su alberi **destinati a cambiare**, e nessuna di quelle
ore dice qualcosa su ciò che verrà rilasciato: la sola campagna che parla del
prodotto è quella che gira sul codice congelato.

La garanzia non è tolta, è **spostata**: nessun rilascio esce senza una
campagna lunga completa eseguita esattamente sul codice che si rilascia. Ciò
che sparisce è la ripetizione su alberi intermedi, non la copertura.

Quelle tappe non restano scoperte. Fra un commit e il rilascio agiscono i tre
gradini sopra — gate deterministici e suite completa prima del commit, smoke
dei target coinvolti prima del merge, campagna da 30 minuti dopo — e il
gradino lungo arriva quando il bersaglio smette di muoversi.

**«Una» conta le scadenze, non le esecuzioni.** L'esito vale per l'albero su
cui è stato ottenuto, e **qualunque modifica al candidato lo invalida**: una
campagna che trova un difetto obbliga a una correzione, e quella correzione
produce un albero nuovo, da rifuzzare per intero. Non esiste un esito
ereditato da un albero precedente, e non esiste una ripetizione parziale sui
soli target «interessati» dalla correzione. Il ciclo si chiude quando una
campagna completa passa su un candidato che da quel momento non cambia più.

`PR-13`, se sarà realizzata dopo il rilascio, appartiene al **ciclo
successivo** e porta la scadenza di quel ciclo: la sua campagna lunga precede
la **sua** release, non questa.

#### Le due condizioni, e perché sono nella regola

La campagna lunga vale **solo** su una VM qualificata e con un watchdog
esterno al processo. Non sono raccomandazioni: senza, una campagna può
apparire in corso per ore senza esserlo.

I due limiti che il fuzzer si porta dentro non bastano.
`-max_total_time` è valutato **dentro** il ciclo di fuzzing: se il ciclo non
gira, il limite non viene mai guardato. `-timeout` arriva come **segnale**, e
un task in stato `D` non lo gestisce: il segnale resta **pendente** finché
dura il blocco. Un processo fermo fuori dal ciclo sfugge a entrambi, e la
campagna resta immobile senza dichiararsi tale.

**Il watchdog arresta a una sola condizione: il muro di tempo**, cioè il
limite per target più la grazia. Nient'altro.

La regola è volutamente povera. La tentazione è di riconoscere il blocco
mentre accade — poca CPU, un task in stato `D`, uno stack fermo in una `mmap`
— e fermare prima. Ma quella non è una regola esatta: «poca CPU» non ha una
soglia, una fotografia coglie un istante e non dimostra che quell'istante
duri, e `mmap` descrive i blocchi **osservati finora**, non tutti quelli
possibili. Un criterio così arresta anche run sani, e li archivia come
blocchi: l'errore peggiore, perché produce un esito falso invece di
un'assenza di esito.

Il silenzio del log, in particolare, non è un battito mancato. `libFuzzer`
stampa `pulse` a potenze di due del numero di ingressi, e `NEW`/`REDUCE` solo
quando scopre copertura: un target sano e già maturo può tacere a lungo, e
tanto più a lungo quanto più ha girato, perché la distanza fra due potenze di
due cresce con esse.

Il silenzio serve lo stesso, ma per un'altra cosa: **innesca la fotografia
diagnostica**. Stack, stato dei task, consumo, pressioni. Costa nulla, e un
blocco non fotografato è un blocco perso — mentre un blocco fotografato e
lasciato correre fino al muro costa solo tempo di macchina.

#### I tre esiti di un target

Alla scadenza si tirano le somme, e gli esiti possibili sono **tre**. Non due:
un'uscita non-zero non è la stessa cosa in tutti i casi, e trattarla come
un'unica categoria cancellerebbe proprio il risultato che il fuzzing esiste
per produrre.

| esito | quando | che cosa comporta |
|---|---|---|
| **riuscita** | durata richiesta completata, uscita **autonoma** con `rc=0`, **zero** artefatti | il target è qualificato |
| **rilievo** | crash, diagnosi del sanitizer o di `libFuzzer`, oppure un artefatto scritto | **blocca la release** e apre riproduzione e minimizzazione |
| **incompleta** | arresto al muro, interruzione infrastrutturale, o uscita prematura non classificabile | il target **non** è qualificato, e va rieseguito |

La distinzione che conta è fra le ultime due. Un crash è una campagna
**conclusa con un risultato**: ha trovato qualcosa, e quel qualcosa vale più
di un'ora di verde. Un arresto al muro è una campagna **che non ha finito di
guardare**, e non dice niente né in un senso né nell'altro. Archiviare la
prima come la seconda vorrebbe dire buttare via un difetto.

**Il codice d'uscita da solo non distingue le tre categorie**, in nessuna
direzione. Un artefatto vale come rilievo **anche con `rc=0`**: il target può
aver scritto l'ingresso che lo fa cadere e poi essere uscito ordinatamente.
Un'uscita prematura **con `rc=0`** può essere incompleta: un processo che
termina pulito ma prima della durata richiesta non ha guardato quanto doveva.
E un'uscita non-zero può essere l'una o l'altra.

Quello che decide sono i **fatti del run**: l'artefatto, la diagnosi, la
durata effettiva. Il codice d'uscita è uno dei dati, non il criterio.

Fra questi, l'artefatto è il più affidabile e va guardato **per primo**: un
target che ha scritto un artefatto è un rilievo anche se il processo è poi
morto in modo confuso. Ma va attribuito **a questo run** — vedi il criterio
aperto in [`stato-e-roadmap.md`](stato-e-roadmap.md) — perché una directory
che conserva l'artefatto di ieri produrrebbe un rilievo che nessuno ha
trovato oggi.

Una campagna incompleta va registrata per quello che è: **eseguita in parte,
non completata**. Ha girato, ha prodotto corpus, log ed evidenza, e quel
materiale si conserva — ma **non vale come qualificazione**, perché la
qualificazione è la copertura dell'ora intera, non l'essere partiti.

#### Che cosa rende qualificata una VM

Non basta che l'host non abbia mostrato blocchi non attribuiti: quel criterio
è **necessario e non sufficiente**, e da solo qualificherebbe a vuoto una
macchina mai usata, dove l'assenza di blocchi non è un fatto ma una mancanza
di osservazioni.

Servono anche i dati dell'ambiente — configurazione, versione del kernel,
risorse assegnate, ed **esclusività**: che nient'altro di significativo giri
sulla macchina durante la campagna — registrati insieme all'esito e verificati
dallo strumento, non attestati a mano. Il come sta nel criterio aperto in
[`stato-e-roadmap.md`](stato-e-roadmap.md).

«Target coinvolti» si legge dal codice toccato, non dal nome della PR: chi
modifica il decoder del protocollo passa da `protocollo_frame` anche se la PR
parla d'altro.

Un difetto trovato da una campagna **apre una correzione bloccante per la
release**, non una voce di arretrato. Se una campagna lunga trova qualcosa
dopo il merge, la release aspetta quella fix: il momento in cui il difetto è
emerso non cambia che cosa sarebbe successo in produzione.

#### Rilievi aperti

Qui stanno i difetti che una campagna ha trovato e che nessuno ha ancora
chiuso. Stanno **in questo documento e non in un arretrato** perché il loro
effetto è su di esso: finché una voce è qui, la release non parte.

##### `wkt_operations` — panic dentro `geo` su un poligono degenere

Trovato il **2026-09-01** dallo smoke fuzz, su Ubuntu 24.04.4 con kernel
`6.8.0-138-generic`. Diciannove target su venti verdi.

```
panicked at geo-0.33.1/src/algorithm/simplify.rs:108:5
assertion `left != right` failed — left: 0, right: 0
```

L'ingresso è un WKT `polyGon((2444…4444` con coordinate separate dai byte di
ritorno a capo, avanzamento riga e tabulazione — `\r`, `\n` e `\t`. Il parser lo accetta, ne esce una geometria degenere, e `simplify` di
`geo` ci inciampa con un'`assert_ne!`.

È un **panic di una dipendenza raggiungibile da ingresso non fidato**: il
processo muore, non restituisce un errore. Riproduzione deterministica,
verificata rieseguendo l'artefatto sull'ingresso salvato.

Il difetto **non appartiene a `PR-7`**, e va detto perché è durante la sua
qualificazione che è emerso: il percorso è `wkt_operations` →
`plenora-kernels-geo` → `geo 0.33.1`, e il diff di `PR-7` non tocca nessuno dei
tre né `Cargo.lock`. Lo smoke che questo documento richiede prima del merge è
quello **dei target coinvolti dalla modifica**, e `wkt_operations` non è fra
quelli: è stato eseguito lo smoke completo, ed è così che il difetto si è visto.

Le due strade da valutare, e nessuna è ovvia: respingere la geometria degenere
**prima** di passarla a `geo`, oppure trattarlo come difetto della dipendenza —
un `simplify` che va in panic su un ingresso che il proprio parser ha accettato
non è un contratto che il chiamante può rispettare.

L'artefatto e la diagnosi non stanno nel repository: `fuzz/artifacts` è in
`.gitignore`, e un ingresso che fa cadere il processo non è un file da
distribuire con il codice. Stanno con l'evidenza della campagna che li ha
prodotti.

### Preparare l'ambiente locale

`fuzz-smoke.sh` e `fuzz-campaign.sh` girano in un'immagine
(`plenora-rust:nightly-fuzz`) e montano `cargo-fuzz` da una cartella dell'host
(`FUZZBIN_HOST`, default `$HOME/.plenora-fuzz/bin`). Nessuno dei due nasce da
solo: si costruiscono una volta, con la configurazione **del workflow** e non
con una variante locale — stessa nightly datata, stessa versione di
`cargo-fuzz`.

```sh
# 1. L'immagine: la pinnata piu' la nightly datata di fuzz.yml. Nessun
#    componente in piu': il workflow non ne chiede, e chiederne uno che quella
#    nightly non ha fa fallire la build senza dire perche'.
printf 'FROM rust:1.98\nRUN rustup toolchain install nightly-2026-08-01\n\
ENV RUSTUP_TOOLCHAIN=nightly-2026-08-01\n' \
  | docker build -t plenora-rust:nightly-fuzz -
# 2. `cargo-fuzz` dove gli script lo montano. `--root` scrive in `bin/`, ed e'
#    quel `bin/` che viene montato: il binario deve trovarsi in
#    `/fuzzbin/cargo-fuzz`, non in `/fuzzbin/bin/cargo-fuzz`.
#    `MSYS_NO_PATHCONV=1` serve su Git Bash per Windows, che altrimenti
#    riscrive `/out` in un percorso Windows dentro l'argomento di `-v`; sugli
#    altri sistemi e' una variabile che nessuno legge, quindi si lascia.
MSYS_NO_PATHCONV=1 docker run --rm -v "$HOME/.plenora-fuzz:/out" \
  plenora-rust:nightly-fuzz \
  cargo install cargo-fuzz --version 0.13.2 --locked --root /out
# 3. Lo smoke dei soli target coinvolti.
FUZZ_TARGETS=protocollo_frame scripts/fuzz-smoke.sh
```

Se qualcosa manca, gli script **lo dicono e si fermano prima di partire**
(`scripts/fuzz-preflight.sh`, condiviso fra smoke e campagna). Non è cortesia:
senza quel controllo Docker fallisce con «pull access denied for
plenora-rust» — che manda a cercare credenziali per un'immagine che non è su
nessun registry — o con un mount vuoto, e in una campagna da ore lo si scopre
la mattina dopo.

I casi sono tre e vanno **distinti**, perché una diagnosi sbagliata costa più
di nessuna diagnosi:

- **il daemon non risponde.** Si controlla per primo: `docker image inspect`
  fallisce allo stesso modo se l'immagine non c'è e se Docker è spento, quindi
  senza questo controllo un Docker Desktop chiuso veniva riportato come
  «immagine mancante» e mandava a ricostruire un'immagine che c'era già;
- **l'immagine non c'è.** Ora che il daemon risponde, l'assenza è assenza;
- **il binario non è eseguibile.** Si prova quello **configurato**
  (`FUZZ_CARGO_FUZZ`, default `/fuzzbin/cargo-fuzz`) dentro il container e con
  lo stesso mount del run vero, non un percorso sull'host: chi punta a un
  binario dell'immagine non ha bisogno di alcun mount e non va rifiutato, e
  chi ne monta uno con un altro nome non va approvato guardando altrove.
  `--version` dimostra insieme presenza ed eseguibilità — un binario per
  l'architettura sbagliata esiste e non parte.

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

La campagna lunga ha **una scadenza per ciclo di rilascio**, dopo `PR-12`, sul
candidato congelato: la tabella qui sopra è l'unico posto in cui quella
scadenza è scritta. Il lavoro sulla memoria governata resta il contesto in cui
è maturata, ed è descritto in [`stato-e-roadmap.md`](stato-e-roadmap.md).

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
| **formato del piano v6** | `schema_version: 6` è un formato **distinto**, non la v5 con un campo in più. Le v4 e v5 restano accettate esattamente come prima. Il campo `max_domain_memory_bytes` esiste solo in v6: un v4 o v5 che lo dichiara è rifiutato, perché `deny_unknown_fields` non conosce quel nome. Non esiste migrazione v5→v6, e non è una svista — cambierebbe l'identità dei documenti migrati, e va progettata con chi consuma gli hash |
| **dominio nuovo del `plan_hash`** | un piano v6 hasha in `plenora/plan_hash/v6`, distinto da quello della v5. Un v5 e un v6 per il resto identici hanno perciò identità **diverse**. Da non generalizzare: la v4 è migrata NEL canonico v5 e continua a condividerne il `plan_hash`, e le regressioni della migrazione lo verificano |
| `ValidatedGraph::plan()` rende `&PianoValidato` | era `&ValidatedPlanV5`: chi lo chiama non compila più. Non c'era alternativa che non mentisse — un grafo costruito da un piano v6 avrebbe reso come v5 un documento che v5 non è, e chi ne avesse ricavato la forma canonica avrebbe ottenuto un `plan_hash` del dominio sbagliato. Gli accessori del contenitore sono **per campo** (`inputs()`, `output()`, `nodes()`, `crs()`, `crs_decisions()`), per la stessa ragione |
| `PianoValidato` è `#[non_exhaustive]` | dalla nascita: una v7 non deve essere un'altra rottura per chi vi fa `match` da fuori |
| `PlanV6` e `LimitsOverrideV6` sono pubblici e **serializzabili** | tipi paralleli alla v5, non di sola lettura: chi costruisce un v6 a mano deve poterlo emettere. Gli `skip_serializing_if` sono gli stessi della v5, quindi un limite assente non compare invece di diventare `null`, e il giro `serializza → deserializza` rende lo stesso documento |
| `schema_version` negli output CLI | `validate` e `run` la dichiaravano **fissata** a 5. Ora la dice il piano: un riepilogo su un piano v6 dichiara `6`. Chi leggeva quel campo dava per scontato un `5` che ora può essere un `6` — e prima avrebbe letto `5` accanto a un `plan_hash` di un altro dominio |
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
