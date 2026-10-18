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

## Assenze che bloccano il rilascio

La raccolta offline ha prodotto 217 file per 109 crate di terzi. Sette crate
non portavano testi nella cache. Per `flatbuffers 25.12.19`, `geo-traits 0.3.0`
e `rstar 0.12.2`, cinque testi provengono dal commit locale di riferimento:
SHA-256 e blob Git verificati, URL upstream e provenienza conservati nel
registro. Nessun accesso alla rete.

Restano senza testo locale:

- `geo-types 0.7.19`;
- `geozero 0.15.1`;
- `i_key_sort 0.11.0`;
- `i_overlay 9.0.0`;
- i sette crate del prodotto: manca il testo autorevole della licenza di
  famiglia nel checkout; non viene inventato né sostituito.

Ogni assenza ha una ragione nel registro, con il commit di
`.cargo_vcs_info.json` per i quattro crate esterni. **La registrazione non è
una deroga**: il gate e il confezionamento falliscono fino al recupero dei
testi. Non si afferma che i testi upstream non esistano, solo che non sono
presenti nelle fonti locali consultate. Per sbloccare il rilascio occorrono
quei testi verificati con la loro provenienza e il testo Plenora autorevole.

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
