# AGENTS.md

Le regole che non si negoziano. Il resto lo dice il codice.

## Regole

1. **Niente failure silenziose.** Un risultato sbagliato è peggio di un
   errore. Ordinamenti, confronti, conversioni numeriche e formati sono esatti
   per costruzione; un caso limite non gestibile si rifiuta con un errore
   esplicito, mai con un valore plausibile. L'unico errore piccolo accettato
   è la **precisione geografica di 1 cm a terra** (README, «Limiti
   dichiarati»; analogo del modello a precisione fissa di GEOS o di
   `gridSize = 0.01` di PostGIS): sotto il centimetro un risultato
   geometrico può differire dall'esatto (vertici spostati, schegge e parti
   sottili fuse o sparite, aree diverse di circa perimetro per 1 cm); sopra,
   ogni errore è esplicito. Un calcolo che sposterebbe il risultato oltre
   1 cm (griglia di overlay con il suo aggancio, punto di noding
   arrotondato) si rifiuta.
2. **Niente `unsafe`** (`unsafe_code = "forbid"` nel workspace). Una
   dipendenza nuova entra solo con una motivazione scritta accanto al pin in
   `Cargo.toml`, e con versione esatta (`=x.y.z`).
3. **Ogni ottimizzazione di un kernel ha un oracolo**: un test differenziale
   o proptest che la confronta con il percorso generico, input per input.
4. **Determinismo**: stesso input, stesso output. Nessun ordine che dipenda
   da hash, thread o tempo.
5. **Errori senza dati**: mai valori di righe o colonne nei messaggi
   d'errore, nemmeno in diagnostica.
6. **Seconda revisione** solo per la logica critica: ordinamenti, confronti,
   conversioni numeriche, aggregazioni, serializzazione.
7. **Prima del commit**: fmt, clippy con `-D warnings`, clippy anti-panic
   sulle librerie, test completi. Tutti verdi. Un gate non eseguito si
   dichiara non eseguito.

## Comandi

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo clippy --workspace --lib --locked -- -D unsafe-code \
  -D clippy::unwrap_used -D clippy::expect_used -D clippy::panic \
  -D clippy::unreachable -D clippy::todo -D clippy::unimplemented
cargo test --workspace --locked
```

Toolchain fissata in `rust-toolchain.toml` (1.98.0).
