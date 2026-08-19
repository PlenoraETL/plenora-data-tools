# Hotfix del 2026-08-18 — gate fuzz rosso e panic UTF-8

Due problemi, entrambi chiusi. Perimetro limitato a questi: nessun
orchestratore, nessun SDK, nessuna trasformazione nuova.

---

## 1. Il gate di rilascio rosso su `24a42a3` — regressione mia

**Finding.** CI di `24a42a3`, job `gate manifesto di rilascio`, step `Verifica
lockfile fuzz`:

```
error[E0004]: non-exhaustive patterns: `NumericBound::Decimal { .. }` not covered
  --> fuzz/fuzz_targets/diff_kernels.rs:213
error: could not compile `plenora-data-tools-fuzz` (bin "diff_kernels")
```

**Causa.** La stabilizzazione ha aggiunto la variante `Decimal { unscaled,
scale }` a `plenora_kernels_table::NumericBound` — verificato: su `4b9edda`
l'enum aveva tre varianti, `git log -S` attribuisce la quarta a `24a42a3`.
L'oracolo del target `diff_kernels` aveva un `match` esaustivo sulle tre
vecchie varianti, dentro una funzione dichiarata «Replica di
`filtering::bound_as_f64`».

**Perche' e' sfuggita.** `fuzz/` e' **fuori dai `members`** del workspace:
`cargo clippy/test --workspace --all-targets` non lo compila. Sono 17 target,
15 dei quali consumano le API pubbliche dei cinque crate. Il gate che lo
compila **esisteva gia'** in CI (`ci.yml`) ed e' quello che ha colto l'errore;
mancava dalla mia routine locale.

**Il fix non era un braccio in piu'.** `filtering::bound_as_f64` nel kernel
**non esiste piu'**: la stabilizzazione ha sostituito la degradazione a `f64`
con il confronto esatto per tipo (`scalar_compare`, `i128` scalato per i
decimali). L'oracolo replicava una semantica abbandonata, e su due percorsi di
fallback degradava a `f64` **sia il valore sia l'estremo**. Aggiungere un
braccio che arrotonda avrebbe fatto compilare un oracolo sbagliato: un
differenziale con oracolo obsoleto o segnala differenze inesistenti, o
concorda per la ragione sbagliata.

`bound_as_f64` e' stata quindi **eliminata** e sostituita da
`compare_scalar_exact`, che:

- sul testo confronta nel dominio esatto (`NumericBound::parse` +
  `compare_bounds`), con la stessa normalizzazione del kernel (trim, virgola
  decimale) e nessun arrotondamento;
- su tutto il resto **fallisce fail-closed** («tipo non confrontabile
  esattamente») invece di approssimare. Il kernel su quei tipi fallisce a sua
  volta, e il target confronta gli errori come tali (`(Err(_), Err(_)) => {}`).

**Ricerca della classe sui 17 target.**

| cercato | esito |
|---|---|
| `match` esaustivi su `NumericBound` | uno solo, quello rotto — ora eliminato. L'altro `match` sul tipo ha gia' un braccio jolly |
| altre repliche della vecchia semantica | tre superstiti (`numeric_eq`, `ordered_typed`, `within_bounds`), **confrontate riga per riga col kernel: fedeli**. `ordered_typed` differisce solo perche' il kernel ha tipizzato l'operatore per totalita' |
| `match` esaustivi su altre enum pubbliche | uno su `Operator` (`oracle_evaluate`). **Resta esaustivo di proposito**: e' la rete che costringe l'oracolo a trattare un operatore nuovo invece di ignorarlo. Il gate che lo protegge e' la compilazione del crate fuzz |
| altri consumatori fuori workspace | `executor_dag` usa `Inputs::with`, deprecata dalla stabilizzazione: quattro **warning**, non errori. Segnalato, non corretto: migrare a `with_contract` cambia gli input del target, fuori dal perimetro di questo hotfix |

**Casi aggiunti al generatore** — senza, il ramo `Decimal` non viene mai
costruito e il confronto esatto non e' esercitato:

- scale positive: `10.5`, `10.50` (stesso valore, scritture diverse), `0.1` e
  `0.10000000000000001` (stesso `f64`, valori diversi);
- scala negativa: `1e3`, `-2.5e2`;
- oltre 2^53: `9007199254740992`/`...93`, `-9007199254740993`, `u64::MAX`;
- non rappresentabile in `i128`: un decimale a 45 cifre, che deve ricadere su
  `F64` senza rompere il confronto.

Entrano sia nei valori della **colonna** (`edge_string`) sia negli **estremi**
(`filter_value`): servono entrambi, perche' il percorso non tipizzato si
raggiunge solo con una colonna testuale.

**Regressioni deterministiche nel workspace.** I target fuzz non girano nella
suite, quindi la semantica su cui l'oracolo ora si appoggia e' fissata da due
test in `plenora-kernels-table`:

- `il_confronto_di_un_estremo_decimale_e_esatto` — scale positive e negative,
  oltre 2^53, intero contro decimale, non rappresentabile, NaN;
- `il_filtro_ordinato_su_colonna_testuale_non_passa_da_f64` — il percorso non
  tipizzato visto dal kernel: `> 9007199254740992` su una colonna Utf8 seleziona
  `9007199254740993`, che in `f64` sparirebbe.

Il secondo test **ha corretto la mia attesa**: avevo assunto che `==` su
colonna testuale fosse numerico. Non lo e' — il kernel confronta il TESTO
(`scalar_as_string`), mentre gli operatori ordinati e `between` passano dal
numerico esatto. L'asimmetria e' ora fissata come tale, letta dal codice e non
assunta.

**Il gate in CI.** Il comando esatto, gia' presente e ora in un **job
proprio** (`gate compilazione target fuzz`):

```
cargo check --manifest-path fuzz/Cargo.toml --locked
```

Stabile, `--locked`, tutti i `[[bin]]` dei 17 target, nessuna campagna
libFuzzer, nessun nightly, nessun sanitizer. Richiede un compilatore C++ (build
script di `libfuzzer-sys`): presente sui runner, da installare in container
(`g++`).

**Correzione a una versione precedente di questo report.** In prima stesura
avevo scritto che spostare lo step in coda al job risolveva il problema della
cascata. **Non lo risolve.** Con step in serie il primo fallimento nasconde
sempre tutto cio' che segue: la coda cambia solo *quali* controlli restano
nascosti, e nel caso concreto avrebbe spostato l'occultamento dai due
`cargo audit` alla compilazione del crate fuzz. La forma giusta e' che
controlli indipendenti siano **job indipendenti**, ed e' quella adottata (vedi
sezione 4).

---

## 2. Il panic UTF-8 di `analyze_geo` — preesistente

**Finding.** Campagna fuzz notturna, fallita ogni notte dal 14/08:

```
thread panicked at crates/plenora-kernels-geo/src/analyze/helpers.rs:116:45
==5234== ERROR: libFuzzer: deadly signal
Test unit written to artifacts/analyze_geo/crash-fd1eba39798feba74d4fc8837358f35c82a4a34a
```

**Causa.** `&hex[index..index + 2]`: slicing di `&str` per **indici di byte**.
Il controllo a monte verificava la sola PARITA' della lunghezza in byte, che
non implica il confine di carattere: `"aéb"` e' lungo quattro byte — pari — ma
l'indice 2 cade dentro la codifica UTF-8 di `é`, e affettare fuori da un
confine e' un **panic**, non un errore. L'input arriva dalla configurazione di
un piano.

R6 non lo copriva: non c'e' nessun `unwrap`/`expect`/`panic!` — il panic e'
dentro l'indicizzazione.

**Fix.** La decodifica lavora ora sui **byte**: un esadecimale valido e' per
definizione ASCII, quindi ogni byte fuori da quell'insieme e' gia' un input
non valido, non un carattere da interpretare. Nessuna stringa viene mai
affettata.

**La stessa decodifica esisteva in due copie** — `plenora-kernels-geo` e
`plenora-engine/src/prepare.rs:1876` — **entrambe con lo stesso difetto**, ed
entrambe raggiungibili dalla config di un piano. Sono state unificate in
`plenora_kernels_geo::wkb_hex_to_bytes`, accanto al validatore che la
accompagna sempre; i due chiamanti traducono solo l'esito nel proprio errore.

**Ricerca della classe** — slicing di `&str` con intervalli calcolati o
costanti, su tutto il codice di produzione (due passate: filtro per nome e poi
ogni indicizzazione con intervallo, 35 candidati esaminati).

| sito | verdetto |
|---|---|
| `kernels-geo/src/analyze/helpers.rs:116` | **difetto** — corretto |
| `engine/src/prepare.rs:1876` | **difetto**, stessa forma, raggiungibile da config — corretto |
| `kernels-table/src/cleansing.rs:695-697` | **sicuro**: preceduto da un controllo che TUTTI i byte 0..10 sono cifre ASCII e `-`, quindi ogni indice e' un confine |
| `kernels-geo/src/construction.rs:74` | **sicuro**: l'indice viene da `find('(')`, che restituisce un confine per definizione; e la riga sotto usa gia' `head.get(..5)`, la forma non panicante dove l'indice e' arbitrario |
| `kernels-table/src/security.rs:798,800` | **sicuro**: gli offset vengono da `char_indices()`, confini per costruzione |
| `kernels-table/src/strings.rs:229,287` | **sicuro**: gli offset vengono da `regex::CaptureLocations`, che il crate garantisce sui confini |
| `formula.rs:128/170/186`, `protocol.rs`, `pair.rs`, `envelope.rs`, `topology.rs`, `joins.rs`, `crs.rs` | **fuori classe**: sono slice di byte o `Vec`, non `&str` |

Corretti solo i due realmente raggiungibili con input non fidato, come da
consegna.

**Regressioni.**

- `il_wkb_esadecimale_non_va_mai_in_panic_su_input_ostile` — il caso del crash
  (`"aéb"`) piu' altri cinque multi-byte di lunghezza pari, i casi dispari e
  non-hex, e la conferma che gli esadecimali validi continuano a decodificare
  (un POINT(1 2) reale incluso);
- `il_wkb_di_config_ostile_e_un_errore_di_piano` — lo stesso attraverso
  `validate_wkb_hex`, cioe' il percorso dell'analizzatore da cui il fuzzer
  arrivava;
- `nessun_wkb_di_config_ostile_manda_in_panic_l_analisi` — replay
  deterministico dell'invariante «mai panic» attraverso
  `analyze_geo_contract`, per ogni operazione che accetta un WKB da config
  (`geo.within`, `geo.line_locate_point`, `geo.snap`) su 14 input ostili.

**Verifica di discriminazione.** Ripristinando lo slicing per indici di byte,
entrambe le prime due regressioni cadono con il panic esatto della campagna:

```
byte index 2 is not a char boundary; it is inside 'é' (bytes 1..3) of `aéb`
```

---

## 3. I manifesti storici non erano piu' eseguibili — checker portato in casa

Emerso rieseguendo gli step che il gate rosso teneva `skipped`, ed e' emerso
solo perche' li ho eseguiti.

**Che cosa ho trovato.** Cinque step del job dipendono da
`PlenoraETL/plenora-contracts`:

```
- name: Clone plenora-contracts (pin v2.0-rc10)
  run: git clone --depth 1 --branch v2.0-rc10 ...
- name: Verifica manifesto storico 1.0.3 / 1.0.2 / 1.0.1 / 1.0.0 / rc1
  run: python "$RUNNER_TEMP/plenora-contracts/scripts/check_release_manifest.py" ...
```

Oggi quel repository:

- **non ha alcun tag** (`gh api .../tags` restituisce lista vuota) e ha il solo
  branch `main`: il clone di `v2.0-rc10` fallisce con exit 128;
- **non contiene piu'** `scripts/check_release_manifest.py`.

Il motivo e' dichiarato nel repository stesso (`CUTOVER.md`): «The repository
replacement was completed on 2026-08-18. The former remote history, branches
and tags were not carried into this repository.» Lo stesso documento elenca
fra le attivita' rimanenti: «Verify that no active gate depends on files
removed with the old repository» — ed e' esattamente il caso di `data-tools`.

**Quando si e' rotto.** La CI passava su `4b9edda` il 2026-08-07, manifesti
compresi: il tag esisteva. La sostituzione e' del 2026-08-18, lo stesso giorno
del commit di stabilizzazione. Nella CI di `24a42a3` questi step erano
**skipped**, perche' il fallimento del check fuzz li precedeva.

**Conseguenza dello spostamento che ho fatto.** Avendo messo il check fuzz in
coda, la prossima esecuzione **raggiungera'** il clone e fallira' li'. Il job
resta rosso, ma per una ragione diversa e visibile invece che nascosta dietro
la prima. Preferisco che si veda.

### Recupero: provenienza si', contenuto no

Tentato per primo, come da consegna:

| tentativo | esito |
|---|---|
| `gh api .../contents/scripts?ref=v2.0-rc10` | 404, «No commit found for the ref» |
| tag e branch del repository nuovo | zero tag, solo `main` |
| copia locale in questo repository o nella storia git | assente |
| `scripts/check_release_manifest.py` in `plenora-contracts@main` | non esiste piu' |

**Dai log dell'ultima esecuzione riuscita** (run 31166125840, 2026-08-07, su
`4b9edda`) sono stati recuperati **due fatti, e nessun altro**:

1. la **provenienza del pin** — `refs/tags/v2.0-rc10` era l'oggetto
   `ba77b16…`, che risolveva al commit
   `3598259bbe07d1c853453ff34ca2c1d1d28a0272`, lo stesso valore che tutti e
   cinque i manifesti dichiarano in `icd.revision`;
2. il **contratto osservabile** del verdetto — formato, testo dell'avviso
   `C2.2`, elenco dei criteri non automatizzati.

**Non e' stato recuperato il repository storico, e il contenuto dello script
originale non e' stato ne' ricostruito ne' riverificato.** Nessuna
dichiarazione di equivalenza con quello script: qui c'e' una verifica scritta
in questo repository, il cui perimetro e' esattamente quello elencato sotto.

### Il checker locale, e il perimetro che dichiara

`scripts/check_release_manifest.py` e' **interamente locale e immutabile**:
non clona e non consulta `plenora-contracts`, ne' i suoi tag ne' i suoi
contenuti. Il `plenora-contracts` attuale e' trattato come una **nuova linea
normativa**, e i manifesti storici non vanno reinterpretati con regole nate
dopo di loro.

Il verdetto `pass` significa esattamente questo, e nulla di piu':

| criterio | verifica |
|---|---|
| C1.1 | `manifest_version`, `component`, `component_version` |
| C1.2 | il nome del file e' la versione (eccezione storica `rc.json`, ammessa solo per `0.1.0-rcN`) |
| C2.2 | pin ICD **esatto**: `plenora-contracts`, `v2.0-rc10`, `3598259…`. Non «un tag non vuoto e 40 cifre esadecimali» — un pin formalmente valido ma diverso sarebbe una riscrittura della provenienza. L'ESISTENZA della revisione ICD resta un **avviso**: vive in un repository che non consultiamo |
| C3.1 | `revision` e' un commit di QUESTO repository ed e' antenato di `HEAD` |
| C4.1 | `claims.system_rc` e `avionic_certification` false, `component_rc` booleano |
| C4.2 | coerenza fra `verification_claim` e `independent_review` |
| C5.3 | la catena, contro una **mappa locale esplicita**: forma esatta di `supersedes` (chiavi e valori) e tag del predecessore |
| C1.2 | il documento verificato deve stare in `repo/release/`, dopo la risoluzione del percorso |

Restano dichiarati non automatizzati **C2.1, C2.3, C3.2, C4.3, C5.1, C5.2**,
stampati a ogni esecuzione insieme all'elenco di quelli automatizzati.

### La catena: la FORMA storica, non i soli valori

`CATENA` registra per ciascuno dei cinque **il dizionario `supersedes` come e'
stato dichiarato**, chiavi comprese — non una descrizione dei suoi campi:

```
rc.json     -> nessun predecessore (l'unico ammesso a non averne)
1.0.0.json  -> {manifest, component_version, revision}
1.0.1.json  -> {manifest, component_version, revision}
1.0.2.json  -> {manifest, component_version, revision}
1.0.3.json  -> {manifest, component_version, revision, tag, tag_object}
```

Verificare i soli campi *presenti* lasciava passare due riscritture opposte su
una catena dichiarata immutabile: **togliere** `tag`/`tag_object` da `1.0.3` e
**aggiungerli** ai tre che non li avevano. Il confronto e' quindi sull'insieme
delle chiavi prima ancora che sui valori, e una chiave sconosciuta e' un
errore come le altre.

Il registro dice **che cosa** ogni manifesto deve dichiarare; il repository
dice se quella dichiarazione e' **ancora vera**: il tag del predecessore deve
essere esattamente l'oggetto registrato in `TAG_PREDECESSORE` — che vive fuori
da `CATENA` proprio perche' quella registra la forma del documento e non deve
contenere campi che il documento non ha.

Fail-closed su quattro lati:

- **l'assenza del predecessore e' un errore** per tutti tranne `rc.json`. Era
  il difetto piu' grave della prima stesura: `supersedes` mancante passava
  sempre, e un test lo sanciva come verde;
- **chiavi mancanti o in piu' sono errori**, non varianti;
- un manifesto **non elencato** nella mappa e' un errore, con il messaggio che
  rimanda al profilo della linea normativa sotto cui nasce;
- il tag del predecessore nel repository deve essere l'oggetto registrato.

### Il documento verificato deve stare in `repo/release/`

Il vincolo di percorso esisteva per `supersedes.manifest` e **non** per il
manifesto stesso: una copia esterna chiamata `1.0.3.json` veniva validata sul
solo basename — e il basename e' cio' che decide catena e criteri.

Il controllo avviene **dopo** la risoluzione del percorso, quindi con una sola
condizione — il parent risolto dev'essere `repo/release/` risolta — coglie
copie esterne, percorsi assoluti, risalite e symlink che puntano fuori. Il
documento non viene nemmeno caricato.

### Percorsi e radice JSON

`supersedes.manifest` e' un dato di configurazione: percorsi **assoluti**
(`/etc/passwd`, `C:/…`, UNC), **risalite** `..` e file **fuori da `release/`**
sono rifiutati prima di toccare il filesystem.

Un JSON valido la cui radice non sia un oggetto era **gia'** un errore
controllato — `_load_json` lo rifiuta con `ValueError`, catturata e tradotta in
`C1.1`, verificato eseguendolo su `[1,2,3]`, `"stringa"`, `42`, `null`, `true`.
Non c'era traceback; ora c'e' anche la regressione che lo fissa.

### Manifesti futuri: profilo separato

`--profilo` esiste e accetta oggi il solo valore `storico`. Un profilo diverso
**fallisce con un errore esplicito** invece di applicare per inerzia le regole
storiche: i manifesti nati sotto la nuova linea normativa richiederanno regole
costruite su quella struttura.

### Test, e una correzione al mio stesso metodo

`scripts/test_check_release_manifest.py`: **52 test** (uno saltato su Windows —
la creazione di symlink richiede privilegi; gira sul runner Linux).

I test che mutano un manifesto non possono piu' vivere in una directory
temporanea qualunque, proprio per il vincolo appena introdotto: girano contro
un **clone a oggetti condivisi** del repository (`git clone --shared
--no-checkout`), che porta con se' storia e tag annotati senza copiare nulla.

**Correzione al giro precedente.** Avevo riportato «25/25» e poi «33/33
mutanti uccisi». Quei numeri **non valgono**: l'harness passava
`PATH: ""` al sottoprocesso, quindi `git` non era raggiungibile e OGNI mutante
risultava ucciso per errore di ambiente, non perche' il controllo fosse
coperto. L'harness ora usa l'ambiente reale e **verifica per prima cosa che la
suite non mutata passi** — senza quel controllo di sanita' un ambiente rotto fa
sembrare tutto coperto.

Rieseguito correttamente, il primo verdetto onesto e' stato **5 mutanti
sopravvissuti**: `supersedes` non-oggetto, manifesto del predecessore
mancante, tag del repository diverso da quello registrato, e due controlli
sull'object store. I primi tre sono ora coperti da altrettante regressioni.

Risultato attuale: **30 controlli, 29 uccisi, 1 sopravvissuto dichiarato**.

Il sopravvissuto e' il confronto fra il tag e il commit registrato. Superato il
controllo sull'oggetto del tag, in git l'identita' di un oggetto ne determina
il contenuto: tipo e commit di destinazione non sono piu' variabili, quindi
**nessuna modifica a un manifesto puo' raggiungerlo**. Resta perche' difende da
un object store incoerente — riferimento presente, oggetto assente — che non so
costruire in modo portabile. E' scritto nel codice e contato qui come non
coperto, invece di essere spacciato per verificato.

Il vincolo sulla directory canonica non passa da `errori.append` (e' un
`return` anticipato) e l'harness non lo muta: verificato a parte,
neutralizzandolo — **tre test cadono**.

I cinque manifesti sono stati **rieseguiti localmente**: tutti `pass (0 errori,
1 avvisi)`.

## 4. Sette job, nessuno che nasconde gli altri

| job | forma |
|---|---|
| `gate manifesto storico <nome>` | **matrix** sui cinque manifesti, `fail-fast: false` |
| `gate test dei checker di rilascio` | job proprio: i due mutation test, il secondo con `if: always()` |
| `gate audit supply-chain` | `cargo audit` workspace e fuzz, il secondo con `if: always()` |
| `gate compilazione target fuzz` | `cargo check --manifest-path fuzz/Cargo.toml --locked` |

Nessun `needs:` fra loro. I cinque manifesti non sono piu' cinque step in
serie: erano ancora un caso di «il primo fallimento nasconde i successivi»,
solo su scala piu' piccola. Il test dei checker sta in un job separato perche'
un manifesto rotto non deve nascondere un checker rotto, ne' viceversa.

Il clone di `plenora-contracts` **e' rimosso**: nessun gate di questo
repository dipende piu' da un repository esterno.

## Gate eseguiti

| gate | esito |
|---|---|
| `cargo fmt --all --check` | pulito |
| Clippy `--workspace --all-targets --locked` | zero errori, zero warning |
| R6 anti-panic (cinque crate, `--lib --bins`) | pulito |
| Suite default `--workspace --no-fail-fast` | **1401 test, 0 falliti** |
| Clippy full-backends | pulito |
| R6 full-backends | pulito |
| Test full-backends | **1463 test, 0 falliti** |
| `cargo check --manifest-path fuzz/Cargo.toml --locked` | **pulito** (4 warning di deprecazione preesistenti in `executor_dag`) |
| Smoke mirato `analyze_geo` | replay deterministico verde (vedi sotto) |
| Coverage (soglie CI 90 / 85 / 89) | superata: **92.08 / 87.64 / 92.91** |
| Test checker del candidato | **20 test, OK** |
| Gate audit supply-chain: workspace | **pulito** (187 dipendenze, zero advisory) |
| Gate audit supply-chain: fuzz | **pulito** (152 dipendenze, zero advisory) |
| Test checker dei manifesti storici | **52 test, OK** (1 saltato: symlink su Windows); **29/30 mutanti uccisi**, 1 dichiarato non coperto |
| I cinque manifesti storici | **tutti pass** (0 errori, 1 avviso ciascuno) |
| `HEAD` | `24a42a3`, invariato: **nessun commit, nessun push** |

### Esito finale della CI di `24a42a3`

Attesa fino al completamento: unico rosso il gate manifesto.

| job | esito |
|---|---|
| `gate manifesto di rilascio` | **failure** (il finding qui sopra) |
| `test windows-latest` | **success** |
| `test full-backends (linux)` | **success** |
| `gate coverage (linux)` | success |
| `test ubuntu-latest` | success |

### Gli step prima saltati, ora eseguiti

I sei step che il fallimento del check fuzz aveva reso `skipped` sono stati
rieseguiti in container:

- **mutation test del checker del candidato**: 20 test, OK;
- **`cargo audit --deny warnings`** sul workspace: 187 dipendenze, **zero
  advisory**, nessun `--ignore` necessario — gli stessi conteggi documentati
  nel commento del workflow;
- **`cargo audit --deny warnings --file fuzz/Cargo.lock`**: 152 dipendenze,
  **zero advisory**;
- **manifesti storici**: bloccati a monte dal clone, sezione 3.

Cioe': dei controlli che il gate rosso teneva nascosti, i tre eseguibili sono
**puliti**, e il quarto ha rivelato un problema che era li' da prima.

### Smoke di `analyze_geo`: che cos'e' e che cosa non e'

La campagna libFuzzer **non e' eseguibile su questa macchina**: l'immagine
`plenora-rust:nightly-fuzz` e il binario `cargo-fuzz` non ci sono (DER-001,
gia' registrata). Al suo posto ho eseguito un **replay deterministico** dello
stesso invariante attraverso lo stesso punto d'ingresso pubblico del target
(`analyze_geo_contract`), sull'elenco di input ostili scritto a mano.

Non esplora e non sostituisce la campagna: copre esattamente il percorso su
cui la campagna e' andata in panic, e lo copre in una suite che gira sempre.

---

## Che cosa resta aperto

- ~~La campagna fuzz completa resta non eseguita~~ — **eseguita e verde**,
  vedi la sezione «Verdetto» in coda.
- **`executor_dag` usa `Inputs::with`, deprecata**: quattro warning. Migrarlo
  a `with_contract` e' un cambiamento agli input del target, fuori perimetro.
- **`fuzz/` resta fuori dal workspace**: e' una scelta strutturale del
  progetto (grafo di dipendenze proprio, lockfile separato). Il presidio e' il
  gate `cargo check --locked`, ora in coda al job e nella routine locale.
- **La verifica dei manifesti storici e' ora nostra e volutamente immutabile.**
  Non segue il nuovo `plenora-contracts` e non deve seguirlo: quei documenti
  descrivono rilasci gia' avvenuti. Cio' che manca e' il **profilo per i
  manifesti futuri**, da costruire sulla nuova struttura: oggi `--profilo`
  diverso da `storico` fallisce con un errore esplicito, che e' il
  comportamento giusto finche' quel profilo non esiste.
- **Il perimetro della verifica e' quello dichiarato**, non quello dello script
  perduto: sette criteri automatizzati, sei dichiarati non automatizzati.
  Nessuna equivalenza con l'originale e' affermata, perche' nessuna e'
  dimostrabile.
- **I criteri C2.1, C2.3, C3.2, C4.3, C5.1, C5.2 restano non automatizzati**,
  come prima: la verifica li stampa a ogni esecuzione invece di lasciarli
  impliciti.
- **DER-010** e **DER-011** restano invariati.
- **Clippy MSVC non e' un rischio aperto**, ed era un errore dei verbali
  precedenti (compreso questo, in prima stesura): il job `test windows-latest`
  gira su `windows-latest` con la toolchain nativa `1.92.0` — quindi MSVC — ed
  esegue **sia** il gate anti-panic R6 **sia** `cargo clippy --workspace
  --all-targets --locked`. Il lint su MSVC e' coperto da quando quel job
  esiste. Resta non verificato il solo **full-backends su Windows**, perche'
  quel job e' Linux-only: e' un'altra cosa, e non e' il gate MSVC generico.

---

## Verdetto: CI e campagna fuzz su `671214c`

Il commit e' `671214c`, su `main`.

### CI — 12 job su 12 verdi

`gate manifesto storico` rc / 1.0.0 / 1.0.1 / 1.0.2 / 1.0.3 (cinque celle
indipendenti), `gate test dei checker di rilascio`, `gate audit supply-chain`,
`gate compilazione target fuzz`, `gate coverage (linux)`, `test ubuntu-latest`,
`test windows-latest`, `test full-backends (linux)`.

I due gate che erano rossi o saltati sul commit precedente — compilazione del
crate fuzz e manifesti storici — sono verdi, e i due `cargo audit` che il
fallimento in serie teneva nascosti sono stati eseguiti.

Prima esecuzione tentata: **tutti i job morti in 2 secondi con zero step**, per
fatturazione GitHub («The job was not started because recent account payments
have failed…»), non per il codice. Rieseguita a repository reso pubblico.

### Campagna fuzz — 16 target ATTIVI su 16 verdi

Run 32221206230, `workflow_dispatch`, 30 minuti per target, 05:54 → 06:28 UTC.

`analyze_geo`, il target che dal 14/08 cadeva entro il primo minuto:

```
#13303724  DONE  cov: 3339  ft: 9013  corp: 1830/265Kb  exec/s: 7386
Done 13303724 runs in 1801 second(s)
```

13,3 milioni di esecuzioni, nessun crash, nessun artefatto. Verde anche
`diff_kernels`, l'oracolo riscritto in questo hotfix.

**`arrow_transform` NON e' stato eseguito**: e' in quarantena nella matrice di
`fuzz.yml` dal 2026-08-07, con la motivazione accanto — `libfuzzer-sys`
installa un hook di panico che chiama `process::abort()` prima dell'unwinding,
quindi il target non puo' osservare la barriera, che e' invece verificata dal
test `ipc_decode_converte_il_panico_di_arrow_in_errore`. La quarantena resta un
residuo dichiarato: **16 su 16 attivi**, non 17 su 17.

### Residui reali dopo questa chiusura

**DER-010**, **DER-011**, e la **quarantena di `arrow_transform`**. Nient'altro.
