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

## Conseguenze

- Una sola forma numerica nel workspace: il contratto è intero, non
  per-sito. Un futuro confronto bit-a-bit con un componente esterno che
  usi FMA va dichiarato come divergenza, non scoperto a posteriori.
- La scelta è reversibile solo con un nuovo ADR: adottare FMA in modo
  uniforme richiede una decisione esplicita (piattaforme target,
  garanzie della std) e la riconversione misurata di tutti i siti —
  mai introduzioni puntuali.
