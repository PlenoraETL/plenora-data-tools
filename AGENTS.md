# AGENTS.md

Il documento che si legge per primo. Non descrive il repository — quello lo fa
il repository — ma dice **che cosa non è negoziabile** e **dove sta il resto**.

## Le regole che non cambiano

Sono policy, non fatti del codice: non si generano da nessuna parte, e questo è
l'unico posto in cui sono scritte.

1. **Niente failure silenziose.** Un risultato sbagliato è peggio di un
   errore. Ordinamenti, confronti, conversioni numeriche e formati sono esatti
   per costruzione; un caso limite non gestibile si rifiuta con un errore
   esplicito, mai con un valore plausibile. L'unico errore piccolo accettato
   è la **precisione geografica di 1 cm a terra**
   ([`docs/limiti.md`](docs/limiti.md#precisione-delle-operazioni-geografiche-1-cm-a-terra);
   analogo del modello a precisione fissa di GEOS o di `gridSize = 0.01` di
   PostGIS): sotto il centimetro un risultato geometrico può differire
   dall'esatto (vertici spostati, schegge e parti sottili fuse o sparite,
   aree diverse di circa perimetro per 1 cm); sopra, ogni errore è
   esplicito. Un calcolo che sposterebbe il risultato oltre 1 cm (griglia di
   overlay con il suo aggancio, punto di noding arrotondato) si rifiuta.
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
   dichiara non eseguito. Prima del merge in `main` la CI
   (`.github/workflows/ci.yml`) è verde su Linux **e** Windows.

## Dove sta il resto

| serve | sta in |
| --- | --- |
| che cosa il codice dichiara oggi | [`docs/inventario.md`](docs/inventario.md) — generato |
| le operazioni | [`docs/operazioni.md`](docs/operazioni.md) — generato dalle schede |
| dove le garanzie si fermano | [`docs/limiti.md`](docs/limiti.md) e i limiti dichiarati di ogni guida |
| l'indice delle guide | [`docs/README.md`](docs/README.md) |
| come si costruisce e che cosa si esegue | [`README.md`](README.md) |
| perché una decisione è stata presa | `git log` |

## Prima di dire «fatto»

I comandi stanno nel [`README.md`](README.md#cosa-fa-girare-le-prove) e in
`.github/workflows/ci.yml`: sono la fonte, e ricopiarli qui li farebbe
divergere al primo cambiamento. Comprendono le guardie Python di documenti,
commenti e modelli di costo, che la CI esegue con gli altri gate.

La suite lunga ([`README.md`, «Suite lunga»](README.md#suite-lunga)) è
obbligatoria prima del merge: la suite di default ne gira un sottoinsieme
deterministico.

Toolchain fissata in `rust-toolchain.toml` (1.98.0).
