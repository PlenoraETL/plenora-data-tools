# Riproducibilità della documentazione generata

`docs/kernel-signatures.md` è **generato** da `docs/_build/assemble.py` a
partire dal catalogo (`crates/plenora-core/src/catalog.rs`) e dai frammenti
(`docs/_fragments/*.md`). Deve coincidere byte per byte con la rigenerazione,
e la CI lo verifica a ogni push.

```bash
python docs/_build/assemble.py            # rigenera
python docs/_build/assemble.py --verify   # confronta senza toccare il worktree
```

Nessuna dipendenza: solo la libreria standard di Python. Nessun pacchetto da
installare, nessun container, nessun font, nessuno stato della macchina.

## Perché serviva

Il generatore è rimasto **rotto per mesi in silenzio**. `rustfmt` aveva
riformattato la macro `op!` del catalogo su più righe, la regex del parser
aveva smesso di agganciarla, e lo script usciva con `MISMATCH` senza
riscrivere nulla. I verbali di review del 2026-08-18 lo registravano già
(«`docs/_build/assemble.py` è fuori uso, già su HEAD») e la CI restava verde
lo stesso, perché nessuno rigenerava.

Un generatore che si ferma non rompe niente di visibile: il documento resta
com'era e sembra aggiornato. Solo un confronto lo scopre.

## Che cosa è fissato

| sorgente di variazione | come è chiusa |
|---|---|
| formato della macro `op!` | parser a parentesi, non più una regex: regge multilinea, annidamento e chiavi opzionali in qualsiasi ordine |
| fine riga | LF esplicito, scritto in **byte**, come impone `.gitattributes` (`eol=lf`) |
| conteggi nell'intestazione | calcolati dal catalogo, non ricordati a mano |
| divario fra catalogo e documento | `NON_DOCUMENTATE`, un'asserzione: se cambia, la generazione fallisce |
| dipendenze esterne | nessuna |

Il confronto è in byte e non in testo di proposito: `write_text`/`read_text`
su Windows traducono le fine riga in entrambi i versi, quindi un confronto
testuale passerebbe su un file che byte per byte è diverso da quello prodotto
su Linux.

## Che cosa il generatore rifiuta

Ogni condizione qui sotto ferma la generazione con la propria diagnosi,
invece di produrre un documento incompleto che sembra completo:

| condizione | perché non può passare in silenzio |
|---|---|
| **zero operazioni** lette dal catalogo | non è un catalogo vuoto: è il parser che non aggancia più |
| chiave opzionale di `op!` **sconosciuta** | l'operazione dichiara qualcosa che il documento non riporterebbe |
| variante `CrsRequirement` **sconosciuta** | un requisito CRS reale sparirebbe dalla firma |
| **frammento duplicato** (stesso id in due file) | una delle due firme sparirebbe, e nessuno saprebbe quale |
| **operazione ripetuta** nelle sezioni | comparirebbe due volte e falserebbe i conteggi dell'intestazione |
| operazione documentata **assente dal catalogo**, o senza frammento | il documento parlerebbe di ciò che non esiste |
| **divario** catalogo/documento diverso da quello dichiarato | una nuova operazione non documentata resterebbe invisibile |

## Le regressioni girano sempre

`autotest()` viene eseguito a ogni invocazione dello script, non in una suite
che qualcuno deve ricordarsi di lanciare — costa microsecondi, e il guasto che
ha motivato tutto questo era esattamente del tipo che nessuno va a cercare.

Copre: invocazione su una riga, invocazione multilinea in forma `rustfmt`,
invocazione annidata con `Some(...)`, `&[...]` e più chiavi opzionali,
catalogo senza operazioni, chiave opzionale sconosciuta, variante CRS
sconosciuta, frammenti duplicati (e frammenti distinti, che devono passare),
sezioni senza ripetizioni.

## La prova che il gate non è vacuo

Un gate mai visto fallire non è un gate. Le mutazioni provate su una copia del
repository, ciascuna rifiutata con la propria diagnosi:

| mutazione | esito |
|---|---|
| una riga alterata a mano in `kernel-signatures.md` | rifiutata: numero della prima riga divergente, e i due contenuti |
| macro `op!` rinominata nel catalogo | rifiutata: «zero operazioni lette… è il parser a non agganciarlo più» |
| variante `CrsRequirement` inventata | rifiutata, con il nome della variante |
| stesso id in due frammenti | rifiutata, con i due file che se lo contendono |

La copia intatta passa, e il worktree originale resta invariato.

## Il gate in CI

Job `docs-riproducibili` in `.github/workflows/ci.yml`. Copia il repository in
`$RUNNER_TEMP/doc-check`, rigenera **lì**, confronta, e infine controlla con
`git status --porcelain --untracked-files=all` che **la copia** non sia stata
toccata — non il worktree originale, che il gate non ha comunque mai usato.

Un gate che rigenera il file che sta verificando non verifica nulla: dichiara
verde qualunque cosa produca.

## Il PDF non esiste più

Fino al 2026-08-21 esisteva anche `docs/kernel-signatures.pdf`, generato da
`md2pdf.py`. Non era riproducibile — caricava Arial da `C:/Windows/Fonts` e
lasciava che `fpdf2` timbrasse l'ora corrente, quindi due esecuzioni
consecutive sulla stessa macchina davano già file diversi — e renderlo tale
avrebbe richiesto di vendorizzare font, licenze e un lock del grafo Python.

Decisione del maintainer: il PDF è **documentazione obsoleta, non una capacità
di data-tools**. Pipeline rimossa. Il Markdown è l'unica documentazione
generata, e non ha dipendenze.
