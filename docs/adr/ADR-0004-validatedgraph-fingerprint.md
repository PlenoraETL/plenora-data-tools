# ADR 4 — Identità del `ValidatedGraph` e fingerprint del catalogo

- **Stato**: accettato (design)
- **Decisioni collegate**: D7, D17, D20
- **Riferimenti**: `Architetture.md` §6.1, §7

## Contesto

Un `ValidatedGraph` può essere riusato (stesso piano, input diversi) e potrebbe
essere conservato in cache. Se il catalogo, i backend o l'engine cambiano, un
grafo validato in precedenza potrebbe non essere più corretto. Il fingerprint
deve cambiare **se e solo se** cambia la semantica: un hash del binario o del
codice compilato sarebbe instabile tra compilatori, piattaforme e build —
troppo fragile; un hash dei soli ID delle operazioni sarebbe cieco ai cambi di
comportamento — troppo debole.

## Decisione

### Fingerprint del catalogo

Ogni operazione dichiara versioni esplicite per componente:

```rust
struct OperationDescriptor {
    id: &'static str,
    semantic_version: u32,           // cambia il comportamento dell'operazione
    config_schema_version: u32,      // cambia la forma/significato della config
    contract_analysis_version: u32,  // cambia analyze_contract
    kernel_version: u32,             // cambia l'implementazione del kernel
    // + capability, execution_class, determinism
}
```

Il `catalog_fingerprint` è l'hash dei descrittori serializzati in **ordine
stabile**, incluse le versioni, le capability, la classe di esecuzione e le
regole di determinismo. Due cataloghi con gli stessi nomi ma semantica diversa
producono fingerprint diversi.

**Perimetro del fingerprint (v1)**: l'hash è calcolato sui **soli descrittori
delle operazioni usate dal piano** (`planner.rs`), non sull'intero catalogo.
Scelta consapevole: un grafo validato non dipende da operazioni che non usa,
quindi un bump di versione su un'operazione estranea non deve invalidarlo. La
proprietà "stessi nomi, semantica diversa → fingerprint diverso" vale per
ogni operazione effettivamente referenziata dal piano.

Significato esatto delle quattro versioni:

- `semantic_version`: cambia il **comportamento osservabile** dell'operazione;
- `kernel_version`: cambia l'implementazione fisica o il risultato potenziale
  (anche senza cambio di contratto);
- `contract_analysis_version`: cambia l'inferenza del `DataContract`;
- `config_schema_version`: cambia forma o significato della configurazione.

**Disciplina di versionamento**: il controllo CI "il diff tocca il kernel → la
PR tocca il descrittore" è solo un'**euristica** (falsi positivi possibili,
modifiche semantiche in helper esterni non intercettate). La garanzia reale è
la combinazione di: snapshot canonico del catalogo in CI, golden test
semantici per operazione, review obbligatoria del descrittore, changelog per
operazione, test differenziali sui kernel (contro i progetti di origine e, per
il tabellare, contro il riferimento Python Manipola).

### Canonicalizzazione del piano (`plan_hash`)

Il `plan_hash` è calcolato sul piano **canonico migrato**, non sul JSON grezzo:

- ordine dei campi JSON irrilevante;
- numeri e default normalizzati;
- alias legacy sostituiti dagli ID canonici;
- la migrazione canonica **materializza i default**: config omessa e config con
  default esplicito producono lo stesso piano canonico, quindi lo stesso hash.

Due piani legacy semanticamente equivalenti migrano allo stesso piano canonico:
è ciò che rende cache e riproducibilità affidabili.

### Contenuto dell'identità

```rust
struct ValidatedGraph {
    plan_hash: PlanHash,
    catalog_fingerprint: CatalogFingerprint,
    engine_version: EngineVersion,
    arrow_version: ArrowVersion,
    required_capabilities: CapabilitySet,   // include backend GEOS/PROJ
                                            // e il profilo di publish richiesto
    input_contract_fingerprints: Vec<ContractFingerprint>,
    plan_format_version: u16,
}
```

L'executor rifiuta il grafo su qualunque mismatch (catalogo, versione Arrow,
backend, profilo di publish non supportato, contratto di input diverso) con
errore di mismatch esplicito — mai procedere "alla cieca".

Sul **contratto di input** la garanzia va detta per esteso, perche' dipende da
cosa il chiamante fornisce e non e' uniforme (vedi l'emendamento in fondo):
quando l'input porta il proprio `DataContract` il confronto e' sul fingerprint
completo; quando non lo porta, il confronto e' sullo schema Arrow completo —
campi *e* metadati di campo. Il secondo e' il minimo garantito, non lo stesso
controllo: due contratti distinti che condividono lo schema (CRS risolto
contro mancante) lo superano entrambi. Chi ha bisogno del confine chiuso usa
`Inputs::add_with_contract`.

### Serializzazione

Nella v1 il `ValidatedGraph` vive **solo in memoria** (validate → execute nello
stesso processo). La serializzazione persistente è rimandata: richiede un
formato versionato dedicato e la gestione della compatibilità tra versioni
dell'engine, e non è necessaria per i casi d'uso attuali.

## Conseguenze

- Il caching dei grafi validati è sicuro: ogni invalidazione è esplicita.
- Test obbligatori: stesso piano con fingerprint diverso; stessi ID con
  `semantic_version` diversa; alias → stesso hash del piano canonico; config
  omessa vs esplicita → stesso hash.
- Quando sarà introdotta la serializzazione, questo ADR va esteso con il
  formato e le regole di compatibilità.

## Emendamento 2026-08-16 — `plan_hash` e proprieta' tipizzate della geometria

La review statica del 2026-08-16 ha trovato due modi diversi in cui
l'identita' del grafo non distingueva contratti o piani distinti.

**Canonicalizzazione dei numeri del piano.** `canonical_number` passava ogni
numero JSON per `f64` prima di deciderne la forma canonica. Gli interi oltre
2^53 non hanno un `f64` esatto: `9007199254740993` arrotondava a
`9007199254740992.0`, superava la guardia `|v| <= 2^53` e veniva
canonicalizzato come `9007199254740992`. Due config semanticamente diverse
ottenevano lo stesso piano canonico e **lo stesso `plan_hash`**, rendendo
insicuri cache e riuso del piano. La canonicalizzazione si applica ora solo
ai numeri *originariamente* in virgola mobile: un intero JSON e' gia'
canonico e passa invariato a qualunque magnitudo, `u64::MAX` incluso.

**Chiavi JSON duplicate.** Il piano veniva deserializzato con la regola
`serde_json` «vince l'ultima», che risolve i duplicati **prima** della
validazione e del `plan_hash`: due testi diversi producevano lo stesso hash,
e la chiave scartata poteva essere quella che l'autore intendeva. Un
documento con chiavi ripetute e' ora rifiutato
(`plenora_core::json::ensure_no_duplicate_keys`), non risolto.

**`input_contract_fingerprints`.** Le proprieta' tipizzate della geometria
(`encoding`, `types`) non riferiscono `FieldId` e non sono identita' interna
del grafo: sono parte del contratto osservabile e ora entrano nel
fingerprint. Restandone fuori, due contratti che dichiaravano tipi
geometrici diversi — `exact:point` ed `exact:polygon` — condividevano il
fingerprint, e un grafo validato sul primo veniva riusato sul secondo senza
rivalidazione. Un contratto che non dichiara nulla produce lo stesso JSON di
prima: i fingerprint esistenti restano stabili.

**`check_input_compatibility`** invoca ora `DataContract::validate()` sul
contratto fornito prima di confrontarne il fingerprint: un contratto
strutturalmente invalido ha comunque un fingerprint, e senza questo passo
poteva superare il controllo di compatibilita' per poi fallire a runtime.

## Emendamento 2026-08-16 (secondo giro) — scope, `row_count`, binding all'esecuzione

**Nel fingerprint entrano anche `scope` di `geometry.types` e `row_count`.**
Lo scope distingue una dichiarazione valida per lo schema da una valida per
il dataset intero: sono garanzie di forza diversa, e due contratti che le
dichiarano diverse non sono lo stesso contratto. `row_count` non riferisce
`FieldId` — a differenza di `sorted_by`, che resta escluso — ed entra
nell'analisi (`map_row_count` lo propaga nei contratti d'arco): due input che
dichiarano cardinalita' diverse producono analisi diverse. Entrambi entrano
solo quando ci sono, quindi i fingerprint dei contratti che non li dichiarano
restano invariati.

**Il confine dell'esecuzione si puo' chiudere davvero.** `execute` confrontava
solo `provided.fields()` con i campi del contratto validato: i METADATI di
campo — che portano le chiavi canoniche della geometria — restavano fuori, e
due sorgenti con gli stessi campi e geometrie diverse passavano identiche. Ora
il confronto e' sullo schema completo, metadati inclusi.

Resta che uno schema uguale non implica un contratto uguale (CRS risolto
contro mancante, tipi dichiarati diversi). Per questo `Inputs::add_with_contract`
permette all'input di portare il proprio `DataContract`: l'esecuzione ne
verifica allora il **fingerprint completo** contro quello registrato nel
grafo, la stessa garanzia di `check_input_compatibility`, applicata al
momento in cui i dati entrano. La CLI passa sempre i contratti della
discovery; per chi incorpora l'engine come libreria e' la forma da usare, e
il confronto sullo schema resta il minimo garantito per chi non lo fa.

## Emendamento 2026-08-16 (terzo giro) — `-0.0` non e' `0`

`canonical_number` emette come intero i double che coincidono con la propria
parte troncata. `-0.0` supera quel test — `(-0.0).trunc() == -0.0` e
`-0.0 == 0.0` — e diventava `0`: due piani con segni diversi ottenevano lo
stesso testo canonico e quindi lo **stesso `plan_hash`**, che e' la stessa
classe di difetto degli interi oltre 2^53. Il segno dello zero e' osservabile
(divisione, `atan2`, formattazione), quindi non e' un dettaglio da
normalizzare: `-0.0` resta in forma floating e i due piani restano distinti.

## Emendamento 2026-08-16 (quarto giro) — profilo stretto degli input

Dichiarare onestamente il profilo debole non lo rende adatto a una garanzia
safety-critical: chi ha bisogno del confine chiuso deve poterlo ESIGERE, non
ricordarsi di usare la variante giusta a ogni chiamata. `Inputs::strict()`
costruisce un insieme in cui `Inputs::add` — l'ingresso senza contratto —
fallisce, e passa solo `add_with_contract`.

E' additivo di proposito: rendere stretto il comportamento di `Inputs::new()`
romperebbe ogni chiamante che oggi compila, ed e' una decisione di rilascio
(semver) che non spetta a questa API.

**Decisioni di rilascio applicate (2026-08-16).** Il maintainer le ha prese, e
sono ora nel codice:

- `Inputs::add` e `Inputs::with` sono **deprecate** (`#[deprecated]`) e
  restano in vigore solo per non rompere i chiamanti esistenti. Il percorso
  permissivo continua a funzionare e a essere testato finche' esiste; i test
  che lo coprono dichiarano un `#[allow(deprecated)]` mirato, cosi' la
  rimozione futura ha un elenco esatto di punti da toccare;
- la **CLI** costruisce ora `Inputs::strict()`: non puo' piu' inviare un input
  senza contratto neanche per errore di manutenzione, e non e' piu' solo una
  convenzione rispettata;
- il **futuro SDK Python** non esporra' affatto il percorso permissivo: li'
  non esistono chiamanti da non rompere, quindi il profilo stretto e' l'unico
  (vincolo registrato in `docs/piano-usabilita.md`);
- le struct pubbliche di metriche (`ExecutionMetrics`, `NodeMetrics`,
  `SegmentMetrics`, `MemoryMetrics`, `SpillMetrics`) sono
  `#[non_exhaustive]`: crescono con l'osservabilita' del componente e ogni
  campo nuovo non deve essere una rottura semver per chi le legge.

L'inventario completo della superficie rotta e' in
`docs/api-breaking-2026-08-16.md`.


## Emendamento 2026-08-20 — separatore di dominio del `plan_hash` (ADR 15)

Il `plan_hash` non è più `SHA256(canonical_json)`. È

```
plan_hash = SHA256("plenora/plan_hash/v5\0" || canonical_json)
```

dove il prefisso è un **separatore di dominio** che nomina la versione del
formato canonico.

### Perché

ADR 15 ha rinominato il budget di memoria del piano, e la versione canonica è
passata da 4 a 5. In pratica ogni `plan_hash` cambia comunque: la chiave dei
limiti compare nella forma canonica di *ogni* piano, perché la
canonicalizzazione materializza i default. Ma «in pratica» è una proprietà del
**contenuto**, non una garanzia della funzione: basterebbe un formato futuro
in cui la differenza non tocca i byte canonici perché due piani con contratti
diversi condividano un hash.

Un `ValidatedGraph` può essere conservato in cache (è il contesto di questa
ADR). Un grafo riusato per sbaglio con un hash prodotto prima della v5 è un
piano eseguito sotto un contratto di memoria che non è più il suo. Quel caso
va reso **impossibile**, non improbabile — e un separatore di dominio lo rende
impossibile per costruzione.

### Regola per il futuro

Il dominio nomina la versione canonica: **cambiare il formato canonico
significa cambiare il dominio**. Un test lo verifica (il dominio deve
contenere `v{PLAN_SCHEMA_VERSION_V5}`), così una versione bumped senza dominio
aggiornato non passa la suite.

### Che cosa NON cambia

- il `catalog_fingerprint` resta `SHA256` dei descrittori serializzati con i
  prefissi di lunghezza: non ha versioni di formato piano da separare, e
  toccarlo invaliderebbe l'identità del catalogo senza ragione;
- il `ContractFingerprint` degli input resta invariato;
- la canonicalizzazione del piano non cambia regole: ordine, default
  materializzati e normalizzazione dei numeri sono quelli di sopra.
