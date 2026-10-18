# Avvisi di licenza negli artefatti

Il rilascio distribuisce la CLI dentro uno ZIP, la wheel Python e il sorgente
`tar.gz`. Tutti portano `CRATE.json` e gli avvisi crate per crate:
`LICENSES/crate-rust/<nome>-<versione>/` nella CLI e nel sorgente,
`<distribuzione>.dist-info/licenses/crate-rust/` nella wheel.
La licenza del prodotto resta quella di famiglia, source-available Plenora.
Le licenze di terzi non si applicano ai crate del prodotto.

## Raccolta offline e copertura

`scripts/avvisi_crate.py` usa `cargo metadata --offline --locked` e quattro
grafi tramite `cargo tree --offline --locked`: CLI (`plenora-cli`) e wheel
(`plenora-data-py`, feature `pyo3/extension-module`), per
`x86_64-unknown-linux-gnu` e `x86_64-pc-windows-msvc`. Gli archi sono
`normal,no-proc-macro`: niente dipendenze di sviluppo, build o macro
procedurali eseguite sull'host. Cargo tree non ha un'opzione `--release`:
le feature sono quelle delle build release del workflow; il profilo non
cambia la risoluzione delle dipendenze. Un confronto aggiuntivo verifica che
le sonde dei wheel delle prove non cambino il grafo. Sono dipendenze runtime potenzialmente
linkate, non una misura del codice sopravvissuto all'eliminazione del linker.

Il registro `assurance/registries/avvisi-crate.json` elenca separatamente i
quattro grafi. Ogni artefatto porta la loro unione, dichiarata nel manifesto:
120 crate, dei quali 113 di terzi (compresi i tre fork vendor) e sette del
prodotto. Questo sovrainsieme rende disponibili anche gli avvisi delle
dipendenze dell'altro sistema e del binding Python.

I testi sono versionati in `assurance/licenze-crate/`: la raccolta copia
`LICENSE*`, `LICENCE*`, `COPYING*`, `NOTICE*`, `COPYRIGHT*`, `UNLICENSE*`,
anche nelle sottocartelle (incluse le licenze native in zstd-sys), e il
`license-file` del manifesto. Non appiattisce i percorsi. Il registro conserva
fonte e SHA-256, normalizzato solo nei fine riga; il gate confronta anche i
testi disponibili nella cache Cargo o nei fork vendor. Percorsi ambigui,
file vuoti, file mancanti, impronte diverse, testi estranei e grafi obsoleti
sono errori. Il testo di famiglia, quando disponibile in `LICENSE` alla
radice, viene copiato per ciascun crate del prodotto senza modificarlo.

Non serve cargo-about né una ricerca in rete durante la costruzione.
L'approccio deriva da plenora-IO-tools al commit
`9795e7984542c2664c07b9314fd7bceab6e42a83`.

## Assenze: regola e stato

Un'assenza registrata **non è una deroga**: il gate e il confezionamento dei
profili interessati falliscono fino al recupero del testo, verificato con la
sua provenienza (origine https e blob Git ricalcolato dai byte conservati).
Non si afferma che un testo upstream non esista, solo che non è presente
nelle fonti locali consultate.

Oggi il registro non ha assenze: 120 crate, tutti con testi verificati. I
testi non presenti nei pacchetti della cache provengono dall'upstream ai
commit dichiarati nell'origine — `geo-types`, `geozero`, `i_key_sort`,
`i_overlay`, `siphasher` (il cui `COPYING` è di soli rinvii), oltre a
`flatbuffers`, `geo-traits` e `rstar` — con SHA-256 e blob Git nel registro.
I crate del prodotto portano la `LICENSE` di famiglia alla radice del
repository, la stessa di plenora-database-tools; il testo coerente con i
repository pubblici resta una decisione aperta del titolare e il suo
aggiornamento sostituirà il file, non questo meccanismo.

## Comandi e integrazione

Dopo aver popolato la cache (la CI usa `cargo fetch --locked`), tutti i
comandi delle licenze sono offline:

```sh
python scripts/avvisi_crate.py --aggiorna
python scripts/avvisi_crate.py
python -m unittest discover -s scripts -p "test_avvisi_crate.py"
```

`--aggiorna` raccoglie i testi, conserva i recuperi upstream già registrati e
poi esegue il gate. Non cancella silenziosamente testi obsoleti: il gate li
segnala e occorre riconciliare il registro e la cartella.

Il workflow `.github/workflows/rilascio.yml` finalizza gli artefatti prima
delle prove e delle attestazioni:

```sh
python scripts/licenze_artefatto.py --tipo cli --input dist/plenora-data-linux-x86_64 --output dist/plenora-data-linux-x86_64.zip
python scripts/licenze_artefatto.py --tipo wheel --input dist/pacchetto.whl --output dist/pacchetto.whl
python scripts/licenze_artefatto.py --tipo sorgente --input dist/source.tar.gz --output dist/source.tar.gz
python scripts/check-licenze-artefatto.py --tipo cli --input dist/plenora-data-linux-x86_64.zip
```

Gli ultimi due tipi accettano anche la verifica senza `--output`. La wheel
viene riscritta prima della firma, con `RECORD` aggiornato su tutti i byte
finali, e poi installata per le prove. Gli ZIP mantengono il bit eseguibile
della CLI; il sorgente conserva i modi dei file di `git archive`.
I binari sciolti sono usati per le prove, poi rimossi da `dist`: gli allegati,
il manifesto di adozione e le attestazioni indicano gli ZIP.

La verifica legge gli archivi senza estrarli e confronta manifesto e testi
con il registro del checkout: rimuovere un crate anche da `CRATE.json` non
rende verde l'artefatto. La copertura del grafo corrente è verificata dal
gate Cargo in CI e di nuovo prima di confezionare. La verifica degli
artefatti scaricati si ripete prima della pubblicazione senza richiedere
Cargo. Anche `sdk-python.yml` finalizza i wheel prima di caricarli come
artefatti della run. Una build maturin grezza è un intermedio: solo la wheel finalizzata e
verificata è un artefatto di rilascio.

Le dipendenze Python installate separatamente (PyArrow) portano i propri
avvisi. L'inventario non pretende di coprire runtime del sistema operativo o
librerie native aggiunte da strumenti esterni; copre i crate e i testi nativi
presenti nei loro pacchetti. Le dipendenze build/dev non sono distribuite come
codice compilato da questo rilascio; lo SBOM continua a inventariarle a parte.
