# AGENTS.md

Il documento che si legge per primo. Non descrive il repository — quello lo fa
il repository — ma dice **che cosa non è negoziabile** e **dove sta il resto**.

## Le regole che non cambiano

Questo progetto si tratta come una libreria safety-critical (direttiva del
maintainer, 2026-07-27). Sono policy, non fatti del codice, e questo è l'unico
posto in cui sono scritte.

1. **Niente failure silenziose.** Un risultato sbagliato è sempre peggio di un
   errore. Ordinamenti, confronti, conversioni numeriche e formati dati sono
   esatti per costruzione; un caso limite non gestibile si rifiuta con un
   errore esplicito.
2. **I test verdi non bastano.** Una modifica a logica critica — contabilità,
   comparatori, serializzazione, concorrenza — si rilegge da un
   secondo lettore, non solo nella sintesi di chi l'ha scritta.
3. **Ogni bug è una classe.** Trovato un difetto, si cerca la stessa classe in
   tutto il codebase prima di chiudere la correzione.
4. **Deviazioni esplicite.** Uno scostamento da un contratto o da un
   invariante si scrive nel codice **e** in
   [`docs/errori-e-limiti.md`](docs/errori-e-limiti.md), con regola, ambito,
   hazard e condizione di rientro. Una garanzia indebolita si dichiara come
   tale.
5. **Determinismo testato**
   ([`architettura.md#determinismo`](docs/architettura.md#determinismo)):
   stesso input, stesso output. Ordine logico (`BatchSequence`), mai
   temporale; un'ottimizzazione si verifica con un oracolo contro il percorso
   generico.
6. **Nessun `unsafe`** nel workspace. Nessuna dipendenza nuova senza una
   motivazione documentata; versioni pinnate esatte.
7. **Suite completa prima del commit**, e CI verde su Linux e Windows. I
   comandi sono nel [`README.md`](README.md#cosa-fa-girare-le-prove).
8. **Errori senza dati.** Mai valori di righe o colonne nei messaggi d'errore,
   nemmeno in diagnostica (regola di `plenora-core/src/error.rs`).

## Dove sta il resto

| serve | sta in |
| --- | --- |
| che cosa manca, in ordine | [`docs/stato-e-roadmap.md`](docs/stato-e-roadmap.md) — autorità unica sullo stato |
| dove le garanzie si fermano | [`docs/errori-e-limiti.md`](docs/errori-e-limiti.md) — il registro dei limiti |
| crate, flusso, determinismo, memoria, backend | [`docs/architettura.md`](docs/architettura.md) |
| il formato del piano | [`docs/piano-v5.md`](docs/piano-v5.md) |
| comandi, canali, exit code | [`docs/cli.md`](docs/cli.md) |
| le operazioni | [`docs/operazioni.md`](docs/operazioni.md) — generato con `python docs/_build/assemble.py`, non si modifica a mano |
| gate, piattaforme, procedura di rilascio | [`docs/release.md`](docs/release.md) |
| il profilo isolato | [`docs/isolamento.md`](docs/isolamento.md), con le misure dei prototipi in [`docs/prototipi-isolamento.md`](docs/prototipi-isolamento.md) |
| il catalogo delle operazioni | lo snapshot `crates/plenora-engine/tests/catalog_snapshot.snap`: ogni cambio è esplicito in PR |
| come si costruisce e che cosa si esegue | [`README.md`](README.md) |
| perché una decisione è stata presa | `git log` |

La superficie documentale è **chiusa**: un Markdown nuovo entra solo
aggiornando l'allowlist di `scripts/verifica_documentazione.py`, che è
l'autorità sul conteggio.

## Prima di dire «fatto»

I comandi stanno nel [`README.md`](README.md#cosa-fa-girare-le-prove) e nei
workflow sotto `.github/workflows/`: sono la fonte, e ricopiarli qui li farebbe
divergere al primo cambiamento. Ciò che questo documento aggiunge è l'ordine di
lettura del risultato:

- i gate della CI devono essere verdi su Linux **e** Windows;
- il verde autoritativo dell'isolamento arriva solo dalla VM Linux dedicata,
  con cgroup v2 e sottoalbero delegato: un container non lo prova;
- se un gate non è stato eseguito, si dice che non è stato eseguito. Un gate
  saltato non è un gate passato.
