# Prototipi bloccanti della fase 4 — misure ed esito

I due prototipi richiesti da [`isolamento.md`](isolamento.md) sono stati
eseguiti. Il codice sta in `prototipi/pt-linux` e `prototipi/pt-windows`:
**non** sono membri del workspace, non sono coperti dai gate e non sono
promuovibili senza una PR propria.

Ogni misura riportata qui è una riga stampata dal prototipo. Sono numeri, non
riassunti: se questo documento e l'output divergono, ha ragione l'output.

---

## 1. Esito

Ciascun prototipo doveva dimostrare **tutte e tre** le proprietà. Le ha
dimostrate.

| | nascita già vincolata | contenimento | attribuzione |
|---|---|---|---|
| **PT-Linux** | dimostrata | dimostrata | dimostrata |
| **PT-Windows** | dimostrata | dimostrata | dimostrata |

Non c'è quindi la condizione che avrebbe fermato tutto — nessuna proprietà
manca, nessun ripiego è stato usato al posto di un meccanismo assente.

**Ma il disegno cambia lo stesso.** Sette misure contraddicono ciò che il
progetto dava per scontato. Una di esse — la §4.2 — è un difetto che sarebbe
arrivato in produzione come un **successo silenzioso su un guasto di risorse**,
cioè precisamente la cosa che la fase 4 esiste per impedire.

---

## 2. Ambiente di misura

Va scritto per primo, perché ogni numero qui sotto vale **in questa
configurazione** e in nessun'altra finché non è rimisurato.

| | Linux | Windows |
|---|---|---|
| sistema | kernel `6.18.33.2-microsoft-standard-WSL2` | Windows 11 Pro `10.0.26200` |
| esecuzione | container `rust:1.98`, `--privileged --cgroupns=private` | nativa |
| toolchain | 1.98.0 | 1.98.0 msvc |
| gerarchia | cgroup v2 unificata, controller `cpuset cpu io memory hugetlb pids rdma` | Job Object |
| privilegi | `root`, mappatura identità completa | utente interattivo |
| overcommit | `vm.overcommit_memory = 1` | — |

Il kernel è quello di WSL2, non un kernel bare metal. Il profilo seccomp
predefinito di Docker **non** ha filtrato `clone3`.

---

## 3. Le misure

### 3.1 Linux

| scenario | domanda | misura |
|---|---|---|
| **S0** | `clone3` è disponibile? | sì — `size=0` risponde `EINVAL`, non `ENOSYS` |
| **S0** | `CLONE_INTO_CGROUP` è riconosciuto? | sì — descrittore invalido risponde `EBADF` |
| **S0-bis** | la gerarchia è delegabile? | sì, **dopo** aver svuotato il cgroup corrente |
| **S1** | tetto 2 MiB: il worker parte? | sì, picco 524 288 byte |
| **S1-bis** | tetto 128 KiB: il worker parte? | **no** — ucciso da `SIGKILL` prima di stampare, `oom_kill 1` |
| **S2** | il worker nasce dentro il dominio? | sì, `/proc/self/cgroup` = `0::/pt-linux/s2-nascita` |
| **S2** | il worker può creare un sotto-cgroup? | sì (ma girava come `root`) |
| **S3** | richieste non toccate da 1 GiB a 64 TiB | **tutte accettate**, nessun evento |
| **S4** | allocazione toccata oltre il tetto | `memory.peak` = `memory.max` = 67 108 864, `SIGKILL` |
| **S4** | allocazione negata? | **no**, mai |
| **S4** | attribuzione | `max +38`, `oom +1`, `oom_kill +1` in `memory.events.local` |
| **S5** | figlio che sfonda, `memory.oom.group=0` | figlio ucciso, **padre vivo, uscita 0** |
| **S6** | figlio che sfonda, `memory.oom.group=1` | intero dominio ucciso, `oom_group_kill +1`, `oom_kill +3` |

### 3.2 Windows

| scenario | domanda | misura |
|---|---|---|
| **S1** | residuo del loader con `CREATE_SUSPENDED` | **376 832 byte** impegnati prima dell'associazione |
| **S1** | il job contabilizza quel residuo? | **sì** — `PeakJobMemoryUsed` = 376 832 subito dopo l'associazione |
| **S1** | si può riprendere il processo? | sì, ma solo enumerando i thread: `std` non espone l'handle |
| **S2** | il worker nasce dentro il job? | sì, `IsProcessInJob` vero alla prima istruzione |
| **S2** | quanto costa lo spawner al dominio? | 651 264 byte, su 1 458 176 di picco del job |
| **S3** | richieste non toccate da 1 GiB a 64 TiB | **tutte negate** |
| **S3** | `PeakJobMemoryUsed` dopo le richieste negate | **70 506 457 686 016** — circa 64 TiB mai posseduti |
| **S4** | allocazione toccata oltre il tetto | fermata a 62 914 560 su un tetto di 67 108 864 |
| **S4** | terminazione o rifiuto? | **rifiuto**: `TryReserveError`, processo **vivo**, riserva piccola successiva **riuscita** |
| **S4** | attribuzione | `JOB_MEMORY_LIMIT` sulla completion port |
| **S5** | figlio del worker | eredita il job, condivide il tetto, viene fermato — **padre vivo, uscita 0** |
| **S6** | job annidato con tetto più alto | creato e assegnato con successo, **ma il tetto esterno continua a valere** |
| **S7** | tetto 256 KiB | nessun worker nasce; lo spawner muore con `STATUS_STACK_OVERFLOW`, `ABNORMAL_EXIT_PROCESS` |

---

## 4. Sette conseguenze sul disegno

### 4.1 Le due piattaforme contengono in modi opposti

Linux **termina**: nessuna allocazione è mai stata negata, il processo è stato
ucciso da `SIGKILL` con il picco esattamente al tetto. Windows **nega**: il
processo è rimasto vivo, ha ricevuto un errore di allocazione, ed è persino
riuscito a riservare un blocco piccolo subito dopo.

La formula «il processo viene fermato al tetto» descriveva una sola delle due
piattaforme, ed è stata sostituita.

### 4.2 Un tetto rispettato non basta: il capofila può dichiarare successo

È il risultato più importante, e su **entrambe** le piattaforme.

Con un figlio che sfonda il tetto, il processo capofila è sopravvissuto e ha
restituito **uscita 0** — su Linux con `memory.oom.group=0`, su Windows sempre.
Un supervisore che classificasse guardando il codice d'uscita del processo che
ha avviato leggerebbe «successo» mentre il dominio ha appena registrato un
evento di limite.

Sarebbe un successo silenzioso su un guasto di risorse, cioè esattamente ciò
che `F4-2` esiste per impedire. Le conseguenze:

- su Linux, `memory.oom.group=1` **non è un'opzione**: senza, un OOM parziale
  è indistinguibile da un successo. Con esso il dominio muore intero e il
  capofila non può più mentire;
- su Windows non esiste l'equivalente, quindi la classificazione **non può**
  fondarsi sul codice d'uscita: l'evidenza del dominio ha comunque la
  precedenza, ed è per questo che la §10.3 di [`isolamento.md`](isolamento.md)
  mette l'evidenza sopra l'esito dichiarato.

### 4.3 `PeakJobMemoryUsed` non è prova di consumo

Nello scenario delle richieste negate il contatore ha registrato **70 506 457
686 016 byte** — circa 64 TiB — per allocazioni che il sistema aveva rifiutato.
Il contatore misura le *addebitazioni tentate*, non la memoria posseduta.

Usarlo come evidenza di attribuzione produrrebbe numeri assurdi in un messaggio
d'errore. L'evidenza su Windows è **la notifica** sulla completion port; i
contatori servono alla diagnostica e vanno letti sapendo che cosa contano.

### 4.4 Su Linux il tetto non si manifesta mai come rifiuto

Richieste da 1 GiB fino a 64 TiB sono state **tutte accettate** senza generare
un solo evento, perché non toccate. Il tetto del cgroup si applica
all'addebito, che avviene alla prima scrittura sulla pagina.

Vale con `vm.overcommit_memory = 1`. Con `overcommit_memory = 2` la risposta
potrebbe essere diversa, e **non è stata misurata**. Un worker che si difendesse
con `try_reserve` non otterrebbe alcuna protezione da questo tetto: non è una
via percorribile.

### 4.5 Il residuo del loader Windows: misurato, e più piccolo del temuto

`376 832` byte impegnati prima dell'associazione al job. Il progetto lo
chiamava «piccolo e costante» prima di averlo misurato, ed era una stima
travestita da fatto: la parola è stata tolta.

Due cose il prototipo ha aggiunto:

1. il job **contabilizza** quel residuo al momento dell'associazione — il
   contatore del job riporta esattamente `376 832` subito dopo — quindi non è
   memoria invisibile al tetto.

   Con due riserve, perché la misura è indiretta: il contatore è lo stesso di
   cui la §4.3 dice che non è affidabile come prova di consumo, e **non** è
   stato provato il caso in cui il residuo *superi* il tetto, cioè se in quel
   caso l'associazione fallisca o riesca lasciando il dominio già oltre;
2. la strada dello **spawner** lo azzera per il worker — il worker nasce già
   dentro il job — al prezzo di lasciare vivo un processo intermedio che costa
   `651 264` byte del dominio.

Il residuo non sparisce: si sposta da «non coperto» a «coperto ma consumato».
La seconda forma è preferibile perché è governabile.

### 4.6 Serve un pavimento dichiarato per `max_domain_memory_bytes`

Sotto una certa soglia il dominio non è vitale, e i due sistemi lo dicono in
modi diversi e sgradevoli:

| | tetto | esito |
|---|---|---|
| Linux | 128 KiB | `SIGKILL` prima che il worker stampi una riga |
| Windows | 256 KiB | `STATUS_STACK_OVERFLOW`, nessun worker nato |

Entrambi restano **attribuibili** — l'evento del dominio c'è in tutti e due i
casi — ma un piano che chiede un tetto sotto il pavimento non merita di
arrivare fin lì: va respinto in validazione, con un errore che dice qual è il
pavimento. Un `STATUS_STACK_OVERFLOW` è una risposta pessima a un errore di
configurazione.

### 4.7 `clone3` c'è, ma su Linux non serve

`clone3` è disponibile e `CLONE_INTO_CGROUP` è riconosciuto, anche sotto il
profilo seccomp predefinito di Docker. Richiedono però una `syscall` grezza,
cioè `unsafe`.

Esiste una strada **interamente sicura** che dà la stessa proprietà:

1. il processo intermedio scrive il proprio identificativo in `cgroup.procs` —
   una scrittura su file;
2. poi si sostituisce con l'immagine del worker via `exec`, che in Rust è una
   funzione sicura.

Il worker così non esiste **mai** fuori dal dominio: la sua immagine viene
mappata quando il processo è già dentro. La misura di `S1-bis` lo conferma dal
lato opposto — con un tetto di 128 KiB il worker è stato ucciso *prima* di
riuscire a stampare, quindi il tetto era in vigore già durante il caricamento.

---

## 5. Fattibilità senza `unsafe`

Era una delle domande, ed è quella con la risposta più netta e più asimmetrica.

| | serve `unsafe`? | superficie |
|---|---|---|
| **Linux** | **no** | scrittura su `cgroup.procs`, `exec`, lettura di `memory.events.local`, `memory.max`, `memory.peak`. Tutto `std` e tutto sicuro |
| **Windows** | **sì, inevitabilmente** | dieci funzioni FFI: creazione e apertura del job, impostazione e interrogazione dei limiti, associazione del processo, associazione della porta, prelievo dei messaggi, misura del commit, e — solo sulla strada `CREATE_SUSPENDED` — enumerazione dei thread per riprendere il processo |

Su Windows non c'è alcuna via sicura: i Job Object sono superficie del sistema
operativo e nient'altro. Le opzioni sono due, e vanno decise prima di
scrivere il dominio Windows:

- una **deroga esplicita** in [`errori-e-limiti.md`](errori-e-limiti.md), con
  regola, perimetro, pericolo e condizione di rientro, che confini l'`unsafe` a
  un modulo unico e minuscolo;
- una **dipendenza vettata** che assorba la FFI, con il costo di dipendere da
  qualcun altro per una garanzia di sicurezza.

La strada dello spawner riduce comunque la superficie: elimina l'enumerazione
dei thread e la ripresa del processo, che sono la parte più sgradevole.

---

## 6. Nessun ripiego

Va detto perché era una condizione, non perché sia successo qualcosa:

- nessun meccanismo assente è stato sostituito da un altro;
- dove qualcosa non funzionava, il prototipo si è fermato e l'ha stampato — la
  prima esecuzione ha fallito la delega con `EBUSY` su tutti gli scenari, e non
  ha proseguito su una strada diversa;
- nessuna proprietà è stata dedotta: ogni «sì» in questo documento ha una
  misura accanto.

Due errori del prototipo stesso sono stati trovati e corretti prima di
fidarsi dei numeri: la costante di `CLONE_INTO_CGROUP` era sbagliata, e il
descrittore fasullo del probe superava `INT_MAX`, che il kernel respinge
**prima** di guardare il flag. Entrambi avrebbero fatto concludere «flag non
riconosciuto» quando invece lo è.

---

## 7. Che cosa **non** è stato dimostrato

La parte che conta quanto la §1, perché ogni riga qui è una promessa che non
possiamo ancora fare.

| | stato |
|---|---|
| macOS | nulla è stato prototipato. Resta non supportato |
| Linux bare metal | non misurato: il kernel era quello di WSL2 |
| `overcommit_memory = 2` | non misurato. La §4.4 vale per la politica permissiva |
| delega a utente non privilegiato | non misurata: il prototipo girava come `root`. La delega vera — cgroup di proprietà di un utente comune — è un'altra cosa |
| container con `/sys/fs/cgroup` in sola lettura | non misurato, ed è la configurazione **predefinita** di Docker: il prototipo ha richiesto privilegi e namespace privato |
| supervisore già dentro un job con tetto inferiore | non misurato. È stato misurato il caso vicino — il worker che si annida — ma non questo |
| residuo del loader **maggiore** del tetto | non misurato: non si sa se l'associazione fallisca o riesca lasciando il dominio già oltre |
| più domini concorrenti | non misurato: gli scenari sono sequenziali |
| costo in memoria della verifica in streaming | non misurato. È l'oggetto della `PR-6`, non di questi prototipi |
| durata sotto carico reale | non misurata: i carichi sono sintetici e brevi |

La riga sui container in sola lettura è quella che pesa di più
operativamente: significa che il profilo isolato, così com'è, **non gira** in
un container Docker predefinito. È una limitazione da dichiarare, non da
scoprire.
