# Checkpoint per la seconda review — 2026-08-16

Stato del worktree al momento della consegna: **83 percorsi** modificati o
nuovi (82 piu' questo documento), nessun commit. Cinque giri di review statica chiusi piu' due blocchi di
usabilita' (CLI e verifica §1.4). Suite: **1316 test passati, 0 falliti, 1
ignorato**.

Questo documento non sostituisce la seconda lettura: e' la mappa per farla.
Dove ho riletto il mio stesso lavoro, l'ho scritto; una rilettura dell'autore
non e' una revisione indipendente.

---

## 1. Inventario dei percorsi

`+6368 / -1386` righe sui file tracciati, piu' 12 file nuovi.

### `plenora-core` — 6 percorsi

| percorso | natura |
|---|---|
| `src/catalog.rs` | M — `ExpansionConstraint::exceeded` (decisione esatta del vincolo binario), `JoinExpansion` declassata a metrica osservabile |
| `src/contract.rs` | M — validazione del contratto: unicita' dei nomi, geometria `Binary`, chiavi canoniche presenti sempre validate, costanti delle chiavi, `FieldAllocator` fallibile |
| `src/diagnostics.rs` | M — `merge_into`, `into_partial`, reticolo di completezza, `RowDiagnosticsMergeError` |
| `src/limits.rs` | M — `Limits::validate`, `expansion_exceeded` (esatto, base `u64`) e la variante interna a 128 bit |
| `src/lib.rs` | M — export del modulo `json` |
| `src/json.rs` | **nuovo** — `ensure_no_duplicate_keys` |

### `plenora-engine` — 21 percorsi

| percorso | natura |
|---|---|
| `src/ipc_boundary.rs` + `src/ipc_boundary/tests.rs` | **nuovi** — confine unico di lettura Arrow IPC (barriera anti-panico, framing pre-validato, tetti derivati dal piano) |
| `src/parallelism.rs` | **nuovo** — applicazione di `max_parallelism` al pool del processo |
| `src/geo_transport/ipc.rs` | M — pre-validazione del framing, footer con lunghezze esatte, EOS stretto, budget dei nodi di schema, tetto sui record batch anche nel file format |
| `src/geo_transport/error.rs` | M — sette varianti nuove di errore del confine |
| `src/geo_transport/transport.rs`, `unary.rs` | M — allineamento al confine, artefatto fuzz troncato all'EOS |
| `src/executor.rs` + `src/executor/tests.rs` | M — espansione esatta, contatori saturanti dichiarati, `Inputs` strict/atomico, `Input::Batches` a variante struct |
| `src/planner.rs` + `tests.rs` | M — fingerprint del contratto pubblico, `check_declared_input_contracts` |
| `src/plan.rs` + `tests.rs` | M — `canonical_number` (interi oltre 2^53, `-0.0`), chiavi duplicate rifiutate |
| `src/prepare.rs` + `tests.rs` | M — limiti effettivi rivalidati |
| `src/governor.rs` | M — accounting atomico, `MemoryMetrics` non esaustiva |
| `src/table_engine/executor.rs` | M — accumulo spill che dichiara la saturazione |
| `src/temp_store.rs`, `src/lib.rs` | M — allineamenti |
| `tests/review_regressions.rs`, `tests/catalog_coverage.rs` | **nuovi** — 17 + 3 test |
| `tests/geo_binary_oracle.rs`, `tests/geo_fusion_oracle.rs` | M — `allow(deprecated)` mirati |
| `examples/bench_geo_binary.rs`, `bench_geo_fusion.rs` | M — idem |

### Kernel — 30 percorsi

`plenora-kernels-table` (27): `exact_compare.rs` e `hashing.rs` **nuovi**;
`lib.rs` (confronti esatti, `NumericBound` con `U64`/`Decimal`, normalizzazione
dei decimali), `joins.rs` (conteggio a due fasi su entrambi i percorsi),
`aggregation/*` (comparatori nativi, prevalidazione dell'ordinabilita'),
`spill.rs` (`SpillMetrics::accumulate` + `saturated`), `formula.rs` (stack
residuo), `analyze/*`, `governance.rs`, `quality.rs`, `reshape.rs`,
`setops.rs`, `dates.rs`, `cleansing.rs`, `filtering.rs`, `fuzzy.rs`,
`expressions/*`, `analysis.rs`; `tests/review_regressions.rs` **nuovo** (11
test).

`plenora-kernels-geo` (3): `arrow_adapter.rs`, `analyze/producers.rs`,
`analyze/quality.rs` — allineamento alle costanti canoniche e ai confronti
esatti.

### CLI — 10 percorsi

| percorso | natura |
|---|---|
| `src/main.rs` | M — `describe`, `--input NOME=PERCORSO`, envelope su stdout, exit code per categoria, `--format` globale, `--version --json`, superficie dei flag dichiarata |
| `tests/matrice_cli.rs` | **nuovo** — matrice §1.4 (11 test) |
| `tests/examples_e2e.rs` | **nuovo** — esempio E1 end-to-end (2 test) |
| `tests/nogeo_cli.rs` | M — 5 test nuovi sulle convenzioni, canale invertito |
| `tests/dag_v4_cli.rs` | M — 2 test nuovi sul binding degli input, canale invertito |
| `tests/geo_cli_failclosed.rs`, `roundtrip_smoke.rs`, `ipc_canonical_determinism.rs`, `xyzm_roundtrip_cli.rs` | M — canale invertito |

### Documentazione — 10 percorsi

| percorso | natura |
|---|---|
| `docs/adr/ADR-0003` | M — emendamento: canale dell'envelope, exit code come divergenza |
| `docs/adr/ADR-0004` | M — tre emendamenti: `plan_hash`, binding degli input, profilo stretto |
| `docs/adr/ADR-0006` | M — validazione unica dei limiti, `max_parallelism` |
| `docs/adr/ADR-0009` | M — «illeggibile» non e' «assente», chiave presente sempre validata |
| `docs/adr/ADR-0010` | M — conversioni esatte, decimali esatti |
| `docs/deroghe.md` | M — DER-005 riscritta, DER-006/007/008/009 nuove |
| `docs/api-breaking-2026-08-16.md` | **nuovo** — superficie rotta |
| `docs/piano-usabilita.md` | **nuovo** — piano + matrice di allineamento con `plenora-database` |
| `docs/checkpoint-review-2026-08-16.md` | **nuovo** — questo documento |
| `README.md` | M — riscritto attorno a E1, convenzioni di output |

### Esempi — 1 percorso (4 file)

`examples/e1-filtro-ordinamento/` — piano, dati, output atteso, README.

### Test nuovi: 44 in totale

17 (engine regressions) + 11 (table regressions) + 3 (catalog coverage) + 11
(matrice CLI) + 2 (esempi E2E) + 7 (CLI convenzioni e binding) — meno le
sovrapposizioni gia' contate — piu' 11 nel confine IPC e 6 unitari sparsi
(`catalog`, `limits`, `spill`, `formula`, `parallelism`, `executor`).

---

## 2. Cambiamenti osservabili e API rotte

L'inventario completo e' in `docs/api-breaking-2026-08-16.md`. Sintesi per la
review, con il criterio: **osservabile** = un chiamante o uno script se ne
accorge senza leggere il codice.

### API Rust rotte

| elemento | rottura |
|---|---|
| `ExecutionMetrics`, `NodeMetrics`, `SegmentMetrics`, `MemoryMetrics`, `SpillMetrics` | `#[non_exhaustive]` + campi nuovi (`counters_saturated`, `saturated`) |
| `Input::Batches` | da variante tupla a variante struct |
| `Inputs::add` / `with` | **deprecate** (funzionano, con warning) |
| `NumericBound` | varianti nuove `U64`, `Decimal`; forma normalizzata dei decimali |
| `FieldAllocator::{alloc,intern,derive}` | restituiscono `Result` |
| `DataContract::validate`, `Limits::validate` | rifiutano input che prima passavano |
| `plenora_core::limits::expansion_exceeded` | firma `u64` (la variante a 128 bit e' interna) |

### Comportamento CLI

| cambiamento | prima | ora |
|---|---|---|
| Canale dell'envelope | stderr | **stdout**, stderr vuoto |
| Exit code | sempre `2` (130 al cancel) | 2/3/4/5/6/70, 130 al cancel |
| `--inputs` con piano v4 multi-input | accoppiava per posizione | **rifiutato**, con rimedio nominale |
| Flag sconosciuto | ignorato, exit 0 | rifiutato |
| Flag a valore singolo ripetuto | vinceva il primo | rifiutato |
| Comando inesistente | help su stderr + envelope su stdout | solo envelope |
| `--version` | testo | JSON |
| `--format` | non esisteva | globale `json\|markdown` |
| `describe` | non esisteva | nuovo (alias `inspect-dataset`) |
| Comandi legacy | non marcati | deprecati nell'help |

### Semantica dei dati

- confronti numerici esatti dove decidono un limite o un ordine (nessun
  passaggio da `f64` sui conteggi);
- `plan_hash` distingue interi oltre 2^53, `-0.0`, e rifiuta chiavi JSON
  duplicate;
- IPC: compressione rifiutata, EOS stretto, tetti sui messaggi e sui record
  batch, footer con lunghezze esatte.

---

## 3. Mappa finding → fix → regressione → ADR/deroga

Ogni riga e' verificabile: il test nominato fallisce sul codice precedente al
fix.

### Identita' del piano e del contratto

| difetto | fix | regressione | ADR/DER |
|---|---|---|---|
| `plan_hash` collassava gli interi oltre 2^53 | `plan.rs::canonical_number` | `gli_interi_oltre_2_53_non_collassano_nel_piano_canonico` | ADR-0004 §em. 1 |
| `-0.0` canonicalizzato come `0` | idem | `lo_zero_negativo_resta_distinto_dallo_zero_nel_piano_canonico` | ADR-0004 §em. 3 |
| Chiavi JSON duplicate risolte «vince l'ultima» | `core::json::ensure_no_duplicate_keys` | `le_chiavi_duplicate_nel_piano_sono_rifiutate`, `il_json_malformato_resta_un_errore_di_mappatura` | ADR-0004 §em. 1 |
| Proprieta' tipizzate fuori dal fingerprint | `planner::contract_canonical` | test di planner | ADR-0004 §em. 1–2 |
| Contratto: geometria non `Binary`, nomi ripetuti | `contract::validate` | `una_geometria_non_binary_e_rifiutata_dal_contratto`, `i_nomi_di_campo_ripetuti_sono_rifiutati` | ADR-0009 |
| Chiave canonica presente ma malformata trattata come assente | `contract::validate_declared_types` | `le_chiavi_canoniche_malformate_sono_rifiutate_anche_senza_lato_tipizzato` | ADR-0009 §em. 2 |
| Encoding/dimensioni incoerenti coi metadati | idem | `il_contratto_rifiuta_un_encoding_incoerente_coi_metadati`, `..._dimensioni_...` | ADR-0009 |
| `FieldAllocator` ripeteva l'ultimo id a fondo scala | `contract::FieldAllocator` | `l_allocatore_dei_field_id_fallisce_invece_di_ripetere_l_ultimo` | ADR-0004 |
| Binding del contratto opzionale nell'API | `Inputs::strict` + deprecazione | `il_profilo_stretto_rifiuta_un_input_senza_contratto` | ADR-0004 §em. 4 |
| `Inputs::add*` mutava lo stato anche fallendo | `Inputs` via `entry`/`contains_key` | `un_inserimento_duplicato_lascia_inputs_invariato` | — |

### Confine Arrow IPC

| difetto | fix | regressione | ADR/DER |
|---|---|---|---|
| `arrow-ipc` va in panico su schema ostile | `ipc_boundary` + `catch_unwind` | `ipc_decode_converte_il_panico_di_arrow_in_errore` | — |
| Ingressi file/stream/CLI scavalcavano la barriera | `ipc_boundary` come lettore unico | `i_file_prodotti_da_arrow_passano_il_confine_in_entrambi_i_formati` | — |
| Allocazione ostile dai metadati | pre-validazione + tetti | `il_confine_rifiuta_i_metadati_oltre_il_tetto_prima_di_allocare`, `..._i_tetti_su_body_e_numero_di_messaggi` | — |
| Footer che punta fuori regione / blocchi sovrapposti | `validate_footer_blocks` | `..._un_footer_che_punta_fuori_dalla_regione_dati`, `..._con_blocchi_sovrapposti` | — |
| Lunghezze del blocco solo «contenute», non esatte | uguaglianza esatta | `il_confine_esige_che_i_blocchi_dichiarino_le_lunghezze_esatte` | — |
| Byte dopo l'EOS non guardati | EOS stretto | `il_confine_rifiuta_i_byte_dopo_la_fine_dello_stream` | — |
| `max_record_batches` non applicato nel file format | conteggio per tipo di header | `il_confine_applica_il_tetto_sui_record_batch_anche_al_file_format` | — |
| Schema con sottoalberi condivisi (esplosione) | `SchemaBudget` | test di `geo_transport::ipc` | — |
| IPC compresso non limitabile | rifiutato | test di `geo_transport::ipc` | **DER-007** |
| TOCTOU fra validazione e lettura | handle unico + dichiarazione | — (condizione dichiarata) | **DER-008** |

### Numerica esatta

| difetto | fix | regressione | ADR/DER |
|---|---|---|---|
| `to_f64()` usato come test di rappresentabilita' | `exact_f64_from_*` | `le_conversioni_esatte_rifiutano_cio_che_to_f64_arrotondava` | ADR-0010 §em. 1 |
| Sort collassava interi oltre 2^53 | comparatori nativi | `il_sort_su_interi_oltre_2_53_non_li_collassa` | ADR-0010 |
| Decimal confrontati come stringhe / via `f64` | `exact_compare` (U256) | `i_decimal_si_ordinano_per_valore_non_per_stringa` | ADR-0010 §em. 2 |
| Timestamp ordinati per ora locale | comparatori nativi | `i_timestamp_si_ordinano_per_istante_non_per_ora_locale` | ADR-0010 |
| Binario non UTF-8 ordinato come testo | idem | `il_binario_si_ordina_per_byte_anche_se_non_e_utf8` | ADR-0010 |
| Colonne non ordinabili scoperte a meta' sort | `validate_sortable` | `il_sort_rifiuta_le_colonne_non_ordinabili_prima_di_confrontare` | ADR-0001 |
| Zeri non significativi contati come cifre | `NumericBound::parse_decimal` | `gli_zeri_non_significativi_non_fanno_ricadere_un_decimale_su_f64` | ADR-0010 |
| Arrotondamento implicito nelle op che producono `Float64` | dichiarato | oracoli di `expressions` | **DER-005** |

### Limiti, risorse, metriche

| difetto | fix | regressione | ADR/DER |
|---|---|---|---|
| `max_rows` applicato al chunk, non al totale | conteggio a due fasi | `il_join_applica_max_rows_al_totale_non_al_singolo_chunk` | ADR-0006 |
| Join right/outer non contava le righe destre | percorso generico a due fasi | `il_join_right_conta_anche_le_righe_destre_non_abbinate`, `..._outer_produce_lo_stesso_output_...` | ADR-0006 |
| Espansione binaria decisa in `f64` | `ExpansionConstraint::exceeded` | `la_decisione_binaria_e_esatta_anche_dove_le_metriche_arrotondano` | ADR-0006 |
| Predicato pubblico non totale sul dominio | firma `u64` + soglia nulla | `il_predicato_e_totale_su_tutto_il_dominio_dei_conteggi` | ADR-0006 |
| Limiti nulli o `NaN` accettati | `Limits::validate` | `i_limiti_nulli_e_il_fattore_non_finito_sono_rifiutati`, `spill_partitions_*` | ADR-0006 §em. |
| Governor avvolgeva sui byte estremi | accounting atomico | `il_governor_non_avvolge_ne_va_in_panico_sui_byte_estremi` | ADR-0002 |
| Contatori di metrica non controllati | `accumulate` + `counters_saturated` | `l_accumulo_delle_metriche_di_spill_dichiara_la_saturazione` | ADR-0002 |
| `max_parallelism` non vincolava nulla | `parallelism::configure` | `zero_esige_che_il_pool_del_processo_sia_quello_di_default` | **DER-006** |
| Hasher non keyed | dichiarato | — (rischio dichiarato) | **DER-009** |

### Diagnostica e catalogo

| difetto | fix | regressione | ADR/DER |
|---|---|---|---|
| Fusione dei report migliorava la conoscenza | reticolo di completezza | `la_fusione_dei_report_non_migliora_la_conoscenza` | ADR-0003 |
| `into_partial` cancellava i limiti noti | unione | `il_declassamento_a_parziale_non_cancella_i_limiti_gia_noti` | ADR-0003 |
| Stack della formula con operandi residui | controllo esplicito | `lo_stack_con_operandi_residui_e_un_errore_non_un_risultato` | ADR-0001 |
| Operazioni senza analyzer di contratto | gate strutturale | `ogni_operazione_del_catalogo_ha_un_analyzer_di_contratto` (+ meta-test) | ADR-0005 |

### CLI

| difetto | fix | regressione | ADR/DER |
|---|---|---|---|
| Binding posizionale degli input | `--input NOME=PERCORSO`, posizionale rifiutato oltre un input | `due_input_invertiti_non_raggiungono_mai_l_esecuzione` | — |
| Nessun modo di ispezionare un input | `describe` | `e1_filtro_e_ordinamento_riproduce_l_output_atteso` | — |
| Envelope su un canale diverso dal gemello | stdout | `l_envelope_vive_su_stdout_e_stderr_resta_vuoto` | ADR-0003 §em. |
| Un solo exit code per ogni errore | mappatura per categoria | `gli_exit_code_seguono_la_categoria_dell_envelope` | ADR-0003 §em. |
| Flag sconosciuti ignorati | superficie dichiarata | `nessun_comando_ignora_un_flag_che_non_conosce` | — |
| Flag a valore singolo ripetuti | idem | `un_flag_a_valore_singolo_non_si_ripete` | — |
| Help su stderr insieme all'envelope | rimosso | `un_comando_inesistente_emette_solo_l_envelope` | ADR-0003 §em. |

---

## 4. Punti che richiedono una seconda lettura indipendente

In ordine di rischio. Per ciascuno: perche' e' delicato, e cosa guardare.

### 4.1 `exact_compare` — aritmetica a 256 bit scritta a mano

`crates/plenora-kernels-table/src/exact_compare.rs`. Prodotto schoolbook su
limbi, `bit_len`, `checked_shl`, decomposizione IEEE-754. **Nessuna libreria
esterna**: se c'e' un errore di riporto o di spostamento, i test che ho scritto
io potrebbero condividerne l'assunzione. Da rileggere contro un oracolo
indipendente (Python `decimal`/`fractions` su una campagna casuale).

### 4.2 Pre-validazione del framing IPC

`crates/plenora-engine/src/geo_transport/ipc.rs`. E' un parser di formato
binario ostile scritto a mano: offset, allineamenti, vettori flatbuffer,
uguaglianze esatte fra lunghezze dichiarate e reali. Il rischio non e' il caso
che ho pensato, ma quello che non ho pensato. Da rileggere con la specifica
Arrow IPC accanto, e con il fuzz target `arrow_ipc_decode`.

### 4.3 Espansione e limiti in aritmetica esatta

`core::limits::expansion_exceeded` + `catalog::ExpansionConstraint::exceeded`.
Scrivendo il test sul dominio ho trovato un mio difetto (soglia nulla con
esponente grande): il che dice che quella funzione **merita un'altra
passata**, non che ora sia a posto. Da verificare in particolare i rami di
shift oltre 128 e il fail-closed sull'overflow del prodotto.

### 4.4 Contatori saturanti e `counters_saturated`

`executor.rs`. La saturazione e' dichiarata, ma il flag e' uno solo per
esecuzione: se satura un contatore per-nodo, il consumatore sa che «qualcosa»
e' saturo, non cosa. Da valutare se e' abbastanza per l'osservabilita' che
serve.

### 4.5 Inversione del canale dell'envelope

Rovescia una decisione presa deliberatamente il 2026-08-03. La motivazione e'
in ADR-0003 §em.; e' una decisione di prodotto, non tecnica, e va confermata
da chi possiede il contratto pubblico.

### 4.6 Superficie dei flag della CLI

`superficie()` in `main.rs` e' una **lista scritta a mano** confrontata con
l'help da un test. Se un flag esiste nel dispatch ma non nella lista, il test
non lo vede: la lista e' la fonte, e nessuno verifica che sia completa rispetto
al codice che legge gli argomenti. E' il punto piu' debole della verifica
§1.4.

### 4.7 Deroghe nuove

DER-006 (parallelismo di processo), DER-007 (IPC compresso), DER-008 (TOCTOU),
DER-009 (hasher non keyed). Sono rischi **accettati**: la decisione di
accettarli non e' mia.

### 4.8 Perimetro non coperto da questo lavoro

- i quattro comandi CLI legacy sono deprecati ma **non** ri-verificati nel
  merito;
- `crs_resolution` resta non confrontabile fra contratto e metadati (deciso e
  motivato, ma e' una scelta che vale la pena rileggere);
- nessuna modifica ai kernel geometrici oltre l'allineamento alle costanti.

---

## 5. Gate eseguiti

Tutti in container, toolchain pinnata 1.92.0 (la macchina non ha cargo).

| gate | esito | note |
|---|---|---|
| `cargo fmt --all --check` | **OK** | |
| `cargo clippy --workspace --all-targets` (default) | **OK** | pedantic + nursery deny |
| Gate anti-panico R6 (default) | **OK** | |
| `cargo test --workspace --no-fail-fast` | **OK** | 1316 passati, 0 falliti, 1 ignorato |
| Clippy `--features full-backends` (`--all-targets`) | **OK** | GEOS statico + PROJ bundled compilati da sorgente |
| **Gate R6 `--features full-backends`** | **OK** | perimetro feature-gated `geos`/`proj` |
| Test `--features full-backends` | **OK** | 969 passati, 1 ignorato — **un difetto trovato**, vedi sotto |
| Coverage con soglie CI | **OK** | lines 92.04% / functions 87.50% / regions 92.94% (soglie 90/85/89) |
| Clippy per target Windows | **OK (parziale)** | `x86_64-pc-windows-gnu`, non MSVC: vedi limite dichiarato |
| Fuzz smoke (ridotto) | **OK** | 4 target, 45 s ciascuno, nessun crash: vedi limite dichiarato |

### Difetto trovato dal gate full-backends

`dag_v4_geo_pregate_wkb_rejection_carries_authoritative_step_context`
(`dag_v4_cli.rs`) e tre asserzioni in `roundtrip_smoke.rs` erano rimaste sulle
convenzioni PRECEDENTI (`stdout` vuoto, exit `2`). Sono test **compilati solo
con `proj-backend`**: la migrazione del canale li aveva mancati perche' nel
perimetro di default non esistono. E' esattamente la classe di difetti per cui
il job `backends` esiste — lo dice il suo stesso commento in `ci.yml`, a
proposito dei lint su `pair.rs` emersi mesi dopo.

Corretti: exit code dalla categoria (`data_mapping` → 3), `stderr` vuoto,
envelope su `stdout`. **Difetto del worktree corrente**, non ambientale.

### Coverage: numeri e lettura

Misura sul perimetro di default (i backend nativi non sono strumentati, come in
CI). Soglie superate su tutte e tre le metriche.

Il totale (92.04% lines) e' **sotto la misura documentata in `ci.yml` del
2026-08-06** (93.44%), sopra la soglia ma in calo di ~1,4 punti. I file nuovi
con la copertura piu' bassa:

| file | lines | motivo |
|---|---|---|
| `plenora-engine/src/parallelism.rs` | 62.20% | i rami d'errore dipendono dallo stato del pool Rayon del processo: non sono esercitabili in-process senza rompere gli altri test |
| `plenora-engine/src/ipc_boundary.rs` | 78.67% | rami di panico di arrow e formati che i writer non producono |
| `plenora-engine/src/geo_transport/ipc.rs` | 80.03% | percorsi di framing malformato non ancora tutti coperti |
| `plenora-kernels-table/src/exact_compare.rs` | 94.12% | |
| `plenora-kernels-table/src/hashing.rs` | 96.30% | |

**Non ho misurato la baseline su HEAD**: l'attribuzione precisa del calo
richiede una misura sulla revisione senza queste modifiche, che non ho
eseguito. Dire «il calo e' tutto del codice nuovo» sarebbe una supposizione.

### Limiti dichiarati dei due gate parziali

**Clippy Windows.** Il target di CI e' `x86_64-pc-windows-msvc` e richiede una
macchina Windows con MSVC: da un container Linux non e' producibile. Ho
eseguito `x86_64-pc-windows-gnu`, che compila gli stessi `cfg(windows)` con un
altro toolchain: `--lib --bins` e `--all-targets` passano entrambi puliti. E'
una verifica **dei rami condizionali**, non un sostituto del job MSVC.

**Fuzz smoke.** I prerequisiti di `scripts/fuzz-smoke.sh` non esistono su
questa macchina: l'immagine `plenora-rust:nightly-fuzz` non c'e' e il percorso
`C:/tmp/plenora-geo-tools-arrow/.fuzz-cargo/bin` (montato come `/fuzzbin`) non
esiste. **Condizione ambientale, non un difetto del worktree.** Ho ricostruito
l'ambiente equivalente — nightly + `cargo-fuzz` 0.13.2, come fa la CI — ed
eseguito uno smoke **ridotto**: quattro target invece di sedici, 45 secondi
invece di 90. La selezione segue le modifiche di questo worktree
(`arrow_ipc_decode`, `plan_v4_parse`, `executor_dag`, `analyze_table`);
`arrow_transform` resta escluso come in CI, per la ragione documentata nel
workflow (l'hook di panico di `libfuzzer-sys` chiama `abort()` prima
dell'unwinding e renderebbe il target rosso a barriera funzionante). Uno smoke
ridotto non e' la campagna notturna.

Esito: **nessun crash e nessun artefatto** su tutti e quattro i target.

| target | esecuzioni in 45 s |
|---|---|
| `arrow_ipc_decode` | 1 115 434 |
| `plan_v4_parse` | 246 252 |
| `executor_dag` | 7 013 |
| `analyze_table` | 67 637 |

`fuzz/artifacts/` e' rimasto vuoto; corpus e artefatti sono comunque
gitignorati e non entrano nel worktree.
