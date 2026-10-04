# Errori

Ogni errore è un `PlenoraError` con i quattro assi del contratto
`plenora-error-v1` (`plenora-contracts`, `specs/errors/ERRORS-1.0.md`):
categoria, fase, effetto sul supporto e ritentativo. Le enumerazioni sono
quelle dello schema (`concurrent_modification` e il ritentativo
`quarantine` ci sono anche se nessun errore del workspace li produce).
`PlenoraError::public_projection` dà il documento pubblico (`PublicError`,
serializzabile con serde), valido contro `schemas/error-v1.schema.json`
(test `crates/plenora-core/tests/errore_pubblico_schema.rs`, con gli schemi
copiati da `plenora-contracts@4c1569d` e verificati per SHA-256):

- `message` è il testo dell'errore, già senza valori di righe o colonne,
  troncato a 2048 caratteri e mai vuoto. Per un errore di I/O il testo del
  sistema operativo (che può portare percorsi completi) resta nel solo
  `Display` locale: il messaggio pubblico è un testo fisso per
  `ErrorKind` con i soli contesti nostri davanti («io error: output `b`:
  entity not found», `PlenoraError::io_con_contesto`);
- `code`, dove l'errore ne ha uno stabile: `EXECUTION_DEADLINE_EXCEEDED`
  (`Timeout`), `EXECUTION_CANCELLED` (`Cancelled`), e il codice di
  `CrsError` (`CrsError::code`, `match` esaustivo) per un errore CRS nato
  da un `CrsError` (`PlenoraError::CrsCoded`, codice tipizzato, mai letto
  dal messaggio; `CodiceCrs` si ottiene solo da `CrsError::code`, quindi
  un codice fuori dal pattern dello schema non si costruisce). Un
  `PlenoraError::Crs` di solo testo non ha codice;
- `details.row_diagnostics`: il documento `plenora-row-diagnostics-v1`
  intero, se l'errore ha una diagnostica per riga;
- `details` oltre i limiti ERR-011/ERR-012 (byte, profondità, 128
  proprietà o elementi, stringhe da 4096 byte, 2048 nodi) non si tronca: la
  proiezione diventa un errore `internal` con codice
  `ERROR_DETAILS_NOT_PUBLISHABLE`, senza `details`. Con i limiti di esempi
  dei kernel (10) non scatta;
- effetto `unknown` con un ritentativo automatico diventa
  `requires_recovery` (ERR-006); oggi nessun errore ha effetto `unknown`.

`provider` ed `execution_id` non ci sono: nessun errore del workspace ne
ha uno.

Gli errori di I/O si classificano per `ErrorKind`: `TimedOut` →
`timeout`, `Interrupted`/`WouldBlock`/`ResourceBusy` → `transient`, tutti
ritentabili (`safe`); `NotFound` → `not_found`, `PermissionDenied` →
`authorization`, `AlreadyExists` → `conflict`, spazio, quota, file troppo
grande o memoria → `resource_limit`, `Unsupported` → `unsupported`, il resto
→ `io`, tutti `never`. Un `ErrorKind` non classificato è `io` e `never`, la
scelta prudente. Un errore di serde_json che nasce dall'I/O della lettura
resta `Io` con il suo `ErrorKind`; uno di sintassi, di dati o di fine
inattesa è `data_mapping` con il solo genere e la posizione («json error:
dati alla riga 1 colonna 15»), mai il testo che cita il valore letto.

## Effetto di un errore a metà della scrittura

`remote_effect` è `none` per costruzione (ogni file d'uscita è scritto in
modo atomico), tranne dove un confine dichiara di più con
`PlenoraError::with_remote_effect`: oggi solo `esegui_da_file`, che scrive
gli output uno alla volta e marca `partial` un errore dopo il primo output
scritto (i precedenti restano), e `committed` un'interruzione vista dopo
l'ultimo (`esegui_da_file_interrompibile`). Con un effetto già visibile un ritentativo
automatico (`safe`, `after`, `requires_idempotency_key`) diventa
`requires_recovery`; una causa che non si ritenta mai resta `never`. Il
primo effetto dichiarato vince anche sotto i wrapper di fase e di
diagnostica.

La scrittura atomica cancella il temporaneo di una scrittura fallita e ne
controlla l'esito: se la cancellazione fallisce l'errore ha effetto
`unknown` (e con una causa ritentabile `requires_recovery`), anche sopra un
effetto che la causa dichiarava già (`PlenoraError::override_remote_effect`,
l'unico punto in cui un effetto si sostituisce).

- **Temporaneo rimasto dopo una scrittura fallita.**
  *Regola*: un errore di `scrivi_atomico` non lascia nulla alla
  destinazione e cancella il temporaneo `.plenora-io-*.tmp` accanto a lei.
  *Ambito*: `plenora_io::atomico::scrivi_atomico` (ogni scrittura di
  `plenora-io`).
  *Hazard*: se la cancellazione fallisce (permessi cambiati, file bloccato
  da un altro processo), o se il processo muore prima di cancellarlo, il
  temporaneo con i dati scritti fin lì resta nella directory della
  destinazione. Nel primo caso l'errore dice `unknown`; nel secondo non c'è
  errore da leggere.
  *Rientro*: una pulizia dei `.plenora-io-*.tmp` orfani all'avvio, o
  temporanei fuori dalla directory della destinazione dove la rinomina
  atomica lo permette.
