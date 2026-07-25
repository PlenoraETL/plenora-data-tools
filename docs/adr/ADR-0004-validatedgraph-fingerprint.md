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
    required_capabilities: CapabilitySet,   // include backend GEOS/PROJ
                                            // e il profilo di publish richiesto
    input_contract_fingerprints: Vec<ContractFingerprint>,
    plan_format_version: u16,
}
```

L'executor rifiuta il grafo su qualunque mismatch (catalogo, versione Arrow,
backend, profilo di publish non supportato, contratto di input diverso) con
errore di mismatch esplicito — mai procedere "alla cieca".

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
