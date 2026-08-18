# ADR 10 — Forma numerica del contratto: niente FMA

- **Stato**: accettato e attuato (allineamento degli 8 siti preesistenti)
- **Decisioni collegate**: ADR 1 (determinismo bit-esatto)
- **Riferimenti**: `Prestazioni.md` §6; regola 5 di AGENTS.md

## Contesto

Il determinismo di ADR-0001 — stesso input → stesso output, sempre, su
ogni piattaforma supportata — richiede che ogni operazione in virgola
mobile sia esattamente definita. Le operazioni IEEE 754 di base
(moltiplicazione, addizione) sono specificate in modo esatto ovunque. La
forma **fusa** `a.mul_add(b, c)` (FMA, un solo arrotondamento) dà un
risultato diverso dalla forma non fusa `a * b + c` (due arrotondamenti) e
dipende dalla qualità dell'implementazione fused della std sulla
piattaforma: le due forme non sono intercambiabili senza cambiare output
(fino a 1 ULP per operazione).

Il codebase era incoerente: la quasi totalità dei siti numerici usa la
forma non fusa (con `#[allow(clippy::suboptimal_flops)]` motivato), ma 8
siti preesistenti usavano `mul_add` — inclusa l'interpolazione del
quantile in produzione **e** nel suo oracolo di test, che essendo a
specchio rendeva l'incoerenza invisibile ai test di equivalenza.

## Decisione

**La forma non fusa è il contratto numerico del progetto.** Ogni
`a * b + c` resta in due arrotondamenti IEEE, esattamente specificati su
ogni piattaforma. `mul_add`/FMA è vietato nel codice del workspace;
dove clippy suggerisce la fusione (`suboptimal_flops`) si usa l'allow
mirato con il commento standard:

```text
// Niente mul_add/FMA: la fusione cambia l'arrotondamento IEEE e
// violerebbe il determinismo bit-esatto (ADR-0001); la forma non
// fusa e' il contratto numerico.
```

Gli 8 siti preesistenti sono stati allineati (interpolazioni quantile in
`aggregation.rs` produzione e oracolo, binning in `analysis.rs`,
`quantile` in `analysis.rs`, `length_squared` in
`extended_algorithms.rs`, una fixture in `spill.rs`). Il cambio di
risultato è ≤ 1 ULP per sito rispetto al comportamento precedente:
accettato e dichiarato qui; produzione e oracolo sono stati convertiti
nella stessa forma, quindi l'equivalenza bit-a-bit verificata dai test
resta per costruzione.

## Emendamento 2026-08-16 — conversione intero → `f64`: esatta o errore

La review statica del 2026-08-16 ha trovato una seconda incoerenza della
forma numerica, della stessa famiglia ma di impatto maggiore: decine di siti
scrivevano

```rust
value.to_f64().ok_or_else(|| Schema("intero non rappresentabile come f64"))?
```

dichiarando un controllo di rappresentabilità **che non esisteva**.
`num_traits::ToPrimitive::to_f64()` per `i64`/`u64`/`i128` restituisce
sempre `Some`, arrotondando al double più vicino: il ramo di errore era
codice morto e il valore arrotondato proseguiva nella pipeline. Sopra 2^53
due interi distinti diventano lo stesso double — chiavi di `asof_join` che
collassano, filtri che cambiano esito, confronti che invertono l'ordine.

**Decisione.** La conversione intero → `f64` è **esatta o errore**. Gli
helper `exact_f64_from_i64` / `exact_f64_from_u64` / `exact_f64_from_i128`
di `plenora-kernels-table` sono l'unica forma ammessa: verificano che la
parte significativa dell'intero stia in 53 bit, criterio esatto e non
conservativo (2^54, con un solo bit significativo, resta accettato). Il
round-trip ingenuo `value as f64 as i64 == value` **non** è ammesso: agli
estremi il cast satura e `i64::MAX` lo supererebbe pur non essendo
rappresentabile.

Dove il confronto conta, non si converte affatto: `compare_i64`,
`compare_u64`, `compare_i128`, `compare_decimal128` e `scalar_compare`
confrontano nel dominio nativo del tipo — `i128` scalato per i decimal,
millisecondi dall'epoch per i timestamp — senza mai passare da `f64`.

**Arrotondamenti volutamente ammessi**: le operazioni il cui RISULTATO è un
`Float64` per contratto — `table.cast(to: "float")`, `date_diff`,
aggregazioni, statistiche, `expressions` e `formula`. Sono dichiarati sul
sito e l'elenco completo, con il criterio, vive in `docs/deroghe.md`
(DER-005). Ogni percorso che DECIDE — confronti, chiavi, vincoli — è esatto o
fallisce, e per lo più non converte affatto.

## Conseguenze

- Una sola forma numerica nel workspace: il contratto è intero, non
  per-sito. Un futuro confronto bit-a-bit con un componente esterno che
  usi FMA va dichiarato come divergenza, non scoperto a posteriori.
- La scelta è reversibile solo con un nuovo ADR: adottare FMA in modo
  uniforme richiede una decisione esplicita (piattaforme target,
  garanzie della std) e la riconversione misurata di tutti i siti —
  mai introduzioni puntuali.
- Un dato legittimo oltre 2^53 che prima veniva arrotondato in silenzio
  ora produce un errore `Schema`. È il verso voluto (fail-closed), ma è un
  cambiamento osservabile per chi trattava quei valori: la migrazione è
  dichiarare `Float64` in ingresso, non riattivare l'arrotondamento.

## Emendamento 2026-08-16 (secondo giro) — decimali esatti

Il primo emendamento lasciava aperti due punti, entrambi sulla stessa
diagonale: il valore di un `Decimal128` non e' `unscaled`, e'
`unscaled / 10^scale`. Verificare la rappresentabilita' del solo intero non
scalato non dice nulla sul risultato della divisione — `1` con scala `1` vale
`0.1`, che nessun double rappresenta, eppure `unscaled = 1` supera qualunque
controllo sull'intero.

**Conversione.** `exact_f64_from_decimal128` verifica il valore DOPO la
divisione. Il criterio e' esatto: `10^scale = 2^scale * 5^scale`, e una
frazione e' rappresentabile in binario solo se il denominatore ridotto ai
minimi termini e' una potenza di due — serve quindi che `5^scale` divida
esattamente `unscaled`, e che il quoziente stia in 53 bit.

**Confronto.** Un letterale di configurazione in notazione posizionale
(`10.5`, `-0.001`) non diventa piu' un double: `NumericBound::Decimal`
conserva intero non scalato e scala, e il confronto con una colonna
`Decimal128` avviene interamente in `i128`. Restano `F64` solo le forme che
un decimale non e' — notazione esponenziale, infiniti, NaN, o piu' di 38
cifre — dove il valore atteso e' un double per sua natura.

Il confronto fra una colonna `Float64` e un letterale decimale resta invece
nel dominio dei double, ed e' corretto che lo sia: la colonna e'
un'approssimazione per costruzione, e non c'e' un valore esatto da preservare
sul suo lato.
