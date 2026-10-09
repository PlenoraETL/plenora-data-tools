# Errori

Ogni errore è un `PlenoraError` con i quattro assi del contratto
`plenora-error-v1` (`plenora-contracts`, `specs/errors/ERRORS-1.0.md`):
categoria, fase, effetto sul supporto e ritentativo. Le enumerazioni sono
quelle dello schema (`concurrent_modification` e il ritentativo
`quarantine` ci sono anche se nessun errore del workspace li produce).
`PlenoraError::public_projection` dà il documento pubblico (`PublicError`,
serializzabile con serde), valido contro `schemas/error-v1.schema.json`
(test `crates/plenora-core/tests/errore_pubblico_schema.rs`, con gli schemi
copiati da `plenora-contracts@3c395a8` (tag `v1.1.0`) e verificati per SHA-256):

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

Neanche il testo di una dipendenza attraversa il messaggio quando può
citare un valore. Una config di un passo che serde rifiuta
(`config non valida: …`) si descrive con
`plenora_core::json::descrivi_errore_config`: il nome di un campo mancante
o ripetuto, l'elenco dei campi o dei valori ammessi per un campo o un
valore sconosciuto (mai quello scritto), i messaggi fissi del workspace;
un tipo, un valore o una lunghezza non validi danno il solo genere («tipo,
valore o forma di un campo non validi»), senza il campo né il valore. Una
regex scritta nel piano che il crate `regex` rifiuta dice solo se è la
sintassi o il limite di dimensione, mai il testo del crate, che riporta il
pattern.

Lo stesso vale per i testi che il workspace scrive da sé: un letterale
della config fuori elenco (l'unità di `date_trunc`, il `data_type` di
`assert_schema`) si rifiuta con l'elenco dei valori ammessi, un fuso orario
non riconosciuto (`timezone_convert`, `type_cast`) con un messaggio fisso,
una chiave JSON ripetuta nello stesso oggetto con la sola posizione nel
documento: le chiavi di `mapping` di `lookup` sono valori dei dati.
Restano nel messaggio il contesto che serve a trovare l'errore: il passo,
l'operazione, la colonna per nome, il nome di una regola, e i limiti
numerici configurati (`max_candidates`, `max_points`, `max_issues`, i
`limits.max_*` del piano, il budget di memoria) nei messaggi di
superamento: sono parametri di configurazione, non valori di righe o
colonne, e senza di loro il superamento non si diagnostica.

## Effetto di un errore a metà della scrittura

`remote_effect` è `none` per costruzione (ogni file d'uscita è scritto in
modo atomico), tranne dove un confine dichiara di più con
`PlenoraError::with_remote_effect`: oggi solo `esegui_da_file`, che scrive
gli output uno alla volta e marca `partial` un errore dopo il primo output
scritto (i precedenti restano), e `committed` un'interruzione vista dopo
l'ultimo (`esegui_da_file_interrompibile`). Con un effetto già visibile un ritentativo
automatico (`safe`, `after`, `requires_idempotency_key`) diventa
`requires_recovery`; una causa che non si ritenta mai resta `never`.
Fa eccezione la pulizia fallita dopo una pubblicazione provata (fase
`cleanup`, effetto `committed`, oggi solo la cartella temporanea di
`data.run` 3): è `never` con qualunque causa, perché il residuo è solo
locale e un nuovo tentativo pubblicherebbe di nuovo (ERR-015). «Residuo
remoto» in ERR-015 è un residuo sul sistema remoto oggetto
dell'operazione, cioè la destinazione degli artefatti. La cartella
temporanea di lavoro del processo è un residuo locale su qualunque
filesystem si trovi, anche di rete: ritentare l'operazione la
ripubblicherebbe senza pulirla. La prova
(`crates/plenora-cli/src/artefatti.rs`) blocca davvero la rimozione e
controlla gli assi anche nella proiezione pubblica. Il
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
