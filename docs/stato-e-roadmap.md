# Stato e roadmap

Solo il **lavoro ancora aperto**. Quello che è fatto sta nel codice, nei test
e negli altri documenti; qui c'è ciò che manca, in ordine.

## Dove siamo

Il core è una release candidate credibile: il piano v5 è il formato canonico,
la CLI è l'eseguibile distribuito, le 146 operazioni del catalogo sono
documentate, e la CI verifica Linux e Windows con quindici job.

Non è ancora rilasciabile in produzione. Le ragioni sono qui sotto, in
ordine di precedenza.

---

## 1. Reset documentale — **completato**

La documentazione descrive ora soltanto ciò che il progetto è, i contratti
validi, i limiti aperti e la strada verso la produzione. La storia resta in
Git, non nel worktree: nessun archivio, nessun file conservato «per memoria».

La superficie pubblica è chiusa da un gate che rifiuta ogni Markdown fuori
dalla allowlist, i collegamenti rotti e i riferimenti a documenti eliminati.

## 2. La memoria governata non è un tetto duro

**È il limite più serio del progetto**, ed è la ragione per cui il core non è
in produzione.

Il budget governa la **ritenzione** del risultato, non la sua costruzione:
dove il lease è preso dopo l'allocazione — quasi ovunque — un input che
produce un output molto più grande del budget porta all'esaurimento della
memoria **prima** che l'errore esista. Il fallimento è un OOM, non un errore
diagnosticabile. Il quadro completo è in
[`errori-e-limiti.md`](errori-e-limiti.md).

Due strade, entrambe accettabili, nessuna parziale:

1. un **preflight conservativo per ogni operazione**, sul modello di quello
   già esistente per `cross_join`, `concat`, `concat_by_name` e `melt`, con un
   modello per operazione e la copertura delle allocazioni temporanee che
   ciascuna fa;
2. un'**autorizzazione ad allocare** al posto del lease a posteriori:
   reservation preventiva sufficientemente conservativa in entrambi gli
   executor.

I prerequisiti sono già in piedi: il governor ha un permesso atomico —
verifica e prenotazione in una sola operazione — e la contabilità è
linearizzabile. Ciò che manca è spostare le allocazioni, che è il lavoro vero.

Il **profilo isolato** (esecuzione in un processo worker con un limite imposto
dal sistema operativo) è la risposta al livello 2 del contratto, e non è
ancora iniziato: nessun meccanismo è stato prototipato, e su macOS resterà non
supportato finché un prototipo non dimostri copertura *e* attribuzione.

## 3. Campagna fuzz finale

Alla chiusura del punto 2, non prima: una campagna cambia significato se il
codice sotto è ancora in movimento.

Va sciolta la posizione di `arrow_transform`, oggi in quarantena per una
ragione dichiarata: `libfuzzer` aborta prima dell'unwinding, quindi il target
resterebbe rosso a barriera funzionante. Due esiti ammessi, e nessuno dei due
è il silenzio: **riattivarlo**, se `apache/arrow-rs#10575` avrà reso fallibile
la conversione dello schema, oppure **riconfermare la quarantena per
iscritto**, con la ragione aggiornata alla data del rilascio.

## 4. Qualifica prestazionale

Oggi le prestazioni non sono qualificate: esistono una baseline di
riferimento, gli harness e gli esempi `bench_*`, ma **nessun gate le
consuma** e nessuna soglia è applicata da qualcosa (vedi
[`release.md`](release.md)). Finché è così, «è abbastanza veloce» è
un'opinione, non un esito.

Serve, prima della produzione:

- una **matrice fissata**: quali scenari, quali scale, quali feature — decisa
  una volta e non a ogni misura, altrimenti due campagne non si confrontano;
- un **ambiente controllato** su cui misurare, dichiarato insieme ai numeri:
  le esecuzioni note vengono da container su host di sviluppo, e la
  variabilità dell'host oggi è dentro il dato;
- **soglie esplicite**, con la regressione che le fa fallire: una soglia che
  nessuno applica è una nota, non un limite;
- il **confronto con la baseline e con la release precedente**, perché il
  numero che conta non è il valore assoluto ma la differenza.

Il lavoro non è in questo ramo e non lo anticipa: qui c'è la dichiarazione di
che cosa manca, così che nessuno legga i numeri sparsi nel codice come se
fossero un verdetto.

## 5. Release

Gate, piattaforme, packaging e procedura sono in [`release.md`](release.md).
Il bump di versione è **maggiore**: la superficie pubblica è cambiata in modo
incompatibile (rinomina del campo dei limiti, versione canonica del piano,
tipi rinominati, `plan_hash` in un dominio nuovo).

---

## Dopo la release

Non prima, e in quest'ordine.

### M3 — scheduler parallelo

Oggi l'esecuzione fra i nodi del DAG è **seriale**; il parallelismo esiste
solo dentro i kernel. M3 introduce l'esecuzione concorrente dei rami
indipendenti.

I prerequisiti già fatti: permesso atomico del governor (senza il quale
nascerebbe un TOCTOU), `BatchSequence` assegnata e propagata, contabilità
linearizzabile. Quello che manca: lo scheduler, e il consumatore che riordina
l'output dei rami paralleli secondo la sequenza logica — oggi assegnata e
testata, ma non ancora usata per riordinare, perché in esecuzione seriale
l'ordine logico coincide con quello di scansione.

**Criteri di accettazione**, oltre allo scheduler stesso:

- il property test «stesso piano, schedule forzato **seriale contro
  parallelo**, risultato semanticamente identico». È il test che dimostrerebbe
  il determinismo di livello 1 nel caso che conta, e oggi non è eseguibile
  perché manca il secondo termine del confronto;
- la `BatchSequence` **usata** per riordinare, non solo assegnata;
- nessuna regressione sul picco di memoria governata con rami concorrenti: il
  permesso è atomico, ma la contesa non è ancora stata misurata.

Un vincolo noto: `max_parallelism` dimensiona il pool globale del processo,
non del piano. Un pool per esecuzione richiederebbe un executor con stato
`Send`.

### SDK Python

**Non esiste.** Due vincoli sono già decisi per quando esisterà:

- il percorso permissivo degli input non sarà esposto: solo il profilo
  stretto, dove un input senza contratto è un errore e non una dimenticanza;
- l'inizializzazione del modulo chiamerà `install(PanicPolicy::Sanitized)` —
  installare un hook di panico è un atto esplicito dell'embedder, non un
  effetto collaterale del caricamento di una libreria;
- in-process non ci sarà un tetto duro sulla memoria; il profilo isolato sarà
  disponibile ma rinuncia allo zero-copy e attraversa IPC.

### Nuove trasformazioni

Il catalogo cresce solo dopo che le garanzie di cui sopra sono chiuse.
Aggiungere operazioni a un motore che può ancora andare in OOM senza
diagnosticarlo sposta il problema, non lo risolve.

---

## Debito dichiarato, senza data

Non blocca la release, ma è scritto perché non si perda.

| voce | dove |
|---|---|
| hasher delle chiavi non keyed: costo peggiore quadratico entro il tetto di righe | [`errori-e-limiti.md`](errori-e-limiti.md) |
| finestra TOCTOU fra pre-validazione del framing IPC e lettura | idem |
| `max_parallelism` di processo, non di piano | idem |
| `max_temp_bytes` per dominio: picco fino a ~3× | idem |
| chiavi canoniche emesse prima della ratifica normativa | idem |
| nessuna policy dell'host sui limiti dati/runtime: un piano non fidato sceglie il proprio budget | idem |
| tetti strutturali del piano applicati dopo la deserializzazione: in parse limita il solo tetto sui byte | idem |
| messaggi delle dipendenze: la riga di confine è il **percorso**, non la libreria, e nessun controllo automatico la tiene onesta | idem |
| scavenging temporaneo: l'hostname non è un'identità di macchina, e il PID è verificabile solo su Linux | idem |
| fuzzing su toolchain nightly | [`release.md`](release.md) |
| `rstar` fermo a 0.12.2: `geo` lo impone come dipendenza obbligatoria, e aggiornarlo da solo metterebbe due R-tree nello stesso binario | idem |
