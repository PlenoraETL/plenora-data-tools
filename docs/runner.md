# Runner

`plenora-pipeline` concatena le operazioni **tabellari** e **geo** del
catalogo su tabelle intere in memoria: un `RecordBatch` per nome, niente
streaming ([«Operazioni geo»](#operazioni-geo) per quelle supportate).

```rust
let pipeline = Pipeline::from_json(testo)?;              // o costruita in Rust
let validata = pipeline.validate(&[("ordini", schema)])?; // senza dati
let esito = validata.run(vec![("ordini".into(), tabella)])?;
// esito.outputs: Vec<(String, RecordBatch)>; esito.report: un ReportPasso per passo
```

## Il piano

In Rust il piano si costruisce con le strutture `Pipeline` e `Passo`; dal
testo si legge solo con `Pipeline::from_json`, che rifiuta campi sconosciuti
e chiavi ripetute a ogni profondità, config comprese. `Pipeline` non
implementa `Deserialize`: una lettura serde diretta terrebbe in silenzio
l'ultima di due chiavi ripetute.

```json
{
  "version": 1,
  "inputs": ["ordini", "clienti"],
  "limits": {"max_rows_per_edge": 1000000},
  "steps": [
    {"out": "validi", "op": "table.filter", "in": ["ordini"],
     "config": {"column": "importo", "operator": ">", "value": 0}},
    {"out": "uniti", "op": "table.join", "in": ["validi", "clienti"],
     "config": {"left_keys": ["cliente"], "right_keys": ["id"], "how": "inner"}},
    {"out": "totali", "op": "table.aggregate", "in": ["uniti"],
     "config": {"group_by": ["regione"],
                "aggregations": [{"column": "importo", "function": "sum"}]}}
  ],
  "outputs": ["totali"]
}
```

- `version`: solo `1`.
- `inputs`, `steps[].out`: nomi in forma SSA, ognuno definito una volta;
  un passo usa solo nomi definiti prima (`in`, nell'ordine dell'operazione:
  left, right); `outputs` nomina tabelle definite, senza ripetizioni.
- `op`: id canonico del catalogo; gli alias legacy si rifiutano.
- `config`: la config del kernel; assente vale `{}`.
- `crs` (facoltativo): CRS di piano per i produttori geo, risolto in
  validazione.
- `limits` (facoltativo): sostituisce uno per uno i default di
  `Limits::default()` (`max_input_rows`, `max_output_rows`,
  `max_rows_per_edge`, `max_expansion_factor`, `max_governed_memory_bytes`,
  `max_string_bytes`, `max_regex_bytes`), poi `Limits::validate`. Gli altri
  limiti non sono dichiarabili, perché il runner non li applica: anche
  `max_temp_bytes` e `spill_partitions`, che esistevano finché il runner
  scriveva su disco, oggi sono campi sconosciuti e il piano si rifiuta
  (`InvalidPlan`).

## Validazione

**Regola**: ciò che schemi, config e limiti rendono prevedibile fallisce in
`validate`, mai dopo che qualche passo ha girato.

Prima di qualunque esecuzione, contro gli schemi degli input: nomi SSA;
limiti di complessità del piano (`PlanLimits::default()`: passi, input,
archi, fan-out, profondità, byte di config per passo, lunghezza dei nomi,
byte del testo JSON); operazione, arietà e dispatch (`table.concat` a più
di due input e le operazioni senza dispatch sono `Unsupported`); config
tipizzate una volta; contratti di output passo per passo con
`analyze_table_contract` e i limiti con cui i kernel eseguiranno, o con
`analyze_geo_contract` e il CRS di piano per le geo, un solo
`FieldAllocator`, base degli indici della diagnostica per riga di ogni
passo ([«Diagnostica per riga»](#diagnostica-per-riga)); colonne di ogni
input e di ogni contratto contro `max_columns`. Ogni contratto (input e
uscite dei passi) porta lo schema che il runner emette, con il blocco
canonico delle geometrie (`arrow_schema_from_contract`): il passo
seguente si analizza sullo schema delle tabelle che riceverà, e le
tabelle d'ingresso ricevono quello schema in `run` (stesse colonne, solo
metadati in più; per una tabella senza geometrie nulla cambia). Lo schema
di ogni output, quello che esce dal runner, è a parte: versione del
contratto e identità dei campi anche senza geometrie
([«Metadati Arrow»](metadati-arrow.md#metadati-arrow)).
`table.transpose` e `table.pivot` senza `mapping` si rifiutano: il loro
schema d'uscita dipende dai dati. Con `mapping` (valore pivot come testo →
nome di colonna) lo schema lo fissa la config, e il kernel lo rispetta per
contratto: le colonne indice, poi una colonna per voce del mapping,
nell'ordine delle chiavi, anche per un valore che i dati non contengono
(colonna tutta null); i valori fuori dal mapping non danno colonne, ma le
loro righe contano per le chiavi indice, che restano tutte (con celle
null). Il tipo della colonna viene dall'aggregazione: `Float64` per
`sum`/`mean`/`min`/`max`, `Int64` per `count`, `Utf8` per `concat`, il
tipo del valore per `first` (il default) e `last`. Si rifiutano, in
analisi e nel kernel (`Pivot::verifica_mapping`): nomi di output vuoti,
ripetuti o uguali a una colonna indice; colonne indice ripetute; chiavi che
nessun valore potrebbe incontrare, perche' il valore si confronta con la
chiave come testo: con una `pivot_col` `Int64` o `UInt64` la chiave deve
essere la forma canonica dell'intero (`"1"`, non `"01"` ne' `"1.0"`), e un
mapping su una `pivot_col` che non sia testo o intero (float, date,
timestamp, decimali, booleani) si rifiuta, invece di dare in silenzio una
colonna tutta null. Senza `mapping`, un valore pivot che si chiama come una
colonna indice e' un errore del kernel.

**Ogni regola sulla config sta in un posto solo: l'analisi dei kernel.**
`analyze_table_contract(op, inputs, config, fields, limits)` riceve i limiti
del chiamante e rifiuta, per chiunque chiami i kernel e non solo per il
runner: nomi, testi, regex e conteggi oltre i limiti; nomi ripetuti e liste
vuote dove non hanno senso; chiavi di join, semi/anti, asof, `table_diff`,
FK e reconcile non leggibili come testo, o di lunghezza diversa fra i lati;
la riga intera di `distinct` senza `subset` con colonne che non sono testo;
gli operatori di `filter` e `conditional` sui tipi che il kernel non sa
valutare (testuali su colonne non testuali, ordinati fuori da
`scalar_compare_supported`, `==`/`!=` numerici con un valore non numerico);
i parametri che il kernel ignorerebbe (`date_format`, `precision`, `scale`,
`timezone` di `type_cast` su target che non li usano; `ascending` di
`add_row_number` e `dedup_advanced` senza `order_column`;
`inclusive_min`/`inclusive_max` di `assert_range` senza l'estremo;
`quantile`, `separator`, `distinct`, `skip_null` e `ddof` di `aggregate`
su funzioni che non li usano; `chars_start`, `chars_end` e `mask_char` di
`mask_data` fuori da `mask_type=custom`; `value` di `fill_na` con
`ffill`/`bfill`; `offset` di `window_function` fuori da `lag`/`lead`;
`ddof` di `rolling_window` fuori da `stddev`; `output_column` ed
`extract_all` di `string_extract` con gruppi con nome; `ignore_index` di
`concat` con ogni valore; `n` di `sample` con `fraction`, `random_state`
senza strati su un campione sempre vuoto; `null_literal` di `md5_hash` e
`sha256_hash` fuori da `null_policy=literal`; `invalid` e `ambiguous`
delle operazioni sulle date con ogni valore; `value` di `filter` e
`conditional` con `isnull`/`notnull`; `errors` di `type_cast` su `str`,
`binary_utf8`, `dictionary_utf8`; `separator` di `concat_columns` con una
colonna e di `table_diff` con una sola colonna in `compare_columns`;
`delimiter` di `split_column` con una colonna d'uscita e `max_splits` che
non riduce le parti; `width` 0 di `string_pad`; `n` 0 di `top_n` e
`offset` di `limit` con `n` 0; `min_rows` 0 di `assert_cardinality`;
`tolerance` 0 di `asof_join` con `allow_exact=false`; `columns` vuoto di
`drop_columns`, `renames` vuoto e `rename` di una colonna su se stessa,
`reorder_columns` che non sposta niente; `on_division_by_zero` di `formula`
ed `expression` senza divisioni; i nomi d'uscita che farebbero sparire una
colonna scritta prima: aggregazioni con lo stesso nome o con il nome di una
chiave di `aggregate`, statistiche e parti ripetute di `statistics` e
`date_extract`, due voci di `mask_data` sulla stessa colonna senza
`overwrite`; una voce vuota nell'`index_col` di `pivot`; un campo
sconosciuto dentro un nodo di `expression`; un `null` esplicito per un
parametro facoltativo, che si omette invece di scriverlo `null`). Si
rifiuta solo ciò che la config da sola rende senza effetto con ogni
ingresso: un parametro che non ha effetto soltanto su certe tabelle si
accetta, perché lo stesso piano deve girare su tabelle diverse (`default`
di `align_schema` su una colonna che esiste, `keep_extra` senza colonne non
dichiarate, `drop_columns` e `rename` di colonne assenti, `alphabetical`
con al più una colonna restante, `type_policy` su colonne omogenee,
`separator` di `table_diff` con le colonne ricavate dagli schemi). Un
parametro assente prende il suo default; uno scritto e senza effetto si
rifiuta, nell'analisi e nel kernel, con la stessa funzione
(`verifica_parametri`, `verifica_offset`, `verifica_ascending`,
`verifica_gruppi_con_nome`, `verifica_politiche`, `verifica_valore`,
`verifica_null_literal`, `verifica_separatore`, `verifica_colonne`,
`verifica_parti`, `verifica_risultati`, `nomi_uscita`; il `null` lo rifiuta
la deserializzazione, `plenora_core::json::mai_null`). Il `null` esplicito
si rifiuta in ogni config, tabellare e geo, anche nei campi annidati; i soli
campi dove `null` ha un significato proprio, dichiarato nella scheda, sono
`value` di `filter` e delle condizioni di `conditional` (il testo vuoto),
`result` e `default_value` di `conditional` (un valore d'uscita), `value`
di `fill_na` (riempie con null) e `default` delle colonne di `align_schema`
(la colonna di null). Il censimento di ogni campo di ogni
config tabellare è in `crates/plenora-pipeline/tests/censimento_parametri.rs`
e la parità analisi–kernel di ogni regola in
`crates/plenora-pipeline/tests/parametri_senza_effetto.rs`; il `null` di
ogni campo geo lo provano i test di
`crates/plenora-kernels-geo/src/analyze/config.rs`, e `null` contro assente
per ogni operazione con un campo facoltativo
`crates/plenora-pipeline/tests/null_nelle_config.rs`. Le
asserzioni vacue (`assert_not_null`, `assert_unique`, `assert_schema` senza
colonne, `assert_range` senza estremi, `assert_cardinality` senza vincoli,
`assert_metadata` senza chiavi, `conditional` senza condizioni, `sha256_hash`
e `stable_fingerprint` senza colonne); `melt` con variabile e valore
omonimi, `rename` con una sorgente ripetuta, `explode` con
`empty_policy=drop`; formati di data vuoti; `flatten_json` oltre
`max_columns`; `amount` di `date_add` che nessuna data sopporta, secondo
intercalare dell'ultimo giorno compreso (`dates::verifica_amount`); nomi
delle regole di `validate_rules` oltre 1024 byte; in `expression`, arietà
delle funzioni, pattern letterali di `regex_replace` (sintassi e
`max_regex_bytes`), testi letterali oltre `max_string_bytes` (anche nelle
liste di `in`), divisori
letterali zero e indici letterali negativi di `substring`, questi ultimi
solo dove la valutazione li guarderebbe (nessun argomento che li precede, o
la sostituzione, solo null).

**Testi e regex contro i limiti.** I testi e i pattern della config
(separatori, formati, valori sostitutivi, valori di `lookup`, `fill_na`,
`conditional`, `bin`, `align_schema`, pattern di `replace`, `assert_regex`,
`validate_rules`, `string_extract` ed `expression`) si confrontano con
`max_string_bytes` e `max_regex_bytes` in analisi. I testi che crescono con
i dati li controlla il kernel, con `ResourceLimit`, prima di pubblicarli:
`replace` con regex, `concat_columns`, `string_pad`, `text_normalize`,
`melt`, `transpose`, i testi calcolati da `expression` e `formula`, `concat`
di `aggregate` e `pivot`, `_diff_columns` e `_diff_old_values` di
`table_diff`, `extract_all` di `string_extract`, `mask_data`,
`flatten_json`, le etichette automatiche di `bin`; un pattern di
`regex_replace` calcolato dalle colonne si confronta con `max_regex_bytes`
riga per riga, e uno non valido rifiuta la riga
(`evaluation.invalid_regex`) senza il testo dell'errore del crate `regex`,
che riporterebbe il pattern, cioè una cella. Il testo scritto da un formato
di data non cresce con la cella: l'analisi ne limita la lunghezza con la
larghezza massima di ogni campo (i letterali per la loro lunghezza, l'anno 7
byte, il mese 2, il nome del mese 9…) e il kernel non la ricontrolla. I default dei due limiti sono uno solo per il
piano e per i kernel (`plenora_core::limits::DEFAULT_MAX_STRING_BYTES`, 16
MiB, e `DEFAULT_MAX_REGEX_BYTES`, 64 KiB; prima i kernel avevano 4096 byte
per le regex).

Il runner tiene un solo controllo proprio, perché non riguarda la config ma
l'ambiente del processo: la variabile `key_env` di `table.hmac_sha256`
deve esistere, non essere vuota ed essere UTF-8. La legge la stessa
funzione del kernel (`security::carica_chiave_hmac`), con una causa per
ciascuno dei tre rifiuti e senza il nome della variabile né il valore:
prima il runner accettava un valore non UTF-8 che il kernel poi trattava
come variabile assente.

## Esecuzione

Dopo ogni passo l'output del kernel deve avere nomi, tipi e metadati (di
campo e di schema) del contratto inferito (altrimenti `Internal`) e riceve lo schema del contratto; righe
per arco, colonne, nomi ripetuti e fattore di espansione si controllano
sui dati; il fattore, sulla base che il catalogo dichiara per l'operazione
(`expansion_constraint`), non per quelle che il catalogo ne esenta (righe
da tutto l'ingresso, come `polygonize`, o in numero fisso, come `dissolve`
e `reconcile`) né per `melt`, le cui righe sono le righe d'ingresso per le
colonne valore, fissate da config e schema: al posto del fattore il runner
verifica che l'uscita abbia esattamente quelle righe
([«Fattore di espansione»](#fattore-di-espansione)). Ogni tabella si libera appena ha girato il suo ultimo
consumatore; un'uscita che nessuno usa si libera subito, un input mai usato
prima del primo passo.

Il resoconto dà per passo operazione, righe in ingresso e in uscita,
picco previsto, margine passato al kernel, byte nuovi dell'output
(allocazioni che nessuna tabella residente raggiungeva prima del passo),
byte vivi con l'output e dopo i rilasci, tabelle liberate, e le righe in cui una divisione di `formula` o
`expression` ha trovato un divisore zero (`righe_divisione_per_zero`, un
conteggio senza valori: [«Divisione per zero»](#divisione-per-zero)).
`byte_vivi` (`plenora_core::memoria`) somma le allocazioni
Arrow delle tabelle residenti una volta ciascuna, per inizio
dell'allocazione e capacità, figli compresi: una slice, una rinomina o le
colonne di un batch letto da Arrow IPC non aggiungono nulla.

## Scadenza e annullamento

`PipelineValidata::run_interrompibile(tabelle, &Interruzione)` è `run`
con una scadenza (`Instant`, assoluta) e un segnale di annullamento
(`Arc<AtomicBool>`, alzato da un altro thread), entrambi facoltativi; `run`
è `run_interrompibile` senza nessuno dei due. Il runner li controlla prima
di ogni passo e prima di consegnare gli output:

- annullamento alzato: `Cancelled` (categoria `cancelled`, codice
  `EXECUTION_CANCELLED`);
- scadenza passata: `Timeout` (categoria `timeout`, codice
  `EXECUTION_DEADLINE_EXCEEDED`, come il vettore `data-run-timeout-error`
  del contratto);
- con entrambi vince l'annullamento.

Il testo dice dove («prima del passo `x` (table.sort)», «prima di
consegnare gli output»), la fase anche: `write` prima di un passo,
`finalize` prima della consegna. Nessun output è reso, quindi effetto `none` e
ritentativo `safe`: rieseguire lo stesso piano è deterministico (se
ritentare dopo un annullamento voluto lo decide il chiamante). Una scadenza
RFC 3339 (`plenora.execution.deadline` del binding di runtime) la converte
in `Instant` chi la riceve. Con una scadenza l'esito (output o `Timeout`) dipende
dal tempo, per natura; gli output resi sono sempre quelli di `run`. Il controllo è fra i passi, mai dentro un
kernel (limite in [«Limiti dichiarati del runner»](#limiti-dichiarati-del-runner)).

## Divisione per zero

**Semantica dichiarata** (decisione dell'utente): in `table.formula` e
`table.expression` una divisione con operandi non null e divisore zero
vale null di default; il piano chiede l'errore con
`"on_division_by_zero": "error"`. Non è un null silenzioso: il kernel conta
le righe in cui è successo e il runner le riporta nel resoconto del passo
(`ReportPasso::righe_divisione_per_zero`), con entrambe le politiche (con
`error` un passo riuscito ne ha zero, perché la prima lo fa fallire con la
diagnostica per riga `evaluation.division_by_zero`).

- In `expression` il null è quello del nodo della divisione e segue le
  regole dei null: `coalesce(a / b, 0)` dà 0, un ramo di `case` non scelto
  non si valuta e non conta; una riga con più divisioni per zero conta una
  volta. In `formula` ogni operatore propaga il null, quindi la riga intera
  diventa null.
- Un divisore letterale zero (`x / 0`, `x / -0.0`) resta un errore di piano
  con ogni politica, in validazione (per `expression` prima lo vedeva solo
  il kernel, in esecuzione).
- `on_division_by_zero` scritto in una formula o un'espressione senza
  divisioni si rifiuta, come ogni parametro senza effetto; un valore diverso
  da `"null"` ed `"error"` si rifiuta dalla config.
- Non ci sono modulo né divisione intera: `/` è l'unica divisione.
  `power(0, -1)` (infinito) e un quoziente che trabocca restano risultati non
  finiti (`evaluation.non_finite_result`), non divisioni per zero, con
  qualunque politica. In `formula` un risultato non finito da operandi
  finiti (overflow, anche intermedio come `a / (a * a)`) rifiuta la riga
  come in `expression`; un `NaN` o un infinito già nella colonna si propaga,
  come in `window_function` e `rolling_window`, che rifiutano anch'esse
  l'overflow da valori finiti; un `result` di `conditional` che si legge
  come numero non finito si rifiuta in validazione.

`table.formula` emette diagnostica per riga solo con `"error"` (la
divisione per zero è il suo unico rifiuto per riga), e il catalogo lo
dichiara (`emits_row_diagnostics`). Semantica 3 per `formula`, 4 per
`expression`.

## Fattore di espansione

`max_expansion_factor` (default 100) si controlla dopo il passo, sulle
righe dell'uscita già costruita: non difende la memoria, che difendono il
budget prima e dopo il passo, i preflight dei kernel con il margine e
`max_rows_per_edge` (10 milioni). È una guardia logica contro
un'espansione che nessun piano sensato chiede: un join molti-a-molti su una
chiave sbagliata, un prodotto cartesiano involontario, liste esplose più
lunghe del previsto.

Un left join reale di un arricchimento 1:N è stato rifiutato a 106 volte:
il vincolo dei join era `MaxRelative`, cioè l'uscita sul lato **minore**.
Quella base misura l'asimmetria dei lati, non l'espansione: un left join
di 10 600 righe su una dimensione di 100 righe con la chiave unica vale
106 volte la destra senza duplicare una riga, e ogni arricchimento su una
tabella con meno dell'1% delle righe dell'altra superava il default. Ora
`table.join`, `table.cross_join`, `table.fuzzy_join`, `geo.sjoin` e
`geo.overlay` misurano sulla **somma** dei lati (`SumRelative`), come le
altre operazioni a due ingressi senza una base propria (`clip` e le
booleane allineate sono `LeftRelative`):

- un abbinamento con la chiave unica su almeno un lato (1:1, 1:N, N:1) vale
  al più 1: l'uscita di un inner join è al più il lato maggiore, quella di
  un left o outer join al più la somma dei lati. Nessun arricchimento
  legittimo si avvicina a 100;
- un molti-a-molti vale `Σ l_k · r_k / (L + R)`: con 300 righe per lato
  sulla stessa chiave, 90 000 righe su 600, cioè 150, e si rifiuta; un
  `cross_join` di L per R righe vale `L·R/(L+R)`, circa il lato minore
  (10 000 per 5 scenari: 5, accettato; prima valeva 10 000).

Il default resta 100: dopo il cambio di base supera 100 solo un'uscita che
moltiplica i dati per più di cento volte la loro somma, e con gli ingressi
oltre 100 000 righe il tetto effettivo è già `max_rows_per_edge`.

**Portata della guardia.** Con la somma come base, un prodotto completo
di L per R righe vale `L·R/(L+R)`, cioè circa il lato minore: con il lato
minore di al più 100 righe il fattore 100 non ferma mai un prodotto
completo (100 per 50 000 righe danno 5 milioni di righe, fattore 99,8,
accettato). Il fattore ferma i molti-a-molti fra lati entrambi grandi; la
memoria e le uscite enormi le fermano il budget (prima del passo, con il
modello di costo, e dopo, con i byte veri), i preflight dei kernel
(`cross_join` e `fuzzy_join` contano le coppie prima di allocare) e i
limiti assoluti di righe (`max_rows_per_edge`, `max_output_rows`). Derivarlo
dal budget non avrebbe senso: quando il fattore si controlla la memoria è
già stata allocata e contata. Un piano che vuole un'espansione maggiore la
dichiara (`limits.max_expansion_factor`). Le operazioni con righe fissate
dalla config ne sono fuori (esenzioni del catalogo, e `melt` con la verifica
esatta sopra). Semantica 2 per i cinque join e per `melt` (un'uscita prima
rifiutata ora si produce).

## Diagnostica per riga

**Regola**: ogni ordine valido dei passi si accetta; un payload
`plenora-row-diagnostics-v1` ha solo indici di riga della sorgente
(`index_basis` `source_row_zero_based`, DIAG-002) e, dove la riga della
sorgente non si conosce, nessun indice (DIAG-003: mai un indice indovinato
o di un'altra base).

Un kernel riporta gli indici delle righe del suo primo ingresso (l'unico
per le unarie, il lato left per `assert_foreign_key`). Se quelle righe
siano della sorgente lo decide la validazione, dal catalogo
(`source_row_provenance`), e `PipelineValidata::base_indici(out)` lo dice
prima di eseguire:

- **`BaseIndici::Sorgente`**: l'ingresso discende da un input del piano
  solo attraverso passi che conservano numero e ordine delle righe
  (`rename`, `type_cast`, `formula`…). L'indice è la riga di quell'input,
  da zero; payload e testo sono quelli del kernel.
- **`BaseIndici::SenzaAttribuzione`**: a monte c'è un passo che filtra,
  riordina, espande, unisce o aggrega (`filter`, `sort`, `limit`, `sample`,
  `distinct`, `join`, `aggregate`, `explode`…). Il runner toglie gli esempi
  e tiene conteggi, cause e totale: la completezza `complete` diventa
  `partial` con il limite di conoscenza `read.row_attribution_unavailable`
  e `examples_truncated` vero (DIAG-007: esempi osservati omessi)
  (`RowDiagnostics::senza_attribuzione`). Il testo del kernel prende
  davanti «passo `<out>`: righe rifiutate nell'ingresso `<nome>` del passo,
  non riconducibili alla sorgente (diagnostica senza esempi)». Per trovare
  le righe si esegue a parte il piano spezzato: il prefisso fino a `<nome>`
  e poi il passo su quell'output come input, che ha gli esempi rispetto a
  `<nome>`. Dopo un'aggregazione o un pivot la riga rifiutata è un gruppo
  nuovo, e una sola riga d'origine può non esistere.

La «sorgente» è la tabella d'ingresso del piano: un piano spezzato in due
riporta gli indici del secondo rispetto ai suoi input, cioè alle uscite del
primo. Gli indici non si ricalcolano mai verso la sorgente attraverso un
passo che cambia le righe: nessuna mappa di righe si tiene in memoria, e
il budget non ha niente in più da contare (limite sotto, «Diagnostica senza
esempi dopo un passo che cambia le righe»).

Fase e scope di un rifiuto per riga: l'errore ha la fase derivata
`write` (`ErrorPhase`: l'esecuzione di un passo, il canone non ha una fase
«execute»), mai `read`, perché nessun kernel legge un supporto; lo `scope`
del payload è `read`, l'unico che il contratto v1 dà a un rifiuto di un
kernel: dice che la riga rifiutata è d'ingresso, non che si stava leggendo
un file.

## Operazioni geo

I passi `geo.*` passano dall'analisi dei kernel (`analyze_geo_contract`) e
dai kernel di `plenora-kernels-geo`, con lo stesso budget e gli stessi
controlli dopo il passo delle tabellari. Un passo geo
è una funzione `RecordBatch` → `RecordBatch` (`plenora_pipeline::geo`,
privato): calcola le colonne del contratto d'uscita, nel suo ordine, e le
monta sul suo schema. Niente fusione, niente streaming.

| forma | operazioni | uscita |
| --- | --- | --- |
| 1:1 in place | `centroid`, `convex_hull`, `envelope`, `boundary`, `point_on_surface`, `buffer`, `simplify`, `affine_transform`, `translate`, `scale`, `rotate`, `concave_hull`, `densify`, `snap_to_grid`, `line_substring`, `line_interpolate_point`, `snap`, `reproject` | la geometria della riga, attributi invariati |
| colonne in coda | `area`, `length`, `perimeter`, `geodesic_line_length`, `geodesic_area`, `vertex_count`, `to_wkt`, `bounds_extractor`, `geometry_accessors`, `line_locate_point`; contro la geometria `other_wkb` della config: `distance`, `hausdorff_distance`, `frechet_distance`, `haversine_distance`, `geodesic_distance`, `bearing`, i predicati `predicate_*` | la misura della riga, null per una geometria null |
| sostituzione | `geometry_diagnostics` | le dieci colonne diagnostiche al posto della geometria |
| produttori | `from_coords`, `from_wkt` | colonna geometria in coda, CRS da `crs` della config o di piano |
| espansioni 1:N | `explode`, `delaunay`, `subdivide`, `split` (lama `other_wkb` su ogni riga) | una riga per parte, attributi della riga madre, `__parent_index`; una geometria null non produce righe (`explode`, `delaunay`, `split`) o una riga null (`subdivide`), come a `190c493` |
| in place, su tutta la tabella | `make_valid`, `voronoi`, `clean_topology` | un risultato per riga non null, null le altre; in `clean_topology` una riga assorbita da una precedente diventa null (la geometria dell'uscita è nullable anche quando quella d'ingresso non lo è) |
| colonna in coda, su tutta la tabella | `cluster_dbscan` | etichetta del cluster, null per il rumore |
| aggregazioni a sole geometrie | `dissolve`, `line_builder`, `polygon_builder` (una riga), `line_merge` (una per linea fusa), `polygonize` (con `__class`), `collect` (una per gruppo, con le colonne chiave) | nessun attributo propagato |
| coperture | `coverage_validate`, `shared_paths` | schema nuovo, una riga per problema o tratto |
| griglia | `generate_grid` | le celle; l'ingresso fa solo da innesco |

Le operazioni su due tabelle (left, right; il catalogo chiede lo stesso
CRS proiettato sui due lati) hanno questa semantica delle righe, quella
di `190c493` (`execute_geo_binary` dell'executor per i join, `pair_arrow`
per ritaglio, overlay e booleane, che il DAG d'origine non eseguiva) e
quella che il contratto dell'analisi dichiara:

| operazioni | righe dell'uscita |
| --- | --- |
| `sjoin` (`predicate`), `nearest` (`max_distance`) | una per coppia trovata: le colonne di left di quella riga, `__right_index` (e `distance`), non nullable come la geometria di left nell'uscita; una riga di left senza coppie (o a geometria null) non compare |
| `within`, `count_points_in_polygons` | allineate a left: la colonna in coda (`within`: la geometria di left è dentro una di right; conteggio dei punti di right in ogni poligono di left), null per una geometria di left null |
| `clip` | allineata a left: ogni geometria ritagliata dall'unione di **tutte** le geometrie di right (la maschera), null dove il ritaglio è vuoto |
| `overlay` (`mode`) | una per pezzo: la geometria e le righe d'origine `__left_index`, `__right_index` (null dove il pezzo non viene da quel lato); nessun attributo |
| `intersection`, `union`, `difference`, `symmetric_difference` | allineate: riga `i` di left con riga `i` di right, **stesse righe richieste** (altrimenti `InvalidPlan`, in esecuzione: le righe non si conoscono a secco); null dove uno dei due è null o il risultato è vuoto |

Per `clip` e le quattro booleane un risultato vuoto è null: l'analisi
dichiara ora la geometria dell'uscita nullable anche quando quella di
left non lo è. Per `sjoin` e `within` il tetto delle coppie è il limite
di righe dell'arco, per `nearest` i confronti sono al più il quadrato del
maggiore fra `max_input_rows` e `max_rows_per_edge` (come nel progetto d'origine).

`collect` ordina i gruppi nell'ordine naturale dei valori delle chiavi,
con il comparatore di `table.sort` (`compare_cells_typed`: numeri per
valore, testo per byte, istanti per istante, null dopo i valori); a
`190c493` era l'ordine di una chiave testuale con la lunghezza in testa
(un valore di 10 caratteri prima di uno di 9). Resta un errore dei dati,
in esecuzione, una chiave di dizionario fuori dal dizionario.

**Config.** Si legge una volta, in validazione, con i tipi dell'analisi
(`plenora_kernels_geo::analyze::config`, pubblici per questo): nessuna
seconda copia di nomi e default. Le geometrie della config (`other_wkb`,
`point_wkb`, `reference_wkb`) si decodificano lì, già accettate
dall'analisi, che per `other_wkb` ora verifica anche la validità OGC come
per le altre due, e il tipo che il kernel chiede (`LineString` per
`frechet_distance`; `Point` per `haversine_distance`,
`geodesic_distance`, `bearing`): un kernel l'avrebbe rifiutata alla prima
riga non null, e su una tabella vuota o tutta null mai. Allo stesso modo
l'analisi di `collect` rifiuta le chiavi `group_by` senza un ordine
naturale (`is_sortable` dei kernel tabellari, come `table.sort`). La
validazione rifiuta esattamente ciò che l'analisi rifiuta (test
`la_validazione_rifiuta_esattamente_cio_che_l_analisi_rifiuta`); in più
solo `Unsupported` per le operazioni senza dispatch.

**Prima del kernel**, per ogni colonna geometria d'ingresso: ogni cella
si decodifica (contratto WKB strutturale) e ogni coordinata deve stare nel
dominio di validità del CRS della colonna (`Crs`, [«CRS
integrati»](crs.md#crs-integrati): i kernel non ricevono un CRS); se il
contratto dichiara i tipi geometrici con un elenco (`exact`, o `mixed`
con elenco), ogni cella deve essere di un tipo dichiarato (`Schema`). La
validazione OGC la fa il kernel, una volta per geometria; la decodifica
strutturale del controllo di dominio è quindi una seconda passata sui
byte, il prezzo di un controllo in un posto solo. Le geometrie prodotte
da `from_coords` e `from_wkt` stanno nel dominio del CRS dell'uscita.

**Precisione.** I kernel che la chiedono (`buffer`, `subdivide`, `split`,
`make_valid`, `dissolve`, `polygonize`, `voronoi`, `clean_topology`,
`coverage_validate`)
ricevono 1 cm a terra nelle unità del CRS della colonna
(`Precision::from_crs`, [«Precisione delle operazioni
geografiche»](limiti.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)),
calcolata in validazione.

**Dopo il kernel.** Una geometria null in una colonna che il contratto
dichiara non nullable è un errore del passo (`InvalidPlan`): per esempio
`point_on_surface` di una geometria vuota, o `from_coords` con una
coordinata null (il contratto dichiara la geometria prodotta non
nullable; a `190c493` diventava null). Ogni geometria prodotta deve avere
un tipo che il contratto d'uscita dichiara, altrimenti `Internal`
(analisi e kernel divergono).

**Limiti passati ai kernel**: il limite di righe dell'arco d'uscita
(`max_output_rows` per un output del piano, `max_rows_per_edge`
altrimenti, come nel progetto d'origine) come tetto delle righe prodotte da
espansioni, `line_merge` e `polygonize`; `MAX_CLEAN_VERTICES` e
`MAX_NODING_WORK` dei kernel; `100_000` punti per `voronoi` senza
`max_points`; `MAX_CELL_COORDINATES` per `concave_hull`, `densify` e
`delaunay`, `10^8` coppie di coordinate per riga per `hausdorff_distance`
e `frechet_distance` (l'ordine di `MAX_NODING_WORK`); una `Int64` di
`from_coords` oltre `2^53` in modulo si rifiuta (non è esatta in `f64`).

**Errori.** Categoria come nei kernel: `Internal` ciò che non ha concluso
o un'invariante violata, `Unsupported` la precisione insufficiente,
`ResourceLimit` un limite di lavoro, d'uscita o di coppie superato dai
dati (la definizione di `PlenoraError::ResourceLimit`: il piano è
corretto, sono i dati a non entrarci; a `190c493` e fino alle versioni di
catalogo precedenti a questo ciclo erano `InvalidPlan`), `InvalidPlan` il
resto (una config illeggibile anche per `geo.reproject`, che rendeva
`InvalidConfiguration`), con il nome dell'operazione e del passo; il testo
è quello dei kernel, senza valori.
Il primo errore è quello della prima riga, in ordine di riga.

**Diagnostica per riga.** Delle geo che il catalogo dichiara con
diagnostica per riga, nel runner la emette solo `from_wkt` (l'adapter dei
kernel), con la base degli indici delle tabellari
([«Diagnostica per riga»](#diagnostica-per-riga)); le altre rendono il
primo errore, senza indici di sorgente (limite sotto).

## Budget di memoria

`limits.max_governed_memory_bytes` del piano è il budget del runner. Tutto
sta in memoria: niente va su disco. Due regole lo tengono:

- **vivibilità**: ogni tabella si libera appena ha girato il suo ultimo
  consumatore ([«Esecuzione»](#esecuzione)), e `byte_vivi` conta esatti i
  byte delle tabelle residenti (`plenora_core::memoria`);
- **rifiuto prima di eseguire**: prima di ogni passo deve valere

```text
byte_vivi(residenti) + picco_previsto(passo) <= budget
```

altrimenti il passo si rifiuta con `ResourceLimit`, con il nome del passo e
dell'operazione e senza valori dei dati, prima che il kernel giri.

**Un passo che non sta nel budget si rifiuta; non c'è ripiego su disco.**
Fino al commit `47623ce` il runner, prima di rifiutare, sfrattava su file
Arrow IPC temporanei le tabelle residenti che il passo non usava e, per
`sort`, `distinct`, `aggregate` e le set operation, passava a una variante
dei kernel che scriveva su disco. È un cambiamento osservabile: un piano
che allora riusciva grazie allo sfratto o alla variante su disco oggi si
rifiuta con `ResourceLimit` prima del passo che non sta (le uscite dei
piani che riuscivano in memoria non cambiano). Il rimedio è un budget più
grande: i dati reali per cui il runner è pensato (il portafoglio più
grande, circa 3,3 milioni di righe in ingresso, ha un picco di circa
0,7 GiB) stanno in memoria.

Il picco previsto, per operazione, è

```text
picco_previsto = S * (a + max(r*R + c*B, r_s*R, c_l*B) + k*K + p*P)
```

dove `R` sono le righe di tutti gli ingressi (per `geo.generate_grid` le
celle d'uscita, note a secco), `B` i byte Arrow degli ingressi, `K = R *`
colonne del contratto d'uscita, `P` righe sinistra per righe destra, e
`S = 1.5`. I coefficienti vengono dalle misure Windows v4
(`PeakWorkingSet64` incrementale, profili wide, narrow, distinct avversario
fino a 5 milioni di righe per le tabellari; per le geo profili
default e avversari su feature per vertici, al livello del runner: colonne
GeoArrow-WKB, decodifica con validazione OGC, kernel, codifica) in
`data/misure/catalogo-memoria-v4.json`, estratte dal catalogo della
campagna con la sua provenienza (commit misurato, date, macchina, carico,
SHA-256 del catalogo d'origine; `python scripts/modello_costi.py --estrai
<catalog-v4.json>`). Li generano `python scripts/genera_costi_operazioni.py`
in `crates/plenora-pipeline/src/costi_operazioni.rs` e `python
scripts/genera_costi_geo.py` in `crates/plenora-pipeline/src/costi_geo.rs`,
con le regole di `scripts/modello_costi.py` (`--verifica` rigenera e
confronta; un test confronta l'impronta delle misure). Il catalogo tiene
anche i profili delle varianti dei kernel che scrivevano su disco
(`spilled_*`): restano nelle misure, ma i generatori non li usano. Per ogni
punto osservato `y = max(stima di budget, byte nuovi dell'output, 0)`:

- **piano** `a + r*R + c*B` (con `k*K` per `table.pivot`, con il solo
  `p*P` per `cross_join` e `fuzzy_join`): fra quelli che coprono **ogni**
  punto di **tutti** i profili in memoria dell'operazione, quello con la
  somma minima dei rapporti previsto/misurato (programma lineare risolto esattamente),
  con `a` non oltre il picco più piccolo misurato (la crescita la portano i
  termini per unità) e `c >= 1` dove l'uscita può essere una copia intera
  degli ingressi anche se le fixture ne tengono una parte (sottoinsiemi di
  righe, join senza espansione delle chiavi, chiavi indice di `pivot`);
- **rami di larghezza**: `r_s` inviluppo `max y/R` della classe di
  larghezza di riga (`B/R`) più stretta, `c_l` inviluppo `max y/B` della
  più larga (le classi sono i profili, e per le geo profilo per vertici per
  geometria), sui picchi interi, senza togliere `a`: il programma lineare
  può spostare in `a` parte di un costo che è per riga, e un inviluppo su
  `y - a` non coprirebbe più le righe strette (controesempio verificato a
  ogni generazione, `verifica_controesempio_rami`). Se il costo vero è
  `r1*R + c1*B` con `r1, c1 >= 0`, per un punto misurato di larghezza `w_i`
  vale `y_i/R_i = r1 + c1*w_i`: su righe più strette il costo è al più
  `(y_i/R_i)*R <= r_s*R`, su righe più larghe al più `(y_i/B_i)*B <=
  c_l*B`. Con una sola classe `max(r_s*R, c_l*B)` è per eccesso a ogni
  larghezza;
- coefficienti in millesimi di byte, per eccesso; un'operazione senza
  modello si rifiuta in validazione (`Unsupported`).

L'oracolo `crates/plenora-pipeline/tests/oracolo_costi.rs` verifica
l'invariante su ogni punto osservato: `a + max(...) + ...` senza `S` non è
sotto il picco misurato, quindi la previsione è almeno una volta e mezza la
misura. Fanno eccezione solo i profili avversari geo esclusi per nome
(limite «Modelli di costo geo»). Sugli 879 punti coperti di almeno 1 MiB il
rapporto previsto/misurato con `S` ha mediana 2,92, novantesimo percentile
15,3 e massimo 119 (`geo.predicate_contains`, profilo default); con i
modelli precedenti (v3 per le tabellari, provvisori per le geo, varianti
su disco comprese) era 6,4, 41,8 e 535, con 30 punti sotto la misura senza
`S`.

Da dove veniva il pessimismo dei modelli v3: `max(r*R, c*B)` con `r` e `c`
presi ciascuno dal profilo peggiore. Il `c` di `join` (5,8 byte per byte)
veniva dal profilo narrow, dove 26 byte per riga di input portano 80-100
byte per riga di tabelle hash e indici: applicato a righe reali da 500 byte
e oltre, trasformava un costo per riga in un costo per byte, circa cinque
volte l'input. Il `r` di `pivot` (745 byte per riga) veniva dal profilo
distinct, 64 colonne pivot e un indice tutto distinto: il costo lo fanno
le celle d'uscita, non le righe, e `k*K` le conta sulle colonne del
contratto d'uscita. Il piano additivo mette il costo per riga in `r`,
quello per byte in `c`, quello per cella in `k`.

Anche lo stato iniziale (gli input residenti) e quello finale (gli output
insieme) sono confini: oltre il budget sono un `ResourceLimit`, anche in un
piano senza passi.

Il kernel riceve come `max_governed_memory_bytes` il margine vero, budget
meno byte vivi, così i suoi preflight usano lo spazio che c'è. Lo stesso
margine va ai passi geo (`esegui_kernel` lo passa a `PassoGeo::esegui`),
che lo danno ai kernel i cui risultati crescono con i dati come
`plenora_kernels_geo::margine::MargineMemoria` (limite «Modelli di costo
geo»).

`B` del modello è il maggiore fra i byte vivi degli input e il costo
di una loro copia (`plenora_core::memoria::byte_dati`: colonne che sono lo
stesso array contano ciascuna, perché i kernel le copiano ciascuna). Dopo
il passo, byte vivi con l'output oltre il budget sono un `ResourceLimit`
esplicito.

## Limiti dichiarati del runner

- **Tabelle intere in memoria**: nessuno streaming, nessun batch parziale,
  niente su disco. Un passo che non sta nel budget si rifiuta prima di
  eseguirlo ([«Budget di memoria»](#budget-di-memoria)); fino al commit
  `47623ce` lo stesso piano poteva riuscire sfrattando tabelle su file
  temporanei o con le varianti su disco dei kernel.
- **`byte_vivi` esatto solo per la memoria allocata da Rust.**
  *Regola*: si conta ogni allocazione una volta, per inizio
  (`Buffer::data_ptr`) e capacità (`Buffer::capacity`); escluse le
  strutture Rust (`ArrayData`, `Arc`, schemi) e l'overhead dell'allocatore.
  *Ambito*: `plenora_pipeline::byte_vivi` e i byte del resoconto.
  *Hazard*: per la memoria esterna (FFI, `bytes::Bytes`) Arrow dichiara come
  capacità la vista importata, non l'allocazione: viste con inizi diversi
  della stessa allocazione esterna si sommano, la parte fuori dalle viste
  non si conta, a parità di inizio vale la maggiore. Il conto può essere
  sbagliato in entrambe le direzioni, senza errore. Le tabelle passano a
  `run` per valore: un clone tenuto dal chiamante tiene vive allocazioni
  che il resoconto non vede.
  *Rientro*: un'API di Arrow che distingua la deallocazione `Custom`, o la
  copia delle tabelle esterne in memoria Rust all'ingresso (non fatta in
  F3: il budget conta le tabelle esterne come le vede `byte_vivi`).
- **Budget di memoria: byte Arrow più transitorio previsto, non RSS.**
  *Regola*: il budget garantisce che, a ogni confine di passo, i byte vivi
  delle tabelle residenti stiano sotto `max_governed_memory_bytes`, e che
  prima di ogni passo i byte vivi più il picco previsto dal modello ci
  stiano; l'output si controlla dopo con i byte esatti.
  *Ambito*: `PipelineValidata::run`, modelli in
  `crates/plenora-pipeline/src/costi_operazioni.rs` e `costi_geo.rs`.
  *Hazard*:
  - non è un tetto duro sulla memoria del processo: il transitorio dentro
    il kernel è una previsione empirica (misure Windows di
    `PeakWorkingSet64`, fattore 1,5), non una misura. Il modello copre ogni
    punto misurato (oracolo `oracolo_costi.rs`), ma un input fuori dalle
    fixture misurate (più righe di 5 milioni, 1000 per lato per
    `cross_join` e `fuzzy_join`, distribuzioni di chiavi o config diverse
    da quelle misurate: una `type_cast` di tutte le colonne invece di una)
    può superarla senza errore;
  - fuori dalle larghezze di riga misurate per l'operazione (nella maggior
    parte delle fixture tabellari fra 8 e 150 byte per riga d'ingresso; le
    righe reali arrivano a qualche KiB) la previsione è per eccesso solo se
    il costo vero è una somma non negativa di un termine per riga e uno per
    byte, senza costante oltre `a` (rami `r_s` e `c_l`); un costo che cresce più che linearmente con la
    larghezza di una cella non è coperto;
  - la campagna v4 (`data/misure/catalogo-memoria-v4.json`, campo
    `caveat`) è stata fatta su una macchina carica (CPU media 68 % e fino a
    40 processi di build nella campagna tabellare): la memoria è
    affidabile, i tempi no. È misurata a `24698d6`, prima delle parti
    preparate della validazione OGC (`fd3325c`, `2697f72`: una geometria a
    più parti in validazione trattiene qualche centinaio di byte per
    vertice, per ogni cella in decodifica) e del runner geo di F4: quella
    memoria non è nelle misure, la copre solo il fattore 1,5;
  - i punti che il kernel ha rifiutato per i suoi limiti (`reconcile`,
    `concat` e `concat_by_name` wide) non entrano nel modello;
  - `cross_join` e `fuzzy_join` hanno solo il termine per coppia,
    calibrato sulle larghezze di riga misurate: con righe più larghe il
    picco previsto è basso, e la difesa è il preflight dell'output del
    kernel con il margine passato;
  - `pivot` conta le celle d'uscita come righe in ingresso per colonne
    d'uscita (per eccesso: le righe d'uscita sono le chiavi indice
    distinte, al più le righe in ingresso, che a secco non si conoscono).
    Le colonne sono quelle del contratto validato, che il runner accetta
    solo con `mapping` e che il kernel produce esattamente (una per voce,
    anche se i dati non la contengono; l'esecuzione verifica lo schema);
    le misure hanno da 3 a 65 colonne d'uscita, oltre è estrapolazione
    lineare nelle celle;
  - per `date_extract`, `lookup` e `string_length` (solo profilo wide) la
    crescita per riga fra gli ultimi due campioni supera 1,5: il modello
    resta lineare e copre il campione più grande;
  - esclusi: overhead dell'allocatore e strutture Rust; la memoria esterna
    (FFI) contata come la vede `byte_vivi` (limite sopra);
  - per le operazioni che dipendono dai dati (join, `cross_join`,
    `fuzzy_join`, `pivot`, `transpose`, `explode`, `unnest`, `melt`,
    `aggregate`, `dedup_advanced`, finestre, `flatten_json`) il modello
    copre il caso peggiore misurato; l'output lo limitano i preflight dei
    kernel con il margine passato e `max_rows`, e il controllo esatto dopo
    il passo, cioè dopo che è stato allocato. `c >= 1` copre un join senza
    espansione delle chiavi, non uno molti a molti;
  - un rilascio libera memoria solo se nessun altro tiene l'allocazione:
    un clone tenuto dal chiamante la tiene viva, e il resoconto non lo vede.

  *Rientro*: contabilità esplicita delle strutture di chiavi nei kernel
  (limite «Memoria delle chiavi dei kernel in memoria non governata»),
  misure di righe più larghe e dei casi oltre il dominio misurato, una campagna a macchina scarica sul
  `main` corrente, e un allocatore contato per il processo (un tetto vero,
  non una previsione).
- **Modelli di costo geo.**
  *Regola*: ogni operazione geo ha un modello generato dalle misure v4 al
  livello del runner, con le stesse regole delle tabellari; alcuni profili
  avversari ne sono esclusi per nome (`esclusi` in `costi_geo.rs`, motivi
  in `scripts/genera_costi_geo.py`): `buffer` su linee a zig-zag,
  `count_points_in_polygons`, `sjoin` e `within` con ogni coppia
  candidata, `overlay` e `coverage_validate` su sovrapposizioni, `nearest`
  con pareggi.
  *Ambito*: `crates/plenora-pipeline/src/costi_geo.rs`, tutte le `geo.*`.
  *Hazard*: la memoria dei profili esclusi cresce con una grandezza che il
  runner non conosce prima del passo (coppie candidate, pezzi
  d'intersezione, vicini equidistanti, forma della geometria): coprirli
  renderebbe il modello di ordini di grandezza più alto sui profili
  ordinari (per `sjoin` il default fino a 1500 volte la misura). Su quei
  profili la previsione arriva fino a circa 200 volte sotto la misura (mediana
  0,23 con `S`); il `buffer` a zig-zag da 1000 vertici per geometria ha
  misurato 20 GiB su 15 MiB d'ingresso. I punti andati oltre il tempo
  massimo della campagna (`buffer`, `make_valid`, `split`) o
  rifiutati (`line_merge`, `polygonize`, `voronoi`) non sono nel modello.

  **Modelli non rigenerati dopo il buffer a blocchi e la revisione di split.**
  Le misure di questo ciclo (laboratorio della campagna v4 sul codice
  corrente, Windows, 32 thread, mediana di 5 del `PeakWorkingSet64` oltre
  quello di un processo con gli stessi ingressi) non sono nel catalogo e
  i modelli restano quelli v4:
  - il `buffer` a zig-zag ora sta sotto il modello del profilo ordinario
    (1.000 linee da 1.000 vertici: 117 MB contro 398 MB senza `S`; 10.000:
    832 MB contro 3,9 GB), ma resta escluso finché una campagna non ne
    rigenera le misure;
  - `split`, a un thread e con le parti nel buffer contiguo, sta sotto il
    modello senza `S` su ogni punto rimisurato: lame a zig-zag 1.000 righe
    da 100 vertici 17 MB contro 39 MB, 1.000 da 1.000 vertici 115 MB
    contro 386 MB, 10.000 da 1.000 vertici (prima oltre il tempo massimo)
    1,9 GB contro 3,9 GB; la lama singola 10.000 da 1.000 vertici sotto il
    rumore della misura contro 1,9 GB;
  - `make_valid` sulle stelle invalide a 10.000 righe da 1.000 vertici,
    prima oltre il tempo massimo, 2,2 GB contro 2,4 GB senza `S`.

  La dipendenza del transitorio dai thread vale per ogni kernel per riga
  in parallelo: le misure v4 sono a 32 thread, e una macchina con più
  core ne trattiene di più (il buffer nel runner ne ha al più 64 in volo).
  **Guardia di memoria nei kernel: riduce il rischio, non è un tetto.** I
  kernel di questi profili ricevono il margine del passo
  (`plenora_kernels_geo::margine`, budget meno byte vivi) e contano le
  allocazioni **più grandi** del nostro codice, dove crescono con i dati: le
  geometrie decodificate degli ingressi (stimate dalle intestazioni del WKB
  prima di decodificarle), le coppie confermate di `sjoin`, `within` e
  `count_points_in_polygons` (con la riga più larga di left ripetuta
  nell'uscita, mai la media), i vicini equidistanti di `nearest`, le
  coppie candidate e i pezzi di `overlay`, le coppie candidate e le
  sovrapposizioni di `coverage_validate`, la copia di lavoro, i blocchi e
  l'uscita trattenuta del `buffer` (un solo conto per passo, righe a
  blocchi fissi di 64). Quando quelle non entrano nel margine il kernel si
  ferma con un `ResourceLimit` che lo nomina, invece di allocarle. Il
  controllo è deterministico.

  **Non è un tetto garantito.** Restano fuori: il transitorio dentro una
  chiamata di `geo` o di `i_overlay` (incroci, grafi, il risultato di un
  overlay fino al primo controllo); le strutture di `rstar` (gli R-tree,
  proporzionali agli ingressi); la crescita dei builder Arrow delle colonne
  d'uscita; alcuni vettori ausiliari (riferimenti alle righe, gruppi per
  riga, indici) e il transitorio delle codifiche WKB e della validazione
  OGC; l'overhead dell'allocatore. Il controllo dopo il passo con i byte
  esatti resta. *Rientro*: un limite di memoria del processo imposto dal
  sistema operativo (job object su Windows, cgroup su Linux), previsto con
  l'infrastruttura; una grandezza a secco per queste espansioni nel
  modello.
- **Transitorio oltre la previsione non rilevato.**
  *Regola*: prima del passo si controlla la previsione del modello, dopo il
  passo i byte vivi esatti delle tabelle residenti con l'uscita; la memoria
  che il kernel alloca e libera durante il passo non si misura.
  *Ambito*: ogni passo di `PipelineValidata::run` (il controllo dopo il
  passo in `esecuzione.rs` conta solo i buffer Arrow ancora vivi con
  l'uscita); in particolare il transitorio dentro `geo` e `i_overlay` dei
  sette profili geo esclusi dal modello, che il margine dei kernel non vede
  (limite «Modelli di costo geo»: i risultati trattenuti invece si
  contano), e le espansioni che dipendono dai dati senza preflight di
  memoria nel kernel (`join` molti a molti, `explode`, `unnest`, le geo
  fuori da quei profili).
  *Hazard*: un transitorio già liberato alla fine del passo può aver
  superato il budget senza alcun errore: nessun `ResourceLimit`, e nessun
  esaurimento di memoria se la macchina ne ha. Il budget dichiarato è stato
  superato in silenzio; il resoconto riporta solo i byte vivi dopo il
  passo. Se la macchina non ne ha, il processo si ferma per esaurimento di
  memoria invece che con un errore del runner.
  *Rientro*: un limite di memoria del processo imposto dal sistema
  operativo (job object, cgroup), previsto con l'infrastruttura: un tetto
  vero, che la guardia dei kernel geo (limite «Modelli di costo geo») non
  è.
- **Geo senza diagnostica per riga.**
  *Regola*: un passo geo rende il primo errore in ordine di riga, senza
  report `plenora-row-diagnostics-v1`; gli indici che alcuni messaggi dei
  kernel riportano sono righe dell'ingresso del passo, non della sorgente.
  *Ambito*: ogni `geo.*` tranne `from_wkt`.
  *Hazard*: un indice di riga dopo un passo che cambia righe o ordine
  (`table.filter`, `table.sort`) non punta alla riga del file d'origine.
  *Rientro*: la raccolta completa per riga del passo geo di `190c493`
  (`collect_cell_failures`), con la base degli indici del runner.
- **Diagnostica senza esempi dopo un passo che cambia le righe.**
  *Regola*: gli esempi della diagnostica per riga hanno l'indice della
  sorgente (`source_row_zero_based`) solo quando nessun passo a monte
  cambia numero o ordine delle righe; altrimenti il payload ha conteggi,
  cause e totale senza esempi, completezza `partial` e limite di
  conoscenza `read.row_attribution_unavailable`, e il testo nomina passo e
  ingresso ([«Diagnostica per riga»](#diagnostica-per-riga)). Garanzia
  indebolita: quante righe e perché, non quali.
  *Ambito*: ogni passo il cui primo ingresso discende da un'operazione con
  `source_row_provenance` `Unavailable` (catalogo).
  *Hazard*: nessun indice sbagliato (il contratto vieta la base
  dell'ingresso del passo che il runner pubblicava fino a `da5f779`), ma la
  riga rifiutata si trova solo eseguendo a parte il piano spezzato, e dopo
  un'aggregazione, un join o un'esplosione una sola riga d'origine può non
  esistere. `run` fallisce senza output parziali.
  *Rientro*: una mappa di righe verso la sorgente per i passi che
  selezionano o permutano senza duplicare (`filter`, `sort`, `limit`,
  `sample`, `distinct`), composta passo per passo e contata nei byte vivi,
  che riporterebbe gli esempi con l'indice della sorgente; i passi che
  duplicano o creano righe (`join`, `explode`, `aggregate`) restano senza
  esempi, perché il contratto v1 vuole un indice della sorgente unico per
  esempio.
- **Scadenza e annullamento solo fra i passi.**
  *Regola*: `run_interrompibile` controlla scadenza e annullamento prima
  di ogni passo e prima di consegnare gli output
  ([«Scadenza e annullamento»](#scadenza-e-annullamento)), mai dentro un
  kernel.
  *Ambito*: `PipelineValidata::run_interrompibile`; anche i controlli dopo
  il passo (budget, contratto) e la validazione degli input girano fino in
  fondo.
  *Hazard*: un passo lungo (un join enorme, un overlay) finisce anche
  oltre la scadenza o dopo l'annullamento, e l'errore arriva al controllo
  successivo: il ritardo è al più la durata del passo in corso. Il
  risultato non è mai sbagliato: un'esecuzione interrotta non rende
  output, una finita prima della scadenza li rende tutti.
  *Rientro*: controlli cooperativi dentro i kernel lunghi (per blocco di
  righe o di coppie), con lo stesso `Interruzione` passato ai kernel.
- **Run-end e union rifiutati al confine.**
  *Regola*: uno schema con una colonna `RunEndEncoded` o `Union`, a
  qualunque profondità (valori di una dictionary, figli di liste, struct e
  mappe), si rifiuta con `Unsupported` prima di ogni passo
  (`plenora_core::contract::arrow_schema::verifica_tipi_supportati`, in
  `contract_from_arrow_schema`), alla lettura Arrow IPC prima di
  decodificare i blocchi (Parquet non li produce) e in scrittura prima di
  creare il file. Nessun kernel li vede.
  *Ambito*: input del runner, `plenora-io`.
  *Hazard*: in Arrow 60.0.0 `concat` di run-end trabocca sulle fini
  `Int16` (la somma delle lunghezze in `concat_run_arrays` non è
  controllata: panico con `overflow-checks`) e `logical_nulls` sbaglia
  sulle union dense a un campo con id diverso da 0 (raccoglie i null con
  l'id fisso 0): una cella nulla diventerebbe un valore senza errore.
  `take` su run-end, che in 59.2.0 ignorava gli indici nulli, in 60.0.0 li
  tratta. Tutti e tre verificati sul sorgente e con una sonda fuori dal
  workspace, non con l'oracolo del rientro. Chi chiama i kernel
  direttamente, fuori dal runner, non ha il controllo.
  *Rientro*: quando Arrow corregge `take`/`concat` sulle run-end e
  `logical_nulls` delle union a un campo, verificato con un oracolo contro
  `logical_nulls` e contro la stessa tabella senza codifica.

- **Errori che dipendono dai valori delle celle, in esecuzione.**
  *Regola*: la validazione rifiuta ciò che config, schema e limiti rendono
  prevedibile; ciò che dipende dal valore di una cella fallisce, con un
  errore esplicito, quando il kernel la legge.
  *Ambito*: testo non numerico in una colonna Utf8 letta come numero
  (confronti ordinati, aggregazioni, statistiche, `assert_range`); valori
  che non si convertono nel tipo chiesto (`type_cast`, parse delle date);
  un `amount` di `date_add` che alcune date sopportano e quelle dei dati no;
  in `expression`, regex e indici di `substring` calcolati dalle colonne,
  divisori non letterali nulli con `on_division_by_zero=error`; testi
  prodotti oltre `max_string_bytes` e pattern calcolati oltre
  `max_regex_bytes`; le asserzioni violate dai dati.
  *Hazard*: i passi a monte hanno già girato quando l'errore arriva.
  *Rientro*: nessuno previsto, è la natura del dato. L'oracolo
  `crates/plenora-pipeline/tests/oracolo_config.rs` esegue ogni config che
  l'analisi accetta (varianti di ogni operazione del catalogo) e ammette in
  esecuzione solo queste classi, elencate con il motivo; per le config che
  l'analisi rifiuta con una regola «il kernel fallirebbe», chiama il kernel
  direttamente e verifica che fallisca davvero (nessun rifiuto falso).
  Oltre i dati delle fixture non prova: il confine di `verifica_amount` sul
  secondo intercalare ha un test a parte.
- **Parametri ignorati: censimento dei campi di primo livello.**
  *Regola*: nessun parametro scritto si ignora; si rifiuta in analisi e nel
  kernel.
  *Ambito*: `censimento_parametri.rs` legge da serde i campi di ogni config
  tabellare e fallisce per un campo senza voce; `parametri_senza_effetto.rs`
  prova ogni regola in validazione, nel kernel e sulla config gemella;
  l'oracolo `oracolo_config.rs` prova le regole contro i kernel sulle
  varianti delle fixture. I campi delle strutture annidate (aggregazioni,
  mascherature, regole, condizioni, colonne di `align_schema`, nodi di
  `expression`) non si enumerano da soli: li copre la voce del campo che li
  contiene.
  *Hazard*: restano accettati, e dichiarati, i parametri che hanno effetto
  ma non cambiano il risultato su certi dati (`distinct` con `min`/`max`);
  quelli senza effetto solo su certi schemi d'ingresso (elencati in
  «Validazione»: la regola è rifiutare ciò che la config da sola rende
  senza effetto); e questi senza effetto in casi limite: `fill_na` con `method=value` e senza
  `value` (riempie con null, non cambia niente); `unit` di `date_add` con
  `amount` 0 (riformatta soltanto); le politiche sui null
  (`allow_null`, `nulls_equal`, `null_policy`) su colonne che lo schema
  dichiara non nullable, perché la nullabilità dichiarata è spesso
  prudente e una dictionary non nullable può contenere null logici;
  `max_splits` di `split_column` che lascia sempre null le ultime colonne
  (ha effetto sull'ultima parte); `var_name` e `value_name` di `melt` che
  collidono con una colonna, rinominati con un suffisso come dichiara la
  scheda.
  *Rientro*: un parametro nuovo entra con la sua voce nel censimento, la sua
  regola in una `verifica_*` condivisa e un caso in
  `parametri_senza_effetto.rs`; le politiche sui null su colonne non
  nullable si rifiuteranno quando la nullabilità dei contratti sarà esatta.
- **Limiti dei testi: regole di config nell'analisi, testi prodotti nei
  kernel.**
  *Regola*: un testo o un pattern della config oltre `max_string_bytes` o
  `max_regex_bytes` si rifiuta in analisi; un testo prodotto dai dati oltre
  `max_string_bytes` si rifiuta nel kernel ([«Validazione»](#validazione)).
  *Ambito*: i kernel chiamati direttamente, fuori dal runner; le funzioni
  `expression`, `formula`, `aggregate`, `replace` e `mask_data`, che usano
  `Limits::default()` (le varianti `_con_effetti` e `_con_limiti` ricevono
  i limiti del chiamante, e il runner usa quelle); `geo.to_wkt`, che scrive
  WKT senza confrontarlo con `max_string_bytes`.
  *Hazard*: chi chiama un kernel senza l'analisi può passare testi e
  pattern di config oltre i limiti, e con le funzioni senza limiti ottiene
  i limiti di default, non i suoi; il WKT di una geometria grande supera
  `max_string_bytes` senza errore.
  *Rientro*: i limiti come parametro di ogni kernel tabellare che produce
  testo (oggi cambierebbe la firma di decine di chiamate); il controllo di
  `max_string_bytes` in `geo.to_wkt` con le geo.
- **Chiave HMAC controllata in validazione, dal runner**: è ambiente, non
  config, quindi non sta nell'analisi dei kernel; la legge la stessa
  funzione del kernel. La variabile d'ambiente può cambiare fra `validate`
  e `run`, e in quel caso l'errore arriva al passo.
- **Nome del passo negli errori**: aggiunto al messaggio conservando la
  categoria; gli errori con diagnostica per riga o già strutturati restano
  quelli del kernel, salvo la diagnostica sulla base dell'ingresso del
  passo, che nomina passo e ingresso.
