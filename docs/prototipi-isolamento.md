# Prototipi di isolamento della fase 4 — misure ed esito

I prototipi stanno in `prototipi/pt-linux` e `prototipi/pt-windows`: **non**
sono membri del workspace, non sono coperti dai gate e non sono promuovibili
senza una PR propria. Gli output grezzi stanno in `prototipi/misure/`.

Sono **evidenza esplorativa**, prodotta in quattro cicli. Il primo concluse
che le tre proprietà erano dimostrate; il secondo mostrò che era prematuro; il
terzo chiuse le domande su quiescenza e uccidibilità; il quarto ha trovato una
combinazione di evidenza che la classificazione dichiarava esaustiva senza
esserlo, e ha mostrato che il sigillo del dominio si disfa dall'interno quando
worker e supervisore condividono i privilegi. Dove questo documento e l'output
grezzo divergono, ha ragione l'output.

---

## 1. Come si riproducono

Non è un dettaglio di cortesia: senza i comandi esatti, un numero riportato
qui non è verificabile da nessun altro.

**PT-Linux** — richiede privilegi e namespace di cgroup privato:

```sh
docker run --rm --privileged --cgroupns=private \
  -v <repo>/prototipi:/proto \
  -w /proto/pt-linux -e CARGO_TARGET_DIR=/tmp/t rust:1.98 \
  bash -c "cargo build -q; useradd -u 1000 -m worker; /tmp/t/debug/pt-linux"
```

L'utente `worker` serve allo scenario `L9`: senza, la prova sui privilegi
ceduti non è eseguibile.

**PT-Windows** — nativo, nessun privilegio particolare:

```powershell
cd prototipi\pt-windows
cargo run
```

Ogni misura è una riga `MISURA <chiave> <valore>`. Le chiavi con `n=`, `min=`,
`max=` sono ripetizioni; le altre sono campioni singoli e vanno lette come
tali.

---

## 2. Ambiente di misura

Ogni numero vale **in questa configurazione** e in nessun'altra finché non è
rimisurato.

| | Linux | Windows |
|---|---|---|
| sistema | kernel `6.18.33.2-microsoft-standard-WSL2` | Windows 11 Pro `10.0.26200` |
| esecuzione | container `rust:1.98`, `--privileged --cgroupns=private` | nativa |
| toolchain | 1.98.0 | 1.98.0 msvc |
| gerarchia | cgroup v2 unificata, controller `cpuset cpu io memory hugetlb pids rdma` | Job Object |
| privilegi | `root`, tranne dove indicato | utente interattivo |
| overcommit | `vm.overcommit_memory = 1` | — |

Il kernel è quello di WSL2, non bare metal. Il profilo seccomp predefinito di
Docker **non** ha filtrato `clone3`.

---

## 3. Esito, per proprietà

| | nascita vincolata | contenimento | **attribuzione** |
|---|---|---|---|
| **PT-Linux** | dimostrata | dimostrata (nel senso della semantica di `memory.max`, non come massimo istantaneo) | dimostrata **sotto quattro condizioni verificabili**: separazione dei privilegi, dominio sigillato, `oom_score_adj` normalizzato, quiescenza letta da `cgroup.events` (§5.1, §5.4, §5.5, §5.6) |
| **PT-Windows** | dimostrata | dimostrata | **no** — e la piattaforma è non supportata (§5.2, §6) |

Le prime due colonne reggono ovunque. La terza regge su Linux **solo se il
supervisore verifica ciò che ha configurato**, e non regge affatto su Windows.

Le tre condizioni Linux non sono raccomandazioni: ciascuna ha uno scenario che
mostra che cosa succede senza. Sono diventate il preflight della §9-bis di
[`isolamento.md`](isolamento.md).

---

## 4. Le misure

### 4.1 Linux

| scenario | domanda | misura |
|---|---|---|
| `S0` | `clone3` disponibile? | sì — `size=0` risponde `EINVAL`, non `ENOSYS` |
| `S0` | `CLONE_INTO_CGROUP` riconosciuto? | sì — descrittore invalido risponde `EBADF` |
| `S0-bis` | gerarchia delegabile? | sì, **dopo** aver svuotato il cgroup corrente |
| `S1-bis` | tetto 128 KiB | worker ucciso da `SIGKILL` prima di stampare |
| `S2` | il worker nasce dentro? | sì, `/proc/self/cgroup` = il dominio |
| `S3` | richieste non toccate, da 1 GiB a 64 TiB | **tutte accettate**, nessun evento |
| `S4` | allocazione toccata oltre il tetto | `memory.peak` = `memory.max`, `SIGKILL`, mai un rifiuto |
| `S5` | figlio che sfonda, `memory.oom.group=0` | figlio ucciso, **padre vivo, uscita 0** |
| `S6` | figlio che sfonda, `memory.oom.group=1` | dominio ucciso intero, `oom_group_kill +1` |
| `L7` | `cgroup.max.depth=0` regge? | sì — il `mkdir` del worker fallisce con `EAGAIN` |
| `L8` | sottogruppo, via ingenua | `mkdir` riesce, **delega del controller rifiutata** con `EBUSY`; l'evidenza resta locale |
| `L9` | worker `uid 1000`, cartella di root | parte regolarmente, `mkdir` rifiutato con `EACCES` |
| `L10` | quiescenza (§5.3) | alla `wait`: **1 processo vivo, `oom_kill` = 0**; dopo 200 ms: `oom_kill` = 1 |
| `L12` | sottogruppo, via **non** ingenua | crea, si sposta, delega: **tutti e tre riescono** |
| `L12` | evidenza dopo l'evasione | `local.oom_kill` **0**, `gerarchico.oom_kill` **1** |
| `L13` | la stessa evasione, dominio sigillato | fermata al primo passo, `EAGAIN` |
| `L11` | picco d'avvio | **262 144 – 524 288** byte (`n=5`) |
| `L11` | picco sotto tetto, `n=5` | **67 108 864** in tutte e cinque — esattamente il tetto |
| `L14` | quiescenza, orfano nello **stesso** cgroup | `cgroup.procs` e `populated` concordano; `populated` a 0 dopo 175 ms |
| `L15` | quiescenza, processo vivo in un **sottogruppo** | `cgroup.procs` = **0**, `populated` = **1**: il file è cieco, il kernel no |
| `L16a` | `oom_score_adj = -1000` ereditato, `oom.group=1` | worker **non ucciso**: `oom` +305, `oom_kill` **0**, servito `cgroup.kill` |
| `L16b` | lo stesso, con normalizzazione a 0 | `oom` +1, `oom_kill` +2, `oom_group_kill` +1, nessun `cgroup.kill` |
| `L17` | preflight: scrivi e rileggi | `memory.max`, `memory.oom.group`, `cgroup.max.depth`, `memory.swap.max` — tutti concordi |
| `L17` | opzione `memory_localevents` | **assente** nella gerarchia provata |
| `L18` | padre a 32 MiB, dominio a 256 MiB | il worker muore per il tetto **del padre**: `Ol` **0**, `Kl` **1**, `Kh` **1** |
| `L18` | dove sta l'evidenza | il padre registra `oom` 1 e `oom_kill` **0**; il dominio il contrario |
| `L18` | picco del padre contro il suo tetto | `33 554 432` contro `33 554 432`: **superamento zero** |
| `L18` | picco del dominio contro il picco del padre | `33 787 904` contro `33 554 432`: il figlio supera il padre che lo contiene |
| `L19a` | worker con lo **stesso** UID del supervisore | riscrive `max.depth`, `memory.max`, `oom.group`, `swap.max` **ed esce dal dominio**: 5 su 5 |
| `L19b` | worker a `Uid: 1000` | tutte e cinque rifiutate con `EACCES` |

### 4.2 Windows

| scenario | domanda | misura |
|---|---|---|
| `S3` | richieste non toccate | **tutte negate** |
| `S4` | allocazione toccata oltre il tetto | fermata a 62 914 560 su 67 108 864; **rifiuto**, processo vivo |
| `S6` | job annidato con tetto più alto | creato e assegnato, **ma il tetto esterno continua a valere** |
| `S7` | tetto 256 KiB | nessun worker nasce, `STATUS_STACK_OVERFLOW` |
| `W1` | capofila a 0, figlio sfonda, porta **mai** drenata | `ViolationLimitFlags` = `0x200` — la prova c'è |
| `W2` | **nessuna** completion port associata | `ViolationLimitFlags` = `0x200` — la prova non dipende dalla porta |
| `W3` | quiescenza | alla `wait` violazione **visibile**; dopo l'attesa **sparita** |
| `W4` | supervisore già dentro un job esterno | contenimento e prova reggono entrambi |
| `W6` | quanto dura la prova interrogabile | **25 ms** |
| `W7` | picco del job come prova durevole | vedi §5.2 |
| `W5` | residuo del loader | **376 832 – 389 120** byte (`n=5`) |
| `W5` | commit dello spawner | **634 880 – 647 168** byte (`n=5`) |

**Come sono stati ottenuti questi numeri, e perché conta.** Tre stesure di
questo documento hanno riportato tre intervalli diversi per il residuo del
loader, e tutte e tre erano sbagliate. Il meccanismo era sempre lo stesso:
scrivevo il riassunto guardando la console, poi rieseguivo il prototipo per
aggiungere uno scenario, e il grezzo cambiava sotto il testo. Non era
disattenzione da correggere con più attenzione: era un procedimento che non
poteva funzionare.

I valori riportati qui sono ora **estratti dai file di `prototipi/misure/`**
dopo l'ultima esecuzione, con un controllo che rifiuta le chiavi presenti con
valori diversi in grezzi diversi. Se un numero di questo documento non si
trova in quei file, è il documento a essere sbagliato.

---

## 5. Che cosa i cicli hanno cambiato

### 5.1 Linux — `memory.events.local` è cieco, e si può renderlo irrilevante

Il primo ciclo leggeva `memory.events.local` perché non aggrega i discendenti,
e attribuire a un worker un evento generato altrove sarebbe inventare. Il
ragionamento resta giusto; la conclusione era incompleta.

`L8` sembrava rassicurante: il worker crea il sottogruppo ma **non** riesce a
delegargli il controller della memoria — `EBUSY`, la regola dei processi
interni — quindi le cariche restano al dominio e l'evidenza si vede. Fermarsi
lì sarebbe stato comodo e sbagliato: quella regola guarda **dove stanno i
processi, non chi sono**.

`L12` prova la sequenza intera:

| passo | esito |
|---|---|
| 1. crea il sottogruppo | riuscito |
| 2. **ci sposta se stesso** — ora il dominio non ha più processi propri | riuscito |
| 3. delega `+memory` al dominio | **riuscita** |
| 4. sfonda, da dentro il sottogruppo | ucciso |

Con questo esito:

```
    delta.local.oom_kill        0     <- il supervisore non vede nulla
    delta.gerarchico.oom_kill   1
```

Il supervisore **non resta cieco**: `local.oom` è salito a 1, perché il
limite raggiunto è quello del dominio. Ciò che perde è il **luogo**
dell'uccisione, non il fatto che il limite sia stato toccato.

La distinzione conta, e la prima stesura di questa sezione la mancava
scrivendo che il supervisore non vedeva nulla. Le due letture portano a due
conclusioni opposte:

| che cosa legge il classificatore | che cosa conclude |
|---|---|
| il solo `oom_kill` locale | «nessuna uccisione qui» → `Internal`, cioè un difetto nostro invece di un limite raggiunto |
| `oom` locale **e** `oom_kill` gerarchico | «il mio limite è stato raggiunto, l'uccisione è avvenuta sotto» → limite reale, attribuzione perduta |

È per questo che l'evidenza è una **struttura** e non un booleano: nessuno dei
due contatori, da solo, distingue le situazioni che contano. La tabella
completa delle combinazioni è in
[`isolamento.md`](isolamento.md).

Resta però che l'attribuzione — *a chi* è successo — si perde, e con essa la
possibilità di dire al chiamante qualcosa di più preciso di «un discendente ha
sfondato». Per questo il sigillo del dominio è un requisito e non un
irrobustimento.

**Un ragionamento che non è stato misurato.** Se il worker desse al
sottogruppo un `memory.max` proprio e più basso, il limite raggiunto sarebbe
quello del figlio e anche `local.oom` resterebbe a zero. È plausibile e
segue dalla semantica dei contatori, ma **non è stato provato**: lo scenario
provato delega il controller senza impostare un tetto interno.

**La difesa è rendere l'evasione impossibile, non inseguirla.** Tre strati,
misurati:

| strato | misura | effetto |
|---|---|---|
| `cgroup.max.depth = 0` | `L7`, `L13` | il kernel rifiuta ogni discendente, `EAGAIN`, e l'evasione si ferma al primo passo |
| worker non privilegiato | `L9` | `mkdir` rifiutato con `EACCES`, indipendentemente dal kernel |
| `memory.events` gerarchico | `L12` | se i primi due mancassero, l'evidenza c'è comunque |

Non è un *oppure*: il terzo strato non sostituisce i primi due, perché
l'evidenza gerarchica dice *che* è successo, non *a chi*. Serve a non perdere
l'informazione, non a mantenere l'attribuzione.

### 5.2 Windows — nessuna fonte singola è sufficiente

Il primo ciclo si fondava sulla notifica alla completion port. La review ha
osservato che Microsoft non la garantisce sempre. Il secondo ciclo ha cercato
un'alternativa interrogabile e ne ha trovate due, ciascuna con un difetto
diverso.

| fonte | c'è sempre? | sopravvive alla quiescenza? | falsi positivi |
|---|---|---|---|
| notifica sulla completion port | **no** — non garantita | non pertinente (è push) | no |
| `JobObjectLimitViolationInformation` | sì: `W1` la trova senza aver mai drenato la porta, `W2` senza porta del tutto, `W4` con job ereditato | **no — 25 ms** (`W6`) | no |
| `PeakJobMemoryUsed >= JobMemoryLimit` | sì | **sì** — identico a 500 ms (`W7`) | **sì** |

`W6` è la misura che decide: la violazione interrogabile è visibile al primo
campione e **sparita entro 25 ms**. È un *livello*, non un *fermo*. E la
barriera di quiescenza richiede di aspettare: i due requisiti sono in tensione
diretta, e un disegno che interroghi il sistema dopo l'attesa corre contro un
orologio che non controlla.

`W7` misura il costo della terza fonte:

| carico | picco | ≥ tetto | verdetto |
|---|---|---|---|
| sfonda toccando | 68 706 304 | sì | corretto |
| **figlio che sfonda, capofila a 0** | 69 341 184 | sì | corretto — è il caso della review |
| resta sotto il tetto | 1 437 696 | no | corretto |
| **chiede 64 TiB e accetta il rifiuto** | 70 506 457 706 496 | sì | **falso positivo** |

Il picco conta gli addebiti **tentati**, non la memoria posseduta: un worker
che chiede molto e gestisce il rifiuto risulta aver sfondato senza averlo
fatto.

**Che cosa ne segue.** Nessuna delle tre basta da sola, e la combinazione non
produce attribuzione infallibile: produce evidenza a strati con un
comportamento residuo dichiarato.

1. Il supervisore **drena la porta e interroga la violazione durante
   l'esecuzione**, non alla fine, e accumula ciò che vede in un **fermo
   proprio** — durevole perché è nostro, non del sistema operativo.
2. Alla barriera legge il **proprio** fermo, mai il sistema operativo: quello
   è già scaduto.
3. Il picco durevole resta come **rete**, e può solo spingere verso il *non
   pubblicare*.
4. Il falso positivo del picco impone un **obbligo al worker**: non chiedere
   mai più di quanto intende usare. È un vincolo dichiarabile e verificabile,
   non una speranza.

Il rischio residuo cambia direzione ma non sparisce: diventa **rifiutare un
risultato valido**, non pubblicarne uno incompleto. Per una libreria
safety-critical è il verso giusto — e va detto, non lasciato scoprire.

### 5.3 La barriera di quiescenza è necessaria, misurata

`L10` la dimostra invece di argomentarla:

| | |
|---|---|
| codice d'uscita del capofila | `0` |
| processi vivi al ritorno della `wait` | **1** |
| `oom_kill` visibile in quell'istante | **0** |
| dopo 200 ms di attesa della quiescenza | `oom_kill` = **1** |

Il capofila esce pulito, un orfano resta vivo, e l'evidenza arriva **dopo**.
Pubblicare al ritorno della `wait` avrebbe pubblicato con un OOM in volo.
Chiudere la coda degli eventi non basta: la chiusura dimostra che non
accettiamo più eventi, non che non ne stiano arrivando.

Su Windows `W3` mostra la stessa struttura — un processo attivo al ritorno
della `wait` — ma con l'aggravante della §5.2: lì aspettare la quiescenza fa
**sparire** la prova.

---

### 5.4 La quiescenza non si legge da `cgroup.procs`

Il secondo ciclo attendeva il dominio vuoto camminando ricorsivamente le
directory. Funzionava, ma è una lettura **non atomica** di molti file mentre i
processi si spostano: una corsa mascherata da controllo.

Il segnale giusto è `cgroup.events`, campo `populated`, che il kernel
garantisce valere 1 finché il cgroup **o un suo discendente** contiene
processi vivi. `L14` è il controllo — con l'orfano nello stesso cgroup i due
segnali concordano — e `L15` è la prova:

```
    cgroup.procs alla wait          0     <- il dominio sembra vuoto
    scansione ricorsiva             1
    cgroup.events populated         1     <- il kernel sa che non lo e'
```

Un processo del dominio vivo un livello sotto, e il file che la barriera
leggeva dice zero.

**Un errore dello scenario, che vale la pena raccontare.** Al primo tentativo
`L15` non riprodusse nulla: `cgroup.procs` valeva 1, e sembrava che il
problema non esistesse. Il motivo era che il nipote non aveva ancora finito di
spostarsi nel sottogruppo quando il capofila usciva — quindi il file lo vedeva
ancora, per caso e non per correttezza. Era la stessa corsa che lo scenario
doveva misurare, osservata dal vivo e nel verso che non produce un allarme.
Ora il capofila attende che il nipote sia **insediato** e lo scenario dichiara
se la misura è valida.

### 5.5 `memory.oom.group` ha un'eccezione, ed è ereditaria

Il kernel non uccide i task con `oom_score_adj = -1000`, **nemmeno con il
group kill attivo**. È un valore che si eredita: un worker avviato da un
chiamante protetto — un servizio di sistema, un container con priorità — lo
riceve senza che nessuno lo scriva.

`L16` misura i due bracci:

| | `oom_score_adj` | `oom` | `oom_kill` | `oom_group_kill` | esito |
|---|---|---|---|---|---|
| senza normalizzazione | `-1000` | **+305** | **0** | 0 | il dominio **non si è spento**: è servito `cgroup.kill` |
| con normalizzazione | `0` | +1 | +2 | +1 | fermato dal group kill, come atteso |

Il risultato è peggiore della semplice sopravvivenza. Trecentocinque
invocazioni dell'OOM e nessuna uccisione significa un dominio che ha raggiunto
il limite centinaia di volte **e non avanza**: non un worker che continua a
lavorare, ma un'esecuzione bloccata. Il primo tentativo di questo scenario si
è infatti impiantato, ed è stato l'impianto a rivelarlo.

Ne seguono due cose, entrambe nel progetto:

1. lo spawner scrive `oom_score_adj = 0` prima della `exec`, lo **rilegge**, e
   fallisce chiuso se la rilettura non torna;
2. il supervisore ha bisogno di `cgroup.kill`, che manda `SIGKILL` a tutti i
   processi del cgroup **senza consultare** `oom_score_adj`. È l'unica via
   d'uscita quando il worker è protetto.

E una terza sull'evidenza: `oom` alto con `oom_kill` a zero è una **firma
diagnostica**, non un non-evento. Leggere solo `oom_kill` avrebbe detto
«nessun problema di risorse» su un dominio fermo.

---

### 5.6 Il sigillo si disfa dall'interno, se i privilegi sono gli stessi

Il terzo ciclo aveva presentato il worker non privilegiato come **uno di tre
strati**, dimostrandolo con un supervisore `root` che abbassava il worker a
`Uid: 1000`. La dimostrazione era vera e la conclusione sbagliata: non provava
il caso normale di una libreria embedded, dove coordinatore e worker girano
**con lo stesso UID**.

`L19` prova quel caso. Dopo che il preflight ha stabilito tutto, il worker:

| operazione | `Uid: 0` (stesso del supervisore) | `Uid: 1000` |
|---|---|---|
| `cgroup.max.depth` → 10 | **riuscita** | `EACCES` |
| `memory.max` → 1 GiB | **riuscita** | `EACCES` |
| `memory.oom.group` → 0 | **riuscita** | `EACCES` |
| `memory.swap.max` → `max` | **riuscita** | `EACCES` |
| uscire dal dominio scrivendosi altrove | **riuscita** | `EACCES` |

Cinque su cinque. Alla fine dello scenario il dominio aveva
`cgroup.max.depth = 10`, `memory.max = 1073741824`, `memory.oom.group = 0` — e
il worker non era più dentro.

Non è un worker ostile: chiama le stesse API del supervisore perché ne ha gli
stessi diritti. `NG-7` dice che il modello di minaccia è il guasto, ma un
guasto che riscrive `memory.max` produce l'effetto di un attacco.

**Ne segue che la separazione dei privilegi non è uno strato: è il
meccanismo.** Senza, il preflight verifica uno stato che dura fino
all'istruzione successiva. Le forme accettabili — e il rifiuto quando nessuna
è disponibile — sono in [`isolamento.md`](isolamento.md).

### 5.7 `oom_kill` non dice quale limite è stato raggiunto

`oom_kill` conta i processi del cgroup uccisi da **qualunque** OOM killer, non
solo dal proprio limite. Un antenato con un tetto più basso uccide quindi il
worker senza che il tetto del dominio venga mai raggiunto.

`L18` lo riproduce: padre a 32 MiB, dominio a 256 MiB.

```
    dominio  Ol = 0    Kl = 1    Kh = 1     picco = 33 787 904
    padre    oom = 1   oom_kill = 0
```

Il picco del dominio si ferma al tetto **del padre**, che è la conferma che
il proprio non è mai entrato in gioco. E l'evidenza è **divisa fra due
livelli**: il padre sa *perché*, il dominio sa *chi*. Nessuno dei due, da
solo, racconta la storia.

Questa combinazione — `Ol = 0` con `Kl ≥ 1` — era assente dalla tabella di
classificazione, che si dichiarava esaustiva. La correzione, e la regola più
generale che ne segue — i delta sono contatori su un intervallo e **non
dimostrano una causa** — sono in [`isolamento.md`](isolamento.md).

---

### 5.8 I picchi non misurano la memoria posseduta

La review ha ipotizzato che `L18` avesse osservato un superamento temporaneo
del tetto: il picco del dominio (`33 787 904`) supera il tetto del padre
(`33 554 432`) di `233 472` byte, e poiché il dominio è contenuto nel padre,
il padre doveva averlo superato.

L'inferenza è ragionevole e la misura diretta la smentisce. Letto il picco del
padre — cosa che nessuno aveva fatto, ed è la ragione per cui l'affermazione
«mai osservato» era senza prove:

```
    padre   memory.max    33 554 432
    padre   memory.peak   33 554 432     superamento: 0
    dominio memory.peak   33 787 904     233 472 oltre il picco del padre
```

Il padre non ha superato il proprio tetto. Ma il **figlio** ha un picco
maggiore del **padre che lo contiene**, e questo sotto una lettura ingenua
della gerarchia non può accadere.

La spiegazione plausibile — ipotesi, non misura — è che l'addebito passi al
livello del figlio, che ha un tetto molto più alto, venga registrato nel suo
picco, e sia poi respinto più in alto dal tetto del padre. Il picco del figlio
conterebbe allora un addebito **tentato e mai posseduto**: la stessa classe di
difetto che il ciclo su Windows aveva trovato in `PeakJobMemoryUsed`, su
un'altra piattaforma e con un altro nome.

Ne segue una regola, e vale per entrambe le piattaforme: **i valori di picco
non sono evidenza di memoria posseduta**, e non vanno usati per ragionare sul
consumo di un antenato. Servono al dimensionamento, con l'incertezza
dichiarata — anche i picchi riportati nella §4 di questo documento.

---

## 6. Fattibilità senza `unsafe`

| | serve `unsafe`? | superficie |
|---|---|---|
| **Linux** | **no** | scrittura su `cgroup.procs`, `exec`, `uid`/`gid`, lettura di `memory.events`, `memory.max`, `memory.peak`, scrittura di `cgroup.max.depth`. Tutto `std`, tutto sicuro |
| **Windows** | **sì** | dodici funzioni FFI: creazione, apertura, tetto, limite di notifica, interrogazione dei limiti, delle violazioni e della contabilità, associazione del processo e della porta, prelievo dei messaggi, misura del commit, enumerazione dei thread |

`AGENTS.md` proibisce `unsafe` nel workspace come **regola permanente**. Una
deroga non è quindi una strada disponibile in una normale PR della fase 4:
cambierebbe la governance, ed è una decisione del maintainer.

Restano due strade, e la prima è stata verificata invece che immaginata.

**La dipendenza sicura, oggi, non esiste.** `win32job` 2.0.3 — l'unico
involucro sicuro dei Job Object in circolazione — espone `create`,
`assign_process`, `query_extended_limit_info`, `query_process_id_list` e i
limiti di working set. Non espone:

- **`JOB_OBJECT_LIMIT_JOB_MEMORY`**, cioè il tetto stesso: l'unico limite di
  memoria che offre è `JOB_OBJECT_LIMIT_WORKINGSET`, che è un'altra cosa;
- l'associazione a una completion port;
- i limiti di notifica;
- le informazioni sulle violazioni — cioè la fonte della §5.2.

Non copre né il contenimento né l'attribuzione. Il campo della struttura
estesa è `pub(crate)`, quindi non si aggira: si può solo estendere il crate a
monte.

**La conclusione onesta è quindi la seconda strada:** finché non esiste una
dipendenza vettata che copra tetto ed evidenza, il profilo isolato su Windows
è **non supportato**, accanto a macOS. Non è un ripiego — è ciò che le regole
del progetto impongono quando l'unica implementazione richiederebbe di
violarle.

---

## 7. Nessun ripiego

- nessun meccanismo assente è stato sostituito da un altro;
- dove qualcosa non funzionava, il prototipo si è fermato e l'ha stampato;
- ogni «sì» in questo documento ha una misura accanto.

Errori del prototipo trovati e corretti prima di fidarsi dei numeri:

| errore | conseguenza se non trovato |
|---|---|
| costante di `CLONE_INTO_CGROUP` sbagliata | «flag non riconosciuto» quando invece lo è |
| descrittore del probe oltre `INT_MAX` | il kernel respinge prima di guardare il flag: stessa conclusione falsa |
| `L8` fermato al primo rifiuto | «l'evasione è impossibile» quando serve un passo in più |
| scenario `inerte` assente in PT-Linux | il «picco d'avvio» avrebbe riportato il tetto del dominio |
| riassunto e grezzo da esecuzioni diverse | un campione singolo presentato come *il* valore |

---

## 8. Che cosa **non** è stato dimostrato

Vale quanto la §3: ogni riga è una promessa che non possiamo fare.

| | stato |
|---|---|
| **attribuzione infallibile su Windows** | **no** — §5.2. Nessuna fonte singola basta e la combinazione lascia un falso positivo dichiarato |
| macOS | nulla è stato prototipato. Resta non supportato |
| Linux bare metal | non misurato: il kernel era quello di WSL2 |
| `overcommit_memory = 2` | non misurato |
| delega a utente non privilegiato | **parzialmente** misurata: `L9` mostra che il worker `uid 1000` non tocca il dominio. Non è stata provata una gerarchia *posseduta* da un utente comune |
| container con `/sys/fs/cgroup` in sola lettura | non misurato, ed è il **default** di Docker: il prototipo ha richiesto privilegi e namespace privato |
| residuo del loader maggiore del tetto | non misurato |
| assenza di falsi negativi del picco su Windows | **non dimostrata**: i casi provati sono stati rilevati, il che non è la stessa cosa. Non si può quindi affermare che il rischio residuo sia solo «rifiutare output validi» |
| gerarchia montata con `memory_localevents` | **non misurata**: la gerarchia provata non aveva l'opzione. Che con il dominio sigillato sia irrilevante è un ragionamento, non un'osservazione |
| `oom_score_adj` non normalizzabile | non misurato: il caso in cui la scrittura fallisca per mancanza di privilegi non è stato provato, solo previsto |
| sottogruppo con un `memory.max` proprio | **non misurato**: che in quel caso anche `local.oom` resti a zero è un ragionamento sulla semantica dei contatori, non un'osservazione |
| kernel diversi dal 6.18 | non misurati. `cgroup.max.depth`, `cgroup.kill` e `memory.events.local` hanno storie di introduzione diverse |
| separazione dei privilegi in un caso embedded reale | **non misurata**: `L19b` mostra che un UID distinto basta, non che il coordinatore possa sempre ottenerlo. Helper del control plane e mount namespace non sono stati prototipati |
| ordine causale degli eventi | **non osservabile** con i contatori: sono aggregati su un intervallo. Un registro ordinato via notifica non è stato prototipato |
| superamento temporaneo di `memory.max` | **non osservato** dalla misura diretta — il picco dell'antenato coincide col suo tetto — ma i contatori sono fra loro incoerenti (§5.8), quindi non è nemmeno escluso |
| perché il picco del figlio superi quello del padre | **non spiegato**: l'ipotesi dell'addebito tentato e respinto più in alto è un ragionamento, non una misura |
| finestra fra `rename` e ritorno dell'esito | non misurata: è una finestra logica, non una che si possa osservare dall'interno del processo che muore |
| più domini concorrenti | non misurato: gli scenari sono sequenziali |
| costo in memoria della verifica in streaming | non misurato |
| durata sotto carico reale | non misurata: i carichi sono sintetici e brevi |

La riga sui container in sola lettura pesa di più operativamente: il profilo
isolato, così com'è, **non gira** in un container Docker predefinito.
