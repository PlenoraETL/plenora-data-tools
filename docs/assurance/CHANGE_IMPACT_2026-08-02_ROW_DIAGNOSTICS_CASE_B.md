# Change impact analysis — row diagnostics Data Tools, Case B

Data: 2026-08-02.

> Record storico del vertical slice Case B. Lo stato successivo e la chiusura
> dei gap Data Tools sono descritti in
> `CHANGE_IMPACT_2026-08-03_ROW_DIAGNOSTICS_PUBLIC_OUTPUTS.md`.

## Baseline e fonte normativa

- baseline funzionale: `af812aaf30f7a189dbdafe78429ef628636bec70`;
- Contracts 2.0-rc17: `5dc445ee3e6a6af277d4e2685afd1bc969fbe664`;
- contratto: `plenora-row-diagnostics-v1`.

I manifesti e gli SHA delle release precedenti restano storici e immutati. Questo
incremento richiede una nuova qualificazione e non eredita la CI del candidato
`1.0.2`.

## Problema

`table.type_cast` usava `errors=coerce` come default. Una data non valida veniva
trasformata in null e pubblicata nell'unico output, indistinguibile da un null
sorgente. Con `errors=raise` veniva riportato soltanto il primo errore in prosa,
senza indice sorgente o conteggi.

## Decisione del vertical slice

Per il percorso `Utf8 -> Date32` con `errors=coerce|raise`:

- nessuna riga con data non valida entra nell'output accettato;
- il kernel produce una diagnostica bounded con causa
  `conversion.invalid_date`, colonna e indice sorgente checked;
- il wrapper `PlenoraError::RowDiagnostics` conserva il payload attraverso i
  tag di fase senza alterare categoria, effetto o retry;
- il segmento streaming continua a drenare dopo il primo rifiuto, sopprime ogni
  output successivo e fonde i report in ordine sorgente;
- l'errore terminale viene emesso solo a scansione completa;
- un'interruzione dopo rifiuti osservati produce `completeness=partial`, omette
  `total` e dichiara `data_tools.input_stream_interrupted` o
  `data_tools.processing_interrupted`;
- l'envelope CLI aggiunge `error.row_diagnostics` solo quando presente.

La deviazione controllata dalla regola generale first-error-wins è registrata in
`docs/adr/ADR-0003-failure-cancellazione-crash.md`.

## Evidenza TDD

- RED: `errors=coerce` restituiva un batch con null e non esponeva
  `row_diagnostics`;
- GREEN: fixture 1.025 righe con difetti 4/1004, anche su due batch;
- offset esplicito del kernel: indice locale 4 + offset 1.000 = 1.004;
- report cross-batch completo con conteggio 2 ed esempi 4/1004;
- interruzione dopo il primo rifiuto: report partial con limite di conoscenza;
- carrier preservato attraverso `table_engine::execute_batch`;
- carrier preservato anche nel replay `StoredEdgeError` degli archi condivisi,
  tramite snapshot boxed di categoria, fase, effetto, retry e contesto; il test
  copre `DataMapping`, `Execution`, `Cancelled` e un `Tagged<Io>` non diagnostico;
- un test `EdgeShared` reale con due lettori verifica cancellation, fase read,
  contesto, esempi row-scoped e identità del Display sia sull'errore originale
  sia sul replay, partendo da un execution id vuoto e applicando lo stesso tag
  di confine; verifica inoltre che un ID preesistente non venga sovrascritto;
- lo snapshot usa `execution_location`, conserva il reason semantico anche con
  ID inizialmente vuoto e rigenera il Display canonico quando il confine
  assegna l'execution id; primo ramo e replay non divergono;
- il report terminale DAG conserva node, operation ed execution_id; dopo il
  primo rifiuto i batch successivi eseguono solo fino al nodo diagnostico e
  non raggiungono kernel downstream;
- il planner rifiuta `Date32 + Coerce/Raise` dopo qualunque altro nodo finché non
  esiste provenance row-level; il percorso diretto resta ammesso;
- cancellation wrapped conserva categoria, contesto DAG ed exit semantics
  130 anche attraverso fan-out/replay;
- la CLI legacy rifiuta `Date32 + Coerce/Raise` e richiede DAG v4, evitando report
  falsamente completi su stream non drenati;
- ogni report viene validato internamente prima dell'attach; un payload non
  conforme non è serializzabile e l'envelope degrada fail-closed a `Internal`;
- l'exit code deriva dalla categoria dell'envelope effettivamente emesso: una
  cancellation con diagnostics invalide degradata a `Internal` esce con 2,
  non con il codice cancellation 130;
- il merge è transazionale: overflow/incompatibilità non mutano l'aggregate e
  le osservazioni precedenti restano disponibili come report `partial`;
- envelope CLI additivo e assente sugli errori ordinari.

## Evidenza eseguita

- suite workspace senza PROJ: tutti i target verdi; i due oracle geo lunghi sono
  stati inoltre rieseguiti isolati dopo il timeout globale;
- suite `plenora-cli`, `plenora-engine` e `plenora-kernels-geo` con
  `--features plenora-cli/proj-backend`: verde;
- Clippy workspace `--all-targets` con PROJ e `-D warnings`: verde;
- gate produzione `unsafe`/`unwrap`/`expect`/`panic`/`unreachable`/`todo`/
  `unimplemented`: verde;
- replay CLI reale sul corpus rc17 in due batch 1.000+25 sul binario corrente:
  exit 2, nessun output pubblicato, report con indici 4/1004 e conteggio 2,
  contesto DAG completo;
- validatore Contracts + oracle esatto:
  `DATA_CASE_B_FINAL_SHA_SCHEMA_SEMANTICS_ORACLE_CONTEXT_NO_OUTPUT_PASS`.
- dopo le correzioni finali al validator/merge: `plenora-core` 82/82,
  `plenora-engine --lib` 334/334, test envelope CLI e Clippy focalizzati verdi;
  dopo la correzione P1 execution-id, test carrier e Clippy Core/Engine sono
  verdi e la suite workspace completa current-tree con PROJ è terminata con
  exit code 0 senza target falliti; anche Clippy workspace `--all-targets`
  con PROJ e `-D warnings` è verde. Il successivo rafforzamento test-only del
  tracer `EdgeShared` è verde con Clippy Engine; anche il rerun workspace con
  PROJ sul tree comprensivo di tale test è terminato senza target falliti.
- la matrice Core rc17 copre inoltre report write completi/outcome ignoti,
  riproducendo campo per campo i due oracle write: una rejection osservata su
  5.200 input e partizione effetti 1/200/4.999/0, con rollback outcome noto o
  ignoto; copre inoltre overflow checked, key policy, limiti Unicode, knowledge
  limits duplicati e write state mancanti (8/8 test mirati verdi).

Il `cargo fmt --all -- --check` locale sotto WSL segnala un diff baseline-wide
su file non modificati (profilo rustfmt/line wrapping del checkout Windows). Non
è stato applicato: il diff funzionale resta limitato ai file dichiarati e
`git diff --check` è verde. Il formatter CI su clone Linux resta un gate della
futura PR.

## Limiti aperti — bloccanti per dichiarare Data Tools conforme

Questo vertical slice **non qualifica ancora l'intero modulo**:

1. gli altri target e sorgenti di `table.type_cast` conservano ancora le
   politiche storiche e devono essere convertiti a rifiuto motivato;
2. `Date32 + Coerce/Raise` è conservativamente limitato all'input diretto del
   componente; per ammetterlo dopo altri nodi serve un sidecar provenance
   row-level. Il planner rifiuta questi piani invece di emettere indici falsi;
3. gli altri nodi tabellari e geografici devono essere censiti per coercizioni,
   null sintetici, drop impliciti e value validation mancante;
4. un'uscita esplicita `accepted/rejected` non esiste ancora; senza tale nodo il
   comportamento resta fail-closed sull'intera operazione;
5. review indipendenti completate con correzione dei finding High/Medium; CI
   same-SHA e catene cross-library sono ancora da completare;
6. `PlenoraError` è un enum pubblico: il nuovo variant è source-breaking per
   consumer con match esaustivi. Prima di una release serve una decisione semver
   esplicita (major oppure carrier compatibile approvato e ridisegnato);
7. indici e conteggi fisicamente osservabili usano `u64`: payload JSON con
   interi oltre `u64::MAX`, pur rappresentabili da runtime ad aritmetica
   arbitraria, vengono rifiutati in deserializzazione anziché perdere precisione
   o avvolgere il valore.

Nessun tag, release, merge o claim di conformità completa è autorizzato da
questo record.
